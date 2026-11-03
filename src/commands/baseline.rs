use crate::migration;
use crate::mongo::Store;
use crate::workspace::Workspace;
use anyhow::{Result, bail};
use std::path::PathBuf;

/// Marks a database that already has the schema as being up to date, so the
/// migrations that built it are never run against it.
pub struct Baseline {
    env: String,
    store: Store,
    migrations_dir: PathBuf,
    version: Option<u32>,
}

impl Baseline {
    pub async fn new(env: String, version: Option<u32>) -> Result<Self> {
        let workspace = Workspace::open(&env).await?;

        Ok(Self {
            env,
            store: workspace.store,
            migrations_dir: workspace.migrations_dir,
            version,
        })
    }

    pub async fn run(&self) -> Result<()> {
        if !self.store.is_empty().await? {
            bail!(
                "{} already has migration history in {} - baseline is only for adopting a database mongor has never touched",
                self.store.database_name(),
                self.store.collection_name()
            );
        }

        let version = self.resolve_version()?;
        self.store.ensure_version_index().await?;
        self.store.set_baseline(version).await?;

        println!(
            "baselined {} ({}) at {version:05}",
            self.env,
            self.store.database_name()
        );
        println!("migrations up to {version:05} will never run against it");
        Ok(())
    }

    fn resolve_version(&self) -> Result<u32> {
        if let Some(version) = self.version {
            return Ok(version);
        }

        let migrations = migration::discover(&self.migrations_dir)?;
        match migrations.last() {
            Some(m) => Ok(m.version),
            None => bail!(
                "no migrations found in {} - pass --version to baseline anyway",
                self.migrations_dir.display()
            ),
        }
    }
}
