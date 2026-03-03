use std::time::Duration;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use std::{collections::HashSet, fs, path::PathBuf};
use reqwest::blocking::Client;

use crate::{config, discovery, storage, syncer};

const APP_NAME: &str = "nimbus";
const APP_USER_AGENT: &str = "nimbus/0.1";

#[derive(Debug, Parser)]
#[command(name = APP_NAME, version, about = "Nimbus feed reader for KDE + CLI")]
pub struct CliArgs {
    #[command(subcommand)]
    pub command: Option<Commands>,

    #[arg(long, short, default_value_t = 15)]
    pub interval_minutes: u64,

    #[arg(long)]
    pub start_minimized: bool,

    #[arg(long, default_value = "")]
    pub db: String,

    #[arg(long, global = true)]
    pub json: bool,
}

#[derive(Debug, Subcommand)]
pub enum Commands {
    /// Add a feed URL directly or discover from a website URL
    Add {
        /// Feed URL(s) or site URL(s) to add or discover
        #[arg(required = true)]
        urls: Vec<String>,
        #[arg(long)]
        discover: bool,
        #[arg(long)]
        title: Option<String>,
    },
    /// Remove a feed by id
    Remove {
        id: i64,
    },
    /// List configured feeds
    Feeds,
    /// Discover RSS/Atom links from a website
    Discover {
        url: String,
    },
    /// Force a sync now
    Sync,
    /// Run periodic sync loop in the background
    Daemon,
    /// List entries
    List {
        #[arg(long)]
        feed: Option<i64>,
        #[arg(long, default_value_t = 25)]
        limit: usize,
        #[arg(long)]
        unread_only: bool,
    },
    /// Search entries by text
    Search {
        query: String,
        #[arg(long)]
        feed: Option<i64>,
        #[arg(long, default_value_t = 25)]
        limit: usize,
        #[arg(long)]
        unread_only: bool,
    },
    /// Export configured site URLs for backup
    Export {
        /// Optional file path. If omitted, writes to stdout.
        output: Option<PathBuf>,
    },
    /// Import site URLs from a backup file and add/discover feeds.
    Import {
        /// Backup file path (one URL per line).
        input: PathBuf,
    },
    /// Mark an entry as read/unread
    Read {
        id: i64,
        #[arg(long)]
        unread: bool,
    },
    /// Open GUI window (default if no command)
    Gui,
}

