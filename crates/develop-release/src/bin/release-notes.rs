use develop_release::release_notes::{
    FixedNotes, RenderedNotes, release_notes, with_default_ai_runtime,
};
use develop_tools_core::cli_help::Help;
use std::io::Write;

struct RenderArgs {
    from: String,
    to: String,
    tag: String,
    repo: Option<String>,
    no_ai: bool,
}

impl RenderArgs {
    fn parse(mut args: impl Iterator<Item = String>) -> Result<Self, &'static str> {
        let (mut from, mut to, mut tag, mut repo) = (None, None, None, None);
        let mut no_ai = false;
        while let Some(arg) = args.next() {
            let slot = match arg.as_str() {
                "--from" => &mut from,
                "--to" => &mut to,
                "--tag" => &mut tag,
                "--repo" => &mut repo,
                "--no-ai" if !no_ai => {
                    no_ai = true;
                    continue;
                }
                _ => return Err("arguments_invalid"),
            };
            if slot.is_some() {
                return Err("arguments_invalid");
            }
            let value = args
                .next()
                .filter(|value| !value.is_empty() && !value.starts_with('-'))
                .ok_or("arguments_invalid")?;
            *slot = Some(value);
        }
        Ok(Self {
            from: from.ok_or("arguments_invalid")?,
            to: to.ok_or("arguments_invalid")?,
            tag: tag.ok_or("arguments_invalid")?,
            repo,
            no_ai,
        })
    }
}

/// 生の Git/manifest エラーには入力や本文が含まれうるため、既知の構造 code だけを返す。
fn structure_code(error: &str) -> &'static str {
    // section_at の固定した説明だけを外し、入力値を含みうる後続の文章は判定しない。
    let error = error
        .strip_prefix("release.notes の changelog-section を作れません（")
        .unwrap_or(error);
    [
        "manifest_duplicated",
        "manifest_legacy",
        "section_missing",
        "section_duplicate",
        "section_empty",
        "changelog_stamp_ambiguous",
    ]
    .into_iter()
    .find(|code| {
        error
            .strip_prefix(code)
            .is_some_and(|rest| rest.starts_with(':'))
    })
    .unwrap_or("fixed_input_invalid")
}

fn render() -> Result<RenderedNotes, &'static str> {
    let args = RenderArgs::parse(std::env::args().skip(2))?;
    let root = std::env::current_dir().map_err(|_| "fixed_input_invalid")?;
    let fixed = FixedNotes::resolve(&root, &args.from, &args.to, &args.tag, args.repo.as_deref())
        .map_err(|error| structure_code(&error))?;
    with_default_ai_runtime(|settings, runtime| fixed.render(args.no_ai, settings, runtime))
        .map_err(|error| structure_code(&error))
}

fn render_main() -> i32 {
    let result = render().and_then(|rendered| {
        // usage と出力検証の完了後に、本文だけを一括で渡す。
        std::io::stdout()
            .lock()
            .write_all(rendered.body.text.as_bytes())
            .map_err(|_| "stdout_failed")?;
        Ok(rendered.diagnostic)
    });
    match result {
        Ok(diagnostic) => {
            eprintln!("{diagnostic}");
            0
        }
        Err(code) => {
            eprintln!("release-notes render: 入力・抽出・出力の契約を満たしません");
            eprintln!(
                "{}",
                serde_json::json!({"source":"error","reason_code":code})
            );
            1
        }
    }
}

fn run() -> Result<(), String> {
    let mut tag = None;
    let mut repo = None;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--help" | "-h" => {
                print!(
                    "{}",
                    Help::new(
                        "release-notes <vX.Y.Z> [--repo owner/name]\n       release-notes render --from <vX.Y.Z> --to <full-commit-SHA> --tag <vA.B.C> [--repo owner/name] [--no-ai]",
                        "tag: 前 tag からの月別追記を決定論で抽出し、旧刻印の他版項目を除外します。宣言と LLM は使いません。\nrender: 固定した from/to と終点 commit の宣言から本文だけを生成します。--from は既存 tag、--to は完全な commit SHA、--tag は予定 tag です。",
                        "tag: 前 tag と repo を git から解決。render: repo は origin から解決し、宣言が ai なら最大 1 回推論（--no-ai で抑止）。stdout は本文だけ、stderr の最後の 1 行が JSON（source/reason_code）。AI 成功では常に dictionary_missing boolean（private 辞書欠落時 true、それ以外 false）、fallback 理由保存失敗は recording_warning=fallback_reason_unrecorded で通知。fallback は再推論しません。help は release-notes --help で表示（render --help は文法エラー）。",
                        "tag: なし。render: API の最大 1 回の送信、usage 台帳と fallback JSONL への追記だけ。版 file・CHANGELOG・index・commit・branch・tag・remote・GitHub Release・prod は変更しません。",
                    )
                    .exit("0 = 本文生成成功（render は source=ai/changelog/no_ai/fallback、reason_code は成功時 null・fallback 時は理由）。1 = 文法・tag・repo・git 履歴・宣言・CHANGELOG の構造不備（render は stdout 空、source=error と reason_code）、または出力失敗")
                    .examples(&[
                        "release-notes v0.5.0",
                        "release-notes v0.5.0 --repo owner/project",
                        "release-notes render --from v0.4.0 --to <full-commit-SHA> --tag v0.5.0 --repo owner/project --no-ai"
                    ])
                    .render()
                );
                return Ok(());
            }
            "--repo" => repo = Some(args.next().ok_or("--repo の値がありません")?),
            _ if arg.starts_with('-') || tag.is_some() => return Err(format!("不明な引数: {arg}")),
            _ => tag = Some(arg),
        }
    }
    let tag = tag.ok_or("使い方: release-notes <vX.Y.Z> [--repo owner/name]")?;
    let root = std::env::current_dir().map_err(|err| format!("cwd を読めません: {err}"))?;
    print!("{}", release_notes(&root, &tag, repo.as_deref())?);
    Ok(())
}

fn main() {
    if std::env::args().nth(1).as_deref() == Some("render") {
        std::process::exit(render_main());
    }
    if let Err(err) = run() {
        eprintln!("release-notes: {err}");
        std::process::exit(1);
    }
}
