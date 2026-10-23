//! Messages API を用途の表で呼ぶ核。
//!
//! - [`purpose`] — 用途の表（model 名はここにだけ書く）
//! - [`settings`] — 環境側の非公開の設定
//! - [`transport`] — 送信（test は fixture に置き換える）
//! - [`usage`] — usage 台帳
//!
//! 推論は 1 回だけで再試行しない。失敗は [`Failure::code`] の理由 code で返し、fallback に
//! するかは呼び出し側が決める。

mod key;
mod output;
pub mod purpose;
pub mod settings;
pub mod transport;
pub mod usage;

use std::path::PathBuf;

use serde_json::Value;

use crate::purpose::Purpose;
use crate::transport::{ApiRequest, FailureKind, Transport};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyRef(String);

impl KeyRef {
    pub fn new(value: impl Into<String>) -> Option<Self> {
        let value = value.into();
        (!value.trim().is_empty()).then_some(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

pub struct Runtime<'a> {
    pub home: PathBuf,
    pub op_program: PathBuf,
    pub op_service_account_token: Option<String>,
    pub ledger: PathBuf,
    pub transport: &'a dyn Transport,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Output {
    Line(String),
    Json(Value),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Failure {
    UnknownPurpose,
    InputRejected,
    BudgetExceeded,
    KeyMissing,
    ApiTimeout,
    ApiFailed,
    UsageUnrecorded,
    ResponseRejected,
}

impl Failure {
    pub fn code(&self) -> &'static str {
        match self {
            Self::UnknownPurpose => "unknown_purpose",
            Self::InputRejected => "input_rejected",
            Self::BudgetExceeded => "budget_exceeded",
            Self::KeyMissing => "key_missing",
            Self::ApiTimeout => "api_timeout",
            Self::ApiFailed => "api_failed",
            Self::UsageUnrecorded => "usage_unrecorded",
            Self::ResponseRejected => "response_rejected",
        }
    }
}

pub fn check_budget(purpose: &Purpose, input_bytes: usize) -> Result<(), Failure> {
    if purpose.estimate_micro_usd(input_bytes) > purpose.max_cost_micro_usd {
        return Err(Failure::BudgetExceeded);
    }
    Ok(())
}

/// 用途 → 入力 → 費用 → 鍵 → 送信 → 台帳 → 出力形 の順に確かめる。送る前に止まれるものは
/// 先に止め、送った後は応答が使えなくても台帳に 1 行残す。
pub fn run(
    purpose_name: &str,
    input: &[u8],
    key_ref: &KeyRef,
    runtime: &Runtime<'_>,
) -> Result<Output, Failure> {
    let purpose = purpose::find(purpose_name).ok_or(Failure::UnknownPurpose)?;
    let input = std::str::from_utf8(input).map_err(|_| Failure::InputRejected)?;
    if input.len() > purpose.max_input_bytes {
        return Err(Failure::InputRejected);
    }
    check_budget(purpose, input.len())?;
    let api_key = key::resolve(key_ref, runtime).ok_or(Failure::KeyMissing)?;

    let body = serde_json::json!({
        "model": purpose.model,
        "max_tokens": purpose.max_tokens,
        "messages": [{"role": "user", "content": format!("{}{input}", purpose.instruction)}],
    });
    let sent = runtime.transport.send(&ApiRequest {
        body,
        api_key: &api_key,
    });
    let response = match sent {
        Ok(response) => response,
        Err(error) => {
            // 送った後の失敗も、取れた分だけ台帳に残してから返す。
            usage::append(
                &runtime.ledger,
                purpose.name,
                error.request_id.as_deref(),
                &Value::Null,
            )
            .map_err(|_| Failure::UsageUnrecorded)?;
            return Err(match error.kind {
                FailureKind::Timeout => Failure::ApiTimeout,
                FailureKind::Failed => Failure::ApiFailed,
            });
        }
    };

    let parsed = serde_json::from_str::<Value>(&response.body).ok();
    let request_id = response.request_id.clone().or_else(|| {
        parsed
            .as_ref()
            .and_then(|value| value.get("request_id"))
            .and_then(Value::as_str)
            .map(str::to_owned)
    });
    let usage = parsed
        .as_ref()
        .and_then(|value| value.get("usage"))
        .cloned()
        .unwrap_or(Value::Null);
    usage::append(&runtime.ledger, purpose.name, request_id.as_deref(), &usage)
        .map_err(|_| Failure::UsageUnrecorded)?;

    if !(200..300).contains(&response.status) {
        return Err(Failure::ApiFailed);
    }
    let text = parsed
        .as_ref()
        .and_then(extract_text)
        .ok_or(Failure::ResponseRejected)?;
    output::parse(purpose.output, text).ok_or(Failure::ResponseRejected)
}

fn extract_text(response: &Value) -> Option<&str> {
    response
        .get("content")?
        .as_array()?
        .iter()
        .find(|item| item.get("type").and_then(Value::as_str) == Some("text"))?
        .get("text")?
        .as_str()
}
