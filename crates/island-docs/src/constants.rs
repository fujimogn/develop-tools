//! 島の docs 検査の定数。数値と語彙はここ、repo × path の行は
//! repo 内の `config/island-docs-scopes.json` から読む。
//!
//! repo は GitHub の `owner/name`（`remote.origin.url` から引く）で指す。

use std::sync::OnceLock;

use serde::Deserialize;

/// 行が効く repo。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RepoScope {
    /// 島の全 repo。
    All,
    /// 1 つの repo（GitHub の `owner/name`）。
    Repo(&'static str),
}

impl RepoScope {
    pub fn matches(self, repo: Option<&str>) -> bool {
        match self {
            Self::All => true,
            Self::Repo(name) => repo == Some(name),
        }
    }
}

/// 容量上限の 1 行。`path` は repo root からの `/` 区切り。
#[derive(Debug, Clone, Copy)]
pub struct ByteLimit {
    pub scope: RepoScope,
    pub path: &'static str,
    pub max_bytes: usize,
}

/// 島標準（`docs/develop-docs.md`「上限と退避」）の TODO.md 上限。
pub const TODO_MAX_BYTES: usize = 36_000;
/// 島標準（`docs/develop-docs.md`「上限と退避」）の CHANGELOG.md 上限。
pub const CHANGELOG_MAX_BYTES: usize = 120_000;
/// 島の入口文書の上限（design §5.2）。repo ごとに変えない。
pub const AGENTS_MAX_BYTES: usize = 12_000;
pub const CLAUDE_MAX_BYTES: usize = 2_500;
pub const CONTEXT_MAX_BYTES: usize = 16_000;

/// link を検査しない tracked Markdown（元の文書）の prefix。
#[derive(Debug, Clone, Copy)]
pub struct SourceExclusion {
    pub scope: RepoScope,
    pub prefix: &'static str,
}

// link 検査の対象は tracked の `*.md` 全部。表に載せた prefix の下だけを外す。
// untracked と ignore 済みは tracked 基準なので最初から入らない。

/// リンク先の端末固有の絶対 path の prefix。許可 repo は島の scope 表で決める。
pub const HOST_PATH_PREFIXES: &[&str] = &["/home/", "/Users/", "~/"];

/// 行番号の fragment（`#L40`、`#L10-L20`）。GitHub の file 表示で行へ飛べるので見出しと照合しない。
pub const LINE_ANCHOR_PATTERN: &str = r"^L[0-9]+(-L[0-9]+)?$";

/// 外部 link とみなす scheme（design §5.1: `^[a-z][a-z0-9+.-]*:`）。大文字の scheme も外部に数える。
pub const EXTERNAL_SCHEME_PATTERN: &str = r"^[A-Za-z][A-Za-z0-9+.-]*:";

/// 検査対象の Markdown の拡張子。
pub const MARKDOWN_EXTENSION: &str = ".md";

/// 島の全 repo が tracked に持つ入口・記録の file（design §5.2「入口・索引」）。
pub const REQUIRED_FILES: &[&str] = &[
    "README.md",
    "AGENTS.md",
    "CLAUDE.md",
    "CONTEXT.md",
    "docs/README.md",
    "docs/adr/README.md",
    "TODO.md",
    "CHANGELOG.md",
];

/// 島が §3.1 の管理区画を配る file。容量はこの区画を除いて数える。
pub const ISLAND_BLOCK_PATH: &str = "AGENTS.md";

/// docs の root。配置・H1・索引・絵文字の検査はこの下の Markdown を見る。
pub const DOCS_DIR: &str = "docs";
/// 索引の file 名。docs の各文書は、同じ dir か上位 dir（`docs` まで）のこの file から link される。
pub const INDEX_FILE_NAME: &str = "README.md";

/// 索引だけを免除する記録 directory。docs 配下の任意の深さで親 directory 名を照合する。
pub fn is_index_record_path(path: &str) -> bool {
    let Some((root, relative)) = path.split_once('/') else {
        return false;
    };
    if root != DOCS_DIR {
        return false;
    }
    let Some((parents, _)) = relative.rsplit_once('/') else {
        return false;
    };
    parents
        .split('/')
        .any(|dir| scopes().index_record_dirs.contains(&dir))
}

/// docs 直下に置ける dir の 1 行。島では配置を決めていないので、行のある repo だけを検査する。
#[derive(Debug, Clone, Copy)]
pub struct DocsLayout {
    pub scope: RepoScope,
    /// docs 直下に置ける dir。file は索引（`README.md`）だけ置ける。
    pub allowed_dirs: &'static [&'static str],
}

