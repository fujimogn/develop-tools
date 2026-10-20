//! 参照の一覧を読む順序: 最初の 1 件を直列 → 残りを並列 → 一時失敗だけを間隔を空けて直列で読み直す。
//!
//! - **single-flight**: 同じ参照を複数の key が指しても `op` は 1 回だけ起動する。
//! - **最初の 1 件で打ち切る**: 未認証・`op` なしなら残りを読まない（並列に N 個の承認
//!   dialog や同じ失敗を N 回出さない）。
//! - **並列度の上限**: 1Password デスクトップアプリへの接続が同時に集中すると単発で落ちる
//!   （dev 機の実測: 6 並列 40 件のうち 4〜5 件）。上限を置いたうえで、落ちた分を読み直す。
//! - **読み直し**: 一時失敗（[`Failure::is_transient`]）だけを、待ち時間の列の回数だけ直列で読む。
//!   1 件の失敗が書き込み中止に直結するので、単発の詰まりで止めない。

use std::collections::BTreeMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;
use std::time::Duration;

use crate::op::{Failure, OpClient};
use crate::secret::Secret;

/// 並列度の既定。子 repo の旧実装（`DEFAULT_OP_READ_CONCURRENCY`）と同じ値。
pub const DEFAULT_CONCURRENCY: usize = 6;
/// 読み直しの前の待ち時間。要素数が読み直しの最大回数（ADR-0014 v3 の「最大 2 回」）。
pub const RETRY_BACKOFF: [Duration; 2] = [Duration::from_millis(500), Duration::from_millis(1500)];

pub struct Options<'a> {
    pub concurrency: usize,
    pub backoff: &'a [Duration],
    pub sleep: &'a dyn Fn(Duration),
}

impl Default for Options<'_> {
    fn default() -> Self {
        Self {
            concurrency: DEFAULT_CONCURRENCY,
            backoff: &RETRY_BACKOFF,
            sleep: &thread::sleep,
        }
    }
}

/// 参照ごとの結果。**値は [`Secret`] のまま**持つ。
#[derive(Debug, Default)]
pub struct Resolved {
    pub values: BTreeMap<String, Secret>,
    pub failures: BTreeMap<String, Failure>,
}

/// `references` を読む。重複は 1 回にまとめる（入力の順は最初の出現で決める）。
pub fn resolve(client: &dyn OpClient, references: &[&str], options: &Options<'_>) -> Resolved {
    let mut unique: Vec<&str> = Vec::new();
    for reference in references {
        if !unique.contains(reference) {
            unique.push(reference);
        }
    }
    let mut resolved = Resolved::default();
    let Some((first, rest)) = unique.split_first() else {
        return resolved;
    };

    let first_result = client.read(first);
    let abort = first_result
        .as_ref()
        .err()
        .copied()
        .filter(|f| f.aborts_all());
    record(&mut resolved, first, first_result);
    if let Some(failure) = abort {
        for reference in rest {
            resolved.failures.insert((*reference).to_owned(), failure);
        }
        return resolved;
    }

    for (reference, result) in rest
        .iter()
        .zip(read_parallel(client, rest, options.concurrency))
    {
        record(&mut resolved, reference, result);
    }

    for wait in options.backoff {
        if resolved.failures.values().any(|f| f.aborts_all()) {
            break;
        }
        let targets: Vec<String> = resolved
            .failures
            .iter()
            .filter(|(_, failure)| failure.is_transient())
            .map(|(reference, _)| reference.clone())
            .collect();
        if targets.is_empty() {
            break;
        }
        (options.sleep)(*wait);
        for reference in targets {
            let result = client.read(&reference);
            resolved.failures.remove(&reference);
            let stop = result.as_ref().err().is_some_and(|f| f.aborts_all());
            record(&mut resolved, &reference, result);
            if stop {
                break;
            }
        }
    }
    resolved
}

fn record(resolved: &mut Resolved, reference: &str, result: Result<Secret, Failure>) {
    match result {
        Ok(value) => {
            resolved.values.insert(reference.to_owned(), value);
        }
        Err(failure) => {
            resolved.failures.insert(reference.to_owned(), failure);
        }
    }
}

/// 上限 `concurrency` 本の worker で読む。結果は `references` と同じ順で返す。
fn read_parallel(
    client: &dyn OpClient,
    references: &[&str],
    concurrency: usize,
) -> Vec<Result<Secret, Failure>> {
    let results: Mutex<Vec<Option<Result<Secret, Failure>>>> =
        Mutex::new((0..references.len()).map(|_| None).collect());
    let next = AtomicUsize::new(0);
    let workers = concurrency.clamp(1, references.len().max(1));
    thread::scope(|scope| {
        for _ in 0..workers {
            scope.spawn(|| {
                loop {
                    let index = next.fetch_add(1, Ordering::Relaxed);
                    let Some(reference) = references.get(index) else {
                        break;
                    };
                    let result = client.read(reference);
                    results.lock().expect("結果の lock")[index] = Some(result);
                }
            });
        }
    });
    results
        .into_inner()
        .expect("結果の lock")
        .into_iter()
        .map(|result| result.unwrap_or(Err(Failure::Failed)))
        .collect()
}

#[cfg(test)]
mod tests;