pub fn run(args: CliArgs) -> Result<()> {
    let db_path = config::resolve_db_path(if args.db.is_empty() {
        None
    } else {
        Some(args.db.as_ref())
    })?;

    let store = storage::Store::open(&db_path)?;
    let client = Client::builder()
        .user_agent(APP_USER_AGENT)
        .timeout(Duration::from_secs(30))
        .build()?;

    match args.command.unwrap_or(Commands::Gui) {
        Commands::Gui => Ok(()),
        Commands::Add {
            urls,
            discover,
            title,
        } => {
            let mut added_count = 0usize;
            for src in urls {
                let discovered = if discover {
                    match discovery::discover_feed_urls(&src, &client) {
                        Ok(result) => result.feeds,
                        Err(err) => {
                            eprintln!("warn: discovery failed for {src} ({err}), trying source as a feed URL");
                            match discovery::normalize_url(&src) {
                                Ok(normalized) => vec![normalized],
                                Err(_) => continue,
                            }
                        }
                    }
                } else {
                    vec![discovery::normalize_url(&src)?]
                };

                for feed_url in discovered {
                    let added = store.add_feed(&feed_url, title.clone(), Some(src.clone()))?;
                    println!("added feed [{}] {}", added.id, added.url);
                    let _ = syncer::sync_single_feed(&store, &client, added.id);
                    added_count += 1;
                }
            }
            if added_count == 0 {
                println!("No new feeds added.");
            }
            Ok(())
        }
        Commands::Remove { id } => {
            let count = store.remove_feed(id)?;
            if count == 0 {
                println!("no feed with id {}", id);
            } else {
                println!("removed {}", id);
            }
            Ok(())
        }
        Commands::Feeds => {
            let feeds = store.list_feeds()?;
            if args.json {
                println!("{}", serde_json::to_string_pretty(&feeds)?);
                return Ok(());
            }
            if feeds.is_empty() {
                println!("No feeds configured. Add one with `{APP_NAME} add <url> --discover`.");
            } else {
                for f in feeds {
                    println!(
                        "{} | {} | {}",
                        f.id,
                        f.url,
                        f.title.unwrap_or_else(|| "(untitled)".to_string())
                    );
                }
            }
            Ok(())
        }
        Commands::Discover { url } => {
            let discovered = discovery::discover_feed_urls(&url, &client).context("discovering feed URLs")?;
            if args.json {
                println!("{}", serde_json::to_string_pretty(&discovered)?);
            } else {
                for found in discovered.feeds {
                    println!("{found}");
                }
            }
            Ok(())
        }
        Commands::Sync => {
            let report = syncer::sync_all_feeds(&store, &client);
            println!(
                "synced {} feeds, {} entries inspected, {} new, {} duplicates cleaned",
                report.processed_feeds,
                report.processed_entries,
                report.new_entries,
                report.deduped_entries
            );
            if !report.errors.is_empty() {
                eprintln!("errors:");
                for err in report.errors {
                    eprintln!("  - {err}");
                }
            }
            Ok(())
        }
        Commands::Daemon => loop {
            let report = syncer::sync_all_feeds(&store, &client);
            println!(
                "{}: synced {} feeds, {} entries inspected, {} new",
                chrono::Utc::now().format("%Y-%m-%d %H:%M:%S"),
                report.processed_feeds,
                report.processed_entries,
                report.new_entries
            );
            if report.deduped_entries > 0 {
                println!("{}: removed {} duplicate entries", chrono::Utc::now().format("%Y-%m-%d %H:%M:%S"), report.deduped_entries);
            }
            if !report.errors.is_empty() {
                for err in report.errors {
                    eprintln!("error: {err}");
                }
            }
            std::thread::sleep(Duration::from_secs(args.interval_minutes * 60));
        },
        Commands::List {
            feed,
            limit,
            unread_only,
        } => {
            let entries = store.list_entries(feed, unread_only, None, limit, 0)?;
            if args.json {
                println!("{}", serde_json::to_string_pretty(&entries)?);
            } else if entries.is_empty() {
                if unread_only {
                    println!("No unread entries.");
                } else {
                    println!("No entries found.");
                }
            } else {
                for e in entries {
                    println!("{}", format_entry_console(&e, true));
                }
            }
            Ok(())
        }
        Commands::Search {
            query,
            feed,
            limit,
            unread_only,
        } => {
            let entries = store.list_entries(feed, unread_only, Some(&query), limit, 0)?;
            if args.json {
                println!("{}", serde_json::to_string_pretty(&entries)?);
            } else if entries.is_empty() {
                println!("No entries match '{query}'.");
            } else {
                for e in entries {
                    println!("{}", format_entry_console(&e, false));
                }
            }
            Ok(())
        }
        Commands::Export { output } => {
            let feeds = store.list_feeds()?;
            let mut seen = HashSet::new();
            let mut lines: Vec<String> = Vec::new();

            for feed in feeds {
                let site = feed.site_url.unwrap_or_else(|| canonical_site_url(&feed.url));
                let key = site.to_lowercase();
                if seen.insert(key) {
                    lines.push(site);
                }
            }

            let payload = lines.join("\n");
            if let Some(path) = output {
                fs::write(&path, format!("{payload}\n")).with_context(|| format!("writing {}", path.display()))?;
                println!("exported {} site(s) to {}", lines.len(), path.display());
            } else {
                if lines.is_empty() {
                    println!("# no feeds configured");
                } else {
                    print!("{payload}");
                }
            }
            Ok(())
        }
        Commands::Import { input } => {
            let raw = fs::read_to_string(&input).with_context(|| format!("reading {}", input.display()))?;
            let mut attempted: usize = 0;
            let mut discovered: usize = 0;
            let mut fallback: usize = 0;

            for line in raw.lines() {
                let source = line.trim();
                if source.is_empty() || source.starts_with('#') {
                    continue;
                }
                let discover_targets = match discovery::discover_feed_urls(source, &client) {
                    Ok(found) => {
                        attempted += 1;
                        found.feeds
                    }
                    Err(err) => {
                        if args.json {
                            eprintln!("warn: discovery failed for {source}: {err}");
                        }
                        attempted += 1;
                        match discovery::normalize_url(source) {
                            Ok(normalized) => vec![normalized],
                            Err(_) => {
                                fallback += 1;
                                Vec::new()
                            }
                        }
                    }
                };

                if discover_targets.is_empty() {
                    fallback += 1;
                    continue;
                }

                for feed_url in discover_targets {
                    let _ = store.add_feed(&feed_url, None, Some(source.to_string()));
                    discovered += 1;
                }
            }

            if attempted == 0 {
                println!("no import sites found");
            } else {
                let fail_count = attempted.saturating_sub(discovered);
                println!(
                    "import complete: {attempted} site(s) processed, {discovered} feed URL(s) added/updated, {fallback} fallback/skipped, {fail_count} failed"
                );
            }
            Ok(())
        }
        Commands::Read { id, unread } => {
            store.mark_read(id, !unread)?;
            println!("updated {}", id);
            Ok(())
        }
    }
}

fn canonical_site_url(url: &str) -> String {
    let trimmed = url.trim().trim_end_matches('/');
    match url::Url::parse(trimmed) {
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


fn format_entry_console(entry: &storage::FeedEntry, include_body: bool) -> String {
    let state = if entry.read { "read" } else { "unread" };
    let t = storage::format_time(entry.published_at.or(Some(entry.inserted_at)));
    let body = if include_body {
        entry.summary.clone()
    } else {
        String::new()
    };
    format!(
        "[{state}] #{} [{}] {}\n  {} | {}\n  {}",
        entry.id,
        entry.feed_title.clone().unwrap_or_else(|| "unknown feed".to_string()),
        entry.title,
        t,
        entry.link,
        body,
    )
}