/// repo 固有の文書が、列挙したすべての README から link される契約。
#[derive(Debug, Clone)]
pub struct IndexRequirement {
    pub scope: RepoScope,
    pub path: &'static str,
    pub indexes: &'static [&'static str],
}

// 「repo × docs 直下」の表は同梱 JSON から読む。

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ScopeConfig {
    version: u8,
    byte_limits: Vec<RawByteLimit>,
    link_source_exclusions: Vec<RawSourceExclusion>,
    host_path_link_allow_scopes: Vec<String>,
    index_record_dirs: Vec<RawIndexRecordDirs>,
    docs_layouts: Vec<RawDocsLayout>,
    #[serde(default)]
    index_requirements: Vec<RawIndexRequirement>,
    #[serde(default)]
    retired_historical: Vec<RawSourceExclusion>,
    #[serde(default)]
    retired_extra_sources: Vec<RawSourceExclusion>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawIndexRecordDirs {
    scope: String,
    dirs: Vec<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RawByteLimit {
    scope: String,
    path: String,
    max_bytes: LimitValue,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum LimitValue {
    Constant(String),
    Bytes(usize),
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawSourceExclusion {
    scope: String,
    prefix: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RawDocsLayout {
    scope: String,
    allowed_dirs: Vec<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawIndexRequirement {
    scope: String,
    path: String,
    indexes: Vec<String>,
}

/// repo の写し（[`SCOPE_COPY_PATH`]）から読んだ「repo × path」の表。
pub struct ScopeTable {
    byte_limits: Vec<ByteLimit>,
    link_source_exclusions: Vec<SourceExclusion>,
    host_path_link_allow_scopes: Vec<RepoScope>,
    index_record_dirs: &'static [&'static str],
    docs_layouts: Vec<DocsLayout>,
    index_requirements: Vec<IndexRequirement>,
    retired_historical: Vec<SourceExclusion>,
    retired_extra_sources: Vec<SourceExclusion>,
}

/// 各 repo が tracked に持つ scope 表の写し（旧配置）。内蔵の表は無く、写しの無い repo は検査できない。
pub const SCOPE_COPY_PATH: &str = "config/island-docs-scopes.json";

/// 新配置。旧配置とはちょうど片方だけを置く（[`crate::check::select_scope_copy`]）。
pub const SCOPE_COPY_PATH_DOT_CONFIG: &str = ".config/island-docs-scopes.json";

impl ScopeTable {
    /// 旧配置の写しの bytes を読む。写しが無ければ（`None`）失敗にする。`root` は診断の path にだけ使う。
    pub fn from_copy(root: &std::path::Path, copy: Option<&[u8]>) -> Result<Self, String> {
        Self::from_copy_at(root, SCOPE_COPY_PATH, copy)
    }

    /// `relative` に置かれた写しの bytes を読む。診断には選んだ path を示す。
    pub fn from_copy_at(
        root: &std::path::Path,
        relative: &str,
        copy: Option<&[u8]>,
    ) -> Result<Self, String> {
        let path = root.join(relative);
        let bytes = copy.ok_or_else(|| {
            format!(
                "{} がありません（scope 表の写しを repo に置いてください）",
                path.display()
            )
        })?;
        let text = std::str::from_utf8(bytes)
            .map_err(|error| format!("{} が不正です: {error}", path.display()))?;
        parse_scope_table(text).map_err(|error| format!("{} が不正です: {error}", path.display()))
    }

    pub fn byte_limits_for<'a>(
        &'a self,
        repo: Option<&'a str>,
    ) -> impl Iterator<Item = &'a ByteLimit> + 'a {
        self.byte_limits
            .iter()
            .filter(move |limit| limit.scope.matches(repo))
    }
}

static ACTIVE_SCOPES: OnceLock<ScopeTable> = OnceLock::new();

#[cfg(not(test))]
fn scopes() -> &'static ScopeTable {
    ACTIVE_SCOPES
        .get()
        .expect("scope 表は検査の前に repo の写しから読む")
}

/// unit test は `example/*` の repo だけを持つ fixture の表で判定する。
#[cfg(test)]
fn scopes() -> &'static ScopeTable {
    ACTIVE_SCOPES.get_or_init(|| {
        parse_scope_table(include_str!("scope-fixture.json")).expect("fixture の scope 表は有効")
    })
}

/// 検査する snapshot の写しの表を、プロセスで一度だけ使う表にする。[`crate::check::run`] が呼ぶ。
/// `check::evaluate` を直接呼ぶ test 用の入口で、公開の API としては約束しない。
#[doc(hidden)]
pub fn initialize_scopes(table: ScopeTable) -> Result<(), String> {
    ACTIVE_SCOPES
        .set(table)
        .map_err(|_| "scope 表を二度初期化できません".to_owned())
}

fn embedded_str(value: String) -> &'static str {
    // 1 プロセスで 1 repo の表を読む。既存の判定 API は静的な path 参照を返す。
    Box::leak(value.into_boxed_str())
}

fn parse_scope(scope: String) -> Result<RepoScope, String> {
    if scope == "all" {
        return Ok(RepoScope::All);
    }
    let Some((owner, name)) = scope.split_once('/') else {
        return Err(format!("scope は owner/name または all: {scope:?}"));
    };
    if owner.is_empty()
        || name.is_empty()
        || name.contains('/')
        || scope.chars().any(char::is_whitespace)
    {
        return Err(format!("scope は owner/name または all: {scope:?}"));
    }
    Ok(RepoScope::Repo(embedded_str(scope)))
}

fn valid_path(path: &str) -> bool {
    !path.is_empty()
        && !path.starts_with('/')
        && !path.contains('\\')
        && path.split('/').all(|part| !part.is_empty() && part != "..")
}

fn valid_prefix(prefix: &str) -> bool {
    prefix.ends_with('/') && valid_path(prefix.trim_end_matches('/'))
}

fn max_bytes(value: LimitValue) -> Result<usize, String> {
    let bytes = match value {
        LimitValue::Constant(name) => match name.as_str() {
            "TODO_MAX_BYTES" => TODO_MAX_BYTES,
            "CHANGELOG_MAX_BYTES" => CHANGELOG_MAX_BYTES,
            "AGENTS_MAX_BYTES" => AGENTS_MAX_BYTES,
            "CLAUDE_MAX_BYTES" => CLAUDE_MAX_BYTES,
            "CONTEXT_MAX_BYTES" => CONTEXT_MAX_BYTES,
            _ => return Err(format!("未知の容量上限定数: {name}")),
        },
        LimitValue::Bytes(bytes) => bytes,
    };
    if bytes == 0 {
        return Err("容量上限は正の値が必要です".to_owned());
    }
    Ok(bytes)
}

fn parse_scope_table(text: &str) -> Result<ScopeTable, String> {
    let raw: ScopeConfig = serde_json::from_str(text).map_err(|err| err.to_string())?;
    if raw.version != 1 || raw.byte_limits.is_empty() {
        return Err("scope 表の version または byteLimits が不正です".to_owned());
    }

    if raw.index_record_dirs.len() != 1 || raw.index_record_dirs[0].scope != "all" {
        return Err("索引免除 directory は scope all の一行だけが必要です".to_owned());
    }
    let record_dirs = &raw.index_record_dirs[0].dirs;
    if record_dirs.is_empty()
        || record_dirs.iter().any(|dir| {
            !valid_path(dir)
                || dir.contains('/')
                || record_dirs.iter().filter(|other| *other == dir).count() != 1
        })
    {
        return Err("索引免除 directory 名が不正または重複しています".to_owned());
    }
    let index_record_dirs: Vec<&'static str> =
        record_dirs.iter().cloned().map(embedded_str).collect();

    let mut byte_limits = Vec::with_capacity(raw.byte_limits.len());
    for row in raw.byte_limits {
        if !valid_path(&row.path) {
            return Err(format!("容量上限の path が不正: {:?}", row.path));
        }
        let scope = parse_scope(row.scope)?;
        if byte_limits.iter().any(|entry: &ByteLimit| {
            entry.path == row.path && (entry.scope == scope || entry.scope == RepoScope::All)
        }) || (scope == RepoScope::All
            && byte_limits
                .iter()
                .any(|entry: &ByteLimit| entry.path == row.path))
        {
            return Err(format!(
                "容量上限の行が重複または島の行を上書き: {}",
                row.path
            ));
        }
        byte_limits.push(ByteLimit {
            scope,
            path: embedded_str(row.path),
            max_bytes: max_bytes(row.max_bytes)?,
        });
    }

    let link_source_exclusions =
        parse_exclusions(raw.link_source_exclusions, valid_prefix, "link 除外")?;
    let retired_historical = parse_exclusions(
        raw.retired_historical,
        |prefix| valid_path(prefix.trim_end_matches('/')),
        "退役語の履歴",
    )?;
    let retired_extra_sources =
        parse_exclusions(raw.retired_extra_sources, valid_prefix, "退役語の追加範囲")?;

    let mut host_path_link_allow_scopes = Vec::new();
    for name in raw.host_path_link_allow_scopes {
        let scope = parse_scope(name)?;
        if scope == RepoScope::All || host_path_link_allow_scopes.contains(&scope) {
            return Err("端末 path link の許可 repo が不正または重複しています".to_owned());
        }
        host_path_link_allow_scopes.push(scope);
    }

    let mut docs_layouts = Vec::with_capacity(raw.docs_layouts.len());
    for row in raw.docs_layouts {
        let scope = parse_scope(row.scope)?;
        if row.allowed_dirs.is_empty()
            || row
                .allowed_dirs
                .iter()
                .any(|dir| !valid_path(dir) || dir.contains('/'))
            || docs_layouts
                .iter()
                .any(|entry: &DocsLayout| entry.scope == scope)
        {
            return Err("docs 配置行が不正または重複しています".to_owned());
        }
        let allowed_dirs: Vec<&'static str> =
            row.allowed_dirs.into_iter().map(embedded_str).collect();
        docs_layouts.push(DocsLayout {
            scope,
            allowed_dirs: Box::leak(allowed_dirs.into_boxed_slice()),
        });
    }
    let mut index_requirements = Vec::with_capacity(raw.index_requirements.len());
    for row in raw.index_requirements {
        let scope = parse_scope(row.scope)?;
        if scope == RepoScope::All
            || !valid_path(&row.path)
            || !row.path.starts_with("docs/")
            || !row.path.ends_with(MARKDOWN_EXTENSION)
            || row.path == "docs/README.md"
            || row.indexes.is_empty()
            || row.indexes.iter().any(|index| {
                !valid_path(index)
                    || !index.starts_with("docs/")
                    || !index.ends_with("/README.md")
                    || index == &row.path
                    || row.indexes.iter().filter(|other| *other == index).count() != 1
            })
            || index_requirements
                .iter()
                .any(|entry: &IndexRequirement| entry.scope == scope && entry.path == row.path)
        {
            return Err(format!("索引要求の行が不正または重複: {}", row.path));
        }
        let indexes: Vec<&'static str> = row.indexes.into_iter().map(embedded_str).collect();
        index_requirements.push(IndexRequirement {
            scope,
            path: embedded_str(row.path),
            indexes: Box::leak(indexes.into_boxed_slice()),
        });
    }
    Ok(ScopeTable {
        byte_limits,
        link_source_exclusions,
        host_path_link_allow_scopes,
        index_record_dirs: Box::leak(index_record_dirs.into_boxed_slice()),
        docs_layouts,
        index_requirements,
        retired_historical,
        retired_extra_sources,
    })
}

fn parse_exclusions(
    rows: Vec<RawSourceExclusion>,
    valid: fn(&str) -> bool,
    label: &str,
) -> Result<Vec<SourceExclusion>, String> {
    let mut parsed = Vec::with_capacity(rows.len());
    for row in rows {
        if !valid(&row.prefix) {
            return Err(format!("{label}の prefix が不正: {:?}", row.prefix));
        }
        let scope = parse_scope(row.scope)?;
        if parsed
            .iter()
            .any(|entry: &SourceExclusion| entry.scope == scope && entry.prefix == row.prefix)
        {
            return Err(format!("{label}の行が重複: {}", row.prefix));
        }
        parsed.push(SourceExclusion {
            scope,
            prefix: embedded_str(row.prefix),
        });
    }
    Ok(parsed)
}

/// ADR の置き場（直下の file だけが ADR 本体）。
pub const ADR_DIR: &str = "docs/adr";
/// ADR の置き場にあって ADR 本体ではない file。
pub const ADR_NON_RECORD_FILES: &[&str] = &["README.md", "adr-template.md"];
/// ADR の status 語彙（島の正本 `bin/_adr-standard-core.ts` の `validStatus`）。後継は
/// [`ADR_SUPERSEDED_PREFIX`] に 4 桁の番号を続けた 1 語で書く（別キー `superseded-by` は使わない）。
pub const ADR_STATUSES: &[&str] = &["proposed", "accepted", "rejected", "deprecated"];
pub const ADR_SUPERSEDED_PREFIX: &str = "superseded by ADR-";

/// OpenSpec の change の置き場。`archive` は完了した change なので検査しない。
pub const OPENSPEC_CHANGES_DIR: &str = "openspec/changes";
pub const OPENSPEC_ARCHIVE_DIR_NAME: &str = "archive";
/// active change が持つ file（design §5.2）。
pub const OPENSPEC_REQUIRED_FILES: &[&str] = &["proposal.md", "tasks.md"];
/// active change の名前。日付の prefix は有っても無くてもよい（design §5.2）。
pub const OPENSPEC_CHANGE_NAME_PATTERN: &str =
    r"^(?:[0-9]{4}-[0-9]{2}-[0-9]{2}-)?[a-z0-9]+(?:-[a-z0-9]+)*$";

/// 島標準の TODO / CHANGELOG（`docs/develop-docs.md`）。repo 直下の file だけが対象。
pub const TODO_PATH: &str = "TODO.md";
pub const CHANGELOG_PATH: &str = "CHANGELOG.md";
/// TODO.md の `##` 見出しに使える語と、その順。全部は要らないが、使うならこの順に
/// 1 回ずつ置く。項目の無い見出しも置いてよい。分野で分けたいときは見出しではなく項目名の先頭に書く。
/// - 進行中: 今手を動かしている／次の一手が決まっている
/// - 次に着手: 着手できるが、まだ始めていない
/// - 待ち: 外部要因・利用者の判断・モノや現地の都合で止まっている（理由は各項目の現在地に書く）
/// - いつか: 時期未定
pub const TODO_SECTIONS: &[&str] = &["進行中", "次に着手", "待ち", "いつか"];

/// 絵文字とみなす文字の範囲（移設元: `check-docs` の `emoji.rs`）。✓ ✗ や矢印のような
/// 文字としての記号は含めない。
pub const EMOJI_RANGES: &[(char, char)] = &[
    ('\u{1F300}', '\u{1FAFF}'),
    ('\u{2600}', '\u{26FF}'),
    ('\u{23E9}', '\u{23FA}'),
    ('\u{2705}', '\u{2705}'),
    ('\u{2714}', '\u{2714}'),
    ('\u{274C}', '\u{274C}'),
    ('\u{274E}', '\u{274E}'),
    ('\u{2753}', '\u{2757}'),
    ('\u{2B50}', '\u{2B50}'),
    ('\u{2B55}', '\u{2B55}'),
];

/// 退役語の registry（形式は移設元の `docs/agents/retired-identifiers.json`）。語は各 repo が
/// この path に置き、形式と検査は島が持つ（design §5.2）。file が無い repo は検査しない。
pub const RETIRED_REGISTRY_PATH: &str = "docs/agents/retired-identifiers.json";
/// 行にこの注記（理由付き）があれば、その行の退役語を許す。
pub const RETIRED_SUPPRESSION_PREFIX: &str = "<!-- retired-ok:";

/// 退役語を検査しない、当時の記録の prefix（path そのものも prefix として数える）。
pub const RETIRED_HISTORICAL: &[SourceExclusion] = &[
    SourceExclusion {
        scope: RepoScope::All,
        prefix: "CHANGELOG.md",
    },
    SourceExclusion {
        scope: RepoScope::All,
        prefix: "docs/adr/",
    },
    SourceExclusion {
        scope: RepoScope::All,
        prefix: "docs/archive/",
    },
    SourceExclusion {
        scope: RepoScope::All,
        prefix: "docs/experiments/",
    },
    SourceExclusion {
        scope: RepoScope::All,
        prefix: "docs/plans/",
    },
    SourceExclusion {
        scope: RepoScope::All,
        prefix: "docs/reviews/",
    },
    SourceExclusion {
        scope: RepoScope::All,
        prefix: "docs/superpowers/plans/",
    },
];

/// Bun 実行例も退役語と同じ履歴記録を除く。追加分は、script 名が将来の package.json と
/// 無関係になりやすい記録だけ。`RETIRED_HISTORICAL` と合わせて使う。
pub const SCRIPT_REFERENCE_HISTORICAL_EXTRA: &[SourceExclusion] = &[
    SourceExclusion {
        scope: RepoScope::All,
        prefix: "docs/review/",
    },
    SourceExclusion {
        scope: RepoScope::All,
        prefix: "docs/superpowers/specs/",
    },
    SourceExclusion {
        scope: RepoScope::All,
        prefix: "docs/operations/runbook/snapshots/",
    },
    SourceExclusion {
        scope: RepoScope::All,
        prefix: ".claude/lessons/",
    },
    SourceExclusion {
        scope: RepoScope::All,
        prefix: ".superpowers/",
    },
    SourceExclusion {
        scope: RepoScope::All,
        prefix: "openspec/changes/",
    },
];

/// 島の横断ガイド・子 repo template・監査記録は、この repo の package scripts の
/// 実行手順ではない。対象 repo 固有の docs/README.md などは除外しない。
pub const SCRIPT_REFERENCE_CONTEXT_EXCLUSIONS: &[SourceExclusion] = &[
    SourceExclusion {
        scope: RepoScope::All,
        prefix: "templates/",
    },
    SourceExclusion {
        scope: RepoScope::All,
        prefix: "docs/develop-",
    },
    SourceExclusion {
        scope: RepoScope::All,
        prefix: "docs/bun-toolchain-audit.md",
    },
];

pub fn byte_limits_for<'a>(repo: Option<&'a str>) -> impl Iterator<Item = &'a ByteLimit> + 'a {
    scopes().byte_limits_for(repo)
}

