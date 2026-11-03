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
            .stderr(predicate::str::contains("it may still be running"));
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
