mod cli;
mod commands;
mod config;
mod migration;
mod mongo;
mod mongosh;

use anyhow::Result;
use clap::Parser;

use crate::cli::{Cli, Commands};
use crate::commands::Init;

// cargo run -- init --migrations-dir migrations
// cargo run -- new migration_name
// cargo run -- status
#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    match &cli.command {
        Commands::Init(args) => Init::new(args.migrations_dir.clone()).run()?,

        Commands::New { name } => {
            let cfg = config::load()?;
            migration::create(name, &cfg.migrations.path)?;
        }

        Commands::Status { env } => {
            let cfg = config::load()?;
            let environment = cfg.environment(env)?;
            let uri = config::resolve_uri(environment)?;

            let store =
                mongo::Store::connect(&uri, &environment.database, &cfg.migrations.collection)
                    .await?;
            let migrations = migration::discover(&cfg.migrations.path)?;
            let applied = store.applied().await?;

            println!("environment  {env}");
            println!("database     {}", store.database_name());
            match mongosh::Mongosh::resolve().await {
                Ok(m) => println!("mongosh      {}", m.version()),
                Err(e) => println!("mongosh      unavailable: {e:#}"),
            }
            println!();

            if migrations.is_empty() {
                println!("no migrations found in {}", cfg.migrations.path.display());
            } else {
                println!("{:<9} {:<32} Status", "Version", "Migration");
                for m in &migrations {
                    let status = match applied.get(&m.version) {
                        Some(record) => {
                            if m.checksum()? == record.checksum {
                                "applied"
                            } else {
                                "MODIFIED"
                            }
                        }
                        None => "pending",
                    };

                    println!(
                        "{:<9} {:<32} {}",
                        format!("{:04}", m.version),
                        m.name,
                        status
                    );
                }
            }
        }

        Commands::Migrate { env } => println!("migrate {env}"),
    }

    Ok(())
}
