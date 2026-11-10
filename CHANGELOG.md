# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.6.16] - 2026-11-10

### Added

- `release <bump> --first-release-base <full OID>` builds the first Release of a repository that has a monthly `CHANGELOG.md` and no previous tag. The body comes from the items added since the given commit or tree, and every item marked `/⏳` must match the body. When the base is ambiguous or the body is empty, `release` makes no version commit and no tag.

## [0.6.15] - 2026-11-10

### Fixed

- Accept `CHANGELOG.md` items whose title contains bold text, such as `- **Shorten the config to **9 lines**（YYYY-MM-DD）**: Summary`. 0.6.14 rejected them. Bold text in an item body is still not read as the title, date or version stamp.

## [0.6.14] - 2026-11-10

### Added

- Add the `changelog-migrate` binary to `develop-release`. It plans version stamps for existing `CHANGELOG.md` items from the git history and writes them only for a fixed input with no held items. It is not part of the Release assets; build it from source.
- Accept the stamped title forms `- **Title（YYYY-MM-DD/⏳）**`, `- **Title（YYYY-MM-DD/vX.Y.Z）**` and `- **Title（YYYY-MM-DD/[vX.Y.Z](Release URL)）**` (full-width parentheses) when `release.changelogStamp` is declared. `release` writes the Release link when it stamps an item.

### Changed

- Raise the `CHANGELOG.md` size limit to 360,000 bytes. The archive target stays at 80% of the limit.
- `release-notes <tag>` reads only `release.changelogStamp` from the end commit, and rejects the older forms when it is `true`.

### Fixed

- Stop reading bold text in an item body as the item title, date or version stamp.
- Stop printing the `origin` URL, which can carry a token, in repository resolution errors.
- `release-notes render --repo` uses the given repository instead of resolving `origin` again.

## [0.6.13] - 2026-11-09

### Added

- Add `ci-change-scope` with its dependency proof mode to the public CLI binary set.

### Changed

- The `resolve` script downloads release assets through the authenticated GitHub API when `GITHUB_TOKEN` or `GH_TOKEN` is set, and retries transient failures with a bounded limit. Without a token it keeps using the public download URL. A token failure no longer falls back silently to anonymous access. Checksum verification is unchanged.

## [0.6.12] - 2026-11-06

### Changed

- Check only the link at the end of an entry's first line as the CHANGELOG commit link. Commit links in the middle of an entry or on continuation lines are ordinary references, and labels made only of digits (such as ADR or run numbers) are no longer taken for commits. Release bodies apply the same rule to the original first line.
- Raise the AI release-note input capacity to 256 KiB. Cost estimates now use the higher price tier when the input may exceed 100K tokens, the per-request cost limit stays at USD 1, and the JSON output budget is larger.
- Clarify when release-note callers must stop before tagging, how to reuse render fallback bodies, and how verified input items supply trailing commit links.

### Fixed

- Fix `run-heavy-check` on Linux leaving zombies of descendants that moved to another process group; the supervisor now reaps every adopted child while it runs. The slot is still held until the worker group is confirmed gone.
- Fix the heavy-check handshake on macOS failing with `EINVAL` when the supervisor closes the socket right after the acknowledgement.

## [0.6.11] - 2026-11-06

### Fixed

- Fix `run-heavy-check` on macOS reporting a failed stop after cancellation: a process group that only contains unreaped zombies returns `EPERM` there, which is now treated as still stopping until the existing confirmation budget ends. The slot is still held until the group is confirmed gone.
- Fix the heavy-check handshake failing on macOS when the worker closes its socket right after acknowledging the slot.

### Notes

- Version 0.6.10 has a tag only; no GitHub Release was created because the macOS runtime tests failed.

## [0.6.10] - 2026-11-06

### Fixed

- Fix the heavy-check harness failing to compile on macOS because its `openpty` arguments used const null pointers instead of mutable null pointers.

### Notes

- Version 0.6.9 has a tag only; no GitHub Release was created because the macOS build failed.

## [0.6.9] - 2026-11-06

### Added

