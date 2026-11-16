#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")"; source ./env.sh
docker build --platform "$DOCKER_PLATFORM" -t thir-build docker
mkdir -p "$OUT"
docker run --rm --platform "$DOCKER_PLATFORM" -e THIR_ARCH="$THIR_ARCH" \
  -e DATA_IMG="$DATA_IMG" -e DATA_SIZE="$DATA_SIZE" \
  -v "$PWD":/work -v thir-build:/build -w /work thir-build scripts/build-in-docker.sh "${1:-all}"
