use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use dirs;

pub fn default_db_path() -> Result<PathBuf> {
    let root = dirs::data_dir().or_else(dirs::home_dir).context("unable to resolve data dir")?;
    let base = root.join("nimbus");
    std::fs::create_dir_all(&base).context("creating app data dir")?;
    Ok(base.join("feeds.sqlite"))
}

pub fn resolve_db_path(path: Option<&Path>) -> Result<PathBuf> {
    match path {
        Some(explicit) => Ok(explicit.to_path_buf()),
        None => default_db_path(),
    }
}
