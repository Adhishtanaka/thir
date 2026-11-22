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
# THIR_ARCH=x86_64 builds natively inside an emulated linux/amd64 container (hours), in its own volume and image; the arm64 volume is never touched
if [ "${THIR_ARCH:-aarch64}" = x86_64 ]; then V=thir-web-build-x86_64; PLAT=linux/amd64; else V=thir-web-build; PLAT=linux/arm64; fi
# build the image only if missing (a later alpine:3.24 pull of another arch would otherwise rebuild it with different packages)
[ "$(docker image inspect $V -f '{{.Architecture}}' 2>/dev/null)" = ${PLAT#*/} ] || docker build -q --platform $PLAT -t $V -f docker/web.Dockerfile docker >/dev/null
docker volume create $V >/dev/null
run() { docker run --rm --platform $PLAT -v $V:/build -v "$PWD/scripts:/scripts:ro" -v "$PWD:/work" -e WPE_VER=$WPE_VER -e WPE_SHA=$WPE_SHA -e FFMPEG_VER=$FFMPEG_VER \
  -e JOBS="${JOBS:-2}" -e FORCE="${FORCE:-}" "$@"; }
case "${1:-all}" in
  addon) run $V sh /scripts/build-web-inner.sh all
         run $V sh /scripts/build-web-inner.sh thirweb
         run $V sh /scripts/stage-web.sh
         docker run --rm -v $V:/build:ro -v "$2:/out" $V cp /build/web.sqsh /out/web.sqsh ;;
  *) run $V sh /scripts/build-web-inner.sh "${1:-all}" ;;
esac
