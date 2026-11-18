#!/bin/sh
set -eu

# Re-enter the public test entrypoint with hostile startup configuration. Every
# command on PATH is allowlisted; even a regression cannot reach a real gh.
if [ "${RELEASE_NOTES_SANDBOX:-}" != 1 ]; then
  sandbox=$(mktemp -d /tmp/develop-meta-gh-isolation-shell-XXXXXXXX)
  printf 'release-notes sandbox: %s\n' "$sandbox"
  # Remove only this fixture's known files; never recursively delete a directory.
  trap 'rm -f -- "$sandbox"/bin/* "$sandbox/hostbin/gh" "$sandbox/host-home/.bashrc" "$sandbox/tripwire"; rmdir -- "$sandbox/bin" "$sandbox/hostbin" "$sandbox/host-home" "$sandbox"' 0
  mkdir -p "$sandbox/bin" "$sandbox/hostbin" "$sandbox/host-home"
  for tool in sh bash env awk sha256sum cp dirname mktemp mkdir chmod cat rm ln; do
    ln -s "$(command -v "$tool")" "$sandbox/bin/$tool"
  done
  cat > "$sandbox/hostbin/gh" <<'EOF'
#!/bin/sh
printf 'host gh invoked\n' >> "$RELEASE_NOTES_TRIPWIRE"
exit 97
EOF
  chmod +x "$sandbox/hostbin/gh"
  printf 'PATH="%s/hostbin:$PATH"; export PATH\n' "$sandbox" > "$sandbox/host-home/.bashrc"
  result=0
  "$sandbox/bin/env" -i PATH="$sandbox/bin:$sandbox/hostbin" \
    HOME="$sandbox/host-home" SSH_CLIENT='127.0.0.1 12345 22' \
    BASH_ENV="$sandbox/host-home/.bashrc" RELEASE_NOTES_SANDBOX=1 \
    RELEASE_NOTES_TRIPWIRE="$sandbox/tripwire" \
    "$sandbox/bin/sh" "$0" || result=$?
  if [ -e "$sandbox/tripwire" ]; then
    echo 'isolation failed: host gh tripwire was invoked' >&2
    exit 1
  fi
  exit "$result"
fi

script=$(CDPATH= cd -- "$(dirname -- "$0")/../scripts" && pwd)/release-notes.sh
workflow=$(CDPATH= cd -- "$(dirname -- "$0")/../.github/workflows" && pwd)/release.yml
root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
fixture=$(mktemp -d /tmp/develop-meta-gh-isolation-shell-XXXXXXXX)
printf 'release-notes fixture: %s\n' "$fixture"
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
mkdir -p bin home scripts .github/workflows
for tool in sh bash env awk sha256sum cp; do
  ln -s "$(command -v "$tool")" "bin/$tool"
done
cp "$workflow" .github/workflows/release.yml
cp "$script" scripts/release-notes.sh
printf '#!/bin/sh\nexit 0\n' > scripts/check-release-tag.sh
printf '#!/bin/sh\nexit 0\n' > bin/git
cat > bin/gh <<'EOF'
#!/bin/sh
printf '%s\n' "$@" > gh.args
printf '%s\n' "$0" >> gh.calls
cp release-notes.md gh.notes
EOF
chmod +x bin/git bin/gh
matrix_targets=$(awk '/^[[:space:]]+matrix:/ { m = 1; next } m && /^[[:space:]]+runs-on:/ { exit } m && /^[[:space:]]+target:/ { print $2 }' "$workflow")
for target in $matrix_targets; do
  : > "develop-tools-v0.7.0-$target.tar.gz"
  : > "develop-tools-v0.6.0-$target.tar.gz"
done
workflow_step() {
  "$fixture/bin/env" -i PATH="$fixture/bin" HOME="$fixture/home" \
    RELEASE_TAG="$2" "$fixture/bin/bash" --noprofile --norc -e "$1"
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
printf '%s\n' release create v0.6.0 develop-tools-v0.6.0-*.tar.gz SHA256SUMS --verify-tag --title v0.6.0 --notes-file release-notes.md > expected.args
[ "$(cat gh.args)" = "$(cat expected.args)" ] || { echo 'unexpected gh argv' >&2; exit 1; }
[ "$(cat gh.calls)" = "$fixture/bin/gh" ] || { echo 'unexpected gh resolution or call count' >&2; exit 1; }
[ "$(cat gh.notes)" = "$expected" ] || { echo 'Release body is not the changelog section' >&2; exit 1; }
refuse 0.6.0 'tag without v'

printf '# Changelog\n\n## [Unreleased]\n\n- Pending change.\n' > CHANGELOG.md
refuse v0.6.0 'Unreleased-only changelog'

printf '# Changelog\n\n## [0.6.0] - 2026-10-20\n\n## [0.5.0] - 2026-10-01\n\n- Older.\n' > CHANGELOG.md
refuse v0.6.0 'empty section'

rm CHANGELOG.md
refuse v0.6.0 'missing changelog'
