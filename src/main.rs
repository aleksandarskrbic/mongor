mod cli;
mod commands;
mod config;
mod migration;
mod mongo;
mod mongosh;
mod prompt;

use anyhow::Result;
use clap::Parser;

use crate::cli::{Cli, Commands};
use crate::commands::{Baseline, Init, Migrate, NewMigration, Resolve, Rollback, Status};

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
        Commands::Migrate(args) => Migrate::new(args.env, args.to, args.yes).await?.run().await,
        Commands::Baseline(args) => Baseline::new(args.env, args.version).await?.run().await,
        Commands::Rollback(args) => Rollback::new(args.env, args.yes).await?.run().await,
        Commands::Resolve(args) => {
            Resolve::new(
                args.env,
                args.version,
                args.action.retry,
                args.action.applied,
                args.yes,
            )
            .await?
            .run()
            .await
        }
    }
}
