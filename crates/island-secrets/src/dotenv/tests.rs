//! quote・parser・merge の契約。Bun の実物との往復は `tests/bun_dotenv.rs` が見る。

use super::*;

/// 往復を崩しやすい文字。旧実装の食い違い（調査記録 §3.3-2・§3.3-6）はどれもこの組み合わせで出る。
const TRICKY: [char; 12] = [
    '$', '\'', '"', '`', '\\', '\n', '#', ' ', '{', 'n', 'A', '=',
];

/// 長さ `len` の全組み合わせ（12^len 通り）。
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

fn round_trip(value: &str) -> Option<String> {
    let line = format!("KEY={}", quote(value).ok()?);
    let entries = parse(&line);
    assert_eq!(entries.len(), 1, "{line:?}");
    assert_eq!(entries[0].lines, 0..line.split('\n').count(), "{line:?}");
    Some(entries[0].value.clone())
}

/// 性質: quote できた値は、読み戻すと元の値に戻る（長さ 0〜4 の全組み合わせ、2 万通り強）。
#[test]
fn every_quoted_value_reads_back_unchanged() {
    let mut refused = 0;
    for len in 0..=4 {
        for value in all_strings(len) {
            match round_trip(&value) {
                Some(read) => assert_eq!(read, value, "{:?}", quote(&value)),
                None => refused += 1,
            }
        }
    }
    // 断る値は「末尾 backslash が奇数」と「3 種の quote が揃う」ものだけで、一部に留まる。
    assert!(refused < 4_000, "断った値が多すぎる: {refused}");
}

/// 旧実装で崩れた代表値（§3.3-2 の `$`、§3.3-6 の escape の戻し）。
#[test]
fn representative_values_survive() {
    for value in [
        "$HOME",
        "p@ss$word",
        "${NOT_A_VAR}",
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
    ] {
        assert_eq!(round_trip(value).as_deref(), Some(value), "{value:?}");
    }
}

#[test]
fn values_that_cannot_round_trip_are_refused() {
    assert_eq!(quote("a\rb"), Err(Unrepresentable::ControlCharacter));
    assert_eq!(quote("nul\0"), Err(Unrepresentable::ControlCharacter));
    assert_eq!(quote(r"odd\"), Err(Unrepresentable::TrailingBackslash));
    assert_eq!(
        quote(r#"all ' ` " three"#),
        Err(Unrepresentable::NoUsableQuote)
    );
    assert_eq!(quote(r"' ` \"), Err(Unrepresentable::TrailingBackslash));
    assert_eq!(quote(r"' ` \ x"), Err(Unrepresentable::NoUsableQuote));
}

/// 他人が書いた行も key と行範囲を読める（`KEY =`、`export`、複数行、コメント）。
#[test]
fn foreign_lines_are_read_with_their_spans() {
    let content = "# comment\nexport A=1\nB = 'two'\nC=\"x\ny\"\nnot a line\nD=plain # tail\n";
    let entries = parse(content);
    let summary: Vec<(&str, &str, std::ops::Range<usize>)> = entries
        .iter()
        .map(|e| (e.key.as_str(), e.value.as_str(), e.lines.clone()))
        .collect();
    assert_eq!(
        summary,
        [
            ("A", "1", 1..2),
            ("B", "two", 2..3),
            ("C", "x\ny", 3..5),
            ("D", "plain", 6..7),
        ]
    );
}

fn secrets(pairs: &[(&str, &str)]) -> Vec<(String, Secret)> {
    pairs
        .iter()
        .map(|(k, v)| ((*k).to_owned(), Secret::new(*v)))
        .collect()
}

/// `merge-preserves-unmanaged`: 管理外の行は残し、管理対象だけを置き換え、無い key は見出しの後へ足す。
#[test]
fn merge_keeps_unmanaged_lines_and_appends_new_keys() {
    let existing = "# local memo\nLOCAL=keep\nTOKEN=old\n\n";

    let merged = merge(existing, &secrets(&[("TOKEN", "new"), ("API_KEY", "k")])).unwrap();

    assert_eq!(
        merged.expose(),
        format!("# local memo\nLOCAL=keep\nTOKEN='new'\n\n{MANAGED_HEADER}\nAPI_KEY='k'\n")
    );
}

/// `KEY =` を key と読まないと、同じ key が 2 行になる。重複は 1 行へ畳む。
#[test]
fn merge_replaces_spaced_and_duplicate_keys_once() {
    let existing = "TOKEN = 'old'\nOTHER=1\nexport TOKEN=older\n";

    let merged = merge(existing, &secrets(&[("TOKEN", "new")])).unwrap();

    assert_eq!(merged.expose(), "TOKEN='new'\nOTHER=1\n");
}

/// 既存の複数行の値は、続きの行ごと置き換える（行単位の置換だと続きの行が残る）。
#[test]
fn merge_replaces_every_line_of_a_multiline_value() {
    let existing = "PEM='-----BEGIN-----\nold\n-----END-----'\nAFTER=1\n";

    let merged = merge(existing, &secrets(&[("PEM", "fresh")])).unwrap();

    assert_eq!(merged.expose(), "PEM='fresh'\nAFTER=1\n");
}

#[test]
fn merge_into_nothing_writes_only_the_managed_block() {
    let merged = merge("", &secrets(&[("B", "2"), ("A", "1")])).unwrap();
    assert_eq!(
        merged.expose(),
        "# --- 1Password から secrets:inject で生成 (宣言対象の key は次回 inject で更新) ---\nA='1'\nB='2'\n"
    );
}

/// 表せない値が 1 件でもあれば merge しない。理由には key だけが載り、値は載らない。
#[test]
fn merge_refuses_without_leaking_the_value() {
    let error = merge("", &secrets(&[("OK", "fine"), ("BAD", "sentinel-value\r")])).unwrap_err();
    assert_eq!(error.key, "BAD");
    assert!(!error.to_string().contains("sentinel"), "{error}");
}
