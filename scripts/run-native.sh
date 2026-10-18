#!/bin/bash
set -euo pipefail
cd "$(dirname "$0")/.."
profile=debug
build_only=false
for argument in "$@"; do
  case "$argument" in
    --build-only) build_only=true ;;
    --release) profile=release ;;
    *) echo "Usage: $0 [--build-only] [--release]" >&2; exit 2 ;;
  esac
done
export ZIG
ZIG="$(python3 scripts/bootstrap-native.py --check-build-dependencies)"
build_args=(build --locked -p crewroom-desktop -p crewroom-service -p crewroom-runtime -j 6)
if [[ "$profile" == release ]]; then build_args+=(--release); fi
cargo "${build_args[@]}"
# Cargo resolves configured and environment target directories itself.
target_directory="$(cargo metadata --locked --format-version 1 --no-deps | python3 -c 'import json, sys; print(json.load(sys.stdin)["target_directory"])')"
# Keep each package immutable, including any copy an existing UI uses.
package_root="$(mktemp -d "$target_directory/$profile/native-package.XXXXXX")"
case "$(uname -s)" in
  Darwin) app_bundle="$package_root/Crewroom.app"; launcher="$app_bundle/Contents/MacOS/crewroom-launcher" ;;
  Linux) app_bundle="$package_root/Crewroom"; launcher="$app_bundle/bin/crewroom-launcher" ;;
  *) echo 'Native launch supports macOS and Linux.' >&2; exit 2 ;;
esac
python3 scripts/package-native.py --binary-directory "$target_directory/$profile" --output "$app_bundle"
if $build_only; then exit 0; fi
native_data="${CREWROOM_NATIVE_DATA_DIR:-$PWD/.crewroom/rust}"
native_roles="${CREWROOM_ROLE_PATH:-$HOME/.config/crewroom/roles.json}"
[[ "$native_data" == /* ]] || native_data="$PWD/$native_data"
[[ "$native_roles" == /* ]] || native_roles="$PWD/$native_roles"
exec "$launcher" --data-dir "$native_data" --roles "$native_roles"
