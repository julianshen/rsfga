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
cargo install cargo-tarpaulin || echo "Warning: cargo-tarpaulin installation failed"
cargo install cargo-audit || echo "Warning: cargo-audit installation failed"
cargo install cargo-watch || echo "Warning: cargo-watch installation failed"
cargo install cargo-udeps || echo "Warning: cargo-udeps installation failed"

echo "==> Verifying installations..."
echo "Rust: $(rustc --version)"
echo "Cargo: $(cargo --version)"
echo "Protobuf: $(protoc --version)"
echo "GitHub CLI: $(gh --version | head -n1)"
echo "Claude Code: $(claude --version)"
echo "Node: $(node --version)"

echo "==> Development environment ready!"
echo "Run 'cargo build' to build the project."
