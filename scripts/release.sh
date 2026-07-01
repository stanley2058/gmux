#!/usr/bin/env bash
set -euo pipefail

usage() {
    printf 'usage: just release <version>\n' >&2
    printf 'example: just release 0.1.5\n' >&2
}

fail() {
    printf 'release: %s\n' "$1" >&2
    exit 1
}

run_quiet_test_fast() {
    local log
    log="$(mktemp "${TMPDIR:-/tmp}/gmux-release-test-fast.XXXXXX.log")"

    printf 'running just test-fast...'
    if just --quiet test-fast >"$log" 2>&1; then
        rm -f "$log"
        printf ' ok\n'
        return 0
    fi

    printf ' failed\n' >&2
    printf 'just test-fast output:\n' >&2
    sed 's/^/  /' "$log" >&2
    printf 'full log: %s\n' "$log" >&2
    exit 1
}

if [ "$#" -ne 1 ]; then
    usage
    exit 2
fi

version="$1"
tag="v$version"

case "$version" in
    v*) fail 'pass the bare Cargo version, not a v-prefixed tag' ;;
esac

if ! [[ "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.-]+)?(\+[0-9A-Za-z.-]+)?$ ]]; then
    fail "invalid version: $version"
fi

repo_root="$(git rev-parse --show-toplevel)"
cd "$repo_root"

if [ -n "$(git status --porcelain)" ]; then
    git status --short >&2
    fail 'working tree must be clean before preparing a release'
fi

if git rev-parse -q --verify "refs/tags/$tag" >/dev/null; then
    fail "tag already exists locally: $tag"
fi

if git ls-remote --exit-code --tags origin "refs/tags/$tag" >/dev/null 2>&1; then
    fail "tag already exists on origin: $tag"
fi

current_version="$(python3 - <<'PY'
from pathlib import Path
import re

text = Path("Cargo.toml").read_text()
match = re.search(r'(?m)^version\s*=\s*"([^"]+)"\s*$', text)
if not match:
    raise SystemExit("Cargo.toml package version not found")
print(match.group(1))
PY
)"

if [ "$current_version" = "$version" ]; then
    fail "Cargo.toml is already at version $version"
fi

python3 - "$version" <<'PY'
from pathlib import Path
import re
import sys

version = sys.argv[1]
path = Path("Cargo.toml")
text = path.read_text()
updated, count = re.subn(
    r'(?m)^(version\s*=\s*")([^"]+)("\s*)$',
    rf'\g<1>{version}\g<3>',
    text,
    count=1,
)
if count != 1:
    raise SystemExit("Cargo.toml package version not found")
path.write_text(updated)
PY

python3 - "$version" <<'PY'
from pathlib import Path
import sys

version = sys.argv[1]
path = Path("Cargo.lock")
lines = path.read_text().splitlines(keepends=True)
in_package = False
in_gmux = False

for index, line in enumerate(lines):
    stripped = line.rstrip("\n")
    if stripped == "[[package]]":
        in_package = True
        in_gmux = False
    elif in_package and stripped == 'name = "gmux"':
        in_gmux = True
    elif in_gmux and stripped.startswith("version = "):
        newline = "\n" if line.endswith("\n") else ""
        lines[index] = f'version = "{version}"{newline}'
        path.write_text("".join(lines))
        break
else:
    raise SystemExit("Cargo.lock gmux package version not found")
PY

lock_version="$(python3 - <<'PY'
from pathlib import Path

in_gmux = False
for line in Path("Cargo.lock").read_text().splitlines():
    if line == "[[package]]":
        in_gmux = False
    elif line == 'name = "gmux"':
        in_gmux = True
    elif in_gmux and line.startswith('version = '):
        print(line.split('"', 2)[1])
        break
else:
    raise SystemExit("Cargo.lock gmux package version not found")
PY
)"

if [ "$lock_version" != "$version" ]; then
    fail "Cargo.lock gmux version is $lock_version, expected $version"
fi

run_quiet_test_fast

git add Cargo.toml Cargo.lock
git commit -m "chore: bump version to $version"
git tag -a "$tag" -m "$tag"

printf 'prepared release %s\n' "$tag"
printf 'push when ready:\n'
printf '  just release-push %s\n' "$version"
