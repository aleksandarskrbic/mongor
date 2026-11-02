# Shared settings for the sandbox scripts. Source it in your own shell too:
#
#   source scripts/sandbox-env.sh
#
# Override anything by exporting it first, e.g. SANDBOX_DB=other.

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SANDBOX="$ROOT/sandbox"
MONGOR="$ROOT/target/debug/mongor"

# No database in the path on purpose: mongor takes the database from
# mongor.toml, so a bare host URI proves the config is what decides.
MONGO_HOST="${MONGO_HOST:-mongodb://localhost:27017}"

# Must match [environments.dev] database in templates/mongor.toml, since the
# sandbox is scaffolded by the real `mongor init`.
SANDBOX_DB="${SANDBOX_DB:-myapp_dev}"

export MONGO_DEV_URI="$MONGO_HOST"

# rm -rf guard: only ever the sandbox directory inside this repo.
sandbox_guard() {
    if [ "$SANDBOX" != "$ROOT/sandbox" ]; then
        echo "refusing to touch $SANDBOX" >&2
        exit 1
    fi
}

mongo_eval() {
    mongosh "$MONGO_HOST" --quiet --eval "$1"
}
