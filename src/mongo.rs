use anyhow::{Context, Result};
use futures_util::TryStreamExt;
use mongodb::bson::doc;
use mongodb::{Collection, Database};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::time::Duration;

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MigrationRecord {
    version: u32,
    name: String,
    checksum: String,
}

pub fn records(db: &Database, collection: &str) -> Collection<MigrationRecord> {
    db.collection(collection)
}

pub async fn applied(db: &Database, collection: &str) -> Result<BTreeMap<u32, MigrationRecord>> {
    let cursor = records(db, collection)
        .find(doc! {})
        .await
        .with_context(|| format!("failed to query {}", collection))?;

    let found: Vec<MigrationRecord> = cursor
        .try_collect()
        .await
        .with_context(|| format!("failed to read {}", collection))?;

    Ok(found.into_iter().map(|r| (r.version, r)).collect())
}

pub async fn connect(uri: &str, database: &str) -> Result<mongodb::Database> {
    let mut options = mongodb::options::ClientOptions::parse(uri)
        .await
        .context("invalid MongoDB connection URI")?;
    options.app_name = Some("mongor".to_string());
    options.server_selection_timeout = Some(Duration::from_secs(5));

    let client = mongodb::Client::with_options(options).context("failed to create Mongo client")?;
    let db = client.database(database);

    db.run_command(doc! { "ping": 1})
        .await
        .with_context(|| format!("failed to reach MongoDB for database `{database}`"))?;

    Ok(db)
}
