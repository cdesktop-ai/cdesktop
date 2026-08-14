use regex::Regex;
use uuid::Uuid;

pub fn git_branch_id(input: &str) -> String {
    // 1. lowercase
    let lower = input.to_lowercase();

    // 2. replace non-alphanumerics with hyphens
    let re = Regex::new(r"[^a-z0-9]+").unwrap();
    let slug = re.replace_all(&lower, "-");

    // 3. trim extra hyphens
    let trimmed = slug.trim_matches('-');

    // 4. take up to 16 chars, then trim trailing hyphens again
    let cut: String = trimmed.chars().take(16).collect();
    cut.trim_end_matches('-').to_string()
}

pub fn short_uuid(u: &Uuid) -> String {
    // to_simple() gives you a 32-char hex string with no hyphens
    let full = u.simple().to_string();
    full.chars().take(4).collect() // grab the first 4 chars
}

pub fn truncate_to_char_boundary(content: &str, max_len: usize) -> &str {
    if content.len() <= max_len {
        return content;
    }

    let cutoff = content
        .char_indices()
        .map(|(idx, _)| idx)
        .chain(std::iter::once(content.len()))
        .take_while(|&idx| idx <= max_len)
        .last()
        .unwrap_or(0);

    debug_assert!(content.is_char_boundary(cutoff));
    &content[..cutoff]
}

/// Decode UTF-8 data that arrives in arbitrary byte chunks.
///
/// A stream reader is allowed to split a multi-byte UTF-8 character between
/// chunks. Decoding each chunk independently with `from_utf8_lossy` would
/// replace those incomplete sequences with `�`. Keep incomplete bytes until
/// the next chunk so text such as Cyrillic and emoji survives streaming.
#[derive(Default)]
pub struct Utf8ChunkDecoder {
    pending: Vec<u8>,
}

impl Utf8ChunkDecoder {
    pub fn decode(&mut self, bytes: &[u8]) -> String {
        self.pending.extend_from_slice(bytes);
        let mut decoded = String::new();

        loop {
            match std::str::from_utf8(&self.pending) {
                Ok(text) => {
                    decoded.push_str(text);
                    self.pending.clear();
                    break;
                }
                Err(error) => {
                    let valid_len = error.valid_up_to();
                    if valid_len > 0 {
                        decoded.push_str(
                            std::str::from_utf8(&self.pending[..valid_len])
                                .expect("valid UTF-8 prefix was reported as valid"),
                        );
                        self.pending.drain(..valid_len);
                    }

                    if let Some(error_len) = error.error_len() {
                        decoded.push('\u{FFFD}');
                        self.pending.drain(..error_len);
                    } else {
                        // The remaining bytes are a valid but incomplete
                        // character; wait for the next stream chunk.
                        break;
                    }
                }
            }
        }

        decoded
    }

    /// Flush an incomplete final sequence when the byte stream ends.
    pub fn finish(&mut self) -> String {
        let mut decoded = self.decode(&[]);
        if !self.pending.is_empty() {
            decoded.push('\u{FFFD}');
            self.pending.clear();
        }
        decoded
    }
}

#[cfg(test)]
mod tests {

    #[test]
    fn test_truncate_to_char_boundary() {
        use super::truncate_to_char_boundary;

        let input = "a".repeat(10);
        assert_eq!(truncate_to_char_boundary(&input, 7), "a".repeat(7));

        let input = "hello world";
        assert_eq!(truncate_to_char_boundary(input, input.len()), input);

        let input = "🔥🔥🔥"; // each fire emoji is 4 bytes
        assert_eq!(truncate_to_char_boundary(input, 5), "🔥");
        assert_eq!(truncate_to_char_boundary(input, 3), "");
    }

    #[test]
    fn utf8_chunk_decoder_preserves_text_split_at_every_byte() {
        use super::Utf8ChunkDecoder;

        let input = "русский текст и действия 🚀";
        let bytes = input.as_bytes();

        for split in 0..=bytes.len() {
            let mut decoder = Utf8ChunkDecoder::default();
            let mut decoded = decoder.decode(&bytes[..split]);
            decoded.push_str(&decoder.decode(&bytes[split..]));
            decoded.push_str(&decoder.finish());

            assert_eq!(decoded, input, "failed at byte split {split}");
        }
    }

    #[test]
    fn utf8_chunk_decoder_replaces_invalid_bytes_without_panicking() {
        use super::Utf8ChunkDecoder;

        let mut decoder = Utf8ChunkDecoder::default();

        assert_eq!(decoder.decode(&[b'a', 0xf0]), "a");
        assert_eq!(decoder.decode(&[0x28, b'b']), "�(b");
        assert_eq!(decoder.finish(), "");
    }
}
