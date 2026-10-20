# develop-tools

Command-line checks for repository documentation, staged changes, secret declarations and release versions. Each tagged version is published as a GitHub Release with prebuilt archives and a `SHA256SUMS` file.

Supported targets:

- `x86_64-unknown-linux-gnu` (Linux x86_64)
- `aarch64-apple-darwin` (macOS arm64)

## Install with the resolver

The `resolve` script fetches the version pinned in the calling repository, verifies it, caches it and runs the requested binary. Commit `.develop-tools.json` at the repository root:

```json
{
  "version": "0.6.0",
  "sha256": {
    "x86_64-unknown-linux-gnu": "<64 lowercase hexadecimal characters>",
    "aarch64-apple-darwin": "<64 lowercase hexadecimal characters>"
  }
}
```

Copy `resolve` into the repository and run it from the repository root:

```sh
./resolve <pre-commit|pre-push|ci|release> <binary> [args...]
./resolve bin-dir <pre-commit|pre-push|ci|release>
```

- The resolver downloads `develop-tools-v<version>-<target>.tar.gz` and `SHA256SUMS` from the `v<version>` Release. The archive must match both the published `SHA256SUMS` line and the `sha256` pinned for the target; any mismatch fails without extracting the archive.
- Verified binaries are cached in `${XDG_CACHE_HOME:-$HOME/.cache}/develop-tools/<version>/<target>`, and later runs reuse the cache without downloading. `bin-dir` prints that directory instead of running a binary.
- Set `DEVELOP_TOOLS_BIN_DIR` to use binaries from a local directory instead. The resolver fails if the requested binary is not there.
- When the tools cannot be fetched (no network, a missing asset or an unsupported target), `pre-commit` prints a notice and skips the check so that CI checks it instead. `pre-push`, `ci` and `release` fail.

The resolver needs `curl`, `tar`, `jq` and `sha256sum` or `shasum`. In GitHub Actions, [`fujimogn/develop-tools-action`](https://github.com/fujimogn/develop-tools-action) runs the same resolver and adds the cache directory to `PATH`.

## Binaries

| Binary | Purpose |
|---|---|
| `develop-docs-check` | Check local Markdown links, heading anchors and size limits in tracked, staged or ranged files. |
| `check-staged-docs` | Check a staged `TODO.md` and `CHANGELOG.md`. |
| `check-staged-adr` | Check the structure of staged architecture decision records and their numbers against the ADR numbering contract in the staged `.develop-tools.json`. |
| `develop-secrets` | Build development secret files from secret references or standard input without printing the values. |
| `release` | Advance the declared version files, fast-forward `main` and create a local tag. `release push` publishes them. |
| `check-release-version` | Before a push, check that the release tag matches the declared version files. |
| `archive-changelog` | Move older months of `CHANGELOG.md` out of the file and into Git history. |

Run a binary with `--help` for its options.

## Version files

`release` and `check-release-version` read the version files declared under `release.versionFiles` in `.develop-tools.json`. Each entry has a `path` relative to the repository root and a `kind`:

```json
{
  "release": {
    "versionFiles": [
      { "path": "Cargo.toml", "kind": "cargo-workspace" },
      { "path": "Cargo.lock", "kind": "cargo-lock" }
    ]
  }
}
```

| `kind` | Version field |
|---|---|
| `package-json` | `version` in `package.json` |
| `cargo-workspace` | `[workspace.package] version` in `Cargo.toml` |
| `cargo-package` | `[package] version` in `Cargo.toml` |
| `cargo-lock` | The `version` of the declared Cargo packages in `Cargo.lock` |
| `toml-root` | The top-level `version` in a TOML file |

All declared files must hold the same version. Set `release.changelogStamp` to `true` to also stamp the root `CHANGELOG.md` in the version commit.

## Versioning

The Release version follows [Semantic Versioning](https://semver.org/spec/v2.0.0.html) within 0.x: a patch release for changes callers can adopt without adjustment, including new features, and a minor release when callers must adjust. A major release comes only after an explicit stability declaration. Each Release body is the matching section of [`CHANGELOG.md`](./CHANGELOG.md).

## Development

Each binary belongs to one crate under [`crates/`](./crates/), and every crate is a member of the workspace in this directory. Run the tests from this directory. `cargo test` needs [Bun](https://bun.sh) (the version in [`.bun-version`](./.bun-version)) on `PATH`: one test reads a generated `.env` back with Bun's own dotenv parser and fails when `bun` is missing.

```sh
cargo test --locked
sh tests/release-tag.sh
sh tests/release-notes.sh
sh tests/release-contracts.sh
TEST_SHELL="$(command -v bash)" bash tests/resolve.sh
```

## License

[MIT](./LICENSE)
