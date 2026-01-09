#!/bin/bash
set -euo pipefail

echo "==> Installing system dependencies..."
sudo apt-get update
sudo apt-get install -y protobuf-compiler

echo "==> Installing Claude Code..."
npm install -g @anthropic-ai/claude-code

echo "==> Installing Rust components..."
rustup component add clippy rustfmt

echo "==> Installing cargo tools..."
# Install each tool separately with explicit error handling
# Using --locked to match CI workflow and ensure reproducible builds
cargo install --locked cargo-tarpaulin || echo "Warning: cargo-tarpaulin installation failed"
cargo install --locked cargo-audit || echo "Warning: cargo-audit installation failed"
cargo install --locked cargo-watch || echo "Warning: cargo-watch installation failed"
# Note: cargo-udeps requires nightly Rust. Install manually if needed:
# rustup install nightly && cargo +nightly install cargo-udeps

echo "==> Verifying installations..."
echo "Rust: $(rustc --version)"
echo "Cargo: $(cargo --version)"
echo "Protobuf: $(protoc --version)"
echo "GitHub CLI: $(gh --version | head -n1)"
echo "Claude Code: $(claude --version)"
echo "Node: $(node --version)"

echo "==> Development environment ready!"
echo "Run 'cargo build' to build the project."
