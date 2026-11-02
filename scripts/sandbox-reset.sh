#!/usr/bin/env bash
# Wipes the sandbox directory and its database, then scaffolds a fresh project.
set -euo pipefail

source "$(dirname "${BASH_SOURCE[0]}")/sandbox-env.sh"
sandbox_guard

echo "==> building"
cargo build -q --manifest-path "$ROOT/Cargo.toml"

echo "==> dropping database $SANDBOX_DB"
mongo_eval "db.getSiblingDB(\"$SANDBOX_DB\").dropDatabase()" >/dev/null

echo "==> clearing $SANDBOX"
rm -rf "$SANDBOX"
mkdir -p "$SANDBOX"

echo "==> mongor init"
cd "$SANDBOX" && "$MONGOR" init

cat <<EOF

sandbox ready: $SANDBOX
  database     $SANDBOX_DB
  MONGO_DEV_URI=$MONGO_DEV_URI

next:
  ./scripts/sandbox-seed.sh          # three migrations with rollbacks
  cd sandbox && export MONGO_DEV_URI="$MONGO_DEV_URI"
EOF
