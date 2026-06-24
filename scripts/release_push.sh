#!/usr/bin/env bash
set -euo pipefail

usage() {
    printf 'usage: just release-push <version>\n' >&2
    printf 'example: just release-push 0.1.5\n' >&2
}

fail() {
    printf 'release-push: %s\n' "$1" >&2
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
    fail 'working tree must be clean before pushing a release'
fi

branch="$(git symbolic-ref --quiet --short HEAD)" || fail 'cannot push release from a detached HEAD'

if ! git rev-parse -q --verify "refs/tags/$tag" >/dev/null; then
    fail "tag does not exist locally: $tag"
fi

tag_commit="$(git rev-list -n 1 "$tag")"
head_commit="$(git rev-parse HEAD)"
if [ "$tag_commit" != "$head_commit" ]; then
    fail "tag $tag points at $tag_commit, not HEAD $head_commit"
fi

cargo_version="$(python3 - <<'PY'
from pathlib import Path
import re

text = Path("Cargo.toml").read_text()
match = re.search(r'(?m)^version\s*=\s*"([^"]+)"\s*$', text)
if not match:
    raise SystemExit("Cargo.toml package version not found")
print(match.group(1))
PY
)"

if [ "$cargo_version" != "$version" ]; then
    fail "Cargo.toml version is $cargo_version, expected $version"
fi

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

if git ls-remote --exit-code --tags origin "refs/tags/$tag" >/dev/null 2>&1; then
    fail "tag already exists on origin: $tag"
fi

git push origin "HEAD:$branch"
git push origin "refs/tags/$tag"

printf 'pushed release %s\n' "$tag"
