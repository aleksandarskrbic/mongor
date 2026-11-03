use crate::cli;
use anyhow::Result;
use clap_complete::{Shell, generate};
use std::io;

/// Prints a completion script for one shell to stdout.
///
/// Generated from the parser itself, so a flag added anywhere in `cli` is
/// completed without anything here changing.
pub struct Completions {
    shell: Shell,
}

impl Completions {
    pub fn new(shell: Shell) -> Self {
        Self { shell }
    }

    pub fn run(&self) -> Result<()> {
        let mut command = cli::command();

        // The script hard-codes the name it completes, so it has to be the name
        // on PATH - under `cargo run` argv[0] is a path into target/.
        let bin_name = command.get_name().to_string();

        generate(self.shell, &mut command, bin_name, &mut io::stdout());

        Ok(())
    }
}
