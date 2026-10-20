# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

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
