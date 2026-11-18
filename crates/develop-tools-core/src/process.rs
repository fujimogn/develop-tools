//! 子の失敗の組み立て。exit・signal と理由の token はここだけが作る。
//!
//! `develop-core` は同じ名前を再公開する。呼び出し側の path は変えない。

use std::process::{ExitStatus, Output};

/// A child's safe reason and termination values, without diagnostic text or paths.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChildFailure {
    pub reason: String,
    pub exit: Option<i32>,
    pub signal: Option<i32>,
}

impl ChildFailure {
    pub fn new(reason: Option<&str>, exit: Option<i32>, signal: Option<i32>) -> Self {
        let fallback = if signal.is_some() {
            "child_signal"
        } else {
            "child_exit"
        };
        Self {
            reason: reason
                .filter(|reason| is_reason_token(reason))
                .unwrap_or(fallback)
                .to_owned(),
            exit,
            signal,
        }
    }

    pub fn from_status(status: ExitStatus, reason: Option<&str>) -> Self {
        use std::os::unix::process::ExitStatusExt;
        Self::new(reason, status.code(), status.signal())
    }

    /// 子の stdout / stderr から理由の token を取り、終了と組む。stderr の本文は含めない。
    pub fn from_output(output: &Output) -> Self {
        let token = child_reason_token(&output.stdout, &output.stderr, usize::MAX);
        Self::from_status(output.status, token.as_deref())
    }
}

impl From<&'static str> for ChildFailure {
    fn from(reason: &'static str) -> Self {
        Self::new(Some(reason), None, None)
    }
}

impl std::fmt::Display for ChildFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}", self.reason)?;
        if let Some(exit) = self.exit {
            write!(formatter, " / exit={exit}")?;
        }
        if let Some(signal) = self.signal {
            write!(formatter, " / signal={signal}")?;
        }
        Ok(())
    }
}

/// Only a reason code, never free text or a path.
pub fn is_reason_token(value: &str) -> bool {
    let mut chars = value.chars();
    match chars.next() {
        Some(first) if first.is_ascii_lowercase() => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

/// Prefer stdout JSON's reason. Otherwise accept one stderr line that is a token,
/// or a token followed by `;`, or by `:` when the token contains `_`.
/// The caller supplies its capture limit.
pub fn child_reason_token(stdout: &[u8], stderr: &[u8], output_limit: usize) -> Option<String> {
    if stdout.len() >= output_limit || stderr.len() >= output_limit {
        return None;
    }
    if let Ok(value) = serde_json::from_slice::<serde_json::Value>(stdout) {
        if let Some(reason) = value.get("reason").and_then(|item| item.as_str()) {
            if is_reason_token(reason) {
                return Some(reason.to_owned());
            }
        }
    }
    let text = std::str::from_utf8(stderr).ok()?;
    let trimmed = text.trim_end_matches(['\n', '\r']);
    if trimmed.chars().any(|c| c == '\n' || c == '\r') {
        return None;
    }
    leading_reason_token(trimmed).map(str::to_owned)
}

/// A whole-line token, `token;` before a child termination, or `token:sub`
/// when `token` contains `_`. A `:` after a single word is a git or cargo
/// prefix (`fatal`, `error`, `warning`, `hint`, `usage`), not an island code.
/// Island codes are snake_case, so `_` rejects those prefixes without a
/// denylist that the next prefix would miss.
fn leading_reason_token(line: &str) -> Option<&str> {
    if is_reason_token(line) {
        return Some(line);
    }
    let bytes = line.as_bytes();
    if bytes.first().is_none_or(|byte| !byte.is_ascii_lowercase()) {
        return None;
    }
    let end = bytes
        .iter()
        .position(|byte| !byte.is_ascii_lowercase() && !byte.is_ascii_digit() && *byte != b'_')?;
    let token = &line[..end];
    if !is_reason_token(token) {
        return None;
    }
    let mut rest = line[end..].chars();
    let separator = rest.next()?;
    if separator != ':' && separator != ';' {
        return None;
    }
    let after = rest.as_str();
    if separator == ':' && !token.contains('_') {
        return None;
    }
    if separator == ':' && is_reason_token(after) {
        return Some(token);
    }
    if separator == ':' {
        let (sub, tail) = after.split_once(';')?;
        if sub.is_empty() || sub.chars().any(char::is_whitespace) {
            return None;
        }
        return is_child_detail_continuation(tail).then_some(token);
    }
    is_child_detail_continuation(after).then_some(token)
}

/// The trailing child-termination display, when `input` ends with one.
/// `reason / exit=N`, `reason / signal=N`, or both. The reason may follow ` / ` or `; `.
pub fn child_termination_suffix(input: &str) -> Option<&str> {
    let numeric = |value: &str, prefix: &str| {
        value.strip_prefix(prefix).is_some_and(|number| {
            !number.is_empty() && number.bytes().all(|byte| byte.is_ascii_digit())
        })
    };
    let (body, last) = input.rsplit_once(" / ")?;
    let (body, has_signal) = if numeric(last, "signal=") {
        (body, true)
    } else if numeric(last, "exit=") {
        (body, false)
    } else {
        return None;
    };
    let body = if has_signal {
        match body.rsplit_once(" / ") {
            Some((earlier, exit)) if numeric(exit, "exit=") => earlier,
            _ => body,
        }
    } else {
        body
    };
    let slash_at = body.rfind(" / ").map(|index| index + 3);
    let semi_at = body.rfind("; ").map(|index| index + 2);
    let reason_at = match (slash_at, semi_at) {
        (Some(slash), Some(semi)) => slash.max(semi),
        (Some(slash), None) => slash,
        (None, Some(semi)) => semi,
        (None, None) => 0,
    };
    let reason = &body[reason_at..];
    is_reason_token(reason).then_some(&input[reason_at..])
}

fn is_child_detail_continuation(after: &str) -> bool {
    let after = after.strip_prefix(' ').unwrap_or(after);
    child_termination_suffix(after).is_some_and(|suffix| suffix.len() == after.len())
}