pub fn is_link_source(repo: Option<&str>, path: &str) -> bool {
    path.ends_with(MARKDOWN_EXTENSION)
        && !scopes()
            .link_source_exclusions
            .iter()
            .any(|rule| rule.scope.matches(repo) && path.starts_with(rule.prefix))
}

pub fn allows_host_path_links(repo: Option<&str>) -> bool {
    scopes()
        .host_path_link_allow_scopes
        .iter()
        .any(|scope| scope.matches(repo))
}

/// 退役語と Bun 実行例を検査しない当時の記録か。島の行は [`RETIRED_HISTORICAL`]、repo の行は scope 表。
pub fn is_retired_historical(repo: Option<&str>, path: &str) -> bool {
    matches_prefix(RETIRED_HISTORICAL, repo, path)
        || matches_prefix(&scopes().retired_historical, repo, path)
}

/// 島の範囲（repo 直下と `docs/`）の外で、退役語を検査する prefix に当たるか。行は scope 表だけが持つ
/// （配る側の skill のように、写しではなく正本を置く repo の行）。
pub fn is_retired_extra_source(repo: Option<&str>, path: &str) -> bool {
    matches_prefix(&scopes().retired_extra_sources, repo, path)
}

/// `rules` の中に、`repo` に効いて `path` に当たる行があるか。
pub fn matches_prefix(rules: &[SourceExclusion], repo: Option<&str>, path: &str) -> bool {
    rules
        .iter()
        .any(|rule| rule.scope.matches(repo) && path.starts_with(rule.prefix))
}

