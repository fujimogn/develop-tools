//! ADR の正本形式の判定。
//!
//! **消費者が 2 つある判定だけを置く。**
//!   - `develop-sync` の `migrate-legacy-adrs` — 移行後の archive が正しいか
//!   - `develop-health` の `check-project-integrity` — 既存 ADR の archive が正しいか
//!
//! 「共有 lib crate は重複が 3 回出るまで立てない」に対して 2 回目だが、例外として立てた。
//! ルールの目的は**共有面が分からないうちに抽象化するのを防ぐ**ことで、ここは
//! 共有面が 2 関数に確定しており推測の余地が無い。一方で重複した場合の代償が大きい —
//! SHA-256 と区切りの意味論がわずかにずれると、**片方が通す archive をもう片方が弾く**。
//! しかも症状は特定の archive を検査したときにしか出ず、両側のテストが緑のまま食い違う。
//!
//! **入れないもの**: template の plan / apply（消費者は `sync-adr-template` だけ）、
//! ADR の構造・採番・索引の検査（消費者は `check-project-integrity` だけ）。
//! 判断基準は消費者の数で、1 消費者ならその crate に置く。

use sha2::{Digest, Sha256};

/// 移行済み ADR の Original Record が、記録された SHA-256 と一致するか。
pub fn validate_archive(content: &str) -> bool {
    validate_archive_with_marker(content, "Legacy source SHA-256", "Original Record")
}

/// 公開用に sanitize された ADR の archive が、公開側 SHA-256 と一致するか。
///
/// private / public の 2 つの hash を持ち、**照合するのは public 側**。
/// private hash は原文の同一性を後から示すための記録で、公開物には原文が無い。
/// 移設時に 3 点壊れていたので直した（移設前の TS は全部通していた）。
///   1. **marker がファイル先頭にある前提**だった。実 ADR は frontmatter と本文が先で、
///      marker は末尾近くに来る → 実ファイルを 1 件も検証できない
///   2. **`-->` の後ろの改行が 1 つ**という前提だった。実際は空行 1 つ（改行 2 つ）
///   3. **CRLF を扱えなかった**。TS 側は `\r?\n` で明示的にテストしている
pub fn validate_sanitized_archive(content: &str) -> bool {
    let private_marker = "<!-- Private legacy source SHA-256: ";
    let Some(start) = content.find(private_marker) else {
        return false;
    };
    let rest = &content[start + private_marker.len()..];

    // **本文は正規化しない。** digest は CRLF のまま採られているので、
    // 改行を寄せると正しい archive が不一致になる。構造だけを緩く読む。
    let Some((private_hash, rest)) =
        split_once_flexible(rest, " -->\n<!-- Public sanitized record SHA-256: ")
    else {
        return false;
    };
    let Some((public_hash, body)) = rest.split_once(" -->") else {
        return false;
    };
    if !is_sha256(private_hash) || !is_sha256(public_hash) {
        return false;
    }
    let Some(record) = strip_prefix_flexible(body, "\n\n## Sanitized Original Record\n\n") else {
        return false;
    };
    archived_hash(record, public_hash)
}

fn is_sha256(value: &str) -> bool {
    value.len() == 64 && value.chars().all(|c| c.is_ascii_hexdigit())
}

/// `\n` を `\r?\n` として扱う `strip_prefix`。本文は書き換えない。
fn strip_prefix_flexible<'a>(text: &'a str, pattern: &str) -> Option<&'a str> {
    let mut rest = text;
    for (index, part) in pattern.split('\n').enumerate() {
        if index > 0 {
            rest = rest
                .strip_prefix("\r\n")
                .or_else(|| rest.strip_prefix('\n'))?;
        }
        rest = rest.strip_prefix(part)?;
    }
    Some(rest)
}

/// `\n` を `\r?\n` として扱う `split_once`。
fn split_once_flexible<'a>(text: &'a str, pattern: &str) -> Option<(&'a str, &'a str)> {
    let head = pattern.split('\n').next()?;
    let mut from = 0;
    while let Some(index) = text[from..].find(head) {
        let at = from + index;
        if let Some(rest) = strip_prefix_flexible(&text[at..], pattern) {
            return Some((&text[..at], rest));
        }
        from = at + head.len();
    }
    None
}

fn validate_archive_with_marker(content: &str, marker: &str, heading: &str) -> bool {
    let needle = format!("<!-- {marker}: ");
    let Some(start) = content.find(&needle) else {
        return false;
    };
    let Some(hash_end) = content[start + needle.len()..].find(" -->") else {
        return false;
    };
    let hash = &content[start + needle.len()..start + needle.len() + hash_end];
    if !is_sha256(hash) {
        return false;
    }
    let after = &content[start + needle.len() + hash_end + 4..];
    let Some(record) = record_after_heading(after, heading) else {
        return false;
    };
    archived_hash(record, hash)
}

