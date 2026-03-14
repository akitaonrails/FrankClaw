#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SseEvent {
    pub(crate) event: Option<String>,
    pub(crate) data: String,
}

#[derive(Debug, Default)]
pub struct SseDecoder {
    buffer: String,
    event: Option<String>,
    data_lines: Vec<String>,
}

impl SseDecoder {
    pub(crate) fn push(&mut self, chunk: &[u8]) -> Vec<SseEvent> {
        self.buffer.push_str(&String::from_utf8_lossy(chunk));
        let mut events = Vec::new();

        while let Some(newline_index) = self.buffer.find('\n') {
            let mut line = self.buffer[..newline_index].to_string();
            self.buffer.drain(..=newline_index);
            if line.ends_with('\r') {
                line.pop();
            }

            if line.is_empty() {
                if let Some(event) = self.flush_event() {
                    events.push(event);
                }
                continue;
            }

            if let Some(rest) = line.strip_prefix(':') {
                let _ = rest;
                continue;
            }

            let (field, value) = match line.split_once(':') {
                Some((field, value)) => (field, value.strip_prefix(' ').unwrap_or(value)),
                None => (line.as_str(), ""),
            };

            match field {
                "event" => self.event = Some(value.to_string()),
                "data" => self.data_lines.push(value.to_string()),
                _ => {}
            }
        }

        events
    }

    pub(crate) fn finish(&mut self) -> Option<SseEvent> {
        if !self.buffer.trim().is_empty() {
            let remainder = std::mem::take(&mut self.buffer);
            let mut line = remainder;
            if line.ends_with('\r') {
                line.pop();
            }
            let (field, value) = match line.split_once(':') {
                Some((field, value)) => (field, value.strip_prefix(' ').unwrap_or(value)),
                None => (line.as_str(), ""),
            };
            match field {
                "event" => self.event = Some(value.to_string()),
                "data" => self.data_lines.push(value.to_string()),
                _ => {}
            }
        }

        self.flush_event()
    }

    fn flush_event(&mut self) -> Option<SseEvent> {
        if self.event.is_none() && self.data_lines.is_empty() {
            return None;
        }

        Some(SseEvent {
            event: self.event.take(),
            data: self.data_lines.drain(..).collect::<Vec<_>>().join("\n"),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decoder_handles_chunked_event_boundaries() {
        let mut decoder = SseDecoder::default();
        let mut events = decoder.push(b"event: ping\ndata: hel");
        assert!(events.is_empty());

        events.extend(decoder.push(b"lo\n\ndata: world\n\n"));

        assert_eq!(
            events,
            vec![
                SseEvent {
                    event: Some("ping".into()),
                    data: "hello".into(),
                },
                SseEvent {
                    event: None,
                    data: "world".into(),
                }
            ]
        );
    }

    #[test]
    fn decoder_joins_multiline_data() {
        let mut decoder = SseDecoder::default();
        let events = decoder.push(b"data: one\ndata: two\n\n");

        assert_eq!(
            events,
            vec![SseEvent {
                event: None,
                data: "one\ntwo".into(),
            }]
        );
    }
}
