//! The repository-local tool pin has one schema and two distribution forms.
use serde_json::{Value, json};

pub const PATH: &str = ".develop-tools.json";
pub const RESOLVER_PATH: &str = ".develop-tools/resolve";
pub const LINUX_TARGET: &str = "x86_64-unknown-linux-gnu";
pub const MACOS_TARGET: &str = "aarch64-apple-darwin";
/// ADR 番号の契約の欄。値の正本は配る側の台帳で、配布がこの名前のまま写す。
pub const ADR_CONTRACT_KEYS: [&str; 3] = [
    "adrMigratedThrough",
    "adrReservedNumbers",
    "adrSanitizedNumbers",
];

pub enum Pin<'a> {
    Legacy {
        version: &'a str,
        linux: &'a str,
    },
    Resolver {
        version: &'a str,
        linux: &'a str,
        macos: &'a str,
    },
    KeepResolver,
    /// 道具を配る側の repo。版の固定を持たない宣言のまま書き換える。
    Unpinned,
}

/// 島の語彙の OS。GitHub の `runner.os` や Release asset の target triple とは混ぜない。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Platform {
    Linux,
    Macos,
    Windows,
}

impl Platform {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Linux => "linux",
            Self::Macos => "macos",
            Self::Windows => "windows",
        }
    }

    fn parse(name: &str) -> Option<Self> {
        [Self::Linux, Self::Macos, Self::Windows]
            .into_iter()
            .find(|platform| platform.as_str() == name)
    }
}

/// staged の ADR 検査が照合する番号の契約。欄の無い配列は空として読む。
#[derive(Debug, Default, PartialEq, Eq)]
pub struct AdrContract {
    pub migrated_through: Option<u32>,
    pub reserved_numbers: Vec<u32>,
    pub sanitized_numbers: Vec<u32>,
}

#[derive(Debug, PartialEq, Eq)]
pub struct PlatformCheck {
    pub cargo_workspace: String,
    pub bun: bool,
    pub setup: Option<String>,
}

/// 軽い CI（`ci-develop.yml` の標準の区画）の宣言。repo の中身の事実なので repo が書き、
/// develop-sync は読むだけで書き換えない。
#[derive(Debug, PartialEq, Eq)]
pub struct CiDeclaration {
    pub rust: Option<RustCi>,
    pub bun: Option<BunCi>,
}

#[derive(Debug, PartialEq, Eq)]
pub struct RustCi {
    pub workspaces: Vec<String>,
    /// 区画の Rust job の最後に `run:` へそのまま置く 1 行の command。
    pub extra_steps: Vec<String>,
}

#[derive(Debug, PartialEq, Eq)]
pub struct BunCi {
    pub dirs: Vec<String>,
}

/// repo が `.develop-tools.json` で宣言する値。版の固定（`version`・`sha256`）は
/// [`known_shape`] が見るので、ここでは読まない。道具を配る側の repo の形でも読める。
#[derive(Debug)]
pub struct Declaration {
    pub integration_branch: Option<String>,
    /// 宣言が無ければ空。既定の OS を補わない。
    pub platforms: Vec<Platform>,
    pub platform_check: PlatformCheck,
    pub preflight_workflow: Option<String>,
    pub release: Option<Value>,
    /// 契約の欄が 1 つも無ければ `None`。
    pub adr_contract: Option<AdrContract>,
    /// `ci` の欄が無ければ `None`。
    pub ci: Option<CiDeclaration>,
}

fn optional_string(
    object: &serde_json::Map<String, Value>,
    key: &str,
) -> Result<Option<String>, String> {
    match object.get(key) {
        None => Ok(None),
        Some(Value::String(text)) if !text.is_empty() => Ok(Some(text.clone())),
        Some(_) => Err(format!("不正な {key}")),
    }
}

fn adr_number(value: &Value, key: &str) -> Result<u32, String> {
    value
        .as_u64()
        .and_then(|number| u32::try_from(number).ok())
        .ok_or_else(|| format!("不正な {key}"))
}

