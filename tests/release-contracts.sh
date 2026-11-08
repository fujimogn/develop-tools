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

# Release の実際の一覧から梱包し、example が bin と並んでいても収録しない。
jq -e '.binaries | index("run-heavy-check") != null and (map(select(. == "ci-change-scope")) | length) == 1 and index("heavy-check-harness") == null' "$root/config/release-binaries.json" > /dev/null
# 名前の形式は workflow と同じ制約で検査し、word splitting は binary 名だけに限る。
jq -e '.binaries | all(.[]; test("^[A-Za-z0-9_-]+$"))' "$root/config/release-binaries.json" > /dev/null
artifact_fixture=$(mktemp -d "${TMPDIR:-/tmp}/develop-meta-release-contract.XXXXXXXX")
trap 'rm -f "$artifact_fixture"/bin/* "$artifact_fixture"/artifact.tar.gz; rmdir "$artifact_fixture"/bin "$artifact_fixture"' 0
mkdir "$artifact_fixture/bin"
for binary in $(jq -r '.binaries[]' "$root/config/release-binaries.json"); do
  printf '%s\n' "$binary" > "$artifact_fixture/bin/$binary"
done
printf 'test only\n' > "$artifact_fixture/bin/heavy-check-harness"
set -- $(jq -r '.binaries[]' "$root/config/release-binaries.json")
COPYFILE_DISABLE=1 tar -czf "$artifact_fixture/artifact.tar.gz" -C "$artifact_fixture/bin" "$@"
listing=$(tar -tzf "$artifact_fixture/artifact.tar.gz" | sort)
test "$listing" = "$(jq -r '.binaries[]' "$root/config/release-binaries.json" | sort)"
printf '%s\n' "$listing" | awk '$0 == "run-heavy-check" { found = 1 } END { exit !found }' > /dev/null
printf '%s\n' "$listing" | awk '$0 == "ci-change-scope" { count++ } END { exit count != 1 }' > /dev/null
if printf '%s\n' "$listing" | awk '$0 == "heavy-check-harness" { found = 1 } END { exit !found }' > /dev/null; then
  exit 1
fi
printf 'ok: public archive includes ci-change-scope and run-heavy-check, excludes the example harness\n'
