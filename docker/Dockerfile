FROM rust:1.99-bookworm
RUN apt-get update && apt-get install -y --no-install-recommends bc bison flex libssl-dev libelf-dev xz-utils gcc-x86-64-linux-gnu e2fsprogs \
 && rm -rf /var/lib/apt/lists/*
RUN rustup target add aarch64-unknown-linux-musl x86_64-unknown-linux-musl
