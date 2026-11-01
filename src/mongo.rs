use anyhow::{Context, Result};
use futures_util::TryStreamExt;
use mongodb::bson::{DateTime, doc, to_bson};
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
                    "failed to update {version:04} in {}",
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
            .find(doc! { "status": { "$ne": to_bson(&MigrationStatus::Success)? } })
            .sort(doc! { "version": 1 })
            .await
            .with_context(|| format!("failed to query {}", self.migrations.name()))?;

        cursor
            .try_collect()
            .await
            .with_context(|| format!("failed to read {}", self.migrations.name()))
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
