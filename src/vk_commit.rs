//! Candidate commit + the Text-commit seam.
//!
//! See CONTEXT.md ("Candidate commit", "Text commit"). `commit` deletes the
//! partial prefix and inserts the chosen word as one atomic Text-commit
//! *replace*. The real adapter drives `SendInput` (`vk_nav::SendInputSink`);
//! `BufSink` records the resulting text so the commit decision is testable
//! without Win32 or a live foreground field. Two adapters make the seam real.

use std::io;

/// The Text-commit seam: a delete-then-insert applied as one atomic injection.
/// `del` counts **characters** to remove (one backspace each); `ins` is the
/// word injected after.
pub trait TextSink {
    fn replace(&mut self, del: usize, ins: &str) -> io::Result<()>;
}

/// Outcome of a Candidate commit. `injected` is false when the Text-commit
/// adapter reported an error — callers gate dictionary/buffer writes on it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Committed {
    pub word: String,
    pub deleted: usize,
    pub injected: bool,
}

/// Replace `del` characters of partial prefix with `word` through `sink`.
pub fn commit(word: &str, del: usize, sink: &mut dyn TextSink) -> Committed {
    let injected = sink.replace(del, word).is_ok();
    Committed {
        word: word.to_string(),
        deleted: del,
        injected,
    }
}

/// In-memory Text-commit adapter for tests: models the focused field's tail.
#[cfg(test)]
pub struct BufSink {
    pub buf: String,
    pub fail: bool,
}

#[cfg(test)]
impl BufSink {
    pub fn new(seed: &str) -> Self {
        BufSink {
            buf: seed.to_string(),
            fail: false,
        }
    }
    pub fn failing(seed: &str) -> Self {
        BufSink {
            buf: seed.to_string(),
            fail: true,
        }
    }
}

#[cfg(test)]
impl TextSink for BufSink {
    fn replace(&mut self, del: usize, ins: &str) -> io::Result<()> {
        if self.fail {
            return Err(io::Error::other("sink failed"));
        }
        for _ in 0..del {
            self.buf.pop();
        }
        self.buf.push_str(ins);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replace_swaps_prefix_for_word() {
        let mut sink = BufSink::new("keyb");
        let c = commit("keyboard", 4, &mut sink);
        assert!(c.injected);
        assert_eq!(sink.buf, "keyboard");
        assert_eq!(c.deleted, 4);
    }

    #[test]
    fn failed_sink_reports_not_injected() {
        let mut sink = BufSink::failing("keyb");
        let c = commit("keyboard", 4, &mut sink);
        assert!(!c.injected);
        assert_eq!(sink.buf, "keyb"); // untouched
    }
}
