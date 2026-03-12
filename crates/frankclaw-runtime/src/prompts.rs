//! Centralized prompt templates.
//!
//! All LLM-facing text lives in `prompts/*.md` files alongside this crate.
//! Templates are embedded at compile time via `include_str!` — no runtime I/O.
//! Placeholders use `{name}` syntax and are replaced by the `render` helper.
//!
//! To audit or update prompts, edit the files in `crates/frankclaw-runtime/prompts/`.

// -- Agent system prompt sections --

pub const AGENT_IDENTITY: &str = include_str!("../prompts/agent_identity.md");
pub const AGENT_TOOLS: &str = include_str!("../prompts/agent_tools.md");
pub const AGENT_SAFETY: &str = include_str!("../prompts/agent_safety.md");
pub const AGENT_CONTEXT: &str = include_str!("../prompts/agent_context.md");

// -- Subagent prompt sections --

pub const SUBAGENT_IDENTITY: &str = include_str!("../prompts/subagent_identity.md");
pub const SUBAGENT_TIMEOUT: &str = include_str!("../prompts/subagent_timeout.md");
pub const SUBAGENT_CAN_SPAWN: &str = include_str!("../prompts/subagent_can_spawn.md");
pub const SUBAGENT_MAX_DEPTH: &str = include_str!("../prompts/subagent_max_depth.md");

// -- Context engine --

pub const CONTEXT_COMPACTION: &str = include_str!("../prompts/context_compaction.md");

/// Replace `{key}` placeholders in a template with provided values.
///
/// ```
/// use frankclaw_runtime::prompts::render;
/// let result = render("Hello {name}, you are {role}.", &[
///     ("name", "Alice"),
///     ("role", "an assistant"),
/// ]);
/// assert_eq!(result, "Hello Alice, you are an assistant.");
/// ```
pub fn render(template: &str, vars: &[(&str, &str)]) -> String {
    let mut result = template.to_string();
    for (key, value) in vars {
        result = result.replace(&format!("{{{key}}}"), value);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_replaces_placeholders() {
        let result = render("Hello {name}, welcome to {place}.", &[
            ("name", "Bob"),
            ("place", "FrankClaw"),
        ]);
        assert_eq!(result, "Hello Bob, welcome to FrankClaw.");
    }

    #[test]
    fn render_leaves_unknown_placeholders_intact() {
        let result = render("Hello {name}, {unknown} stays.", &[("name", "Alice")]);
        assert_eq!(result, "Hello Alice, {unknown} stays.");
    }

    #[test]
    fn render_handles_empty_vars() {
        let result = render("No placeholders here.", &[]);
        assert_eq!(result, "No placeholders here.");
    }

    #[test]
    fn render_handles_repeated_placeholder() {
        let result = render("{x} and {x} again.", &[("x", "hi")]);
        assert_eq!(result, "hi and hi again.");
    }

    #[test]
    fn agent_identity_template_has_placeholder() {
        assert!(AGENT_IDENTITY.contains("{agent_name}"));
    }

    #[test]
    fn agent_tools_template_has_placeholder() {
        assert!(AGENT_TOOLS.contains("{tool_list}"));
    }

    #[test]
    fn agent_context_template_has_placeholders() {
        assert!(AGENT_CONTEXT.contains("{agent_id}"));
        assert!(AGENT_CONTEXT.contains("{model_id}"));
        assert!(AGENT_CONTEXT.contains("{date}"));
        assert!(AGENT_CONTEXT.contains("{tool_count}"));
    }

    #[test]
    fn subagent_identity_template_has_placeholders() {
        assert!(SUBAGENT_IDENTITY.contains("{depth}"));
        assert!(SUBAGENT_IDENTITY.contains("{max_depth}"));
    }

    #[test]
    fn context_compaction_template_has_placeholder() {
        assert!(CONTEXT_COMPACTION.contains("{pruned_count}"));
    }

    #[test]
    fn all_templates_are_nonempty() {
        for (name, template) in [
            ("AGENT_IDENTITY", AGENT_IDENTITY),
            ("AGENT_TOOLS", AGENT_TOOLS),
            ("AGENT_SAFETY", AGENT_SAFETY),
            ("AGENT_CONTEXT", AGENT_CONTEXT),
            ("SUBAGENT_IDENTITY", SUBAGENT_IDENTITY),
            ("SUBAGENT_TIMEOUT", SUBAGENT_TIMEOUT),
            ("SUBAGENT_CAN_SPAWN", SUBAGENT_CAN_SPAWN),
            ("SUBAGENT_MAX_DEPTH", SUBAGENT_MAX_DEPTH),
            ("CONTEXT_COMPACTION", CONTEXT_COMPACTION),
        ] {
            assert!(!template.trim().is_empty(), "{name} should not be empty");
        }
    }
}
