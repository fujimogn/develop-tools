# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.6.3] - 2026-10-24

### Added

- `island-docs` reads the scope table copy from `.config/island-docs-scopes.json` or `config/island-docs-scopes.json`. It fails when both exist, even with the same bytes, and when the copy is not a regular file (a symlink is refused).

### Fixed

- With `release.notes.language: "en"`, the compare line at the top of the Release body reads `Compare:` instead of the Japanese heading.
- `check-release-version` no longer fails in a repository whose manifest declares no `release.versionFiles` (for example a repository without versions): it prints one line to stderr and skips the working-tree check. A broken declaration still fails, and tags sent with `--pre-push` are still checked.
- `release push` in a repository whose integration branch is `main` no longer stops when the tip of `main` has moved past the release commit and carries no version tag. It pushes the tag for the version declared at `main`, after checking that the tag commit is an ancestor of `origin/main` and that every `main` push run for that commit succeeded.

## [0.6.2] - 2026-10-23

### Added

- Look for the version-pin manifest at `.config/develop-tools.json` first and at `.develop-tools.json` second. `resolve`, `release`, `check-release-version`, the staged ADR check and `develop-docs-check --check-local-version` share this lookup, and each one stops when both files exist (for `release` and `check-release-version`, a broken symlink counts as present). `develop-docs-check --check-local-version` exits 2 in that case and stays silent on other read errors, as before.
- `develop-secrets` documents `.config/develop-secrets.jsonc` as the default declaration path. Tools that search a repository for the declaration try `.config/develop-secrets.jsonc`, then `.develop-secrets.jsonc`, then `config/develop-secrets.jsonc`, and fail when more than one exists.
- `release.notes` in the manifest selects how `release` writes the Release body: `changelog-additions`, `changelog-section` (the dated section of the version; a missing, duplicated or empty section stops `release` before it writes anything) or `ai` (one summary request with a `fallback`). With `release.notes`, `release` writes the body into an annotated tag, prints the body and its source, and accepts `--no-ai` to skip the request. See the README for the fields and the environment settings.
- With `mode: "ai"`, the items are scrubbed of dates, commit SHAs and URLs and numbered before the request, and the summary is used only when it passes the output checks (forbidden terms, internal names for `audience: "public"`, URLs, tokens and item references outside the input). Items the summary leaves out are pointed to through a compare link at the end. Every request, including a timed-out or truncated one, writes one row to the usage ledger.
- `release check` validates the `release.notes` declaration and, in a repository that declares `mode: "ai"`, reports which environment settings are present.
- Ship `release-notes` in the Release archives.
- The manifest accepts `ci.bun.ciInstall`, so a Bun CI job can run the repository's `ci:install` script instead of the default install.
- The manifest accepts a list of paths in `platformCheck.cargoWorkspace` and a nextest filterset in `platformCheck.runIgnored`, matching the platform check workflow.

### Changed

- The `changelogStamp` of `release` also stamps `（未リリース）` without a date and keeps any note before `、未リリース）`, replacing only that suffix with `v<version> で prod 反映）`.

### Fixed

- When `release` has moved the main branch but cannot create the tag, it stops, says which refs it moved, and prints the command that creates the tag (with the body, for an annotated tag) before `release push`.
- With `mode: "changelog-section"`, `release` keeps leading whitespace in the version section and reports a missing `CHANGELOG.md` with its own message.
- Version parsing rejects signs and other non-digit characters (for example `+1.2.3`), and reading release notes from `CHANGELOG.md` recognizes a code fence only from a run of its opening character.

## [0.6.1] - 2026-10-21

The `v0.6.0` tag has no Release: its Test run failed and its Release workflow did not start. 0.6.1 is the first release with prebuilt archives.

### Fixed

- Install Bun in the Test and Release workflows. `cargo test` reads a generated `.env` back with Bun's dotenv parser, so the Test run for 0.6.0 failed on both runners, and the Release build would fail the same way. The tests now document that they need `bun` (the version in `.bun-version`) on `PATH`.
- Pass `TEST_SHELL` to `tests/resolve.sh` as an absolute path in the workflows and the README. The missing-command cases run the resolver with a restricted `PATH`, where a bare `bash` or `dash` is not found.

## [0.6.0] - 2026-10-21

### Added

- Publish the tools from the public `fujimogn/develop-tools` repository. Each Release carries prebuilt archives for `x86_64-unknown-linux-gnu` and `aarch64-apple-darwin` and a `SHA256SUMS` file, and its body is the matching section of this changelog.
- `resolve` fetches the version pinned in `.develop-tools.json`, verifies the archive against both the published and the pinned SHA256, and caches the binaries.

### Changed

- Fetch the tools from public Releases without a token. Earlier versions were distributed only from a private source.

Versions before 0.6.0 were not published.