pub fn docs_layout_for(repo: Option<&str>) -> Option<&'static DocsLayout> {
    scopes()
        .docs_layouts
        .iter()
        .find(|layout| layout.scope.matches(repo))
}

pub fn index_requirement_for(repo: Option<&str>, path: &str) -> Option<&'static IndexRequirement> {
    scopes()
        .index_requirements
        .iter()
        .find(|requirement| requirement.scope.matches(repo) && requirement.path == path)
}

pub fn index_requirements_for(
    repo: Option<&str>,
) -> impl Iterator<Item = &'static IndexRequirement> {
    scopes()
        .index_requirements
        .iter()
        .filter(move |requirement| requirement.scope.matches(repo))
}

pub fn is_emoji(c: char) -> bool {
    EMOJI_RANGES
        .iter()
        .any(|(start, end)| (*start..=*end).contains(&c))
}

#[cfg(test)]
mod tests {
    use super::*;

    // 配布側の scope_filter_covers_every_field_in_island_docs_schema と対で欄の追加漏れを検出する。
    #[test]
    fn scope_table_fields_are_exhaustive() {
        let ScopeTable {
            byte_limits: _,
            link_source_exclusions: _,
            host_path_link_allow_scopes: _,
            index_record_dirs: _,
            docs_layouts: _,
            index_requirements: _,
            retired_historical: _,
            retired_extra_sources: _,
        } = scopes();
    }

