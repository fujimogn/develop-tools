//! `op` の 1 回分の起動と、失敗の固定カテゴリ。
//!
//! **ここ以外から `op` を起動しない。** 値を出さない約束（[`Secret`]）と、失敗を固定カテゴリへ
//! 縮める約束（生の stderr を外へ出さない）を 1 か所で守るため。

use std::ffi::OsString;
use std::io::ErrorKind;
use std::process::{Command, Stdio};

use crate::secret::Secret;

/// `op` が未認証のときの exit code。**未認証の判定は文言ではなくこの値で行う。**
///
/// 旧実装（7 repo）は stderr を `/sign|account|authoriz|not currently/i` で読んでいた。
/// service account の「vault に権限が無い」系の文言にも `account` が入るので、1 件の権限不足で
/// 全件を「未認証」として打ち切りうる（調査記録 §3.3-5）。`infra` も同じ値を使う。
pub const EXIT_NOT_SIGNED_IN: i32 = 6;

/// 失敗の固定カテゴリ。**表示はこの語だけ**で、op の生の stderr は出さない（stderr には
/// vault・item の構成や、場合によっては値の断片が載りうる）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Failure {
    /// `op signin` がまだ、または service account の token が無効。
    NotSignedIn,
    /// `op` が PATH に無い。
    OpMissing,
    /// 読めたが値が空。
    Empty,
    /// 参照先（vault / item / field）が無い、または見る権限が無い。
    NotFound,
    /// 1Password デスクトップアプリへの接続が詰まった（並列読みで単発に起きる）。
    DesktopConnect,
    /// それ以外。
    Failed,
}

impl Failure {
    /// 表示語。子 repo の旧実装の語（`認証が必要` など）に揃え、`参照先なし` だけ足した。
    pub fn label(self) -> &'static str {
        match self {
            Self::NotSignedIn => "認証が必要",
            Self::OpMissing => "opコマンドなし",
            Self::Empty => "値が空",
            Self::NotFound => "参照先なし",
            Self::DesktopConnect => "デスクトップ接続失敗",
            Self::Failed => "取得失敗",
        }
    }

    /// 間隔を空けて読み直す価値があるか。決定的な失敗は読み直さない。
    pub fn is_transient(self) -> bool {
        matches!(self, Self::DesktopConnect | Self::Failed)
    }

    /// 残りの参照を読んでも同じ理由で落ちるか。**最初の 1 件がこれなら残りを読まない。**
    pub fn aborts_all(self) -> bool {
        matches!(self, Self::NotSignedIn | Self::OpMissing)
    }
}

/// `op` の起動。テストは実物の代わりに偽物を渡す。
///
/// 並列に読むので `Sync` を求める。
pub trait OpClient: Sync {
    /// `op read <reference>` の値。
    fn read(&self, reference: &str) -> Result<Secret, Failure>;
    /// vault を読めるか（service account の事前確認）。
    fn vault_access(&self, vault: &str) -> Result<(), Failure>;
}

/// PATH 上の `op` を起動する。
pub struct OpCli {
    program: OsString,
    /// file から読んだ token。`op` の子プロセスにだけ渡す（env から来たときは `None` で継承）。
    token: Option<Secret>,
}

impl Default for OpCli {
    fn default() -> Self {
        Self {
            program: OsString::from("op"),
            token: None,
        }
    }
}

impl OpCli {
    pub fn with_token(token: Option<Secret>) -> Self {
        Self {
            token,
            ..Self::default()
        }
    }

    fn command(&self, args: &[&str]) -> Command {
        let mut command = Command::new(&self.program);
        // **stdin を塞がない。** 塞ぐと `op` が 1Password デスクトップアプリの承認待ちへ入れず
        // `connecting to desktop app timed out` で落ちる。承認がキャッシュ済みの端末では成功する
        // ので、承認が要る端末でだけ露見する（ADR-0014 v3、調査記録 §3.3-4）。
        command
            .args(args)
            .stdin(Stdio::inherit())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        crate::preflight::pass_to_child(&mut command, self.token.as_ref());
        command
    }
}

