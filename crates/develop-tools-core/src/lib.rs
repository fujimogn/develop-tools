//! develop tools が共有する最小の土台。
//!
//! 公開する道具が使う部分を置く。配る側の非公開の土台も、これを re-export して同じものを使う。
//! 島の台帳・置き場の名前・島の path は持たない（それらは配る側の非公開の土台が持つ）。
//!
//! - [`strip_git_env`] — 継いだ `GIT_*` を落とす。誤ると別 repo の `.git` を書き換える。
//! - [`managed_block`] — 配布と検査が同じ marker 規則で管理区画を読む。
//! - [`github_repo`] — GitHub の `owner/repo` の解析。host の部分一致を残すと別 repo を指す。

use std::path::Path;
use std::process::Command;

mod github_repo;
pub use github_repo::github_repo;

pub mod cli_help;
pub mod develop_tools_manifest;
pub mod managed_block;
#[cfg(test)]
mod managed_block_tests;

/// Release のみが進める branch。日常統合先は repo の `.develop-tools.json` の `integrationBranch` から読む。
pub const RELEASE_BRANCH: &str = "main";

pub mod freshness {
    //! 手元の binary と repo が固定した版の比較。
    use std::path::Path;

    /// 子 repo の固定版より手元の Release binary が古いときだけ案内する。
    /// 固定 file を読めない・形式が不明な場合は hook を止めない。
    pub fn pinned_version_warning(manifest: &Path, local: &str) -> Option<String> {
        fn version(raw: &str) -> Option<[u64; 3]> {
            let parts: Vec<_> = raw.split('.').collect();
            if parts.len() != 3 {
                return None;
            }
            let mut numbers = [0; 3];
            for (number, part) in numbers.iter_mut().zip(parts) {
                if part.is_empty() || (part.len() > 1 && part.starts_with('0')) {
                    return None;
                }
                *number = part.parse().ok()?;
            }
            Some(numbers)
        }
        let bytes = std::fs::read(manifest).ok()?;
        let value: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
        let pinned = value.get("version")?.as_str()?;
        (version(local)? < version(pinned)?).then(|| {
            format!(
                "develop tools: 手元の binary {local} は固定版 {pinned} より古いです。固定版の道具を入れ直してください"
            )
        })
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn pinned_version_warns_only_when_local_is_older() {
            let dir = tempfile::tempdir().unwrap();
            let pin = dir.path().join(".develop-tools.json");
            std::fs::write(&pin, r#"{"version":"1.2.3","sha256":{}}"#).unwrap();
            let warning = pinned_version_warning(&pin, "1.2.2").unwrap();
            assert!(warning.contains("1.2.2"));
            assert!(warning.contains("1.2.3"));
            assert!(warning.contains("入れ直して"));
            assert_eq!(pinned_version_warning(&pin, "1.2.3"), None);
            assert_eq!(pinned_version_warning(&pin, "1.3.0"), None);
            assert_eq!(pinned_version_warning(&pin, "bad"), None);
            std::fs::write(&pin, "not JSON").unwrap();
            assert_eq!(pinned_version_warning(&pin, "1.2.2"), None);
            std::fs::remove_file(&pin).unwrap();
            assert_eq!(pinned_version_warning(&pin, "1.2.2"), None);
        }
    }
}

/// Release の日常統合先。repo の `.develop-tools.json` だけを読み、file が無ければ失敗する
/// （置き忘れた repo で main から版を上げないため）。欄が無ければ main。
pub fn release_integration_branch(repo_root: &Path) -> Result<Option<String>, String> {
    let pin = repo_root.join(develop_tools_manifest::PATH);
    let source = match std::fs::read_to_string(&pin) {
        Ok(source) => source,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Err(format!(
                "{} がありません（日常統合先 integrationBranch を宣言してください。main だけで運用するなら {{}} を置けば main 扱い）",
                pin.display()
            ));
        }
        Err(error) => return Err(format!("{}: {error}", pin.display())),
    };
    let value: serde_json::Value =
        serde_json::from_str(&source).map_err(|error| format!("{}: {error}", pin.display()))?;
    let declared = develop_tools_manifest::declaration(&value)
        .map_err(|error| format!("{}: {error}", pin.display()))?;
    let branch = declared
        .integration_branch
        .unwrap_or_else(|| RELEASE_BRANCH.to_owned());
    let valid = git(repo_root)
        .args(["check-ref-format", "--branch", &branch])
        .output()
        .map_err(|error| format!("integrationBranch 検証失敗: {error}"))?
        .status
        .success();
    if !valid {
        return Err(format!("{}: 不正な integrationBranch", pin.display()));
    }
    Ok(Some(branch))
}

