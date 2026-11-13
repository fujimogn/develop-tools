//! 起動 context 計測と文書警告が共用する agent profile。

#[derive(Clone, Copy)]
pub struct AgentRule {
    pub name: &'static str,
    pub global_docs: &'static [&'static str],
    pub chain_docs: &'static [&'static str],
    pub local_docs: &'static [&'static str],
    pub rule_dirs: &'static [&'static str],
    pub global_skill_dirs: &'static [&'static str],
    pub local_skill_dirs: &'static [&'static str],
    pub expand_imports: bool,
}

pub const RULES: &[AgentRule] = &[
    AgentRule {
        name: "Claude Code",
        global_docs: &[".claude/CLAUDE.md"],
        chain_docs: &["CLAUDE.md", "CLAUDE.local.md"],
        local_docs: &[],
        rule_dirs: &[".claude/rules"],
        global_skill_dirs: &[".claude/skills"],
        local_skill_dirs: &[".claude/skills"],
        expand_imports: true,
    },
    AgentRule {
        name: "Codex",
        global_docs: &[".codex/AGENTS.md"],
        chain_docs: &["AGENTS.md"],
        local_docs: &[],
        rule_dirs: &[],
        global_skill_dirs: &[".codex/skills", ".agents/skills"],
        local_skill_dirs: &[".agents/skills"],
        expand_imports: false,
    },
    AgentRule {
        name: "Grok",
        global_docs: &[".grok/AGENTS.md", ".claude/CLAUDE.md"],
        chain_docs: &[],
        local_docs: &["AGENTS.md", "CLAUDE.md"],
        rule_dirs: &[".claude/rules"],
        global_skill_dirs: &[".grok/skills"],
        local_skill_dirs: &[".grok/skills"],
        expand_imports: false,
    },
    AgentRule {
        name: "Kimi",
        global_docs: &[],
        chain_docs: &[],
        local_docs: &["AGENTS.md", ".kimi-code/AGENTS.md"],
        rule_dirs: &[],
        global_skill_dirs: &[".kimi-code/skills", ".agents/skills"],
        local_skill_dirs: &[".kimi-code/skills", ".agents/skills"],
        expand_imports: false,
    },
    AgentRule {
        name: "agy",
        global_docs: &[
            ".gemini/AGENTS.md",
            ".gemini/GEMINI.md",
            ".gemini/config/AGENTS.md",
            ".gemini/config/GEMINI.md",
        ],
        chain_docs: &[],
        local_docs: &["AGENTS.md", "GEMINI.md", ".agents/AGENTS.md"],
        rule_dirs: &[],
        global_skill_dirs: &[".gemini/antigravity-cli/skills"],
        local_skill_dirs: &[".agents/skills"],
        expand_imports: false,
    },
];

/// 常駐文書の import 行から宛先の文字列を取り出す。
///
/// 行頭が `@` で、前後の空白を除いたあと空でなく、空白を含まないときだけ `Some`。
/// path の解決は呼出側に残す。文書警告は index と links、context 計測は filesystem と HOME。
pub fn import_target(line: &str) -> Option<&str> {
    let target = line.strip_prefix('@')?.trim();
    if target.is_empty() || target.contains(char::is_whitespace) {
        None
    } else {
        Some(target)
    }
}
