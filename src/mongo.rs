use anyhow::{Context, Result};
use futures_util::TryStreamExt;
use mongodb::bson::{DateTime, doc};
use mongodb::{Collection, Database};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::time::Duration;

/// One row of migration history. This is a stored format: renaming a field or
/// changing `rename_all` invalidates every record already in the collection.
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MigrationRecord {
    pub version: u32,
    pub name: String,
    pub file_name: String,
    pub checksum: String,
    pub applied_at: DateTime,
    pub execution_time_ms: i64,
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

    pub async fn record(&self, record: &MigrationRecord) -> Result<()> {
        self.migrations.insert_one(record).await.with_context(|| {
            format!(
                "failed to record {} in {}",
                record.file_name,
                self.migrations.name()
            )
        })?;

        Ok(())
    }

    pub async fn applied(&self) -> Result<BTreeMap<u32, MigrationRecord>> {
        let cursor = self
            .migrations
            .find(doc! {})
            .await
            .with_context(|| format!("failed to query {}", self.migrations.name()))?;

        let found: Vec<MigrationRecord> = cursor
            .try_collect()
            .await
            .with_context(|| format!("failed to read {}", self.migrations.name()))?;

        Ok(found.into_iter().map(|r| (r.version, r)).collect())
    }
}
