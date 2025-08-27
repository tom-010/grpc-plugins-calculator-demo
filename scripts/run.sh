#!/bin/bash

set -e

echo "Building project..."
cargo build

echo "Running supervisor..."
cargo run --bin supervisor