    /// 退役語の repo 行は写しの欄で持つ。欄の無い写しは行なし、file の path も prefix として書ける。
    #[test]
    fn retired_rows_are_optional_scope_table_fields() {
        let base = r#""version":1,"byteLimits":[{"scope":"all","path":"TODO.md","maxBytes":1}],"linkSourceExclusions":[],"hostPathLinkAllowScopes":[],"indexRecordDirs":[{"scope":"all","dirs":["archive"]}],"docsLayouts":[]"#;
        let without = parse_scope_table(&format!("{{{base}}}")).expect("欄の無い写し");
        assert!(without.retired_historical.is_empty());
        assert!(without.retired_extra_sources.is_empty());

        let with = parse_scope_table(&format!(
            r#"{{{base},"retiredHistorical":[{{"scope":"example/meta","prefix":"docs/old-notes.md"}}],"retiredExtraSources":[{{"scope":"example/meta","prefix":".claude/skills/"}}]}}"#
        ))
        .expect("欄のある写し");
        assert!(matches_prefix(
            &with.retired_historical,
            Some("example/meta"),
            "docs/old-notes.md"
        ));
        assert!(!matches_prefix(
            &with.retired_historical,
            Some("example/other"),
            "docs/old-notes.md"
        ));
        assert!(matches_prefix(
            &with.retired_extra_sources,
            Some("example/meta"),
            ".claude/skills/a/SKILL.md"
        ));

        for bad in [
            r#""retiredHistorical":[{"scope":"example/meta","prefix":"/abs"}]"#,
            r#""retiredExtraSources":[{"scope":"example/meta","prefix":".claude/skills"}]"#,
        ] {
            assert!(
                parse_scope_table(&format!("{{{base},{bad}}}")).is_err(),
                "{bad}"
            );
        }
    }

