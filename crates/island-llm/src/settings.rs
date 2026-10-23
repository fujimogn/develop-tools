//! 利用者の環境側の非公開の設定（`~/.config/develop-tools/llm.json`）。
//!
//! 鍵の参照・禁止語・内部名は crate にも repo の宣言にも置かない。

use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::KeyRef;

/// repo の宣言の `audience`。設定が無いときの理由 code が変わる（design §2.1・§5 の 3）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Audience {
    Private,
    /// 辞書と内部名が揃わなければ推論しない。
    Public,
}

#[derive(Debug, PartialEq, Eq)]
pub enum SettingsError {
    Missing,
    Invalid,
    KeyRefMissing,
}

impl SettingsError {
    /// file が無い環境では、public は辞書も無いので `dictionary_missing` にする。
    pub fn code(&self, audience: Audience) -> &'static str {
        match (self, audience) {
            (Self::Missing, Audience::Public) => "dictionary_missing",
            (Self::Missing | Self::KeyRefMissing, _) => "key_missing",
            (Self::Invalid, _) => "config_invalid",
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub struct DictionaryMissing;

#[derive(Debug, PartialEq, Eq)]
pub struct Dictionary {
    pub forbidden_terms: Vec<String>,
    pub internal_names: Vec<String>,
}

#[derive(Deserialize)]
struct Raw {
    anthropic_api_key_ref: Option<String>,
    forbidden_terms: Option<Vec<String>>,
    internal_names: Option<Vec<String>>,
    usage_ledger: Option<PathBuf>,
}

#[derive(Debug)]
pub struct Settings {
    key_ref: KeyRef,
    forbidden_terms: Option<Vec<String>>,
    internal_names: Option<Vec<String>>,
    usage_ledger: Option<PathBuf>,
}

impl Settings {
    pub fn key_ref(&self) -> &KeyRef {
        &self.key_ref
    }

    /// usage 台帳の置き場。crate は既定値を持たず、無ければ呼び出し側が送らずに止める。
    pub fn usage_ledger(&self) -> Option<&Path> {
        self.usage_ledger.as_deref()
    }

    /// 禁止語だけ。内部名が無くても private の照合は禁止語で続ける。
    pub fn forbidden_terms(&self) -> Option<&[String]> {
        self.forbidden_terms.as_deref()
    }

    /// 禁止語と内部名の両方が揃うときだけ返す。公開 repo で推論を止める判定に使う。
    pub fn dictionary(&self) -> Result<Dictionary, DictionaryMissing> {
        match (&self.forbidden_terms, &self.internal_names) {
            (Some(forbidden_terms), Some(internal_names)) => Ok(Dictionary {
                forbidden_terms: forbidden_terms.clone(),
                internal_names: internal_names.clone(),
            }),
            _ => Err(DictionaryMissing),
        }
    }
}

pub fn default_path(home: &Path) -> PathBuf {
    home.join(".config/develop-tools/llm.json")
}

/// 絶対 path でない `usage_ledger` は、読む側の cwd で置き場が変わるので不正とする。
fn read_raw(path: &Path) -> Result<Raw, SettingsError> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Err(SettingsError::Missing);
        }
        Err(_) => return Err(SettingsError::Invalid),
    };
    let raw = serde_json::from_str::<Raw>(&text).map_err(|_| SettingsError::Invalid)?;
    if raw
        .usage_ledger
        .as_ref()
        .is_some_and(|path| !path.is_absolute())
    {
        return Err(SettingsError::Invalid);
    }
    Ok(raw)
}

pub fn load(path: &Path) -> Result<Settings, SettingsError> {
    let raw = read_raw(path)?;
    let key_ref = raw
        .anthropic_api_key_ref
        .and_then(KeyRef::new)
        .ok_or(SettingsError::KeyRefMissing)?;
    Ok(Settings {
        key_ref,
        forbidden_terms: raw.forbidden_terms,
        internal_names: raw.internal_names,
        usage_ledger: raw.usage_ledger,
    })
}

/// usage 台帳の欄だけを読む。鍵の参照を別の所から読む呼び出し側のため、鍵の欄は問わない。
pub fn usage_ledger(path: &Path) -> Result<Option<PathBuf>, SettingsError> {
    Ok(read_raw(path)?.usage_ledger)
}

/// `release check` が報告する、設定の各欄の有無。値は持たない。
#[derive(Debug, PartialEq, Eq)]
pub struct Presence {
    pub key_ref: bool,
    pub forbidden_terms: bool,
    pub internal_names: bool,
    pub usage_ledger: bool,
}

/// [`load`] と同じ読み方で各欄の有無だけを返す。鍵の参照が無くても他の欄を数える。
pub fn presence(path: &Path) -> Result<Presence, SettingsError> {
    let raw = read_raw(path)?;
    Ok(Presence {
        key_ref: raw.anthropic_api_key_ref.and_then(KeyRef::new).is_some(),
        forbidden_terms: raw.forbidden_terms.is_some(),
        internal_names: raw.internal_names.is_some(),
        usage_ledger: raw.usage_ledger.is_some(),
    })
}
