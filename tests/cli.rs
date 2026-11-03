use assert_cmd::Command;
use predicates::prelude::*;
use std::fs;
use std::path::PathBuf;
use tempfile::TempDir;

/// A throwaway mongor project: a temporary directory with a `mongor.toml`
/// pointing at its own database, so tests can run in parallel.
struct Project {
    dir: TempDir,
    database: String,
}

impl Project {
    fn new(database: &str) -> Project {
        let dir = TempDir::new().unwrap();
        fs::create_dir(dir.path().join("migrations")).unwrap();

        fs::write(
            dir.path().join("mongor.toml"),
            format!(
                r#"[migrations]
path = "migrations"
collection = "mongor_migrations"

[environments.test]
uri_env = "MONGOR_TEST_URI"
database = "{database}"
"#
            ),
        )
        .unwrap();

        let project = Project {
            dir,
            database: database.to_string(),
        };

        // Rows left by a previous run would make `migrate` report nothing to do,
        // so every test starts from an empty database.
        project.eval("db.dropDatabase()");
        project
    }

    fn mongor(&self) -> Command {
        let mut command = Command::cargo_bin("mongor").unwrap();
        command
            .current_dir(self.dir.path())
            .env("MONGOR_TEST_URI", uri());
        command
    }

    fn path(&self, file_name: &str) -> PathBuf {
        self.dir.path().join("migrations").join(file_name)
    }

    fn write(&self, file_name: &str, body: &str) {
        fs::write(self.path(file_name), body).unwrap();
    }

    /// The recorded status of one migration, or `None` if there is no row.
    fn recorded_status(&self, version: u32) -> Option<String> {
        // Printed only when the row exists: `print(undefined)` would be an empty
        // line, indistinguishable from a row whose status is missing.
        let status = self.eval(&format!(
            "const row = db.mongor_migrations.findOne({{ version: {version} }}); if (row) print(row.status);"
        ));

        (!status.is_empty()).then_some(status)
    }

    /// Runs JavaScript against this project's database and returns stdout.
    ///
    /// Shells out to mongosh rather than using the Rust driver: the driver is
    /// async, and a one-line query is not worth a tokio runtime per test.
    fn eval(&self, js: &str) -> String {
        let output = std::process::Command::new("mongosh")
            .arg(uri())
            .args(["--quiet", "--eval"])
            // `void`, for the same reason `Mongosh::run_script` uses it: without
            // it mongosh echoes the assignment whenever `js` has no value of its
            // own, and the database name comes back as the result.
            .arg(format!(
                "void (db = db.getSiblingDB(\"{}\")); {js}",
                self.database
            ))
            .output()
            .expect("mongosh must be on PATH");

        assert!(
            output.status.success(),
            "mongosh failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );

        String::from_utf8_lossy(&output.stdout).trim().to_string()
    }
}

fn uri() -> String {
    std::env::var("MONGOR_TEST_URI").unwrap_or_else(|_| "mongodb://localhost:27017".to_string())
}

#[test]
fn prints_its_version() {
    Command::cargo_bin("mongor")
        .unwrap()
        .arg("--version")
        .assert()
        .success();
}

#[test]
fn new_writes_both_directions() {
    let project = Project::new("mongor_it_new");

    project
        .mongor()
        .args(["new", "create_users"])
        .assert()
        .success();

    assert!(project.path("00001_create_users.up.js").is_file());
    assert!(project.path("00001_create_users.down.js").is_file());
}

#[test]
fn migrate_applies_pending_then_reports_nothing_to_do() {
    let project = Project::new("mongor_it_migrate");
    project.write(
        "00001_create_users.up.js",
        r#"db.users.insertOne({ email: "a@example.com" });"#,
    );

    project
        .mongor()
        .args(["migrate", "--env", "test", "--yes"])
        .assert()
        .success()
        .stdout(predicate::str::contains("applied 1 migration(s)"));

    assert_eq!(project.eval("print(db.users.countDocuments())"), "1");

    project
        .mongor()
        .args(["migrate", "--env", "test", "--yes"])
        .assert()
        .success()
        .stdout(predicate::str::contains("nothing to migrate"));

    // The migration ran once, not twice.
    assert_eq!(project.eval("print(db.users.countDocuments())"), "1");
}

#[test]
fn migrate_refuses_a_migration_modified_after_it_was_applied() {
    let project = Project::new("mongor_it_tamper");
    project.write("00001_seed.up.js", r#"db.users.insertOne({ n: 1 });"#);

    project
        .mongor()
        .args(["migrate", "--env", "test", "--yes"])
        .assert()
        .success();

    // Same version, different contents: the checksum on the row no longer
    // describes what is on disk.
    project.write("00001_seed.up.js", r#"db.users.insertOne({ n: 2 });"#);

    project
        .mongor()
        .args(["migrate", "--env", "test", "--yes"])
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "00001_seed.up.js was modified after it was applied",
        ));

    // Refused, not half-applied.
    assert_eq!(project.eval("print(db.users.countDocuments())"), "1");
}

#[test]
fn a_failing_migration_is_recorded_as_failed() {
    let project = Project::new("mongor_it_failure");
    project.write(
        "00001_explodes.up.js",
        r#"db.users.insertOne({ n: 1 });
throw new Error("boom");"#,
    );

    project
        .mongor()
        .args(["migrate", "--env", "test", "--yes"])
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "leaving the database in an unknown state",
        ));

    // The write before the throw survived: a failed migration does not mean an
    // untouched database, which is the whole reason the row is kept.
    assert_eq!(project.eval("print(db.users.countDocuments())"), "1");
    assert_eq!(project.recorded_status(1).as_deref(), Some("failed"));
}