    #[test]
    fn emoji_ranges_keep_text_symbols() {
        for c in ['⚠', '✅', '🚫', '💡', '⭐', '⏸', '🔁', '✔', '❌'] {
            assert!(is_emoji(c), "{c} を絵文字として拾う");
        }
        for c in ['✓', '✗', '→', '①', '—', 'あ', 'A'] {
            assert!(!is_emoji(c), "{c} は文字記号として許す");
        }
    }

    /// 同じ repo × path に 2 行あると、どちらが効くかが表の順に依存する。
    #[test]
    fn each_repo_and_path_has_one_limit() {
        for (index, limit) in scopes().byte_limits.iter().enumerate() {
            for other in &scopes().byte_limits[index + 1..] {
                assert!(
                    !(limit.scope == other.scope && limit.path == other.path),
                    "{:?} {} が 2 行ある",
                    limit.scope,
                    limit.path
                );
            }
        }
    }

    #[test]
    fn island_rows_apply_to_every_repo_and_repo_rows_only_to_that_repo() {
        let home: Vec<_> = byte_limits_for(Some("example/home"))
            .map(|limit| limit.path)
            .collect();
        assert!(home.contains(&"TODO.md"));
        assert!(home.contains(&"home/AGENTS.md"));
        let unknown: Vec<_> = byte_limits_for(None).map(|limit| limit.path).collect();
        assert_eq!(
            unknown,
            [
                "AGENTS.md",
                "CLAUDE.md",
                "CONTEXT.md",
                "TODO.md",
                "CHANGELOG.md"
            ]
        );
    }

