use clap::{Args, Parser, Subcommand};
use std::path::PathBuf;

// `about` pulls the description from Cargo.toml, so --help and crates.io agree.
#[derive(Parser)]
#[command(name = "mongor", version, about)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Subcommand)]
pub enum Commands {
    /// Create mongor.toml and a migrations directory
    Init(InitArgs),

    /// Create a new migration file
    New(NewArgs),

    /// Show applied and pending migrations
    Status(StatusArgs),

    /// Apply all pending migrations
    Migrate(MigrateArgs),

    /// Adopt a database that already has the schema, without running anything
    Baseline(BaselineArgs),

    /// Undo the most recently applied migration
    Rollback(RollbackArgs),
}

#[derive(Args)]
pub struct InitArgs {
    /// Directory that will hold migration files
    #[arg(long, default_value = "migrations")]
    pub migrations_dir: PathBuf,
}

#[derive(Args)]
pub struct NewArgs {
    /// Name of the migration, e.g. create_email_index
    pub name: String,
}

#[derive(Args)]
pub struct StatusArgs {
    /// Environment from mongor.toml to target
    #[arg(long)]
    pub env: String,
}

#[derive(Args)]
pub struct MigrateArgs {
    /// Environment from mongor.toml to target
    #[arg(long)]
    pub env: String,

    /// Apply without asking for confirmation
    #[arg(long)]
    pub yes: bool,
}

#[derive(Args)]
pub struct BaselineArgs {
    /// Environment from mongor.toml to target
    #[arg(long)]
    pub env: String,

    /// Highest version to assume applied; defaults to the newest file on disk
    #[arg(long)]
    pub version: Option<u32>,
}

#[derive(Args)]
pub struct RollbackArgs {
    /// Environment from mongor.toml to target
    #[arg(long)]
    pub env: String,
}
