#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")"; source ./env.sh
# browser and media apps (web.sqsh): build WebKit/ffmpeg if stale (cached in the thir-web-build[-x86_64] volume), stage it with Mesa, copy web.sqsh to $OUT
if [ -n "$THIR_WEB" ] && [ "${1:-all}" != test ] && ! { [ "${1:-all}" = data ] && [ -f "$OUT/web.sqsh" ]; }; then mkdir -p "$OUT"; scripts/build-web.sh addon "$PWD/$OUT"; fi
docker build --platform "$DOCKER_PLATFORM" -t thir-build docker
mkdir -p "$OUT"
# the Mac's UTC offset becomes thir's default time zone, e.g. +0530 -> +05:30
# VirtualBox ARM screen size (kernel cmdline video=): the Mac's full-screen size, so the VM fills it 1:1
if [ "$THIR_FLAVOR" = vbox ]; then VM_RES=${VM_RES:-1920x1080}; else VM_RES=${VM_RES:-$(scripts/screen-res.sh 1)}; fi
THIR_TZ=${THIR_TZ:-$(date +%z | sed -E 's/^([+-][0-9]{2})([0-9]{2})$/\1:\2/')}
docker run --rm --platform "$DOCKER_PLATFORM" -e THIR_ARCH="$THIR_ARCH" -e THIR_FLAVOR="$THIR_FLAVOR" -e THIR_SMALL="${THIR_SMALL:-}" -e THIR_VIVID="${THIR_VIVID:-}" -e THIR_TZ="$THIR_TZ" -e VM_RES="$VM_RES" \
  -e DATA_IMG="$DATA_IMG" -e DATA_SIZE="$DATA_SIZE" \
  -v "$PWD":/work -v thir-build:/build -w /work thir-build scripts/build-in-docker.sh "${1:-all}"
