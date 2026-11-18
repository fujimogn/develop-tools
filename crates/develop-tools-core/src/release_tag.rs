//! 宣言した Release tag の規則と、厳密な3整数の版。

/// `x.y.z` の 3 つ組。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Version {
    pub major: u64,
    pub minor: u64,
    pub patch: u64,
}

impl std::fmt::Display for Version {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

impl std::str::FromStr for Version {
    type Err = String;

    /// **前ゼロ・数字以外（`+1` のような符号を含む）・余分な節を弾く。** `0.1.00` や `0.1.2.3` を
    /// 通すと、書き戻した文字列と tag 名が食い違って guard がすり抜ける。
    fn from_str(raw: &str) -> Result<Self, Self::Err> {
        let parts: Vec<&str> = raw.split('.').collect();
        if parts.len() != 3 {
            return Err(format!("semver ではありません: {raw}"));
        }
        let mut nums = [0u64; 3];
        for (slot, part) in nums.iter_mut().zip(parts) {
            if part.is_empty()
                || !part.bytes().all(|byte| byte.is_ascii_digit())
                || (part.len() > 1 && part.starts_with('0'))
            {
                return Err(format!("semver ではありません: {raw}"));
            }
            *slot = part
                .parse()
                .map_err(|_| format!("semver ではありません: {raw}"))?;
        }
        Ok(Version {
            major: nums[0],
            minor: nums[1],
            patch: nums[2],
        })
    }
}

/// 検証済みの tag prefix。省略時の歴史互換だけをこの型で既定化する。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TagPrefix(String);

impl Default for TagPrefix {
    fn default() -> Self {
        Self("v".to_owned())
    }
}

impl TagPrefix {
    pub fn new(prefix: &str) -> Result<Self, String> {
        let bytes = prefix.as_bytes();
        if !bytes.first().is_some_and(u8::is_ascii_alphabetic)
            || !bytes
                .last()
                .is_some_and(|byte| byte.is_ascii_alphabetic() || matches!(byte, b'_' | b'-'))
            || !bytes
                .iter()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
            || prefix.contains("..")
        {
            return Err(format!("不正な release.tagPrefix: {prefix:?}"));
        }
        Ok(Self(prefix.to_owned()))
    }

    pub fn format(&self, version: Version) -> String {
        format!("{}{version}", self.0)
    }

    pub fn parse(&self, tag: &str) -> Option<Version> {
        tag.strip_prefix(&self.0)?.parse().ok()
    }
}
