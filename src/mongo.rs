use anyhow::{Context, Result, anyhow, bail};
use futures_util::TryStreamExt;
use mongodb::bson::{Bson, DateTime, Document, doc, to_bson};
use mongodb::error::{ErrorKind, WriteFailure};
use mongodb::options::IndexOptions;
use mongodb::{Collection, Database, IndexModel};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::time::Duration;

/// The name the driver would generate for `{ version: 1 }` anyway, spelled out
/// so the errors that mention the index can name it.
const VERSION_INDEX: &str = "version_1";

const DUPLICATE_KEY: i32 = 11000;
const INDEX_OPTIONS_CONFLICT: i32 = 85;
const INDEX_KEY_SPECS_CONFLICT: i32 = 86;

/// One row of migration history. This is a stored format: renaming a field or
/// changing `rename_all` invalidates every record already in the collection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MigrationStatus {
    Running,
    Success,
    Failed,
    /// Its rollback script was started and has not finished.
    RollingBack,
    /// A marker, not a migration: every version up to and including this one is
    /// assumed to be already applied, and will never be run.
    Baseline,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MigrationRecord {
    pub version: u32,
    pub name: String,
    pub file_name: String,
    pub checksum: String,
    pub status: MigrationStatus,
    pub started_at: DateTime,
    pub finished_at: Option<DateTime>,
    pub execution_time_ms: Option<i64>,
}

pub struct Store {
    db: Database,
    migrations: Collection<MigrationRecord>,
}

impl Store {
    pub async fn connect(uri: &str, database: &str, collection: &str) -> Result<Store> {
        let mut options = mongodb::options::ClientOptions::parse(uri)
            .await
            .context("invalid MongoDB connection URI")?;
        options.app_name = Some("mongor".to_string());
        options.server_selection_timeout = Some(Duration::from_secs(5));

        let client =
            mongodb::Client::with_options(options).context("failed to create Mongo client")?;
        let db = client.database(database);

        db.run_command(doc! { "ping": 1})
            .await
            .with_context(|| format!("failed to reach MongoDB for database `{database}`"))?;

        let migrations = db.collection(collection);

        Ok(Store { db, migrations })
    }

    pub fn database_name(&self) -> &str {
        self.db.name()
    }

    pub fn collection_name(&self) -> &str {
        self.migrations.name()
    }

    /// Creates the unique index that makes concurrent runs safe: whoever
    /// inserts the row for a version owns it, and the other run is refused
    /// instead of applying the same migration twice.
    ///
    /// Called by the commands that insert rows rather than by `connect`:
    /// `createIndexes` writes, and would also create the collection, which is
    /// not something `status` may do to a database it only reads.
    pub async fn ensure_version_index(&self) -> Result<()> {
        let index = IndexModel::builder()
            .keys(doc! { "version": 1 })
            .options(
                IndexOptions::builder()
                    .name(VERSION_INDEX.to_string())
                    .unique(true)
                    .build(),
            )
            .build();

        let Err(error) = self.migrations.create_index(index).await else {
            return Ok(());
        };

        let collection = self.migrations.name();
        match error_code(&error) {
            // The same unique index exists under a name someone else chose, so
            // what we wanted is already true.
            Some(INDEX_OPTIONS_CONFLICT) => Ok(()),
            Some(INDEX_KEY_SPECS_CONFLICT) => bail!(
                "{collection} already has a different index named `{VERSION_INDEX}`\n\
                 Drop it, then run the command again:\n  \
                 db.getCollection(\"{collection}\").dropIndex(\"{VERSION_INDEX}\")"
            ),
            Some(DUPLICATE_KEY) => bail!(
                "{collection} has more than one row for the same version, so `{VERSION_INDEX}` cannot be created\n\
                 Find them and keep one row per version:\n  \
                 db.getCollection(\"{collection}\").aggregate([{{ $group: {{ _id: \"$version\", rows: {{ $sum: 1 }} }} }}, {{ $match: {{ rows: {{ $gt: 1 }} }} }}])"
            ),
            _ => Err(error).with_context(|| {
                format!(
                    "failed to create `{VERSION_INDEX}` on {collection}\n\
                     Create it by hand if mongor's database user may not:\n  \
                     db.getCollection(\"{collection}\").createIndex({{ version: 1 }}, {{ unique: true, name: \"{VERSION_INDEX}\" }})"
                )
            }),
        }
    }

