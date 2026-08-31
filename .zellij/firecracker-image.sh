#!/bin/sh
set -eu

export DEBIAN_FRONTEND=noninteractive
apt-get update
apt-get install -y --no-install-recommends \
    build-essential ca-certificates curl git libssl-dev mold musl-tools \
    nodejs npm openssh-client openssh-server pkg-config protobuf-compiler \
    ripgrep rsync

curl --proto '=https' --tlsv1.2 -fsSL https://sh.rustup.rs | sh -s -- \
    -y --profile minimal --default-toolchain stable
export PATH="/root/.cargo/bin:/usr/local/bin:$PATH"
rustup component add rustfmt clippy
rustup target add wasm32-wasip1 x86_64-unknown-linux-musl
cargo install cargo-nextest --locked

npm install --global n
n lts
hash -r
npm install --global @openai/codex@latest @anthropic-ai/claude-code@latest
npm uninstall --global n

cat >/etc/profile.d/zellij-dev.sh <<'EOF'
export PATH="/root/.cargo/bin:/usr/local/bin:$PATH"
EOF
for command in cargo cargo-nextest clippy-driver rustc rustfmt rustup; do
    ln -sf "/root/.cargo/bin/$command" "/usr/local/bin/$command"
done

rustc --version
cargo --version
cargo nextest --version
codex --version
claude --version
protoc --version
rg --version

apt-get clean
rm -rf /var/lib/apt/lists/* /root/.cargo/registry /root/.cargo/git /root/.npm
