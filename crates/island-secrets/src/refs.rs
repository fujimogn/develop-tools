//! 入力: 「env の key → op 参照」の一覧（JSON object）。
//!
//! 何をどこへ入れるか（registry / template / multi-target / optional）は repo 側の adapter が
//! 決め、核はこの平らな一覧だけを受け取る。

/// 1 件分。参照は secret ではない（vault / item / field の名前）ので表示してよい。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub key: String,
    pub reference: String,
}

/// `{"KEY": "op://vault/item/field", ...}` を読む。順序は書かれた順のまま返す。
pub fn parse_json(text: &str) -> Result<Vec<Entry>, String> {
    let value: serde_json::Value =
        serde_json::from_str(text).map_err(|error| format!("JSON として読めません: {error}"))?;
    let object = value
        .as_object()
        .ok_or("最上位は {\"KEY\": \"op://…\"} の object にしてください")?;
    if object.is_empty() {
        return Err("参照が 1 件もありません".to_owned());
    }
    object
        .iter()
        .map(|(key, reference)| {
            if !is_env_key(key) {
                return Err(format!(
                    "{key:?} は env の key にできません（[A-Za-z_][A-Za-z0-9_]*）"
                ));
            }
            let reference = reference
                .as_str()
                .ok_or_else(|| format!("{key} の参照が文字列ではありません"))?;
            if vault_of(reference).is_none() {
                return Err(format!(
                    "{key} の参照は op://<vault>/<item>/<field> の形ではありません"
                ));
            }
            Ok(Entry {
                key: key.clone(),
                reference: reference.to_owned(),
            })
        })
        .collect()
}

pub fn is_env_key(key: &str) -> bool {
    let mut chars = key.chars();
    chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// `op://<vault>/<item>/[<section>/]<field>` の vault。形が違えば `None`。
pub fn vault_of(reference: &str) -> Option<&str> {
    let path = reference.strip_prefix("op://")?;
    let segments: Vec<&str> = path.split('/').collect();
    let well_formed = (3..=4).contains(&segments.len())
        && segments
            .iter()
            .all(|s| !s.is_empty() && !s.chars().any(char::is_whitespace));
    well_formed.then_some(segments[0])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_and_references_keep_their_written_order() {
        let entries =
            parse_json(r#"{"Z_KEY": "op://v/z/f", "A_KEY": "op://v/a/s/f"}"#).expect("parse");
        let keys: Vec<&str> = entries.iter().map(|e| e.key.as_str()).collect();
        assert_eq!(keys, ["Z_KEY", "A_KEY"]);
    }

    #[test]
    fn malformed_input_is_rejected_before_reading() {
        for text in [
            "[]",
            "{}",
            r#"{"1BAD": "op://v/i/f"}"#,
            r#"{"KEY": 1}"#,
            r#"{"KEY": "op://v/i"}"#,
            r#"{"KEY": "https://v/i/f"}"#,
            r#"{"KEY": "op://v//f"}"#,
        ] {
            assert!(parse_json(text).is_err(), "{text}");
        }
    }

    #[test]
    fn the_vault_is_the_first_segment() {
        assert_eq!(
            vault_of("op://example-vault/Cloudflare/token"),
            Some("example-vault")
        );
        assert_eq!(vault_of("op://v/item/section/field"), Some("v"));
        assert_eq!(vault_of("op://v/item"), None);
    }
}
