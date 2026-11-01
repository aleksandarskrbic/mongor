use crate::migration::Migration;
use crate::mongo::{MigrationRecord, MigrationStatus, Store};
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
        self.verify_nothing_unfinished().await?;

        let migrations = migration::discover(&self.migrations_dir)?;
        if migrations.is_empty() {
            println!("no migrations found in {}", self.migrations_dir.display());
            return Ok(());
        }

        let applied = self.store.applied().await?;
        self.verify_applied(&migrations, &applied)?;

        let baseline = self.store.baseline().await?;
        if let Some(version) = baseline {
            println!("baseline is {version:04}, so earlier migrations are skipped");
        }

        let pending: Vec<&Migration> = migrations
            .iter()
            .filter(|m| !applied.contains_key(&m.version))
            .filter(|m| baseline.is_none_or(|version| m.version > version))
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

        let record = MigrationRecord {
            version: migration.version,
            name: migration.name.clone(),
            file_name: migration.file_name.clone(),
            checksum,
            status: MigrationStatus::Running,
            started_at: DateTime::now(),
            finished_at: None,
            execution_time_ms: None,
        };

        // Written before the script runs: if we lose the process mid-migration,
        // this row is the only evidence the database may have been touched.
        self.store.start(&record).await?;

        let started = Instant::now();
        let outcome = self
            .mongosh
            .run_script(&migration.path, &self.uri, self.store.database_name())
            .await;
        let elapsed = started.elapsed().as_millis().try_into().unwrap_or(i64::MAX);

        let status = if outcome.is_ok() {
            MigrationStatus::Success
        } else {
            MigrationStatus::Failed
        };
        let recorded = self.store.finish(migration.version, status, elapsed).await;

        // The script's own failure is the more useful of the two errors.
        outcome.with_context(|| {
            format!(
                "{} failed, leaving the database in an unknown state",
                migration.file_name
            )
        })?;
        recorded.with_context(|| {
            format!(
                "{} ran but the result was not recorded",
                migration.file_name
            )
        })?;

        println!("  ok in {elapsed} ms");
        Ok(())
    }

    /// Refuses to run while any row is `running` or `failed`. A migration that
    /// did not finish may have changed the database, and MongoDB will not undo
    /// it for us, so the only safe move is to stop and let a human look.
    async fn verify_nothing_unfinished(&self) -> Result<()> {
        let unfinished = self.store.unfinished().await?;
        if unfinished.is_empty() {
            return Ok(());
        }

        let mut lines = vec![format!(
            "{} unfinished migration(s) in {}",
            unfinished.len(),
            self.store.collection_name()
        )];

        for record in &unfinished {
            let detail = match record.status {
                // Possibly still running: killing mongor does not kill mongosh.
                MigrationStatus::Running => format!(
                    "started {} and never finished - it may still be running",
                    timestamp(record.started_at)
                ),
                MigrationStatus::Failed => format!("failed at {}", timestamp(record.started_at)),
                // unfinished() cannot return either of these.
                MigrationStatus::Success | MigrationStatus::Baseline => continue,
            };

            lines.push(format!(
                "  {:04}  {}  {}",
                record.version, record.file_name, detail
            ));
        }

        lines.push(String::new());
        lines.push("Inspect the database, then clear the row to retry:".to_string());
        for record in &unfinished {
            lines.push(format!(
                "  db.getCollection(\"{}\").deleteOne({{ version: {} }})",
                self.store.collection_name(),
                record.version
            ));
        }

        bail!("{}", lines.join("\n"))
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

/// RFC 3339, so a timestamp in an error reads the same as one in the database.
fn timestamp(at: DateTime) -> String {
    at.try_to_rfc3339_string()
        .unwrap_or_else(|_| at.to_string())
}
