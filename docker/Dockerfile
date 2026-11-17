FROM rust:1.99-bookworm
RUN apt-get update && apt-get install -y --no-install-recommends bc bison flex libssl-dev libelf-dev xz-utils gcc-x86-64-linux-gnu libc6-dev-amd64-cross e2fsprogs dosfstools mtools fdisk \
 && rm -rf /var/lib/apt/lists/*
RUN rustup target add aarch64-unknown-linux-musl x86_64-unknown-linux-musl
