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

        let mut child = base_command(&self.program)
            .args(["--nodb", "--norc", "--quiet", "--eval"])
            .arg(&bootstrap)
            .arg("--file")
            .arg(script)
            .env(URI_ENV, uri)
            // Off by default, because tokio cannot reap a child it kills during
            // a panic or a runtime shutdown. Worth it here: every other way out
            // of this function leaves a mongosh writing to a database whose row
            // says the migration is still running.
            .kill_on_drop(true)
            .spawn()
            .with_context(|| not_runnable(&self.program))?;

        let status = tokio::select! {
            status = child.wait() => status.with_context(|| not_runnable(&self.program))?,

            signal = interrupted() => {
                // `kill` here is SIGKILL and unconditional. A migration halfway
                // through an updateMany cannot be asked to finish tidily, and
                // the row is about to say so, so there is nothing to gain by
                // being gentle - only a child that might outlive the message.
                child.kill().await.ok();

                bail!(
                    "{signal} received, so mongosh was killed - the database may be half-migrated"
                );
            }
        };

        if !status.success() {
            bail!("mongosh {status}");
        }

        Ok(())
    }
}

/// Resolves when the operator stops the run, naming the signal that did it.
///
/// Ctrl-C needs no help reaching mongosh: the terminal signals the whole
/// foreground process group, and the child is in it. SIGTERM is the case that
/// does - `kill`, a cancelled CI job and a container being stopped all signal
/// mongor alone, so without this the script carries on writing as an orphan
/// while mongor exits and the row still reads `running`. The next operator then
/// sees an interrupted migration and runs `mongor resolve`, against a database
/// something is still changing.
///
/// Installing a handler also means Ctrl-C no longer kills mongor outright, so
/// the row gets marked before we exit. The window is one update; a second
/// Ctrl-C is not handled and will not shorten it.
async fn interrupted() -> &'static str {
    use tokio::signal::unix::{SignalKind, signal};

    // A failure here means the process cannot take signals at all. Returning a
    // future that never resolves leaves the old behaviour - the run continues
    // and the child is reaped by `wait` - rather than failing a migration over
    // the absence of a safety net.
    let Ok(mut interrupt) = signal(SignalKind::interrupt()) else {
        return std::future::pending().await;
    };
    let Ok(mut terminate) = signal(SignalKind::terminate()) else {
        return std::future::pending().await;
    };

    // The patterns are refutable on purpose. `select!` disables an arm whose
    // output does not match rather than taking it, so a stream that has gone
    // away drops out of the race instead of reporting a signal nobody sent and
    // killing a healthy migration. `else` is the case where both have dropped
    // out, which would otherwise panic.
    tokio::select! {
        Some(()) = interrupt.recv() => "SIGINT",
        Some(()) = terminate.recv() => "SIGTERM",
        else => std::future::pending().await,
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
