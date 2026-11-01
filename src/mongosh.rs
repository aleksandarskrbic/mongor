use anyhow::{Context, Result, bail};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use tokio::process::Command;

/// The URI reaches mongosh through the environment, not `argv`, which `ps`
/// exposes to every user on the machine.
const URI_ENV: &str = "MONGOR_URI";

pub struct Mongosh {
    program: PathBuf,
    version: String,
}

impl Mongosh {
    pub async fn resolve() -> Result<Mongosh> {
        let program = PathBuf::from("mongosh");

        let output = base_command(&program)
            .arg("--version")
            .output()
            .await
            .with_context(|| not_runnable(&program))?;

        if !output.status.success() {
            bail!(
                "`{} --version` failed ({}): {}",
                program.display(),
                output.status,
                String::from_utf8_lossy(&output.stderr).trim()
            );
        }

        let version = String::from_utf8_lossy(&output.stdout).trim().to_string();
        Ok(Mongosh { program, version })
    }

    pub fn version(&self) -> &str {
        &self.version
    }

    /// Runs `script` with `db` bound to `database` on `uri`.
    ///
    /// stdout and stderr are inherited: a migration's output is the audit trail.
    pub async fn run_script(&self, script: &Path, uri: &str, database: &str) -> Result<()> {
        // mongosh has no `--db` flag — the database comes from the URI path, and
        // a URI without one silently means `test`. `--eval` runs before `--file`
        // and shares its global scope, so this is the `db` the script sees.
        // `void` stops mongosh echoing the assignment.
        let bootstrap = format!(r#"void (db = Mongo(process.env.{URI_ENV}).getDB("{database}"))"#);

        let status = base_command(&self.program)
            .args(["--nodb", "--norc", "--quiet", "--eval"])
            .arg(&bootstrap)
            .arg("--file")
            .arg(script)
            .env(URI_ENV, uri)
            .status()
            .await
            .with_context(|| not_runnable(&self.program))?;

        if !status.success() {
            bail!("mongosh {status}");
        }

        Ok(())
    }
}

/// The flags every mongosh invocation shares: never read stdin, so a script
/// that expects input fails instead of hanging.
fn base_command(program: &Path) -> Command {
    let mut command = Command::new(program);
    command.stdin(Stdio::null());
    command
}

fn not_runnable(program: &Path) -> String {
    format!(
        "could not run `{}` — install mongosh and put it on your PATH",
        program.display()
    )
}
