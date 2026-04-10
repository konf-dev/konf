#!/bin/bash
set -e

# Create a dummy smrti crate to satisfy dependency resolution in CI
# where SSH keys are not available for the private smrti repository.

STUB_DIR="vendor/smrti/konf-tool-memory-smrti"

mkdir -p "$STUB_DIR/src"

# 1. Create a Cargo.toml that references workspace dependencies
cat <<EOF > "$STUB_DIR/Cargo.toml"
[package]
name = "konf-tool-memory-smrti"
version = "0.1.0"
edition = "2021"
license = "BUSL-1.1"

[dependencies]
konf-tool-memory = { path = "../../../crates/konf-tool-memory" }
serde_json = "1.0"
anyhow = "1.0"
EOF

# 2. Create a lib.rs with the expected connect() function
cat <<EOF > "$STUB_DIR/src/lib.rs"
use std::sync::Arc;
use konf_tool_memory::MemoryBackend;

pub async fn connect(_config: &serde_json::Value) -> anyhow::Result<Arc<dyn MemoryBackend>> {
    anyhow::bail!("smrti is a private dependency and is stubbed in CI")
}
EOF

# 3. Update the local .cargo/config.toml to point to the stub
mkdir -p .cargo
cat <<EOF > .cargo/config.toml
[patch."ssh://git@github.com/konf-dev/smrti.git"]
konf-tool-memory-smrti = { path = "$STUB_DIR" }
EOF

echo "Stubbed smrti dependency with connect() function successfully."
