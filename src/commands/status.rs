use crate::mongo::{MigrationStatus, Store};
use crate::mongosh::Mongosh;
use crate::{config, migration};
use anyhow::Result;
use std::collections::BTreeMap;
use std::path::PathBuf;

/// Reports which migrations are applied, pending, or modified since they ran.
pub struct Status {
    env: String,
    store: Store,
    migrations_dir: PathBuf,
}

impl Status {
    pub async fn new(env: String) -> Result<Self> {
        let cfg = config::load()?;
        let environment = cfg.environment(&env)?;
        let uri = config::resolve_uri(environment)?;

        let store = Store::connect(&uri, &environment.database, &cfg.migrations.collection).await?;

        Ok(Self {
            env,
            store,
            migrations_dir: cfg.migrations.path,
        })
    }

    pub async fn run(&self) -> Result<()> {
        self.print_header().await;
        self.print_migrations().await
    }

    async fn print_header(&self) {
        println!("environment  {}", self.env);
        println!("database     {}", self.store.database_name());

        match Mongosh::resolve().await {
            Ok(m) => println!("mongosh      {}", m.version()),
            Err(e) => println!("mongosh      unavailable: {e:#}"),
        }

        println!();
    }

    async fn print_migrations(&self) -> Result<()> {
        let migrations = migration::discover(&self.migrations_dir)?;

        if migrations.is_empty() {
            println!("no migrations found in {}", self.migrations_dir.display());
            return Ok(());
        }

        let applied = self.store.applied().await?;
        let baseline = self.store.baseline().await?;

        let unfinished: BTreeMap<u32, MigrationStatus> = self
            .store
            .unfinished()
            .await?
            .into_iter()
            .map(|r| (r.version, r.status))
            .collect();

        println!("{:<9} {:<32} Status", "Version", "Migration");
        for m in &migrations {
            let status = if let Some(record) = applied.get(&m.version) {
                if m.checksum()? == record.checksum {
                    "applied"
                } else {
                    "MODIFIED"
                }
            } else if let Some(status) = unfinished.get(&m.version) {
                match status {
                    MigrationStatus::Running => "RUNNING",
                    MigrationStatus::Failed => "FAILED",
                    // unfinished() returns neither of these.
                    MigrationStatus::Success | MigrationStatus::Baseline => "pending",
                }
            } else if baseline.is_some_and(|version| m.version <= version) {
                "baseline"
            } else {
                "pending"
            };

            println!(
                "{:<9} {:<32} {}",
                format!("{:04}", m.version),
                m.name,
                status
            );
        }

        Ok(())
    }
}
