//! 名前を**独立した語**として含むかの判定。
//!
//! 島の検査（`tools/develop-health`）と AI 出力の照合（`develop-release`）が同じ規則で内部名を探す。

/// ASCII 英数字・`-`・`_` だけを語の続きとみなし、前後が語の字でないときだけ一致とする。
pub fn contains_bounded_name(text: &str, name: &str, ignore_ascii_case: bool) -> bool {
    if name.is_empty() {
        return false;
    }
    let bytes = text.as_bytes();
    bytes
        .windows(name.len())
        .enumerate()
        .any(|(start, candidate)| {
            let matches = if ignore_ascii_case {
                candidate.eq_ignore_ascii_case(name.as_bytes())
            } else {
                candidate == name.as_bytes()
            };
            matches
                && (start == 0 || !is_name_char(bytes[start - 1]))
                && (start + name.len() == bytes.len() || !is_name_char(bytes[start + name.len()]))
        })
}

fn is_name_char(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_'
}

#[cfg(test)]
mod tests {
    use super::contains_bounded_name;

    /// AI 出力の照合が `ignore_ascii_case = true` で頼る判定の表。
    const BOUNDARY_CASES: &[(&str, &str, bool)] = &[
        ("secret-repo を直した", "secret-repo", true),
        ("Secret-Repo を直した", "secret-repo", true),
        ("secret-repository を直した", "secret-repo", false),
        ("my-secret-repo を直した", "secret-repo", false),
        ("secret-repo_x を直した", "secret-repo", false),
        ("secret-repo.md を直した", "secret-repo", true),
        ("「secret-repo」を直した", "secret-repo", true),
        ("先頭secret-repo末尾", "secret-repo", true),
        ("secret-repo", "secret-repo", true),
        ("secret", "secret-repo", false),
        ("何でも", "", false),
    ];

    #[test]
    fn the_word_boundary_rule_matches_the_table() {
        for (text, name, expected) in BOUNDARY_CASES {
            assert_eq!(
                contains_bounded_name(text, name, true),
                *expected,
                "{text} / {name}"
            );
        }
    }

    #[test]
    fn case_is_kept_unless_asked_to_ignore_it() {
        assert!(!contains_bounded_name(
            "Secret-Repo を直した",
            "secret-repo",
            false
        ));
        assert!(contains_bounded_name(
            "secret-repo を直した",
            "secret-repo",
            false
        ));
    }
}
