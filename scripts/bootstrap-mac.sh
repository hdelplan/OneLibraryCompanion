#!/bin/bash

set -e

echo "======================================"
echo " Pioneer Companion - Mac Bootstrap"
echo "======================================"
echo

echo "Checking required development tools..."
echo

# Xcode Command Line Tools
if xcode-select -p >/dev/null 2>&1; then
    echo "✓ Xcode Command Line Tools"
else
    echo "✗ Xcode Command Line Tools not installed"
    echo "  Install with: xcode-select --install"
fi

# Git
if command -v git >/dev/null 2>&1; then
    echo "✓ Git: $(git --version)"
else
    echo "✗ Git not installed"
fi

# Rust
if command -v rustc >/dev/null 2>&1; then
    echo "✓ Rust: $(rustc --version)"
    echo "✓ Cargo: $(cargo --version)"
else
    echo "✗ Rust not installed"
fi

echo
echo "Bootstrap check complete."
