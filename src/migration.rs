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

const UP_SUFFIX: &str = ".up.js";
const DOWN_SUFFIX: &str = ".down.js";

pub fn migration_file_name(version: u32, name: &str) -> String {
    format!("{version:0VERSION_WIDTH$}_{name}{UP_SUFFIX}")
}

pub fn rollback_file_name(version: u32, name: &str) -> String {
    format!("{version:0VERSION_WIDTH$}_{name}{DOWN_SUFFIX}")
}

fn parse(file_name: &str) -> Option<(u32, &str)> {
    parse_suffixed(file_name, UP_SUFFIX)
}

pub fn parse_rollback(file_name: &str) -> Option<(u32, &str)> {
    parse_suffixed(file_name, DOWN_SUFFIX)
}

// The returned name borrows from `file_name`, not from `suffix`, and with two
// `&str` parameters the compiler cannot guess which - so say it.
fn parse_suffixed<'a>(file_name: &'a str, suffix: &str) -> Option<(u32, &'a str)> {
    let rest = file_name.strip_suffix(suffix)?;
    let (version, name) = rest.split_once('_')?;

    if version.is_empty() || name.is_empty() || !version.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }

    Some((version.parse().ok()?, name))
}

/// Returns every `<version>_<name>.up.js` in `migrations_dir`, sorted by version.
pub fn discover(migrations_dir: &Path) -> Result<Vec<Migration>> {
    let migrations = discover_suffixed(migrations_dir, UP_SUFFIX)?;

    // Only migrations: two `.up.js` at one version have no defined order, so
    // nothing can run. Two `.down.js` are merely untidy, and aborting here
    // would throw away a whole `validate` report over one stale file.
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

/// The `.down.js` half. Only `validate` discovers rollbacks: `rollback` builds
/// the name from the record, so it can undo a migration whose `.up.js` is gone.
pub fn discover_rollbacks(migrations_dir: &Path) -> Result<Vec<Migration>> {
    discover_suffixed(migrations_dir, DOWN_SUFFIX)
}

/// The `.down.js` beside a migration, found by what it parses to rather than by
/// rebuilding the name - so a hand-written `1_x.up.js` pairs with `1_x.down.js`
/// instead of a five-digit name nobody wrote.
pub fn find_rollback(migrations_dir: &Path, version: u32, name: &str) -> Result<Option<Migration>> {
    Ok(discover_rollbacks(migrations_dir)?
        .into_iter()
        .find(|r| r.version == version && r.name == name))
}

fn discover_suffixed(migrations_dir: &Path, suffix: &str) -> Result<Vec<Migration>> {
    let entries = fs::read_dir(migrations_dir)
        .with_context(|| format!("failed to read {}", migrations_dir.display()))?;

    let mut migrations = Vec::new();

    for entry in entries {
        let entry =
            entry.with_context(|| format!("failed to read {}", migrations_dir.display()))?;

        // A directory named like a script is not one, and treating it as a
        // migration turns every later read of it into an error.
        if !entry.path().is_file() {
            continue;
        }

        let os_name = entry.file_name();
        let Some(file_name) = os_name.to_str() else {
            continue;
        };

        let Some((version, name)) = parse_suffixed(file_name, suffix) else {
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
    Ok(migrations)
}

/// Names in `migrations_dir` that were plainly meant to be scripts but are not
/// ones mongor can run.
///
/// `discover` skips whatever it cannot parse, so a typo is otherwise a migration
/// that never runs and says nothing. Sorted, so the report is stable.
pub fn suspicious_file_names(migrations_dir: &Path) -> Result<Vec<String>> {
    let entries = fs::read_dir(migrations_dir)
        .with_context(|| format!("failed to read {}", migrations_dir.display()))?;

    let mut names = Vec::new();

    for entry in entries {
        let entry =
            entry.with_context(|| format!("failed to read {}", migrations_dir.display()))?;

        // A directory is not a script however it is named.
        if !entry.path().is_file() {
            continue;
        }

        let os_name = entry.file_name();
        // A name that is not UTF-8 cannot be a migration, and we could not print
        // it back either.
        let Some(file_name) = os_name.to_str() else {
            continue;
        };

        // Editor swap files and the like are not attempts at a migration, and
        // failing someone's pre-commit check over `.00001_x.up.js.swp` - a file
        // they never created - is worse than missing a real typo.
        if file_name.starts_with('.') {
            continue;
        }

        if parse(file_name).is_none()
            && parse_rollback(file_name).is_none()
            && looks_like_a_script(file_name)
        {
            names.push(file_name.to_string());
        }
    }

    names.sort();
    Ok(names)
}

/// Whether a name that failed to parse still looks like an attempt at one of our
/// scripts: a suffix that is there but mis-cased or followed by something else,
/// or a `.js` file numbered like a migration but with no direction.
fn looks_like_a_script(file_name: &str) -> bool {
    let lower = file_name.to_ascii_lowercase();

    if lower.contains(UP_SUFFIX) || lower.contains(DOWN_SUFFIX) {
        return true;
    }

    lower.ends_with(".js")
        && lower.split_once('_').is_some_and(|(version, _)| {
            !version.is_empty() && version.chars().all(|c| c.is_ascii_digit())
        })
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
    fn parses_well_formed_rollback_names() {
        assert_eq!(
            parse_rollback("00001_create_email_index.down.js"),
            Some((1, "create_email_index"))
        );
        assert_eq!(parse_rollback("00001_create_email_index.up.js"), None);
    }

    #[test]
    fn flags_names_that_only_look_like_scripts() {
        let cases = [
            "00001_x.up.js.bak",
            "00001_X.UP.JS",
            "_no_version.up.js",
            "abc_not_a_number.up.js",
            "00002_no_direction.js",
            "00001_x.down.js.orig",
        ];

        for file_name in cases {
            assert!(looks_like_a_script(file_name), "should flag {file_name}");
        }
    }

    #[test]
    fn ignores_files_that_were_never_meant_to_be_scripts() {
        for file_name in [
            "README.md",
            ".gitkeep",
            "helpers.js",
            "lib_utils.js",
            "notes.txt",
        ] {
            assert!(!looks_like_a_script(file_name), "should ignore {file_name}");
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
