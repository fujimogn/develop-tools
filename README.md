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
| `inspect-adr` | Inspect all working-tree ADRs, including numbering, archives, the template and the README index. |
| `run-heavy-check` | Serialize heavy checks for the current account while preserving payload arguments and standard input. |
| `develop-secrets` | Build development secret files from secret references or standard input without printing the values. |
| `release` | Advance the declared version files, fast-forward `main` and create a local tag. `release push` publishes them. |
| `check-release-version` | Before a push, check that the release tag matches the declared version files. |
| `archive-changelog` | Move older months of `CHANGELOG.md` out of the file and into Git history. |

Run a binary with `--help` for its options.

## Inspect all ADRs

Run `./resolve ci inspect-adr` from the repository root. Declare at least one ADR numbering field (`adrMigratedThrough`, `adrReservedNumbers` or `adrSanitizedNumbers`) in `.config/develop-tools.json`. The command uses this current manifest contract; it does not read the retired `docs/adr/adr-contract.json`. The command reads the working tree, including unstaged and untracked ADRs; it does not require a Git index. Keep the template baseline at `scripts/develop-tools/adr-template.md` and the local template at `docs/adr/adr-template.md`. The baseline is a Markdown document and must remain available when replacing older scripts with this binary.

The command reports template drift, missing README entries, duplicate or missing numbers, reserved-number reuse and invalid legacy or sanitized archive hashes. It writes no files. Exit codes are `0` for no findings, `1` for documentation violations and `2` for argument or input-reading failures. Use `check-staged-adr` for staged changes; that command does not check whole-tree template drift.

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

All declared files must hold the same version. Set `release.changelogStamp` to `true` to also stamp the root `CHANGELOG.md` in the version commit. With this declaration, use `- **Title** YYYY-MM-DD ⏳: Summary` for unreleased entries; `release` replaces ` ⏳` with `/vX.Y.Z` (for example, `- **Title** 2026-11-04/v0.1.155: Summary`).

## Release notes

`release-notes <vX.Y.Z> [--repo owner/name]` は前 tag から指定 tag までの月別追記を決定論で抽出します。宣言と LLM は使いません。旧刻印 `（…、vX.Y.Z で prod 反映）` の他版項目も除外するように変更されています。短い刻印の他版除外は維持し、対象版・未リリース・印なしは通常の抽出対象です。旧刻印と短い刻印が重なる曖昧な題は構造エラーになります。成功は本文を stdout に出して exit 0、tag・repo・履歴・抽出の不備は exit 1 です。

### 本文だけの生成

```text
release-notes render --from <vX.Y.Z> --to <full-commit-SHA> --tag <vA.B.C> [--repo owner/name] [--no-ai]
```

help は `release-notes --help` で表示します。`release-notes render --help` は文法エラー（exit 1）です。

`--from` は既存 release tag、`--to` は repository の object format に合う完全な commit SHA、`--tag` は予定の v<semver> です。from は to の祖先で別 commit、予定 tag は from と別名にします。予定 tag は未作成でも構いません。未知・重複・値欠落の option、余計な位置引数は拒否します。repo を省略すると origin から解決し、fetch や GitHub API は呼びません。clean/branch/TTY は不要で detached worktree でも使えます。

終点 commit の `CHANGELOG.md` と `.config/develop-tools.json` の `release.notes` を読み、dirty な作業 tree や後続 HEAD の内容は使いません。宣言の欠落・不正、旧 `.develop-tools.json` のみ、両 path の併存は失敗です。初回 release は扱いません。明示範囲の中間 tag は許すため、適切な前 tag の選択は呼出側の責務です。

