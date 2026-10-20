#!/bin/sh
set -eu

fail() {
  printf 'Release notes: %s\n' "$1" >&2
  exit 1
}

[ "$#" -eq 1 ] || fail 'usage: release-notes.sh v<semver>'
tag=$1
case "$tag" in
  v[0-9]*.[0-9]*.[0-9]*) ;;
  *) fail "tag の形式が不正です: $tag" ;;
esac
[ -f CHANGELOG.md ] || fail 'CHANGELOG.md がありません'

# 本文は `## [X.Y.Z] - YYYY-MM-DD` の節から次の `## ` の見出しの前まで。前後の空行は落とす。
notes=$(awk -v version="${tag#v}" '
  /^## / {
    if (in_section) exit
    if ($0 ~ /^## \[[0-9]+\.[0-9]+\.[0-9]+\] - [0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9][[:space:]]*$/ && $2 == "[" version "]") {
      in_section = 1
    }
    next
  }
  in_section {
    if ($0 ~ /^[[:space:]]*$/) { blank++; next }
    if (printed) for (; blank > 0; blank--) print ""
    blank = 0
    print
    printed = 1
  }
' CHANGELOG.md) || fail 'CHANGELOG.md を読めません'
[ -n "$notes" ] || fail "CHANGELOG.md に日付付きの $tag の節が無いか、節が空です"
printf '%s\n' "$notes"
