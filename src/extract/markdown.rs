//! HTML to Markdown, for pasted rich text and web pages: headings, paragraphs, emphasis, lists,
//! links, code, quotes and tables. Scripts, forms' controls and hidden elements are dropped;
//! whitespace collapses as a browser shows it.

use super::dom::{Data, Dom, NodeId};
use std::collections::HashSet;

/// The Markdown for `node` and everything under it. Relative links and images are resolved
/// against `base` when it is given.
pub fn to_markdown(dom: &Dom, node: NodeId, base: Option<&str>) -> String {
    let none = HashSet::new();
    Converter::new(dom, base, &none).run(node).join("\n\n")
}

/// The Markdown for `nodes`, in order, leaving out the nodes in `skip` and everything under
/// them.
pub fn to_markdown_without(
    dom: &Dom,
    nodes: &[NodeId],
    base: Option<&str>,
    skip: &HashSet<NodeId>,
) -> String {
    nodes
        .iter()
        .flat_map(|&node| Converter::new(dom, base, skip).run(node))
        .collect::<Vec<_>>()
        .join("\n\n")
}

/// Elements that start a block of their own.
const BLOCKS: &[&str] = &[
    "address",
    "article",
    "aside",
    "blockquote",
    "body",
    "caption",
    "center",
    "dd",
    "details",
    "dialog",
    "dir",
    "div",
    "dl",
    "dt",
    "fieldset",
    "figcaption",
    "figure",
    "footer",
    "form",
    "h1",
    "h2",
    "h3",
    "h4",
    "h5",
    "h6",
    "header",
    "hgroup",
    "hr",
    "html",
    "legend",
    "li",
    "main",
    "menu",
    "nav",
    "ol",
    "p",
    "pre",
    "section",
    "summary",
    "table",
    "tbody",
    "td",
    "tfoot",
    "th",
    "thead",
    "tr",
    "ul",
];

/// Elements whose content is never shown as text.
const SKIPPED: &[&str] = &[
    "audio", "button", "canvas", "embed", "head", "iframe", "input", "map", "nav", "noscript",
    "object", "script", "select", "style", "svg", "template", "textarea", "video",
];

fn is_block(name: &str) -> bool {
    BLOCKS.contains(&name)
}

/// Pasted rich text as Markdown, when it has structure plain text would lose (headings,
/// lists, links, tables, code, quotes); `None` when it is plain paragraphs, which paste better
/// as the plain text beside them.
pub fn rich_text(fragment: &str, base: Option<&str>) -> Option<String> {
    let dom = Dom::parse(fragment);
    let rich = dom
        .descendants(dom.root())
        .into_iter()
        .any(|node| match dom.name(node) {
            Some(
                "h1" | "h2" | "h3" | "h4" | "h5" | "h6" | "ul" | "ol" | "table" | "pre" | "code"
                | "blockquote",
            ) => true,
            Some("a") => dom
                .attr(node, "href")
                .is_some_and(|href| !href.trim().is_empty() && !href.starts_with('#')),
            Some("p") => word_list_level(&dom, node).is_some(),
            _ => false,
        });
    if !rich {
        return None;
    }
    let markdown = to_markdown(&dom, dom.root(), base);
    (!markdown.trim().is_empty()).then_some(markdown)
}

/// The level of a list paragraph Word pastes (`style="mso-list:l0 level2 lfo1"`).
fn word_list_level(dom: &Dom, id: NodeId) -> Option<usize> {
    let style = dom.attr(id, "style")?.to_ascii_lowercase();
    let list = &style[style.find("mso-list:")? + "mso-list:".len()..];
    if list.trim_start().starts_with("ignore") {
        return None;
    }
    let level = &list[list.find("level")? + "level".len()..];
    let digits: String = level.chars().take_while(char::is_ascii_digit).collect();
    digits.parse().ok().filter(|&level| level >= 1)
}

