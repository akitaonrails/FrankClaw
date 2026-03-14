//! Context engine: manages conversation context within model token budgets.
//!
//! Handles token estimation, message pruning, and context compaction so that
//! long conversations don't exceed model context windows.

use frankclaw_core::model::{CompletionMessage, ModelDef};
use frankclaw_core::types::Role;

/// Safety margin multiplier — we target (budget / SAFETY_MARGIN) to leave
/// headroom for the model's response and system prompt overhead.
const SAFETY_MARGIN: f64 = 1.2;

/// Minimum number of recent messages to always keep, even if over budget.
/// Prevents the context from being pruned to nothing.
const MIN_KEEP_MESSAGES: usize = 4;

/// Overhead tokens reserved for system prompt framing, tool definitions, etc.
const SYSTEM_OVERHEAD_TOKENS: u32 = 2048;

/// Approximate chars-per-token ratio for estimation.
/// Conservative (real ratio is ~4 for English, lower for CJK/code).
const CHARS_PER_TOKEN: f64 = 3.5;

// Compaction template is loaded from prompts/context_compaction.md

/// Result of context optimization.
#[derive(Debug, Clone)]
pub struct ContextWindow {
    /// Messages to send to the model (pruned/compacted to fit budget).
    pub messages: Vec<CompletionMessage>,
    /// Estimated input tokens for the optimized context.
    pub estimated_tokens: u32,
    /// Number of messages pruned from history.
    pub pruned_count: usize,
    /// Whether compaction summary was inserted.
    pub compacted: bool,
}

/// Estimate the token count for a string using a chars/token heuristic.
pub fn estimate_tokens(text: &str) -> u32 {
    let chars = text.len() as f64;
    (chars / CHARS_PER_TOKEN).ceil() as u32
}

/// Estimate total tokens for a slice of messages.
pub fn estimate_messages_tokens(messages: &[CompletionMessage]) -> u32 {
    messages
        .iter()
        .map(|msg| {
            // Per-message overhead: role marker + framing (~4 tokens per message)
            estimate_tokens(&msg.content) + 4
        })
        .sum()
}

/// Calculate the available token budget for input context given a model definition.
pub fn available_input_budget(model: &ModelDef, system_prompt: Option<&str>) -> u32 {
    let total = model.context_window;
    let reserved_output = model.max_output_tokens;
    let system_tokens = system_prompt.map(|s| estimate_tokens(s)).unwrap_or(0);
    let overhead = SYSTEM_OVERHEAD_TOKENS + system_tokens;

    let raw_budget = total.saturating_sub(reserved_output).saturating_sub(overhead);
    // Apply safety margin
    (raw_budget as f64 / SAFETY_MARGIN) as u32
}

/// Optimize a message list to fit within the model's context window.
///
/// Strategy:
/// 1. If messages fit within budget, return as-is.
/// 2. Otherwise, drop oldest messages (preserving tool_use/tool_result pairs).
/// 3. Insert a "[Previous conversation summary]" marker noting what was pruned.
///
/// This is a "sliding window with summary marker" approach — simpler than
/// LLM-based summarization but effective and zero-latency.
pub fn optimize_context(
    messages: Vec<CompletionMessage>,
    model: &ModelDef,
    system_prompt: Option<&str>,
) -> ContextWindow {
    let budget = available_input_budget(model, system_prompt);

    // Always repair orphaned tool messages before sending to the API.
    // This prevents errors like "messages with role 'tool' must be a response
    // to a preceding message with 'tool_calls'" from OpenAI.
    let mut messages = messages;
    repair_tool_pairing(&mut messages);
    // Merge consecutive same-role messages to satisfy providers requiring
    // strict alternation (Anthropic, Gemini).
    merge_consecutive_same_role(&mut messages);

    let total_tokens = estimate_messages_tokens(&messages);

    if total_tokens <= budget {
        return ContextWindow {
            estimated_tokens: total_tokens,
            pruned_count: 0,
            compacted: false,
            messages,
        };
    }

    // Need to prune. Drop oldest messages while preserving integrity.
    let mut kept = messages;
    let mut pruned_count = 0;

    while estimate_messages_tokens(&kept) > budget && kept.len() > MIN_KEEP_MESSAGES {
        // Remove from the front (oldest messages).
        kept.remove(0);
        pruned_count += 1;

        // Repair orphaned tool results: if the first message is now a Tool
        // response without its preceding Assistant tool_call, remove it too.
        while !kept.is_empty() && kept[0].role == Role::Tool {
            kept.remove(0);
            pruned_count += 1;
        }
    }

    // Insert a summary marker at the beginning so the model knows context was pruned.
    let summary = crate::prompts::render(crate::prompts::CONTEXT_COMPACTION, &[
        ("pruned_count", &pruned_count.to_string()),
    ]);

    kept.insert(
        0,
        CompletionMessage::text(Role::User, summary),
    );

    let estimated_tokens = estimate_messages_tokens(&kept);

    ContextWindow {
        messages: kept,
        estimated_tokens,
        pruned_count,
        compacted: pruned_count > 0,
    }
}

