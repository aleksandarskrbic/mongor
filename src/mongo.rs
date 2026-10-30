use anyhow::{Context, Result};
use mongodb::bson::doc;
use std::time::Duration;

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