/// marker の後ろから見出しを探し、その次の空行の後ろ（＝ fence 行の先頭）を返す。
///
/// **marker と見出しが隣接しているとは限らない。** 実 ADR は marker のあとに
/// `## Follow-up（…）` のような節を挟むことがある（移設時に隣接前提で書いていて、
/// 実在の ADR が `invalid-legacy-archive` になった。2026-08-27）。
/// TS 側は `[\s\S]*?^## <heading>\s*$` で「間に何が入ってもよい」としている。
fn record_after_heading<'a>(after: &'a str, heading: &str) -> Option<&'a str> {
    let target = format!("## {heading}");
    let mut offset = 0;

    while offset < after.len() {
        let rest = &after[offset..];
        let index = rest.find(&target)?;
        let at = offset + index;

        // 行頭であること。見出し文字列が本文中に現れただけのものを拾わない。
        let at_line_start = at == 0 || after[..at].ends_with('\n');
        if at_line_start {
            let tail = &after[at + target.len()..];
            // 見出し行の末尾に空白があってもよい（TS の `\s*$`）。
            let tail = tail.trim_start_matches([' ', '\t']);
            // 見出し行の改行 + 空行。
            if let Some(body) = strip_prefix_flexible(tail, "\n\n") {
                return Some(body);
            }
        }
        offset = at + target.len();
    }
    None
}

/// code fence で囲まれた原文の SHA-256 を照合する。
///
/// **末尾改行の有無を両方試す**のが要点。移行時の書き出しで末尾改行が付くかは
/// 経路によって変わり、片方だけを見ると正しい archive を不正と判定する。
fn archived_hash(record: &str, expected: &str) -> bool {
    let Some(first_end) = record.find('\n') else {
        return false;
    };
    // fence 行は CRLF を含み得る。**判定は行から `\r` を除いて**行い、本文は触らない。
    let first = record[..first_end].trim_end_matches('\r');
    let Some(fence) = first.strip_suffix("markdown") else {
        return false;
    };
    if fence.len() < 4 || !fence.chars().all(|c| c == '~' || c == '`') {
        return false;
    }
    let body = &record[first_end + 1..];

    // 閉じ fence は `\r?\n` の後ろ。CRLF なら `\r` は archived に含めない。
    let (close_start, close_len) = match (
        body.rfind(&format!("\r\n{fence}")),
        body.rfind(&format!("\n{fence}")),
    ) {
        (Some(crlf), _) => (crlf, fence.len() + 2),
        (None, Some(lf)) => (lf, fence.len() + 1),
        (None, None) => return false,
    };
    if !body[close_start + close_len..].trim().is_empty() {
        return false;
    }
    let archived = &body[..close_start];

    // **候補は「そのまま」と「末尾に改行を足したもの」。**
    //
    // `close_start` は閉じ fence 直前の改行の位置なので、`archived` は既に末尾改行を
    // 落としている。ここからさらに `strip_suffix('\n')` を試すと候補が
    // 「改行 0 個」と「改行 -1 個」になり、**正解の「改行 1 個」が入らない**。
    // 移設時にそうなっていて、実 ADR 166 件が全部 `invalid-legacy-archive` になった
    // （合成データのテストでは通っていた。2026-08-27 に実データ照合で発覚）。
    //
    // 元の改行様式で足す。CRLF の archive は CRLF のまま digest が採られている。
    let newline = if archived.contains("\r\n") {
        "\r\n"
    } else {
        "\n"
    };
    hex_digest(archived) == expected || hex_digest(&format!("{archived}{newline}")) == expected
}

