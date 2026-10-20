//! link と容量上限の外にある検査項目の結果の形。各項目は自分の module（[`crate::docs`]、
//! [`crate::adr`]、[`crate::openspec`]、[`crate::records`]、[`crate::emoji`]、[`crate::retired`]）
//! が [`RuleFinding`] を返し、[`crate::check`] が `--staged` の絞り込みをする。

use std::fmt;

/// 検査項目。表示の label は試走の集計（design §5.4）にも使う。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Rule {
    RequiredFile,
    DocsLayout,
    DocsName,
    Heading,
    Index,
    Adr,
    OpenSpec,
    Todo,
    /// 移設元から移した TODO の見出し・項目の形（島標準の文書には無い規則。design §5.5）。
    TodoLayout,
    Changelog,
    Emoji,
    RetiredTerm,
}

impl Rule {
    pub fn label(self) -> &'static str {
        match self {
            Self::RequiredFile => "required-file",
            Self::DocsLayout => "docs-layout",
            Self::DocsName => "docs-name",
            Self::Heading => "h1",
            Self::Index => "index",
            Self::Adr => "adr",
            Self::OpenSpec => "openspec",
            Self::Todo => "todo",
            Self::TodoLayout => "todo-layout",
            Self::Changelog => "changelog",
            Self::Emoji => "emoji",
            Self::RetiredTerm => "retired-term",
        }
    }
}

/// 検査項目の問題 1 件。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuleFinding {
    pub rule: Rule,
    /// 問題のある path（repo root からの `/` 区切り。dir のこともある）。
    pub path: String,
    pub line: Option<usize>,
    pub message: String,
    /// `path` の他に、変われば結果が変わる path（`--staged` はこれが変わったときも報告する）。
    pub related: Vec<String>,
}

impl RuleFinding {
    pub fn new(rule: Rule, path: &str, message: impl Into<String>) -> Self {
        Self {
            rule,
            path: path.to_owned(),
            line: None,
            message: message.into(),
            related: Vec::new(),
        }
    }

    #[must_use]
    pub fn at(mut self, line: usize) -> Self {
        self.line = Some(line);
        self
    }

    #[must_use]
    pub fn related<'a>(mut self, paths: impl IntoIterator<Item = &'a str>) -> Self {
        self.related.extend(paths.into_iter().map(str::to_owned));
        self
    }
}

impl fmt::Display for RuleFinding {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.line {
            Some(line) => write!(f, "{}:{line}: ", self.path)?,
            None => write!(f, "{}: ", self.path)?,
        }
        write!(f, "[{}] {}", self.rule.label(), self.message)
    }
}
