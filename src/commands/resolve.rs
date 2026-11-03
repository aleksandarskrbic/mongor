use crate::mongo::{MigrationRecord, MigrationStatus, Store};
use crate::prompt::confirm;
use crate::{config, mongo};
use anyhow::{Result, bail};

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
            yes,
        })
    }

    pub async fn run(&self) -> Result<()> {
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

        match self.action {
            Action::Retry => {
                self.store.remove(record.version).await?;
                println!(
                    "  {:05} {} is cleared - migrate will run {} again",
                    record.version, record.name, record.file_name
                );
            }
            Action::Applied => {
                self.store.mark_applied(record.version).await?;
                println!(
                    "  {:05} {} is recorded as applied - its script was not run",
                    record.version, record.name
                );
            }
        }

        Ok(())
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