fn hex_digest(content: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(content.as_bytes());
    // sha2 0.11 の `finalize()` は `LowerHex` を持たない `Array` を返す。
    hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn digest(text: &str) -> String {
        hex_digest(text)
    }

    fn archive(body: &str) -> String {
        format!(
            "<!-- Legacy source SHA-256: {} -->\n\n## Original Record\n\n~~~~markdown\n{body}\n~~~~\n",
            digest(body)
        )
    }

    #[test]
    fn a_matching_archive_is_valid() {
        assert!(validate_archive(&archive("# 旧 ADR\n\n本文\n")));
    }

    #[test]
    fn a_tampered_body_is_rejected() {
        let mut content = archive("# 旧 ADR\n");
        content = content.replace("# 旧 ADR", "# 改ざん");
        assert!(!validate_archive(&content), "本文を変えたら不一致になる");
    }

    #[test]
    fn a_missing_marker_is_rejected() {
        assert!(!validate_archive(
            "## Original Record\n\n~~~~markdown\nx\n~~~~\n"
        ));
    }

    #[test]
    fn a_short_hash_is_rejected() {
        let content =
            "<!-- Legacy source SHA-256: abc -->\n\n## Original Record\n\n~~~~markdown\nx\n~~~~\n";
        assert!(!validate_archive(content));
    }

    /// 末尾改行の有無どちらでも通る。移行経路によって付いたり付かなかったりする。
    #[test]
    fn both_trailing_newline_forms_are_accepted() {
        // (a) hash が archived そのもので採られている場合。
        let without = format!(
            "<!-- Legacy source SHA-256: {} -->\n\n## Original Record\n\n~~~~markdown\nx\n~~~~\n",
            digest("x")
        );
        assert!(validate_archive(&without), "末尾改行なしの hash");

        // (b) hash が **末尾改行つき**で採られている場合。実 ADR 169 件はこちら。
        let with = format!(
            "<!-- Legacy source SHA-256: {} -->\n\n## Original Record\n\n~~~~markdown\nx\n~~~~\n",
            digest("x\n")
        );
        assert!(validate_archive(&with), "末尾改行ありの hash");
    }

    /// **marker と `## Original Record` は隣接しているとは限らない。**
    /// 実 ADR は移行後に `## Follow-up（…）` のような節を marker の直後へ足すことがある
    /// （実在の ADR がこの形で `invalid-legacy-archive` になった）。
    #[test]
    fn a_section_between_the_marker_and_the_heading_is_allowed() {
        let body = "# 旧記録\n\n本文。\n";
        let hash = hex_digest(body);
        let content = format!(
            "# ADR\n\n<!-- Legacy source SHA-256: {hash} -->\n\n## Follow-up（2026-07-22）\n\n後日談。\n\n## Original Record\n\n~~~~markdown\n{body}~~~~\n"
        );
        assert!(validate_archive(&content), "間に節がある形を拒否している");
    }

    /// 移行の道具の sanitized テストと同じ組み立て。
    #[test]
    fn a_sanitized_archive_in_a_real_adr_document_is_valid() {
        let public_record = "# Sanitized decision\n\nPublic facts only.";
        let content = format!(
            "---\nstatus: accepted\ndate: 2026-06-01\n---\n\n# ADR-0001: decision\n\n<!-- Private legacy source SHA-256: {} -->\n<!-- Public sanitized record SHA-256: {} -->\n\n## Sanitized Original Record\n\n~~~~markdown\n{public_record}\n~~~~\n",
            "a".repeat(64),
            digest(public_record)
        );
        assert!(
            validate_sanitized_archive(&content),
            "marker はファイル先頭ではなく本文の後ろに来る"
        );
    }

    /// CRLF の sanitized archive も公開本文 digest を照合する。
    #[test]
    fn a_crlf_sanitized_archive_is_valid() {
        let public_record = "# Sanitized decision\r\n\r\nPublic facts only.";
        let content = format!(
            "---\r\nstatus: accepted\r\ndate: 2026-06-01\r\n---\r\n\r\n# ADR-0001: decision\r\n\r\n<!-- Private legacy source SHA-256: {} -->\r\n<!-- Public sanitized record SHA-256: {} -->\r\n\r\n## Sanitized Original Record\r\n\r\n~~~~markdown\r\n{public_record}\r\n~~~~\r\n",
            "a".repeat(64),
            digest(public_record)
        );
        assert!(validate_sanitized_archive(&content), "CRLF でも通る");
    }

    /// fence は 4 文字以上。原文が 3 連バッククォートを含んでも壊れないため。
    #[test]
    fn a_three_character_fence_is_rejected() {
        let body = "x";
        let content = format!(
            "<!-- Legacy source SHA-256: {} -->\n\n## Original Record\n\n```markdown\n{body}\n```\n",
            digest(body)
        );
        assert!(!validate_archive(&content));
    }

    #[test]
    fn trailing_content_after_the_fence_is_rejected() {
        let body = "x";
        let content = format!(
            "<!-- Legacy source SHA-256: {} -->\n\n## Original Record\n\n~~~~markdown\n{body}\n~~~~\n余計な行\n",
            digest(body)
        );
        assert!(
            !validate_archive(&content),
            "archive の後ろに本文が続いていたら不正"
        );
    }

    #[test]
    fn a_sanitized_archive_with_a_wrong_public_hash_is_rejected() {
        let body = "# 公開用\n";
        let content = format!(
            "<!-- Private legacy source SHA-256: {} -->\n<!-- Public sanitized record SHA-256: {} -->\n\n## Sanitized Original Record\n\n~~~~markdown\n{body}\n~~~~\n",
            "0".repeat(64),
            "1".repeat(64)
        );
        assert!(!validate_sanitized_archive(&content));
    }

    /// sanitized は専用 marker を要求する。通常の archive 形式では通らない。
    #[test]
    fn the_two_archive_forms_are_not_interchangeable() {
        let normal = archive("x");
        assert!(!validate_sanitized_archive(&normal));
    }
}
