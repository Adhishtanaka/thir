#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")"; source ./env.sh
docker build --platform "$DOCKER_PLATFORM" -t thir-build docker
mkdir -p "$OUT"
# the Mac's UTC offset becomes thir's default time zone, e.g. +0530 -> +05:30
# VirtualBox ARM screen size (kernel cmdline video=): the Mac's full-screen size, so the VM fills it 1:1
VM_RES=${VM_RES:-$(scripts/screen-res.sh 1)}
THIR_TZ=${THIR_TZ:-$(date +%z | sed -E 's/^([+-][0-9]{2})([0-9]{2})$/\1:\2/')}
docker run --rm --platform "$DOCKER_PLATFORM" -e THIR_ARCH="$THIR_ARCH" -e THIR_TZ="$THIR_TZ" -e VM_RES="$VM_RES" \
  -e DATA_IMG="$DATA_IMG" -e DATA_SIZE="$DATA_SIZE" \
  -v "$PWD":/work -v thir-build:/build -w /work thir-build scripts/build-in-docker.sh "${1:-all}"
