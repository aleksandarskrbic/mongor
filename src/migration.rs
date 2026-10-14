use anyhow::{Context, Result};
use std::fs;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};

pub const TEMPLATE: &str = include_str!("../templates/migration.js");

#[derive(Debug)]
pub struct Migration {
    pub version: u32,
    pub name: String,
    pub file_name: String,
    pub path: PathBuf,
}

fn parse(file_name: &str) -> Option<(u32, &str)> {
    let rest = file_name.strip_prefix('V')?;
    let (version, remainder) = rest.split_once("__")?;
    let name = remainder.strip_suffix(".js")?;
    Some((version.parse().ok()?, name))
}

/// Returns every `V<version>__<name>.js` in `migrations_dir`, sorted by version.
pub fn discover(migrations_dir: &Path) -> Result<Vec<Migration>> {
    let entries = fs::read_dir(migrations_dir)
        .with_context(|| format!("failed to read {}", migrations_dir.display()))?;

    let mut migrations = Vec::new();

    for entry in entries {
        let entry =
            entry.with_context(|| format!("failed to read {}", migrations_dir.display()))?;

        let os_name = entry.file_name();
        let Some(file_name) = os_name.to_str() else {
            continue;
        };

        let Some((version, name)) = parse(file_name) else {
            continue;
        };

        migrations.push(Migration {
            version,
            name: name.to_string(),
            file_name: file_name.to_string(),
            path: entry.path(),
        });
    }

    migrations.sort_by_key(|m| m.version);

    for pair in migrations.windows(2) {
        if pair[0].version == pair[1].version {
            anyhow::bail!(
                "duplicate migration version {:04}: {} and {}",
                pair[0].version,
                pair[0].file_name,
                pair[1].file_name
            );
        }
    }

    Ok(migrations)
}

pub fn next_version(migrations_dir: &Path) -> Result<u32> {
    let migrations = discover(migrations_dir)?;
    Ok(migrations.last().map_or(1, |m| m.version + 1))
}

pub fn create(name: &str, migrations_dir: &Path) -> Result<()> {
    if name.is_empty() || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
        anyhow::bail!(
            "migration name must be non-empty ASCII letters, digits and underscores: {name}"
        );
    }

    let version = next_version(migrations_dir)?;
    let file_name = format!("V{version:04}__{name}.js");
    let path = migrations_dir.join(&file_name);

    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .with_context(|| format!("failed to create {}", path.display()))?;

    file.write_all(TEMPLATE.as_bytes())
        .with_context(|| format!("failed to write {}", path.display()))?;

    println!("created {}", path.display());
    Ok(())
}
