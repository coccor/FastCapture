//! HTML parsed the way browsers parse it (html5ever's tree builder), into a small DOM that is
//! read only once built: elements with their attributes, and text. Comments, doctypes and
//! template contents are dropped.

use html5ever::interface::tree_builder::{ElementFlags, NodeOrText, QuirksMode, TreeSink};
use html5ever::tendril::{StrTendril, TendrilSink};
use html5ever::{Attribute, QualName, parse_document};
use std::borrow::Cow;
use std::cell::{Cell, Ref, RefCell};
use std::rc::Rc;

/// How deep the tree may grow. The parser looks through every open element for each tag it
/// reads, so a page nested a million levels deep would take hours. Once the page nests deeper
/// than this, the rest of it is read as text, its tags dropped. Browsers cap the depth of what
/// they build at 512 too.
pub const MAX_DEPTH: usize = 512;

/// How much of the page is parsed between looks at how deep it has grown.
const CHUNK: usize = 4 * 1024;

/// A node's index in its `Dom`.
pub type NodeId = usize;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Data {
    Document,
    /// The local name, lowercase, and the attributes in document order.
    Element {
        name: String,
        attrs: Vec<(String, String)>,
    },
    Text(String),
}

#[derive(Clone, Debug)]
struct Node {
    parent: Option<NodeId>,
    children: Vec<NodeId>,
    data: Data,
}

#[derive(Clone, Debug)]
pub struct Dom {
    nodes: Vec<Node>,
}

impl Dom {
    pub fn parse(html: &str) -> Self {
        let deepest = Rc::new(Cell::new(0));
        let sink = Sink {
            deepest: Rc::clone(&deepest),
            ..Sink::default()
        };
        let mut parser = parse_document(sink, Default::default());
        let mut rest = html;
        while !rest.is_empty() && deepest.get() <= MAX_DEPTH {
            // Chunks end before a tag where they can, so the text-only rest starts at one.
            let mut end = rest.len().min(CHUNK);
            while !rest.is_char_boundary(end) {
                end += 1;
            }
            if end < rest.len()
                && let Some(tag) = rest[..end].rfind('<').filter(|&at| at > 0)
            {
                end = tag;
            }
            parser.process(StrTendril::from_slice(&rest[..end]));
            rest = &rest[end..];
        }
        if !rest.is_empty() {
            parser.process(StrTendril::from(text_only(rest)));
        }
        parser.finish()
    }

    /// The document node.
    pub fn root(&self) -> NodeId {
        0
    }

    pub fn data(&self, id: NodeId) -> &Data {
        &self.nodes[id].data
    }

    pub fn parent(&self, id: NodeId) -> Option<NodeId> {
        self.nodes[id].parent
    }

    pub fn children(&self, id: NodeId) -> &[NodeId] {
        &self.nodes[id].children
    }

    /// The element's name, or `None` for text and the document.
    pub fn name(&self, id: NodeId) -> Option<&str> {
        match &self.nodes[id].data {
            Data::Element { name, .. } => Some(name),
            _ => None,
        }
    }

    pub fn is(&self, id: NodeId, name: &str) -> bool {
        self.name(id) == Some(name)
    }

    pub fn attr(&self, id: NodeId, attr: &str) -> Option<&str> {
        match &self.nodes[id].data {
            Data::Element { attrs, .. } => attrs
                .iter()
                .find(|(name, _)| name == attr)
                .map(|(_, value)| value.as_str()),
            _ => None,
        }
    }

    /// The node and everything under it, in document order.
    pub fn descendants(&self, id: NodeId) -> Vec<NodeId> {
        let mut found = Vec::new();
        let mut stack = vec![id];
        while let Some(next) = stack.pop() {
            found.push(next);
            stack.extend(self.nodes[next].children.iter().rev());
        }
        found
    }

    /// The first element named `name` under `id` (or `id` itself).
    pub fn find(&self, id: NodeId, name: &str) -> Option<NodeId> {
        self.descendants(id)
            .into_iter()
            .find(|&node| self.is(node, name))
    }

    /// All the text under `id`, as it is in the source.
    pub fn text_of(&self, id: NodeId) -> String {
        self.descendants(id)
            .into_iter()
            .filter_map(|node| match &self.nodes[node].data {
                Data::Text(text) => Some(text.as_str()),
                _ => None,
            })
            .collect()
    }
}

