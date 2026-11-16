#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")"; source ./env.sh
docker build --platform "$DOCKER_PLATFORM" --build-arg RUST_TARGET="$RUST_TARGET" -t netos-build docker
mkdir -p out
docker run --rm --platform "$DOCKER_PLATFORM" -e NETOS_ARCH="$NETOS_ARCH" \
  -v "$PWD":/work -v netos-build:/build -w /work netos-build scripts/build-in-docker.sh "${1:-all}"
