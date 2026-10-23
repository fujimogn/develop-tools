//! 核の `run` を fixture の transport と偽の `op` で回す。本物の API は呼ばない。

use std::cell::{Cell, RefCell};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use island_llm::settings::{self, Audience, SettingsError};
use island_llm::transport::{ApiRequest, ApiResponse, FailureKind, Transport, TransportError};
use island_llm::{Failure, KeyRef, Output, Runtime, run};
use serde_json::Value;
use tempfile::TempDir;

const KEY_REF: &str = "op://test-vault/test-item/api-key";

/// 呼ばれた回数と最後の body を数え、決めた応答を返す。
struct FixtureTransport {
    reply: Result<ApiResponse, TransportError>,
    calls: Cell<usize>,
    last_body: RefCell<Option<Value>>,
}

impl FixtureTransport {
    fn text(text: &str) -> Self {
        let body = serde_json::json!({
            "content": [{"type": "text", "text": text}],
            "usage": {"input_tokens": 10, "output_tokens": 3},
        });
        Self::with(Ok(ApiResponse {
            status: 200,
            request_id: Some("req_fixture".to_owned()),
            body: body.to_string(),
        }))
    }

    fn with(reply: Result<ApiResponse, TransportError>) -> Self {
        Self {
            reply,
            calls: Cell::new(0),
            last_body: RefCell::new(None),
        }
    }
}

impl Transport for FixtureTransport {
    fn send(&self, request: &ApiRequest<'_>) -> Result<ApiResponse, TransportError> {
        self.calls.set(self.calls.get() + 1);
        assert_eq!(request.api_key.expose(), "test-api-key");
        *self.last_body.borrow_mut() = Some(request.body.clone());
        self.reply.clone()
    }
}

/// 偽の `op` は test binary で 1 回だけ書く。test ごとに書くと、別の test の fork が書き込み中の
/// fd を持ったまま exec する競合（ETXTBSY）で `op` の起動が落ちる。呼ばれた印は、子に渡る
/// service account の token（= 各 test の一時 directory）の下に置く。
fn fake_op() -> &'static Path {
    static FAKE_OP: OnceLock<TempDir> = OnceLock::new();
    FAKE_OP
        .get_or_init(|| {
            let dir = tempfile::tempdir().expect("fake op directory");
            let path = dir.path().join("fake-op");
            std::fs::write(
                &path,
                format!(
                    "#!/bin/sh\n: > \"$OP_SERVICE_ACCOUNT_TOKEN/op-called\"\n[ \"$1\" = read ] && [ \"$2\" = '{KEY_REF}' ] || exit 7\nprintf 'test-api-key'\n"
                ),
            )
            .expect("fake op");
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))
                .expect("make fake op executable");
            dir
        })
        .path()
}

struct Setup {
    temp: TempDir,
}

impl Setup {
    fn new() -> Self {
        let temp = tempfile::tempdir().expect("temporary directory");
        std::fs::create_dir_all(temp.path().join("home")).expect("home");
        Self { temp }
    }

    fn runtime<'a>(&self, transport: &'a dyn Transport) -> Runtime<'a> {
        Runtime {
            home: self.temp.path().join("home"),
            op_program: fake_op().join("fake-op"),
            op_service_account_token: Some(self.temp.path().display().to_string()),
            ledger: self.ledger(),
            transport,
        }
    }

    fn ledger(&self) -> PathBuf {
        self.temp.path().join("state/usage.jsonl")
    }

    fn op_was_called(&self) -> bool {
        self.temp.path().join("op-called").exists()
    }

    fn settings_path(&self) -> PathBuf {
        self.temp.path().join("llm.json")
    }
}

fn key_ref() -> KeyRef {
    KeyRef::new(KEY_REF).expect("non-empty key ref")
}

fn ledger_rows(path: &Path) -> Vec<Value> {
    std::fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .map(|line| serde_json::from_str(line).expect("usage row JSON"))
        .collect()
}

#[test]
fn an_unknown_purpose_is_rejected_before_op_and_the_api() {
    let setup = Setup::new();
    let transport = FixtureTransport::text("要約");

    let failure = run(
        "no-such-purpose",
        b"text",
        &key_ref(),
        &setup.runtime(&transport),
    )
    .expect_err("unknown purpose");

    assert_eq!(failure, Failure::UnknownPurpose);
    assert_eq!(failure.code(), "unknown_purpose");
    assert_eq!(transport.calls.get(), 0);
    assert!(!setup.op_was_called());
}

#[test]
fn input_one_byte_over_the_limit_is_rejected_before_the_api() {
    let setup = Setup::new();
    let transport = FixtureTransport::text("要約");
    let limit = island_llm::purpose::find("digest")
        .expect("digest purpose")
        .max_input_bytes;

    let failure = run(
        "digest",
        &vec![b'a'; limit + 1],
        &key_ref(),
        &setup.runtime(&transport),
    )
    .expect_err("over the limit");

    assert_eq!(failure.code(), "input_rejected");
    assert_eq!(transport.calls.get(), 0);
    assert!(!setup.op_was_called());

    // 上限ちょうどは通る。
    run(
        "digest",
        &vec![b'a'; limit],
        &key_ref(),
        &setup.runtime(&transport),
    )
    .expect("at the limit");
    assert_eq!(transport.calls.get(), 1);
}