/// The span holding a Word list item's bullet or number (`style="mso-list:Ignore"`).
fn word_list_marker_span(dom: &Dom, id: NodeId) -> bool {
    dom.attr(id, "style").is_some_and(|style| {
        style
            .to_ascii_lowercase()
            .replace(char::is_whitespace, "")
            .contains("mso-list:ignore")
    })
}

/// Dropped with everything under it: the elements above, and hidden ones.
fn skipped(dom: &Dom, id: NodeId) -> bool {
    let Some(name) = dom.name(id) else {
        return false;
    };
    SKIPPED.contains(&name)
        || dom.attr(id, "hidden").is_some()
        || dom.attr(id, "aria-hidden") == Some("true")
        || word_list_marker_span(dom, id)
        || dom.attr(id, "style").is_some_and(|style| {
            style
                .to_ascii_lowercase()
                .replace(char::is_whitespace, "")
                .contains("display:none")
        })
}

fn contains_block(dom: &Dom, id: NodeId) -> bool {
    dom.descendants(id)
        .into_iter()
        .skip(1)
        .any(|node| dom.name(node).is_some_and(is_block))
}

struct Converter<'a> {
    dom: &'a Dom,
    base: Option<&'a str>,
    /// Nodes left out on top of the ones `skipped` drops.
    skip: &'a HashSet<NodeId>,
    blocks: Vec<String>,
    inline: String,
    /// The last block is an item of a Word list, so the next item joins it.
    in_word_list: bool,
}

impl<'a> Converter<'a> {
    fn new(dom: &'a Dom, base: Option<&'a str>, skip: &'a HashSet<NodeId>) -> Self {
        Self {
            dom,
            base,
            skip,
            blocks: Vec::new(),
            inline: String::new(),
            in_word_list: false,
        }
    }

    /// The blocks `node` holds.
    fn run(mut self, node: NodeId) -> Vec<String> {
        self.node(node);
        self.flush();
        self.blocks
    }

    fn skips(&self, id: NodeId) -> bool {
        self.skip.contains(&id) || skipped(self.dom, id)
    }

    fn sub(&self, node: NodeId) -> Vec<String> {
        let mut sub = Converter::new(self.dom, self.base, self.skip);
        sub.children(node);
        sub.flush();
        sub.blocks
    }

    fn children(&mut self, node: NodeId) {
        for &child in self.dom.children(node) {
            self.node(child);
        }
    }

    fn node(&mut self, id: NodeId) {
        match self.dom.data(id) {
            Data::Document => self.children(id),
            Data::Text(text) => self.inline.push_str(&escape(&collapse(text))),
            Data::Element { name, .. } => {
                if self.skips(id) {
                    return;
                }
                if is_block(name) {
                    self.block(id, name);
                } else if contains_block(self.dom, id) {
                    // An inline element around blocks (a link around a card): its content
                    // as blocks, without the inline formatting.
                    self.children(id);
                } else {
                    let text = self.inline_of(id);
                    self.inline.push_str(&text);
                }
            }
        }
    }

    /// The paragraph collected so far, as a block.
    fn flush(&mut self) {
        let text = std::mem::take(&mut self.inline);
        let lines: Vec<String> = text
            .split('\n')
            .map(|line| escape_line_start(&squeeze(line)))
            .collect();
        let start = lines.iter().position(|line| !line.is_empty());
        let end = lines.iter().rposition(|line| !line.is_empty());
        if let (Some(start), Some(end)) = (start, end) {
            self.blocks.push(lines[start..=end].join("\n"));
            self.in_word_list = false;
        }
    }

    fn push(&mut self, block: String) {
        if !block.trim().is_empty() {
            self.blocks.push(block);
            self.in_word_list = false;
        }
    }

