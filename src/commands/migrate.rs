use crate::migration::Migration;
use crate::mongo::{MigrationRecord, MigrationStatus, Store};
use crate::mongosh::Mongosh;
use crate::prompt::confirm;
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
    to: Option<u32>,
    yes: bool,
    dry_run: bool,
}

impl Migrate {
    pub async fn new(env: String, to: Option<u32>, yes: bool, dry_run: bool) -> Result<Self> {
        let cfg = config::load()?;
        let environment = cfg.environment(&env)?;
        let uri = config::resolve_uri(environment)?;

        // Unlike `status`, a missing mongosh is fatal, so resolve it before
        // opening a connection we would only throw away. A dry run resolves it
        // too: a plan that ignores a precondition of the run is not a plan.
        let mongosh = Mongosh::resolve().await?;
        let store = Store::connect(&uri, &environment.database, &cfg.migrations.collection).await?;

        Ok(Self {
            env,
            uri,
            store,
            mongosh,
            migrations_dir: cfg.migrations.path,
            to,
            yes,
            dry_run,
        })
    }

    pub async fn run(&self) -> Result<()> {
        self.store.ensure_nothing_unfinished(&self.env).await?;

        let migrations = migration::discover(&self.migrations_dir)?;
        if migrations.is_empty() {
            println!("no migrations found in {}", self.migrations_dir.display());
            return Ok(());
        }

        let applied = self.store.applied().await?;
        self.verify_applied(&migrations, &applied)?;

        let baseline = self.store.baseline().await?;
        self.verify_target(&migrations, &applied, baseline)?;

        if let Some(version) = baseline {
            println!("baseline is {version:05}, so earlier migrations are skipped");
        }

        let mut pending: Vec<&Migration> = migrations
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

        // Capped after the emptiness check: "nothing to migrate" is a fact about
        // the database, not about this run's ceiling.
        let total = pending.len();
        if let Some(target) = self.to {
            pending.retain(|m| m.version <= target);
        }

        self.verify_pending(&pending)?;

        // The count carries the cap because --yes suppresses the list below: in
        // a CI log this line is the only evidence the run was capped.
        match self.to {
            Some(target) => println!(
                "{} of {total} migration(s) pending for {} ({}), up to {target:05}",
                pending.len(),
                self.env,
                self.store.database_name()
            ),
            None => println!(
                "{total} migration(s) pending for {} ({})",
                self.env,
                self.store.database_name()
            ),
        }

        // Every guard above only reads, so a dry run runs all of them and stops
        // here, one line short of the first write.
        if self.dry_run {
            print_pending(&pending);
            println!("dry run, nothing was applied - re-run without --dry-run to apply");
            return Ok(());
        }

        // Listed only when we are about to ask: the apply loop below names each
        // migration as it runs, so printing them twice is noise. The count above
        // is what an interrupted unattended run still needs.
        if !self.yes {
            print_pending(&pending);

            confirm(&format!(
                "Do you want to apply these {} migration(s)?",
                pending.len()
            ))?;
        }

        // Below the prompt, not above it: `createIndexes` is DDL, and declining
        // must leave a database mongor has never migrated exactly as it was.
        // Still before the first script, so a permissions problem stops the run
        // rather than half-applying it.
        self.store.ensure_version_index().await?;

        println!();
        for m in &pending {
            self.apply(m).await?;
        }

        println!("applied {} migration(s)", pending.len());

        let withheld = total - pending.len();
        if withheld > 0 {
            println!("{withheld} migration(s) still pending - raise --to to continue");
        }

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

    /// Refuses every `--to` that names nothing to do. A cap that selected no
    /// migration would print an ordinary plan and exit 0, which is
    /// indistinguishable from a staged rollout that worked.
    fn verify_target(
        &self,
        migrations: &[Migration],
        applied: &BTreeMap<u32, MigrationRecord>,
        baseline: Option<u32>,
    ) -> Result<()> {
        let Some(target) = self.to else {
            return Ok(());
        };

        if !migrations.iter().any(|m| m.version == target) {
            bail!(
                "--to {target:05} matches no migration in {} - run `mongor status --env {}` to see which versions exist",
                self.migrations_dir.display(),
                self.env
            );
        }

        if let Some(version) = baseline
            && target <= version
        {
            bail!(
                "--to {target:05} is at or below the baseline {version:05} - migrations up to the baseline never run"
            );
        }

        if applied.contains_key(&target) {
            bail!(
                "--to {target:05} is already applied to {} - raise --to, or drop it to apply everything pending",
                self.env
            );
        }

        // Not the same as the check above: the target is pending, but the
        // database is already past it, so the state it asks for cannot be built.
        if let Some(latest) = applied.keys().next_back().copied()
            && target < latest
        {
            bail!(
                "--to {target:05} is behind {}, which is already at {latest:05} - roll back first, or raise --to",
                self.env
            );
        }

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

/// The plan, surrounded by blank lines. A dry run and the confirmation prompt
/// print the same block, so there is one definition of what a plan looks like.
fn print_pending(pending: &[&Migration]) {
    println!();
    for m in pending {
        println!("  {:05}  {}", m.version, m.name);
    }
    println!();
}
