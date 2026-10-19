//! A web page's main content and its facts. The content is found the way reading views find
//! it: paragraphs score their parent blocks by length and commas, class and id names that mean
//! "article" or "sidebar" push the score up or down, links count against it, and the best block
//! is kept with the siblings that belong to it. Navigation, asides, footers, share bars and
//! link lists inside it are left out.

use super::dom::{Data, Dom, NodeId};
use super::markdown;
use std::collections::{HashMap, HashSet};

/// Less main text than this, in characters, and the page counts as having none.
pub const MIN_TEXT: usize = 200;

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Page {
    /// `og:title`, else `<title>`.
    pub title: Option<String>,
    /// `og:site_name`, else the host without `www.`.
    pub site: Option<String>,
    /// `article:published_time`, as the page gives it.
    pub published: Option<String>,
    /// The main content as Markdown, links resolved against the page; empty when the page has
    /// no main text.
    pub markdown: String,
}

/// The page at `url` (after redirects) with `html`.
pub fn extract(html: &str, url: &str) -> Page {
    let dom = Dom::parse(html);
    let mut page = metadata(&dom, url);
    let body = dom.find(dom.root(), "body").unwrap_or(dom.root());
    let skip = clutter(&dom, body);
    let content = main_content(&dom, body, &skip);
    let markdown = markdown::to_markdown_without(&dom, &content, Some(url), &skip);
    if markdown.chars().filter(|c| c.is_alphanumeric()).count() >= MIN_TEXT * 3 / 4 {
        page.markdown = markdown;
    }
    page
}

/// Words in class and id names that mean content…
const POSITIVE: &[&str] = &[
    "article", "body", "content", "entry", "main", "markdown", "page", "post", "prose", "story",
    "text",
];

/// …and words that mean everything around it.
const NEGATIVE: &[&str] = &[
    "ad",
    "ads",
    "advert",
    "advertisement",
    "banner",
    "breadcrumb",
    "breadcrumbs",
    "comment",
    "comments",
    "cookie",
    "cookies",
    "footer",
    "footnote",
    "masthead",
    "menu",
    "modal",
    "nav",
    "navbar",
    "navigation",
    "newsletter",
    "outbrain",
    "pager",
    "pagination",
    "popup",
    "promo",
    "recommended",
    "related",
    "share",
    "sharing",
    "sidebar",
    "signup",
    "social",
    "sponsor",
    "sponsored",
    "subscribe",
    "taboola",
    "toolbar",
    "trending",
    "widget",
];

/// The words of an element's class and id, lowercase.
fn words(dom: &Dom, id: NodeId) -> Vec<String> {
    [dom.attr(id, "class"), dom.attr(id, "id")]
        .into_iter()
        .flatten()
        .flat_map(|value| {
            value
                .split(|c: char| !c.is_ascii_alphanumeric())
                .filter(|word| !word.is_empty())
                .map(str::to_ascii_lowercase)
                .collect::<Vec<_>>()
        })
        .collect()
}

/// +25 for a content word, −25 for a clutter word.
fn class_weight(dom: &Dom, id: NodeId) -> f64 {
    let words = words(dom, id);
    let has = |list: &[&str]| words.iter().any(|word| list.contains(&word.as_str()));
    (if has(POSITIVE) { 25.0 } else { 0.0 }) - (if has(NEGATIVE) { 25.0 } else { 0.0 })
}

fn text_length(dom: &Dom, id: NodeId) -> usize {
    let mut length = 0;
    let mut space = true;
    for c in dom.text_of(id).chars() {
        if c.is_whitespace() {
            if !space {
                length += 1;
            }
            space = true;
        } else {
            length += 1;
            space = false;
        }
    }
    length
}

/// How much of the text under `id` is link text, 0 to 1.
fn link_density(dom: &Dom, id: NodeId) -> f64 {
    let total = text_length(dom, id);
    if total == 0 {
        return 0.0;
    }
    let links: usize = dom
        .descendants(id)
        .into_iter()
        .filter(|&node| dom.is(node, "a"))
        .map(|node| text_length(dom, node))
        .sum();
    links as f64 / total as f64
}

fn under_any(dom: &Dom, id: NodeId, names: &[&str]) -> bool {
    std::iter::successors(dom.parent(id), |&node| dom.parent(node))
        .any(|node| dom.name(node).is_some_and(|name| names.contains(&name)))
}

