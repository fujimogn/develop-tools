//! Messages API への送信。test は [`Transport`] を fixture で置き換え、本物の API を呼ばない。

use std::time::Duration;

use island_secrets::Secret;
use serde_json::Value;

const API_VERSION: &str = "2023-06-01";
const API_PATH: &str = "/v1/messages";

pub struct ApiRequest<'a> {
    pub body: Value,
    pub api_key: &'a Secret,
}

#[derive(Debug, Clone)]
pub struct ApiResponse {
    pub status: u16,
    pub request_id: Option<String>,
    pub body: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailureKind {
    Timeout,
    Failed,
}

/// 送信の失敗。header まで届いていれば `request_id` を運ぶ（API 側で処理・課金されていても
/// 台帳に残せるように）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransportError {
    pub kind: FailureKind,
    pub request_id: Option<String>,
}

pub trait Transport {
    fn send(&self, request: &ApiRequest<'_>) -> Result<ApiResponse, TransportError>;
}

/// 本番の送信。HTTP の status は error にせず [`ApiResponse::status`] で返す。
pub struct HttpTransport {
    pub base_url: String,
    pub timeout: Duration,
}

impl Transport for HttpTransport {
    fn send(&self, request: &ApiRequest<'_>) -> Result<ApiResponse, TransportError> {
        let http_request = ureq::http::Request::builder()
            .method("POST")
            .uri(format!("{}{API_PATH}", self.base_url.trim_end_matches('/')))
            .header("content-type", "application/json")
            .header("anthropic-version", API_VERSION)
            .header("x-api-key", request.api_key.expose())
            .body(request.body.to_string())
            .map_err(|_| failed(FailureKind::Failed, None))?;
        let agent = ureq::Agent::new_with_config(
            ureq::Agent::config_builder()
                .http_status_as_error(false)
                .timeout_global(Some(self.timeout))
                .build(),
        );
        let mut response = agent
            .run(http_request)
            .map_err(|error| failed(classify(&error), None))?;
        let request_id = response
            .headers()
            .get("request-id")
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned);
        let status = response.status().as_u16();
        let body = response
            .body_mut()
            .read_to_string()
            .map_err(|error| failed(classify(&error), request_id.clone()))?;
        Ok(ApiResponse {
            status,
            request_id,
            body,
        })
    }
}

fn failed(kind: FailureKind, request_id: Option<String>) -> TransportError {
    TransportError { kind, request_id }
}

fn classify(error: &ureq::Error) -> FailureKind {
    match error {
        ureq::Error::Timeout(_) => FailureKind::Timeout,
        _ => FailureKind::Failed,
    }
}
