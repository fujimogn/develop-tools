# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.6.0] - 2026-10-21

### Added

- Publish the tools from the public `fujimogn/develop-tools` repository. Each Release carries prebuilt archives for `x86_64-unknown-linux-gnu` and `aarch64-apple-darwin` and a `SHA256SUMS` file, and its body is the matching section of this changelog.
- `resolve` fetches the version pinned in `.develop-tools.json`, verifies the archive against both the published and the pinned SHA256, and caches the binaries.

### Changed

- Fetch the tools from public Releases without a token. Earlier versions were distributed only from a private source.

Versions before 0.6.0 were not published.