    /// 島の入口文書の上限は repo によらず 1 つ。repo 行で同じ path の値を変えられない。
    #[test]
    fn island_limits_are_the_same_for_every_repo() {
        let expected = [
            ("AGENTS.md", 12_000),
            ("CLAUDE.md", 2_500),
            ("CONTEXT.md", 16_000),
            ("TODO.md", 36_000),
            ("CHANGELOG.md", 120_000),
        ];
        for repo in [
            None,
            Some("example/other"),
            Some("example/home"),
            Some("example/primary"),
        ] {
            for (path, max_bytes) in expected {
                let found: Vec<_> = byte_limits_for(repo)
                    .filter(|limit| limit.path == path)
                    .map(|limit| limit.max_bytes)
                    .collect();
                assert_eq!(found, [max_bytes], "{repo:?} {path}");
            }
        }
        for limit in &scopes().byte_limits {
            if let RepoScope::Repo(name) = limit.scope {
                assert!(
                    !scopes()
                        .byte_limits
                        .iter()
                        .any(|island| island.scope == RepoScope::All && island.path == limit.path),
                    "{name} が島の行 {} を上書きしている",
                    limit.path
                );
            }
        }
    }

    #[test]
    fn docs_readme_has_no_byte_limit_in_any_repo() {
        for repo in [
            None,
            Some("example/meta"),
            Some("example/home"),
            Some("example/primary"),
        ] {
            assert!(
                byte_limits_for(repo).all(|limit| limit.path != "docs/README.md"),
                "{repo:?}"
            );
        }
    }

    /// HOME の入口（`~/AGENTS.md`、`~/CLAUDE.md`）は島の入口と同じ値（`home/AGENTS.md` の上限行と一致）。
    #[test]
    fn home_entry_limits_match_the_island_entry_limits() {
        let home: Vec<_> = byte_limits_for(Some("example/home"))
            .map(|limit| (limit.path, limit.max_bytes))
            .collect();
        assert!(home.contains(&("home/AGENTS.md", AGENTS_MAX_BYTES)));
        assert!(home.contains(&("home/CLAUDE.md", CLAUDE_MAX_BYTES)));
    }

    #[test]
    fn host_path_link_exception_cannot_expand_to_all_repos() {
        assert!(allows_host_path_links(Some("example/home")));
        assert!(!allows_host_path_links(Some("example/primary")));
        assert!(!allows_host_path_links(None));

        let mut config: serde_json::Value =
            serde_json::from_str(include_str!("scope-fixture.json")).expect("fixture config");
        config["hostPathLinkAllowScopes"] = serde_json::json!(["all"]);
        assert!(parse_scope_table(&config.to_string()).is_err());
    }

