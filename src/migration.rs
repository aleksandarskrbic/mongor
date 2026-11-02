use anyhow::{Context, Result};
use sha2::{Digest, Sha256};
use std::fs;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};

pub const TEMPLATE: &str = include_str!("../templates/migration.js");
pub const ROLLBACK_TEMPLATE: &str = include_str!("../templates/rollback.js");

#[derive(Debug)]
pub struct Migration {
    pub version: u32,
    pub name: String,
    pub file_name: String,
    pub path: PathBuf,
}

impl Migration {
    /// SHA-256 of the file's raw bytes, as `sha256:<lowercase hex>`.
    ///
    /// The hex half matches `shasum -a 256 <file>`, so a user can verify by hand.
    /// The prefix lets us change algorithm later without invalidating old records.
    pub fn checksum(&self) -> Result<String> {
        let bytes = fs::read(&self.path)
            .with_context(|| format!("failed to read {}", self.path.display()))?;

        Ok(format!("sha256:{:x}", Sha256::digest(&bytes)))
    }
}

/// Width used for generated file names. `parse` accepts any width, so a
/// hand-written `1_foo.up.js` is not silently ignored.
const VERSION_WIDTH: usize = 5;

pub fn migration_file_name(version: u32, name: &str) -> String {
    format!("{version:0VERSION_WIDTH$}_{name}.up.js")
}

pub fn rollback_file_name(version: u32, name: &str) -> String {
    format!("{version:0VERSION_WIDTH$}_{name}.down.js")
}

fn parse(file_name: &str) -> Option<(u32, &str)> {
    let rest = file_name.strip_suffix(".up.js")?;
    let (version, name) = rest.split_once('_')?;

    if version.is_empty() || name.is_empty() || !version.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }

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
                "duplicate migration version {:05}: {} and {}",
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

    write_new(
        &migrations_dir.join(migration_file_name(version, name)),
        TEMPLATE,
    )?;
    write_new(
        &migrations_dir.join(rollback_file_name(version, name)),
        ROLLBACK_TEMPLATE,
    )?;

    Ok(())
}

fn write_new(path: &Path, contents: &str) -> Result<()> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .with_context(|| format!("failed to create {}", path.display()))?;

    file.write_all(contents.as_bytes())
        .with_context(|| format!("failed to write {}", path.display()))?;

    println!("created {}", path.display());
    Ok(())
}

/// Whether the script at `path` does anything, read leniently so that invalid
/// UTF-8 is a file with no statements rather than an error.
pub fn has_statements_in_file(path: &Path) -> Result<bool> {
    let bytes = fs::read(path).with_context(|| format!("failed to read {}", path.display()))?;

    Ok(has_statements(&String::from_utf8_lossy(&bytes)))
}

/// Whether a script does anything at all, ignoring blank lines and `//` lines.
///
/// Both generated templates are entirely comments. Running one would exit 0
/// without doing anything, so mongor has to tell "nothing to do" from "nobody
/// filled this in" before it runs the script, not after.
pub fn has_statements(source: &str) -> bool {
    source
        .lines()
        .map(str::trim)
        .any(|line| !line.is_empty() && !line.starts_with("//"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn neither_generated_template_counts_as_filled_in() {
        assert!(!has_statements(TEMPLATE));
        assert!(!has_statements(ROLLBACK_TEMPLATE));
    }

    #[test]
    fn recognises_scripts_that_do_something() {
        let cases = [
            ("db.users.dropIndex(\"email_1\");", true),
            ("// just a comment", false),
            ("", false),
            ("   \n\t\n  ", false),
            ("// explanation\ndb.users.drop();", true),
            ("  db.users.drop();  ", true),
            ("//db.users.drop();", false),
        ];

        for (source, expected) in cases {
            assert_eq!(has_statements(source), expected, "for {source:?}");
        }
    }

    #[test]
    fn generates_five_digit_pairs() {
        assert_eq!(
            migration_file_name(1, "create_users"),
            "00001_create_users.up.js"
        );
        assert_eq!(
            rollback_file_name(1, "create_users"),
            "00001_create_users.down.js"
        );
        assert_eq!(migration_file_name(42000, "x"), "42000_x.up.js");
    }

    #[test]
    fn parses_well_formed_file_names() {
        let cases = [
            ("00001_create_email_index.up.js", 1, "create_email_index"),
            ("1_x.up.js", 1, "x"),
            ("00042_backfill_status.up.js", 42, "backfill_status"),
            (
                "0010_name_with_underscores.up.js",
                10,
                "name_with_underscores",
            ),
        ];

        for (file_name, version, name) in cases {
            assert_eq!(
                parse(file_name),
                Some((version, name)),
                "parsing {file_name}"
            );
        }
    }

    #[test]
    fn rejects_anything_that_is_not_a_migration() {
        let cases = [
            "00001_create_email_index.down.js", // a rollback file, not a migration
            "00001_no_direction.js",
            "_no_version.up.js",
            "abc_not_a_number.up.js",
            "-1_negative.up.js",
            "99999999999_overflows_u32.up.js",
            "00001_wrong_extension.up.txt",
            "00001_trailing.up.js.bak",
            "+1_leading_plus.up.js",
            "00001_.up.js",
            "00001.up.js", // no name at all
            "README.md",
            "",
        ];

        for file_name in cases {
            assert_eq!(parse(file_name), None, "should reject {file_name}");
        }
    }
}
