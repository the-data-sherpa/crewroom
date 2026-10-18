#!/bin/bash
# GPUI operator Resume with disposable Rust daemon, Git, processes and Docker.
set -euo pipefail
cd "$(dirname "$0")/.."
export ZIG
ZIG="$(python3 scripts/bootstrap-native.py)"
cargo build --workspace --examples --locked
cargo build --workspace --locked
artifacts="$(mktemp -t crewroom-native-tests)"
trap 'rm -f "$artifacts"' EXIT
cargo test -p crewroom-desktop --locked --no-run --message-format=json-render-diagnostics > "$artifacts"
export CREWROOM_NATIVE_TEST_BINARY
CREWROOM_NATIVE_TEST_BINARY="$(python3 - "$artifacts" <<'PY'
import json, sys
for line in open(sys.argv[1]):
    value = json.loads(line)
    if value.get('reason') == 'compiler-artifact' and value.get('profile', {}).get('test') and value.get('executable'):
        print(value['executable'])
PY
)"
node --import tsx --test --test-concurrency=1 scripts/rust/native-resume.test.ts scripts/rust/native-controls.test.ts scripts/rust/native-intake.test.ts