fn adr_numbers(object: &serde_json::Map<String, Value>, key: &str) -> Result<Vec<u32>, String> {
    match object.get(key) {
        None => Ok(Vec::new()),
        Some(Value::Array(items)) => items.iter().map(|item| adr_number(item, key)).collect(),
        Some(_) => Err(format!("不正な {key}")),
    }
}

/// `value` の object から ADR 番号の契約を読む。欄が 1 つも無ければ `None`。
pub fn adr_contract(value: &Value) -> Result<Option<AdrContract>, String> {
    let object = value.as_object().ok_or("JSON object が必要です")?;
    if !ADR_CONTRACT_KEYS
        .iter()
        .any(|key| object.contains_key(*key))
    {
        return Ok(None);
    }
    let [migrated, reserved, sanitized] = ADR_CONTRACT_KEYS;
    Ok(Some(AdrContract {
        migrated_through: object
            .get(migrated)
            .map(|value| adr_number(value, migrated))
            .transpose()?,
        reserved_numbers: adr_numbers(object, reserved)?,
        sanitized_numbers: adr_numbers(object, sanitized)?,
    }))
}

/// repo 直下からの相対 path を `/` 区切りで書く。共通 workflow の plan も同じ規則で読むので、
/// OS で意味が変わる `\` と `:`（Windows の drive）、外へ出る `..`、書き方の揺れ（`./`・`//`・末尾の `/`）を受けない。
/// 改行などの制御文字は、workflow の YAML の引用の中で空白へ折り畳まれて別の path になるので受けない。
fn relative_workspace(path: &str) -> bool {
    path == "."
        || (!path.contains(['\\', ':'])
            && !path.chars().any(char::is_control)
            && path
                .split('/')
                .all(|segment| !matches!(segment, "" | "." | "..")))
}

/// `value` の object から軽い CI の宣言を読む。欄が無ければ `None`。
pub fn ci_declaration(value: &Value) -> Result<Option<CiDeclaration>, String> {
    let object = value.as_object().ok_or("JSON object が必要です")?;
    let Some(ci) = object.get("ci") else {
        return Ok(None);
    };
    let ci = stack(ci, "ci", &["rust", "bun"])?;
    if ci.is_empty() {
        return Err("ci は rust か bun の片方か両方を持たなければなりません".into());
    }
    let rust = match ci.get("rust") {
        None => None,
        Some(rust) => {
            let rust = stack(rust, "ci.rust", &["workspaces", "extraSteps"])?;
            Some(RustCi {
                workspaces: directories(rust, "ci.rust", "workspaces")?,
                extra_steps: extra_steps(rust)?,
            })
        }
    };
    let bun = match ci.get("bun") {
        None => None,
        Some(bun) => Some(BunCi {
            dirs: directories(stack(bun, "ci.bun", &["dirs"])?, "ci.bun", "dirs")?,
        }),
    };
    Ok(Some(CiDeclaration { rust, bun }))
}

/// 宣言の object を読み、`keys` の外の欄を名前付きで拒む。
fn stack<'a>(
    value: &'a Value,
    name: &str,
    keys: &[&str],
) -> Result<&'a serde_json::Map<String, Value>, String> {
    let object = value
        .as_object()
        .ok_or_else(|| format!("{name} は object でなければなりません"))?;
    match object.keys().find(|key| !keys.contains(&key.as_str())) {
        Some(key) => Err(format!("{name}.{key} は未知の欄です")),
        None => Ok(object),
    }
}

/// repo 直下からの相対 directory を重複なしの空でない配列で読む。
fn directories(
    object: &serde_json::Map<String, Value>,
    parent: &str,
    key: &str,
) -> Result<Vec<String>, String> {
    let name = format!("{parent}.{key}");
    let paths = object
        .get(key)
        .and_then(Value::as_array)
        .filter(|paths| !paths.is_empty())
        .ok_or_else(|| format!("{name} は空でない配列でなければなりません"))?;
    let mut directories: Vec<String> = Vec::new();
    for path in paths {
        let path = path
            .as_str()
            .filter(|path| relative_workspace(path))
            .ok_or_else(|| format!("{name} に不正な path があります: {path}"))?;
        if directories.iter().any(|known| known == path) {
            return Err(format!("{name} が重複しています: {path}"));
        }
        directories.push(path.to_owned());
    }
    Ok(directories)
}

