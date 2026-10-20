//! `.env` の quote・parser・merge。**書いた値を Bun の dotenv が読み戻すと同じ値になる**ことが契約。
//!
//! 基準は Bun（1.4 系、2026-09-30 に dev で実測）の dotenv。旧実装はここが崩れていた:
//!
//! - Bun は **single quote の中でも `$NAME` を展開する**。`'a$HOME'` は `a/home/…` になる。
//!   旧実装の「`'` を含まなければ single quote」は `$` を守れない。
//!   別の旧実装は double quote で `$` を escape しない（調査記録 §3.3-2）。
//! - Bun が外すのは `\$` → `$` だけで、`\"` `\\` `` \` `` の backslash は残る。したがって quote の
//!   文字そのものは値に入れられない。double quote の中では `\n` が改行になる。
//! - 閉じの quote は `\` の直後だと閉じにならない（`\\` は対で読み飛ばす）。値の末尾の
//!   backslash が奇数個だと閉じられない。値の中の CR は LF に変わる。
//!
//! そこで quote は「値に含まれない quote 文字を選び、`$` を全部 `\$` にする」とし、それでも
//! 表せない値（上の制約に当たるもの）は書かずに止める（[`Unrepresentable`]）。黙って崩すより、
//! 止めて気付かせる方が安い。

use std::fmt;

use crate::secret::Secret;

/// 追記する key の前に 1 度だけ置く見出し。
pub const MANAGED_HEADER: &str =
    "# --- 1Password から secrets:inject で生成 (宣言対象の key は次回 inject で更新) ---";

const QUOTES: [char; 3] = ['\'', '`', '"'];

/// Bun の dotenv で往復できない値。**値そのものは持たない。**
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unrepresentable {
    /// NUL か CR を含む（CR は Bun が LF に変える）。
    ControlCharacter,
    /// 末尾の backslash が奇数個（閉じの quote を escape してしまう）。
    TrailingBackslash,
    /// `'` と `` ` `` を両方含み、かつ `"` か backslash も含む。
    NoUsableQuote,
}

impl fmt::Display for Unrepresentable {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::ControlCharacter => "NUL か CR を含む値",
            Self::TrailingBackslash => "末尾の backslash が奇数個の値",
            Self::NoUsableQuote => "3 種類の quote 文字をすべて含む値",
        })
    }
}

/// `KEY=` の右辺を作る。
pub fn quote(value: &str) -> Result<String, Unrepresentable> {
    if value.contains(['\0', '\r']) {
        return Err(Unrepresentable::ControlCharacter);
    }
    let trailing_backslashes = value.chars().rev().take_while(|c| *c == '\\').count();
    if trailing_backslashes % 2 == 1 {
        return Err(Unrepresentable::TrailingBackslash);
    }
    // double quote は `\n` を改行に変えるので、backslash を含む値には使わない。
    let quote = QUOTES
        .into_iter()
        .find(|q| !value.contains(*q) && (*q != '"' || !value.contains('\\')))
        .ok_or(Unrepresentable::NoUsableQuote)?;
    let mut out = String::with_capacity(value.len() + 2);
    out.push(quote);
    for c in value.chars() {
        if c == '$' {
            out.push('\\');
        }
        out.push(c);
    }
    out.push(quote);
    Ok(out)
}

/// 読み取った 1 件。`lines` は元の行番号の範囲（複数行の quote 値を含む）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub key: String,
    pub value: String,
    pub lines: std::ops::Range<usize>,
}

/// Bun と同じ規則で読む。ただし unescaped の `$NAME` は展開しない（この writer は `$` を全部
/// escape するので、書いた値は展開の影響を受けない。他人が書いた行は key と行範囲だけが要る）。
pub fn parse(content: &str) -> Vec<Entry> {
    let lines: Vec<&str> = content.split('\n').collect();
    let mut entries = Vec::new();
    let mut index = 0;
    while index < lines.len() {
        let Some((key, rest)) = split_assignment(lines[index]) else {
            index += 1;
            continue;
        };
        let (value, last) = match rest.chars().next().filter(|c| QUOTES.contains(c)) {
            Some(quote) => match find_closing(&lines, index, rest, quote) {
                Some((inner, last)) => (decode(quote, &inner), last),
                None => (unquoted(rest), index),
            },
            None => (unquoted(rest), index),
        };
        entries.push(Entry {
            key: key.to_owned(),
            value,
            lines: index..last + 1,
        });
        index = last + 1;
    }
    entries
}

/// `[export ]KEY[ ]=[ ]rest` を分ける。`KEY =` も key として読む（読まないと、同じ key の行が
/// 重なる）。
fn split_assignment(line: &str) -> Option<(&str, &str)> {
    let line = line.strip_suffix('\r').unwrap_or(line).trim_start();
    let line = line
        .strip_prefix("export")
        .filter(|rest| rest.starts_with([' ', '\t']))
        .map_or(line, str::trim_start);
    let (key, rest) = line.split_once('=')?;
    let key = key.trim_end();
    crate::refs::is_env_key(key).then_some((key, rest.trim_start()))
}