/// The text of `html` with its tags and comments dropped, and scripts and styles with their
/// contents. A tag that isn't inline becomes a space, so words either side stay apart. A `<`
/// that starts no tag stays, as the parser reads it as text.
fn text_only(html: &str) -> String {
    const INLINE: &[&str] = &[
        "a", "abbr", "b", "code", "em", "i", "mark", "s", "small", "span", "strong", "sub", "sup",
        "u",
    ];
    let mut out = String::with_capacity(html.len() / 2);
    let mut rest = html;
    while let Some(at) = rest.find('<') {
        out.push_str(&rest[..at]);
        rest = &rest[at..];
        let after = &rest[1..];
        if let Some(comment) = after.strip_prefix("!--") {
            rest = comment.find("-->").map_or("", |end| &comment[end + 3..]);
            continue;
        }
        let starts_tag = after
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphabetic() || matches!(c, '/' | '!' | '?'));
        if !starts_tag {
            out.push('<');
            rest = after;
            continue;
        }
        let Some(close) = rest.find('>') else {
            rest = "";
            break;
        };
        let name: String = after
            .trim_start_matches('/')
            .chars()
            .take_while(char::is_ascii_alphanumeric)
            .map(|c| c.to_ascii_lowercase())
            .collect();
        let start_tag = !after.starts_with('/');
        rest = &rest[close + 1..];
        if start_tag && (name == "script" || name == "style") {
            let end_tag = format!("</{name}");
            rest = rest
                .as_bytes()
                .windows(end_tag.len())
                .position(|window| window.eq_ignore_ascii_case(end_tag.as_bytes()))
                .and_then(|end| rest[end..].find('>').map(|close| &rest[end + close + 1..]))
                .unwrap_or("");
            continue;
        }
        if !INLINE.contains(&name.as_str()) {
            out.push(' ');
        }
    }
    out.push_str(rest);
    out
}

/// The tree as the parser builds it. Handles are indexes, so nodes can be moved about while
/// the vector grows.
#[derive(Default)]
struct Sink {
    nodes: RefCell<Vec<Building>>,
    /// How deep the tree has grown, at its deepest.
    deepest: Rc<Cell<usize>>,
}

struct Building {
    parent: Option<NodeId>,
    children: Vec<NodeId>,
    data: Raw,
    /// How many ancestors it had when it was put in its place.
    depth: usize,
}

enum Raw {
    Document,
    Element {
        name: QualName,
        attrs: Vec<Attribute>,
        template: Option<NodeId>,
    },
    Text(String),
    /// Comments, processing instructions: kept as nodes so the parser's handles stay valid,
    /// dropped from the result.
    Other,
}

impl Sink {
    fn add(&self, data: Raw) -> NodeId {
        let mut nodes = self.nodes.borrow_mut();
        nodes.push(Building {
            parent: None,
            children: Vec::new(),
            data,
            depth: 0,
        });
        nodes.len() - 1
    }

    fn detach(nodes: &mut [Building], id: NodeId) {
        if let Some(parent) = nodes[id].parent.take() {
            nodes[parent].children.retain(|&child| child != id);
        }
    }

    /// Puts `child` under `parent` at `at`, merging text into a text node right before it.
    fn insert(&self, parent: NodeId, at: usize, child: NodeOrText<NodeId>) {
        let mut nodes = self.nodes.borrow_mut();
        let id = match child {
            NodeOrText::AppendText(text) => {
                if let Some(&before) = at
                    .checked_sub(1)
                    .and_then(|i| nodes[parent].children.get(i))
                    && let Raw::Text(existing) = &mut nodes[before].data
                {
                    existing.push_str(&text);
                    return;
                }
                nodes.push(Building {
                    parent: None,
                    children: Vec::new(),
                    data: Raw::Text(text.to_string()),
                    depth: 0,
                });
                nodes.len() - 1
            }
            NodeOrText::AppendNode(node) => {
                Self::detach(&mut nodes, node);
                node
            }
        };
        let at = at.min(nodes[parent].children.len());
        nodes[parent].children.insert(at, id);
        nodes[id].parent = Some(parent);
        let depth = nodes[parent].depth + 1;
        nodes[id].depth = depth;
        self.deepest.set(self.deepest.get().max(depth));
    }
}

