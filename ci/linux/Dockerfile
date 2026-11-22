# Clean Debian bookworm + zsh 5.9 for the Rust checks. Used by scripts/test-all.mjs --docker and by linux-docker in CI.
# Same apt packages and rustup pins as the old local watt-linux-test:bookworm-rust image, but the stable toolchain.
FROM debian:bookworm

ENV DEBIAN_FRONTEND=noninteractive

RUN apt-get update \
 && apt-get install -y --no-install-recommends \
    build-essential pkg-config file ca-certificates curl wget tar \
    libwebkit2gtk-4.1-dev libjavascriptcoregtk-4.1-dev libsoup-3.0-dev libgtk-3-dev \
    librsvg2-dev libssl-dev libxdo-dev libayatana-appindicator3-dev \
    libdbus-1-dev libsecret-1-dev zsh bash procps \
 && rm -rf /var/lib/apt/lists/*

ENV RUSTUP_HOME=/usr/local/rustup \
    CARGO_HOME=/usr/local/cargo \
    PATH=/usr/local/cargo/bin:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin

# rustup-init 1.29.3, checked against the sha256 pins of the rust base image.
RUN set -eux; \
    arch="$(dpkg --print-architecture)"; \
    case "$arch" in \
        amd64) rustArch='x86_64-unknown-linux-gnu'; rustupSha256='b1c8cb14e30889e7e2999e6b69e62d9e9b9ffc0f4df8180564d306d9680e6c55' ;; \
        arm64) rustArch='aarch64-unknown-linux-gnu'; rustupSha256='9f608c142e0e38a78957d0661ba1dfaf0b6fb37184cfaa4b9a0aa57af6184282' ;; \
        *) echo >&2 "unsupported architecture: $arch"; exit 1 ;; \
    esac; \
    url="https://static.rust-lang.org/rustup/archive/1.29.3/${rustArch}/rustup-init"; \
    wget --progress=dot:giga -O rustup-init "$url"; \
    echo "${rustupSha256} *rustup-init" | sha256sum -c -; \
    chmod +x rustup-init; \
    ./rustup-init -y --no-modify-path --profile minimal --default-toolchain stable -c clippy --default-host "${rustArch}"; \
    rm rustup-init; \
    mkdir -p "$CARGO_HOME/registry"; \
    chmod -R a+w "$RUSTUP_HOME" "$CARGO_HOME"; \
    rustc --version; cargo --version; cargo clippy --version

RUN useradd -m -s /bin/bash tester \
 && mkdir -p /work /target \
 && chown tester:tester /work /target "$CARGO_HOME/registry"

# Tests run in /work, a writable copy of the mounted repo (see scripts/test-all.mjs). Target dir is a volume.
ENV CARGO_TARGET_DIR=/target
USER tester
WORKDIR /work
ENV HOME=/home/tester
