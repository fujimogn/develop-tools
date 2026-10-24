//! push 前の版数 guard（lefthook から走る）。
//!
//! 宣言した版 file を常に照合し、tag があれば tag と main への包含を照合する。
//! `--pre-push` では stdin の送る tag ごとに、その tag の commit の版 file と main への包含も照合する。

use std::io::Read;
use std::path::Path;

use develop_release::{
    PushedTag, current_repo_root, pushed_release_tags, version_files::VersionFiles,
};
use develop_tools_core::{RELEASE_BRANCH, cli_help::Help};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|arg| arg == "--help" || arg == "-h") {
        let help = Help::new(
            "check-release-version [--pre-push] [--help]",
            "check-release-version — push 前に release tag と declared version files の整合性を検査します。",
            "宣言した全 file の版を常に照合します。release.versionFiles を宣言していない repo では作業木の照合を飛ばし、理由を stderr に 1 行出します（宣言が壊れているときは失敗します）。HEAD に v<semver> tag がある場合だけ tag と main への包含を確認します。--pre-push では Git pre-push の stdin を読み、送る refs/tags/v<semver> ごとに、その tag の commit の版 file と tag の一致、その commit の main への包含も確認します（削除の行と tag 以外の行は読み飛ばします）。repo の外では soft-skip します。",
            "なし。git tag、.develop-tools.json、release.versionFiles の file を読み取ります。--pre-push では stdin と tag の commit の同じ file を読み取ります。",
        )
        .exit("0 = release でない / 一致 / repo 外・版の宣言なしの soft-skip、1 = git の失敗、tag・版数の不一致、main 外の tag、または設定の読み取りエラー")
        .examples(&["check-release-version", "check-release-version --pre-push < <pre-push stdin>"]);
        print!("{}", help.render());
        return;
    }
    let pre_push = args.iter().any(|arg| arg == "--pre-push");

    let root = match current_repo_root() {
        Ok(Some(root)) => root,
        Ok(None) => return,
        Err(err) => fail(&err),
    };
    // 版を持たない repo は作業木の照合を飛ばす。送る tag の照合は宣言の有無にかかわらず行う。
    if VersionFiles::undeclared(&root) {
        eprintln!(
            "check-release-version: release.versionFiles の宣言が無いため版の照合を飛ばします"
        );
    } else {
        check_head(&root);
    }
    if pre_push {
        let mut input = String::new();
        if let Err(err) = std::io::stdin().read_to_string(&mut input) {
            fail(&format!("stdin を読めません: {err}"));
        }
        for pushed in pushed_release_tags(&input) {
            check_pushed(&root, &pushed);
        }
    }
}

fn fail(message: &str) -> ! {
    eprintln!("check-release-version: {message}");
    std::process::exit(1);
}

fn fail_tag(err: &str) -> ! {
    eprintln!("check-release-version: {err}");
    eprintln!(
        "  tag の食い違いは `release <patch | minor | major | x.y.z>` で打ち直してください。"
    );
    std::process::exit(1);
}

/// 作業木の版 file を照合し、HEAD の tag があれば tag と main への包含も照合する。
fn check_head(root: &Path) {
    let out = match develop_tools_core::git(root)
        .args(["tag", "--points-at", "HEAD"])
        .output()
    {
        Ok(out) => out,
        Err(err) => fail(&format!("git tag を起動できません: {err}")),
    };
    if !out.status.success() {
        fail(&format!(
            "git tag --points-at HEAD に失敗しました: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    let tags: Vec<String> = String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(|line| line.trim().to_owned())
        .filter(|line| !line.is_empty())
        .collect();

    let versions = VersionFiles::load(root).unwrap_or_else(|err| fail(&err));
    match versions.check_tags(&tags) {
        Ok(None) => {}
        Ok(Some(version)) => {
            require_in_main(root, "HEAD", &format!("v{version}"));
            println!("check-release-version: OK (v{version})");
        }
        Err(err) => fail_tag(&err),
    }
}

/// 送る tag の commit の版 file を読み、tag との一致と main への包含を照合する。
fn check_pushed(root: &Path, pushed: &PushedTag) {
    let out = match develop_tools_core::git(root)
        .args([
            "rev-parse",
            "--verify",
            "--quiet",
            &format!("{}^{{commit}}", pushed.object),
        ])
        .output()
    {
        Ok(out) => out,
        Err(err) => fail(&format!("git rev-parse を起動できません: {err}")),
    };
    if !out.status.success() {
        fail(&format!("{} の commit を解決できません", pushed.tag));
    }
    let commit = String::from_utf8_lossy(&out.stdout).trim().to_owned();
    let versions = VersionFiles::load_at(root, &commit)
        .unwrap_or_else(|err| fail(&format!("{}: {err}", pushed.tag)));
    if let Err(err) = versions.check_tags(std::slice::from_ref(&pushed.tag)) {
        fail_tag(&err);
    }
    require_in_main(root, &commit, &pushed.tag);
    println!("check-release-version: OK (push {})", pushed.tag);
}

fn require_in_main(root: &Path, commit: &str, tag: &str) {
    let in_main = develop_tools_core::git(root)
        .args([
            "merge-base",
            "--is-ancestor",
            commit,
            &format!("refs/heads/{RELEASE_BRANCH}"),
        ])
        .status();
    if !matches!(in_main, Ok(status) if status.success()) {
        fail(&format!(
            "{tag} の commit は {RELEASE_BRANCH} に含まれていません"
        ));
    }
}
