//! Reading what was captured, on this PC and with no model: text files, and later web pages,
//! PDFs and Word files. A capture's sources share one budget, so a single capture can't cost
//! much.

pub mod docx;
pub mod dom;
pub mod markdown;
pub mod page;
pub mod pdf;
pub mod text;

/// Characters of extracted text one capture may send. Your own text doesn't count.
pub const MAX_CHARS: usize = 60_000;
/// PDF pages one capture may send.
pub const MAX_PAGES: usize = 20;

const CUT_MARK: &str = "\n… cut to fit …";

/// What is left of a capture's allowance, handed out to its sources in capture order, and the
/// names of the sources that didn't fit whole.
#[derive(Debug)]
pub struct Budget {
    chars: usize,
    pages: usize,
    truncated: Vec<String>,
}

impl Default for Budget {
    fn default() -> Self {
        Self::new(MAX_CHARS, MAX_PAGES)
    }
}

impl Budget {
    pub fn new(chars: usize, pages: usize) -> Self {
        Self {
            chars,
            pages,
            truncated: Vec::new(),
        }
    }

    /// `text` as far as the budget allows: whole, cut at a line or word with a mark saying so,
    /// or `None` when nothing is left. A cut or dropped source is recorded under `name`.
    pub fn take(&mut self, name: &str, text: String) -> Option<String> {
        let length = text.chars().count();
        if length <= self.chars {
            self.chars -= length;
            return Some(text);
        }
        self.cut(name);
        let room = self.chars.saturating_sub(CUT_MARK.chars().count());
        if room == 0 {
            self.chars = 0;
            return None;
        }
        let end = text
            .char_indices()
            .nth(room)
            .map_or(text.len(), |(index, _)| index);
        let head = &text[..end];
        // A line end in the second half, else a space, else mid-word.
        let at = head
            .rfind('\n')
            .filter(|&at| at * 2 >= head.len())
            .or_else(|| head.rfind(char::is_whitespace).filter(|&at| at > 0))
            .unwrap_or(head.len());
        self.chars = 0;
        Some(format!("{}{CUT_MARK}", head[..at].trim_end()))
    }

    /// How many of a document's `pages` may be sent; the rest are recorded as cut.
    pub fn take_pages(&mut self, name: &str, pages: usize) -> usize {
        let given = pages.min(self.pages);
        self.pages -= given;
        if given < pages {
            self.cut(name);
        }
        given
    }

    /// Records `name` as cut for another reason (a long log's middle).
    pub fn cut(&mut self, name: &str) {
        if !self.truncated.iter().any(|known| known == name) {
            self.truncated.push(name.to_owned());
        }
    }

    /// The sources that didn't fit whole, in the order they were cut.
    pub fn truncated(&self) -> &[String] {
        &self.truncated
    }

    /// PDF pages that may still be read.
    pub fn pages_left(&self) -> usize {
        self.pages
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sources_share_the_budget_in_order() {
        let mut budget = Budget::new(100, 20);
        assert_eq!(budget.take("a.txt", "a".repeat(40)), Some("a".repeat(40)));
        let second = "word ".repeat(20); // 100 characters: 60 fit.
        let cut = budget.take("page", second).unwrap();
        assert!(cut.ends_with("… cut to fit …"), "{cut}");
        assert!(cut.starts_with("word word"));
        assert!(cut.chars().count() <= 60, "{}", cut.chars().count());
        assert!(!cut.contains("wor\n"), "cut at a space");
        assert_eq!(budget.take("c.log", "more".to_owned()), None);
        assert_eq!(budget.truncated(), ["page", "c.log"]);
    }

    #[test]
    fn a_cut_prefers_a_line_end_and_counts_characters() {
        let mut budget = Budget::new(40, 20);
        let text = "ăăăăăăăăăăăăăăăăăăăă\nsecond line goes past the end".to_owned();
        let cut = budget.take("x", text).unwrap();
        assert_eq!(cut, "ăăăăăăăăăăăăăăăăăăăă\n… cut to fit …");
    }

    #[test]
    fn pages_past_the_limit_are_cut() {
        let mut budget = Budget::default();
        assert_eq!(budget.take_pages("a.pdf", 12), 12);
        assert_eq!(budget.take_pages("b.pdf", 12), 8);
        assert_eq!(budget.take_pages("c.pdf", 3), 0);
        budget.cut("b.pdf");
        assert_eq!(budget.truncated(), ["b.pdf", "c.pdf"]);
    }
}
