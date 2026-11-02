use anyhow::{Context, Result, bail};
use futures_util::TryStreamExt;
use mongodb::bson::{Bson, DateTime, doc, to_bson};
use mongodb::{Collection, Database};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::time::Duration;

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

    /// Writes the row before the migration runs, replacing anything left by an
    /// earlier attempt: the history holds one row per version.
    pub async fn start(&self, record: &MigrationRecord) -> Result<()> {
        self.migrations
            .replace_one(doc! { "version": record.version }, record)
            .upsert(true)
            .await
            .with_context(|| {
                format!(
                    "failed to record {} in {}",
                    record.file_name,
                    self.migrations.name()
                )
            })?;

        Ok(())
    }

    pub async fn finish(
        &self,
        version: u32,
        status: MigrationStatus,
        execution_time_ms: i64,
    ) -> Result<()> {
        self.migrations
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
    pub async fn ensure_nothing_unfinished(&self) -> Result<()> {
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
                // unfinished() cannot return either of these.
                MigrationStatus::Success | MigrationStatus::Baseline => continue,
            };

            lines.push(format!(
                "  {:05}  {}  {}",
                record.version, record.file_name, detail
            ));
        }

        lines.push(String::new());
        lines.push("Inspect the database, then clear the row to retry:".to_string());
        for record in &unfinished {
            lines.push(format!(
                "  db.getCollection(\"{collection}\").deleteOne({{ version: {} }})",
                record.version
            ));
        }

        bail!("{}", lines.join("\n"))
    }

    /// The highest version that ran to completion, or `None` if the database is
    /// empty or holds only a baseline.
    pub async fn last_applied(&self) -> Result<Option<MigrationRecord>> {
        self.migrations
            .find_one(doc! { "status": to_bson(&MigrationStatus::Success)? })
            .sort(doc! { "version": -1 })
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

        self.migrations
            .insert_one(&record)
            .await
            .with_context(|| format!("failed to write baseline to {}", self.migrations.name()))?;

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

/// RFC 3339, so a timestamp in an error reads the same as one in the database.
fn timestamp(at: DateTime) -> String {
    at.try_to_rfc3339_string()
        .unwrap_or_else(|_| at.to_string())
}
