use crate::migration::Migration;
use crate::mongo::{MigrationRecord, Store};
use crate::{config, migration, mongo};
use anyhow::{Context, Result, bail};
use std::collections::BTreeMap;
use std::path::PathBuf;

/// Checks the migration set against recorded history without changing either.
///
/// Reads only: this is the command a pull request runs, and a check that can
/// write is a check nobody will point at production.
pub struct Validate {
    env: Option<String>,
    /// `None` under `--offline`: every database check then runs over nothing.
    store: Option<Store>,
    migrations_dir: PathBuf,
}

impl Validate {
    pub async fn new(env: Option<String>, offline: bool) -> Result<Self> {
        let cfg = config::load()?;

        // No mongosh: validate runs no script, and a CI image that only checks
        // migrations should not have to install a shell to do it.
        let mut store = None;

        if !offline {
            let name = env
                .as_deref()
                .context("pass --env <env> to check against a database, or --offline")?;
            let environment = cfg.environment(name)?;
            let uri = config::resolve_uri(environment)?;

            store = Some(
                Store::connect(&uri, &environment.database, &cfg.migrations.collection).await?,
            );
        }

        Ok(Self {
            env,
            store,
            migrations_dir: cfg.migrations.path,
        })
    }

    pub async fn run(&self) -> Result<()> {
        self.print_header();

        let migrations = migration::discover(&self.migrations_dir)?;
        if migrations.is_empty() {
            // Not a short circuit, unlike status and migrate: an empty directory
            // is when the applied-row checks matter most.
            println!("no migrations found in {}", self.migrations_dir.display());
            println!();
        }

        // Offline leaves both empty, which turns every database check into a
        // loop over nothing - no mode flag inside the checks themselves.
        let (applied, baseline) = match &self.store {
            Some(store) => (store.applied().await?, store.baseline().await?),
            None => (BTreeMap::new(), None),
        };

        let mut errors = Vec::new();
        let mut warnings = Vec::new();

        self.check_file_names(&mut errors)?;
        self.check_statements(&migrations, &applied, baseline, &mut errors)?;
        self.check_checksums(&migrations, &applied, &mut errors)?;
        self.check_missing_files(&migrations, &applied, &mut errors);
        self.check_renames(&migrations, &applied, &mut errors);
        self.check_baseline(&migrations, baseline, &mut errors);

        if let Some(store) = &self.store {
            self.check_unfinished(store, &mut errors).await?;
            self.check_unknown(store, &mut errors).await?;
        }

        self.check_rollbacks(&migrations, &mut warnings)?;
        self.check_orphan_rollbacks(&migrations, &mut warnings)?;
        self.check_gaps(&migrations, &mut warnings);
        self.check_out_of_order(&migrations, &applied, baseline, &mut warnings);

        self.report(migrations.len(), &errors, &warnings)
    }

    fn print_header(&self) {
        match (self.env.as_deref(), &self.store) {
            (Some(env), Some(store)) => {
                println!("environment  {env}");
                println!(
                    "database     {} ({})",
                    store.database_name(),
                    store.collection_name()
                );
            }
            _ => println!("database     not checked (--offline)"),
        }

        println!("migrations   {}", self.migrations_dir.display());
        println!();
    }

    fn check_file_names(&self, errors: &mut Vec<String>) -> Result<()> {
        for file_name in migration::suspicious_file_names(&self.migrations_dir)? {
            errors.push(problem(
                "file-name",
                &file_name,
                "is not a migration mongor can run - expected <version>_<name>.up.js",
            ));
        }

        Ok(())
    }

    /// Only files migrate would actually run: a placeholder below the baseline
    /// never runs, so failing CI over it would be stricter than migrate itself.
    fn check_statements(
        &self,
        migrations: &[Migration],
        applied: &BTreeMap<u32, MigrationRecord>,
        baseline: Option<u32>,
        errors: &mut Vec<String>,
    ) -> Result<()> {
        for m in migrations {
            // An applied file's contents are governed by its recorded checksum.
            if applied.contains_key(&m.version) {
                continue;
            }

            if baseline.is_some_and(|version| m.version <= version) {
                continue;
            }

            if !migration::has_statements_in_file(&m.path)? {
                errors.push(problem(
                    "no-statements",
                    &m.file_name,
                    "contains no statements - write the migration or delete the file",
                ));
            }
        }

        Ok(())
    }

