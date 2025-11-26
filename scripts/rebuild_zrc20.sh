#!/usr/bin/env bash
set -euo pipefail

echo "Stopping zord service"
# placeholder for stopping service

DB_PATH=${DB_PATH:-/var/lib/zord}

rm -f "$DB_PATH"/tokens.redb "$DB_PATH"/balances.redb "$DB_PATH"/transfer_inscriptions.redb

# placeholder for restart
