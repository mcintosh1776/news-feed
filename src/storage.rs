use anyhow::{Context, Result};
use chrono::{DateTime, TimeZone, Utc};
use rusqlite::{params, Connection, Error as SqliteError, ErrorCode, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use url::Url;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Feed {
    pub id: i64,
    pub url: String,
    pub title: Option<String>,
    pub site_url: Option<String>,
    pub added_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FeedEntry {
    pub id: i64,
    pub feed_id: i64,
    pub feed_title: Option<String>,
    pub feed_url: Option<String>,
    pub external_id: String,
    pub link: String,
    pub title: String,
    pub summary: String,
    pub content: String,
    pub published_at: Option<i64>,
    pub inserted_at: i64,
    pub read: bool,
}

pub struct Store {
    conn: Connection,
}

const DEFAULT_FEEDS: &[(&str, &str)] = &[
    ("https://atlas21.com/feed/", "Atlas21"),
    ("https://www.theblock.co/", "The Block"),
];

fn canonical_host(raw: &str) -> String {
    let trimmed = raw.trim().trim_end_matches('/');
    match Url::parse(trimmed) {
        Ok(url) => {
            let mut host = url.host_str().unwrap_or("").to_ascii_lowercase();
            if let Some(stripped) = host.strip_prefix("www.") {
                host = stripped.to_string();
            }
            if let Some(port) = url.port() {
                host.push(':');
                host.push_str(&port.to_string());
            }
            if host.is_empty() {
                trimmed.to_string()
            } else {
                host
            }
        }
        Err(_) => trimmed.to_string(),
    }
}

fn sanitize_title(value: Option<String>) -> Option<String> {
    match value {
        Some(v) => {
            let v = v.trim();
            if v.is_empty() {
                None
            } else {
                Some(v.to_string())
            }
        }
        None => None,
    }
}

fn feed_title_from_url(url: &str) -> String {
    let trimmed = url.trim().trim_end_matches('/');
    match Url::parse(trimmed) {
        Ok(url) => {
            let host = url.host_str().unwrap_or(trimmed).trim_start_matches("www.").to_string();
            if host.is_empty() {
                trimmed.to_string()
            } else {
                host
            }
        }
        Err(_) => trimmed.to_string(),
    }
}

fn same_host(lhs: &str, rhs: &str) -> bool {
    canonical_host(lhs).eq_ignore_ascii_case(&canonical_host(rhs))
}

fn row_to_feed(row: &rusqlite::Row<'_>) -> rusqlite::Result<Feed> {
    Ok(Feed {
        id: row.get(0)?,
        url: row.get(1)?,
        title: row.get(2)?,
        site_url: row.get(3)?,
        added_at: row.get(4)?,
    })
}

impl Store {
    pub fn open(path: &std::path::Path) -> Result<Self> {
        let conn = Connection::open(path).with_context(|| format!("opening sqlite db at {}", path.display()))?;
        let store = Self { conn };
        store.init()?;
        store.seed_default_feeds()?;
        store.dedupe_same_site_feeds()?;
        Ok(store)
    }

    fn dedupe_same_site_feeds(&self) -> Result<usize> {
        let mut stmt = self
            .conn
            .prepare("SELECT id, url, title, site_url, added_at FROM feeds")?;
        let all_feeds = stmt
            .query_map([], row_to_feed)?
            .collect::<Result<Vec<_>, _>>()?;

        let mut buckets: HashMap<String, Vec<Feed>> = HashMap::new();
        for feed in all_feeds {
            let key = canonical_host(&feed.url);
            if key.is_empty() {
                continue;
            }
            buckets.entry(key).or_default().push(feed);
        }

        let mut removed = 0usize;
        for mut cluster in buckets.into_values() {
            if cluster.len() < 2 {
                continue;
            }

            cluster.sort_by(|a, b| b.added_at.cmp(&a.added_at).then(b.id.cmp(&a.id)));
            let mut keeper = cluster.remove(0);
            for dup in cluster {
                let _ = self.conn.execute(
                    "DELETE FROM entries WHERE feed_id = ?1 AND external_id IN (SELECT external_id FROM entries WHERE feed_id = ?2)",
                    params![keeper.id, dup.id],
                )?;

                let _ = self.conn.execute(
                    "UPDATE entries SET feed_id = ?1 WHERE feed_id = ?2",
                    params![keeper.id, dup.id],
                )?;

                let mut keeper_title = keeper.title.clone().and_then(|title| if title.is_empty() { None } else { Some(title) });
                let mut keeper_site = keeper.site_url.clone();
                if keeper_title.is_none() {
                    keeper_title = dup.title.clone();
                }
                if keeper_site.is_none() {
                    keeper_site = dup.site_url.clone();
                }
                keeper.title = keeper_title;
                keeper.site_url = keeper_site;

                self.conn.execute(
                    "UPDATE feeds
                     SET title = COALESCE(?1, title),
                         site_url = COALESCE(?2, site_url)
                     WHERE id = ?3",
                    params![keeper.title, keeper.site_url, keeper.id],
                )?;

                let _ = self.conn.execute("DELETE FROM feeds WHERE id = ?1", params![dup.id])?;
                removed += 1;
            }
        }

        Ok(removed)
    }

    fn init(&self) -> Result<()> {
        self.conn.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS feeds (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                url TEXT NOT NULL UNIQUE,
                title TEXT,
                site_url TEXT,
                added_at INTEGER NOT NULL
            );

            CREATE TABLE IF NOT EXISTS entries (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                feed_id INTEGER NOT NULL,
                external_id TEXT NOT NULL,
                link TEXT NOT NULL,
                title TEXT NOT NULL,
                summary TEXT NOT NULL DEFAULT '',
                content TEXT NOT NULL DEFAULT '',
                published_at INTEGER,
                inserted_at INTEGER NOT NULL,
                read_at INTEGER,
                UNIQUE(feed_id, external_id),
                FOREIGN KEY(feed_id) REFERENCES feeds(id) ON DELETE CASCADE
            );

            CREATE INDEX IF NOT EXISTS idx_entries_feed_read_time ON entries(feed_id, read_at, published_at);
            CREATE INDEX IF NOT EXISTS idx_entries_published ON entries(published_at DESC);
            PRAGMA journal_mode = WAL;
            PRAGMA synchronous = NORMAL;
            "#,
        )?;
        Ok(())
    }

    pub fn seed_default_feeds(&self) -> Result<usize> {
        let mut inserted = 0usize;

        for (url, title) in DEFAULT_FEEDS.iter() {
            match self.conn.query_row(
                "SELECT 1 FROM feeds WHERE url = ?1",
                [url],
                |row| row.get::<_, i64>(0),
            ) {
                Ok(_) => {}
                Err(rusqlite::Error::QueryReturnedNoRows) => {
                    let _ = self.add_feed(url, Some((*title).to_string()), None)?;
                    inserted += 1;
                }
                Err(err) => return Err(anyhow::anyhow!(err)),
            }
        }

        Ok(inserted)
    }

    pub fn update_feed_source(&self, feed_id: i64, feed_url: &str, site_url: &str) -> Result<usize> {
        let count: usize = match self.conn.execute(
            "UPDATE feeds SET url = ?1, site_url = ?2 WHERE id = ?3",
            params![feed_url, site_url, feed_id],
        ) {
            Ok(count) => count,
            Err(err) => {
                if let SqliteError::SqliteFailure(inner, _) = &err {
                    if inner.code == ErrorCode::ConstraintViolation
                        && inner.extended_code == rusqlite::ffi::SQLITE_CONSTRAINT_UNIQUE
                    {
                        return Ok(0);
                    }
                }
                return Err(err.into());
            }
        };
        Ok(count)
    }

    pub fn set_feed_title(&self, feed_id: i64, title: &str) -> Result<usize> {
        let title = title.trim();
        if title.is_empty() {
            return Ok(0);
        }

        let count = self.conn.execute(
            "UPDATE feeds SET title = ?1 WHERE id = ?2",
            params![title, feed_id],
        )?;
        Ok(count)
    }

    pub fn add_feed(&self, url: &str, title: Option<String>, site_url: Option<String>) -> Result<Feed> {
        let now = Utc::now().timestamp();
        let title = sanitize_title(title).or_else(|| Some(feed_title_from_url(url)));

        let mut existing = self
            .conn
            .query_row(
                "SELECT id, url, title, site_url, added_at FROM feeds WHERE url = ?1",
                params![url],
                |row| {
                    Ok(Feed {
                        id: row.get(0)?,
                        url: row.get(1)?,
                        title: row.get(2)?,
                        site_url: row.get(3)?,
                        added_at: row.get(4)?,
                    })
                },
            )
            .optional()?;

        if let Some(mut feed) = existing.take() {
            if feed.title.is_none() && title.is_some() {
                self.conn.execute(
                    "UPDATE feeds SET title = ?1, site_url = COALESCE(?2, site_url) WHERE id = ?3",
                    params![title, site_url, feed.id],
                )?;
                feed.title = title;
                feed.site_url = site_url;
            }
            return Ok(feed);
        }

        if let Some(new_site) = site_url.clone() {
            let mut stmt = self.conn.prepare("SELECT id, url, title, site_url, added_at FROM feeds")?;
            let candidates = stmt.query_map([], row_to_feed)?;

            for existing in candidates {
                let existing = existing?;
                let same_feed_channel = same_host(&existing.url, url)
                    || same_host(&existing.url, &new_site)
                    || existing.site_url.as_deref().is_some_and(|site| same_host(site, url) || same_host(site, &new_site));

                if same_feed_channel {
                    let maybe_updated = if let Err(err) = self.conn.execute(
                        "UPDATE feeds
                         SET url = ?1,
                             title = COALESCE(?2, title),
                             site_url = COALESCE(?3, site_url)
                         WHERE id = ?4",
                        params![url, title, new_site, existing.id],
                    ) {
                        if let SqliteError::SqliteFailure(inner, _) = &err {
                            if inner.code == ErrorCode::ConstraintViolation
                                && inner.extended_code == rusqlite::ffi::SQLITE_CONSTRAINT_UNIQUE
                            {
                                false
                            } else {
                                return Err(err.into());
                            }
                        } else {
                            return Err(err.into());
                        }
                    } else {
                        true
                    };

                    let mut feed = existing;
                    if maybe_updated {
                        feed.url = url.to_string();
                    }
                    if title.is_some() {
                        feed.title = title;
                    }
                    if feed.site_url.is_none() {
                        feed.site_url = Some(new_site);
                    }
                    if !maybe_updated {
                        let _ = self.conn.execute(
                            "UPDATE feeds
                             SET title = COALESCE(?1, title),
                                 site_url = COALESCE(?2, site_url)
                             WHERE id = ?3",
                            params![feed.title, feed.site_url, feed.id],
                        )?;
                    }
                    return Ok(feed);
                }
            }
        }

        match self.conn.execute(
            "INSERT INTO feeds (url, title, site_url, added_at) VALUES (?1, ?2, ?3, ?4)",
            params![url, title, site_url, now],
        ) {
            Ok(_) => {
                let id = self.conn.last_insert_rowid();
                Ok(Feed {
                    id,
                    url: url.to_string(),
                    title,
                    site_url,
                    added_at: now,
                })
            }
            Err(err) => {
                if let SqliteError::SqliteFailure(err_inner, _) = &err {
                    if err_inner.code == ErrorCode::ConstraintViolation
                        && err_inner.extended_code == rusqlite::ffi::SQLITE_CONSTRAINT_UNIQUE
                    {
                        if let Some(feed) = self
                            .conn
                            .query_row(
                                "SELECT id, url, title, site_url, added_at FROM feeds WHERE url = ?1",
                                params![url],
                                |row| {
                                    Ok(Feed {
                                        id: row.get(0)?,
                                        url: row.get(1)?,
                                        title: row.get(2)?,
                                        site_url: row.get(3)?,
                                        added_at: row.get(4)?,
                                    })
                                },
                            )
                            .optional()?
                        {
                            return Ok(feed);
                        }
                    }
                }
                Err(err.into())
            }
        }
    }

    pub fn remove_feed(&self, id: i64) -> Result<usize> {
        let count = self.conn.execute("DELETE FROM feeds WHERE id = ?1", params![id])?;
        Ok(count)
    }

    pub fn list_feeds(&self) -> Result<Vec<Feed>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, url, title, site_url, added_at
             FROM feeds
             ORDER BY title IS NULL, title, added_at DESC",
        )?;

        let rows = stmt
            .query_map([], |row| {
                Ok(Feed {
                    id: row.get(0)?,
                    url: row.get(1)?,
                    title: row.get(2)?,
                    site_url: row.get(3)?,
                    added_at: row.get(4)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;

        Ok(rows)
    }

    pub fn get_feed(&self, id: i64) -> Result<Option<Feed>> {
        let feed = self
            .conn
            .query_row(
                "SELECT id, url, title, site_url, added_at FROM feeds WHERE id = ?1",
                params![id],
                |row| {
                    Ok(Feed {
                        id: row.get(0)?,
                        url: row.get(1)?,
                        title: row.get(2)?,
                        site_url: row.get(3)?,
                        added_at: row.get(4)?,
                    })
                },
            )
            .optional()?;
        Ok(feed)
    }

    pub fn upsert_entry(
        &self,
        feed_id: i64,
        external_id: &str,
        link: &str,
        title: &str,
        summary: &str,
        content: &str,
        published_at: Option<i64>,
    ) -> Result<bool> {
        let inserted = self.conn.execute(
            "INSERT INTO entries
             (feed_id, external_id, link, title, summary, content, published_at, inserted_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
             ON CONFLICT(feed_id, external_id) DO NOTHING",
            params![
                feed_id,
                external_id,
                link,
                title,
                summary,
                content,
                published_at,
                Utc::now().timestamp()
            ],
        )?;
        Ok(inserted > 0)
    }

    pub fn upsert_or_update_entry(
        &self,
        feed_id: i64,
        external_id: &str,
        link: &str,
        title: &str,
        summary: &str,
        content: &str,
        published_at: Option<i64>,
    ) -> Result<bool> {
        let mutated = self.conn.execute(
            "INSERT INTO entries
             (feed_id, external_id, link, title, summary, content, published_at, inserted_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
             ON CONFLICT(feed_id, external_id)
             DO UPDATE SET link = excluded.link,
                           title = excluded.title,
                           summary = excluded.summary,
                           content = excluded.content,
                           published_at = excluded.published_at",
            params![
                feed_id,
                external_id,
                link,
                title,
                summary,
                content,
                published_at,
                Utc::now().timestamp(),
            ],
        )?;
        Ok(mutated > 0)
    }

    pub fn mark_read(&self, entry_id: i64, read: bool) -> Result<usize> {
        let count = if read {
            self.conn.execute(
                "UPDATE entries SET read_at = ?1 WHERE id = ?2",
                params![Utc::now().timestamp(), entry_id],
            )?
        } else {
            self.conn.execute("UPDATE entries SET read_at = NULL WHERE id = ?1", params![entry_id])?
        };
        Ok(count)
    }

    pub fn prune_read_entries_older_than_hours(&self, max_age_hours: i64) -> Result<usize> {
        let cutoff = Utc::now()
            .timestamp()
            .saturating_sub(max_age_hours.saturating_mul(60 * 60));
        let count = self
            .conn
            .execute("DELETE FROM entries WHERE read_at IS NOT NULL AND read_at < ?1", params![cutoff])?;
        Ok(count)
    }

    pub fn unread_count(&self, feed_id: Option<i64>) -> Result<i64> {
        let count = if let Some(feed_id) = feed_id {
            self.conn.query_row(
                "SELECT COUNT(1) FROM entries WHERE read_at IS NULL AND feed_id = ?1",
                params![feed_id],
                |row| row.get(0),
            )?
        } else {
            self.conn
                .query_row("SELECT COUNT(1) FROM entries WHERE read_at IS NULL", [], |row| row.get(0))?
        };
        Ok(count)
    }

    pub fn list_entries(
        &self,
        feed_id: Option<i64>,
        unread_only: bool,
        search: Option<&str>,
        limit: usize,
        offset: usize,
    ) -> Result<Vec<FeedEntry>> {
        let mut query = String::from(
            "SELECT e.id, e.feed_id, e.external_id, e.link, e.title, e.summary, e.content, e.published_at, e.inserted_at, e.read_at, f.title, COALESCE(f.site_url, f.url)
             FROM entries e
             JOIN feeds f ON f.id = e.feed_id
             WHERE 1 = 1",
        );

        let mut params_vec: Vec<rusqlite::types::Value> = Vec::new();
        if let Some(feed_id) = feed_id {
            query.push_str(" AND e.feed_id = ?");
            params_vec.push(feed_id.into());
        }

        if unread_only {
            query.push_str(" AND e.read_at IS NULL");
        }

        if let Some(term) = search {
            query.push_str(" AND (e.title LIKE ? OR e.summary LIKE ? OR e.link LIKE ? OR f.title LIKE ?)");
            let wildcard = format!("%{}%", term);
            params_vec.extend([
                wildcard.clone().into(),
                wildcard.clone().into(),
                wildcard.clone().into(),
                wildcard.into(),
            ]);
        }

        query.push_str(" ORDER BY COALESCE(e.published_at, e.inserted_at) DESC LIMIT ? OFFSET ?");
        params_vec.push((limit as i64).into());
        params_vec.push((offset as i64).into());

        let mut stmt = self.conn.prepare(&query)?;
        let rows = stmt
            .query_map(rusqlite::params_from_iter(params_vec), |row| {
                let read_at: Option<i64> = row.get(9)?;
                Ok(FeedEntry {
                    id: row.get(0)?,
                    feed_id: row.get(1)?,
                    external_id: row.get(2)?,
                    link: row.get(3)?,
                    title: row.get(4)?,
                    summary: row.get(5)?,
                    content: row.get(6)?,
                    published_at: row.get(7)?,
                    inserted_at: row.get(8)?,
                    read: read_at.is_some(),
                    feed_title: row.get::<_, Option<String>>(10)?,
                    feed_url: Some(row.get::<_, String>(11)?),
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;

        Ok(rows)
    }

    pub fn latest_readable_timestamp(&self, ts: Option<i64>) -> Option<DateTime<Utc>> {
        ts.and_then(|v| Utc.timestamp_opt(v, 0).single())
    }

    pub fn count_feeds(&self) -> Result<i64> {
        let n: i64 = self.conn.query_row("SELECT COUNT(1) FROM feeds", [], |row| row.get(0))?;
        Ok(n)
    }
}

pub fn format_time(ts: Option<i64>) -> String {
    ts.and_then(|value| Utc.timestamp_opt(value, 0).single())
        .map(|dt| dt.format("%Y-%m-%d %H:%M").to_string())
        .unwrap_or_else(|| "unknown".to_string())
}