impl OpClient for OpCli {
    fn read(&self, reference: &str) -> Result<Secret, Failure> {
        let output = self
            .command(&["read", reference])
            .output()
            .map_err(spawn_failure)?;
        if !output.status.success() {
            return Err(classify(
                output.status.code(),
                &String::from_utf8_lossy(&output.stderr),
            ));
        }
        // 値を文字に直せないなら `.env` に書けない。lossy で置き換えると値が黙って変わる。
        let raw = String::from_utf8(output.stdout).map_err(|_| Failure::Failed)?;
        let value = Secret::new(strip_one_newline(&raw));
        if value.is_empty() {
            return Err(Failure::Empty);
        }
        Ok(value)
    }

    fn vault_access(&self, vault: &str) -> Result<(), Failure> {
        let mut command = self.command(&["vault", "get", vault, "--format=json"]);
        // vault の metadata は使わない。受け取らずに捨てる。
        command.stdout(Stdio::null());
        let output = command.output().map_err(spawn_failure)?;
        if output.status.success() {
            return Ok(());
        }
        Err(classify(
            output.status.code(),
            &String::from_utf8_lossy(&output.stderr),
        ))
    }
}

fn spawn_failure(error: std::io::Error) -> Failure {
    if error.kind() == ErrorKind::NotFound {
        Failure::OpMissing
    } else {
        Failure::Failed
    }
}

/// `op read` は値の後に改行を 1 つ付ける。**それだけを落とす**（trim しない）。
///
/// 全体を trim すると、PEM の末尾改行のように意味のある空白が変わる（`infra` と同じ扱い）。
fn strip_one_newline(raw: &str) -> &str {
    raw.strip_suffix('\n')
        .map(|value| value.strip_suffix('\r').unwrap_or(value))
        .unwrap_or(raw)
}

/// 失敗した `op` の結果を固定カテゴリへ縮める。**未認証は exit code だけで決める。**
///
/// stderr を見るのは、読み直すかどうか（接続の詰まり / 参照先なし）を分けるためだけ。
/// 文言が変わっても「取得失敗」へ落ちて読み直されるだけで、未認証の誤判定は起きない。
pub fn classify(exit_code: Option<i32>, stderr: &str) -> Failure {
    if exit_code == Some(EXIT_NOT_SIGNED_IN) {
        return Failure::NotSignedIn;
    }
    let stderr = stderr.to_ascii_lowercase();
    if stderr.contains("connecting to desktop app") {
        return Failure::DesktopConnect;
    }
    const NOT_FOUND: [&str; 5] = [
        "isn't an item",
        "isn't a field",
        "isn't a vault",
        "no item found",
        "not found",
    ];
    if NOT_FOUND.iter().any(|phrase| stderr.contains(phrase)) {
        return Failure::NotFound;
    }
    Failure::Failed
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 未認証は exit code 6 だけで決まる。`account` を含む権限不足の文言では打ち切らない
    /// （調査記録 §3.3-5 の正規表現 `/sign|account|authoriz|not currently/i` との差）。
    #[test]
    fn only_exit_code_six_means_not_signed_in() {
        assert_eq!(classify(Some(6), "anything at all"), Failure::NotSignedIn);
        for stderr in [
            "[ERROR] You are not currently signed in",
            "[ERROR] the service account does not have access to this vault in your account",
            "authorization failed for this request",
        ] {
            let failure = classify(Some(1), stderr);
            assert_ne!(failure, Failure::NotSignedIn, "{stderr}");
            assert!(!failure.aborts_all(), "{stderr}");
        }
    }

    #[test]
    fn transient_and_deterministic_failures_are_separated() {
        assert_eq!(
            classify(Some(1), "[ERROR] connecting to desktop app timed out"),
            Failure::DesktopConnect
        );
        assert_eq!(
            classify(Some(1), "[ERROR] \"x\" isn't an item in the \"v\" vault"),
            Failure::NotFound
        );
        assert_eq!(classify(Some(1), "network is unreachable"), Failure::Failed);
        assert_eq!(classify(None, ""), Failure::Failed);
        assert!(Failure::DesktopConnect.is_transient());
        assert!(Failure::Failed.is_transient());
        for failure in [
            Failure::NotSignedIn,
            Failure::OpMissing,
            Failure::Empty,
            Failure::NotFound,
        ] {
            assert!(!failure.is_transient(), "{failure:?}");
        }
    }

    #[test]
    fn only_the_newline_op_adds_is_removed() {
        assert_eq!(strip_one_newline("value\n"), "value");
        assert_eq!(strip_one_newline("value\r\n"), "value");
        assert_eq!(strip_one_newline("pem\n\n"), "pem\n");
        assert_eq!(strip_one_newline("  spaced  "), "  spaced  ");
    }
}
