#!/usr/bin/env bash
# Fills the sandbox with migrations worth running. Assumes sandbox-reset.sh ran.
set -euo pipefail

source "$(dirname "${BASH_SOURCE[0]}")/sandbox-env.sh"
sandbox_guard

if [ ! -f "$SANDBOX/mongor.toml" ]; then
    echo "no sandbox yet - run ./scripts/sandbox-reset.sh first" >&2
    exit 1
fi

cd "$SANDBOX"

# Generated through `mongor new` rather than written by hand, so seeding
# exercises the same code path a user does.
"$MONGOR" new create_email_index
"$MONGOR" new backfill_status
"$MONGOR" new drop_legacy_field

cat > migrations/00001_create_email_index.up.js <<'JS'
db.users.createIndex({ email: 1 }, { unique: true });
JS
cat >> migrations/00001_create_email_index.down.js <<'JS'
db.users.dropIndex("email_1");
JS

cat > migrations/00002_backfill_status.up.js <<'JS'
db.users.updateMany({ status: { $exists: false } }, { $set: { status: "active" } });
JS
cat >> migrations/00002_backfill_status.down.js <<'JS'
db.users.updateMany({ status: "active" }, { $unset: { status: "" } });
JS

# Left as generated: an irreversible migration, which `rollback` must refuse.
cat > migrations/00003_drop_legacy_field.up.js <<'JS'
db.users.updateMany({}, { $unset: { legacy: "" } });
JS

# Something for the migrations to act on.
mongo_eval "db.getSiblingDB(\"$SANDBOX_DB\").users.insertMany([
    { email: \"a@example.com\", legacy: 1 },
    { email: \"b@example.com\", legacy: 2 }
])" >/dev/null

cat <<EOF

seeded 3 migrations in $SANDBOX/migrations
  00001  create_email_index   rollback: yes
  00002  backfill_status      rollback: yes
  00003  drop_legacy_field    rollback: none (.down.js left as generated)

try:
  cd sandbox
  export MONGO_DEV_URI="$MONGO_DEV_URI"
  $MONGOR status --env dev
  $MONGOR migrate --env dev
  $MONGOR rollback --env dev     # refused: 00003 has no rollback
EOF
