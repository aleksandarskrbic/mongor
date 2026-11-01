use crate::{config, migration};
use anyhow::Result;
use std::path::PathBuf;

/// Creates the next migration file.
pub struct NewMigration {
    name: String,
    migrations_dir: PathBuf,
}

impl NewMigration {
    pub fn new(name: String) -> Result<Self> {
        let cfg = config::load()?;

        Ok(Self {
            name,
            migrations_dir: cfg.migrations.path,
        })
    }

    pub fn run(&self) -> Result<()> {
        migration::create(&self.name, &self.migrations_dir)
    }
}
