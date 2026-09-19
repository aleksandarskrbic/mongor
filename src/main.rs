use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use std::fs;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};

const CONFIG_FILE: &str = "mongor.toml";
const CONFIG_TEMPLATE: &str = include_str!("../templates/mongor.toml");

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

fn main() -> Result<()> {
    let cli = Cli::parse();

    match &cli.command {
        Commands::Init { migrations_dir } => init(migrations_dir)?,
        Commands::New { name } => println!("new: {name}"),
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
