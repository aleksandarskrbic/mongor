use anyhow::Result;

/// Applies all pending migrations.
pub struct Migrate {
    env: String,
}

impl Migrate {
    pub fn new(env: String) -> Self {
        Self { env }
    }

    pub async fn run(&self) -> Result<()> {
        println!("migrate {}", self.env);
        Ok(())
    }
}
