use frankclaw_core::channel::InboundAttachment;

pub(crate) fn text_or_attachment_placeholder(
    text: Option<&str>,
    attachments: &[InboundAttachment],
) -> Option<String> {
    let text = text
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string);
    if text.is_some() {
        return text;
    }

    if attachments.is_empty() {
        None
    } else {
        Some(attachment_placeholder(attachments))
    }
}

pub(crate) fn text_quote_or_attachment_placeholder(
    text: Option<&str>,
    quote: Option<&str>,
    attachments: &[InboundAttachment],
) -> Option<String> {
    text_or_attachment_placeholder(text, attachments).or_else(|| {
        quote
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
    })
}

pub(crate) fn attachment_placeholder(attachments: &[InboundAttachment]) -> String {
    if attachments.len() > 1 {
        return "<media:attachments>".into();
    }

    let mime = attachments
        .first()
        .map(|attachment| attachment.mime_type.as_str())
        .unwrap_or("application/octet-stream");
    if mime.starts_with("image/") {
        "<media:image>".into()
    } else if mime.starts_with("audio/") {
        "<media:audio>".into()
    } else if mime.starts_with("video/") {
        "<media:video>".into()
    } else {
        "<media:attachment>".into()
    }
}

#[cfg(test)]
mod tests {
    use frankclaw_core::channel::InboundAttachment;

    use super::*;

    #[test]
    fn text_or_attachment_placeholder_prefers_non_empty_text() {
        let attachments = vec![InboundAttachment {
            media_id: None,
            mime_type: "image/jpeg".into(),
            filename: Some("photo.jpg".into()),
            size_bytes: Some(42),
            url: None,
        }];

        assert_eq!(
            text_or_attachment_placeholder(Some(" hello "), &attachments).as_deref(),
            Some("hello")
        );
    }

    #[test]
    fn text_or_attachment_placeholder_falls_back_to_media_marker() {
        let attachments = vec![InboundAttachment {
            media_id: None,
            mime_type: "audio/ogg".into(),
            filename: Some("voice.ogg".into()),
            size_bytes: Some(42),
            url: None,
        }];

        assert_eq!(
            text_or_attachment_placeholder(None, &attachments).as_deref(),
            Some("<media:audio>")
        );
    }
}
