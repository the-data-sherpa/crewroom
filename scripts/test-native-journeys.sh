#!/bin/bash
# Native GPUI/service/Git and loopback-provider journeys; no production data.
set -euo pipefail
cd "$(dirname "$0")/.."
export ZIG
ZIG="$(python3 scripts/bootstrap-native.py)"
cargo build --workspace --locked
cargo test -p crewroom-desktop --locked native_journey -- --ignored --test-threads=1
