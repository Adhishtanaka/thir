#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")"; source ./env.sh
if [ "${1:-}" = gui ]; then   # window + keyboard for den; the serial shell stays on this terminal
  shift; echo "thir: den in the QEMU window (Alt+Enter splits). Ctrl-A X in this terminal quits QEMU" >&2
  exec "${QEMU[@]}" -display cocoa -serial mon:stdio "$@"
fi
echo "thir: Ctrl-A X quits QEMU" >&2
exec "${QEMU[@]}" -nographic "$@"
