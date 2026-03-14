//! Auto-reply command system: structured command detection and dispatch.
//!
//! Detects `/command` prefixes in user messages, dispatches to registered
//! handlers, and extracts inline directives before passing to the model.
//! Commands bypass the model entirely; directives modify model behavior.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

/// Result of command processing.
#[derive(Debug, Clone)]
pub struct CommandResult {
    /// Direct reply text (bypasses the model).
    pub reply: Option<String>,
    /// Whether to continue to the model after command processing.
    pub continue_to_model: bool,
}

impl CommandResult {
    /// Command was handled; send reply and stop.
    pub fn handled(reply: impl Into<String>) -> Self {
        Self {
            reply: Some(reply.into()),
            continue_to_model: false,
        }
    }

    /// Command was handled silently; no reply, stop processing.
    pub fn handled_silent() -> Self {
        Self {
            reply: None,
            continue_to_model: false,
        }
    }

    /// Not a command; continue to model.
    pub fn pass() -> Self {
        Self {
            reply: None,
            continue_to_model: true,
        }
    }
}

/// Category for grouping commands in help output.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CommandCategory {
    Session,
    Options,
    Status,
}

/// Definition of a command.
#[derive(Debug, Clone)]
pub struct CommandDef {
    /// Primary command name (e.g., "help").
    pub name: &'static str,
    /// Aliases (e.g., ["h"]).
    pub aliases: &'static [&'static str],
    /// Short description for help output.
    pub description: &'static str,
    /// Whether this command accepts arguments.
    pub accepts_args: bool,
    /// Category for grouping.
    pub category: CommandCategory,
}

/// Parsed command from user input.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedCommand {
    /// Command name (lowercase, without the `/` prefix).
    pub name: String,
    /// Arguments after the command name.
    pub args: String,
}

/// Inline directives extracted from message body.
///
/// These modify model behavior but don't bypass the model.
#[derive(Debug, Clone, Default)]
pub struct InlineDirectives {
    /// Thinking level override (e.g., "off", "low", "medium", "high").
    pub think: Option<String>,
    /// Model override for this message only.
    pub model: Option<String>,
    /// The message body with directives stripped.
    pub cleaned_body: String,
}

// ── Built-in command definitions ────────────────────────────────────────

/// All registered commands.
pub static COMMANDS: &[CommandDef] = &[
    CommandDef {
        name: "help",
        aliases: &["h"],
        description: "Show available commands",
        accepts_args: false,
        category: CommandCategory::Status,
    },
    CommandDef {
        name: "status",
        aliases: &[],
        description: "Show current session and agent status",
        accepts_args: false,
        category: CommandCategory::Status,
    },
    CommandDef {
        name: "reset",
        aliases: &["new"],
        description: "Reset the current session",
        accepts_args: false,
        category: CommandCategory::Session,
    },
    CommandDef {
        name: "compact",
        aliases: &[],
        description: "Force context compaction",
        accepts_args: false,
        category: CommandCategory::Session,
    },
    CommandDef {
        name: "stop",
        aliases: &[],
        description: "Cancel the current generation",
        accepts_args: false,
        category: CommandCategory::Session,
    },
    CommandDef {
        name: "model",
        aliases: &["m"],
        description: "Switch or show the current model",
        accepts_args: true,
        category: CommandCategory::Options,
    },
    CommandDef {
        name: "think",
        aliases: &["t"],
        description: "Set thinking level (off, low, medium, high)",
        accepts_args: true,
        category: CommandCategory::Options,
    },
    CommandDef {
        name: "usage",
        aliases: &[],
        description: "Show token usage for this session",
        accepts_args: false,
        category: CommandCategory::Status,
    },
];

// ── Detection & parsing ─────────────────────────────────────────────────

/// Check if a message starts with a known command.
pub fn detect_command(message: &str) -> Option<ParsedCommand> {
    let trimmed = message.trim();
    if !trimmed.starts_with('/') {
        return None;
    }

    // Extract command name (until first whitespace).
    let after_slash = &trimmed[1..];
    let (cmd_name, args) = match after_slash.find(char::is_whitespace) {
        Some(pos) => (&after_slash[..pos], after_slash[pos..].trim()),
        None => (after_slash, ""),
    };

    let cmd_lower = cmd_name.to_ascii_lowercase();

    // Strip @botname suffix (e.g., "/help@mybot").
    let cmd_clean = cmd_lower.split('@').next().unwrap_or(&cmd_lower);

    // Check against registered commands and aliases.
    let is_known = COMMANDS.iter().any(|def| {
        def.name == cmd_clean || def.aliases.contains(&cmd_clean)
    });

    if is_known {
        Some(ParsedCommand {
            name: resolve_alias(cmd_clean).to_string(),
            args: args.to_string(),
        })
    } else {
        None
    }
}