#[test]
fn a_line_with_a_line_break_is_rejected_after_one_usage_row() {
    let setup = Setup::new();
    let transport = FixtureTransport::text("一行目\n二行目");

    let failure =
        run("digest", b"text", &key_ref(), &setup.runtime(&transport)).expect_err("line break");

    assert_eq!(failure.code(), "response_rejected");
    assert_eq!(transport.calls.get(), 1);
    assert_eq!(ledger_rows(&setup.ledger()).len(), 1);
}

#[test]
fn json_output_must_be_exactly_one_json_value() {
    for bad in ["not json", r#"{"summary":"a"} {"summary":"b"}"#, ""] {
        let setup = Setup::new();
        let transport = FixtureTransport::text(bad);

        let failure = run(
            "release-notes",
            b"1. text",
            &key_ref(),
            &setup.runtime(&transport),
        )
        .expect_err("bad JSON");

        assert_eq!(failure.code(), "response_rejected", "{bad:?}");
        assert_eq!(ledger_rows(&setup.ledger()).len(), 1, "{bad:?}");
    }

    let setup = Setup::new();
    let transport = FixtureTransport::text(r#"{"summary":"s","items":[]}"#);
    let output = run(
        "release-notes",
        b"1. text",
        &key_ref(),
        &setup.runtime(&transport),
    )
    .expect("one JSON value");
    assert_eq!(
        output,
        Output::Json(serde_json::json!({"summary": "s", "items": []}))
    );
}

#[test]
fn missing_settings_or_key_ref_is_key_missing() {
    let setup = Setup::new();

    let absent = settings::load(&setup.settings_path()).expect_err("no file");
    assert_eq!(absent, SettingsError::Missing);
    assert_eq!(absent.code(Audience::Private), "key_missing");
    // 設定が無い環境では public は辞書も無い（design §2.1）。
    assert_eq!(absent.code(Audience::Public), "dictionary_missing");

    std::fs::write(setup.settings_path(), r#"{"forbidden_terms":["x"]}"#).unwrap();
    let no_key = settings::load(&setup.settings_path()).expect_err("no key ref");
    assert_eq!(no_key, SettingsError::KeyRefMissing);
    assert_eq!(no_key.code(Audience::Private), "key_missing");
    assert_eq!(no_key.code(Audience::Public), "key_missing");

    std::fs::write(setup.settings_path(), r#"{"anthropic_api_key_ref":"  "}"#).unwrap();
    assert_eq!(
        settings::load(&setup.settings_path()).expect_err("blank key ref"),
        SettingsError::KeyRefMissing
    );

    std::fs::write(setup.settings_path(), "{").unwrap();
    let invalid = settings::load(&setup.settings_path()).expect_err("broken JSON");
    assert_eq!(invalid, SettingsError::Invalid);
    assert_eq!(invalid.code(Audience::Public), "config_invalid");

    assert!(!setup.op_was_called());
}

#[test]
fn the_dictionary_is_present_only_when_both_lists_are() {
    let setup = Setup::new();
    std::fs::write(
        setup.settings_path(),
        format!(r#"{{"anthropic_api_key_ref":"{KEY_REF}","forbidden_terms":["段取り"]}}"#),
    )
    .unwrap();
    let loaded = settings::load(&setup.settings_path()).expect("settings");
    assert_eq!(loaded.key_ref(), &key_ref());
    assert!(loaded.dictionary().is_err());

    std::fs::write(
        setup.settings_path(),
        format!(
            r#"{{"anthropic_api_key_ref":"{KEY_REF}","forbidden_terms":["段取り"],"internal_names":["example-org"]}}"#
        ),
    )
    .unwrap();
    let loaded = settings::load(&setup.settings_path()).expect("settings");
    let dictionary = loaded.dictionary().expect("both lists");
    assert_eq!(dictionary.forbidden_terms, ["段取り"]);
    assert_eq!(dictionary.internal_names, ["example-org"]);
}

/// crate は台帳の既定値を持たない。欄が無ければ `None` で、呼び出し側が送らずに止める。
#[test]
fn the_usage_ledger_comes_only_from_an_absolute_settings_field() {
    let setup = Setup::new();
    let path = setup.settings_path();
    std::fs::write(&path, format!(r#"{{"anthropic_api_key_ref":"{KEY_REF}"}}"#)).unwrap();
    assert_eq!(
        settings::load(&path).expect("settings").usage_ledger(),
        None
    );
    assert_eq!(settings::usage_ledger(&path), Ok(None));

    // 鍵の参照を別の所から読む呼び出し側のため、台帳の欄だけでも読める。
    std::fs::write(&path, r#"{"usage_ledger":"/state/usage.jsonl"}"#).unwrap();
    assert_eq!(
        settings::usage_ledger(&path),
        Ok(Some(PathBuf::from("/state/usage.jsonl")))
    );
    assert!(settings::presence(&path).expect("presence").usage_ledger);

    std::fs::write(
        &path,
        format!(r#"{{"anthropic_api_key_ref":"{KEY_REF}","usage_ledger":"/state/usage.jsonl"}}"#),
    )
    .unwrap();
    assert_eq!(
        settings::load(&path).expect("settings").usage_ledger(),
        Some(Path::new("/state/usage.jsonl"))
    );

    // 相対 path は読む側の cwd で置き場が変わるので不正。
    std::fs::write(
        &path,
        format!(r#"{{"anthropic_api_key_ref":"{KEY_REF}","usage_ledger":"state/usage.jsonl"}}"#),
    )
    .unwrap();
    assert_eq!(
        settings::load(&path).expect_err("relative"),
        SettingsError::Invalid
    );
    assert_eq!(settings::usage_ledger(&path), Err(SettingsError::Invalid));
}

#[test]
fn a_missing_service_account_token_is_key_missing_without_calling_op() {
    let setup = Setup::new();
    let transport = FixtureTransport::text("要約");
    let mut runtime = setup.runtime(&transport);
    runtime.op_service_account_token = None;

    let failure = run("digest", b"text", &key_ref(), &runtime).expect_err("no token");

    assert_eq!(failure.code(), "key_missing");
    assert!(!setup.op_was_called());
    assert_eq!(transport.calls.get(), 0);
}

#[test]
fn a_successful_line_sends_the_table_row_once_and_records_usage() {
    let setup = Setup::new();
    let transport = FixtureTransport::text("短い要約");

    let output = run(
        "digest",
        "本文".as_bytes(),
        &key_ref(),
        &setup.runtime(&transport),
    )
    .expect("success");

    assert_eq!(output, Output::Line("短い要約".to_owned()));
    assert_eq!(transport.calls.get(), 1);
    let purpose = island_llm::purpose::find("digest").unwrap();
    let body = transport.last_body.borrow().clone().unwrap();
    assert_eq!(body["model"], purpose.model);
    assert_eq!(body["max_tokens"], purpose.max_tokens);
    assert_eq!(
        body["messages"][0]["content"],
        format!("{}本文", purpose.instruction)
    );
    let rows = ledger_rows(&setup.ledger());
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["purpose"], "digest");
    assert_eq!(rows[0]["request_id"], "req_fixture");
    assert_eq!(rows[0]["usage"]["output_tokens"], 3);
    assert!(
        rows[0]["timestamp"]
            .as_str()
            .is_some_and(|t| t.ends_with('Z'))
    );
}

#[test]
fn transport_failures_map_to_their_codes_and_are_not_retried() {
    let cases = [
        (
            Err(TransportError {
                kind: FailureKind::Timeout,
                request_id: None,
            }),
            "api_timeout",
        ),
        (
            Err(TransportError {
                kind: FailureKind::Failed,
                request_id: Some("req_cut".to_owned()),
            }),
            "api_failed",
        ),
        (
            Ok(ApiResponse {
                status: 529,
                request_id: None,
                body: r#"{"type":"error"}"#.to_owned(),
            }),
            "api_failed",
        ),
    ];
    for (reply, code) in cases {
        let setup = Setup::new();
        let transport = FixtureTransport::with(reply);

        let failure = run("digest", b"text", &key_ref(), &setup.runtime(&transport))
            .expect_err("transport failure");

        assert_eq!(failure.code(), code);
        assert_eq!(transport.calls.get(), 1, "{code}: one attempt only");
        // 送った後の失敗も台帳に 1 行残す。
        assert_eq!(ledger_rows(&setup.ledger()).len(), 1, "{code}");
    }
}

#[test]
fn an_estimate_over_the_cost_cap_is_not_sent() {
    // 表の各行で、入力の上限と max_tokens を使い切っても上限の内に収まることを固定する。
    for purpose in island_llm::purpose::PURPOSES {
        let estimate = purpose.estimate_micro_usd(purpose.max_input_bytes);
        assert!(
            estimate <= purpose.max_cost_micro_usd,
            "{}: {estimate} > {}",
            purpose.name,
            purpose.max_cost_micro_usd
        );
    }
    let purpose = island_llm::purpose::find("digest").unwrap();
    let over = island_llm::purpose::Purpose {
        max_cost_micro_usd: purpose.estimate_micro_usd(4) - 1,
        ..*purpose
    };
    assert_eq!(
        island_llm::check_budget(&over, 4)
            .expect_err("over the cap")
            .code(),
        "budget_exceeded"
    );
    island_llm::check_budget(purpose, 4).expect("under the cap");
}
