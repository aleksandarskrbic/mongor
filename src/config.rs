use anyhow::{Context, Result};
use serde::Deserialize;
use std::fs;
use std::path::PathBuf;

pub const FILE_NAME: &str = "mongor.toml";
pub const TEMPLATE: &str = include_str!("../templates/mongor.toml");

#[derive(Debug, Deserialize)]
pub struct Config {
    pub migrations: MigrationsConfig,
}

#[derive(Debug, Deserialize)]
pub struct MigrationsConfig {
    pub path: PathBuf,
    pub collection: String,
}

pub fn load() -> Result<Config> {
    let contents = fs::read_to_string(FILE_NAME)
        .with_context(|| format!("failed to read {FILE_NAME} (run `mongor init`?)"))?;
    toml::from_str(&contents).with_context(|| format!("failed to parse {FILE_NAME}"))
}