/// The blocks to leave out wherever they are: asides, footers, the site's header, elements
/// named like clutter, and blocks that are mostly links.
fn clutter(dom: &Dom, body: NodeId) -> HashSet<NodeId> {
    let mut skip = HashSet::new();
    let mut stack: Vec<NodeId> = dom.children(body).iter().rev().copied().collect();
    while let Some(id) = stack.pop() {
        let Some(name) = dom.name(id) else {
            continue;
        };
        let words = words(dom, id);
        let has = |list: &[&str]| words.iter().any(|word| list.contains(&word.as_str()));
        let structural = matches!(name, "article" | "main");
        let unlikely = !structural && has(NEGATIVE) && !has(POSITIVE);
        let around = matches!(name, "aside" | "footer" | "nav")
            || (name == "header" && !under_any(dom, id, &["article", "main"]));
        let link_list = matches!(name, "ul" | "ol" | "div" | "section" | "table" | "p")
            && text_length(dom, id) > 0
            && link_density(dom, id) > 0.5;
        if unlikely || around || link_list {
            skip.insert(id);
        } else {
            stack.extend(dom.children(id).iter().rev().copied());
        }
    }
    skip
}

/// The blocks that hold the main content, in document order.
fn main_content(dom: &Dom, body: NodeId, skip: &HashSet<NodeId>) -> Vec<NodeId> {
    let mut scores: HashMap<NodeId, f64> = HashMap::new();
    let mut stack = vec![body];
    while let Some(id) = stack.pop() {
        if skip.contains(&id) {
            continue;
        }
        stack.extend(dom.children(id).iter().copied());
        let Some(name) = dom.name(id) else {
            continue;
        };
        // Paragraphs, and divs used as paragraphs (text with no blocks inside).
        let paragraph = matches!(name, "p" | "pre" | "td" | "blockquote")
            || (name == "div"
                && !dom
                    .children(id)
                    .iter()
                    .any(|&child| dom.name(child).is_some_and(is_block)));
        if !paragraph {
            continue;
        }
        let text = dom.text_of(id);
        let length = text_length(dom, id);
        if length < 25 {
            continue;
        }
        let score = 1.0 + text.matches([',', '，']).count() as f64 + (length / 100).min(3) as f64;
        let ancestors = std::iter::successors(dom.parent(id), |&node| dom.parent(node)).take(3);
        for (level, ancestor) in ancestors.enumerate() {
            if dom.name(ancestor).is_none() {
                break;
            }
            let entry = scores
                .entry(ancestor)
                .or_insert_with(|| initial_score(dom, ancestor));
            *entry += score / (level as f64 + 1.0);
        }
    }
    let Some((best, best_score)) = scores
        .iter()
        .map(|(&id, &score)| (id, score * (1.0 - link_density(dom, id))))
        .max_by(|a, b| a.1.total_cmp(&b.1).then(b.0.cmp(&a.0)))
    else {
        return vec![body];
    };
    let Some(parent) = dom.parent(best).filter(|_| best != body) else {
        return vec![best];
    };
    let threshold = (best_score * 0.2).max(10.0);
    dom.children(parent)
        .iter()
        .copied()
        .filter(|&sibling| {
            if sibling == best {
                return true;
            }
            if skip.contains(&sibling) || dom.name(sibling).is_none() {
                return false;
            }
            let score = scores
                .get(&sibling)
                .map(|&score| score * (1.0 - link_density(dom, sibling)));
            if score.is_some_and(|score| score >= threshold) {
                return true;
            }
            dom.is(sibling, "p") && {
                let length = text_length(dom, sibling);
                let density = link_density(dom, sibling);
                (length > 80 && density < 0.25)
                    || (length > 0
                        && density == 0.0
                        && dom.text_of(sibling).trim_end().ends_with('.'))
            }
        })
        .collect()
}

fn is_block(name: &str) -> bool {
    matches!(
        name,
        "address"
            | "article"
            | "aside"
            | "blockquote"
            | "div"
            | "dl"
            | "figure"
            | "footer"
            | "form"
            | "h1"
            | "h2"
            | "h3"
            | "h4"
            | "h5"
            | "h6"
            | "header"
            | "hr"
            | "main"
            | "nav"
            | "ol"
            | "p"
            | "pre"
            | "section"
            | "table"
            | "ul"
    )
}

