use crate::migration::Migration;
use crate::mongo::{MigrationRecord, Store};
use crate::{config, migration};
use anyhow::{Result, bail};
use std::collections::BTreeMap;
use std::path::PathBuf;

/// Applies all pending migrations.
pub struct Migrate {
    env: String,
    store: Store,
    migrations_dir: PathBuf,
}

impl Migrate {
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
        let migrations = migration::discover(&self.migrations_dir)?;
        if migrations.is_empty() {
            println!("no migrations found in {}", self.migrations_dir.display());
            return Ok(());
        }

        let applied = self.store.applied().await?;
        self.verify_applied(&migrations, &applied)?;

        let pending: Vec<&Migration> = migrations
            .iter()
            .filter(|m| !applied.contains_key(&m.version))
            .collect();

        if pending.is_empty() {
            println!(
                "nothing to migrate, {} already applied to {}",
                applied.len(),
                self.env
            );
            return Ok(());
        }

        println!("{} migration(s) pending for {}", pending.len(), self.env);
        for m in &pending {
            println!("  {:04}  {}", m.version, m.name);
        }

        Ok(())
    }

    fn verify_applied(
        &self,
        migrations: &[Migration],
        applied: &BTreeMap<u32, MigrationRecord>,
    ) -> Result<()> {
        for m in migrations {
            let Some(record) = applied.get(&m.version) else {
                continue;
            };

            let checksum = m.checksum()?;
            if checksum != record.checksum {
                bail!(
                    "{} was modified after it was applied\n  recorded: {}\n  on disk:  {}",
                    m.file_name,
                    record.checksum,
                    checksum
                );
            }
        }

        Ok(())
    }
}
