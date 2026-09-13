# mongor

A standalone MongoDB migration CLI, written in Rust and inspired by Flyway and Goose.

> **Status: early development.** The CLI skeleton parses commands; none of them do
> real work yet. Not usable, not published to crates.io. Follow along if you like,
> but don't point it at a database.

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

Forward migrations are `V`-prefixed, rollbacks are `U`-prefixed and optional.

```
migrations/
├── V001__create_email_index.js
├── U001__create_email_index.js
└── V002__backfill_status.js
```

```js
// V001__create_email_index.js
await db.users.createIndex({ email: 1 }, { unique: true });
```

```js
// U001__create_email_index.js
await db.users.dropIndex("email_1");
```

## Configuration

```toml
# mongor.toml
default_env = "dev"

[migrations]
path = "migrations"
collection = "_mongor_migrations"

[environments.dev]
uri_env = "MONGO_DEV_URI"
database = "myapp_dev"

[environments.prod]
uri_env = "MONGO_PROD_URI"
database = "myapp"
require_confirm = true
```

Credentials never live in the config file — only the *name* of the environment
variable holding the connection URI.

```sh
export MONGO_PROD_URI="mongodb://..."
```

## Commands

| Command | Description | Status |
| --- | --- | --- |
| `mongor init` | Create `mongor.toml` and `migrations/` | planned |
| `mongor new <name>` | Create the next `V<nnn>__<name>.js` | planned |
| `mongor status` | Compare local files against applied history | planned |
| `mongor migrate` | Apply pending migrations | planned |
| `mongor rollback` | Run the matching `U` file for the latest migration | planned |

## Design notes

MongoDB migrations are **not atomic**. A failed `updateMany` may have modified
some documents. Mongor therefore never assumes *process failed == database
unchanged*; migration state is tracked explicitly so a partially applied
migration can be detected rather than blindly retried.

Applied migrations are immutable. If a file's SHA-256 checksum no longer matches
what was recorded when it ran, Mongor refuses to migrate and tells you which
migration was modified.

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