| 結果 | stdout | 終了 code | stderr の終端 JSON / 記録 |
|---|---|---|---|
| AI 成功 | 検証済み Markdown（末尾 LF） | 0 | `{"source":"ai","reason_code":null,"dictionary_missing":false}`。`dictionary_missing` boolean は常に入り、private 辞書欠落時は `true`、それ以外は `false`。usage 台帳は送信 request ごとに1行 |
| 決定論の宣言 / `--no-ai` | 抽出本文 | 0 | 決定論宣言は `source:"changelog"`、ai 宣言の `--no-ai` は `source:"no_ai"`。`reason_code:null`、API/op 不使用 |
| 設定・費用・通信・出力・usage の失敗 | 同じ抽出からの fallback 本文 | 0 | `source:"fallback"` と既存 `reason_code`。fallback JSONL へ追記し、再推論しない |
| 文法・from/to・宣言・CHANGELOG の構造不備 | 空 | 1 | `source:"error"` と安定した `reason_code`。秘密を除いた説明。API 不使用 |
| fallback 理由の保存失敗 | fallback 本文 | 0 | 同じ fallback 診断に `recording_warning:"fallback_reason_unrecorded"` を追加 |

stdout は全検証と必須 usage 記録後の本文だけ、stderr の最後の1行は JSON です。人向け警告はその前に出ることがあります。本文・key・prompt・API 生応答は stderr に出しません。出力先への書込み失敗は exit 1 / `stdout_failed` です。

副作用は、許可された API の最大1回の送信、設定された usage 台帳と Git common dir の `release-notes-fallback.jsonl` への追記に限ります。版 file・CHANGELOG・index・commit・branch・tag・remote・GitHub Release・prod は変更しません。fallback JSONL の各行は `timestamp_unix_seconds`、`tag`、`reason_code` の既存3欄だけです。fallback 理由と課金 usage は別記録であり、本文生成は Release 公開や prod 成功を示しません。

呼び出し元への引渡し例（記号は確認済みの値で置換する argv 配列）:

```text
["release-notes", "render", "--from", "<確認済み前tag>", "--to", "<版commitの完全SHA>", "--tag", "<予定tag>", "--repo", "owner/repo"]
```

版 commit 作成後・tag 作成前に shell 補間せず起動し、stdout/stderr/code を別々に捕捉します。非0・起動不可・timeout・本文や終端JSONの不備なら既存抽出へ戻し、`--no-ai` の再実行や再推論はしません。CLI の0だけでAI成功とは判定せず、既知 source/code の組と本文を検証します。採用本文は版SHAに結び付けて保存・再利用し、tag公開とprod操作のgateは呼び出し元が保持します。

### 宣言と既存 release

`release.notes` は既存 `release` では注釈 tag に本文を書く方式も選択します。宣言なしでは lightweight tag と従来の抽出を維持します。`render` では宣言が必須です。

```json
{"release":{"notes":{"mode":"ai","fallback":"changelog-additions","language":"ja","audience":"private"}}}
```

| 欄 | 値 | 既定 |
|---|---|---|
| `mode` | `changelog-additions`（月別追記）、`changelog-section`（対象版の日付付き節）、`ai` | 必須 |
| `fallback` | `changelog-additions` / `changelog-section` | ai のとき必須 |
| `audience` | `private` / `public`（強い出力検査） | ai のとき必須、既定なし |
| `language` | `ja` / `en` | `ja` |

AI は日付・SHA・URL・刻印メタデータを scrub し、番号付き入力で1回だけ送信します。`release <bump> --no-ai` も推論を抑止します。fallback 理由は `key_missing`、`config_invalid`、`dictionary_missing`、`input_rejected`、`budget_exceeded`、`api_timeout`、`api_failed`、`response_rejected`、`output_rejected:<position>.<kind>`、`usage_unrecorded`、`unknown_purpose` です。既存 `release` は tag 作成後に本文と出所を表示します。`release push` 前に確認してください。本文を変えるときは `git tag -f -a <tag> <commit>` で注釈 tag を作り直します。既存 tag の変更は人の承認と運用規約に従ってください。`language:ja/en` で推論の指示言語を切り替え、JSON 出力と検証の契約は共通です。fallback JSONL は clone 内の linked worktree で共有され、push されません。記録行は tag 作成の証明にはなりません。初回 release・changelog・`--no-ai`・AI 成功では fallback 行を追記しません。ログは `cat "$(git rev-parse --path-format=absolute --git-common-dir)/release-notes-fallback.jsonl"` で読めます。

