/// Split text into chunks by paragraph boundaries, merging small paragraphs
/// to stay near `target_size` characters per chunk. Tracks line numbers.
pub fn chunk_text(text: &str, target_size: usize) -> Vec<Chunk> {
    let target = if target_size == 0 { 1500 } else { target_size };
    let mut chunks = Vec::new();
    let mut current_text = String::new();
    let mut current_line_start = 1usize;
    let mut chunk_line_start = 1usize;
    let mut chunk_index = 0usize;

    for line in text.lines() {
        let is_blank = line.trim().is_empty();

        // If adding this line would exceed the target and we already have content,
        // and we're at a paragraph boundary, flush.
        if is_blank
            && !current_text.trim().is_empty()
            && current_text.len() >= target
        {
            chunks.push(Chunk {
                text: current_text.trim().to_string(),
                line_start: chunk_line_start,
                line_end: current_line_start.saturating_sub(1).max(chunk_line_start),
                index: chunk_index,
            });
            chunk_index += 1;
            current_text.clear();
            chunk_line_start = current_line_start + 1;
        }

        if current_text.is_empty() && !is_blank {
            chunk_line_start = current_line_start;
        }
        if !current_text.is_empty() || !is_blank {
            if !current_text.is_empty() {
                current_text.push('\n');
            }
            current_text.push_str(line);
        }
        current_line_start += 1;
    }

    // Flush remaining text.
    if !current_text.trim().is_empty() {
        chunks.push(Chunk {
            text: current_text.trim().to_string(),
            line_start: chunk_line_start,
            line_end: current_line_start.saturating_sub(1).max(chunk_line_start),
            index: chunk_index,
        });
    }

    chunks
}

/// A text chunk with position info.
#[derive(Debug, Clone)]
pub struct Chunk {
    pub text: String,
    pub line_start: usize,
    pub line_end: usize,
    pub index: usize,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn single_paragraph() {
        let text = "Hello world, this is a test.";
        let chunks = chunk_text(text, 100);
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].text, "Hello world, this is a test.");
        assert_eq!(chunks[0].line_start, 1);
        assert_eq!(chunks[0].line_end, 1);
        assert_eq!(chunks[0].index, 0);
    }

    #[test]
    fn splits_on_paragraph_boundary() {
        let text = "First paragraph line 1.\nFirst paragraph line 2.\n\nSecond paragraph.\n\nThird paragraph.";
        let chunks = chunk_text(text, 30);
        assert!(chunks.len() >= 2);
        assert!(chunks[0].text.contains("First paragraph"));
        assert!(chunks.last().unwrap().text.contains("paragraph"));
    }

    #[test]
    fn tracks_line_numbers() {
        let text = "Line 1\nLine 2\n\nLine 4\nLine 5";
        let chunks = chunk_text(text, 10);
        assert!(chunks.len() >= 2);
        assert_eq!(chunks[0].line_start, 1);
    }

    #[test]
    fn empty_text() {
        let chunks = chunk_text("", 100);
        assert!(chunks.is_empty());
    }

    #[test]
    fn whitespace_only() {
        let chunks = chunk_text("   \n\n   \n", 100);
        assert!(chunks.is_empty());
    }

    #[test]
    fn small_target_merges() {
        let text = "A\n\nB\n\nC";
        let chunks = chunk_text(text, 1000);
        // With a large target, everything should be in one chunk.
        assert_eq!(chunks.len(), 1);
        assert!(chunks[0].text.contains('A'));
        assert!(chunks[0].text.contains('C'));
    }
}
