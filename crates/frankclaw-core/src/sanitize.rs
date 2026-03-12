//! Input sanitization for prompt injection prevention.
//!
//! Strips Unicode control characters (Cc, Cf categories) that can be used
//! to manipulate LLM behavior, and wraps untrusted external content in
//! security boundary tags.

use crate::model::CompletionMessage;

/// Maximum total prompt size in bytes (2 MB).
/// Prevents token exhaustion / memory abuse from oversized prompts.
pub const MAX_PROMPT_BYTES: usize = 2 * 1024 * 1024;

/// Strip Unicode control characters that could be used for prompt injection.
///
/// Removes:
/// - Unicode category Cc (control chars) except \t, \n, \r (which are useful)
/// - Unicode category Cf (format chars) like zero-width joiners, bidi overrides,
///   soft hyphens, etc.
///
/// This mirrors OpenClaw's `sanitizeForPromptLiteral()` approach.
pub fn sanitize_for_prompt(input: &str) -> String {
    input
        .chars()
        .filter(|&c| {
            if c == '\t' || c == '\n' || c == '\r' {
                return true;
            }
            // Block Unicode Cc (control) and Cf (format) categories.
            // Cc: U+0000..U+001F, U+007F..U+009F
            // Cf: soft hyphen, bidi marks, zero-width chars, etc.
            !c.is_control() && !is_format_char(c)
        })
        .collect()
}

/// Check if a character is in Unicode category Cf (format characters).
///
/// These include invisible characters that can manipulate text rendering
/// and potentially confuse LLMs:
/// - Zero-width space (U+200B)
/// - Zero-width non-joiner (U+200C)
/// - Zero-width joiner (U+200D)
/// - Left-to-right / right-to-left marks (U+200E, U+200F)
/// - Bidi embedding/override/isolate chars (U+202A..U+202E, U+2066..U+2069)
/// - Word joiner (U+2060)
/// - Soft hyphen (U+00AD)
/// - Object replacement (U+FFFC), replacement character is NOT blocked
/// - Byte order marks (U+FEFF)
/// - Interlinear annotation anchors (U+FFF9..U+FFFB)
fn is_format_char(c: char) -> bool {
    matches!(c,
        '\u{00AD}'          // Soft hyphen
        | '\u{200B}'        // Zero-width space
        | '\u{200C}'        // Zero-width non-joiner
        | '\u{200D}'        // Zero-width joiner
        | '\u{200E}'        // Left-to-right mark
        | '\u{200F}'        // Right-to-left mark
        | '\u{202A}'        // Left-to-right embedding
        | '\u{202B}'        // Right-to-left embedding
        | '\u{202C}'        // Pop directional formatting
        | '\u{202D}'        // Left-to-right override
        | '\u{202E}'        // Right-to-left override
        | '\u{2060}'        // Word joiner
        | '\u{2066}'        // Left-to-right isolate
        | '\u{2067}'        // Right-to-left isolate
        | '\u{2068}'        // First strong isolate
        | '\u{2069}'        // Pop directional isolate
        | '\u{FEFF}'        // Byte order mark / zero-width no-break space
        | '\u{FFF9}'        // Interlinear annotation anchor
        | '\u{FFFA}'        // Interlinear annotation separator
        | '\u{FFFB}'        // Interlinear annotation terminator
        | '\u{FFFC}'        // Object replacement character
    )
}

/// Wrap untrusted user text in security boundary tags.
///
/// This tells the LLM to treat the content as user-provided data,
/// not as instructions. The tags create a semantic boundary that
/// helps prevent prompt injection.
pub fn wrap_untrusted_text(text: &str) -> String {
    format!("<untrusted-text>\n{}\n</untrusted-text>", sanitize_for_prompt(text))
}

/// Wrap external content (fetched URLs, media descriptions, etc.)
/// in security boundary tags.
///
/// External content is even less trusted than user messages —
/// it could contain adversarial payloads injected by third parties.
pub fn wrap_external_content(source: &str, content: &str) -> String {
    let safe = sanitize_for_prompt(content);
    format!(
        "<external-content source=\"{}\">\n{}\n</external-content>",
        sanitize_for_prompt(source),
        safe
    )
}

