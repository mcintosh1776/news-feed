use std::time::Duration;
use url::Url;

use anyhow::{Context, Result};
use reqwest::blocking::Client;

use crate::discovery;
use crate::feed_parser::parse_feed_xml;
use crate::storage::Store;

const READ_ENTRIES_RETENTION_HOURS: i64 = 24;

#[derive(Debug, Clone)]
pub struct SyncReport {
    pub processed_feeds: usize,
    pub processed_entries: usize,
    pub new_entries: usize,
    pub errors: Vec<String>,
}

pub fn sync_all_feeds(store: &Store, client: &Client) -> SyncReport {
    let mut report = SyncReport {
        processed_feeds: 0,
        processed_entries: 0,
        new_entries: 0,
        errors: vec![],
    };

    let feeds = match store.list_feeds() {
        Ok(feeds) => feeds,
        Err(err) => {
            report.errors.push(err.to_string());
            return report;
        }
    };

    for feed in feeds {
        match sync_feed(store, client, feed.id, &feed.url) {
            Ok((entry_count, inserted)) => {
                report.processed_feeds += 1;
                report.processed_entries += entry_count;
                report.new_entries += inserted;
            }
            Err(err) => report.errors.push(format!("{}", err)),
        }
    }

    if let Err(err) = store.prune_read_entries_older_than_hours(READ_ENTRIES_RETENTION_HOURS) {
        report.errors.push(format!("cleanup: {err}"));
    }

    report
}

pub fn sync_single_feed(store: &Store, client: &Client, feed_id: i64) -> Result<usize> {
    let feed = store
        .get_feed(feed_id)?
        .ok_or_else(|| anyhow::anyhow!("missing feed {}", feed_id))?;
    let (_entries, inserted) = sync_feed(store, client, feed_id, &feed.url).context("sync feed")?;
    let _ = store.prune_read_entries_older_than_hours(READ_ENTRIES_RETENTION_HOURS)?;
    Ok(inserted)
}

fn sync_feed(
    store: &Store,
    client: &Client,
    feed_id: i64,
    feed_url: &str,
) -> Result<(usize, usize)> {
    let result = sync_feed_once(store, client, feed_id, feed_url);
    if let Ok(report) = result {
        return Ok(report);
    }

    if !feed_url.starts_with("http://") && !feed_url.starts_with("https://") {
        return result;
    }

    if let Ok(found) = discovery::discover_feed_urls(feed_url, client) {
        if found.feeds.is_empty() {
            return result;
        }
        for discovered_url in found.feeds.into_iter() {
            if discovered_url == feed_url {
                continue;
            }

            if let Ok(report) = sync_feed_once(store, client, feed_id, &discovered_url) {
                let source = canonical_site_url(feed_url);
                let _ = store.update_feed_source(feed_id, &discovered_url, &source);
                return Ok(report);
            }
        }
    }

    result
}

fn sync_feed_once(
    store: &Store,
    client: &Client,
    feed_id: i64,
    feed_url: &str,
) -> Result<(usize, usize)> {
    let response = client
        .get(feed_url)
        .timeout(Duration::from_secs(25))
        .send()?;

    if !response.status().is_success() {
        anyhow::bail!("{}: {}", feed_url, response.status());
    }

    let body = response.text()?;
    let parsed = parse_feed_xml(&body)?;
    if let Some(feed_title) = parsed.title.as_ref() {
        let _ = store.set_feed_title(feed_id, feed_title);
    }

    let mut parsed_count = 0;
    let mut inserted_count = 0;

    for item in parsed.entries {
        parsed_count += 1;
        if item.id.is_empty() && item.link.is_empty() {
            continue;
        }

        let inserted = store.upsert_or_update_entry(
            feed_id,
            &item.id,
            &item.link,
            &item.title,
            &item.summary,
            &item.content,
            item.published.map(|ts| ts.timestamp()),
        )?;
        if inserted {
            inserted_count += 1;
        }
    }

    if parsed_count == 0 {
        anyhow::bail!("{}: no entries found", feed_url);
    }

    Ok((parsed_count, inserted_count))
}

fn canonical_site_url(url: &str) -> String {
    let trimmed = url.trim().trim_end_matches('/');
    match Url::parse(trimmed) {
        Ok(parsed) => {
            let host = match parsed.host_str() {
                Some(host) => host.to_string(),
                None => return trimmed.to_string(),
            };
            let mut root = format!("{}://{}", parsed.scheme(), host);
            if let Some(port) = parsed.port() {
                root.push(':');
                root.push_str(&port.to_string());
            }
            root
        }
        Err(_) => trimmed.to_string(),
    }
}