fn initial_score(dom: &Dom, id: NodeId) -> f64 {
    let tag = match dom.name(id).unwrap_or("") {
        "article" => 10.0,
        "main" | "div" => 5.0,
        "pre" | "td" | "blockquote" => 3.0,
        "form" | "ul" | "ol" | "li" | "dl" | "dd" | "dt" | "address" => -3.0,
        "h1" | "h2" | "h3" | "h4" | "h5" | "h6" | "th" => -5.0,
        _ => 0.0,
    };
    tag + class_weight(dom, id)
}

fn metadata(dom: &Dom, url: &str) -> Page {
    let mut meta: HashMap<String, String> = HashMap::new();
    let mut title = None;
    for node in dom.descendants(dom.root()) {
        match dom.name(node) {
            Some("meta") => {
                let key = dom
                    .attr(node, "property")
                    .or_else(|| dom.attr(node, "name"))
                    .or_else(|| dom.attr(node, "itemprop"));
                if let (Some(key), Some(content)) = (key, dom.attr(node, "content")) {
                    let content = squeeze(content);
                    if !content.is_empty() {
                        meta.entry(key.to_ascii_lowercase()).or_insert(content);
                    }
                }
            }
            Some("title") if title.is_none() => {
                title = Some(squeeze(&dom.text_of(node))).filter(|title| !title.is_empty());
            }
            _ => {}
        }
        if let Data::Element { name, .. } = dom.data(node)
            && name == "body"
        {
            break;
        }
    }
    let first = |keys: &[&str]| keys.iter().find_map(|key| meta.get(*key).cloned());
    Page {
        title: first(&["og:title", "twitter:title"]).or(title),
        site: first(&["og:site_name", "application-name"]).or_else(|| host(url)),
        published: first(&["article:published_time", "datepublished", "date"]),
        markdown: String::new(),
    }
}

fn squeeze(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// `example.com` for `https://www.example.com:8080/a`.
fn host(url: &str) -> Option<String> {
    let rest = url.split_once("://")?.1;
    let authority = &rest[..rest.find(['/', '?', '#']).unwrap_or(rest.len())];
    let host = authority
        .rsplit_once('@')
        .map_or(authority, |(_, host)| host);
    let host = if host.starts_with('[') {
        host.find(']').map_or(host, |end| &host[..=end])
    } else {
        host.split(':').next().unwrap_or(host)
    };
    let host = host.strip_prefix("www.").unwrap_or(host);
    (!host.is_empty()).then(|| host.to_ascii_lowercase())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> String {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join(r"tests\fixtures\pages")
            .join(name);
        std::fs::read_to_string(path).unwrap().replace("\r\n", "\n")
    }

    fn check(name: &str, url: &str) -> Page {
        let page = extract(&fixture(&format!("{name}.html")), url);
        let expected = fixture(&format!("{name}.md"));
        assert_eq!(page.markdown, expected.trim_end(), "{name}");
        page
    }

    #[test]
    fn a_news_article_without_its_site_around_it() {
        let page = check("news", "https://www.dailyledger.example/tech/2026/10/lag");
        assert_eq!(
            page.title.as_deref(),
            Some("Payments platform hit by overnight lag")
        );
        assert_eq!(page.site.as_deref(), Some("The Daily Ledger"));
        assert_eq!(page.published.as_deref(), Some("2026-10-18T07:30:00Z"));
    }

    #[test]
    fn a_blog_post_in_divs() {
        let page = check("blog", "https://notes.example.org/posts/batch-size");
        assert_eq!(
            page.title.as_deref(),
            Some("Tuning batch sizes - Field notes")
        );
        assert_eq!(page.site.as_deref(), Some("notes.example.org"));
        assert_eq!(page.published, None);
    }

    #[test]
    fn a_docs_page_keeps_its_code_and_tables() {
        check("docs", "https://docs.example.com/consumer/lag.html");
    }

    #[test]
    fn a_page_of_links_has_no_main_text() {
        let page = extract(&fixture("links.html"), "https://example.com/");
        assert_eq!(page.markdown, "");
        assert_eq!(page.title.as_deref(), Some("Example portal"));
    }

    #[test]
    fn hosts() {
        assert_eq!(
            host("https://www.Example.com:8080/a"),
            Some("example.com".to_owned())
        );
        assert_eq!(host("http://user@[::1]:80/"), Some("[::1]".to_owned()));
        assert_eq!(host("nonsense"), None);
    }
}
