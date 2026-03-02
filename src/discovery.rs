use std::collections::BTreeSet;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use reqwest::blocking::Client;
use scraper::{Html, Selector};
use url::Url;

use crate::feed_parser::looks_like_feed;

#[derive(Clone, Debug, serde::Serialize)]
pub struct DiscoveryResult {
    pub feeds: Vec<String>,
}

pub fn normalize_url(input: &str) -> Result<String> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
            bail!("empty URL provided");
    }

    if trimmed.starts_with("http://") || trimmed.starts_with("https://") {
        Ok(trimmed.to_string())
    } else {
        Ok(format!("https://{}", trimmed))
    }
}

pub fn discover_feed_urls(site_or_feed: &str, client: &Client) -> Result<DiscoveryResult> {
    let base = normalize_url(site_or_feed)?;
    let mut found = BTreeSet::new();

    if looks_like_url(&base) {
        if verify_feed_url(&base, client).is_ok() {
            found.insert(base.to_string());
        }
    }

    for path in ["/feed", "/rss", "/rss.xml", "/atom", "/atom.xml", "/index.xml", "/feed.xml"] {
        let candidate = format!("{}{}", base.trim_end_matches('/'), path);
        if verify_feed_url(&candidate, client).is_ok() {
            found.insert(candidate);
        }
    }

    if let Ok(page) = fetch_body(&base, client) {
        let doc = Html::parse_document(&page);
        if let Ok(link_sel) = Selector::parse("link[rel='alternate']") {
            for node in doc.select(&link_sel) {
                if let Some(href) = node.value().attr("href") {
                    let rel = node.value().attr("rel").unwrap_or("").to_lowercase();
                    let typ = node.value().attr("type").unwrap_or("").to_lowercase();

                    let is_feed = rel.contains("alternate") && (typ.contains("rss") || typ.contains("atom") || typ.contains("xml"));
                    if !is_feed {
                        continue;
                    }

                    if let Ok(base_url) = Url::parse(&base) {
                        if let Ok(url) = base_url.join(href) {
                            let candidate = url.to_string();
                            if verify_feed_url(&candidate, client).is_ok() {
                                found.insert(candidate);
                            }
                        }
                    }
                }
            }
        }
    }

    if found.is_empty() {
        bail!("no feed endpoint found for {}", site_or_feed);
    }

    Ok(DiscoveryResult {
        feeds: found.into_iter().collect(),
    })
}

fn verify_feed_url(url: &str, client: &Client) -> Result<()> {
    let resp = client
        .get(url)
        .timeout(Duration::from_secs(10))
        .send()
        .with_context(|| format!("failed to fetch {url}"))?;

    if !resp.status().is_success() {
        bail!("non-success status {}", resp.status());
    }

    let bytes = resp.bytes()?;
    let body = String::from_utf8_lossy(&bytes);
    if !looks_like_feed(&body) {
        bail!("not a feed");
    }

    Ok(())
}

fn fetch_body(url: &str, client: &Client) -> Result<String> {
    let resp = client
        .get(url)
        .timeout(Duration::from_secs(8))
        .send()?;
    Ok(resp.text()?)
}

fn looks_like_url(candidate: &str) -> bool {
    Url::parse(candidate).is_ok()
}
