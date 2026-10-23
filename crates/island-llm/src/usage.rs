//! usage 台帳。行の形は `llm-api-entry` の台帳と同じ。置き場は呼び出し側が決める。

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::Path;

use serde::Serialize;
use serde_json::Value;

#[derive(Serialize)]
struct UsageRow<'a> {
    timestamp: &'a str,
    purpose: &'a str,
    request_id: Option<&'a str>,
    usage: &'a Value,
}

/// 1 回の送信につき jsonl に 1 行足す。
pub fn append(
    ledger: &Path,
    purpose: &str,
    request_id: Option<&str>,
    usage: &Value,
) -> std::io::Result<()> {
    if let Some(parent) = ledger.parent() {
        fs::create_dir_all(parent)?;
    }
    let timestamp = time::OffsetDateTime::now_utc()
        .format(&time::format_description::well_known::Rfc3339)
        .map_err(std::io::Error::other)?;
    let row = UsageRow {
        timestamp: &timestamp,
        purpose,
        request_id,
        usage,
    };
    let mut line = serde_json::to_vec(&row).map_err(std::io::Error::other)?;
    line.push(b'\n');
    OpenOptions::new()
        .create(true)
        .append(true)
        .open(ledger)?
        .write_all(&line)
}
