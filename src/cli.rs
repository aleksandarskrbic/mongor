use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "mongor", version, about)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Subcommand)]
pub enum Commands {
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
