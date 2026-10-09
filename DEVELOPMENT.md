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
# db._mongor_migrations.find()
```

## Testing the CLI by hand

`cargo run` executes the binary with **your current working directory**, and
Cargo finds `Cargo.toml` by walking up. So work inside `sandbox/` (gitignored)
to keep scaffolding out of the repo root:

```sh
mkdir -p sandbox && cd sandbox
cargo run -- init
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
`V` version, and writes the next one.

```sh
cargo run -- new create_email_index     # -> V0001__create_email_index.js
cargo run -- new add_status_field       # -> V0002__add_status_field.js
ls migrations/
```

Rejected input (creates nothing, exits non-zero):

```sh
cargo run -- new "bad name"             # spaces
cargo run -- new ../../etc/passwd       # path traversal
```

Version scanning is numeric, not lexicographic:

```sh
touch migrations/V0009__manual.js
cargo run -- new after_manual           # -> V0010__after_manual.js, not V0003
```

### `mongor status` / `mongor migrate`

Stubs. They print their own name and do nothing yet.

```sh
cargo run -- status
cargo run -- migrate
```

## End-to-end smoke test

From the repo root, a clean run of everything that currently works:

```sh
docker compose up -d
export MONGO_DEV_URI="mongodb://localhost:27017"

rm -rf sandbox && mkdir -p sandbox && cd sandbox
cargo run -- init
cargo run -- new create_email_index
cargo run -- new backfill_status
ls -la migrations/
cat mongor.toml
cd ..
```

## Resetting state

```sh
docker compose down            # wipe the database
rm -rf sandbox                 # wipe scaffolded files
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