/// Resolve command aliases to the canonical command name.
fn resolve_alias(name: &str) -> &str {
    for def in COMMANDS {
        if def.name == name {
            return def.name;
        }
        if def.aliases.contains(&name) {
            return def.name;
        }
    }
    name
}

/// Extract inline directives from a message body.
///
/// Directives like `/think high` or `/model gpt-4o` are extracted from
/// anywhere in the message and the message body is returned with them removed.
pub fn extract_directives(message: &str) -> InlineDirectives {
    let mut directives = InlineDirectives::default();
    let mut cleaned_parts: Vec<&str> = Vec::new();
    let mut i = 0;
    let chars: Vec<char> = message.chars().collect();
    let len = chars.len();

    // Track byte positions for slicing.
    let mut last_end = 0;

    while i < len {
        // Look for `/` preceded by start-of-string or whitespace.
        if chars[i] == '/' && (i == 0 || chars[i - 1].is_whitespace()) {
            let directive_start_byte = message
                .char_indices()
                .nth(i)
                .map_or(message.len(), |(idx, _)| idx);

            // Extract the directive word.
            let rest = &message[directive_start_byte + 1..];
            let word_end = rest
                .find(|c: char| c.is_whitespace())
                .unwrap_or(rest.len());
            let word = &rest[..word_end];
            let word_lower = word.to_ascii_lowercase();

            match word_lower.as_str() {
                "think" | "t" => {
                    // Capture the argument.
                    let arg_start = directive_start_byte + 1 + word_end;
                    let arg_rest = message[arg_start..].trim_start();
                    let arg_end = arg_rest
                        .find(|c: char| c.is_whitespace())
                        .unwrap_or(arg_rest.len());
                    let level = &arg_rest[..arg_end];

                    if matches!(
                        level,
                        "off" | "low" | "medium" | "high" | "minimal"
                    ) {
                        directives.think = Some(level.to_string());
                        // Push text before this directive.
                        cleaned_parts.push(&message[last_end..directive_start_byte]);
                        // Skip past directive + argument.
                        let total_consumed = (arg_rest.as_ptr() as usize - message.as_ptr() as usize) + arg_end;
                        last_end = total_consumed;
                        i += word.len() + 1 + level.len() + (arg_start - directive_start_byte - 1 - word_end);
                        // Advance past the whole directive.
                        while i < len && !chars[i].is_whitespace() {
                            i += 1;
                        }
                        continue;
                    }
                }
                "model" | "m" => {
                    let arg_start = directive_start_byte + 1 + word_end;
                    let arg_rest = message[arg_start..].trim_start();
                    let arg_end = arg_rest
                        .find(|c: char| c.is_whitespace())
                        .unwrap_or(arg_rest.len());
                    let model_id = &arg_rest[..arg_end];

                    if !model_id.is_empty() {
                        directives.model = Some(model_id.to_string());
                        cleaned_parts.push(&message[last_end..directive_start_byte]);
                        let total_consumed = (arg_rest.as_ptr() as usize - message.as_ptr() as usize) + arg_end;
                        last_end = total_consumed;
                        while i < len && !chars[i].is_whitespace() {
                            i += 1;
                        }
                        continue;
                    }
                }
                _ => {}
            }
        }
        i += 1;
    }

    // Collect remaining text.
    cleaned_parts.push(&message[last_end..]);
    directives.cleaned_body = cleaned_parts.join("").trim().to_string();

    directives
}

