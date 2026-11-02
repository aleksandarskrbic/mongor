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

    /// Runs JavaScript against this project's database and returns stdout.
    ///
    /// Shells out to mongosh rather than using the Rust driver: the driver is
    /// async, and a one-line query is not worth a tokio runtime per test.
    fn eval(&self, js: &str) -> String {
        let output = std::process::Command::new("mongosh")
            .arg(uri())
            .args(["--quiet", "--eval"])
            .arg(format!("db = db.getSiblingDB(\"{}\"); {js}", self.database))
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
        .args(["migrate", "--env", "test"])
        .assert()
        .success()
        .stdout(predicate::str::contains("applied 1 migration(s)"));

    assert_eq!(project.eval("print(db.users.countDocuments())"), "1");

    project
        .mongor()
        .args(["migrate", "--env", "test"])
        .assert()
        .success()
        .stdout(predicate::str::contains("nothing to migrate"));

    // The migration ran once, not twice.
    assert_eq!(project.eval("print(db.users.countDocuments())"), "1");
}
