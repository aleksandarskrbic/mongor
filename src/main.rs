mod cli;
mod commands;
mod config;
mod migration;
mod mongo;
mod mongosh;

use anyhow::Result;
use clap::Parser;

use crate::cli::{Cli, Commands};
use crate::commands::{Init, Migrate, NewMigration, Status};

// cargo run -- init --migrations-dir migrations
// cargo run -- new migration_name
// cargo run -- status
// cargo run -- --help
#[tokio::main]
async fn main() -> Result<()> {
    match Cli::parse().command {
        Commands::Init(args) => Init::new(args.migrations_dir).run(),
        Commands::New(args) => NewMigration::new(args.name)?.run(),
        Commands::Status(args) => Status::new(args.env).await?.run().await,
        Commands::Migrate(args) => Migrate::new(args.env).run().await,
    }
}
