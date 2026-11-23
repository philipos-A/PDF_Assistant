//! System prompt and per-question reader context.

use serde::{Deserialize, Serialize};

pub const SYSTEM_PROMPT: &str = "\
You are a patient reading companion helping someone work through a dense \
technical or academic paper. The full paper is attached as a PDF at the start \
of the conversation. The reader is scrolling through it and asking questions \
as they go.

How to help:
- Explain jargon in plain language first, then add precision. Assume the \
reader is smart but may be new to this field.
- When defining a term, define it the way *this paper* uses it. If the paper's \
usage differs from the common meaning, say so.
- Ground answers in the paper. Cite where things come from with page numbers \
in the form \"p. 4\" or \"pp. 4–5\", and mention section, equation, figure or \
table numbers when they exist (e.g. \"Eq. 3 on p. 5\", \"Fig. 2 on p. 7\").
- Write math in LaTeX: inline as $...$ and display as $$...$$. When \
explaining an equation, say what each symbol means and what the equation is \
doing intuitively.
- For figures and tables, describe what is plotted or listed and what the \
reader should take away from it.
- If something is not in the paper, say so plainly. You may then offer \
general background knowledge, clearly labelled as coming from outside the \
paper. Never invent results, citations, numbers or claims and attribute them \
to the paper.
- Use the reader's current position: if a question is ambiguous (\"what does \
this mean?\"), assume it refers to the selected text or the pages they are \
looking at.
- Keep answers focused. Start with the direct answer, then add detail only \
as needed. Use short paragraphs, and lists where they help.
";

/// Where the reader is when they ask a question: the page they're on, the pages on
/// screen, and any text they've selected (with the page it came from).
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ReaderContext {
    pub current_page: u32,
    #[serde(default)]
    pub visible_pages: Vec<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selected_text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selection_page: Option<u32>,
}

impl ReaderContext {
    /// The `<reader_context>` block prepended to the question. Deterministic, so earlier
    /// turns rebuild byte-for-byte and the prompt cache keeps hitting.
    pub fn render(&self, page_count: Option<usize>) -> String {
        let page = self.current_page;
        let mut lines = vec![match page_count {
            Some(n) => format!("Current page: {page} of {n}"),
            None => format!("Current page: {page}"),
        }];

        let mut pages: Vec<u32> = self
            .visible_pages
            .iter()
            .copied()
            .filter(|&p| p > 0)
            .collect();
        pages.sort_unstable();
        pages.dedup();
        if !pages.is_empty() {
            let list: Vec<String> = pages.iter().map(u32::to_string).collect();
            lines.push(format!("Pages visible on screen: {}", list.join(", ")));
        }

        if let Some(sel) = self
            .selected_text
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            let source = match self.selection_page.filter(|&p| p > 0) {
                Some(p) => format!(" on page {p}"),
                None => String::new(),
            };
            lines.push(format!(
                "Text the reader has selected{source}:\n<selection>\n{sel}\n</selection>"
            ));
        }

        format!("<reader_context>\n{}\n</reader_context>", lines.join("\n"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx(page: u32, visible: &[u32], sel: Option<&str>, sel_page: Option<u32>) -> ReaderContext {
        ReaderContext {
            current_page: page,
            visible_pages: visible.to_vec(),
            selected_text: sel.map(str::to_string),
            selection_page: sel_page,
        }
    }

    #[test]
    fn context_includes_pages_and_selection() {
        let text = ctx(3, &[4, 3, 3, 0], Some("  softmax  "), None).render(Some(10));
        assert!(text.contains("Current page: 3 of 10"));
        assert!(text.contains("Pages visible on screen: 3, 4"));
        assert!(text.contains("Text the reader has selected:\n<selection>\nsoftmax\n</selection>"));
    }

    #[test]
    fn context_names_the_selection_page() {
        let text = ctx(7, &[7], Some("softmax"), Some(3)).render(Some(10));
        assert!(text.contains("Text the reader has selected on page 3:\n<selection>"));
    }

    #[test]
    fn context_omits_empty_parts() {
        let text = ctx(1, &[], Some("   "), Some(2)).render(None);
        assert_eq!(text, "<reader_context>\nCurrent page: 1\n</reader_context>");
    }
}