- Add `run-heavy-check` to the prebuilt archives. It queues heavy checks in FIFO order for the OS account, validates inherited slots for nested checks, and keeps the slot while supervised descendants remain alive.
- Preserve command arguments, binary standard input and payload output through `run-heavy-check`, with waiting progress on stderr and optional JSON Lines timing events on a dedicated file descriptor.
- Add `release-notes render --from <tag> --to <full-commit-SHA> --tag <version>` to generate a release body from fixed inputs without creating commits, tags or GitHub Releases. Standard output contains only the body; the final stderr line contains a JSON diagnostic.
- Record AI release-note fallback reasons in `release-notes-fallback.jsonl` in the common Git directory. Each entry contains only a timestamp, tag and reason code; a warning identifies recording failures.
- Accept the optional `ci.recordOnlySkip` declaration for repositories that use generated CI to skip heavy jobs for qualifying documentation-only pushes.

- Validate trailing CHANGELOG commit links against fixed Git history, including staged documentation checks.
- Add verified commit links to AI-generated Release notes from cited input items, and reject invalid links before creating the release commit or tag.

### Fixed

- Select legacy CHANGELOG stamps by release version and remove their metadata before AI summarization. Reject ambiguous legacy stamps before creating a version commit, including stamps carried over from the previous tag.
- Read fixed release-note inputs from the repository root when invoked from a subdirectory, and use stable diagnostic codes for invalid inputs.

### Notes

- Store the account-wide heavy-check state in `~/.cache/develop-tools/heavy-check`.

- Checks only share the account slot when their entry points invoke `run-heavy-check`. Explicit headless sandbox execution uses an isolated slot, and GitHub Actions runs the payload without the local account lock.
- Terminal foreground restoration covers normal completion and INT/TERM/HUP exit paths. Coordinated job-control stop/resume handling is not guaranteed.

## [0.6.8] - 2026-11-05

### Changed

- Repositories that declare `release.changelogStamp: true` use a compact CHANGELOG title: `- **Title** YYYY-MM-DD/vX.Y.Z: summary` once released and `- **Title** YYYY-MM-DD ⏳: summary` while unreleased. `release` rewrites ` ⏳` to `/vX.Y.Z` in the version commit, and the parenthesized stamp form is no longer written.
- The CHANGELOG checks (`check-staged-docs`, `develop-docs-check`) read `release.changelogStamp` from the manifest and accept only the compact title in those repositories. Repositories without the declaration keep the parenthesized date form and the same messages.
- `archive-changelog` reads dates from both title forms.
- Release notes select items by the compact `/vX.Y.Z` stamp and remove compact dates and markers before sending items to the model.

## [0.6.7] - 2026-11-05

### Added

- `.config/develop-tools.json` accepts an optional `ci.bun.test` declaration (`command`, `shards`, and optional `prepareUses` and `build`) so the distributed CI can run Bun tests as sharded jobs instead of `test:fast`.

### Changed

- AI release notes are told not to add effect, speed, numeric, or comparison claims that are absent from the input, and not to weaken its limitations or qualifications.

### Fixed

- `release` with `release.notes` declared no longer stops on the first release of a repository without any tag: it skips inference and uses the version's CHANGELOG section as the tag message.
- Public AI release notes and package checks reject internal names with English plural suffixes while preserving strict word boundaries.
- Prevent duplicate parallel downloads on Ubuntu 26.04 by requiring a directory creation receipt when acquiring resolver locks with uutils coreutils.

## [0.6.6] - 2026-11-04

### Added

- Support AI release notes with `release.notes.language: "en"`, including the version-section input, English compare guidance and the same public output checks.
- `archive-changelog` archives the oldest dates of the current month when archiving every older month still leaves `CHANGELOG.md` over the limit. The newest date stays in place.

### Fixed

- AI release notes no longer fall back with `response_rejected` because thinking used up the output limit: the release-notes request disables thinking and allows up to 16,000 output tokens.
- Date archiving in `archive-changelog` keeps unindented lines and moves a continuation after a blank line together with its entry.
- `changelog-additions` release notes leave out rewrites of older entries: additions in the same change run as a removed entry are treated as edits and reported with a warning.
- `changelog-additions` reads a removed horizontal rule (`---`) in the diff as body text, not as a file header.

## [0.6.5] - 2026-10-27

### Added

- Add `inspect-adr` for full repository ADR validation.

### Changed

- Honor `includePublic` in release inventories while preserving legacy snapshots.

## [0.6.4] - 2026-10-25

### Changed

- `resolve` no longer reads a root `.develop-tools.json`. A file left only at that path fails and names `.config/develop-tools.json` as the place to move it. Both files still fail.
- The version pin, secret declaration, ADR, and lefthook lookups read only their `.config/` locations. A file left only at a retired root location (or `config/` for secret declarations) fails and names the new path; the old paths are still checked so duplicates are reported.

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
