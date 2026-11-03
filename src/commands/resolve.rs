use crate::migration::Migration;
use crate::mongo::{MigrationRecord, MigrationStatus, Store};
use crate::prompt::confirm;
use crate::{config, mongo};
use anyhow::{Result, bail};
use std::path::PathBuf;

/// Repairs the row left behind by a migration that failed or never finished.
///
/// Bookkeeping only: no script is ever run. The operator is the one who knows
/// what the half-finished script did to the data, so mongor only records that
/// judgement and gets out of the way.
pub struct Resolve {
    env: String,
    store: Store,
    version: u32,
    action: Action,
    migrations_dir: PathBuf,
    yes: bool,
}

enum Action {
    Retry,
    Applied,
}

impl Resolve {
    pub async fn new(
        env: String,
        version: u32,
        retry: bool,
        applied: bool,
        yes: bool,
    ) -> Result<Self> {
        // The arg group in `cli` already rules out both and neither, so this
        // message should never print - but an error costs less than a panic.
        let action = match (retry, applied) {
            (true, false) => Action::Retry,
            (false, true) => Action::Applied,
            _ => bail!("pass exactly one of --retry and --applied"),
        };

        let cfg = config::load()?;
        let environment = cfg.environment(&env)?;
        let uri = config::resolve_uri(environment)?;

        let store = Store::connect(&uri, &environment.database, &cfg.migrations.collection).await?;

        Ok(Self {
            env,
            store,
            version,
            action,
            migrations_dir: cfg.migrations.path,
            yes,
        })
    }

    pub async fn run(&self) -> Result<()> {
        // Checked before the typed read, which cannot deserialize such a row.
        // `resolve` is the command every other refusal points at, so it has to
        // be able to clear the row those refusals are about.
        if let Some(status) = self.unknown_status().await? {
            return self.clear_unknown(&status).await;
        }

        let Some(record) = self.store.record(self.version).await? else {
            bail!(
                "no row for {:05} in {} - nothing to resolve",
                self.version,
                self.store.collection_name()
            );
        };

        let detail = self.detail(&record)?;

        println!(
            "1 migration to resolve in {} ({})",
            self.env,
            self.store.database_name()
        );
        println!();
        println!("  {:05}  {}  {detail}", record.version, record.file_name);
        println!();
        println!("  {}", self.caveat());

        if !self.yes {
            println!();
            confirm(&self.question(&record))?;
        }

        println!();

        // Both writes carry the status `detail()` validated, so a migration that
        // finished while the operator was deciding is not overwritten. The
        // window is real even under --yes: the read above and the write here are
        // separate round trips.
        let acted = match self.action {
            Action::Retry => self.store.remove_unfinished(record.version).await?,
            Action::Applied => {
                let checksum = self.checksum_on_disk(&record)?;
                self.store
                    .mark_applied(record.version, checksum.as_deref())
                    .await?
            }
        };

        if !acted {
            bail!(
                "{:05} is no longer unfinished - it changed while this command was running\n  \
                 run `mongor status --env {}` to see where it got to",
                record.version,
                self.env
            );
        }

        match self.action {
            Action::Retry => println!(
                "  {:05} {} is cleared - migrate will run {} again",
                record.version, record.name, record.file_name
            ),
            Action::Applied => println!(
                "  {:05} {} is recorded as applied - its script was not run",
                record.version, record.name
            ),
        }

        Ok(())
    }

    /// The stored status of this version when mongor does not recognise it.
    async fn unknown_status(&self) -> Result<Option<String>> {
        Ok(self
            .store
            .unknown_rows()
            .await?
            .into_iter()
            .find(|(version, _)| *version == self.version)
            .map(|(_, status)| status))
    }

    /// Drops a row mongor cannot read. Only `--retry` can do this: `--applied`
    /// would be asserting something about a row nobody can interpret.
    async fn clear_unknown(&self, status: &str) -> Result<()> {
        if !matches!(self.action, Action::Retry) {
            bail!(
                "{:05} has status `{status}`, which mongor does not know - only --retry can clear it",
                self.version
            );
        }

        println!(
            "1 row to clear in {} ({})",
            self.env,
            self.store.database_name()
        );
        println!();
        println!(
            "  {:05}  status `{status}` is not one mongor wrote",
            self.version
        );
        println!();
        println!("  {}", self.caveat());

        if !self.yes {
            println!();
            confirm(&format!(
                "Do you want to clear the row for {:05}?",
                self.version
            ))?;
        }

        println!();
        if !self.store.remove_any(self.version).await? {
            bail!(
                "{:05} has no row any more - it changed while this command was running",
                self.version
            );
        }

        println!(
            "  {:05} is cleared - migrate will offer the migration again",
            self.version
        );
        Ok(())
    }

    /// The migration's checksum as the file stands now, or `None` when the file
    /// is gone - in which case the recorded one is all there is.
    fn checksum_on_disk(&self, record: &MigrationRecord) -> Result<Option<String>> {
        let path = self.migrations_dir.join(&record.file_name);
        if !path.is_file() {
            return Ok(None);
        }

        let migration = Migration {
            version: record.version,
            name: record.name.clone(),
            file_name: record.file_name.clone(),
            path,
        };

        Ok(Some(migration.checksum()?))
    }

    /// Only the rows `Store::unfinished` returns can be resolved. A `success`
    /// row is history and a `baseline` row is a marker, so rewriting either
    /// would make the collection claim something that never happened.
    fn detail(&self, record: &MigrationRecord) -> Result<String> {
        if let Some(detail) = mongo::unfinished_detail(record) {
            return Ok(detail);
        }

        // Those are the only two statuses `unfinished_detail` has no line for.
        match record.status {
            MigrationStatus::Success => bail!(
                "{:05} {} is already applied - use `mongor rollback` to undo it",
                record.version,
                record.name
            ),
            _ => bail!(
                "{:05} is the baseline marker in {}, not a migration - resolve cannot change it",
                record.version,
                self.store.collection_name()
            ),
        }
    }

    fn caveat(&self) -> &'static str {
        match self.action {
            Action::Retry => "clearing the row does not undo whatever the script already changed",
            Action::Applied => "recording it as applied does not run the script",
        }
    }

    fn question(&self, record: &MigrationRecord) -> String {
        match self.action {
            Action::Retry => format!(
                "Do you want to clear {:05} {} so migrate runs it again?",
                record.version, record.name
            ),
            Action::Applied => format!(
                "Do you want to record {:05} {} as applied without running it?",
                record.version, record.name
            ),
        }
    }
}
