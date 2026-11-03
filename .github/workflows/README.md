# Workflows

`ci.yml` runs on every push to `main` and every pull request.

| Job | What it does | Needs MongoDB |
| --- | --- | --- |
| `fmt + clippy` | `cargo fmt --all --check`, `cargo clippy --all-targets -- -D warnings` | no |
| `test` | `cargo test --all-targets` against `mongo:8` with `mongosh` installed | yes |

The integration tests in `tests/cli.rs` drive the compiled binary against a real
database. They read `MONGOR_TEST_URI` and fall back to
`mongodb://localhost:27017`, which is also what `docker compose up -d` gives you
locally — so the same `cargo test` runs in both places.
