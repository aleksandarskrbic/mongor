use crate::migration::Migration;
use crate::mongo::{MigrationRecord, MigrationStatus, Store};
use crate::mongosh::Mongosh;
use crate::{config, migration};
use anyhow::{Context, Result, bail};
use mongodb::bson::DateTime;
use std::collections::BTreeMap;
use std::io::{IsTerminal, Write};
use std::path::PathBuf;
use std::time::Instant;

/// Applies all pending migrations.
pub struct Migrate {
    env: String,
    uri: String,
    store: Store,
    mongosh: Mongosh,
    migrations_dir: PathBuf,
    yes: bool,
}

impl Migrate {
    pub async fn new(env: String, yes: bool) -> Result<Self> {
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
            yes,
        })
    }

    pub async fn run(&self) -> Result<()> {
        self.store.ensure_nothing_unfinished().await?;

        let migrations = migration::discover(&self.migrations_dir)?;
        if migrations.is_empty() {
            println!("no migrations found in {}", self.migrations_dir.display());
            return Ok(());
        }

        let applied = self.store.applied().await?;
        self.verify_applied(&migrations, &applied)?;

        let baseline = self.store.baseline().await?;
        if let Some(version) = baseline {
            println!("baseline is {version:05}, so earlier migrations are skipped");
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

        self.verify_pending(&pending)?;

        println!(
            "{} migration(s) pending for {} ({})",
            pending.len(),
            self.env,
            self.store.database_name()
        );

        // Listed only when we are about to ask: the apply loop below names each
        // migration as it runs, so printing them twice is noise. The count above
        // is what an interrupted unattended run still needs.
        if !self.yes {
            println!();
            for m in &pending {
                println!("  {:05}  {}", m.version, m.name);
            }
            println!();

            confirm(&format!(
                "Do you want to apply these {} migration(s)?",
                pending.len()
            ))?;
        }

        println!();
        for m in &pending {
            self.apply(m).await?;
        }

        println!("applied {} migration(s)", pending.len());
        Ok(())
    }

    async fn apply(&self, migration: &Migration) -> Result<()> {
        // Hashed before the run, so the record describes what actually executed.
        let checksum = migration.checksum()?;
        println!("{:05}  {}", migration.version, migration.name);

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

    /// Refuses to apply a migration nobody has written yet. Recording the
    /// template as applied would also record its checksum, so filling the file
    /// in afterwards would trip the "modified after it was applied" guard.
    fn verify_pending(&self, pending: &[&Migration]) -> Result<()> {
        for m in pending {
            if !migration::has_statements_in_file(&m.path)? {
                bail!(
                    "{} has no statements - write the migration before applying it",
                    m.file_name
                );
            }
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

/// Blocks until the operator types `yes`.
///
/// Only the exact word is accepted: `y` and a bare newline are easy to type by
/// reflex, which is the opposite of what a confirmation is for.
fn confirm(question: &str) -> Result<()> {
    // Refusing beats prompting when nobody can answer. In CI stdin is a pipe,
    // and a prompt there would hang until the job timed out.
    if !std::io::stdin().is_terminal() {
        bail!("{question}\n  stdin is not a terminal - pass --yes to apply without confirming");
    }

    print!("{question}\n  Only `yes` will be accepted.\n\n  Enter a value: ");

    // stdout is line buffered and the prompt has no newline, so without this
    // flush the question would appear after the answer was typed.
    std::io::stdout().flush()?;

    // Blocking read inside an async fn. Nothing else is running on the runtime
    // while we wait for a human, so spawn_blocking would only add noise.
    let mut answer = String::new();
    // No newline of our own: we only read when stdin is a terminal, and the
    // terminal already echoed the one the operator typed.
    std::io::stdin().read_line(&mut answer)?;

    if answer.trim() != "yes" {
        bail!("cancelled");
    }

    Ok(())
}
