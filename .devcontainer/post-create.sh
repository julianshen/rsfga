#!/bin/bash
set -e

echo "==> Installing Claude Code..."
npm install -g @anthropic-ai/claude-code

echo "==> Installing Rust components..."
rustup component add clippy rustfmt

echo "==> Installing cargo tools..."
cargo install cargo-tarpaulin cargo-audit cargo-watch 2>/dev/null || true

echo "==> Verifying installations..."
echo "Rust: $(rustc --version)"
echo "Cargo: $(cargo --version)"
echo "GitHub CLI: $(gh --version | head -n1)"
echo "Claude Code: $(claude --version 2>/dev/null || echo 'installed')"
echo "Node: $(node --version)"

echo "==> Building project..."
cargo build

echo "==> Development environment ready!"
