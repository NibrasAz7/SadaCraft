//! Shared helpers for the `sadacraft-mcp` and `sadacraft-cli` binaries:
//! locating the app database and opening it read-only.
//!
//! Lives in `src/bin/` because these bins are part of the `sadacraft`
//! package and the module is not reachable from `app_lib`.

use anyhow::{bail, Context, Result};
use sqlx::sqlite::SqliteConnectOptions;
use sqlx::SqlitePool;
use std::path::PathBuf;

/// App data directory (`%APPDATA%\com.sadacraft.app` on Windows).
/// Matches `app_lib`: `dirs::data_dir()/com.sadacraft.app`.
pub fn app_data_dir() -> Result<PathBuf> {
    dirs::data_dir()
        .map(|d| d.join("com.sadacraft.app"))
        .context("could not resolve the OS data directory")
}

/// Path to the meetings database (`meeting_minutes.sqlite`).
/// See `database::manager::new_from_app_handle`.
pub fn db_path() -> Result<PathBuf> {
    Ok(app_data_dir()?.join("meeting_minutes.sqlite"))
}

/// Open the meetings database in read-only mode. Works while the app is
/// running: the DB is in WAL mode, so readers never block the writer.
pub async fn connect_read_only() -> Result<SqlitePool> {
    let path = db_path()?;
    if !path.exists() {
        bail!(
            "database not found at {} — run the SadaCraft app at least once",
            path.display()
        );
    }
    let options = SqliteConnectOptions::new()
        .filename(&path)
        .read_only(true)
        .pragma("busy_timeout", "5000");
    SqlitePool::connect_with(options)
        .await
        .with_context(|| format!("failed to open {} read-only", path.display()))
}
