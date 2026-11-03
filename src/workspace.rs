use crate::config;
use crate::mongo::Store;
use crate::mongosh::Mongosh;
use anyhow::Result;
use std::path::PathBuf;

/// The `mongor.toml` values a command needs, and the connection they resolve to.
///
/// Five commands turn `--env` into a `Store` the same way: load the file, look
/// the environment up, read the URI out of the process environment, connect.
/// Passing the wrong collection to the last step would write history where
/// nobody looks for it, so the sequence is spelled out once here instead of in
/// each command.
///
/// `validate` stays out deliberately: under `--offline` it must neither connect
/// nor demand an environment, so its setup is a different shape rather than this
/// one repeated. `new` touches no database at all.
pub struct Workspace {
    /// For `migrate` and `rollback`, which hand it to mongosh: that is a child
    /// process and opens its own connection rather than sharing the store's.
    /// The other three callers drop it.
    pub uri: String,
    pub store: Store,
    pub migrations_dir: PathBuf,
}

impl Workspace {
    /// Resolves `env` against `mongor.toml` and connects to its database.
    ///
    /// Commands that go on to run a script want `open_with_mongosh` instead.
    pub async fn open(env: &str) -> Result<Workspace> {
        let cfg = config::load()?;
        let environment = cfg.environment(env)?;
        let uri = config::resolve_uri(environment)?;

        let store = Store::connect(&uri, &environment.database, &cfg.migrations.collection).await?;

        Ok(Workspace {
            uri,
            store,
            migrations_dir: cfg.migrations.path,
        })
    }

    /// Same as `open`, and also resolves the mongosh the caller cannot run without.
    ///
    /// Two constructors rather than one with a flag because what distinguishes
    /// them is the order three failures are reported in, and that order is
    /// easier to keep right when it is written out. A missing `mongor.toml`, an
    /// unknown environment and an unset URI variable all still come first: they
    /// are what the operator asked about.
    pub async fn open_with_mongosh(env: &str) -> Result<(Workspace, Mongosh)> {
        let cfg = config::load()?;
        let environment = cfg.environment(env)?;
        let uri = config::resolve_uri(environment)?;

        // Unlike `status`, a missing mongosh is fatal here, so resolve it before
        // opening a connection we would only throw away. `migrate --dry-run`
        // resolves it too: a plan that ignores a precondition of the run is not
        // a plan.
        let mongosh = Mongosh::resolve().await?;
        let store = Store::connect(&uri, &environment.database, &cfg.migrations.collection).await?;

        let workspace = Workspace {
            uri,
            store,
            migrations_dir: cfg.migrations.path,
        };

        Ok((workspace, mongosh))
    }
}