/// Repair tool_use / tool_result pairing after message pruning.
///
/// If an Assistant message contains `[tool_call:...]` markers but the
/// corresponding Tool result was pruned (or vice versa), remove the
/// orphaned entry to prevent API errors.
pub fn repair_tool_pairing(messages: &mut Vec<CompletionMessage>) {
    // Full scan: remove any Tool message not immediately preceded by an Assistant
    // message with tool_calls (structured) or a text marker, or another valid Tool.
    let mut i = 0;
    while i < messages.len() {
        if messages[i].role == Role::Tool {
            let has_preceding_tool_call = i > 0
                && messages[i - 1].role == Role::Assistant
                && (
                    !messages[i - 1].tool_calls.is_empty()
                    || messages[i - 1].content.contains("[tool_call:")
                );
            // Also allow consecutive Tool messages after one that had a valid predecessor.
            let follows_valid_tool = i > 0 && messages[i - 1].role == Role::Tool;
            if !has_preceding_tool_call && !follows_valid_tool {
                messages.remove(i);
                continue;
            }
        }
        i += 1;
    }

    // Remove trailing Assistant messages that have tool_calls but no following results.
    while messages.last().is_some_and(|m| {
        m.role == Role::Assistant
            && (!m.tool_calls.is_empty() || m.content.contains("[tool_call:"))
    }) {
        messages.pop();
    }
}