    /// Writes the row before the migration runs. The insert is also the lock:
    /// the unique index on `version` lets one run own a version, so a second
    /// `mongor migrate` is refused here rather than applying the file again.
    ///
    /// Nothing is ever replaced - `migrate` refuses to go on while any row is
    /// unfinished, and skips versions that are applied or baselined, so a row
    /// for a pending version can only be another run's, or one mongor did not
    /// write.
    pub async fn start(&self, record: &MigrationRecord) -> Result<()> {
        if let Err(error) = self.migrations.insert_one(record).await {
            if error_code(&error) == Some(DUPLICATE_KEY) {
                return Err(self.already_has_a_row(record.version));
            }

            return Err(error).with_context(|| {
                format!(
                    "failed to record {} in {}",
                    record.file_name,
                    self.migrations.name()
                )
            });
        }

        Ok(())
    }

    /// Something already holds the row for this version. Almost always another
    /// run that inserted first; otherwise a row mongor cannot see, because its
    /// status is not one of the five it knows.
    fn already_has_a_row(&self, version: u32) -> anyhow::Error {
        anyhow!(
            "{} already has a row for {version:05}\n  \
             another mongor run may own it - wait for it to finish, then `mongor status`\n  \
             otherwise inspect it:  db.getCollection(\"{}\").findOne({{ version: {version} }})",
            self.migrations.name(),
            self.migrations.name()
        )
    }

    pub async fn finish(
        &self,
        version: u32,
        status: MigrationStatus,
        execution_time_ms: i64,
    ) -> Result<()> {
        let result = self
            .migrations
            .update_one(
                doc! { "version": version },
                doc! { "$set": {
                    "status": to_bson(&status)?,
                    "finishedAt": DateTime::now(),
                    "executionTimeMs": execution_time_ms,
                }},
            )
            .await
            .with_context(|| {
                format!(
                    "failed to update {version:05} in {}",
                    self.migrations.name()
                )
            })?;

        // A zero-match update means the row was deleted while the script ran -
        // almost always a `mongor resolve --retry` against a live migration.
        // Reporting success would claim history that no longer exists.
        if result.matched_count == 0 {
            bail!(
                "{version:05} has no row in {} any more - it was removed while the migration was running",
                self.migrations.name()
            );
        }

        Ok(())
    }

    /// Rows that are not `success`: a migration still running, or one that
    /// failed. Either way the database may have been modified.
    pub async fn unfinished(&self) -> Result<Vec<MigrationRecord>> {
        let cursor = self
            .migrations
            .find(doc! { "status": { "$in": [
                to_bson(&MigrationStatus::Running)?,
                to_bson(&MigrationStatus::RollingBack)?,
                to_bson(&MigrationStatus::Failed)?,
            ] } })
            .sort(doc! { "version": 1 })
            .await
            .with_context(|| format!("failed to query {}", self.migrations.name()))?;

        cursor
            .try_collect()
            .await
            .with_context(|| format!("failed to read {}", self.migrations.name()))
    }

