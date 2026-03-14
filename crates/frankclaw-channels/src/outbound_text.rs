#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutboundTextFlavor {
    Plain,
    WhatsApp,
}

pub fn normalize_outbound_text(
    text: &str,
    flavor: OutboundTextFlavor,
) -> String {
    let normalized = strip_reasoning_prefix(text);
    let normalized = normalized.trim().replace("\r\n", "\n");

    match flavor {
        OutboundTextFlavor::Plain => normalized,
        OutboundTextFlavor::WhatsApp => normalize_whatsapp_text(&normalized),
    }
}

fn strip_reasoning_prefix(text: &str) -> String {
    let trimmed = text.trim_start();
    let Some(rest) = trimmed.strip_prefix("Reasoning:") else {
        return text.to_string();
    };

    let rest = rest.trim_start();
    if rest.is_empty() {
        return text.to_string();
    }

    let lines = rest.lines().collect::<Vec<_>>();
    let split_index = lines
        .windows(2)
        .position(|window| {
            assert!(window.len() > 1, "windows(2) always yields exactly 2 elements");
            window[0].trim().is_empty() && !window[1].trim().is_empty()
        })
        .map(|index| index + 1);

    match split_index {
        Some(index) if index < lines.len() => lines[index..].join("\n"),
        _ => text.to_string(),
    }
}

fn normalize_whatsapp_text(text: &str) -> String {
    let chars = text.chars().collect::<Vec<_>>();
    let mut out = String::with_capacity(text.len());
    let mut index = 0usize;

    while index < chars.len() {
        if chars[index] == '*' && chars.get(index + 1) == Some(&'*') {
            out.push('*');
            index += 2;
            continue;
        }

        if chars[index] == '~' && chars.get(index + 1) == Some(&'~') {
            out.push('~');
            index += 2;
            continue;
        }

        out.push(chars[index]);
        index += 1;
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_outbound_text_trims_and_normalizes_newlines() {
        assert_eq!(
            normalize_outbound_text("\n\r\n hello\r\n", OutboundTextFlavor::Plain),
            "hello"
        );
    }

    #[test]
    fn normalize_outbound_text_drops_reasoning_prefix_when_followed_by_final_content() {
        let text = "Reasoning:\n- hidden thoughts\n\nFinal answer";
        assert_eq!(
            normalize_outbound_text(text, OutboundTextFlavor::Plain),
            "Final answer"
        );
    }

    #[test]
    fn normalize_outbound_text_keeps_reasoning_label_when_no_final_content_exists() {
        let text = "Reasoning: think harder";
        assert_eq!(
            normalize_outbound_text(text, OutboundTextFlavor::Plain),
            "Reasoning: think harder"
        );
    }

    #[test]
    fn normalize_outbound_text_converts_basic_markdown_for_whatsapp() {
        let text = "**bold** and ~~strike~~";
        assert_eq!(
            normalize_outbound_text(text, OutboundTextFlavor::WhatsApp),
            "*bold* and ~strike~"
        );
    }
}
