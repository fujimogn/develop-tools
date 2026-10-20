//! spec「`$` を含む値」: 核が書いた `.env` を **Bun の dotenv（実物）** で読み戻すと元の値になる。
//!
//! Rust 側の parser は Bun の規則を写したもので、写し間違いは Rust の往復テストでは見つからない。
//! ここは実物の Bun に読ませる。`bun` は `bin/verify` の前提（CI も setup-bun 済み）なので、
//! 見つからなければ skip せずに落とす。

use std::collections::BTreeMap;
use std::process::Command;

use island_secrets::Secret;
use island_secrets::dotenv::{self, quote};

const TRICKY: [char; 12] = [
    '$', '\'', '"', '`', '\\', '\n', '#', ' ', '{', 'n', 'A', '=',
];

fn all_strings(len: u32) -> impl Iterator<Item = String> {
    let base = TRICKY.len();
    (0..base.pow(len)).map(move |mut n| {
        (0..len)
            .map(|_| {
                let c = TRICKY[n % base];
                n /= base;
                c
            })
            .collect()
    })
}

#[test]
fn bun_reads_back_every_value_the_core_writes() {
    let mut values: Vec<String> = [
        "$HOME",
        "p@ss$word$1",
        "${HOME}",
        "it's",
        r#"say "hi""#,
        "back`tick",
        r"C:\path\to",
        "line1\nline2",
        r"literal \n not newline",
        "  padded  ",
        "# not a comment",
        "-----BEGIN KEY-----\nabc$def\n-----END KEY-----\n",
        r"ends with two \\",
        r"dollar \$ escaped",
        "日本語の値",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect();
    values.extend((0..=3).flat_map(all_strings).filter(|v| quote(v).is_ok()));

    let pairs: Vec<(String, Secret)> = values
        .iter()
        .enumerate()
        .map(|(i, v)| (format!("K{i}"), Secret::new(v.as_str())))
        .collect();
    let dir = tempfile::tempdir().unwrap();
    let merged = dotenv::merge("", &pairs).unwrap();
    std::fs::write(dir.path().join(".env"), merged.expose()).unwrap();

    let output = Command::new("bun")
        .args([
            "-e",
            "const out = {}; for (const [k, v] of Object.entries(process.env)) \
             if (/^K\\d+$/.test(k)) out[k] = v; console.log(JSON.stringify(out))",
        ])
        .current_dir(dir.path())
        .env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap_or_default())
        .env("HOME", dir.path())
        .output()
        .expect("bun が見つかりません（bin/verify と同じく with-dev-env から実行してください）");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let read: BTreeMap<String, String> = serde_json::from_slice(&output.stdout).unwrap();

    let mismatches: Vec<String> = pairs
        .iter()
        .filter(|(key, value)| read.get(key).map(String::as_str) != Some(value.expose()))
        .map(|(key, value)| format!("{:?} -> {:?}", value.expose(), read.get(key)))
        .collect();
    assert!(
        mismatches.is_empty(),
        "{} / {} 件が崩れた: {:?}",
        mismatches.len(),
        pairs.len(),
        &mismatches[..mismatches.len().min(20)]
    );
}
