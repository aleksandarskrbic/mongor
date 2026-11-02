# Development

How to build `mongor`, run a local MongoDB, and exercise the CLI by hand.

## Prerequisites

| Tool | Why | Check |
| --- | --- | --- |
| Rust (edition 2024) | builds the CLI | `cargo --version` |
| Docker | runs a local MongoDB | `docker --version` |
| `mongosh` | executes migration scripts | `mongosh --version` |

`mongosh` is resolved from `PATH` — `Command::new("mongosh")` lets the OS do the
lookup. If it is missing, install it from
<https://www.mongodb.com/docs/mongodb-shell/install/>. A managed install under
`~/.mongor/bin` (`mongor setup mongosh`) is a planned feature, not implemented.

## Build and check

```sh
cargo check                 # type- and borrow-check only; fastest inner loop
cargo build                 # produces target/debug/mongor
cargo fmt                   # format (CI will enforce this)
cargo clippy -- -D warnings # lints
cargo test                  # unit + integration tests
```

Run the CLI through Cargo. Note the `--`, which separates Cargo's own flags
from `mongor`'s arguments:

```sh
cargo run -- --help
cargo run -- --version
cargo run -- help new
```

## Local MongoDB

```sh
docker compose up -d            # start
docker compose ps               # confirm it is healthy
docker compose logs -f mongo    # tail logs
docker compose down             # stop and discard all data
```

Then point `mongor` at it. The config file never holds credentials — it holds
the *name* of the variable that does:

```sh
export MONGO_DEV_URI="mongodb://localhost:27017"
```

Poke at the database directly:

```sh
mongosh "mongodb://localhost:27017/myapp_dev"
# show collections
# db.mongor_migrations.find()
```

## Testing the CLI by hand

`cargo run` executes the binary with **your current working directory**, and
Cargo finds `Cargo.toml` by walking up. So work inside `sandbox/` (gitignored)
to keep scaffolding out of the repo root:

```sh
./scripts/sandbox-reset.sh && cd sandbox
```

Everything below assumes you are in `sandbox/`.

### `mongor init`

```sh
cargo run -- init                              # mongor.toml + migrations/
cargo run -- init                              # again: skips, never overwrites
cargo run -- init --migrations-dir db/mongo    # custom location
cargo run -- init --help
```

Flag names are kebab-case: the field `migrations_dir` becomes
`--migrations-dir`. `--migrations_dir` and `-migrations_dir` are both errors.

### `mongor new <name>`

Reads `[migrations] path` from `mongor.toml`, scans for the highest existing
`.up.js` version, and writes the next pair.

```sh
cargo run -- new create_email_index     # -> 00001_create_email_index.{up,down}.js
cargo run -- new add_status_field       # -> 00002_add_status_field.{up,down}.js
ls migrations/
```

Both directions are written every time. A `.down.js` left as generated counts
as "no rollback": `mongor rollback` refuses it instead of running a no-op.

Rejected input (creates nothing, exits non-zero):

```sh
cargo run -- new "bad name"             # spaces
cargo run -- new ../../etc/passwd       # path traversal
```

Version scanning is numeric, not lexicographic:

```sh
touch migrations/00009_manual.up.js
cargo run -- new after_manual           # -> 00010_after_manual.up.js, not 00003
```

### `mongor status`, `migrate`, `rollback`, `baseline`

These need a database, so use the scripts below rather than setting the
sandbox up by hand.

## Sandbox scripts

```sh
./scripts/sandbox-reset.sh     # drop the database, wipe sandbox/, mongor init
./scripts/sandbox-seed.sh      # three migrations, two with rollbacks, plus data
source scripts/sandbox-env.sh  # just export MONGO_DEV_URI in your own shell
```

`sandbox-reset.sh` scaffolds with the real `mongor init`, so the sandbox config
is whatever `templates/mongor.toml` currently produces. `sandbox-seed.sh`
creates its files with `mongor new`, for the same reason.

Settings live in `scripts/sandbox-env.sh` and can be overridden by exporting
them first:

| Variable | Default | Note |
| --- | --- | --- |
| `MONGO_HOST` | `mongodb://localhost:27017` | no database in the path, on purpose |
| `SANDBOX_DB` | `myapp_dev` | must match `templates/mongor.toml` |

## End-to-end smoke test

```sh
docker compose up -d
./scripts/sandbox-reset.sh
./scripts/sandbox-seed.sh

cd sandbox
export MONGO_DEV_URI="mongodb://localhost:27017"
../target/debug/mongor status --env dev      # three pending
../target/debug/mongor migrate --env dev     # applies all three
../target/debug/mongor rollback --env dev    # refused: 0003 has no rollback
```

## Resetting state

```sh
./scripts/sandbox-reset.sh     # database + sandbox files
docker compose down            # stop MongoDB and discard everything
cargo clean                    # wipe build artifacts
```

## Troubleshooting

**`unexpected argument '--migrations_dir' found`** — use kebab-case,
`--migrations-dir`.

**`failed to read mongor.toml`** — you are not in a directory that has been
`init`ed. `cd sandbox` first.

**`Error: ... Connection refused`** — `docker compose up -d`, and check
`docker compose ps`.

**`mongosh: command not found`** — not on `PATH`; see Prerequisites.

**Scaffolded files appear in the repo root** — you ran `cargo run -- init` from
the repo root instead of `sandbox/`. `cargo run` uses your shell's cwd.
