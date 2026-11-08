use std::{collections::BTreeSet, env, path::Path};

use ci_change_scope::{Scope, changed_paths, decide_push_scope};

fn main() {
    let args: Vec<String> = env::args().skip(1).collect();
    if args
        .first()
        .is_some_and(|arg| arg == "--check-dependencies")
    {
        let result = (|| {
            let [_, path] = args.as_slice() else {
                return Err("coverage path required".to_owned());
            };
            let bytes = std::fs::read_to_string(path).map_err(|error| error.to_string())?;
            let covered: BTreeSet<String> =
                serde_json::from_str(&bytes).map_err(|error| error.to_string())?;
            ci_change_scope::dependency::check(Path::new("."), &covered).map(|_| ())
        })();
        if let Err(error) = result {
            eprintln!("ci-change-scope: dependency proof failed: {error}");
            std::process::exit(1);
        }
        return;
    }
    if args.len() == 1 && matches!(args[0].as_str(), "--help" | "-h") {
        print!(
            "{}",
            develop_tools_core::cli_help::Help::new(
                "ci-change-scope [--help] | --check-dependencies <coverage.json>",
                "GitHub push または指定 range の全 commit の変更 path を調べ、記録文書だけなら軽い CI を選ぶ。",
                "環境変数 GITHUB_EVENT_NAME、FORCED_PUSH、BEFORE_SHA、HEAD_SHA から判定し、light=true/false を stdout に出す。schedule / workflow_dispatch は明示 range を受け取り、不明なら false",
                "なし。Git 履歴を読み取るだけ",
            )
            .render()
        );
        return;
    }
    if !args.is_empty() {
        eprintln!("ci-change-scope: unknown arguments");
        std::process::exit(2);
    }
    let event = env::var("GITHUB_EVENT_NAME").unwrap_or_default();
    let before = env::var("BEFORE_SHA").unwrap_or_default();
    let head = env::var("HEAD_SHA").unwrap_or_default();
    let forced = match env::var("FORCED_PUSH").as_deref() {
        Ok("true") => Some(true),
        Ok("false") => Some(false),
        _ => None,
    };
    let paths = if matches!(event.as_str(), "push" | "schedule" | "workflow_dispatch")
        && forced == Some(false)
    {
        changed_paths(Path::new("."), &before, &head)
    } else {
        None
    };
    let scope = decide_push_scope(&event, forced, &before, &head, paths.as_deref());
    println!("light={}", scope == Scope::Light);
    if scope == Scope::Heavy {
        eprintln!("ci-change-scope: heavy (disallowed or unknown change scope)");
    }
}