    /// The same comparison `Migrate::verify_applied` makes, kept separate
    /// because that one stops at the first mismatch and this one must not.
    fn check_checksums(
        &self,
        migrations: &[Migration],
        applied: &BTreeMap<u32, MigrationRecord>,
        errors: &mut Vec<String>,
    ) -> Result<()> {
        for m in migrations {
            let Some(record) = applied.get(&m.version) else {
                continue;
            };

            let checksum = m.checksum()?;
            if checksum != record.checksum {
                errors.push(problem(
                    "checksum",
                    &m.file_name,
                    &format!(
                        "modified after it was applied - recorded {}, on disk {checksum}",
                        record.checksum
                    ),
                ));
            }
        }

        Ok(())
    }

    /// A row with no file on disk: migrate never looks for one, so nothing else
    /// in mongor notices that the history describes code nobody can read.
    fn check_missing_files(
        &self,
        migrations: &[Migration],
        applied: &BTreeMap<u32, MigrationRecord>,
        errors: &mut Vec<String>,
    ) {
        for (version, record) in applied {
            if migrations.iter().any(|m| m.version == *version) {
                continue;
            }

            errors.push(problem(
                "missing-file",
                &record.file_name,
                &format!(
                    "is recorded as applied but is not in {}",
                    self.migrations_dir.display()
                ),
            ));
        }
    }

    /// `rollback` builds the `.down.js` name from the recorded name, not from
    /// disk, so renaming an applied migration leaves it unable to undo that
    /// version - and the checksum is unchanged, so nothing else notices.
    fn check_renames(
        &self,
        migrations: &[Migration],
        applied: &BTreeMap<u32, MigrationRecord>,
        errors: &mut Vec<String>,
    ) {
        for m in migrations {
            let Some(record) = applied.get(&m.version) else {
                continue;
            };

            if record.name == m.name {
                continue;
            }

            errors.push(problem(
                "renamed",
                &m.file_name,
                &format!(
                    "is recorded as {} - rollback would look for {}",
                    record.file_name,
                    migration::rollback_file_name(record.version, &record.name)
                ),
            ));
        }
    }

    /// Migrate skips everything at or below the baseline, so a baseline above
    /// every file makes a deploy that does nothing look successful.
    fn check_baseline(
        &self,
        migrations: &[Migration],
        baseline: Option<u32>,
        errors: &mut Vec<String>,
    ) {
        let Some(version) = baseline else {
            return;
        };

        if migrations
            .last()
            .is_none_or(|highest| version > highest.version)
        {
            errors.push(problem(
                "baseline",
                "baseline",
                &format!(
                    "is {version:05}, higher than every migration on disk - migrate would skip them all"
                ),
            ));
        }
    }

    async fn check_unfinished(&self, store: &Store, errors: &mut Vec<String>) -> Result<()> {
        for record in store.unfinished().await? {
            // unfinished() cannot return a row without a detail line.
            let Some(detail) = mongo::unfinished_detail(&record) else {
                continue;
            };

            errors.push(problem(
                "unfinished",
                &record.file_name,
                &format!(
                    "{detail} - run `mongor resolve {} --env {} --retry`",
                    record.version,
                    self.env.as_deref().unwrap_or("<env>")
                ),
            ));
        }

        Ok(())
    }

    /// A row mongor cannot read is invisible to every other check, so without
    /// this validate calls a database clean that `migrate` refuses outright.
    async fn check_unknown(&self, store: &Store, errors: &mut Vec<String>) -> Result<()> {
        for (version, status) in store.unknown_rows().await? {
            errors.push(problem(
                "unknown-status",
                &format!("{version:05}"),
                &format!(
                    "has status `{status}`, which mongor does not know - run `mongor resolve {version} --env {} --retry`",
                    self.env.as_deref().unwrap_or("<env>")
                ),
            ));
        }

        Ok(())
    }

