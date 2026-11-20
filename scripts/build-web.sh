#!/bin/sh
# P0 spike: build WPE WebKit + ffmpeg in the web.Dockerfile image. Output goes to the thir-web-build volume (/build), never the repo.
# Usage: scripts/build-web.sh [webkit|ffmpeg|all|addon OUTDIR]   (run on the host; builds the image first)
#   all    = ffmpeg + WebKit, skipped when the volume's build stamp matches (versions + build-web-inner.sh); FORCE=1 rebuilds
#   thirweb = just the browser program and the sandbox shim (web/), after a change there
#   addon  = all + thirweb, then stage the add-on (with Mesa) and copy web.sqsh to OUTDIR
set -eu
cd "$(dirname "$0")/.."
WPE_VER=2.54.3
WPE_SHA=787afd32ae23eac061621465390f44a462e874a9ebaba0b65521d7237fa8a5e4
FFMPEG_VER=7.1.1
docker build -q -t thir-web-build -f docker/web.Dockerfile docker >/dev/null
docker volume create thir-web-build >/dev/null
run() { docker run --rm -v thir-web-build:/build -v "$PWD/scripts:/scripts:ro" -v "$PWD:/work" -e WPE_VER=$WPE_VER -e WPE_SHA=$WPE_SHA -e FFMPEG_VER=$FFMPEG_VER \
  -e JOBS="${JOBS:-2}" -e FORCE="${FORCE:-}" "$@"; }
case "${1:-all}" in
  addon) run thir-web-build sh /scripts/build-web-inner.sh all
         run thir-web-build sh /scripts/build-web-inner.sh thirweb
         run thir-web-build sh /scripts/stage-web.sh
         docker run --rm -v thir-web-build:/build:ro -v "$2:/out" thir-web-build cp /build/web.sqsh /out/web.sqsh ;;
  *) run thir-web-build sh /scripts/build-web-inner.sh "${1:-all}" ;;
esac