    /// A Word list paragraph as a list item, joining the item before it.
    fn word_list_item(&mut self, id: NodeId, level: usize) {
        let marker = self
            .dom
            .descendants(id)
            .into_iter()
            .find(|&node| word_list_marker_span(self.dom, node))
            .map(|span| squeeze(&collapse(&self.dom.text_of(span))))
            .filter(|marker| {
                marker.len() <= 4
                    && marker.ends_with(['.', ')'])
                    && marker[..marker.len() - 1]
                        .chars()
                        .all(char::is_alphanumeric)
            })
            .unwrap_or_else(|| "-".to_owned());
        let text = self.sub(id).join(" ");
        if text.is_empty() {
            return;
        }
        let item = format!("{}{marker} {text}", "  ".repeat(level.saturating_sub(1)));
        match self.blocks.last_mut() {
            Some(last) if self.in_word_list => {
                last.push('\n');
                last.push_str(&item);
            }
            _ => self.blocks.push(item),
        }
        self.in_word_list = true;
    }

    fn block(&mut self, id: NodeId, name: &str) {
        self.flush();
        if name == "p"
            && let Some(level) = word_list_level(self.dom, id)
        {
            self.word_list_item(id, level);
            return;
        }
        match name {
            "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => {
                let level = usize::from(name.as_bytes()[1] - b'0');
                let text = squeeze(&self.inline_children(id).replace('\n', " "));
                if !text.is_empty() {
                    self.push(format!("{} {text}", "#".repeat(level)));
                }
            }
            "pre" => self.code_block(id),
            "ul" | "ol" => self.list(id, name == "ol"),
            "blockquote" => {
                let quoted = self.sub(id).join("\n\n");
                let lines: Vec<String> = quoted
                    .lines()
                    .map(|line| {
                        if line.is_empty() {
                            ">".to_owned()
                        } else {
                            format!("> {line}")
                        }
                    })
                    .collect();
                self.push(lines.join("\n"));
            }
            "table" => self.table(id),
            "hr" => self.push("---".to_owned()),
            _ => {
                self.children(id);
                self.flush();
            }
        }
    }

    fn code_block(&mut self, id: NodeId) {
        let mut text = String::new();
        for node in self.dom.descendants(id) {
            match self.dom.data(node) {
                Data::Text(part) => text.push_str(part),
                Data::Element { name, .. } if name == "br" => text.push('\n'),
                _ => {}
            }
        }
        let text = text.trim_end().trim_start_matches('\n');
        if text.trim().is_empty() {
            return;
        }
        let language = [Some(id), self.dom.find(id, "code")]
            .into_iter()
            .flatten()
            .filter_map(|node| self.dom.attr(node, "class"))
            .flat_map(str::split_whitespace)
            .find_map(|class| {
                class
                    .strip_prefix("language-")
                    .or_else(|| class.strip_prefix("lang-"))
            })
            .unwrap_or("");
        self.push(super::text::fence(text, language));
    }

    fn list(&mut self, id: NodeId, ordered: bool) {
        let mut number = self
            .dom
            .attr(id, "start")
            .and_then(|start| start.trim().parse::<i64>().ok())
            .unwrap_or(1);
        let mut items: Vec<String> = Vec::new();
        for &child in self.dom.children(id) {
            if self.skips(child) {
                continue;
            }
            match self.dom.name(child) {
                Some("li") => {
                    let body = self.sub(child).join("\n");
                    if body.is_empty() {
                        continue;
                    }
                    let marker = if ordered {
                        format!("{number}.")
                    } else {
                        "-".to_owned()
                    };
                    number += 1;
                    items.push(indent_after_first(&body, &marker));
                }
                // A list straight inside a list (not inside an item) belongs to the item before.
                Some("ul" | "ol") => {
                    let nested = self.sub_list(child);
                    match items.last_mut() {
                        Some(last) => {
                            let width = if ordered { 3 } else { 2 };
                            last.push('\n');
                            last.push_str(&indent(&nested, width));
                        }
                        None => items.push(nested),
                    }
                }
                _ => {}
            }
        }
        self.push(items.join("\n"));
    }

    fn sub_list(&self, id: NodeId) -> String {
        let mut sub = Converter::new(self.dom, self.base, self.skip);
        sub.list(id, self.dom.is(id, "ol"));
        sub.blocks.join("\n")
    }

    fn table(&mut self, id: NodeId) {
        let mut rows: Vec<Vec<NodeId>> = Vec::new();
        for &child in self.dom.children(id) {
            match self.dom.name(child) {
                Some("tr") => rows.push(self.cells(child)),
                Some("thead" | "tbody" | "tfoot") => {
                    for &row in self.dom.children(child) {
                        if self.dom.is(row, "tr") {
                            rows.push(self.cells(row));
                        }
                    }
                }
                _ => {}
            }
        }
        let cells = || rows.iter().flatten().copied();
        // A table used for layout: its cells hold paragraphs, lists or more tables.
        if cells().any(|cell| contains_block(self.dom, cell)) {
            for cell in cells().collect::<Vec<_>>() {
                self.children(cell);
                self.flush();
            }
            return;
        }
        let texts: Vec<Vec<String>> = rows
            .iter()
            .map(|row| {
                row.iter()
                    .map(|&cell| {
                        squeeze(&self.inline_children(cell).replace('\n', " ")).replace('|', "\\|")
                    })
                    .collect()
            })
            .filter(|row: &Vec<String>| row.iter().any(|cell| !cell.is_empty()))
            .collect();
        let Some(columns) = texts.first().map(Vec::len) else {
            return;
        };
        let spans = cells().any(|cell| {
            ["colspan", "rowspan"].iter().any(|attr| {
                self.dom
                    .attr(cell, attr)
                    .and_then(|span| span.trim().parse::<u32>().ok())
                    .is_some_and(|span| span > 1)
            })
        });
        let line = |row: &Vec<String>| format!("| {} |", row.join(" | "));
        if !spans && texts.iter().all(|row| row.len() == columns) {
            let mut lines = vec![line(&texts[0]), line(&vec!["---".to_owned(); columns])];
            lines.extend(texts[1..].iter().map(line));
            self.push(lines.join("\n"));
        } else {
            let lines: Vec<String> = texts
                .iter()
                .map(|row| {
                    row.iter()
                        .filter(|cell| !cell.is_empty())
                        .cloned()
                        .collect::<Vec<_>>()
                        .join(" | ")
                })
                .collect();
            self.push(lines.join("\n"));
        }
    }

    fn cells(&self, row: NodeId) -> Vec<NodeId> {
        self.dom
            .children(row)
            .iter()
            .copied()
            .filter(|&cell| self.dom.is(cell, "td") || self.dom.is(cell, "th"))
            .collect()
    }

    fn inline_children(&self, id: NodeId) -> String {
        self.dom
            .children(id)
            .iter()
            .map(|&child| match self.dom.data(child) {
                Data::Text(text) => escape(&collapse(text)),
                Data::Element { .. } if self.skips(child) => String::new(),
                Data::Element { .. } => self.inline_of(child),
                Data::Document => String::new(),
            })
            .collect()
    }

    fn inline_of(&self, id: NodeId) -> String {
        let Some(name) = self.dom.name(id) else {
            return String::new();
        };
        match name {
            "br" => "\n".to_owned(),
            "wbr" => String::new(),
            "img" => self.image(id),
            "strong" | "b" => wrap("**", &self.inline_children(id)),
            "em" | "i" | "cite" => wrap("*", &self.inline_children(id)),
            "s" | "del" | "strike" => wrap("~~", &self.inline_children(id)),
            "code" | "kbd" | "samp" | "tt" => code_span(&collapse(&self.dom.text_of(id))),
            "a" => self.link(id),
            _ => self.inline_children(id),
        }
    }

    fn link(&self, id: NodeId) -> String {
        let text = self.inline_children(id);
        let Some(href) = self.dom.attr(id, "href").map(str::trim).filter(|href| {
            !href.is_empty() && !href.to_ascii_lowercase().starts_with("javascript:")
        }) else {
            return text;
        };
        let label = squeeze(&text.replace('\n', " "));
        if label.is_empty() {
            return text;
        }
        let url = link_target(&resolve(self.base, href));
        let (lead, trail) = edges(&text);
        if label == url || label == href {
            format!("{lead}<{url}>{trail}")
        } else {
            format!("{lead}[{label}]({url}){trail}")
        }
    }

    fn image(&self, id: NodeId) -> String {
        let alt = squeeze(&collapse(self.dom.attr(id, "alt").unwrap_or("")));
        let src = self.dom.attr(id, "src").map(str::trim).unwrap_or("");
        if src.is_empty() || src.starts_with("data:") {
            return escape(&alt);
        }
        format!(
            "![{}]({})",
            escape(&alt),
            link_target(&resolve(self.base, src))
        )
    }
}