/// Merge consecutive messages with the same role to satisfy providers
/// that require strict alternation (Anthropic, Gemini).
pub fn merge_consecutive_same_role(messages: &mut Vec<CompletionMessage>) {
    let mut i = 1;
    while i < messages.len() {
        if messages[i].role == messages[i - 1].role
            && messages[i].role != Role::Tool
            && messages[i - 1].tool_calls.is_empty()
            && messages[i].tool_calls.is_empty()
        {
            let content = messages.remove(i).content;
            messages[i - 1].content.push('\n');
            messages[i - 1].content.push_str(&content);
        } else {
            i += 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn msg(role: Role, content: &str) -> CompletionMessage {
        CompletionMessage::text(role, content)
    }

    fn test_model(context_window: u32) -> ModelDef {
        ModelDef {
            id: "test-model".into(),
            name: "Test Model".into(),
            api: frankclaw_core::model::ModelApi::OpenaiCompletions,
            reasoning: false,
            input: vec![frankclaw_core::model::InputModality::Text],
            cost: Default::default(),
            context_window,
            max_output_tokens: 4096,
            compat: frankclaw_core::model::ModelCompat {
                supports_tools: true,
                supports_vision: false,
                supports_streaming: true,
                supports_json_mode: false,
                supports_system_message: true,
            },
        }
    }

    #[test]
    fn estimate_tokens_basic() {
        // ~3.5 chars per token
        assert!(estimate_tokens("hello") > 0);
        assert!(estimate_tokens("hello world, this is a test") > 5);
        // Empty string = 0 tokens
        assert_eq!(estimate_tokens(""), 0);
    }

    #[test]
    fn estimate_tokens_proportional() {
        let short = estimate_tokens("hello");
        let long = estimate_tokens(&"x".repeat(1000));
        assert!(long > short * 10);
    }

    #[test]
    fn available_budget_respects_output_reservation() {
        let model = test_model(100_000);
        let budget = available_input_budget(&model, None);
        // Budget should be less than context_window - max_output_tokens
        assert!(budget < 100_000 - 4096);
        // But still a reasonable fraction
        assert!(budget > 50_000);
    }

    #[test]
    fn available_budget_accounts_for_system_prompt() {
        let model = test_model(100_000);
        let no_system = available_input_budget(&model, None);
        let with_system =
            available_input_budget(&model, Some(&"x".repeat(10_000)));
        assert!(with_system < no_system);
    }

    #[test]
    fn optimize_context_passthrough_when_within_budget() {
        let model = test_model(200_000);
        let messages = vec![
            msg(Role::User, "hello"),
            msg(Role::Assistant, "hi there"),
            msg(Role::User, "how are you"),
        ];

        let result = optimize_context(messages.clone(), &model, None);
        assert_eq!(result.pruned_count, 0);
        assert!(!result.compacted);
        assert_eq!(result.messages.len(), 3);
    }

    #[test]
    fn optimize_context_prunes_old_messages() {
        // Tiny context window to force pruning
        let model = test_model(500);
        let messages: Vec<_> = (0..50)
            .map(|i| {
                msg(
                    if i % 2 == 0 { Role::User } else { Role::Assistant },
                    &format!("message number {} with some padding text to use tokens", i),
                )
            })
            .collect();

        let result = optimize_context(messages, &model, None);
        assert!(result.pruned_count > 0);
        assert!(result.compacted);
        // Should have a summary marker as first message
        assert!(result.messages[0]
            .content
            .contains("Previous conversation summary"));
        // Should still have some recent messages
        assert!(result.messages.len() > MIN_KEEP_MESSAGES);
    }

    #[test]
    fn optimize_context_removes_orphaned_tool_results() {
        let model = test_model(400);
        let messages = vec![
            msg(Role::User, "old message 1"),
            msg(Role::Assistant, "old response 1"),
            msg(Role::User, "old message 2"),
            msg(
                Role::Assistant,
                "[tool_call:search {\"q\": \"test\"}]",
            ),
            msg(Role::Tool, "{\"result\": \"found\"}"),
            msg(Role::User, "recent message with enough tokens to force pruning and this needs to be quite long to actually trigger the context window limit"),
            msg(Role::Assistant, "recent response also needs to be fairly long to contribute to the token count significantly"),
        ];

        let result = optimize_context(messages, &model, None);
        // After pruning, no Tool message should be first (orphaned)
        for (i, m) in result.messages.iter().enumerate() {
            if i == 0 {
                // First should be summary marker (User role)
                continue;
            }
            if m.role == Role::Tool {
                // Tool messages should follow an Assistant message
                assert!(
                    i > 0 && result.messages[i - 1].role == Role::Assistant,
                    "orphaned tool result at index {}",
                    i
                );
            }
        }
    }

    #[test]
    fn repair_tool_pairing_removes_leading_tool_messages() {
        let mut messages = vec![
            msg(Role::Tool, "{\"result\": \"orphaned\"}"),
            msg(Role::Tool, "{\"result\": \"also orphaned\"}"),
            msg(Role::User, "hello"),
            msg(Role::Assistant, "hi"),
        ];

        repair_tool_pairing(&mut messages);
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0].role, Role::User);
    }

    #[test]
    fn repair_tool_pairing_removes_trailing_tool_calls() {
        let mut messages = vec![
            msg(Role::User, "hello"),
            msg(Role::Assistant, "let me search [tool_call:search {}]"),
        ];

        repair_tool_pairing(&mut messages);
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].role, Role::User);
    }

    #[test]
    fn repair_tool_pairing_removes_mid_conversation_orphans() {
        let mut messages = vec![
            msg(Role::User, "hello"),
            msg(Role::Assistant, "hi there"),
            msg(Role::Tool, "{\"result\": \"orphaned mid-conversation\"}"),
            msg(Role::User, "what about this?"),
            msg(Role::Assistant, "sure"),
        ];

        repair_tool_pairing(&mut messages);
        assert_eq!(messages.len(), 4);
        assert_eq!(messages[0].role, Role::User);
        assert_eq!(messages[1].role, Role::Assistant);
        assert_eq!(messages[2].role, Role::User);
        assert_eq!(messages[3].role, Role::Assistant);
    }

    #[test]
    fn repair_tool_pairing_keeps_valid_tool_pairs() {
        let mut messages = vec![
            msg(Role::User, "search for cats"),
            msg(Role::Assistant, "searching [tool_call:search {\"q\": \"cats\"}]"),
            msg(Role::Tool, "{\"results\": [\"cat1\", \"cat2\"]}"),
            msg(Role::User, "thanks"),
        ];

        repair_tool_pairing(&mut messages);
        assert_eq!(messages.len(), 4);
        assert_eq!(messages[2].role, Role::Tool);
    }

    #[test]
    fn repair_tool_pairing_keeps_structured_tool_calls() {
        use frankclaw_core::model::ToolCallResponse;
        let mut messages = vec![
            msg(Role::User, "search for cats"),
            CompletionMessage::assistant_tool_calls(
                "searching",
                vec![ToolCallResponse {
                    id: "call-1".into(),
                    name: "search".into(),
                    arguments: r#"{"q":"cats"}"#.into(),
                }],
            ),
            CompletionMessage::tool_result("call-1", r#"{"results": ["cat1"]}"#),
            msg(Role::User, "thanks"),
        ];

        repair_tool_pairing(&mut messages);
        assert_eq!(messages.len(), 4);
        assert_eq!(messages[2].role, Role::Tool);
    }

    #[test]
    fn repair_tool_pairing_removes_trailing_structured_tool_calls() {
        use frankclaw_core::model::ToolCallResponse;
        let mut messages = vec![
            msg(Role::User, "hello"),
            CompletionMessage::assistant_tool_calls(
                "let me search",
                vec![ToolCallResponse {
                    id: "call-1".into(),
                    name: "search".into(),
                    arguments: "{}".into(),
                }],
            ),
        ];

        repair_tool_pairing(&mut messages);
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].role, Role::User);
    }

    #[test]
    fn merge_consecutive_same_role_merges_users() {
        let mut messages = vec![
            msg(Role::User, "hello"),
            msg(Role::User, "world"),
            msg(Role::Assistant, "hi"),
        ];

        merge_consecutive_same_role(&mut messages);
        assert_eq!(messages.len(), 2);
        assert!(messages[0].content.contains("hello"));
        assert!(messages[0].content.contains("world"));
    }

    #[test]
    fn merge_consecutive_same_role_preserves_tool_messages() {
        use frankclaw_core::model::ToolCallResponse;
        let mut messages = vec![
            msg(Role::User, "search"),
            CompletionMessage::assistant_tool_calls(
                "searching",
                vec![ToolCallResponse {
                    id: "call-1".into(),
                    name: "search".into(),
                    arguments: "{}".into(),
                }],
            ),
            CompletionMessage::tool_result("call-1", "result"),
            msg(Role::Assistant, "here are the results"),
        ];

        merge_consecutive_same_role(&mut messages);
        // Should NOT merge the assistant_tool_calls with the next assistant message
        assert_eq!(messages.len(), 4);
    }

    #[test]
    fn optimize_context_strips_orphaned_tools_even_within_budget() {
        let model = test_model(100_000);
        let messages = vec![
            msg(Role::User, "hello"),
            msg(Role::Assistant, "hi"),
            msg(Role::Tool, "{\"orphaned\": true}"),
            msg(Role::User, "test"),
        ];

        let result = optimize_context(messages, &model, None);
        assert!(!result.compacted);
        // Orphaned tool message should be removed even without pruning.
        for m in &result.messages {
            assert_ne!(m.role, Role::Tool);
        }
        assert_eq!(result.messages.len(), 3);
    }

    #[test]
    fn optimize_context_preserves_min_messages() {
        // Even with a tiny budget, should keep MIN_KEEP_MESSAGES
        let model = test_model(100);
        let messages: Vec<_> = (0..20)
            .map(|i| {
                msg(
                    if i % 2 == 0 { Role::User } else { Role::Assistant },
                    &"x".repeat(100),
                )
            })
            .collect();

        let result = optimize_context(messages, &model, None);
        // +1 for the summary marker
        assert!(result.messages.len() >= MIN_KEEP_MESSAGES + 1);
    }

    #[test]
    fn estimate_messages_tokens_includes_per_message_overhead() {
        let one = estimate_messages_tokens(&[msg(Role::User, "hello")]);
        let content_only = estimate_tokens("hello");
        // Should be content + ~4 overhead tokens
        assert!(one > content_only);
        assert!(one <= content_only + 5);
    }
}
