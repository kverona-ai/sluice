use serde::{Deserialize, Serialize};

/// Which agent (or human) produced a commit. Detection is best-effort and will
/// be replaced by the bridge's provenance store (03 §6, 05 §7.3).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Agent {
    Human,
    ClaudeCode,
    CodexCli,
    GrokBuild,
    DeepSeekHarness,
    Gemini,
    KimiCode,
    QwenCode,
    ZCode,
    Copilot,
    OtherAi,
}

impl Agent {
    /// One-glyph badge used in the log's agent column (prototype: 人 / C / X / G / D).
    pub fn mark(&self) -> &'static str {
        match self {
            Agent::Human => "人",
            Agent::ClaudeCode => "C",
            Agent::CodexCli => "X",
            Agent::GrokBuild => "G",
            Agent::DeepSeekHarness => "D",
            Agent::Gemini => "Ge",
            Agent::KimiCode => "K",
            Agent::QwenCode => "Q",
            Agent::ZCode => "Z",
            Agent::Copilot => "Co",
            Agent::OtherAi => "AI",
        }
    }

    pub fn label(&self) -> &'static str {
        match self {
            Agent::Human => "Human",
            Agent::ClaudeCode => "Claude Code",
            Agent::CodexCli => "Codex CLI",
            Agent::GrokBuild => "Grok Build",
            Agent::DeepSeekHarness => "DeepSeek Harness",
            Agent::Gemini => "Gemini CLI",
            Agent::KimiCode => "Kimi Code",
            Agent::QwenCode => "Qwen Code",
            Agent::ZCode => "Z Code",
            Agent::Copilot => "Copilot CLI",
            Agent::OtherAi => "AI agent",
        }
    }

    pub fn is_ai(&self) -> bool {
        !matches!(self, Agent::Human)
    }

    /// Heuristic detection from the commit message (trailers / co-author lines)
    /// and the author identity. Trailer `Sluice-Agent:` wins when present.
    pub fn detect(message: &str, author_name: &str, author_email: &str) -> Agent {
        let msg = message.to_ascii_lowercase();
        if let Some(v) = trailer_value(&msg, "sluice-agent") {
            return match v.as_str() {
                "claude-code" | "claude" => Agent::ClaudeCode,
                "codex" | "codex-cli" => Agent::CodexCli,
                "grok" | "grok-build" => Agent::GrokBuild,
                "dsh" | "deepseek" | "deepseek-harness" => Agent::DeepSeekHarness,
                "gemini" | "gemini-cli" => Agent::Gemini,
                "kimi" | "kimi-code" => Agent::KimiCode,
                "qwen" | "qwen-code" => Agent::QwenCode,
                "zcode" | "z-code" | "glm" => Agent::ZCode,
                "copilot" | "copilot-cli" => Agent::Copilot,
                "human" => Agent::Human,
                _ => Agent::OtherAi,
            };
        }
        let who = format!("{} {}", author_name, author_email).to_ascii_lowercase();
        let coauthors: Vec<String> = msg
            .lines()
            .filter_map(|l| l.strip_prefix("co-authored-by:"))
            .map(|s| s.trim().to_string())
            .collect();
        let hay = format!("{who} {}", coauthors.join(" "));
        if hay.contains("claude") || hay.contains("anthropic") {
            Agent::ClaudeCode
        } else if hay.contains("codex") || hay.contains("openai") {
            Agent::CodexCli
        } else if hay.contains("grok") || hay.contains("x.ai") {
            Agent::GrokBuild
        } else if hay.contains("deepseek") || hay.contains("dsh") {
            Agent::DeepSeekHarness
        } else if hay.contains("gemini") {
            Agent::Gemini
        } else if hay.contains("kimi") || hay.contains("moonshot") {
            Agent::KimiCode
        } else if hay.contains("qwen") || hay.contains("tongyi") {
            Agent::QwenCode
        } else if hay.contains("zcode")
            || hay.contains("z.ai")
            || hay.contains("zhipu")
            || hay.contains("glm")
        {
            Agent::ZCode
        } else if hay.contains("copilot") {
            Agent::Copilot
        } else if hay.contains("[bot]") || hay.contains(" ai ") || hay.contains("agent@") {
            Agent::OtherAi
        } else {
            Agent::Human
        }
    }
}

fn trailer_value(lower_msg: &str, key: &str) -> Option<String> {
    lower_msg.lines().rev().find_map(|l| {
        let (k, v) = l.split_once(':')?;
        (k.trim() == key).then(|| v.trim().to_string())
    })
}
