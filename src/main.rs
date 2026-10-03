use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use serde::Deserialize;
use std::fs;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};

const CONFIG_FILE: &str = "mongor.toml";
const CONFIG_TEMPLATE: &str = include_str!("../templates/mongor.toml");
const MIGRATION_TEMPLATE: &str = include_str!("../templates/migration.js");

#[derive(Parser)]
#[command(name = "mongor", version, about)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    Init {
        #[arg(long, default_value = "migrations")]
        migrations_dir: PathBuf,
    },
    New {
        name: String,
    },
    Status,
    Migrate,
}

#[derive(Debug, Deserialize)]
struct Config {
    migrations: MigrationsConfig,
}

#[derive(Debug, Deserialize)]
struct MigrationsConfig {
    path: PathBuf,
    collection: String,
}

// cargo run -- init --migrations-dir migrations_dir
// cargo run -- new migration_name
fn main() -> Result<()> {
    let cli = Cli::parse();

    match &cli.command {
        Commands::Init { migrations_dir } => init(migrations_dir)?,
        Commands::New { name } => {
            let config = load_config()?;
            new_migration(name, &config.migrations.path)?;
        }
        Commands::Status => println!("status"),
        Commands::Migrate => println!("migrate"),
    }

    Ok(())
}

fn init(migrations_dir: &Path) -> Result<()> {
    let migrations_path = migrations_dir
        .to_str()
        .context("migrations directory path must be valid UTF-8")?;
    let config = CONFIG_TEMPLATE.replace("{{migrations_path}}", migrations_path);

    match OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(Path::new(CONFIG_FILE))
    {
        Ok(mut file) => {
            file.write_all(config.as_bytes())
                .with_context(|| format!("failed to write {CONFIG_FILE}"))?;
            println!("created {CONFIG_FILE}");
        }
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            println!("skipped {CONFIG_FILE} already exists");
        }
        Err(e) => {
            return Err(e).with_context(|| format!("failed to create {CONFIG_FILE}"));
        }
    }

    if migrations_dir.is_dir() {
        println!("skipped {}/ already exists", migrations_dir.display());
    } else {
        fs::create_dir_all(migrations_dir)
            .with_context(|| format!("failed to create {}", migrations_dir.display()))?;
        println!("created {}/", migrations_dir.display());
    }

    Ok(())
}

fn load_config() -> Result<Config> {
    let contents = fs::read_to_string(CONFIG_FILE)
        .with_context(|| format!("failed to read {CONFIG_FILE} (run mongor init"))?;
    let config: Config =
        toml::from_str(&contents).with_context(|| format!("failed to parse {CONFIG_FILE}"))?;
    Ok(config)
}

fn parse_version(file_name: &str) -> Option<u32> {
    let rest = file_name.strip_prefix('V')?;
    let (version, _) = rest.split_once("__")?;
    version.parse().ok()
}

fn next_version(migrations_dir: &Path) -> Result<u32> {
    let mut highest = 0;

    let entries = fs::read_dir(migrations_dir)
        .with_context(|| format!("failed to read {}", migrations_dir.display()))?;

    for entry in entries {
        let entry =
            entry.with_context(|| format!("failed to read {}", migrations_dir.display()))?;

        if let Some(version) = entry.file_name().to_str().and_then(parse_version) {
            highest = highest.max(version);
        }
    }

    Ok(highest + 1)
}

fn new_migration(name: &str, migrations_dir: &Path) -> Result<()> {
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

    file.write_all(MIGRATION_TEMPLATE.as_bytes())
        .with_context(|| format!("failed to write {CONFIG_FILE}"))?;

    println!("created {}", path.display());
    Ok(())
}