/// `run:` にそのまま置くので、式（`${{`）と改行などの制御文字を受けない。水平タブは 1 行のままなので受ける。
fn extra_steps(rust: &serde_json::Map<String, Value>) -> Result<Vec<String>, String> {
    let steps = match rust.get("extraSteps") {
        None => return Ok(Vec::new()),
        Some(Value::Array(steps)) => steps,
        Some(_) => return Err("ci.rust.extraSteps は配列でなければなりません".into()),
    };
    steps
        .iter()
        .enumerate()
        .map(|(index, step)| match step.as_str() {
            Some(command)
                if !command.is_empty()
                    && !command.contains("${{")
                    && !command
                        .chars()
                        .any(|char| char.is_control() && char != '\t') =>
            {
                Ok(command.to_owned())
            }
            _ => Err(format!(
                "ci.rust.extraSteps[{index}] は式（${{{{）と改行を含まない 1 行の command でなければなりません: {step}"
            )),
        })
        .collect()
}

pub fn declaration(value: &Value) -> Result<Declaration, String> {
    let object = value.as_object().ok_or("JSON object が必要です")?;
    let integration_branch = optional_string(object, "integrationBranch")?;
    let adr_contract = adr_contract(value)?;
    let ci = ci_declaration(value)?;
    let release = match object.get("release") {
        None => None,
        Some(release) if release.is_object() => Some(release.clone()),
        Some(_) => return Err("不正な release".into()),
    };
    let preflight_workflow = match &release {
        Some(release) => optional_string(release.as_object().expect("object"), "preflightWorkflow")
            .map_err(|_| "不正な release.preflightWorkflow".to_owned())?,
        None => None,
    };
    let mut platforms = Vec::new();
    if let Some(declared) = object.get("platforms") {
        let names = declared
            .as_array()
            .filter(|names| !names.is_empty())
            .ok_or("platforms は空でない配列でなければなりません")?;
        for name in names {
            let platform = name
                .as_str()
                .and_then(Platform::parse)
                .ok_or_else(|| format!("platforms に未知の OS があります: {name}"))?;
            if platforms.contains(&platform) {
                return Err(format!("platforms が重複しています: {name}"));
            }
            platforms.push(platform);
        }
        if preflight_workflow.is_none() {
            return Err("platforms を宣言するときは release.preflightWorkflow も必要です".into());
        }
    }
    let platform_check = match object.get("platformCheck") {
        None => None,
        Some(Value::Object(check)) => Some(check),
        Some(_) => return Err("不正な platformCheck".into()),
    };
    let field = |key: &str| platform_check.and_then(|check| check.get(key));
    let platform_check = PlatformCheck {
        cargo_workspace: match field("cargoWorkspace") {
            None => ".".into(),
            Some(Value::String(path)) if relative_workspace(path) => path.clone(),
            Some(_) => return Err("不正な platformCheck.cargoWorkspace".into()),
        },
        bun: match field("bun") {
            None => false,
            Some(Value::Bool(bun)) => *bun,
            Some(_) => return Err("不正な platformCheck.bun".into()),
        },
        setup: match field("setup") {
            None => None,
            Some(Value::String(command)) if !command.is_empty() => Some(command.clone()),
            Some(_) => return Err("不正な platformCheck.setup".into()),
        },
    };
    Ok(Declaration {
        integration_branch,
        platforms,
        platform_check,
        preflight_workflow,
        release,
        adr_contract,
        ci,
    })
}