/// 継いだ `GIT_*` を落とす。
///
/// 継いだ `GIT_*` は repo ローカルの変数に限らず全て外す。repo ローカルの変数だけを外す
/// 実装とは、意図して契約を分けている。
///
/// ⚠️ **`GIT_DIR` は `git -C` にも [`Command::current_dir`] にも勝つ**。
/// hook から走り得るものが `git` を起動するなら必ず通す。実際に `core.bare = true` を
/// 別 repo へ書き込んで checkout を 3 度壊している。
pub fn strip_git_env(command: &mut Command) {
    for (key, _) in std::env::vars_os() {
        if key.as_os_str().as_encoded_bytes().starts_with(b"GIT_") {
            command.env_remove(key);
        }
    }
}

/// `dir` を cwd にした `git` を、継いだ `GIT_*` を落とした状態で組み立てる。
///
/// `git` を起動する箇所はこれを通す。[`strip_git_env`] を呼び忘れる余地を無くすための入口。
pub fn git(dir: &Path) -> Command {
    let mut command = Command::new("git");
    strip_git_env(&mut command);
    command.current_dir(dir);
    command
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn release_branch_prefers_repo_pin_and_main_when_branch_is_absent() {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().join("repo");
        std::fs::create_dir(&repo).unwrap();
        std::fs::write(
            repo.join(".develop-tools.json"),
            r#"{"integrationBranch":"develop"}"#,
        )
        .unwrap();
        assert_eq!(
            release_integration_branch(&repo).unwrap().as_deref(),
            Some("develop")
        );
        std::fs::write(repo.join(".develop-tools.json"), r#"{"version":"0.1.0"}"#).unwrap();
        assert_eq!(
            release_integration_branch(&repo).unwrap().as_deref(),
            Some("main")
        );
        std::fs::write(repo.join(".develop-tools.json"), "null").unwrap();
        assert!(release_integration_branch(&repo).is_err());
    }

    #[test]
    fn release_branch_fails_without_repo_pin() {
        let dir = tempfile::tempdir().unwrap();
        let error = release_integration_branch(dir.path()).unwrap_err();
        assert!(error.contains(".develop-tools.json"), "{error}");
    }

    /// 落とし忘れると別 repo の `.git` を書き換える。組み立てた時点で消えていること。
    #[test]
    fn the_git_builder_drops_inherited_git_env() {
        const CHILD: &str = "DEVELOP_CORE_GIT_ENV_TEST_CHILD";
        if std::env::var_os(CHILD).is_none() {
            // 汚染はこの test だけを走らせる子に限定し、並走する test へ漏らさない。
            let out = Command::new(std::env::current_exe().expect("test executable"))
                .args([
                    "--exact",
                    "tests::the_git_builder_drops_inherited_git_env",
                    "--nocapture",
                ])
                .env(CHILD, "1")
                .env("GIT_DIR", "/nowhere/.git")
                .output()
                .expect("isolated test");
            assert!(
                out.status.success(),
                "isolated git env test failed:\n{}\n{}",
                String::from_utf8_lossy(&out.stdout),
                String::from_utf8_lossy(&out.stderr)
            );
            return;
        }
        assert_eq!(std::env::var("GIT_DIR").as_deref(), Ok("/nowhere/.git"));
        let dir = tempfile::tempdir().expect("tempdir");
        let init = git(dir.path())
            .args(["init", "-q"])
            .output()
            .expect("git init");
        assert!(
            init.status.success(),
            "{}",
            String::from_utf8_lossy(&init.stderr)
        );
        let out = git(dir.path())
            .args(["rev-parse", "--is-inside-work-tree"])
            .output()
            .expect("git");
        assert!(
            out.status.success(),
            "継いだ GIT_DIR が子へ渡っている: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "true");
        assert_eq!(
            std::env::var("GIT_DIR").as_deref(),
            Ok("/nowhere/.git"),
            "test must preserve its inherited environment"
        );
    }
}
