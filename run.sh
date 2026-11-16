#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")"; source ./env.sh
echo "netos: Ctrl-A X quits QEMU" >&2
exec "${QEMU[@]}" -nographic "$@"
