use anyhow::{Context, Result, bail};
use std::path::PathBuf;
use tokio::process::Command;

pub struct Mongosh {
    program: PathBuf,
    version: String,
}

impl Mongosh {
    pub async fn resolve() -> Result<Mongosh> {
        let program = PathBuf::from("mongosh");
        let output = Command::new(&program)
            .arg("--version")
            .output()
            .await
            .with_context(|| {
                format!(
                    "could not run `{}` — install mongosh and put it on your PATH",
                    program.display()
                )
            })?;

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
}