    /// A migration that cannot be undone is allowed, and worth saying out loud
    /// before the deploy rather than during the incident.
    fn check_rollbacks(&self, migrations: &[Migration], warnings: &mut Vec<String>) -> Result<()> {
        for m in migrations {
            let Some(rollback) =
                migration::find_rollback(&self.migrations_dir, m.version, &m.name)?
            else {
                warnings.push(problem(
                    "no-rollback",
                    &m.file_name,
                    &format!(
                        "{} is missing - this migration cannot be undone",
                        migration::rollback_file_name(m.version, &m.name)
                    ),
                ));
                continue;
            };

            if !migration::has_statements_in_file(&rollback.path)? {
                warnings.push(problem(
                    "no-rollback",
                    &m.file_name,
                    &format!(
                        "{} has no statements - this migration cannot be undone",
                        rollback.file_name
                    ),
                ));
            }
        }

        Ok(())
    }

    /// Usually half a rename: rollback builds the file name from the record, so
    /// the stale file is never run and never mentioned.
    fn check_orphan_rollbacks(
        &self,
        migrations: &[Migration],
        warnings: &mut Vec<String>,
    ) -> Result<()> {
        for rollback in migration::discover_rollbacks(&self.migrations_dir)? {
            if migrations
                .iter()
                .any(|m| m.version == rollback.version && m.name == rollback.name)
            {
                continue;
            }

            warnings.push(problem(
                "orphan-rollback",
                &rollback.file_name,
                "has no migration of the same version and name",
            ));
        }

        Ok(())
    }

    /// Gaps are legal - versions are an order, not a sequence - but a file lost
    /// in a bad merge looks exactly like this.
    fn check_gaps(&self, migrations: &[Migration], warnings: &mut Vec<String>) {
        for pair in migrations.windows(2) {
            let (previous, next) = (pair[0].version, pair[1].version);

            if next > previous + 1 {
                warnings.push(problem(
                    "gap",
                    &pair[1].file_name,
                    &format!(
                        "follows {previous:05}, so {} version(s) are missing",
                        next - previous - 1
                    ),
                ));
            }
        }
    }

    /// Migrate applies whatever is unapplied, in version order, so a migration
    /// merged in below the high-water mark runs after the ones it precedes.
    fn check_out_of_order(
        &self,
        migrations: &[Migration],
        applied: &BTreeMap<u32, MigrationRecord>,
        baseline: Option<u32>,
        warnings: &mut Vec<String>,
    ) {
        let Some((highest, _)) = applied.last_key_value() else {
            return;
        };

        for m in migrations {
            if applied.contains_key(&m.version) || m.version >= *highest {
                continue;
            }

            if baseline.is_some_and(|version| m.version <= version) {
                continue;
            }

            warnings.push(problem(
                "out-of-order",
                &m.file_name,
                &format!("is pending, but {highest:05} is already applied"),
            ));
        }
    }

    fn report(&self, migrations: usize, errors: &[String], warnings: &[String]) -> Result<()> {
        print_block("error", errors);
        print_block("warn", warnings);

        if errors.is_empty() && warnings.is_empty() {
            println!("no problems found in {migrations} migration(s)");
            return Ok(());
        }

        if errors.is_empty() {
            println!("{} warning(s), no errors", warnings.len());
            return Ok(());
        }

        bail!("{} error(s) - fix them before deploying", errors.len())
    }
}

/// Severity first, check name second: `validate | grep '^error'` and
/// `validate | grep checksum` are the whole machine-readable contract.
fn problem(check: &str, subject: &str, detail: &str) -> String {
    format!("{check:<15}  {subject:<32}  {detail}")
}

fn print_block(severity: &str, problems: &[String]) {
    if problems.is_empty() {
        return;
    }

    for problem in problems {
        println!("{severity:<5}  {problem}");
    }

    println!();
}
