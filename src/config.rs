use anyhow::{Context, Result, anyhow, bail};
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
        let environment = self.environments.get(name).ok_or_else(|| {
            let available: Vec<&str> = self.environments.keys().map(String::as_str).collect();
            anyhow!(
                "unknown environment `{name}` in {FILE_NAME} (available: {})",
                available.join(", ")
            )
        })?;

        validate_database_name(&environment.database)?;
        Ok(environment)
    }
}

/// Stricter than MongoDB's own rules, because the name is interpolated into the
/// JavaScript mongosh runs: an allowlist removes the need to escape it.
fn validate_database_name(name: &str) -> Result<()> {
    if name.is_empty() {
        bail!("database name in {FILE_NAME} must not be empty");
    }

    if name.len() > 63 {
        bail!("database name in {FILE_NAME} must be at most 63 bytes: {name}");
    }

    if !name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    {
        bail!("database name in {FILE_NAME} must be ASCII letters, digits, `_` or `-`: {name}");
    }

    Ok(())
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_plausible_database_names() {
        for name in ["a", "myapp", "myapp_dev", "my-app", "App1", &"x".repeat(63)] {
            assert!(validate_database_name(name).is_ok(), "should accept {name}");
        }
    }

    #[test]
    fn rejects_names_that_could_escape_the_generated_javascript() {
        let cases = [
            "",
            &"x".repeat(64),
            "my app",
            "my.app",
            "my\"app",
            "my\\app",
            "my'app",
            "my\napp",
            "my$app",
            "my/app",
            "café",
            r#"x");db.dropDatabase();//"#,
        ];

        for name in cases {
            assert!(
                validate_database_name(name).is_err(),
                "should reject {name:?}"
            );
        }
    }
}
