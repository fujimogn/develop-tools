#!/bin/sh
set -eu

fail() {
  printf 'Release gate: %s\n' "$1" >&2
  exit 1
}

[ "$#" -eq 1 ] || fail 'usage: check-release-tag.sh v<semver>'
tag=$1
case "$tag" in
  v[0-9]*.[0-9]*.[0-9]*) ;;
  *) fail "tag の形式が不正です: $tag" ;;
esac
version=${tag#v}
case "$version" in
  *[!0-9.]*|.*|*..*|*.) fail "tag の形式が不正です: $tag" ;;
esac
[ "$(printf '%s' "$version" | awk -F. '{print NF}')" -eq 3 ] || fail "tag の形式が不正です: $tag"
printf '%s\n' "$version" | awk -F. '{ for (i = 1; i <= NF; i++) if (length($i) > 1 && substr($i, 1, 1) == "0") exit 1 }' || fail "tag の形式が不正です: $tag"

manifest_version=$(awk '
  /^\[workspace\.package\][[:space:]]*$/ { in_package = 1; next }
  /^\[/ { in_package = 0 }
  in_package && /^[[:space:]]*version[[:space:]]*=/ {
    line = $0
    sub(/^[[:space:]]*version[[:space:]]*=[[:space:]]*"/, "", line)
    sub(/"[[:space:]]*(#.*)?$/, "", line)
    print line
    count++
  }
  END { if (count != 1) exit 1 }
' Cargo.toml) || fail 'Cargo.toml の workspace.package.version を読めません'
[ "$version" = "$manifest_version" ] || fail "tag $tag と Cargo.toml の版 $manifest_version が一致しません"

tag_commit=$(git rev-parse --verify "refs/tags/$tag^{commit}" 2>/dev/null) || fail "tag がありません: $tag"
head_commit=$(git rev-parse HEAD) || fail 'HEAD を読めません'
[ "$tag_commit" = "$head_commit" ] || fail "tag $tag が HEAD を指していません"
git rev-parse --verify refs/remotes/origin/main >/dev/null 2>&1 || fail 'origin/main がありません'
git merge-base --is-ancestor "$tag_commit" refs/remotes/origin/main || fail "tag $tag の commit が main に含まれません"
