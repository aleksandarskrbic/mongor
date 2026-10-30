use anyhow::{Context, Result, anyhow};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

pub const FILE_NAME: &str = "mongor.toml";
pub const TEMPLATE: &str = include_str!("../templates/mongor.toml");

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub migrations: MigrationsConfig,
    pub environments: BTreeMap<String, Environment>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MigrationsConfig {
    pub path: PathBuf,
    pub collection: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Environment {
    pub uri_env: String,
    pub database: String,
    #[serde(default)]
    pub require_confirm: bool,
}

impl Config {
    pub fn environment(&self, name: &str) -> Result<&Environment> {
        self.environments.get(name).ok_or_else(|| {
            let available: Vec<&str> = self.environments.keys().map(String::as_str).collect();
            anyhow!(
                "unknown environment `{name}` in {FILE_NAME} (available: {})",
                available.join(", ")
            )
        })
    }
}

pub fn load() -> Result<Config> {
    let contents = fs::read_to_string(FILE_NAME)
        .with_context(|| format!("failed to read {FILE_NAME} (run `mongor init`?)"))?;
    toml::from_str(&contents).with_context(|| format!("failed to parse {FILE_NAME}"))
}

pub fn resolve_uri(environment: &Environment) -> Result<String> {
    let name = &environment.uri_env;
    std::env::var(name).with_context(|| {
        format!("environment variable {name} is not set (export {name}=\"mongodb://...\")")
    })
}
