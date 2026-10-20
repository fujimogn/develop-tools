#!/bin/sh
set -eu

root=$(CDPATH='' cd -- "$(dirname -- "$0")/.." && pwd)
resolver=$root/resolve
workflow=$root/.github/workflows/release.yml
fixture=$root/tests/resolve.sh

resolver_targets=$(sed -n 's/.*) target=\([^ ;]*\) ;;/\1/p' "$resolver" | sort)
matrix_targets=$(awk '
  /^[[:space:]]+matrix:/ { in_matrix = 1; next }
  in_matrix && /^[[:space:]]+runs-on:/ { exit }
  in_matrix && /^[[:space:]]+target:/ { print $2 }
' "$workflow" | sort)
test -n "$resolver_targets"
test "$resolver_targets" = "$matrix_targets"

resolver_asset=$(sed -n 's/^asset=//p' "$resolver")
fixture_asset=$(sed -n 's/^asset=//p' "$fixture")
workflow_asset=$(sed -n 's/^[[:space:]]*asset=//p' "$workflow" |
  sed 's/^"//; s/"$//; s/\${RELEASE_TAG}/v$version/; s/\${RELEASE_TARGET}/$target/')
test -n "$resolver_asset"
test "$resolver_asset" = "$fixture_asset"
test "$resolver_asset" = "$workflow_asset"
