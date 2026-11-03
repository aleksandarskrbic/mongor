use crate::mongo::{MigrationStatus, Store};
use crate::mongosh::Mongosh;
use crate::prompt::confirm;
use crate::{config, migration};
use anyhow::{Context, Result, bail};
use std::path::PathBuf;
use std::time::Instant;

/// Undoes the most recently applied migration by running its `U` script.
pub struct Rollback {
    env: String,
    uri: String,
    store: Store,
    mongosh: Mongosh,
    migrations_dir: PathBuf,
    yes: bool,
}

impl Rollback {
    pub async fn new(env: String, yes: bool) -> Result<Self> {
        let cfg = config::load()?;
        let environment = cfg.environment(&env)?;
        let uri = config::resolve_uri(environment)?;

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

        let Some(record) = self.store.last_applied().await? else {
            println!("nothing to roll back in {}", self.env);
            return Ok(());
        };

        // Built from the record rather than from disk, so a migration whose `V`
        // file has since been deleted can still be rolled back.
        let file_name = migration::rollback_file_name(record.version, &record.name);
        let script = self.migrations_dir.join(&file_name);

        if !script.is_file() {
            bail!(
                "{:05} {} has no rollback script - expected {}",
                record.version,
                record.name,
                script.display()
            );
        }

        // Checked before anything is marked, so a migration nobody wrote a
        // rollback for leaves no row to clean up.
        if !migration::has_statements_in_file(&script)? {
            bail!(
                "{:05} {} has no rollback - {} contains no statements",
                record.version,
                record.name,
                script.display()
            );
        }

        println!(
            "1 migration to roll back from {} ({})",
            self.env,
            self.store.database_name()
        );
        println!();
        println!("  {:05}  {}  ({file_name})", record.version, record.name);

        if !self.yes {
            println!();

            confirm(&format!(
                "Do you want to roll back {:05} {}?",
                record.version, record.name
            ))?;
        }

        println!();

        // Marked before the script runs, for the same reason `migrate` marks a
        // migration running: losing the process must leave evidence behind.
        self.store.begin_rollback(record.version).await?;

        let started = Instant::now();
        let outcome = self
            .mongosh
            .run_script(&script, &self.uri, self.store.database_name())
            .await;

        match outcome {
            Ok(()) => {
                // The row goes away entirely: the migration is pending again.
                self.store.remove(record.version).await.with_context(|| {
                    format!(
                        "{file_name} ran but {:05} is still recorded as applied",
                        record.version
                    )
                })?;

                println!("  rolled back in {} ms", started.elapsed().as_millis());
                Ok(())
            }
            Err(error) => {
                self.store
                    .set_status(record.version, MigrationStatus::Failed)
                    .await?;

                Err(error).with_context(|| {
                    format!("{file_name} failed, leaving the database in an unknown state")
                })
            }
        }
    }
}
