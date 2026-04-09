#!/bin/bash
set -e

# Create a dummy smrti crate to satisfy dependency resolution in CI
# where SSH keys are not available for the private smrti repository.

mkdir -p ../smrti/konf-tool-memory-smrti/src
touch ../smrti/konf-tool-memory-smrti/src/lib.rs
cat <<INNEREOF > ../smrti/konf-tool-memory-smrti/Cargo.toml
[package]
name = "konf-tool-memory-smrti"
version = "0.1.0"
edition = "2021"
INNEREOF

mkdir -p .cargo
cat <<INNEREOF > .cargo/config.toml
[patch."ssh://git@github.com/konf-dev/smrti.git"]
konf-tool-memory-smrti = { path = "../smrti/konf-tool-memory-smrti" }
INNEREOF

echo "Stubbed smrti dependency successfully."