/// Enforce a hard byte limit on the total prompt size.
///
/// Returns `true` if the prompt is within limits, `false` if it exceeds
/// `MAX_PROMPT_BYTES` and should be rejected.
pub fn check_prompt_size(messages: &[CompletionMessage], system: Option<&str>) -> bool {
    let mut total: usize = 0;
    if let Some(sys) = system {
        total += sys.len();
    }
    for msg in messages {
        total += msg.content.len();
        if total > MAX_PROMPT_BYTES {
            return false;
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_null_bytes() {
        assert_eq!(sanitize_for_prompt("hello\0world"), "helloworld");
    }

    #[test]
    fn strips_control_chars_except_whitespace() {
        let input = "line1\t\x01tab\nline2\x02\rend";
        let result = sanitize_for_prompt(input);
        assert_eq!(result, "line1\ttab\nline2\rend");
    }

    #[test]
    fn strips_zero_width_chars() {
        let input = "hel\u{200B}lo\u{200D}wo\u{FEFF}rld";
        assert_eq!(sanitize_for_prompt(input), "helloworld");
    }

    #[test]
    fn strips_bidi_override_chars() {
        let input = "normal\u{202E}reversed\u{202C}back";
        assert_eq!(sanitize_for_prompt(input), "normalreversedback");
    }

    #[test]
    fn strips_soft_hyphen() {
        let input = "un\u{00AD}break\u{00AD}able";
        assert_eq!(sanitize_for_prompt(input), "unbreakable");
    }

    #[test]
    fn preserves_normal_unicode() {
        let input = "Hello 世界! 🦀 café résumé";
        assert_eq!(sanitize_for_prompt(input), input);
    }

    #[test]
    fn preserves_emoji_and_math_symbols() {
        let input = "price = $42 × 2 = $84 ✓";
        assert_eq!(sanitize_for_prompt(input), input);
    }

    #[test]
    fn wrap_untrusted_applies_sanitize() {
        let input = "inject\u{200B}ion\0attempt";
        let wrapped = wrap_untrusted_text(input);
        assert!(wrapped.starts_with("<untrusted-text>"));
        assert!(wrapped.ends_with("</untrusted-text>"));
        assert!(wrapped.contains("injectionattempt"));
        assert!(!wrapped.contains('\0'));
        assert!(!wrapped.contains('\u{200B}'));
    }

    #[test]
    fn wrap_external_content_includes_source() {
        let content = wrap_external_content("https://example.com", "some\x01data");
        assert!(content.contains("source=\"https://example.com\""));
        assert!(content.contains("<external-content"));
        assert!(content.contains("somedata"));
    }

    #[test]
    fn check_prompt_size_within_limit() {
        let msgs = vec![
            CompletionMessage::text(crate::types::Role::User, "hello"),
        ];
        assert!(check_prompt_size(&msgs, Some("system")));
    }

    #[test]
    fn check_prompt_size_over_limit() {
        let big = "x".repeat(MAX_PROMPT_BYTES + 1);
        let msgs = vec![
            CompletionMessage::text(crate::types::Role::User, big),
        ];
        assert!(!check_prompt_size(&msgs, None));
    }

    #[test]
    fn check_prompt_size_cumulative() {
        let half = "x".repeat(MAX_PROMPT_BYTES / 2 + 1);
        let msgs = vec![
            CompletionMessage::text(crate::types::Role::User, half.clone()),
            CompletionMessage::text(crate::types::Role::Assistant, half),
        ];
        assert!(!check_prompt_size(&msgs, None));
    }

    #[test]
    fn strips_del_character() {
        assert_eq!(sanitize_for_prompt("abc\x7Fdef"), "abcdef");
    }

    #[test]
    fn strips_c1_control_chars() {
        let input = "test\u{0080}\u{008F}\u{009F}end";
        assert_eq!(sanitize_for_prompt(input), "testend");
    }

    #[test]
    fn empty_input_returns_empty() {
        assert_eq!(sanitize_for_prompt(""), "");
        assert_eq!(wrap_untrusted_text(""), "<untrusted-text>\n\n</untrusted-text>");
    }
}
