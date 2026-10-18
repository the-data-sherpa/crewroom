#!/bin/bash
# Installed beside crewroom, crewroomd, crewroom-host and crewroom-process.
set -euo pipefail
umask 077
fail() { echo "Crewroom launcher: $*" >&2; exit 2; }
usage() {
  echo "Usage: crewroom-launcher --data-dir /absolute/private/data --roles /absolute/private/roles.json [--port 4318] [--headless]"
}
data=''
roles=''
port=4318
headless=false
while (($#)); do
  case "$1" in
    --data-dir|--roles|--port)
      (($# >= 2)) || fail "$1 needs a value."
      case "$1" in --data-dir) data="$2";; --roles) roles="$2";; --port) port="$2";; esac
      shift 2 ;;
    --headless) headless=true; shift ;;
    --help) usage; exit 0 ;;
    *) usage >&2; fail "Unknown option: $1" ;;
  esac
done
[[ "$data" == /* && "$roles" == /* ]] || fail 'Data and role paths must be explicit absolute paths.'
[[ "$port" =~ ^[0-9]+$ && ${#port} -le 5 ]] || fail 'Port must be an integer from 1 through 65535.'
port=$((10#$port))
((port >= 1 && port <= 65535)) || fail 'Port must be from 1 through 65535.'
private_directory() {
  [[ ! -L "$1" ]] || fail 'Private directories cannot be symlinks.'
  /bin/mkdir -p "$1"
  [[ -d "$1" ]] || fail 'Private path is not a directory.'
  [[ "$(owner "$1")" == "$(/usr/bin/id -u)" ]] || fail 'Private directory belongs to another user.'
  local mode
  mode="$(permissions "$1")"
  (( (8#$mode & 077) == 0 )) || fail 'Private directories must have no group or other permissions.'
}
system="$(/usr/bin/uname -s)"
case "$system" in
  Darwin)
    owner() { /usr/bin/stat -f %u "$1"; }
    permissions() { /usr/bin/stat -f %Lp "$1"; } ;;
  Linux)
    owner() { /usr/bin/stat -c %u "$1"; }
    permissions() { /usr/bin/stat -c %a "$1"; }
    if ! $headless && [[ -z "${WAYLAND_DISPLAY:-}" ]]; then
      fail 'The Linux desktop requires a Wayland session (WAYLAND_DISPLAY); use --headless for service-only startup.'
    fi ;;
  *) fail "Unsupported operating system: $system" ;;
esac
private_directory "$data"
private_directory "$(/usr/bin/dirname "$roles")"
data="$(cd "$data" && pwd -P)"
roles="$(cd "$(/usr/bin/dirname "$roles")" && pwd -P)/$(/usr/bin/basename "$roles")"
if [[ -e "$roles" || -L "$roles" ]]; then
  [[ -f "$roles" && ! -L "$roles" ]] || fail 'Role file must be a regular file.'
  [[ "$(owner "$roles")" == "$(/usr/bin/id -u)" ]] || fail 'Role file belongs to another user.'
  mode="$(permissions "$roles")"
  (( (8#$mode & 077) == 0 )) || fail 'Role file must have no group or other permissions.'
fi
binaries="$(cd "$(/usr/bin/dirname "$0")" && pwd -P)"
for name in crewroom crewroomd crewroom-host crewroom-process; do
  [[ -f "$binaries/$name" && -x "$binaries/$name" && ! -L "$binaries/$name" ]] || fail "Missing adjacent executable: $name"
done
daemon="$binaries/crewroomd"
if "$daemon" --data-dir "$data" --status >/dev/null 2>&1; then
  echo 'Reusing the authenticated service. Its existing role, port and runtime configuration remain in effect.'
else
  "$daemon" --data-dir "$data" --roles "$roles" --port "$port" --background
  for ((attempt=0; attempt<240; attempt++)); do
    if "$daemon" --data-dir "$data" --status >/dev/null 2>&1; then break; fi
    /bin/sleep 0.25
  done
  "$daemon" --data-dir "$data" --status >/dev/null 2>&1 || fail "Service did not become ready. Retain and inspect $data/service.log; no unknown process was stopped."
  echo 'Started the bundled service. Closing this launcher or the app leaves it running.'
fi
"$daemon" --data-dir "$data" --status
if $headless; then exit 0; fi
# Provider credentials and the private runtime configuration belong to the
# daemon. The desktop authenticates only with its private connection descriptor.
unset CREWROOM_JIRA_TOKEN CREWROOM_BITBUCKET_TOKEN CREWROOM_RUNTIME_CONFIG
if [[ "$system" == Linux ]]; then
  # The desktop may close independently; only the authenticated daemon owns
  # workflow execution. Keep launch errors observable in the private logs.
  exec "$binaries/crewroom" --connection "$data/native-connection.json" >>"$data/app.stdout.log" 2>>"$data/app.stderr.log"
fi
bundle="$(cd "$binaries/../.." && pwd -P)"
[[ -f "$bundle/Contents/Info.plist" ]] || fail 'GUI launch requires the installed app bundle.'
/usr/bin/open -n "$bundle" --stdout "$data/app.stdout.log" --stderr "$data/app.stderr.log" --args --connection "$data/native-connection.json"
