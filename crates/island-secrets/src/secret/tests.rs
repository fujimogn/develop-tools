//! **この型が守る約束のテスト。** コメントではなく機械で守れていることを確かめる。

use super::*;

/// **`{:?}` に中身が出ない。** `derive(Debug)` へ戻すとここが落ちる。
#[test]
fn the_debug_output_hides_the_value() {
    let secret = Secret::new("ops-token-do-not-print");

    assert_eq!(format!("{secret:?}"), "Secret(***)");
    assert!(!format!("{secret:?}").contains("ops-token"));
}

/// **`{}` にも出ない。** `println!("{token}")` と書いても伏せ字になる。
#[test]
fn the_display_output_hides_the_value() {
    let secret = Secret::new("ops-token-do-not-print");

    assert_eq!(format!("{secret}"), "***");
    assert!(!format!("{secret}").contains("ops-token"));
}

/// 秘密を含む構造体を `{:?}` で出しても漏れない。
/// **包む型を通す限り、うっかり出せない**のがこの型の狙い。
#[test]
fn a_struct_holding_a_secret_does_not_leak_it_either() {
    #[derive(Debug)]
    #[allow(dead_code)] // `{:?}` の出力だけに使う
    struct Credential {
        item: &'static str,
        token: Secret,
    }

    let rendered = format!(
        "{:?}",
        Credential {
            item: "Cloudflare",
            token: Secret::new("ops-token-do-not-print"),
        }
    );

    assert!(rendered.contains("Cloudflare"), "{rendered}");
    assert!(!rendered.contains("ops-token"), "{rendered}");
}

/// 値を取り出せるのは `expose` だけ。**この名前で grep すれば箇所を数えられる。**
#[test]
fn the_value_comes_out_only_through_expose() {
    let secret = Secret::new("value");

    assert_eq!(secret.expose(), "value");
    assert_eq!(secret.len(), 5);
    assert!(!secret.is_empty());
    assert!(Secret::new("").is_empty());
}

/// **`Secret` に `Debug` を derive しない。**
///
/// derive すると `{:?}` に中身が出る。手書きの `Debug` は伏せ字を返すので、
/// うっかり `#[derive(Debug)]` を足したときだけここが落ちる。
#[test]
fn the_secret_type_does_not_derive_debug() {
    let text = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/src/secret.rs"))
        .expect("secret.rs");
    let struct_index = text.find("pub struct Secret").expect("Secret の定義");
    let derive_line = text[..struct_index]
        .lines()
        .last()
        .unwrap_or_default()
        .to_string();

    assert!(
        !derive_line.contains("Debug"),
        "Secret に Debug を derive しない: {derive_line}"
    );
}