    #[test]
    fn exclusions_are_scoped_by_repo() {
        let primary = "docs/references/primary-sources/x.md";
        assert!(!is_link_source(Some("example/primary"), primary));
        assert!(is_link_source(Some("owner/other"), primary));
        assert!(!is_link_source(None, "openspec/changes/archive/x/tasks.md"));
        assert!(!is_link_source(None, "README.txt"));
    }

    /// 写した参照文書は、その repo でだけ見ない。同じ repo の他の文書と、他 repo の同じ path は見る。
    #[test]
    fn copied_references_are_excluded_only_in_their_repo() {
        let site = Some("example/site");
        assert!(!is_link_source(
            site,
            "docs/references/upstream/domain-model.md"
        ));
        assert!(!is_link_source(site, "docs/references/upstream/README.md"));
        assert!(is_link_source(site, "docs/references/README.md"));
        assert!(is_link_source(site, "docs/design/plan.md"));
        assert!(is_link_source(
            Some("example/primary"),
            "docs/references/upstream/domain-model.md"
        ));
    }

    #[test]
    fn repo_rows_set_limits_exclusions_and_layouts() {
        let home = Some("example/home");
        let limits: Vec<_> = byte_limits_for(home)
            .map(|limit| (limit.path, limit.max_bytes))
            .collect();
        assert!(limits.contains(&("home/AGENTS.md", 12_000)));
        assert!(limits.contains(&("home/CLAUDE.md", 2_500)));
        assert!(limits.contains(&("home/.agents/AGENTS.md", 28_000)));
        assert!(limits.contains(&("README.md", 16_000)));

        assert!(!is_link_source(
            Some("example/primary"),
            "docs/references/primary-sources/source.md"
        ));
        assert!(!is_link_source(
            Some("example/site"),
            "docs/references/upstream/README.md"
        ));
        assert!(is_link_source(
            Some("owner/other"),
            "docs/references/upstream/README.md"
        ));

        assert_eq!(
            docs_layout_for(home).map(|layout| layout.allowed_dirs),
            Some(["setup", "operations", "guides", "reference", "adr"].as_slice())
        );
        assert!(docs_layout_for(Some("owner/other")).is_none());
    }

    #[test]
    fn an_added_repo_row_changes_the_decision_without_a_source_change() {
        let mut config: serde_json::Value =
            serde_json::from_str(include_str!("scope-fixture.json")).expect("fixture config");
        config["byteLimits"]
            .as_array_mut()
            .expect("byteLimits")
            .push(serde_json::json!({
                "scope": "example/new-repo",
                "path": "docs/custom.md",
                "maxBytes": 42
            }));
        let table = parse_scope_table(&config.to_string()).expect("new repo row");
        assert_eq!(
            table
                .byte_limits_for(Some("example/new-repo"))
                .find(|limit| limit.path == "docs/custom.md")
                .map(|limit| limit.max_bytes),
            Some(42)
        );
        assert!(
            table
                .byte_limits_for(Some("example/other"))
                .all(|limit| limit.path != "docs/custom.md")
        );
    }

    #[test]
    fn malformed_or_empty_scope_table_is_rejected() {
        assert!(parse_scope_table(r#"{"version":1,"byteLimits":{}}"#).is_err());
        let error = parse_scope_table(
            r#"{"version":1,"byteLimits":[{"scope":"","path":"x.md","maxBytes":1}],"indexRecordDirs":[{"scope":"all","dirs":["docs"]}],"linkSourceExclusions":[],"hostPathLinkAllowScopes":[],"docsLayouts":[]}"#,
        )
        .err()
        .expect("empty scope must be rejected");
        assert!(error.contains("scope は owner/name または all"), "{error}");
    }

    #[test]
    fn index_requirements_reject_duplicates_and_global_rows() {
        let mut config: serde_json::Value =
            serde_json::from_str(include_str!("scope-fixture.json")).expect("fixture config");
        {
            let rows = config["indexRequirements"]
                .as_array_mut()
                .expect("indexRequirements");
            rows.push(rows[0].clone());
        }
        assert!(parse_scope_table(&config.to_string()).is_err());
        config["indexRequirements"].as_array_mut().unwrap().pop();
        config["indexRequirements"][0]["scope"] = serde_json::json!("all");
        assert!(parse_scope_table(&config.to_string()).is_err());
    }
}
