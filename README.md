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

## Release notes

`release.notes` in `.develop-tools.json` chooses how `release` writes the Release body. Without it, `release` keeps creating a lightweight tag and the body comes from the existing extraction. With it, `release` writes the body into an annotated tag.

```json
{
  "release": {
    "notes": { "mode": "changelog-section", "language": "en" }
  }
}
```

| Field | Values | Default |
|---|---|---|
| `mode` | `changelog-additions` (the entries added since the previous tag in a monthly `CHANGELOG.md`), `changelog-section` (the dated `## X.Y.Z (YYYY-MM-DD)` or `## [X.Y.Z] - YYYY-MM-DD` section of the version), or `ai` (a summary with one inference, falling back to `fallback`) | Required |
| `fallback` | `changelog-additions` or `changelog-section`. Only with `mode: "ai"` | Required with `ai` |
| `audience` | `private` or `public`. `public` applies the stronger output checks. Only with `mode: "ai"` | Required with `ai` |
| `language` | `ja` or `en` | `ja` |

`mode: "ai"` with `language: "en"` is not supported yet; `release` stops before writing anything. `release <bump> --no-ai` skips the inference and uses `fallback`.

With `mode: "ai"`, `release` scrubs dates, commit SHAs and URLs from the extracted items, numbers them and makes one Anthropic API request without retrying. The summary becomes the tag body only when it passes the output checks; otherwise the `fallback` body is used and the release continues. The fallback reason is one of `key_missing`, `config_invalid`, `dictionary_missing`, `input_rejected`, `budget_exceeded`, `api_timeout`, `api_failed`, `response_rejected`, `output_rejected:<position>.<kind>`, `usage_unrecorded` and `unknown_purpose`.

After creating the tag, `release` prints the body and its source: `AI の要約`, `fallback（<reason>）` or `--no-ai`. Read it before `release push`. To change it, recreate the tag with `git tag -f -a <tag> <commit>`.

### Environment settings

The API key reference, the forbidden terms and the internal names are not stored in this package or in the repository declaration. `release` reads them from `~/.config/develop-tools/llm.json` in the user's environment; keep the file readable only by the user.

| Field | Meaning |
|---|---|
| `anthropic_api_key_ref` | 1Password secret reference (`op://…`) of the Anthropic API key. It is read with the 1Password service account token from `OP_SERVICE_ACCOUNT_TOKEN` or `~/.config/op/service-account.token` |
| `forbidden_terms` | Terms that must not appear in the summary |
| `internal_names` | Internal names that must not appear in the summary. Checked with `audience: "public"` |
| `usage_ledger` | Absolute path of the JSON Lines file that receives one usage row per request |

Without the file, `release` does not send the request (`key_missing`; `dictionary_missing` with `audience: "public"`). Without `anthropic_api_key_ref`, it does not send the request either (`key_missing` for both audiences). Without `usage_ledger`, it does not send the request either (`usage_unrecorded`). With `audience: "public"`, both `forbidden_terms` and `internal_names` are required (`dictionary_missing`). With `audience: "private"` and no `forbidden_terms`, the request is still sent and the source line shows `AI の要約（dictionary_missing）`. `release check` reports which fields are present in a repository that declares `mode: "ai"`.

### Release workflow

A Release workflow uses the annotation as the body when the tag object is an annotated tag with a non-empty annotation. For a lightweight tag or an empty annotation it uses the output of `release-notes <tag>`. Decide by the object type (`git cat-file -t refs/tags/<tag>`), not by `%(contents)`, which returns the commit message for a lightweight tag.

This repository does not use `release`: its tag is created by the publishing step and the Release body is the matching `CHANGELOG.md` section (`scripts/release-notes.sh`).

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
