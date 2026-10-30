use anyhow::{Context, Result};
use std::fs;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;

use crate::config;

/// Creates `mongor.toml` and the migrations directory.
pub struct Init {
    migrations_dir: PathBuf,
}

impl Init {
    pub fn new(migrations_dir: PathBuf) -> Self {
        Self { migrations_dir }
    }

    pub fn run(&self) -> Result<()> {
        self.write_config()?;
        self.create_migrations_dir()
    }

    fn write_config(&self) -> Result<()> {
        let migrations_path = self
            .migrations_dir
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
                Ok(())
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                println!("skipped {} already exists", config::FILE_NAME);
                Ok(())
            }
            Err(e) => Err(e).with_context(|| format!("failed to create {}", config::FILE_NAME)),
        }
    }

    fn create_migrations_dir(&self) -> Result<()> {
        if self.migrations_dir.is_dir() {
            println!("skipped {}/ already exists", self.migrations_dir.display());
            return Ok(());
        }

        fs::create_dir_all(&self.migrations_dir)
            .with_context(|| format!("failed to create {}", self.migrations_dir.display()))?;
        println!("created {}/", self.migrations_dir.display());

        Ok(())
    }
}
