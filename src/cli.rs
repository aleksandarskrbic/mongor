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
    New {
        /// Name of the migration, e.g. create_email_index
        name: String,
    },

    /// Show applied and pending migrations
    Status {
        /// Environment from mongor.toml to target
        #[arg(long)]
        env: String,
    },

    /// Apply all pending migrations
    Migrate {
        /// Environment from mongor.toml to target
        #[arg(long)]
        env: String,
    },
}

#[derive(Args)]
pub struct InitArgs {
    /// Directory that will hold migration files
    #[arg(long, default_value = "migrations")]
    pub migrations_dir: PathBuf,
}
