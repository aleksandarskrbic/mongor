mod cli;
mod config;
mod migration;

use anyhow::{Context, Result};
use clap::Parser;
use std::fs;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::Path;

use crate::cli::{Cli, Commands};

// cargo run -- init --migrations-dir migrations
// cargo run -- new migration_name
// cargo run -- status
fn main() -> Result<()> {
    let cli = Cli::parse();

    match &cli.command {
        Commands::Init { migrations_dir } => init(migrations_dir)?,

        Commands::New { name } => {
            let cfg = config::load()?;
            migration::create(name, &cfg.migrations.path)?;
        }

        Commands::Status => {
            let cfg = config::load()?;
            let migrations = migration::discover(&cfg.migrations.path)?;

            if migrations.is_empty() {
                println!("no migrations found in {}", cfg.migrations.path.display());
            }
            for m in &migrations {
                println!("{:04}  {}", m.version, m.name);
            }
        }

        Commands::Migrate => println!("migrate"),
    }

    Ok(())
}

fn init(migrations_dir: &Path) -> Result<()> {
    let migrations_path = migrations_dir
        .to_str()
        .context("migrations directory path must be valid UTF-8")?;
    let contents = config::TEMPLATE.replace("{{migrations_path}}", migrations_path);

    match OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(config::FILE_NAME)
    {
        Ok(mut file) => {
            file.write_all(contents.as_bytes())
                .with_context(|| format!("failed to write {}", config::FILE_NAME))?;
            println!("created {}", config::FILE_NAME);
        }
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            println!("skipped {} already exists", config::FILE_NAME);
        }
        Err(e) => {
            return Err(e).with_context(|| format!("failed to create {}", config::FILE_NAME));
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
