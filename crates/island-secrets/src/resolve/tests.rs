//! 読む順序の契約（ADR-0014 の `resolve-*` scenario）を、偽の op client で確かめる。

use std::collections::HashMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use super::*;

/// 参照ごとに「n 回目の結果」を返す偽物。呼ばれた順と同時実行数を記録する。
#[derive(Default)]
struct FakeOp {
    script: HashMap<String, Vec<Result<&'static str, Failure>>>,
    calls: Mutex<Vec<String>>,
    in_flight: AtomicUsize,
    max_in_flight: AtomicUsize,
    delay: Duration,
}

impl FakeOp {
    fn new<R: AsRef<str>>(script: &[(R, Vec<Result<&'static str, Failure>>)]) -> Self {
        Self {
            script: script
                .iter()
                .map(|(reference, answers)| (reference.as_ref().to_owned(), answers.clone()))
                .collect(),
            ..Self::default()
        }
    }

    fn calls(&self) -> Vec<String> {
        self.calls.lock().unwrap().clone()
    }

    fn count(&self, reference: &str) -> usize {
        self.calls()
            .iter()
            .filter(|call| *call == reference)
            .count()
    }
}

impl OpClient for FakeOp {
    fn read(&self, reference: &str) -> Result<Secret, Failure> {
        let now = self.in_flight.fetch_add(1, Ordering::SeqCst) + 1;
        self.max_in_flight.fetch_max(now, Ordering::SeqCst);
        std::thread::sleep(self.delay);
        let attempt = {
            let mut calls = self.calls.lock().unwrap();
            calls.push(reference.to_owned());
            calls.iter().filter(|call| *call == reference).count() - 1
        };
        self.in_flight.fetch_sub(1, Ordering::SeqCst);
        let answers = &self.script[reference];
        answers[attempt.min(answers.len() - 1)].map(Secret::new)
    }

    fn vault_access(&self, _vault: &str) -> Result<(), Failure> {
        Ok(())
    }
}

fn no_sleep(waits: &Mutex<Vec<Duration>>) -> impl Fn(Duration) + '_ {
    move |wait| waits.lock().unwrap().push(wait)
}

fn run(op: &FakeOp, references: &[&str]) -> (Resolved, Vec<Duration>) {
    let waits = Mutex::new(Vec::new());
    let resolved = {
        let sleep = no_sleep(&waits);
        let options = Options {
            sleep: &sleep,
            ..Options::default()
        };
        resolve(op, references, &options)
    };
    (resolved, waits.into_inner().unwrap())
}

/// `resolve-signin-short-circuit`: 最初の 1 件が未認証なら残りを読まず、全件を未認証にする。
#[test]
fn a_first_signin_failure_stops_before_reading_the_rest() {
    let op = FakeOp::new(&[
        ("op://v/a/f", vec![Err(Failure::NotSignedIn)]),
        ("op://v/b/f", vec![Ok("b")]),
        ("op://v/c/f", vec![Ok("c")]),
    ]);

    let (resolved, waits) = run(&op, &["op://v/a/f", "op://v/b/f", "op://v/c/f"]);

    assert_eq!(op.calls(), ["op://v/a/f"]);
    assert!(resolved.values.is_empty());
    assert_eq!(resolved.failures.len(), 3);
    assert!(
        resolved
            .failures
            .values()
            .all(|f| *f == Failure::NotSignedIn)
    );
    assert!(waits.is_empty());
}

/// `op` が無いときも同じ。残りを読んでも同じ理由で落ちる。
#[test]
fn a_missing_op_command_stops_before_reading_the_rest() {
    let op = FakeOp::new(&[
        ("op://v/a/f", vec![Err(Failure::OpMissing)]),
        ("op://v/b/f", vec![Ok("b")]),
    ]);

    let (resolved, _) = run(&op, &["op://v/a/f", "op://v/b/f"]);

    assert_eq!(op.calls(), ["op://v/a/f"]);
    assert_eq!(resolved.failures["op://v/b/f"], Failure::OpMissing);
}

/// single-flight: 同じ参照を 3 つの key が指しても 1 回だけ読む。
#[test]
fn the_same_reference_is_read_once() {
    let op = FakeOp::new(&[
        ("op://v/shared/f", vec![Ok("s")]),
        ("op://v/other/f", vec![Ok("o")]),
    ]);

    let (resolved, _) = run(
        &op,
        &[
            "op://v/shared/f",
            "op://v/other/f",
            "op://v/shared/f",
            "op://v/shared/f",
        ],
    );

    assert_eq!(op.count("op://v/shared/f"), 1);
    assert_eq!(resolved.values.len(), 2);
}

/// `resolve-bounded-concurrency`: 同時に走る `op` は上限を超えない。
#[test]
fn parallel_reads_stay_within_the_limit() {
    let references: Vec<String> = (0..12).map(|i| format!("op://v/item{i}/f")).collect();
    let script: Vec<_> = references.iter().map(|r| (r, vec![Ok("x")])).collect();
    let op = FakeOp {
        delay: Duration::from_millis(20),
        ..FakeOp::new(&script)
    };
    let waits = Mutex::new(Vec::new());
    let sleep = no_sleep(&waits);
    let options = Options {
        concurrency: 3,
        sleep: &sleep,
        ..Options::default()
    };

    let borrowed: Vec<&str> = references.iter().map(String::as_str).collect();
    let resolved = resolve(&op, &borrowed, &options);

    assert_eq!(resolved.values.len(), 12);
    let max = op.max_in_flight.load(Ordering::SeqCst);
    assert!(max <= 3, "同時実行 {max}");
    assert!(max >= 2, "並列に読んでいない（同時実行 {max}）");
}

/// `resolve-transient-retry`: 一時失敗だけを待ってから読み直す。決定的な失敗は読み直さない
/// （spec「1 件だけ取得に失敗する」: 決定的な理由なら再試行しない）。
#[test]
fn only_transient_failures_are_retried_after_waiting() {
    let op = FakeOp::new(&[
        ("op://v/first/f", vec![Ok("1")]),
        (
            "op://v/flaky/f",
            vec![Err(Failure::DesktopConnect), Ok("recovered")],
        ),
        ("op://v/missing/f", vec![Err(Failure::NotFound)]),
        ("op://v/empty/f", vec![Err(Failure::Empty)]),
    ]);

    let (resolved, waits) = run(
        &op,
        &[
            "op://v/first/f",
            "op://v/flaky/f",
            "op://v/missing/f",
            "op://v/empty/f",
        ],
    );

    assert_eq!(op.count("op://v/flaky/f"), 2);
    assert_eq!(op.count("op://v/missing/f"), 1);
    assert_eq!(op.count("op://v/empty/f"), 1);
    assert_eq!(resolved.values["op://v/flaky/f"].expose(), "recovered");
    assert_eq!(resolved.failures["op://v/missing/f"], Failure::NotFound);
    assert_eq!(resolved.failures["op://v/empty/f"], Failure::Empty);
    assert_eq!(waits, [RETRY_BACKOFF[0]]);
}

/// 読み直しは待ち時間の列の回数まで。それでも落ちたら失敗のまま返す。
#[test]
fn retries_stop_after_the_backoff_list() {
    let op = FakeOp::new(&[
        ("op://v/first/f", vec![Ok("1")]),
        ("op://v/down/f", vec![Err(Failure::Failed)]),
    ]);

    let (resolved, waits) = run(&op, &["op://v/first/f", "op://v/down/f"]);

    assert_eq!(op.count("op://v/down/f"), 1 + RETRY_BACKOFF.len());
    assert_eq!(waits, RETRY_BACKOFF);
    assert_eq!(resolved.failures["op://v/down/f"], Failure::Failed);
}