    /// Refuses to go on while any row is unfinished. A script that did not
    /// finish may have changed the database, and MongoDB will not undo it for
    /// us, so the only safe move is to stop and let a human look.
    /// Versions whose status is none of the five mongor knows, with the string
    /// as stored.
    ///
    /// Read as plain documents, because the whole point is that they do not
    /// deserialize. Without this they are invisible: `applied` and `unfinished`
    /// both filter on known values, so such a row looks pending while the
    /// unique index refuses to let anything write over it.
    pub async fn unknown_rows(&self) -> Result<Vec<(u32, String)>> {
        let cursor = self
            .migrations
            .clone_with_type::<Document>()
            .find(doc! { "status": { "$nin": [
                to_bson(&MigrationStatus::Running)?,
                to_bson(&MigrationStatus::Success)?,
                to_bson(&MigrationStatus::Failed)?,
                to_bson(&MigrationStatus::RollingBack)?,
                to_bson(&MigrationStatus::Baseline)?,
            ] } })
            .sort(doc! { "version": 1 })
            .await
            .with_context(|| format!("failed to query {}", self.migrations.name()))?;

        let found: Vec<Document> = cursor
            .try_collect()
            .await
            .with_context(|| format!("failed to read {}", self.migrations.name()))?;

        Ok(found
            .iter()
            .filter_map(|row| {
                let version = row.get_i32("version").map(|v| v as u32).ok()?;
                let status = row.get_str("status").unwrap_or("(not a string)");
                Some((version, status.to_string()))
            })
            .collect())
    }

    /// Drops a row whatever its status, for repairing one mongor cannot read.
    pub async fn remove_any(&self, version: u32) -> Result<bool> {
        let result = self
            .migrations
            .delete_one(doc! { "version": version })
            .await
            .with_context(|| {
                format!(
                    "failed to delete {version:05} from {}",
                    self.migrations.name()
                )
            })?;

        Ok(result.deleted_count > 0)
    }

    /// One row by version, whatever its status: the only reader that can tell
    /// "no such version" from "already applied".
    pub async fn record(&self, version: u32) -> Result<Option<MigrationRecord>> {
        self.migrations
            .find_one(doc! { "version": version })
            .await
            .with_context(|| format!("failed to query {}", self.migrations.name()))
    }

    pub async fn ensure_nothing_unfinished(&self, env: &str) -> Result<()> {
        self.ensure_nothing_unknown(env).await?;

        let unfinished = self.unfinished().await?;
        if unfinished.is_empty() {
            return Ok(());
        }

        let collection = self.migrations.name();
        let mut lines = vec![format!(
            "{} unfinished migration(s) in {collection}",
            unfinished.len()
        )];

        for record in &unfinished {
            // unfinished() cannot return a row without a detail line.
            let Some(detail) = unfinished_detail(record) else {
                continue;
            };

            lines.push(format!(
                "  {:05}  {}  {}",
                record.version, record.file_name, detail
            ));
        }

        lines.push(String::new());
        lines.push("Inspect the database, then resolve each row:".to_string());
        for record in &unfinished {
            lines.push(format!(
                "  mongor resolve {} --env {env} --retry",
                record.version
            ));
        }

        bail!("{}", lines.join("\n"))
    }

    /// The highest version that ran to completion, or `None` if the database is
    /// empty or holds only a baseline.
    /// Refuses while any row carries a status mongor does not know. Such a row
    /// cannot be reasoned about: it is not applied, not unfinished, and the
    /// unique index will not let a new one take its place.
    pub async fn ensure_nothing_unknown(&self, env: &str) -> Result<()> {
        let unknown = self.unknown_rows().await?;
        if unknown.is_empty() {
            return Ok(());
        }

        let collection = self.migrations.name();
        let mut lines = vec![format!(
            "{} row(s) in {collection} have a status mongor does not know",
            unknown.len()
        )];

        for (version, status) in &unknown {
            lines.push(format!("  {version:05}  status `{status}`"));
        }

        lines.push(String::new());
        lines.push("Inspect the database, then clear each row:".to_string());
        for (version, _) in &unknown {
            lines.push(format!("  mongor resolve {version} --env {env} --retry"));
        }

        bail!("{}", lines.join("\n"))
    }

