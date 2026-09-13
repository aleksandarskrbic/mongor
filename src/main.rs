use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "mongor", version, about)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    Init,
    New {
        name: String,
    },
    Status,
    Migrate
}

fn main() {
    let cli = Cli::parse();

    match &cli.command {
        Commands::Init => println!("init"),
        Commands::New { name } => println!("new: {name}"),
        Commands::Status => println!("status"),
        Commands::Migrate => println!("migrate"),
    }
}
