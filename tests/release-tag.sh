#!/bin/sh
set -eu

# Git fixtures must not inherit the caller's worktree or index.
for name in $(env | sed -n 's/^\(GIT_[A-Za-z0-9_]*\)=.*/\1/p'); do
  unset "$name"
done

script=$(CDPATH= cd -- "$(dirname -- "$0")/../scripts" && pwd)/check-release-tag.sh
workflow=$(CDPATH= cd -- "$(dirname -- "$0")/../.github/workflows" && pwd)/release.yml
# The fixture's refusal must precede both build and publication in the workflow.
awk '/^  build:/ { job = "build" } /^  publish:/ { job = "publish" }
     job == "build" && /^    needs: gate$/ { build = 1 }
     job == "publish" && /^    needs: build$/ { publish = 1 }
     END { exit !(build && publish) }' "$workflow"
fixture=$(mktemp -d "${TMPDIR:-/tmp}/release-tag.XXXXXXXX")
trap 'cd /; rm -r -- "$fixture"' 0
printf 'release-tag fixture: %s\n' "$fixture"
cd "$fixture"
git init -q -b main
git config user.name Fixture
git config user.email fixture@example.invalid
printf '[workspace]\nmembers = []\n\n[workspace.package]\nversion = "0.1.0"\n' > Cargo.toml
git add Cargo.toml
git commit -qm initial
git tag v0.1.0
git tag v0.2.0
git update-ref refs/remotes/origin/main HEAD

sh "$script" v0.1.0

if sh "$script" v0.2.0; then
  echo 'mismatched tag was accepted' >&2
  exit 1
fi

for invalid in v01.0.0 v0.01.0 v0.0.01; do
  if sh "$script" "$invalid" 2>invalid.err; then
    echo "leading zero was accepted: $invalid" >&2
    exit 1
  fi
  case $(cat invalid.err) in
    *'tag の形式が不正です'*) ;;
    *) echo "leading zero was not rejected by format validation: $invalid" >&2; exit 1 ;;
  esac
done

git checkout -qb divergent
sed 's/0\.1\.0/0.1.1/' Cargo.toml > next.toml
mv next.toml Cargo.toml
git add Cargo.toml
git commit -qm divergent
git tag v0.1.1
if sh "$script" v0.1.1; then
  echo 'tag outside main was accepted' >&2
  exit 1
fi
