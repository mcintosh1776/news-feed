use anyhow::Result;
use chrono::{DateTime, Utc};
use std::hash::{Hash, Hasher};
use std::collections::hash_map::DefaultHasher;
use roxmltree::Document;

#[derive(Debug, Clone)]
pub struct ParsedEntry {
    pub id: String,
    pub title: String,
    pub link: String,
    pub summary: String,
    pub content: String,
    pub published: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone)]
pub struct ParsedFeed {
    pub title: Option<String>,
    pub entries: Vec<ParsedEntry>,
}

pub fn parse_feed_xml(xml: &str) -> Result<ParsedFeed> {
    let document = Document::parse(xml)?;
    let root = document.root_element();

    let entry_tag = if root.descendants().any(|node| node.is_element() && node.tag_name().name() == "item") {
        "item"
    } else {
        "entry"
    };

    let feed_title = root
        .descendants()
        .find(|node| node.is_element() && node.tag_name().name() == "title")
        .and_then(|node| node.text())
        .map(trimmed)
        .or_else(|| find_tag_text(root, "title"));

    let entries = if entry_tag == "item" {
        root.descendants()
            .filter(|node| node.tag_name().name() == "item")
            .filter_map(parse_rss_item)
            .collect()
    } else {
        root.descendants()
            .filter(|node| node.tag_name().name() == "entry")
            .filter_map(parse_atom_entry)
            .collect()
    };

    Ok(ParsedFeed {
        title: feed_title,
        entries,
    })
}

fn parse_rss_item(node: roxmltree::Node<'_, '_>) -> Option<ParsedEntry> {
    let title = find_tag_text_node(node, "title").unwrap_or_else(|| "Untitled item".to_string());
    let link = find_tag_text_node(node, "link").unwrap_or_default();
    let id = find_tag_text_node(node, "guid").unwrap_or_else(|| link.clone());
    let summary = find_tag_text_node(node, "description").unwrap_or_default();
    let content = find_tag_text_node(node, "content:encoded")
        .or_else(|| find_tag_text_node(node, "content"))
        .or_else(|| Some(summary.clone()))
        .unwrap_or_default();

    let published = find_tag_text_node(node, "pubDate")
        .and_then(|raw| parse_datetime(&raw))
        .or_else(|| find_tag_text_node(node, "date").and_then(|raw| parse_datetime(&raw)));

    Some(ParsedEntry {
        id,
        title,
        link,
        summary,
        content,
        published,
    })
}

fn parse_atom_entry(node: roxmltree::Node<'_, '_>) -> Option<ParsedEntry> {
    let title = find_tag_text_node(node, "title").unwrap_or_else(|| "Untitled entry".to_string());
    let link = node
        .children()
        .find(|n| n.is_element() && n.tag_name().name() == "link")
        .and_then(|n| n.attribute("href"))
        .map(|s| s.to_string())
        .or_else(|| find_tag_text_node(node, "link"))
        .unwrap_or_default();

    let published = find_tag_text_node(node, "published")
        .and_then(|raw| parse_datetime(&raw))
        .or_else(|| find_tag_text_node(node, "updated").and_then(|raw| parse_datetime(&raw)));
    let id = find_tag_text_node(node, "id").unwrap_or_else(|| {
        fallback_entry_id(&link, &title, published.as_ref().map(|dt| dt.to_rfc3339()))
    });
    let summary = find_tag_text_node(node, "summary").unwrap_or_default();
    let content = find_tag_text_node(node, "content").unwrap_or_else(|| summary.clone());

    Some(ParsedEntry {
        id,
        title,
        link,
        summary,
        content,
        published,
    })
}

fn find_tag_text_node(node: roxmltree::Node<'_, '_>, tag: &str) -> Option<String> {
    if let Some((name, local)) = tag.split_once(':') {
        node.children().find(|child| {
            child.is_element() && child.tag_name().name() == local && child.tag_name().namespace().map(|ns| ns.ends_with(name)).unwrap_or(false)
        }).and_then(|n| n.text()).map(trimmed)
    } else {
        node.children()
            .find(|child| child.is_element() && child.tag_name().name() == tag)
            .and_then(|n| n.text())
            .map(trimmed)
    }
}

fn find_tag_text(root: roxmltree::Node<'_, '_>, direct_child: &str) -> Option<String> {
    root.children()
        .find(|child| child.is_element() && child.tag_name().name() == direct_child)
        .and_then(|node| node.text())
        .map(trimmed)
}

fn parse_datetime(raw: &str) -> Option<DateTime<Utc>> {
    chrono::DateTime::parse_from_rfc2822(raw)
        .or_else(|_| chrono::DateTime::parse_from_rfc3339(raw))
        .ok()
        .map(|dt| dt.with_timezone(&Utc))
}

fn trimmed(value: &str) -> String {
    value.trim().to_string()
}

fn random_id() -> String {
    let mut hasher = DefaultHasher::new();
    Utc::now().timestamp_millis().hash(&mut hasher);
    format!("entry-{:016x}", hasher.finish())
}

fn fallback_entry_id(link: &str, title: &str, published: Option<String>) -> String {
    let mut hasher = DefaultHasher::new();
    let mut seed = String::new();
    if !link.trim().is_empty() {
        seed.push_str(link.trim());
    } else if !title.trim().is_empty() {
        seed.push_str(title.trim());
    } else if let Some(published) = &published {
        seed.push_str(published);
    } else {
        return random_id();
    }

    if let Some(published) = published {
        seed.push('|');
        seed.push_str(&published);
    }

    seed.hash(&mut hasher);
    format!("entry-{:016x}", hasher.finish())
}

pub fn looks_like_feed(xml: &str) -> bool {
    parse_feed_xml(xml).map_or(false, |feed| !feed.entries.is_empty())
}