/// `rest` の先頭の quote に対応する閉じを、次の行以降まで探す。`\` は次の 1 文字を読み飛ばす。
fn find_closing(lines: &[&str], start: usize, rest: &str, quote: char) -> Option<(String, usize)> {
    let mut inner = String::new();
    let mut escaped = false;
    for (offset, line) in std::iter::once(&rest[quote.len_utf8()..])
        .chain(lines[start + 1..].iter().copied())
        .enumerate()
    {
        if offset > 0 {
            inner.push('\n');
        }
        let line = line.strip_suffix('\r').unwrap_or(line);
        for c in line.chars() {
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == quote {
                return Some((inner, start + offset));
            }
            inner.push(c);
        }
        escaped = false;
    }
    None
}

fn decode(quote: char, inner: &str) -> String {
    let newlines = if quote == '"' {
        expand_newlines(inner)
    } else {
        inner.to_owned()
    };
    unescape_dollar(&newlines)
}

/// double quote の `\n` を改行にする。`\\` は対のまま残す（Bun は `"\\n"` を `\\n` のまま返す）。
fn expand_newlines(inner: &str) -> String {
    let mut out = String::with_capacity(inner.len());
    let mut chars = inner.chars().peekable();
    while let Some(c) = chars.next() {
        match (c, chars.peek()) {
            ('\\', Some('n')) => {
                chars.next();
                out.push('\n');
            }
            ('\\', Some('\\')) => {
                chars.next();
                out.push_str("\\\\");
            }
            _ => out.push(c),
        }
    }
    out
}

/// `\$` → `$`。他の backslash は残す（Bun と同じ。対にはしない: `\\$` は `\$` になる）。
fn unescape_dollar(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\\' && chars.peek() == Some(&'$') {
            continue;
        }
        out.push(c);
    }
    out
}

/// quote の無い値: 空白で始まる `#` 以降を落とし、前後の空白を落とす。
fn unquoted(rest: &str) -> String {
    let rest = rest.strip_suffix('\r').unwrap_or(rest);
    let end = rest
        .char_indices()
        .find(|(i, c)| *c == '#' && (*i == 0 || rest[..*i].ends_with([' ', '\t'])))
        .map_or(rest.len(), |(i, _)| i);
    unescape_dollar(rest[..end].trim())
}

/// merge できなかった key と理由。値は含まない。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MergeError {
    pub key: String,
    pub reason: Unrepresentable,
}

impl fmt::Display for MergeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} は .env に表せません（{}）", self.key, self.reason)
    }
}

/// 既存の内容へ `values` を merge する。**戻り値は secret を含む**ので [`Secret`] で返す。
///
/// - 管理対象の key は、最初の行（複数行の値なら全行）を置き換え、2 回目以降の同じ key は消す。
///   残すと、読み手によっては古い値が勝つ。
/// - 管理対象でない行（コメント・空行・他の key）はそのまま残す。
/// - 既存に無い key は末尾へ、見出しの後に key 順で足す。
pub fn merge(existing: &str, values: &[(String, Secret)]) -> Result<Secret, MergeError> {
    let mut quoted: Vec<(&str, String)> = values
        .iter()
        .map(|(key, value)| {
            quote(value.expose())
                .map(|q| (key.as_str(), q))
                .map_err(|reason| MergeError {
                    key: key.clone(),
                    reason,
                })
        })
        .collect::<Result<_, _>>()?;
    quoted.sort_by(|a, b| a.0.cmp(b.0));
    let lookup = |key: &str| {
        quoted
            .binary_search_by(|(k, _)| (*k).cmp(key))
            .ok()
            .map(|i| &quoted[i])
    };

    let lines: Vec<&str> = if existing.is_empty() {
        Vec::new()
    } else {
        existing.split('\n').collect()
    };
    let mut replaced: Vec<&str> = Vec::new();
    let mut out: Vec<String> = Vec::with_capacity(lines.len() + quoted.len() + 2);
    let mut entries = parse(existing).into_iter().peekable();
    let mut index = 0;
    while index < lines.len() {
        if let Some(entry) = entries.next_if(|e| e.lines.start == index) {
            if let Some((key, value)) = lookup(&entry.key) {
                if !replaced.contains(key) {
                    out.push(format!("{key}={value}"));
                    replaced.push(key);
                }
                index = entry.lines.end;
                continue;
            }
            for line in &lines[entry.lines.clone()] {
                out.push((*line).to_owned());
            }
            index = entry.lines.end;
            continue;
        }
        out.push(lines[index].to_owned());
        index += 1;
    }
    while out.last().is_some_and(String::is_empty) {
        out.pop();
    }

    let appended: Vec<String> = quoted
        .iter()
        .filter(|(key, _)| !replaced.contains(key))
        .map(|(key, value)| format!("{key}={value}"))
        .collect();
    let mut text = out.join("\n");
    if !appended.is_empty() {
        if !text.is_empty() {
            text.push_str("\n\n");
        }
        text.push_str(MANAGED_HEADER);
        text.push('\n');
        text.push_str(&appended.join("\n"));
    }
    if !text.is_empty() {
        text.push('\n');
    }
    Ok(Secret::new(text))
}

#[cfg(test)]
mod tests;
