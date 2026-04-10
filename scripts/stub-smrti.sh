#!/bin/bash
set -e

# Create a dummy smrti crate to satisfy dependency resolution in CI
# where SSH keys are not available for the private smrti repository.
#
# Crucial: We place this inside the current directory (vendor/smrti) 
# so that Docker-based CI actions (like cargo-deny) can see it in their volume mount.

STUB_DIR="vendor/smrti/konf-tool-memory-smrti"

mkdir -p "$STUB_DIR/src"
touch "$STUB_DIR/src/lib.rs"
cat <<INNEREOF > "$STUB_DIR/Cargo.toml"
[package]
name = "konf-tool-memory-smrti"
version = "0.1.0"
edition = "2021"
license = "BUSL-1.1"
INNEREOF

mkdir -p .cargo
cat <<INNEREOF > .cargo/config.toml
[patch."ssh://git@github.com/konf-dev/smrti.git"]
konf-tool-memory-smrti = { path = "$STUB_DIR" }
INNEREOF

echo "Stubbed smrti dependency successfully at $STUB_DIR"
