#!/bin/sh
set -eu

script=$(CDPATH= cd -- "$(dirname -- "$0")/../scripts" && pwd)/release-notes.sh
workflow=$(CDPATH= cd -- "$(dirname -- "$0")/../.github/workflows" && pwd)/release.yml
root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
fixture=$(mktemp -d "${TMPDIR:-/tmp}/release-notes.XXXXXXXX")
trap 'cd /; rm -r -- "$fixture"' 0
cd "$fixture"

# Run the workflow's own run blocks so that a weakened gate or publish step fails here.
awk '/^  [a-z]+:$/ { job = $1 }
     job == "build:" && /^    needs: gate$/ { build = 1 }
     job == "publish:" && /^    needs: build$/ { publish = 1 }
     END { exit !(build && publish) }' "$workflow"
run_block() {
  awk -v job="  $1:" '
    /^  [a-z]+:$/ { in_job = ($0 == job) }
    in_job && /^        run: \|$/ { in_run = 1; text = ""; next }
    in_run && /^          / { text = text substr($0, 11) "\n"; next }
    in_run { in_run = 0; last = text }
    END { if (in_run) last = text; printf "%s", last }
  ' "$workflow"
}
run_block gate > gate.sh
run_block publish > publish.sh
[ -s gate.sh ] && [ -s publish.sh ]
mkdir -p bin scripts .github/workflows
cp "$workflow" .github/workflows/release.yml
cp "$script" scripts/release-notes.sh
printf '#!/bin/sh\nexit 0\n' > scripts/check-release-tag.sh
printf '#!/bin/sh\nexit 0\n' > bin/git
printf '#!/bin/sh\nprintf "%%s\\n" "$*" > gh.args\ncp release-notes.md gh.notes\n' > bin/gh
chmod +x bin/git bin/gh
matrix_targets=$(awk '/^[[:space:]]+matrix:/ { m = 1; next } m && /^[[:space:]]+runs-on:/ { exit } m && /^[[:space:]]+target:/ { print $2 }' "$workflow")
for target in $matrix_targets; do
  : > "develop-tools-v0.7.0-$target.tar.gz"
  : > "develop-tools-v0.6.0-$target.tar.gz"
done
workflow_step() {
  PATH="$fixture/bin:$PATH" RELEASE_TAG=$2 bash -e "$1"
}

cat > CHANGELOG.md <<'EOF'
# Changelog

## [Unreleased]

### Added

- Pending change.

## [0.6.1]

- Undated section.

## [0.6.0] - 2026-10-20

### Added

- First public release.

## [0.5.0] - 2026-10-01

- Older release.
EOF

sh "$script" v0.6.0 > notes.md
expected='### Added

- First public release.'
[ "$(cat notes.md)" = "$expected" ] || { echo 'unexpected notes:' >&2; cat notes.md >&2; exit 1; }

refuse() {
  if sh "$script" "$1" > refused.md 2> refused.err; then
    echo "$2 was accepted: $1" >&2
    exit 1
  fi
  [ ! -s refused.md ] || { echo "$2 printed notes: $1" >&2; exit 1; }
}
refuse v0.7.0 'tag without a section'
refuse v0.6.1 'section without a date'

# A tag without a section stops in the gate, and publish never creates the Release.
if workflow_step gate.sh v0.7.0 2> gate.err; then
  echo 'gate accepted a tag without a section' >&2
  exit 1
fi
if workflow_step publish.sh v0.7.0 2> publish.err || [ -e gh.args ]; then
  echo 'publish created a Release for a tag without a section' >&2
  exit 1
fi
workflow_step gate.sh v0.6.0
workflow_step publish.sh v0.6.0
case $(cat gh.args) in
  *'--notes-file release-notes.md'*) ;;
  *) echo 'Release was not created from the notes file' >&2; exit 1 ;;
esac
case $(cat gh.args) in
  *--generate-notes*) echo 'Release still uses generated notes' >&2; exit 1 ;;
esac
[ "$(cat gh.notes)" = "$expected" ] || { echo 'Release body is not the changelog section' >&2; exit 1; }
refuse 0.6.0 'tag without v'

printf '# Changelog\n\n## [Unreleased]\n\n- Pending change.\n' > CHANGELOG.md
refuse v0.6.0 'Unreleased-only changelog'

printf '# Changelog\n\n## [0.6.0] - 2026-10-20\n\n## [0.5.0] - 2026-10-01\n\n- Older.\n' > CHANGELOG.md
refuse v0.6.0 'empty section'

rm CHANGELOG.md
refuse v0.6.0 'missing changelog'