    /// The migration applied most recently *in time*, which after a branch
    /// merge is not the highest version - and undoing the highest version would
    /// run a `.down.js` for a migration nobody just touched.
    ///
    /// `version` breaks ties, and sorts rows written before `finishedAt` existed
    /// last, because a missing value sorts lowest descending.
    pub async fn last_applied(&self) -> Result<Option<MigrationRecord>> {
        self.migrations
            .find_one(doc! { "status": to_bson(&MigrationStatus::Success)? })
            .sort(doc! { "finishedAt": -1, "version": -1 })
            .await
            .with_context(|| format!("failed to query {}", self.migrations.name()))
    }

    /// Marks the row as rolling back and restamps `startedAt`, so an abandoned
    /// rollback reports when the rollback began rather than when the migration
    /// was originally applied.
    pub async fn begin_rollback(&self, version: u32) -> Result<()> {
        self.migrations
            .update_one(
                doc! { "version": version },
                doc! { "$set": {
                    "status": to_bson(&MigrationStatus::RollingBack)?,
                    "startedAt": DateTime::now(),
                    "finishedAt": Bson::Null,
                    "executionTimeMs": Bson::Null,
                }},
            )
            .await
            .with_context(|| {
                format!(
                    "failed to update {version:05} in {}",
                    self.migrations.name()
                )
            })?;

        Ok(())
    }

    /// Changes only the status, leaving the timings from the original run.
    pub async fn set_status(&self, version: u32, status: MigrationStatus) -> Result<()> {
        self.migrations
            .update_one(
                doc! { "version": version },
                doc! { "$set": { "status": to_bson(&status)? } },
            )
            .await
            .with_context(|| {
                format!(
                    "failed to update {version:05} in {}",
                    self.migrations.name()
                )
            })?;

        Ok(())
    }

    /// Marks a version applied although mongor never saw its script succeed.
    ///
    /// The duration is cleared rather than kept: it timed the attempt that
    /// failed, and no honest number describes a run that never happened.
    /// Only an unfinished row: `false` means it stopped being one between the
    /// read that authorised this and the write.
    pub async fn mark_applied(&self, version: u32, checksum: Option<&str>) -> Result<bool> {
        let mut set = doc! {
            "status": to_bson(&MigrationStatus::Success)?,
            "finishedAt": DateTime::now(),
            "executionTimeMs": Bson::Null,
        };

        // Re-recorded from disk, so fixing the script before accepting the
        // migration does not leave this environment tripping the immutability
        // guard for an edit made before it was ever applied.
        if let Some(checksum) = checksum {
            set.insert("checksum", checksum);
        }

        let result = self
            .migrations
            .update_one(
                doc! { "version": version, "status": unfinished_filter()? },
                doc! { "$set": set },
            )
            .await
            .with_context(|| {
                format!(
                    "failed to update {version:05} in {}",
                    self.migrations.name()
                )
            })?;

        Ok(result.matched_count > 0)
    }

    /// Drops an unfinished row so `migrate` offers the migration again.
    ///
    /// `false` means there was nothing unfinished left to drop, which is how a
    /// migration that completed while the operator was deciding is caught.
    pub async fn remove_unfinished(&self, version: u32) -> Result<bool> {
        let result = self
            .migrations
            .delete_one(doc! { "version": version, "status": unfinished_filter()? })
            .await
            .with_context(|| {
                format!(
                    "failed to delete {version:05} from {}",
                    self.migrations.name()
                )
            })?;

        Ok(result.deleted_count > 0)
    }

    /// Drops the row: the migration is no longer applied, so `migrate` will
    /// offer it again.
    pub async fn remove(&self, version: u32) -> Result<()> {
        self.migrations
            .delete_one(doc! { "version": version })
            .await
            .with_context(|| {
                format!(
                    "failed to delete {version:05} from {}",
                    self.migrations.name()
                )
            })?;

        Ok(())
    }

    pub async fn is_empty(&self) -> Result<bool> {
        let count = self
            .migrations
            .count_documents(doc! {})
            .await
            .with_context(|| format!("failed to query {}", self.migrations.name()))?;

        Ok(count == 0)
    }

