use develop_release::release_notes::release_notes;
use develop_tools_core::cli_help::Help;

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
                        "release-notes <vX.Y.Z> [--repo owner/name]",
                        "CHANGELOG の前 tag からの月別追記を GitHub Release 本文として出力します。",
                        "前 tag と repo を git から決め、Markdown を標準出力します。",
                        "なし。CHANGELOG.md と git 履歴を読み取ります。",
                    )
                    .exit("0 = 生成成功、1 = tag・repo・git 履歴の不備")
                    .examples(&[
                        "release-notes v0.5.0",
                        "release-notes v0.5.0 --repo owner/project"
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
    if let Err(err) = run() {
        eprintln!("release-notes: {err}");
        std::process::exit(1);
    }
}
