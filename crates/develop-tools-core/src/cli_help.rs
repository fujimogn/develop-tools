//! CLI help の共通見出し。実行処理には関与しない。

pub struct Help<'a> {
    usage: &'a str,
    description: &'a str,
    default: &'a str,
    writes: &'a str,
    exit: Option<&'a str>,
    examples: &'a [&'a str],
}

impl<'a> Help<'a> {
    pub fn new(usage: &'a str, description: &'a str, default: &'a str, writes: &'a str) -> Self {
        Self {
            usage,
            description,
            default,
            writes,
            exit: None,
            examples: &[],
        }
    }

    pub fn exit(mut self, exit: &'a str) -> Self {
        self.exit = Some(exit);
        self
    }

    pub fn examples(mut self, examples: &'a [&'a str]) -> Self {
        self.examples = examples;
        self
    }

    pub fn render(&self) -> String {
        let mut output = format!(
            "Usage: {}\n\n{}\n\nDefault: {}\nWrites: {}\n",
            self.usage, self.description, self.default, self.writes
        );
        if let Some(exit) = self.exit {
            output.push_str(&format!("Exit: {exit}\n"));
        }
        if !self.examples.is_empty() {
            output.push_str("\nExamples:\n");
            for example in self.examples {
                output.push_str(&format!("  {example}\n"));
            }
        }
        output
    }
}
