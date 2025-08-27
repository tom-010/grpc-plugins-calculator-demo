#!/bin/bash

set -e

echo "Building project..."
cargo build --workspace --release

echo "Running supervisor..."
cargo run --bin supervisor