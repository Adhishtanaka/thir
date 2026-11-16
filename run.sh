#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")"
if [ "${1:-}" = gui ]; then
  export GUI_FULL=${GUI_FULL:-1}
  # 1 guest px = 1 Retina px: QEMU sizes its window to guest/backingScale points, and full screen fits the safe area (notch)
  GUI_RES=${GUI_RES:-$(osascript -l JavaScript -e 'function run(argv) { ObjC.import("AppKit"); const s=$.NSScreen.mainScreen, b=s.backingScaleFactor, f=s.frame, i=s.safeAreaInsets, v=s.visibleFrame, full=argv[0]!="0"; const w=full?f.size.width-i.left-i.right:v.size.width, h=full?f.size.height-i.top-i.bottom:v.size.height-28; return Math.round(w*b)+"x"+Math.round(h*b) }' "$GUI_FULL" 2>/dev/null || echo 2560x1600)}
  export GUI_RES
fi
source ./env.sh
[ -f "$DATA_IMG" ] || ./build.sh data
if [ "${1:-}" = gui ]; then
  shift; full=; [ "$GUI_FULL" = 1 ] && full=,full-screen=on
  echo "thir: den at $GUI_RES. Option(Alt)+Enter splits (Cmd works while grabbed; Ctrl+Option+G releases). Ctrl-A X here quits" >&2
  exec "${QEMU[@]}" -display "cocoa$full" -serial mon:stdio "$@"
fi
echo "thir: Ctrl-A X quits QEMU" >&2
exec "${QEMU[@]}" -nographic "$@"