impl TreeSink for Sink {
    type Handle = NodeId;
    type Output = Dom;
    type ElemName<'a> = Ref<'a, QualName>;

    fn finish(self) -> Dom {
        let built = self.nodes.into_inner();
        let mut nodes: Vec<Node> = Vec::with_capacity(built.len());
        for node in &built {
            let data = match &node.data {
                Raw::Document => Data::Document,
                Raw::Element { name, attrs, .. } => Data::Element {
                    name: name.local.to_string(),
                    attrs: attrs
                        .iter()
                        .map(|attr| (attr.name.local.to_string(), attr.value.to_string()))
                        .collect(),
                },
                Raw::Text(text) => Data::Text(text.clone()),
                Raw::Other => Data::Text(String::new()),
            };
            nodes.push(Node {
                parent: node.parent,
                children: node
                    .children
                    .iter()
                    .copied()
                    .filter(|&child| !matches!(built[child].data, Raw::Other))
                    .collect(),
                data,
            });
        }
        Dom { nodes }
    }

    fn parse_error(&self, _message: Cow<'static, str>) {}

    fn get_document(&self) -> NodeId {
        if self.nodes.borrow().is_empty() {
            self.add(Raw::Document);
        }
        0
    }

    fn elem_name<'a>(&'a self, target: &'a NodeId) -> Ref<'a, QualName> {
        Ref::map(self.nodes.borrow(), |nodes| match &nodes[*target].data {
            Raw::Element { name, .. } => name,
            _ => unreachable!("the parser asks only for elements' names"),
        })
    }

    fn create_element(&self, name: QualName, attrs: Vec<Attribute>, flags: ElementFlags) -> NodeId {
        // A template's contents are a separate fragment that is never shown.
        let template = flags.template.then(|| self.add(Raw::Document));
        self.add(Raw::Element {
            name,
            attrs,
            template,
        })
    }

    fn create_comment(&self, _text: StrTendril) -> NodeId {
        self.add(Raw::Other)
    }

    fn create_pi(&self, _target: StrTendril, _data: StrTendril) -> NodeId {
        self.add(Raw::Other)
    }

    fn append(&self, parent: &NodeId, child: NodeOrText<NodeId>) {
        let at = self.nodes.borrow()[*parent].children.len();
        self.insert(*parent, at, child);
    }

    fn append_based_on_parent_node(
        &self,
        element: &NodeId,
        prev_element: &NodeId,
        child: NodeOrText<NodeId>,
    ) {
        if self.nodes.borrow()[*element].parent.is_some() {
            self.append_before_sibling(element, child);
        } else {
            self.append(prev_element, child);
        }
    }

    fn append_doctype_to_document(
        &self,
        _name: StrTendril,
        _public: StrTendril,
        _system: StrTendril,
    ) {
    }

    fn get_template_contents(&self, target: &NodeId) -> NodeId {
        match &self.nodes.borrow()[*target].data {
            Raw::Element {
                template: Some(contents),
                ..
            } => *contents,
            _ => unreachable!("the parser asks only for templates' contents"),
        }
    }

    fn same_node(&self, x: &NodeId, y: &NodeId) -> bool {
        x == y
    }

    fn set_quirks_mode(&self, _mode: QuirksMode) {}

    fn append_before_sibling(&self, sibling: &NodeId, child: NodeOrText<NodeId>) {
        let place = {
            let nodes = self.nodes.borrow();
            nodes[*sibling].parent.map(|parent| {
                let at = nodes[parent]
                    .children
                    .iter()
                    .position(|&child| child == *sibling)
                    .unwrap_or(0);
                (parent, at)
            })
        };
        if let Some((parent, at)) = place {
            self.insert(parent, at, child);
        }
    }

    fn add_attrs_if_missing(&self, target: &NodeId, attrs: Vec<Attribute>) {
        if let Raw::Element {
            attrs: existing, ..
        } = &mut self.nodes.borrow_mut()[*target].data
        {
            for attr in attrs {
                if !existing.iter().any(|known| known.name == attr.name) {
                    existing.push(attr);
                }
            }
        }
    }

    fn remove_from_parent(&self, target: &NodeId) {
        Self::detach(&mut self.nodes.borrow_mut(), *target);
    }

    fn reparent_children(&self, node: &NodeId, new_parent: &NodeId) {
        let mut nodes = self.nodes.borrow_mut();
        let children = std::mem::take(&mut nodes[*node].children);
        let depth = nodes[*new_parent].depth + 1;
        for &child in &children {
            nodes[child].parent = Some(*new_parent);
            nodes[child].depth = depth;
        }
        nodes[*new_parent].children.extend(children);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(dom: &Dom, id: NodeId) -> Vec<String> {
        dom.descendants(id)
            .into_iter()
            .filter_map(|node| dom.name(node).map(str::to_owned))
            .collect()
    }

    #[test]
    fn a_document_parses_with_html_head_and_body() {
        let dom = Dom::parse("<!doctype html><title>T</title><p class=x>Hi <b>there</b><!-- c -->");
        assert_eq!(
            names(&dom, dom.root()),
            ["html", "head", "title", "body", "p", "b"]
        );
        let p = dom.find(dom.root(), "p").unwrap();
        assert_eq!(dom.attr(p, "class"), Some("x"));
        assert_eq!(dom.text_of(p), "Hi there");
        assert_eq!(dom.parent(p), dom.find(dom.root(), "body"));
    }

    #[test]
    fn broken_markup_is_repaired_as_browsers_do() {
        // A stray end tag, unclosed paragraphs and a table's misplaced text.
        let dom = Dom::parse("<div><p>one<p>two</span></div><table><tr><td>a</td></tr>x</table>");
        let body = dom.find(dom.root(), "body").unwrap();
        assert_eq!(
            names(&dom, body),
            ["body", "div", "p", "p", "table", "tbody", "tr", "td"]
        );
        // Text inside a table but outside a cell is moved before the table (foster parenting).
        let texts: Vec<String> = dom
            .children(body)
            .iter()
            .map(|&node| dom.text_of(node))
            .collect();
        assert_eq!(texts, ["onetwo", "x", "a"]);
        assert_eq!(dom.text_of(dom.find(dom.root(), "td").unwrap()), "a");
    }

    #[test]
    fn misnested_formatting_and_templates() {
        // The adoption agency algorithm moves nodes about with reparent_children.
        let dom = Dom::parse("<p><b>1<i>2</b>3</i>4</p><template><p>hidden</p></template>");
        let p = dom.find(dom.root(), "p").unwrap();
        assert_eq!(dom.text_of(p), "1234");
        assert_eq!(names(&dom, p), ["p", "b", "i", "i"]);
        let template = dom.find(dom.root(), "template").unwrap();
        assert_eq!(dom.text_of(template), "");
    }

    #[test]
    fn entities_are_decoded_and_text_nodes_merged() {
        let dom = Dom::parse("<p>a &amp; b&nbsp;c &lt;d&gt;</p>");
        let p = dom.find(dom.root(), "p").unwrap();
        assert_eq!(dom.children(p).len(), 1);
        assert_eq!(dom.text_of(p), "a & b\u{a0}c <d>");
    }

    #[test]
    fn a_page_nested_too_deep_is_read_as_text_past_the_cap() {
        let html = format!(
            "<p>before</p>{}deep<b>er</b><br>down<SCRIPT>gone()</script><!-- <p> --> 1 < 2{}<p>after</p>",
            "<div>".repeat(100_000),
            "</div>".repeat(100_000)
        );
        let started = std::time::Instant::now();
        let dom = Dom::parse(&html);
        assert!(started.elapsed() < std::time::Duration::from_secs(10));
        let body = dom.find(dom.root(), "body").unwrap();
        let text = dom.text_of(body);
        let words: Vec<&str> = text.split_whitespace().collect();
        assert_eq!(words, ["before", "deeper", "down", "1", "<", "2", "after"]);
        let divs = names(&dom, body).iter().filter(|n| *n == "div").count();
        assert!((MAX_DEPTH..MAX_DEPTH + CHUNK).contains(&divs), "{divs}");
    }

    #[test]
    fn a_page_of_any_size_parses_whole_when_shallow() {
        // Many chunks, and a character that crosses a chunk's end.
        let html = format!("<p>{}é</p>", "x".repeat(CHUNK * 3 - 4)).repeat(3);
        let dom = Dom::parse(&html);
        let body = dom.find(dom.root(), "body").unwrap();
        assert_eq!(
            dom.text_of(body),
            html.replace("<p>", "").replace("</p>", "")
        );
    }
}
