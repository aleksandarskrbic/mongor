# mongor

[![CI](https://github.com/aleksandarskrbic/mongor/actions/workflows/ci.yml/badge.svg)](https://github.com/aleksandarskrbic/mongor/actions/workflows/ci.yml)

A standalone MongoDB migration CLI, written in Rust and inspired by Flyway and Goose.

> **Status: early development.** All six commands work and are covered by tests
> that run against a real database. Not yet published to crates.io, no prebuilt
> binaries, and no distributed lock — concurrent runs cannot apply the same
> migration twice, but a whole run is not serialised. Try it on something you can
> throw away.

## Idea

Rust is the control plane. Your migrations are ordinary `mongosh` JavaScript files.

```
                mongor
              Rust binary
                   |
      +------------+------------+
      |                         |
MongoDB Rust driver          mongosh
      |                         |
 migration metadata      your migration JS
 history, checksums
 status
```

Mongor does **not** ship a JavaScript runtime or invent a MongoDB scripting DSL.
It tracks state, validates checksums, and shells out to `mongosh` to run your
scripts. A migration file is a normal `mongosh` script that runs fine on its own.

## Migrations

Migrations are `<version>_<name>.up.js`, rollbacks `<version>_<name>.down.js`.
`mongor new` writes both, so a missing rollback is visible in review. A
`.down.js` that is only comments means the migration cannot be undone, and
`mongor rollback` refuses it rather than reporting a rollback that did nothing.

```
migrations/
├── 00001_create_email_index.down.js
├── 00001_create_email_index.up.js
└── 00002_backfill_status.up.js
```

The version leads the name, so a migration and its rollback sort next to each
other however many you have.

```js
// 00001_create_email_index.up.js
db.users.createIndex({ email: 1 }, { unique: true });
```

```js
// 00001_create_email_index.down.js
db.users.dropIndex("email_1");
```

Scripts are ordinary mongosh JavaScript, executed with `db` already connected to
the target environment's database. Do not use `await`: mongosh parses a
`--file` script as a script rather than a module, so top-level `await` is a
syntax error, and the shell API is awaited for you.

## Configuration

```toml
# mongor.toml
default_env = "dev"

[migrations]
path = "migrations"
collection = "mongor_migrations"

[environments.dev]
uri_env = "MONGO_DEV_URI"
database = "myapp_dev"

[environments.prod]
uri_env = "MONGO_PROD_URI"
database = "myapp"
```

Credentials never live in the config file — only the *name* of the environment
variable holding the connection URI.

```sh
export MONGO_PROD_URI="mongodb://..."
```

## Commands

| Command | Description | Status |
| --- | --- | --- |
| `mongor init` | Create `mongor.toml` and `migrations/` | yes |
| `mongor new <name>` | Create the next `.up.js` and its `.down.js` | yes |
| `mongor status` | Compare local files against applied history | yes |
| `mongor migrate` | Apply pending migrations (`--to` to cap, `--yes` to skip the prompt) | yes |
| `mongor rollback` | Run the matching `.down.js` for the latest migration (`--yes` to skip the prompt) | yes |
| `mongor baseline` | Adopt a database that already has the schema | yes |
| `mongor resolve <version>` | Clear or accept the row left by a failed migration | yes |

`mongor migrate` and `mongor rollback` print what they are about to do and then
ask you to type `yes`, the way `terraform apply` does. In a pipeline, where
there is nobody to answer, they refuse rather than hanging — pass `--yes` to run
unattended:

```sh
mongor migrate --env prod             # prints the plan, waits for `yes`
mongor migrate --env prod --yes       # applies straight away, for CI
mongor rollback --env prod --yes
mongor migrate --env prod --to 42 --yes   # stops after 00042, for a staged rollout
```

`--to` is a one-shot target, not a desired state: it must name a migration that
is actually pending, so a capped run can never exit 0 having applied nothing. A
typo like `--to 1000` is refused rather than quietly applying everything.

When a migration fails or its process is lost, the row it left behind blocks
`migrate` and `rollback` until a human looks at the database. `mongor resolve`
is how you unblock it, once you know what the script managed to do:

```sh
mongor resolve 3 --env prod --retry      # forget the attempt; migrate runs it again
mongor resolve 3 --env prod --applied    # you finished the work by hand
```

`resolve` never runs a script. `--retry` does not undo what the failed script
already changed, and `--applied` does not run anything — both only edit mongor's
own record of what happened. It refuses rows that are not unfinished: an applied
migration is history, and `rollback` is the command for undoing one.

## Design notes

MongoDB migrations are **not atomic**. A failed `updateMany` may have modified
some documents. Mongor therefore never assumes *process failed == database
unchanged*; migration state is tracked explicitly so a partially applied
migration can be detected rather than blindly retried.

Applied migrations are immutable. If a file's SHA-256 checksum no longer matches
what was recorded when it ran, Mongor refuses to migrate and tells you which
migration was modified.

The history collection carries a unique index on `version`, and the row is
written *before* the script runs. That insert is the lock: two `mongor migrate`
runs cannot both apply the same migration, because the second one's insert is
rejected and the run stops. It does not serialise a whole run — two runs can
still apply *different* pending migrations at the same time — so a distributed
lock is still worth having.

## Requirements

- Rust (edition 2024)
- `mongosh` on your `PATH`

## Building

```sh
cargo build
cargo run -- --help
```

## License

Licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
- MIT license ([LICENSE-MIT](LICENSE-MIT))

at your option.

Unless you explicitly state otherwise, any contribution intentionally submitted
for inclusion in this work by you, as defined in the Apache-2.0 license, shall
be dual licensed as above, without any additional terms or conditions.