### Environment settings

The API key reference, the forbidden terms and the internal names are not stored in this package or in the repository declaration. `release` and `release-notes render` read them from `~/.config/develop-tools/llm.json` in the user's environment; keep the file readable only by the user.

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

## run-heavy-check

`run-heavy-check [--executor-context=headless-sandbox] [--] <command...>` は argv を
shell 文字列へ変換せず実行する。wrapper は command の直前の `--` を一つだけ消費し、
stdin（binary / push refs / TTY）と payload の stdout をそのまま渡す。
binary の `--help` / `--version` は取得・状態作成を行わない。
公開 Release の台帳に収録し、既存 resolver で次のように呼び出す。

```sh
./resolve ci run-heavy-check -- cargo test -p my-crate
```

resolver は `--` を含む argv・stdin・終了値を透過する。上例の `--` は
run-heavy-check が一つだけ消費する。resolver 自体は固定版の取得・digest 検証を行うため、
未公開版・digest 不一致・binary 欠落では payload を起動しない。
`heavy-check-harness` は test 専用 example であり、Release artifact には含めない。
この登録に伴う利用側 pin の版・hash 更新は別段で行う。

通常は OS account home の安全な状態で FIFO 取得する。HOME の差替えだけでは取得を省略しない。
account root の EROFS/EACCES/EPERM による fallback は、安全性の確認後に別の安全な HOME だけで行う。
`--executor-context=headless-sandbox` は明示された executor の HOME に独立枠を作り、
account root が書込可能でも host 共通保護とは数えない。`GITHUB_ACTIONS=true` は取得せず raw code を返す。
local の payload 75 は 1 に写し、未実行の取得失敗は 75、signal は 128+signal とする。

stderr に `queued ticket=N`、待機中30秒ごとの `still waiting after Ns; head ticket=N`、
`acquired after Ns`、`completed after Ns; exit=N`、または
`lock unavailable after Ns; tests not run` を表示する。取得後は待機 heartbeat を止める。
heartbeat は外側の deadline を延長しない。

計測は任意の `DEVELOP_HEAVY_CHECK_EVENT_FD`（3以上の明示 FD）に JSON Lines を渡す。
caller はこの open description を計測専用にし、wrapper は nonblocking にして payload へ継承しない。
`event` は `queued/acquired/completed/unavailable`、時間は `lockWaitMs/checkMs`、
終了は `rawExit/rawSignal/result/executed`、範囲は `isolated/context` で表す。
`context` は `host/ci/headless-sandbox/write-denial-fallback`。
正常な exit 130 を signal 2 と推定しない。argv・環境値・接続情報は出力しない。
閉鎖・満杯・無効な計測 FD は best effort で捨て、検査の終了値と deadline を変えない。
共通 JSONL ファイルは作らず、adapter の既存記録へ接続する。

standalone は専用 group を監督し、helper が slot FD を閉じても監督側が子孫終了まで slot を保持する。
有効な継承 slot では再入ticketを作らず、正規の外側 supervisor の control handshake でも起動できる。
外側監督方式の最終 group 停止・cleanup はその supervisor の責務とする。
通常終了・INT/TERM/HUP の終了経路で foreground を復元する。SIGTSTP/SIGTTIN の検知、
監督側との連動停止、停止/再開時の foreground 復元・再移譲は保証しない。
停止中の ticket/slot は死亡回収せず保持し、外側 deadline が無ければ人が resume/cancel する。
Mac の端末・peer PID 契約は platform gate に残る。未接続の入口を保護済みとは扱わない。

## License

[MIT](./LICENSE)
