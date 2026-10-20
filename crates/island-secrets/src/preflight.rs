//! service account の事前確認。**読み始める前に**、token の有無と対象 vault を読めるかを見る。
//!
//! `op` は service account（`OP_SERVICE_ACCOUNT_TOKEN`）で引く。素の `op` は利用者の画面に
//! 承認 dialog を出し、headless の注入が止まる。

use std::fmt;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::Secret;
use crate::op::{Failure, OpClient};

pub const TOKEN_ENV: &str = "OP_SERVICE_ACCOUNT_TOKEN";
/// env が無いときに読む token の置き場（HOME 相対）。人の shell は env を持たないので、
/// ここから読んで `op` の子プロセスにだけ渡す（shell 全体へは export しない）。
pub const TOKEN_FILE: &str = ".config/op/service-account.token";

#[derive(Debug, PartialEq, Eq)]
pub enum PreflightError {
    TokenMissing,
    /// token file が安全でない（symlink・他人の所有・group / other から読める・読めない）。
    TokenFileUnsafe {
        path: PathBuf,
        reason: &'static str,
    },
    Vault {
        vault: String,
        failure: Failure,
    },
}

impl fmt::Display for PreflightError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TokenMissing => write!(
                f,
                "{TOKEN_ENV} も ~/{TOKEN_FILE} もありません。service account の token を置いてください"
            ),
            Self::TokenFileUnsafe { path, reason } => write!(
                f,
                "{} を token として使いません: {reason}（chmod 600 にし、自分の所有の通常 file にしてください）",
                path.display()
            ),
            Self::Vault { vault, failure } => {
                let hint = match failure {
                    Failure::NotSignedIn => "service account の token が無効です",
                    Failure::OpMissing => "op コマンドが見つかりません",
                    Failure::NotFound => {
                        "service account がこの vault を読めません（権限が無いか、名前が違います）"
                    }
                    _ => "vault を確かめられませんでした",
                };
                write!(f, "vault {vault:?}: {hint}（{}）", failure.label())
            }
        }
    }
}

/// `token_present` は `OP_SERVICE_ACCOUNT_TOKEN` が空でなく在るか。値そのものは受け取らない。
pub fn check(
    token_present: bool,
    client: &dyn OpClient,
    vaults: &[&str],
) -> Result<(), PreflightError> {
    if !token_present {
        return Err(PreflightError::TokenMissing);
    }
    for vault in vaults {
        client
            .vault_access(vault)
            .map_err(|failure| PreflightError::Vault {
                vault: (*vault).to_owned(),
                failure,
            })?;
    }
    Ok(())
}

/// token をどこから使うか。env に在れば `op` はそれを継ぐので値は持たない。
#[derive(Debug)]
pub enum Token {
    FromEnv,
    FromFile(Secret),
}

impl Token {
    /// `op` の子プロセスへ明示的に渡す値。env から来たときは継承に任せるので `None`。
    pub fn into_child_secret(self) -> Option<Secret> {
        match self {
            Self::FromEnv => None,
            Self::FromFile(secret) => Some(secret),
        }
    }
}

/// file から読んだ token を、この `op` の子プロセスの env にだけ置く。
///
/// **`op` を起動する道具はここを通す。** 自分の process env へ書くと、`op` 以外の子
/// （`gh`・通知など）まで token を継ぐ。
pub fn pass_to_child(command: &mut Command, token: Option<&Secret>) {
    if let Some(token) = token {
        command.env(TOKEN_ENV, token.expose());
    }
}

/// env を優先し、無ければ `home/TOKEN_FILE` を読む。どちらも無ければ `TokenMissing`。
///
/// 空白だけの env は無いものとして扱う（`op` はそれを token として使えない）。
pub fn locate_token(env_token: Option<&str>, home: Option<&Path>) -> Result<Token, PreflightError> {
    if env_token.is_some_and(|value| !value.trim().is_empty()) {
        return Ok(Token::FromEnv);
    }
    let Some(home) = home else {
        return Err(PreflightError::TokenMissing);
    };
    let path = home.join(TOKEN_FILE);
    match read_token_file(&path)? {
        Some(secret) => Ok(Token::FromFile(secret)),
        None => Err(PreflightError::TokenMissing),
    }
}

/// 安全な token file だけを読む。無ければ `None`。
fn read_token_file(path: &Path) -> Result<Option<Secret>, PreflightError> {
    use std::os::unix::fs::MetadataExt;
    let unsafe_file = |reason| PreflightError::TokenFileUnsafe {
        path: path.to_owned(),
        reason,
    };
    let meta = match std::fs::symlink_metadata(path) {
        Ok(meta) => meta,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err(unsafe_file("状態を読めません")),
    };
    if meta.file_type().is_symlink() {
        return Err(unsafe_file("symlink です"));
    }
    if !meta.is_file() {
        return Err(unsafe_file("通常の file ではありません"));
    }
    // SAFETY: geteuid は引数を取らず失敗しない。
    if meta.uid() != unsafe { libc::geteuid() } {
        return Err(unsafe_file("自分の所有ではありません"));
    }
    if meta.mode() & 0o077 != 0 {
        return Err(unsafe_file("group か other が読み書きできます"));
    }
    let text = std::fs::read_to_string(path).map_err(|_| unsafe_file("読めません"))?;
    let token = text.trim();
    Ok((!token.is_empty()).then(|| Secret::new(token)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_blank_env_token_falls_through_to_the_file() {
        let home = tempfile::tempdir().unwrap();
        assert_eq!(
            locate_token(Some("  "), Some(home.path())).unwrap_err(),
            PreflightError::TokenMissing
        );
        assert!(matches!(
            locate_token(Some("set"), Some(home.path())),
            Ok(Token::FromEnv)
        ));
    }
}