/// Whitespace runs as one space, as a browser shows them. A no-break space is a space too.
fn collapse(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut space = false;
    for c in text.chars() {
        if c.is_ascii_whitespace() || c == '\u{a0}' {
            if !space {
                out.push(' ');
            }
            space = true;
        } else {
            out.push(c);
            space = false;
        }
    }
    out
}

/// One line with its spaces collapsed and trimmed.
fn squeeze(line: &str) -> String {
    line.split(' ')
        .filter(|word| !word.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

/// Markdown's inline characters escaped, so text reads as text. An underscore inside a word
/// (`snake_case`) and a `<` that can't start a tag are left alone.
fn escape(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    for (i, &c) in chars.iter().enumerate() {
        let before = i.checked_sub(1).map(|i| chars[i]);
        let after = chars.get(i + 1).copied();
        let escaped = match c {
            '\\' | '*' | '`' | '[' | ']' => true,
            '_' => {
                !(before.is_some_and(char::is_alphanumeric)
                    && after.is_some_and(char::is_alphanumeric))
            }
            '<' => {
                after.is_some_and(|after| after.is_ascii_alphabetic() || matches!(after, '/' | '!'))
            }
            _ => false,
        };
        if escaped {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// A paragraph line that would read as a heading, list item or quote is escaped.
fn escape_line_start(line: &str) -> String {
    let first = line.split(' ').next().unwrap_or("");
    let block = matches!(
        first,
        "#" | "##" | "###" | "####" | "#####" | "######" | "-" | "+" | ">"
    ) || (first.len() > 1
        && first.ends_with(['.', ')'])
        && first[..first.len() - 1].bytes().all(|b| b.is_ascii_digit()));
    if block && line.len() > first.len() || line == ">" {
        match first.find(['.', ')']) {
            Some(at) if !first.starts_with(['#', '-', '+', '>']) => {
                format!("{}\\{}", &line[..at], &line[at..])
            }
            _ => format!("\\{line}"),
        }
    } else {
        line.to_owned()
    }
}

/// The leading and trailing space of an inline element's text, which goes outside its marks.
fn edges(text: &str) -> (&'static str, &'static str) {
    let lead = if text.starts_with([' ', '\n']) {
        " "
    } else {
        ""
    };
    let trail = if text.ends_with([' ', '\n']) { " " } else { "" };
    (lead, trail)
}

fn wrap(mark: &str, text: &str) -> String {
    let inner = text.trim_matches([' ', '\n']);
    if inner.is_empty() {
        return text.to_owned();
    }
    let (lead, trail) = edges(text);
    format!("{lead}{mark}{inner}{mark}{trail}")
}

fn code_span(text: &str) -> String {
    let inner = text.trim();
    if inner.is_empty() {
        return String::new();
    }
    let longest = inner.split(|c| c != '`').map(str::len).max().unwrap_or(0);
    let ticks = "`".repeat(longest + 1);
    let pad = if inner.starts_with('`') || inner.ends_with('`') {
        " "
    } else {
        ""
    };
    let (lead, trail) = edges(text);
    format!("{lead}{ticks}{pad}{inner}{pad}{ticks}{trail}")
}

/// A URL safe inside `(…)` and `<…>`.
fn link_target(url: &str) -> String {
    url.replace(' ', "%20")
        .replace('(', "%28")
        .replace(')', "%29")
        .replace('<', "%3C")
        .replace('>', "%3E")
}

fn indent(text: &str, width: usize) -> String {
    let pad = " ".repeat(width);
    text.lines()
        .map(|line| {
            if line.is_empty() {
                String::new()
            } else {
                format!("{pad}{line}")
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// `text` after a list marker, its later lines indented to line up under the first.
fn indent_after_first(text: &str, marker: &str) -> String {
    let (first, rest) = text.split_once('\n').unwrap_or((text, ""));
    let mut out = format!("{marker} {first}");
    if !rest.is_empty() {
        out.push('\n');
        out.push_str(&indent(rest, marker.len() + 1));
    }
    out
}

/// `href` resolved against `base`: absolute URLs as they are, then scheme-relative,
/// root-relative, query-only, fragment-only and path-relative ones, with `.` and `..` removed.
pub fn resolve(base: Option<&str>, href: &str) -> String {
    let href = href.trim();
    let Some(base) = base.filter(|_| !has_scheme(href)) else {
        return href.to_owned();
    };
    let Some(scheme_end) = base.find("://") else {
        return href.to_owned();
    };
    let after = &base[scheme_end + 3..];
    let host_end = after.find(['/', '?', '#']).unwrap_or(after.len());
    let origin = &base[..scheme_end + 3 + host_end];
    let rest = &after[host_end..];
    let path = &rest[..rest.find(['?', '#']).unwrap_or(rest.len())];
    if let Some(network) = href.strip_prefix("//") {
        return format!("{}://{network}", &base[..scheme_end]);
    }
    if href.starts_with('#') {
        let without = &base[..base.find('#').unwrap_or(base.len())];
        return format!("{without}{href}");
    }
    if href.starts_with('?') {
        return format!("{origin}{}{href}", if path.is_empty() { "/" } else { path });
    }
    let (href_path, tail) = href.split_at(href.find(['?', '#']).unwrap_or(href.len()));
    let joined = if href_path.starts_with('/') {
        href_path.to_owned()
    } else {
        let dir = &path[..path.rfind('/').map_or(0, |at| at + 1)];
        format!("{}{href_path}", if dir.is_empty() { "/" } else { dir })
    };
    format!("{origin}{}{tail}", remove_dots(&joined))
}

fn has_scheme(href: &str) -> bool {
    let Some(colon) = href.find(':') else {
        return false;
    };
    let scheme = &href[..colon];
    !href[..colon].contains(['/', '?', '#'])
        && scheme.starts_with(|c: char| c.is_ascii_alphabetic())
        && scheme
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
}

fn remove_dots(path: &str) -> String {
    let mut segments: Vec<&str> = Vec::new();
    let parts: Vec<&str> = path.split('/').skip(1).collect();
    for (i, part) in parts.iter().enumerate() {
        let last = i + 1 == parts.len();
        match *part {
            "." => {
                if last {
                    segments.push("");
                }
            }
            ".." => {
                segments.pop();
                if last {
                    segments.push("");
                }
            }
            part => segments.push(part),
        }
    }
    format!("/{}", segments.join("/"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn md(html: &str) -> String {
        let dom = Dom::parse(html);
        to_markdown(
            &dom,
            dom.root(),
            Some("https://example.com/docs/guide/page.html?x=1"),
        )
    }

    #[test]
    fn headings_paragraphs_and_inline_formatting() {
        assert_eq!(
            md(
                "<h1>Title</h1><p>Some <b>bold</b>, <em> slanted </em> and <code>a_b()</code>.</p>\
                <h3>Small  <i>part</i></h3><p>line one<br>line two</p><hr><p>~~<s>gone</s></p>"
            ),
            "# Title\n\nSome **bold**, *slanted* and `a_b()`.\n\n### Small *part*\n\n\
             line one\nline two\n\n---\n\n~~~~gone~~"
        );
    }

    #[test]
    fn links_and_images_resolve_against_the_page() {
        assert_eq!(
            md(
                "<p><a href='../intro.html#start'>Intro</a>, <a href='/'>home</a>, \
                <a href='https://other.org/a b'>https://other.org/a b</a>, \
                <a href='javascript:void(0)'>menu</a>, <a href='x'></a>\
                <img src='img/chart.png' alt='Lag chart'> <img src='data:image/png;base64,AA' alt='inline'></p>"
            ),
            "[Intro](https://example.com/docs/intro.html#start), [home](https://example.com/), \
             <https://other.org/a%20b>, menu, \
             ![Lag chart](https://example.com/docs/guide/img/chart.png) inline"
        );
    }

    #[test]
    fn nested_and_numbered_lists() {
        assert_eq!(
            md(
                "<ul><li>one</li><li>two<ul><li>two a</li><li>two b</li></ul></li><li><p>three</p><p>more</p></li></ul>\
                <ol start=4><li>four</li><li>five<ol><li>inner</li></ol></li></ol>\
                <ul><li>x</li><ul><li>stray</li></ul></ul>"
            ),
            "- one\n- two\n  - two a\n  - two b\n- three\n  more\n\n\
             4. four\n5. five\n   1. inner\n\n\
             - x\n  - stray"
        );
    }

    #[test]
    fn code_blocks_keep_their_whitespace_and_language() {
        assert_eq!(
            md(
                "<pre><code class=\"hljs language-rust\">fn main() {\n    let x = 1;\n}\n</code></pre>\
                <pre>plain<br>text  with  spaces</pre>"
            ),
            "```rust\nfn main() {\n    let x = 1;\n}\n```\n\n```\nplain\ntext  with  spaces\n```"
        );
    }

    #[test]
    fn quotes_and_tables() {
        assert_eq!(
            md(
                "<blockquote><p>Said once.</p><p>Said <b>twice</b>.</p></blockquote>\
                <table><thead><tr><th>Name</th><th>Lag</th></tr></thead>\
                <tbody><tr><td>ledger</td><td>40k</td></tr><tr><td>a|b</td><td></td></tr></tbody></table>"
            ),
            "> Said once.\n>\n> Said **twice**.\n\n\
             | Name | Lag |\n| --- | --- |\n| ledger | 40k |\n| a\\|b |  |"
        );
    }

    #[test]
    fn irregular_and_layout_tables() {
        // A colspan: rows as lines.
        assert_eq!(
            md("<table><tr><td colspan=2>Total</td></tr><tr><td>a</td><td>b</td></tr></table>"),
            "Total\na | b"
        );
        // A table holding paragraphs is layout: its cells' content as blocks.
        assert_eq!(
            md(
                "<table><tr><td><p>Left column</p></td><td><ul><li>item</li></ul></td></tr></table>"
            ),
            "Left column\n\n- item"
        );
    }

    #[test]
    fn scripts_navigation_and_hidden_things_are_dropped() {
        assert_eq!(
            md(
                "<nav><a href=/>Home</a></nav><script>alert(1)</script><style>p{}</style>\
                <p hidden>secret</p><p style='display: none'>gone</p><span aria-hidden=true>★</span>\
                <form><p>Kept: forms wrap whole pages in some sites.</p><input value=x><button>Go</button></form>"
            ),
            "Kept: forms wrap whole pages in some sites."
        );
    }

    #[test]
    fn markdown_characters_in_text_are_escaped() {
        assert_eq!(
            md(
                "<p>2 * 3 = [six] with snake_case and _under_ and <b>&lt;div&gt;</b> and a &lt; b</p>\
                <p># not a heading</p><p>- not a list</p><p>1. not numbered</p><p>-5 is fine</p>"
            ),
            "2 \\* 3 = \\[six\\] with snake_case and \\_under\\_ and **\\<div>** and a < b\n\n\
             \\# not a heading\n\n\\- not a list\n\n1\\. not numbered\n\n-5 is fine"
        );
    }

    #[test]
    fn whitespace_collapses_like_a_browser() {
        assert_eq!(
            md(
                "<div>\n  <p>\n    Spread   over\n    lines&nbsp;&nbsp;here\n  </p>\n  <span>inline</span> <b> next</b>\n</div>"
            ),
            "Spread over lines here\n\ninline **next**"
        );
    }

    #[test]
    fn inline_elements_around_blocks_and_malformed_html() {
        assert_eq!(
            md(
                "<a href='/card'><div><h2>Card</h2><p>Text</p></div></a><p>open <b>bold <i>both</p>after"
            ),
            "## Card\n\nText\n\nopen **bold *both***\n\n***after***"
        );
    }

    #[test]
    fn urls_resolve_like_a_browser() {
        let base = Some("https://example.com/a/b/c.html?q=1#top");
        assert_eq!(resolve(base, "d.html"), "https://example.com/a/b/d.html");
        assert_eq!(resolve(base, "../d"), "https://example.com/a/d");
        assert_eq!(resolve(base, "../../../d"), "https://example.com/d");
        assert_eq!(resolve(base, "./"), "https://example.com/a/b/");
        assert_eq!(resolve(base, "/x?y=2"), "https://example.com/x?y=2");
        assert_eq!(resolve(base, "//cdn.net/z"), "https://cdn.net/z");
        assert_eq!(
            resolve(base, "#end"),
            "https://example.com/a/b/c.html?q=1#end"
        );
        assert_eq!(resolve(base, "?p=3"), "https://example.com/a/b/c.html?p=3");
        assert_eq!(resolve(base, "mailto:a@b.c"), "mailto:a@b.c");
        assert_eq!(
            resolve(Some("https://example.com"), "x"),
            "https://example.com/x"
        );
        assert_eq!(resolve(None, "x"), "x");
    }

    fn fixture(name: &str) -> String {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join(r"tests\fixtures\clipboard")
            .join(name);
        std::fs::read_to_string(path).unwrap()
    }

    #[test]
    fn rich_text_from_chrome_keeps_its_structure() {
        assert_eq!(
            rich_text(
                &fixture("chrome.html"),
                Some("https://kafka.example.org/docs/lag")
            )
            .unwrap(),
            "## Consumer lag\n\n\
             Lag grows when a consumer group falls behind the \
             [topic](https://kafka.example.org/docs/topics)'s newest offset.\n\n\
             - **Check** the batch size.\n- Watch `records-lag-max`."
        );
    }

    #[test]
    fn plain_paragraphs_paste_as_plain_text() {
        assert_eq!(rich_text(&fixture("edge.html"), None), None);
        assert_eq!(rich_text("<p>one</p><p>two <i>three</i></p>", None), None);
        assert_eq!(
            rich_text("<p><a href='#top'>back to top</a></p>", None),
            None
        );
    }

    #[test]
    fn word_lists_become_markdown_lists() {
        assert_eq!(
            rich_text(&fixture("word.html"), None).unwrap(),
            "Ledger batch review\n\nFindings from the **night run**:\n\n\
             - Lag peaked at 40k\n  - only ledger-events\n- Recovered by 02:35\n\
             1. Check the batch size"
        );
    }
}
