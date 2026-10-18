#!/bin/bash
# Run only after the root task provides the tested, pushed combined revision.
set -euo pipefail
umask 077
if (($# != 4)) || [[ "$1" != --plan && "$1" != --execute ]]; then
  echo "Usage: $0 --plan|--execute FULL_COMBINED_SHA /private/tmp/short-new-root /absolute/mcp-agent-mail" >&2
  exit 2
fi
mode="$1"; revision="$2"; release_root="$3"; mail_binary="$4"
[[ "$revision" =~ ^[0-9a-f]{40}$ ]] || { echo 'Use an exact full commit SHA.' >&2; exit 2; }
[[ "$release_root" == /* && ${#release_root} -le 40 && "$mail_binary" == /* ]] || { echo 'Use an absolute new root of at most 40 characters and an absolute Mail executable.' >&2; exit 2; }
source_root="$(cd "$(dirname "$0")/.." && pwd -P)"
[[ "$(git -C "$source_root" rev-parse "$revision^{commit}")" == "$revision" ]]
if [[ "$mode" == --plan ]]; then
  cat <<PLAN
Candidate: $revision
New isolated root: $release_root
1. Require the integrated vendor, migration, launcher and validation sources at that exact commit.
2. Clone into a fresh checkout with no target or node_modules; retain clean source identity.
3. Verify pinned Zig/Rust, build and test release mode once; package four adjacent binaries and launcher.
4. Verify final signed-binary sidecar hashes; record build time and disk size separately from runtime memory.
5. Install fresh validation dependencies; exercise bundled headless startup/reopen/stopped restart.
6. Run real OMP/Mail/Git/Docker whole-state restore and subsequent research with scripted providers.
7. Run baseline, heterogeneous and rotated profiles against adjacent bundled daemon/host/helper binaries: four-hop Mail, SIGKILL/same-PID reattachment and paused shutdown.
8. Record the exact source/artifact receipts. This script does not accept GUI, footprint, rollback or N7 cutover.
Remaining coordinated gates are in docs/native-release-validation.md.
PLAN
  exit 0
fi
[[ ! -e "$release_root" && ! -L "$release_root" ]] || { echo 'Output already exists; preserve it and choose a new root.' >&2; exit 2; }
[[ -f "$mail_binary" && -x "$mail_binary" && ! -L "$mail_binary" ]] || { echo 'Mail must be an explicit regular executable.' >&2; exit 2; }
for required in vendor/gpui-libghostty/CREWROOM.md crates/crewroom-core/src/migrations.rs scripts/launch-native.sh scripts/validate-native-release.sh scripts/rust/verify-release-profiles.py scripts/rust/release-profiles-live.ts; do
  git -C "$source_root" cat-file -e "$revision:$required" || { echo "Combined candidate is missing $required" >&2; exit 2; }
done
# The orchestrator itself must be the revision-bound version being validated.
cmp -s "$0" <(git -C "$source_root" show "$revision:scripts/validate-native-release.sh") || { echo 'Validation script differs from candidate; use its checked-in version.' >&2; exit 2; }
mkdir -p "$release_root/logs"
release_root="$(cd "$release_root" && pwd -P)"
phase=clone
trap 'code=$?; if ((code)); then echo "Failed phase: $phase (exit $code). Preserve $release_root for inspection." >&2; fi' EXIT
run_phase() { phase="$1"; shift; echo "Native release validation: $phase"; "$@" >"$release_root/logs/$phase.log" 2>&1; }
run_phase clone git clone --no-hardlinks --no-checkout "$source_root" "$release_root/checkout"
cd "$release_root/checkout"
git checkout --detach "$revision" >"$release_root/logs/checkout.log" 2>&1
[[ -z "$(git status --porcelain)" && ! -e target && ! -e node_modules ]]
run_phase bootstrap python3 scripts/bootstrap-native.py
export ZIG
ZIG="$(tail -n 1 "$release_root/logs/bootstrap.log")"
run_phase release-build /usr/bin/time -lp cargo build --release --locked -p crewroom-desktop -p crewroom-service -p crewroom-runtime -j 6
run_phase release-tests cargo test --workspace --locked --release -j 6
run_phase package python3 scripts/package-native.py --binary-directory target/release --output "$release_root/Crewroom.app"
run_phase dependency-install npm ci
run_phase retained-build npm run build
run_phase launcher python3 scripts/test-native-launcher.py --bundle "$release_root/Crewroom.app" --output "$release_root/l"
run_phase restore python3 scripts/test-native-installation.py --binary-directory "$release_root/Crewroom.app/Contents/MacOS" --output "$release_root/ops" --mail-binary "$mail_binary"
for profile in baseline heterogeneous rotated; do
  run_phase "profile-$profile" python3 scripts/rust/verify-release-profiles.py --profile "$profile" --binary-directory "$release_root/Crewroom.app/Contents/MacOS" --mail-binary "$mail_binary" --output "$release_root/p-${profile:0:1}"
done
phase=receipt
python3 - "$release_root" "$revision" <<'PY'
import hashlib,json,sys
from pathlib import Path
root=Path(sys.argv[1]);bundle=root/'Crewroom.app';receipt=json.loads((root/'Crewroom.app.receipt.json').read_text())
for name,row in receipt['binaries'].items():
 assert hashlib.sha256((bundle/'Contents/MacOS'/name).read_bytes()).hexdigest()==row['packagedSha256'],name
assert receipt['sourceRevision']==sys.argv[2]
assert json.loads((root/'l/report.json').read_text())['passed']
assert json.loads((root/'ops/report.json').read_text())['passed']
profiles={}
for name in ['baseline','heterogeneous','rotated']:
 path=root/f'p-{name[0]}'/'report.json';proof=json.loads(path.read_text())
 assert proof['passed'] and proof['sourceRevision']==sys.argv[2],name
 for binary,row in proof['binaries'].items():
  assert row['path']==str(bundle/'Contents/MacOS'/binary) and row['sha256']==receipt['binaries'][binary]['packagedSha256'],(name,binary)
 profiles[name]={'reportSha256':hashlib.sha256(path.read_bytes()).hexdigest(),'providerRequests':proof['providerRequests'],'externalMailHops':len(proof['externalMailReceipts']),'ownedProcessesStopped':len(proof['ownedProcesses'])}
result={'profiles':profiles,'revision':sys.argv[2],'headlessReleaseGatesPassed':True,'bundleReceiptSha256':hashlib.sha256((root/'Crewroom.app.receipt.json').read_bytes()).hexdigest(),'logs':{p.name:hashlib.sha256(p.read_bytes()).hexdigest() for p in sorted((root/'logs').glob('*.log'))},'pending':['Native GUI/terminal/accessibility on this bundle','Production runtime footprint','Coverage reconciliation and failed-service gates','Old-build/pre-cutover whole-state rollback inventory','Default cutover and release note']}
(root/'headless-release.json').write_text(json.dumps(result,indent=2)+'\n');print(json.dumps(result))
PY
