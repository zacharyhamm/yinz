FROM debian:trixie-slim

RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        build-essential ca-certificates clang cmake curl git libssl-dev \
        mold pkg-config protobuf-compiler python3 zlib1g-dev \
    && rm -rf /var/lib/apt/lists/*

ENV RUSTUP_HOME=/usr/local/rustup CARGO_HOME=/usr/local/cargo
ENV PATH=/usr/local/cargo/bin:$PATH
RUN curl --proto '=https' --tlsv1.2 -fsSL https://sh.rustup.rs | sh -s -- \
        -y --profile minimal --default-toolchain stable \
        --component rustfmt,clippy \
    && chmod -R a+rX /usr/local/rustup /usr/local/cargo