/// Generate help text for all registered commands.
pub fn help_text() -> String {
    let mut by_category: HashMap<CommandCategory, Vec<&CommandDef>> = HashMap::new();
    for cmd in COMMANDS {
        by_category.entry(cmd.category).or_default().push(cmd);
    }

    let mut parts = Vec::new();
    parts.push("**Available Commands**\n".to_string());

    let order = [
        (CommandCategory::Session, "Session"),
        (CommandCategory::Options, "Options"),
        (CommandCategory::Status, "Status"),
    ];

    for (cat, label) in &order {
        if let Some(cmds) = by_category.get(cat) {
            parts.push(format!("*{label}*"));
            for cmd in cmds {
                let aliases = if cmd.aliases.is_empty() {
                    String::new()
                } else {
                    format!(
                        " ({})",
                        cmd.aliases
                            .iter()
                            .map(|a| format!("/{a}"))
                            .collect::<Vec<_>>()
                            .join(", ")
                    )
                };
                parts.push(format!("  `/{}`{} — {}", cmd.name, aliases, cmd.description));
            }
            parts.push(String::new());
        }
    }

    parts.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detect_known_command() {
        let cmd = detect_command("/help").unwrap();
        assert_eq!(cmd.name, "help");
        assert_eq!(cmd.args, "");
    }

    #[test]
    fn detect_command_with_args() {
        let cmd = detect_command("/model gpt-4o").unwrap();
        assert_eq!(cmd.name, "model");
        assert_eq!(cmd.args, "gpt-4o");
    }

    #[test]
    fn detect_alias() {
        let cmd = detect_command("/h").unwrap();
        assert_eq!(cmd.name, "help");

        let cmd = detect_command("/t high").unwrap();
        assert_eq!(cmd.name, "think");
        assert_eq!(cmd.args, "high");

        let cmd = detect_command("/new").unwrap();
        assert_eq!(cmd.name, "reset");
    }

    #[test]
    fn detect_command_case_insensitive() {
        let cmd = detect_command("/HELP").unwrap();
        assert_eq!(cmd.name, "help");
    }

    #[test]
    fn detect_command_with_bot_suffix() {
        let cmd = detect_command("/help@mybot").unwrap();
        assert_eq!(cmd.name, "help");
    }

    #[test]
    fn detect_unknown_command_returns_none() {
        assert!(detect_command("/unknown_cmd").is_none());
    }

    #[test]
    fn detect_non_command_returns_none() {
        assert!(detect_command("hello world").is_none());
        assert!(detect_command("").is_none());
    }

    #[test]
    fn detect_command_whitespace_trimming() {
        let cmd = detect_command("  /status  ").unwrap();
        assert_eq!(cmd.name, "status");
    }

    #[test]
    fn extract_think_directive() {
        let d = extract_directives("Hello /think high please help");
        assert_eq!(d.think.as_deref(), Some("high"));
        assert!(!d.cleaned_body.contains("/think"));
        assert!(d.cleaned_body.contains("Hello"));
        assert!(d.cleaned_body.contains("help"));
    }

    #[test]
    fn extract_model_directive() {
        let d = extract_directives("/model gpt-4o what is rust?");
        assert_eq!(d.model.as_deref(), Some("gpt-4o"));
        assert!(d.cleaned_body.contains("what is rust?"));
    }

    #[test]
    fn extract_no_directives() {
        let d = extract_directives("Just a normal message");
        assert!(d.think.is_none());
        assert!(d.model.is_none());
        assert_eq!(d.cleaned_body, "Just a normal message");
    }

    #[test]
    fn extract_invalid_think_level_ignored() {
        let d = extract_directives("/think banana");
        assert!(d.think.is_none());
        assert!(d.cleaned_body.contains("/think banana"));
    }

    #[test]
    fn help_text_includes_all_commands() {
        let help = help_text();
        assert!(help.contains("/help"));
        assert!(help.contains("/status"));
        assert!(help.contains("/reset"));
        assert!(help.contains("/model"));
        assert!(help.contains("/think"));
        assert!(help.contains("/compact"));
    }

    #[test]
    fn help_text_shows_aliases() {
        let help = help_text();
        assert!(help.contains("/h"));
        assert!(help.contains("/t"));
        assert!(help.contains("/new"));
    }

    #[test]
    fn command_result_helpers() {
        let handled = CommandResult::handled("ok");
        assert_eq!(handled.reply.as_deref(), Some("ok"));
        assert!(!handled.continue_to_model);

        let silent = CommandResult::handled_silent();
        assert!(silent.reply.is_none());
        assert!(!silent.continue_to_model);

        let pass = CommandResult::pass();
        assert!(pass.reply.is_none());
        assert!(pass.continue_to_model);
    }
}
