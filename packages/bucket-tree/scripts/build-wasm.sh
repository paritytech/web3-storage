#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
#
# Build the wasm module of clients/bucket-tree into pkg/. `pnpm install` runs
# this (prepare script). Needs rustup and wasm-bindgen-cli at the version of
# the wasm-bindgen crate in Cargo.lock.
set -euo pipefail
cd "$(dirname "$0")/../../.."

version=$(awk '/^name = "wasm-bindgen"$/ { getline; print }' Cargo.lock | cut -d'"' -f2)
installed=$(wasm-bindgen --version 2>/dev/null | awk '{ print $2 }' || true)
if [ "$installed" != "$version" ]; then
	echo "error: wasm-bindgen-cli $version is required (found: ${installed:-none})." >&2
	echo "install: cargo install wasm-bindgen-cli --version $version --locked" >&2
	exit 1
fi

rustup target list --installed | grep -qx wasm32-unknown-unknown || rustup target add wasm32-unknown-unknown
cargo build --release --locked -p bucket-tree --no-default-features --features wasm \
	--target wasm32-unknown-unknown
wasm-bindgen --target web --out-dir packages/bucket-tree/pkg \
	"${CARGO_TARGET_DIR:-target}/wasm32-unknown-unknown/release/bucket_tree.wasm"