pub fn hash_valid(hash: &str) -> bool {
    hash.len() == 64
        && hash
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn known_keys(value: &Value) -> bool {
    let Some(object) = value.as_object() else {
        return false;
    };
    !(object.keys().any(|key| {
        !matches!(
            key.as_str(),
            "version"
                | "sha256"
                | "integrationBranch"
                | "release"
                | "platforms"
                | "platformCheck"
                | "adrMigratedThrough"
                | "adrReservedNumbers"
                | "adrSanitizedNumbers"
                | "ci"
        )
    }) || adr_contract(value).is_err()
        || ci_declaration(value).is_err()
        || object
            .get("integrationBranch")
            .is_some_and(|branch| !branch.is_string())
        || object
            .get("release")
            .is_some_and(|release| !release.is_object()))
}

/// 版の固定を持たない宣言（道具を配る側の repo）の形。
pub fn known_unpinned_shape(value: &Value) -> bool {
    known_keys(value)
        && value.get("version").is_none()
        && value.get("sha256").is_none()
        && declaration(value).is_ok()
}

pub fn known_shape(value: &Value) -> bool {
    if !known_keys(value) {
        return false;
    }
    let object = value.as_object().expect("known keys");
    let Some(version) = object.get("version").and_then(Value::as_str) else {
        return false;
    };
    if version.split('.').count() != 3
        || !version
            .split('.')
            .all(|part| !part.is_empty() && part.parse::<u64>().is_ok())
    {
        return false;
    }
    let Some(hashes) = object.get("sha256").and_then(Value::as_object) else {
        return false;
    };
    (hashes.len() == 1 || hashes.len() == 2)
        && hashes.iter().all(|(target, hash)| {
            (target == LINUX_TARGET || target == MACOS_TARGET)
                && hash.as_str().is_some_and(hash_valid)
        })
        && hashes.contains_key(LINUX_TARGET)
        && (hashes.len() == 1 || hashes.contains_key(MACOS_TARGET))
}

/// 版の固定と `integrationBranch` を書き換える。ADR 番号の契約は今の値のまま残す。
pub fn rewrite(
    current: Option<&Value>,
    pin: Pin<'_>,
    branch: Option<&str>,
) -> Result<Vec<u8>, String> {
    rewrite_value(current, pin, branch).and_then(|value| to_bytes(&value))
}

/// [`rewrite`] に加えて、ADR 番号の契約を台帳の値に揃える。`None` なら契約の欄を消す。
pub fn rewrite_with_adr_contract(
    current: Option<&Value>,
    pin: Pin<'_>,
    branch: Option<&str>,
    contract: Option<&AdrContract>,
) -> Result<Vec<u8>, String> {
    let mut value = rewrite_value(current, pin, branch)?;
    let object = value.as_object_mut().ok_or("manifest_invalid")?;
    let [migrated, reserved, sanitized] = ADR_CONTRACT_KEYS;
    // 既にある欄は位置を保ったまま値だけを替え、消すときも他の欄の並びを動かさない。
    // 並びが変わると、揃っている宣言まで書き換えの対象に見える。
    let values = [
        (
            migrated,
            contract.and_then(|contract| contract.migrated_through.map(|number| json!(number))),
        ),
        (
            reserved,
            contract.map(|contract| json!(contract.reserved_numbers)),
        ),
        (
            sanitized,
            contract.map(|contract| json!(contract.sanitized_numbers)),
        ),
    ];
    for (key, value) in values {
        match value {
            Some(value) => {
                object.insert(key.into(), value);
            }
            None => {
                object.shift_remove(key);
            }
        }
    }
    to_bytes(&value)
}

fn to_bytes(value: &Value) -> Result<Vec<u8>, String> {
    Ok(format!(
        "{}\n",
        serde_json::to_string_pretty(value).map_err(|e| e.to_string())?
    )
    .into_bytes())
}

fn rewrite_value(
    current: Option<&Value>,
    pin: Pin<'_>,
    branch: Option<&str>,
) -> Result<Value, String> {
    let known = match pin {
        Pin::Unpinned => current.is_some_and(known_unpinned_shape),
        _ => current.is_none_or(known_shape),
    };
    if !known {
        return Err("manifest_invalid".into());
    }
    let mut value = current.cloned().unwrap_or_else(|| json!({}));
    let object = value.as_object_mut().ok_or("manifest_invalid")?;
    match pin {
        Pin::Legacy { version, linux } => {
            if !hash_valid(linux) {
                return Err("manifest_invalid".into());
            }
            object.insert("version".into(), json!(version));
            object.insert("sha256".into(), json!({LINUX_TARGET: linux}));
        }
        Pin::Resolver {
            version,
            linux,
            macos,
        } => {
            if !hash_valid(linux) || !hash_valid(macos) {
                return Err("manifest_invalid".into());
            }
            object.insert("version".into(), json!(version));
            object.insert(
                "sha256".into(),
                json!({LINUX_TARGET: linux, MACOS_TARGET: macos}),
            );
        }
        Pin::KeepResolver => {
            if current.is_none_or(|value| {
                value["sha256"]
                    .as_object()
                    .is_none_or(|hashes| hashes.len() != 2)
            }) {
                return Err("resolver_manifest_missing".into());
            }
        }
        Pin::Unpinned => {}
    }
    match branch {
        Some(branch) => {
            object.insert("integrationBranch".into(), json!(branch));
        }
        None => {
            object.remove("integrationBranch");
        }
    }
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    // OS 宣言の fixture の正本は `packages/action/tests/fixtures/platform-declarations.json`。
    // この crate は公開する workspace の外を読まないので、使う case だけを同じ名前で写す。
    // 正本の全 case に対する `declaration()` の判定は `tools/develop-core/tests/platform_declaration_parity.rs`
    // が照合する。
    fn manifest(name: &str) -> Value {
        match name {
            "valid-linux-macos" => json!({
                "integrationBranch": "develop",
                "platforms": ["linux", "macos"],
                "platformCheck": {
                    "cargoWorkspace": "tools",
                    "bun": true,
                    "setup": "./bin/ci-install --frozen-lockfile"
                },
                "release": {
                    "preflightWorkflow": "ci.yml",
                    "versionFiles": [{"path": "tools/Cargo.toml", "kind": "cargo-workspace"}]
                }
            }),
            "invalid-unknown-os" => json!({
                "platforms": ["linux", "freebsd"],
                "release": {"preflightWorkflow": "ci.yml"}
            }),
            "invalid-missing-preflight" => json!({
                "platforms": ["linux", "macos"],
                "release": {
                    "versionFiles": [{"path": "tools/Cargo.toml", "kind": "cargo-workspace"}]
                }
            }),
            _ => panic!("{name} is not copied from the shared fixture"),
        }
    }

    #[test]
    fn manifest_without_pin_reads_platform_declaration() {
        let declared = declaration(&manifest("valid-linux-macos")).unwrap();
        assert_eq!(declared.platforms, [Platform::Linux, Platform::Macos]);
        assert_eq!(
            declared.platform_check,
            PlatformCheck {
                cargo_workspace: "tools".into(),
                bun: true,
                setup: Some("./bin/ci-install --frozen-lockfile".into()),
            }
        );
        assert_eq!(declared.preflight_workflow.as_deref(), Some("ci.yml"));
        assert_eq!(declared.integration_branch.as_deref(), Some("develop"));
        assert!(
            declared
                .release
                .is_some_and(|release| release["versionFiles"].is_array())
        );
    }

    #[test]
    fn absent_declaration_has_no_platforms_and_default_check() {
        let declared = declaration(&json!({"version": "0.1.0"})).unwrap();
        assert!(declared.platforms.is_empty());
        assert_eq!(
            declared.platform_check,
            PlatformCheck {
                cargo_workspace: ".".into(),
                bun: false,
                setup: None,
            }
        );
        assert_eq!(declared.preflight_workflow, None);
        assert_eq!(declared.integration_branch, None);
    }

    #[test]
    fn invalid_platform_declarations_are_refused() {
        assert!(declaration(&manifest("invalid-unknown-os")).is_err());
        assert!(declaration(&manifest("invalid-missing-preflight")).is_err());
        let release = json!({"preflightWorkflow": "ci.yml"});
        for platforms in [
            json!([]),
            json!(["linux", "linux"]),
            json!(["Linux"]),
            json!("linux"),
        ] {
            assert!(
                declaration(&json!({"platforms": platforms, "release": release})).is_err(),
                "{platforms}"
            );
        }
        assert!(declaration(&json!({"platformCheck": {"bun": "yes"}})).is_err());
        assert!(declaration(&json!({"integrationBranch": 1})).is_err());
        assert!(declaration(&json!(null)).is_err());
    }

    #[test]
    fn rewrite_keeps_platform_declaration() {
        let mut current = manifest("valid-linux-macos");
        current["version"] = json!("0.1.0");
        current["sha256"] = json!({LINUX_TARGET: "a".repeat(64)});
        assert!(known_shape(&current));
        let hash = "b".repeat(64);
        let bytes = rewrite(
            Some(&current),
            Pin::Legacy {
                version: "0.1.1",
                linux: &hash,
            },
            Some("develop"),
        )
        .unwrap();
        let next: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(next["version"], "0.1.1");
        assert_eq!(next["platforms"], current["platforms"]);
        assert_eq!(next["platformCheck"], current["platformCheck"]);
        assert_eq!(next["release"], current["release"]);
    }

    #[test]
    fn adr_contract_is_read_only_when_a_contract_key_is_declared() {
        assert_eq!(
            adr_contract(&json!({"integrationBranch": "develop"})).unwrap(),
            None
        );
        assert_eq!(
            adr_contract(&json!({"adrMigratedThrough": 21, "adrReservedNumbers": [20]})).unwrap(),
            Some(AdrContract {
                migrated_through: Some(21),
                reserved_numbers: vec![20],
                sanitized_numbers: vec![],
            })
        );
        let declared = declaration(&json!({"adrSanitizedNumbers": [1, 4]})).unwrap();
        assert_eq!(
            declared.adr_contract,
            Some(AdrContract {
                migrated_through: None,
                reserved_numbers: vec![],
                sanitized_numbers: vec![1, 4],
            })
        );
        for invalid in [
            json!({"adrMigratedThrough": -1}),
            json!({"adrMigratedThrough": "21"}),
            json!({"adrMigratedThrough": 4_294_967_296_u64}),
            json!({"adrReservedNumbers": 20}),
            json!({"adrSanitizedNumbers": [1.5]}),
        ] {
            assert!(adr_contract(&invalid).is_err(), "{invalid}");
            assert!(declaration(&invalid).is_err(), "{invalid}");
        }
    }

    #[test]
    fn ci_declaration_reads_rust_and_bun_stacks() {
        let declared = declaration(&json!({"ci": {
            "rust": {
                "workspaces": ["tools", "packages/cli"],
                "extraSteps": ["cargo xtask record-root"]
            },
            "bun": {"dirs": ["."]}
        }}))
        .unwrap();
        assert_eq!(
            declared.ci,
            Some(CiDeclaration {
                rust: Some(RustCi {
                    workspaces: vec!["tools".into(), "packages/cli".into()],
                    extra_steps: vec!["cargo xtask record-root".into()],
                }),
                bun: Some(BunCi {
                    dirs: vec![".".into()],
                }),
            })
        );
        let rust_only = ci_declaration(&json!({"ci": {"rust": {"workspaces": ["."]}}}))
            .unwrap()
            .unwrap();
        assert_eq!(
            rust_only.rust.map(|rust| rust.extra_steps),
            Some(Vec::new())
        );
        assert_eq!(rust_only.bun, None);
        // 水平タブは 1 行のままなので受ける。
        let tab = ci_declaration(
            &json!({"ci": {"rust": {"workspaces": ["."], "extraSteps": ["cargo\ttest"]}}}),
        )
        .unwrap()
        .unwrap();
        assert_eq!(
            tab.rust.map(|rust| rust.extra_steps),
            Some(vec!["cargo\ttest".into()])
        );
        assert_eq!(ci_declaration(&json!({"release": {}})).unwrap(), None);
    }

    #[test]
    fn ci_declaration_refuses_expressions_newlines_and_broken_shapes() {
        let step = |step: &str| json!({"ci": {"rust": {"workspaces": ["."], "extraSteps": ["cargo test", step]}}});
        for (invalid, named) in [
            (step("echo ${{ github.ref }}"), "ci.rust.extraSteps[1]"),
            (
                step("cargo test\ncurl example.com"),
                "ci.rust.extraSteps[1]",
            ),
            (step("cargo test\r"), "ci.rust.extraSteps[1]"),
            (step(""), "ci.rust.extraSteps[1]"),
            (json!({"ci": {}}), "ci"),
            (json!({"ci": []}), "ci"),
            (json!({"ci": {"rust": {}}}), "ci.rust.workspaces"),
            (
                json!({"ci": {"rust": {"workspaces": []}}}),
                "ci.rust.workspaces",
            ),
            (
                json!({"ci": {"rust": {"workspaces": "tools"}}}),
                "ci.rust.workspaces",
            ),
            (
                json!({"ci": {"rust": {"workspaces": [1]}}}),
                "ci.rust.workspaces",
            ),
            (
                json!({"ci": {"rust": {"workspaces": ["../x"]}}}),
                "ci.rust.workspaces",
            ),
            (
                json!({"ci": {"rust": {"workspaces": [".", "."]}}}),
                "ci.rust.workspaces",
            ),
            (
                json!({"ci": {"rust": {"workspaces": ["."], "extraSteps": "x"}}}),
                "ci.rust.extraSteps",
            ),
            (
                json!({"ci": {"rust": {"workspaces": ["."], "extraSteps": [1]}}}),
                "ci.rust.extraSteps[0]",
            ),
            (
                json!({"ci": {"rust": {"workspaces": ["."], "fmt": false}}}),
                "ci.rust.fmt",
            ),
            (json!({"ci": {"rust": true}}), "ci.rust"),
            (json!({"ci": {"bun": {}}}), "ci.bun.dirs"),
            (json!({"ci": {"bun": {"dirs": ["./web"]}}}), "ci.bun.dirs"),
            // 改行は YAML の引用の中で空白へ折り畳まれ、別の directory になる。
            (
                json!({"ci": {"bun": {"dirs": ["web\n          app"]}}}),
                "ci.bun.dirs",
            ),
            (
                json!({"ci": {"rust": {"workspaces": ["tools\r"]}}}),
                "ci.rust.workspaces",
            ),
            (
                json!({"ci": {"bun": {"dirs": ["."], "typecheck": "tsc"}}}),
                "ci.bun.typecheck",
            ),
            (json!({"ci": {"go": {}}}), "ci.go"),
        ] {
            let error = ci_declaration(&invalid).expect_err(&invalid.to_string());
            assert!(error.contains(named), "{invalid}: {error}");
            assert!(declaration(&invalid).is_err(), "{invalid}");
        }
    }

    #[test]
    fn ci_declaration_is_a_known_key_and_survives_rewrite() {
        let ci = json!({"rust": {"workspaces": ["."], "extraSteps": ["cargo xtask check"]}});
        let mut pin = json!({"version": "0.1.0", "sha256": {LINUX_TARGET: "a".repeat(64)}});
        pin["ci"] = ci.clone();
        assert!(known_shape(&pin));
        let hash = "b".repeat(64);
        let next: Value = serde_json::from_slice(
            &rewrite(
                Some(&pin),
                Pin::Legacy {
                    version: "0.1.1",
                    linux: &hash,
                },
                None,
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(next["ci"], ci);
        pin["ci"]["rust"]["extraSteps"] = json!(["echo ${{ secrets.TOKEN }}"]);
        assert!(!known_shape(&pin));

        let own = json!({"release": {"versionFiles": []}, "ci": {"bun": {"dirs": ["."]}}});
        let bytes = rewrite(Some(&own), Pin::Unpinned, None).unwrap();
        assert_eq!(bytes, to_bytes(&own).unwrap());
        let broken = json!({"ci": {"bun": {"dirs": "."}}});
        assert!(rewrite(Some(&broken), Pin::Unpinned, None).is_err());
    }

    #[test]
    fn known_shape_accepts_adr_contract_and_refuses_a_broken_one() {
        let mut pin = json!({"version": "0.1.0", "sha256": {LINUX_TARGET: "a".repeat(64)}});
        pin["adrMigratedThrough"] = json!(9);
        pin["adrReservedNumbers"] = json!([]);
        pin["adrSanitizedNumbers"] = json!([1, 4, 5]);
        assert!(known_shape(&pin));
        pin["adrReservedNumbers"] = json!("20");
        assert!(!known_shape(&pin));
    }

    #[test]
    fn rewrite_with_adr_contract_replaces_or_removes_the_contract_keys() {
        let hash = "b".repeat(64);
        let pin = || Pin::Legacy {
            version: "0.1.1",
            linux: &hash,
        };
        let current = json!({
            "version": "0.1.0",
            "sha256": {LINUX_TARGET: "a".repeat(64)},
            "adrMigratedThrough": 3,
            "adrSanitizedNumbers": [2]
        });
        let contract = AdrContract {
            migrated_through: Some(9),
            reserved_numbers: vec![],
            sanitized_numbers: vec![1, 4, 5],
        };
        let next: Value = serde_json::from_slice(
            &rewrite_with_adr_contract(Some(&current), pin(), None, Some(&contract)).unwrap(),
        )
        .unwrap();
        assert_eq!(next["adrMigratedThrough"], 9);
        assert_eq!(next["adrReservedNumbers"], json!([]));
        assert_eq!(next["adrSanitizedNumbers"], json!([1, 4, 5]));

        let removed: Value = serde_json::from_slice(
            &rewrite_with_adr_contract(Some(&current), pin(), None, None).unwrap(),
        )
        .unwrap();
        for key in ADR_CONTRACT_KEYS {
            assert!(removed.get(key).is_none(), "{key}");
        }

        let own = json!({"release": {"versionFiles": []}, "adrMigratedThrough": 3});
        let next: Value = serde_json::from_slice(
            &rewrite_with_adr_contract(Some(&own), Pin::Unpinned, Some("develop"), Some(&contract))
                .unwrap(),
        )
        .unwrap();
        assert_eq!(next["integrationBranch"], "develop");
        assert_eq!(next["adrMigratedThrough"], 9);
        assert!(next.get("version").is_none() && next.get("sha256").is_none());
        // 版の固定を持たない宣言は、無いときも固定を持つときも書き換えない。
        assert!(rewrite_with_adr_contract(None, Pin::Unpinned, None, None).is_err());
        assert!(rewrite_with_adr_contract(Some(&current), Pin::Unpinned, None, None).is_err());

        // 揃っている宣言は、欄の並びにかかわらず同じ bytes のまま書き戻す（drift に見せない）。
        for synced in [
            json!({
                "integrationBranch": "develop",
                "release": {"versionFiles": []},
                "adrMigratedThrough": 9,
                "adrReservedNumbers": [],
                "adrSanitizedNumbers": [1, 4, 5]
            }),
            json!({
                "adrMigratedThrough": 9,
                "adrReservedNumbers": [],
                "adrSanitizedNumbers": [1, 4, 5],
                "integrationBranch": "develop",
                "release": {"versionFiles": []}
            }),
        ] {
            assert_eq!(
                rewrite_with_adr_contract(
                    Some(&synced),
                    Pin::Unpinned,
                    Some("develop"),
                    Some(&contract)
                )
                .unwrap(),
                to_bytes(&synced).unwrap(),
                "{synced}"
            );
        }

        // 契約を扱わない書き換え（resolver の切り替え）は、今の契約をそのまま残す。
        let kept: Value =
            serde_json::from_slice(&rewrite(Some(&current), pin(), None).unwrap()).unwrap();
        assert_eq!(kept["adrMigratedThrough"], 3);
        assert_eq!(kept["adrSanitizedNumbers"], json!([2]));
    }
}
