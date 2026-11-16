#!/usr/bin/env bash
# Prints the Mac's usable screen size in device pixels: full screen (safe area, below the notch) or, with "0", a window.
# 1 guest px = 1 Retina px when a VM's screen has exactly this size. Width rounded down to 8: drivers round it up otherwise.
osascript -l JavaScript -e 'function run(argv) { ObjC.import("AppKit"); const s=$.NSScreen.mainScreen, b=s.backingScaleFactor, f=s.frame, i=s.safeAreaInsets, v=s.visibleFrame, full=argv[0]!="0"; const w=full?f.size.width-i.left-i.right:v.size.width, h=full?f.size.height-i.top-i.bottom:v.size.height-28; return (Math.floor(w*b/8)*8)+"x"+Math.round(h*b) }' "${1:-1}" 2>/dev/null || echo 2560x1600