#[test]
fn an_unfinished_row_blocks_both_migrate_and_rollback() {
    let project = Project::new("mongor_it_unfinished");

    // Written by hand rather than by killing mongor mid-run: this is the state a
    // lost process leaves behind, and the guard is what we are testing.
    project.eval(
        r#"db.mongor_migrations.insertOne({
            version: 1,
            name: "interrupted",
            fileName: "00001_interrupted.up.js",
            checksum: "sha256:0",
            status: "running",
            startedAt: new Date(),
            finishedAt: null,
            executionTimeMs: null,
        });"#,
    );

    for command in ["migrate", "rollback"] {
        project
            .mongor()
            .args([command, "--env", "test"])
            .assert()
            .failure()
            .stderr(predicate::str::contains("1 unfinished migration(s)"))
            .stderr(predicate::str::contains("it may still be running"))
            // The refusal names the command that clears it.
            .stderr(predicate::str::contains(
                "mongor resolve 1 --env test --retry",
            ));
    }
}

#[test]
fn rollback_undoes_the_last_migration_and_makes_it_pending_again() {
    let project = Project::new("mongor_it_rollback");
    project.write("00001_add_user.up.js", r#"db.users.insertOne({ n: 1 });"#);
    project.write(
        "00001_add_user.down.js",
        r#"db.users.deleteMany({ n: 1 });"#,
    );

    project
        .mongor()
        .args(["migrate", "--env", "test", "--yes"])
        .assert()
        .success();

    project
        .mongor()
        .args(["rollback", "--env", "test", "--yes"])
        .assert()
        .success()
        .stdout(predicate::str::contains("rolled back"));

    assert_eq!(project.eval("print(db.users.countDocuments())"), "0");

    // The row is gone, not marked: the migration is pending again.
    assert_eq!(project.recorded_status(1), None);
    project
        .mongor()
        .args(["status", "--env", "test"])
        .assert()
        .success()
        .stdout(predicate::str::contains("pending"));
}

#[test]
fn rollback_refuses_when_the_down_script_is_missing() {
    let project = Project::new("mongor_it_no_down_file");
    project.write("00001_add_user.up.js", r#"db.users.insertOne({ n: 1 });"#);

    project
        .mongor()
        .args(["migrate", "--env", "test", "--yes"])
        .assert()
        .success();

    project
        .mongor()
        .args(["rollback", "--env", "test", "--yes"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("has no rollback script"));

    // Refused before anything was marked, so the migration is still applied.
    assert_eq!(project.recorded_status(1).as_deref(), Some("success"));
}

#[test]
fn rollback_refuses_when_the_down_script_is_only_comments() {
    let project = Project::new("mongor_it_empty_down");
    project.write("00001_add_user.up.js", r#"db.users.insertOne({ n: 1 });"#);
    project.write(
        "00001_add_user.down.js",
        "// Nobody wrote a rollback for this one.\n",
    );

    project
        .mongor()
        .args(["migrate", "--env", "test", "--yes"])
        .assert()
        .success();

    // Running the file would exit 0 and prove nothing, so mongor refuses rather
    // than claiming it undid something.
    project
        .mongor()
        .args(["rollback", "--env", "test", "--yes"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("contains no statements"));

    assert_eq!(project.recorded_status(1).as_deref(), Some("success"));
}

#[test]
fn migrate_refuses_a_migration_nobody_filled_in() {
    let project = Project::new("mongor_it_empty_up");

    // What `mongor new` leaves behind: comments and nothing else.
    project.write("00001_todo.up.js", "// Write your migration here.\n\n");

    project
        .mongor()
        .args(["migrate", "--env", "test", "--yes"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("has no statements"));

    // Nothing was recorded, so filling the file in later does not trip the
    // "modified after it was applied" guard.
    assert_eq!(project.recorded_status(1), None);
}

#[test]
fn baseline_skips_everything_up_to_its_version() {
    let project = Project::new("mongor_it_baseline");
    project.write(
        "00001_already_there.up.js",
        r#"db.users.insertOne({ n: 1 });"#,
    );
    project.write("00002_new_work.up.js", r#"db.users.insertOne({ n: 2 });"#);

    project
        .mongor()
        .args(["baseline", "--env", "test", "--version", "1"])
        .assert()
        .success()
        .stdout(predicate::str::contains("baselined test"));

    project
        .mongor()
        .args(["migrate", "--env", "test", "--yes"])
        .assert()
        .success()
        .stdout(predicate::str::contains("applied 1 migration(s)"));

    // 00001 never ran, so only 00002's document exists.
    assert_eq!(
        project.eval("print(db.users.countDocuments({ n: 1 }))"),
        "0"
    );
    assert_eq!(
        project.eval("print(db.users.countDocuments({ n: 2 }))"),
        "1"
    );

    // Re-baselining a database with history is refused.
    project
        .mongor()
        .args(["baseline", "--env", "test", "--version", "2"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("already has migration history"));
}

#[test]
fn migrate_refuses_to_apply_without_confirmation() {
    let project = Project::new("mongor_it_confirm");
    project.write("00001_add_user.up.js", r#"db.users.insertOne({ n: 1 });"#);

    // The child's stdin is a pipe, which is what it looks like in CI. Prompting
    // there would hang the job, so mongor has to refuse instead.
    project
        .mongor()
        .args(["migrate", "--env", "test"])
        .assert()
        .failure()
        // The plan is still printed: you can see what would have been applied.
        .stdout(predicate::str::contains("00001  add_user"))
        .stderr(predicate::str::contains("stdin is not a terminal"));

    assert_eq!(project.eval("print(db.users.countDocuments())"), "0");
    assert_eq!(project.recorded_status(1), None);
}

#[test]
fn rollback_refuses_to_run_without_confirmation() {
    let project = Project::new("mongor_it_rollback_confirm");
    project.write("00001_add_user.up.js", r#"db.users.insertOne({ n: 1 });"#);
    project.write(
        "00001_add_user.down.js",
        r#"db.users.deleteMany({ n: 1 });"#,
    );

    project
        .mongor()
        .args(["migrate", "--env", "test", "--yes"])
        .assert()
        .success();

    project
        .mongor()
        .args(["rollback", "--env", "test"])
        .assert()
        .failure()
        .stdout(predicate::str::contains("00001  add_user"))
        .stderr(predicate::str::contains("stdin is not a terminal"));

    // Still applied, and the document the migration wrote is untouched.
    assert_eq!(project.recorded_status(1).as_deref(), Some("success"));
    assert_eq!(project.eval("print(db.users.countDocuments())"), "1");
}

#[test]
fn migrate_creates_the_unique_version_index() {
    let project = Project::new("mongor_it_index");
    project.write("00001_add_user.up.js", r#"db.users.insertOne({ n: 1 });"#);

    project
        .mongor()
        .args(["migrate", "--env", "test", "--yes"])
        .assert()
        .success();

    assert_eq!(
        project.eval(
            r#"print(db.mongor_migrations.getIndexes()
                 .filter(i => i.name === "version_1" && i.unique === true).length)"#
        ),
        "1"
    );

    // The index, not mongor, is what makes a second row impossible.
    let rejected = project.eval(
        r#"try {
             db.mongor_migrations.insertOne({ version: 1, name: "dupe" });
             print("inserted");
           } catch (e) {
             print(e.code);
           }"#,
    );
    assert_eq!(rejected, "11000");
}

#[test]
fn migrate_refuses_a_version_that_already_has_a_row() {
    let project = Project::new("mongor_it_duplicate");
    project.write("00001_add_user.up.js", r#"db.users.insertOne({ n: 1 });"#);

    // A status mongor does not know is invisible to both `applied` and
    // `unfinished`, so 00001 looks pending and the insert is what catches it.
    // Deterministic, unlike racing two real runs.
    project.eval(
        r#"db.mongor_migrations.insertOne({
             version: 1,
             name: "add_user",
             fileName: "00001_add_user.up.js",
             checksum: "sha256:0",
             status: "somethingelse",
             startedAt: new Date(),
             finishedAt: null,
             executionTimeMs: null,
           });"#,
    );

    project
        .mongor()
        .args(["migrate", "--env", "test", "--yes"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("already has a row for 00001"));

    // Refused before the script ran.
    assert_eq!(project.eval("print(db.users.countDocuments())"), "0");
}

#[test]
fn status_does_not_create_the_collection() {
    let project = Project::new("mongor_it_readonly");
    project.write("00001_add_user.up.js", r#"db.users.insertOne({ n: 1 });"#);

    project
        .mongor()
        .args(["status", "--env", "test"])
        .assert()
        .success()
        .stdout(predicate::str::contains("pending"));

    // The index is created by the commands that write, so a read-only command
    // leaves a database it has never migrated completely untouched.
    assert_eq!(
        project.eval(r#"print(db.getCollectionNames().length)"#),
        "0"
    );
}

/// A migration that writes one document and then throws, leaving a `failed`
/// row and a database that was genuinely modified.
fn fail_one_migration(project: &Project) {
    project.write(
        "00001_half_done.up.js",
        r#"db.users.insertOne({ n: 1 });
throw new Error("boom");"#,
    );

    project
        .mongor()
        .args(["migrate", "--env", "test", "--yes"])
        .assert()
        .failure();

    assert_eq!(project.recorded_status(1).as_deref(), Some("failed"));
}

#[test]
fn resolve_retry_clears_the_row_so_migrate_runs_it_again() {
    let project = Project::new("mongor_it_resolve_retry");
    fail_one_migration(&project);

    project
        .mongor()
        .args(["resolve", "1", "--env", "test", "--retry", "--yes"])
        .assert()
        .success()
        .stdout(predicate::str::contains("is cleared"));

    assert_eq!(project.recorded_status(1), None);

    // Fixed by hand, the way an operator would before retrying.
    project.write("00001_half_done.up.js", r#"db.users.insertOne({ n: 2 });"#);

    project
        .mongor()
        .args(["migrate", "--env", "test", "--yes"])
        .assert()
        .success()
        .stdout(predicate::str::contains("applied 1 migration(s)"));

    assert_eq!(project.recorded_status(1).as_deref(), Some("success"));
    // Both documents: --retry never undid what the failed script wrote.
    assert_eq!(project.eval("print(db.users.countDocuments())"), "2");
}

#[test]
fn resolve_applied_records_success_without_running_the_script() {
    let project = Project::new("mongor_it_resolve_applied");
    fail_one_migration(&project);

    project
        .mongor()
        .args(["resolve", "1", "--env", "test", "--applied", "--yes"])
        .assert()
        .success()
        .stdout(predicate::str::contains("recorded as applied"));

    assert_eq!(project.recorded_status(1).as_deref(), Some("success"));
    // The script did not run a second time.
    assert_eq!(project.eval("print(db.users.countDocuments())"), "1");
    // The duration is cleared: it timed the attempt that failed.
    assert_eq!(
        project.eval("print(db.mongor_migrations.findOne({ version: 1 }).executionTimeMs)"),
        "null"
    );

    project
        .mongor()
        .args(["migrate", "--env", "test", "--yes"])
        .assert()
        .success()
        .stdout(predicate::str::contains("nothing to migrate"));
}

#[test]
fn resolve_refuses_rows_that_are_not_unfinished() {
    let project = Project::new("mongor_it_resolve_refuses");
    project.write("00001_add_user.up.js", r#"db.users.insertOne({ n: 1 });"#);

    // No row at all.
    project
        .mongor()
        .args(["resolve", "1", "--env", "test", "--retry", "--yes"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("no row for 00001"));

    project
        .mongor()
        .args(["migrate", "--env", "test", "--yes"])
        .assert()
        .success();

    // History, not an attempt.
    project
        .mongor()
        .args(["resolve", "1", "--env", "test", "--retry", "--yes"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("already applied"));

    assert_eq!(project.recorded_status(1).as_deref(), Some("success"));
}

#[test]
fn resolve_requires_exactly_one_action() {
    let project = Project::new("mongor_it_resolve_args");

    project
        .mongor()
        .args(["resolve", "1", "--env", "test"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("<--retry|--applied>"));

    project
        .mongor()
        .args(["resolve", "1", "--env", "test", "--retry", "--applied"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("cannot be used with"));
}

#[test]
fn resolve_refuses_to_run_without_confirmation() {
    let project = Project::new("mongor_it_resolve_confirm");
    fail_one_migration(&project);

    project
        .mongor()
        .args(["resolve", "1", "--env", "test", "--retry"])
        .assert()
        .failure()
        .stdout(predicate::str::contains("does not undo"))
        .stderr(predicate::str::contains("stdin is not a terminal"));

    assert_eq!(project.recorded_status(1).as_deref(), Some("failed"));
}

/// Three pending migrations, each writing one identifiable document.
fn three_migrations(project: &Project) {
    for n in 1..=3 {
        project.write(
            &format!("0000{n}_step_{n}.up.js"),
            &format!("db.users.insertOne({{ n: {n} }});"),
        );
    }
}

#[test]
fn migrate_to_applies_up_to_the_target_only() {
    let project = Project::new("mongor_it_to");
    three_migrations(&project);

    project
        .mongor()
        .args(["migrate", "--env", "test", "--to", "2", "--yes"])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "2 of 3 migration(s) pending for test",
        ))
        .stdout(predicate::str::contains("up to 00002"))
        .stdout(predicate::str::contains("applied 2 migration(s)"))
        .stdout(predicate::str::contains(
            "1 migration(s) still pending - raise --to to continue",
        ));

    assert_eq!(project.eval("print(db.users.countDocuments())"), "2");
    assert_eq!(project.recorded_status(3), None);

    // Raising the cap continues from where it stopped.
    project
        .mongor()
        .args(["migrate", "--env", "test", "--to", "3", "--yes"])
        .assert()
        .success()
        .stdout(predicate::str::contains("applied 1 migration(s)"));

    assert_eq!(project.eval("print(db.users.countDocuments())"), "3");
}

#[test]
fn migrate_to_rejects_targets_that_would_do_nothing() {
    let project = Project::new("mongor_it_to_rejects");
    three_migrations(&project);

    // No such migration: the failure mode is a typo silently applying all of them.
    project
        .mongor()
        .args(["migrate", "--env", "test", "--to", "99", "--yes"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("--to 00099 matches no migration"));

    project
        .mongor()
        .args(["migrate", "--env", "test", "--to", "2", "--yes"])
        .assert()
        .success();

    // Already applied.
    project
        .mongor()
        .args(["migrate", "--env", "test", "--to", "2", "--yes"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("--to 00002 is already applied"));

    // Nothing was applied by either refusal.
    assert_eq!(project.eval("print(db.users.countDocuments())"), "2");

    // Not a number at all.
    project
        .mongor()
        .args(["migrate", "--env", "test", "--to", "abc"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("invalid value 'abc'"));
}

#[test]
fn migrate_to_rejects_a_target_at_or_below_the_baseline() {
    let project = Project::new("mongor_it_to_baseline");
    three_migrations(&project);

    project
        .mongor()
        .args(["baseline", "--env", "test", "--version", "2"])
        .assert()
        .success();

    project
        .mongor()
        .args(["migrate", "--env", "test", "--to", "1", "--yes"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("at or below the baseline 00002"));
}

#[test]
fn migrate_dry_run_prints_the_plan_and_writes_nothing() {
    let project = Project::new("mongor_it_dry_run");
    three_migrations(&project);

    project
        .mongor()
        .args(["migrate", "--env", "test", "--dry-run"])
        .assert()
        .success()
        .stdout(predicate::str::contains("3 migration(s) pending for test"))
        .stdout(predicate::str::contains("00001  step_1"))
        .stdout(predicate::str::contains("00003  step_3"))
        .stdout(predicate::str::contains("dry run, nothing was applied"));

    // Not one write, not even the collection: the early return is above the
    // index call, which is the first thing migrate does that writes.
    assert_eq!(
        project.eval(r#"print(db.getCollectionNames().length)"#),
        "0"
    );

    // It composes with --to, and the cap shows in the plan.
    project
        .mongor()
        .args(["migrate", "--env", "test", "--to", "2", "--dry-run"])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "2 of 3 migration(s) pending for test",
        ))
        .stdout(predicate::str::contains("up to 00002"))
        .stdout(predicate::str::contains("00002  step_2"))
        .stdout(predicate::str::contains("00003  step_3").not());
}

#[test]
fn migrate_dry_run_conflicts_with_yes() {
    let project = Project::new("mongor_it_dry_run_args");

    // Both flags is a usage error rather than a silent precedence rule.
    project
        .mongor()
        .args(["migrate", "--env", "test", "--dry-run", "--yes"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("cannot be used with"));
}

#[test]
fn migrate_dry_run_still_enforces_the_read_only_guards() {
    let project = Project::new("mongor_it_dry_run_guards");
    project.write("00001_add_user.up.js", r#"db.users.insertOne({ n: 1 });"#);

    project
        .mongor()
        .args(["migrate", "--env", "test", "--yes"])
        .assert()
        .success();

    project.write("00001_add_user.up.js", r#"db.users.insertOne({ n: 2 });"#);

    // A plan that ignored the checksum guard would promise a run that cannot
    // happen, so the dry run refuses too.
    project
        .mongor()
        .args(["migrate", "--env", "test", "--dry-run"])
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "was modified after it was applied",
        ));
}

#[test]
fn validate_passes_a_clean_migration_set() {
    let project = Project::new("mongor_it_validate_clean");
    project.write("00001_add_user.up.js", r#"db.users.insertOne({ n: 1 });"#);
    project.write(
        "00001_add_user.down.js",
        r#"db.users.deleteMany({ n: 1 });"#,
    );

    project
        .mongor()
        .args(["validate", "--env", "test"])
        .assert()
        .success()
        .stdout(predicate::str::contains("no problems found in 1 migration"));

    // Same verdict without a database, which is what a pull request has.
    project
        .mongor()
        .args(["validate", "--offline"])
        .assert()
        .success()
        .stdout(predicate::str::contains("not checked (--offline)"))
        .stdout(predicate::str::contains("no problems found"));
}

#[test]
fn validate_reports_every_error_not_just_the_first() {
    let project = Project::new("mongor_it_validate_errors");
    project.write("00001_add_user.up.js", r#"db.users.insertOne({ n: 1 });"#);
    project.write("00002_add_order.up.js", r#"db.orders.insertOne({ n: 2 });"#);

    project
        .mongor()
        .args(["migrate", "--env", "test", "--yes"])
        .assert()
        .success();

    // Two independent problems at once: migrate would stop at the first.
    project.write("00001_add_user.up.js", r#"db.users.insertOne({ n: 99 });"#);
    project.write(
        "00002_add_order.up.js",
        r#"db.orders.insertOne({ n: 98 });"#,
    );

    project
        .mongor()
        .args(["validate", "--env", "test"])
        .assert()
        .failure()
        .stdout(predicate::str::contains("error  checksum").count(2))
        .stdout(predicate::str::contains("00001_add_user.up.js"))
        .stdout(predicate::str::contains("00002_add_order.up.js"))
        .stderr(predicate::str::contains("2 error(s)"));
}

#[test]
fn validate_flags_rows_with_no_file_and_names_resolve() {
    let project = Project::new("mongor_it_validate_rows");
    project.write("00001_add_user.up.js", r#"db.users.insertOne({ n: 1 });"#);

    project
        .mongor()
        .args(["migrate", "--env", "test", "--yes"])
        .assert()
        .success();

    // History describing code nobody can read.
    std::fs::remove_file(project.path("00001_add_user.up.js")).unwrap();

    // Plus an unfinished row, which must point at the command that clears it.
    project.eval(
        r#"db.mongor_migrations.insertOne({
             version: 7, name: "stuck", fileName: "00007_stuck.up.js",
             checksum: "sha256:0", status: "failed",
             startedAt: new Date(), finishedAt: null, executionTimeMs: null,
           });"#,
    );

    project
        .mongor()
        .args(["validate", "--env", "test"])
        .assert()
        .failure()
        .stdout(predicate::str::contains("error  missing-file"))
        .stdout(predicate::str::contains("error  unfinished"))
        .stdout(predicate::str::contains(
            "mongor resolve 7 --env test --retry",
        ));
}

#[test]
fn validate_warns_without_failing() {
    let project = Project::new("mongor_it_validate_warns");
    // No .down.js, and a gap at 00002.
    project.write("00001_add_user.up.js", r#"db.users.insertOne({ n: 1 });"#);
    project.write("00003_add_order.up.js", r#"db.orders.insertOne({ n: 3 });"#);
    // A rollback whose migration was renamed away.
    project.write("00009_gone.down.js", r#"db.users.drop();"#);
    // A file that looks like a script but cannot be one.
    project.write("00004_typo.up.js.bak", r#"db.users.drop();"#);

    project
        .mongor()
        .args(["validate", "--offline"])
        .assert()
        // The mis-named file is the only error; the rest are warnings.
        .failure()
        .stdout(predicate::str::contains("error  file-name"))
        .stdout(predicate::str::contains("warn   no-rollback").count(2))
        .stdout(predicate::str::contains("warn   gap"))
        .stdout(predicate::str::contains("warn   orphan-rollback"));

    std::fs::remove_file(project.path("00004_typo.up.js.bak")).unwrap();

    project
        .mongor()
        .args(["validate", "--offline"])
        .assert()
        .success()
        .stdout(predicate::str::contains("warning(s), no errors"));
}

#[test]
fn validate_requires_exactly_one_target() {
    let project = Project::new("mongor_it_validate_args");

    project
        .mongor()
        .arg("validate")
        .assert()
        .failure()
        .stderr(predicate::str::contains("<--env <ENV>|--offline>"));

    project
        .mongor()
        .args(["validate", "--env", "test", "--offline"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("cannot be used with"));
}

#[test]
fn completions_generates_a_script_for_each_shell() {
    let project = Project::new("mongor_it_completions");

    for shell in ["bash", "zsh", "fish", "powershell", "elvish"] {
        let output = project
            .mongor()
            .args(["completions", shell])
            .assert()
            .success()
            // Generated from the parser, so every command appears without the
            // completions code knowing any of their names.
            .stdout(predicate::str::contains("validate"))
            .stdout(predicate::str::contains("dry-run"))
            .get_output()
            .stdout
            .clone();

        // The script completes `mongor`, not the path cargo invoked.
        let script = String::from_utf8(output).unwrap();
        assert!(script.contains("mongor"), "{shell} script names the binary");
        assert!(!script.contains("target/debug"), "{shell} leaks the path");
    }
}

#[test]
fn completions_rejects_an_unknown_shell() {
    let project = Project::new("mongor_it_completions_args");

    project
        .mongor()
        .args(["completions", "tcsh"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("invalid value 'tcsh'"))
        // The accepted list comes from clap_complete, not from us.
        .stderr(predicate::str::contains("zsh"));
}
