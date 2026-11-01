use crate::migration::Migration;
use crate::mongo::{MigrationRecord, Store};
use crate::mongosh::Mongosh;
use crate::{config, migration};
use anyhow::{Context, Result, bail};
use mongodb::bson::DateTime;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Instant;

/// Applies all pending migrations.
pub struct Migrate {
    env: String,
    uri: String,
    store: Store,
    mongosh: Mongosh,
    migrations_dir: PathBuf,
}

impl Migrate {
    pub async fn new(env: String) -> Result<Self> {
        let cfg = config::load()?;
        let environment = cfg.environment(&env)?;
        let uri = config::resolve_uri(environment)?;

        // Unlike `status`, a missing mongosh is fatal, so resolve it before
        // opening a connection we would only throw away.
        let mongosh = Mongosh::resolve().await?;
        let store = Store::connect(&uri, &environment.database, &cfg.migrations.collection).await?;

        Ok(Self {
            env,
            uri,
            store,
            mongosh,
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

        println!(
            "applying {} migration(s) to {} ({})",
            pending.len(),
            self.env,
            self.store.database_name()
        );

        for m in &pending {
            self.apply(m).await?;
        }

        println!("applied {} migration(s)", pending.len());
        Ok(())
    }

    async fn apply(&self, migration: &Migration) -> Result<()> {
        // Hashed before the run, so the record describes what actually executed.
        let checksum = migration.checksum()?;
        println!("{:04}  {}", migration.version, migration.name);

        let started = Instant::now();
        self.mongosh
            .run_script(&migration.path, &self.uri, self.store.database_name())
            .await
            .with_context(|| {
                format!(
                    "{} failed, leaving the database in an unknown state",
                    migration.file_name
                )
            })?;

        let record = MigrationRecord {
            version: migration.version,
            name: migration.name.clone(),
            file_name: migration.file_name.clone(),
            checksum,
            applied_at: DateTime::now(),
            execution_time_ms: started.elapsed().as_millis().try_into().unwrap_or(i64::MAX),
        };

        self.store
            .record(&record)
            .await
            .with_context(|| format!("{} ran but was not recorded", migration.file_name))?;

        println!("  ok in {} ms", record.execution_time_ms);
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
