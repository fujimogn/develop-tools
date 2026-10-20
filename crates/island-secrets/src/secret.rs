//! 秘密の値を包む型。**この型を通す限り、うっかりログへ出せない。**
//!
//! 移設前はコメントで「値をログに出さない」と書いてあるだけだった。コメントは
//! 次に触る人が読むとは限らないので、**機械で守れる形**にしてある。
//!
//! - `Debug` も `Display` も伏せ字しか出さない（`derive` しない — 派生させると中身が出る）
//! - 取り出せるのは [`Secret::expose`] だけ。**この名前で grep すれば、値が外へ出る箇所を
//!   全部数えられる**
//! - `Serialize` を実装しない（JSON 出力へ紛れ込ませない）
//!
//! `infra` の 1Password 読み書きも同じ型を使う（`infra::secrets::Secret` は再公開）。

use std::fmt;

/// 1Password から読んだ値など、出力してはいけない文字列。
#[derive(Clone, PartialEq, Eq)]
pub struct Secret(String);

impl Secret {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// **値を外へ出す唯一の口。** 呼ぶ側は、その先がログ・stdout・ファイルでないことに
    /// 責任を持つ。`expose` で grep すれば箇所を数えられる。
    pub fn expose(&self) -> &str {
        &self.0
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// 長さだけは見てよい（空判定・最小長の検査に要る）。
    pub fn len(&self) -> usize {
        self.0.len()
    }
}

/// **`{:?}` でも中身を出さない。** `derive(Debug)` にすると出る。
impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Secret(***)")
    }
}

/// **`{}` でも中身を出さない。** `println!("{token}")` を書いても伏せ字になる。
impl fmt::Display for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("***")
    }
}

#[cfg(test)]
mod tests;