    /// The baselined version, if this database was adopted rather than built
    /// from scratch.
    pub async fn baseline(&self) -> Result<Option<u32>> {
        let record = self
            .migrations
            .find_one(doc! { "status": to_bson(&MigrationStatus::Baseline)? })
            .await
            .with_context(|| format!("failed to query {}", self.migrations.name()))?;

        Ok(record.map(|r| r.version))
    }

    /// Writes the marker row. The record has no checksum because no file was
    /// ever run: the row says "assume this was already done", nothing more.
    pub async fn set_baseline(&self, version: u32) -> Result<()> {
        let record = MigrationRecord {
            version,
            name: "baseline".to_string(),
            file_name: "baseline".to_string(),
            checksum: String::new(),
            status: MigrationStatus::Baseline,
            started_at: DateTime::now(),
            finished_at: None,
            execution_time_ms: None,
        };

        if let Err(error) = self.migrations.insert_one(&record).await {
            if error_code(&error) == Some(DUPLICATE_KEY) {
                return Err(self.already_has_a_row(version));
            }

            return Err(error).with_context(|| {
                format!("failed to write baseline to {}", self.migrations.name())
            });
        }

        Ok(())
    }

    /// Migrations that ran to completion, keyed by version. Rows still `running`
    /// or left `failed` are absent: they are attempts, not history.
    pub async fn applied(&self) -> Result<BTreeMap<u32, MigrationRecord>> {
        let cursor = self
            .migrations
            .find(doc! { "status": to_bson(&MigrationStatus::Success)? })
            .await
            .with_context(|| format!("failed to query {}", self.migrations.name()))?;

        let found: Vec<MigrationRecord> = cursor
            .try_collect()
            .await
            .with_context(|| format!("failed to read {}", self.migrations.name()))?;

        Ok(found.into_iter().map(|r| (r.version, r)).collect())
    }
}

/// The statuses `unfinished` returns, as a filter fragment. A write authorised
/// by reading one of these must not land on a row that has since become
/// something else.
fn unfinished_filter() -> Result<Document> {
    Ok(doc! {
        "$in": [
            to_bson(&MigrationStatus::Running)?,
            to_bson(&MigrationStatus::RollingBack)?,
            to_bson(&MigrationStatus::Failed)?,
        ]
    })
}

/// The sentence describing an unfinished row, shared so that the refusal which
/// blocks an operator and the `resolve` that unblocks them read identically.
///
/// `None` for the two statuses that are not unfinished at all.
pub(crate) fn unfinished_detail(record: &MigrationRecord) -> Option<String> {
    let detail = match record.status {
        // Possibly still running: killing mongor does not kill mongosh.
        MigrationStatus::Running => format!(
            "started {} and never finished - it may still be running",
            timestamp(record.started_at)
        ),
        MigrationStatus::RollingBack => format!(
            "was being rolled back at {} and never finished",
            timestamp(record.started_at)
        ),
        MigrationStatus::Failed => {
            format!("started {} and failed", timestamp(record.started_at))
        }
        MigrationStatus::Success | MigrationStatus::Baseline => return None,
    };

    Some(detail)
}

/// The server reports a duplicate key as a write error on an insert and as a
/// command error on `createIndexes`, so both shapes have to be unwrapped.
/// `mongodb::error::Error::code` would do this, but the driver keeps it private.
fn error_code(error: &mongodb::error::Error) -> Option<i32> {
    match error.kind.as_ref() {
        ErrorKind::Write(WriteFailure::WriteError(write_error)) => Some(write_error.code),
        ErrorKind::Command(command_error) => Some(command_error.code),
        _ => None,
    }
}

/// RFC 3339, so a timestamp in an error reads the same as one in the database.
fn timestamp(at: DateTime) -> String {
    at.try_to_rfc3339_string()
        .unwrap_or_else(|_| at.to_string())
}
