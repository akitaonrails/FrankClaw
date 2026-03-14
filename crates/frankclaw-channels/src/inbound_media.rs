use frankclaw_core::media::infer_mime_from_name;

pub fn infer_inbound_mime_type(
    explicit: Option<&str>,
    filename: Option<&str>,
    url: Option<&str>,
) -> String {
    let explicit = explicit
        .map(str::trim)
        .filter(|value| !value.is_empty());
    if let Some(explicit) = explicit {
        return explicit.to_string();
    }

    filename
        .and_then(infer_mime_from_name)
        .map(str::to_string)
        .or_else(|| infer_from_url(url))
        .unwrap_or_else(|| "application/octet-stream".to_string())
}

fn infer_from_url(url: Option<&str>) -> Option<String> {
    let url = url?.trim();
    let path = url
        .split('?')
        .next()
        .unwrap_or(url)
        .rsplit('/')
        .next()
        .unwrap_or(url);
    infer_mime_from_name(path).map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::infer_inbound_mime_type;

    #[test]
    fn prefers_explicit_mime_type_when_present() {
        assert_eq!(
            infer_inbound_mime_type(
                Some("image/custom"),
                Some("photo.png"),
                Some("https://example.test/photo.jpg"),
            ),
            "image/custom"
        );
    }

    #[test]
    fn infers_from_filename_when_provider_omits_type() {
        assert_eq!(
            infer_inbound_mime_type(None, Some("voice-note.m4a"), None),
            "audio/mp4"
        );
    }

    #[test]
    fn infers_from_url_when_filename_is_missing() {
        assert_eq!(
            infer_inbound_mime_type(None, None, Some("https://cdn.example.test/path/report.pdf?sig=1")),
            "application/pdf"
        );
    }
}
