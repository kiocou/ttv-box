//! SQLite-backed application storage.

use std::path::Path;
use std::sync::{Mutex, MutexGuard};

use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::AppError;

const LATEST_SCHEMA_VERSION: i64 = 3;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct MediaRecord {
    pub id: String,
    pub account_id: Option<String>,
    pub library_id: Option<String>,
    pub kind: String,
    pub title: String,
    pub original_title: Option<String>,
    pub sort_key: Option<String>,
    pub year: Option<i64>,
    pub art_url: Option<String>,
    pub backdrop_url: Option<String>,
    pub rating: Option<f64>,
    pub duration_seconds: Option<i64>,
    pub source_type: String,
    pub remote_path: Option<String>,
    pub payload: Option<Value>,
}

impl MediaRecord {
    pub fn new(id: impl Into<String>, kind: impl Into<String>, title: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            account_id: None,
            library_id: None,
            kind: kind.into(),
            title: title.into(),
            original_title: None,
            sort_key: None,
            year: None,
            art_url: None,
            backdrop_url: None,
            rating: None,
            duration_seconds: None,
            source_type: "local".to_owned(),
            remote_path: None,
            payload: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct WatchHistory {
    pub id: i64,
    pub media_id: String,
    pub position_seconds: f64,
    pub duration_seconds: f64,
    pub completed: bool,
    pub watched_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Favorite {
    pub media_id: String,
    pub created_at: String,
}

/// Provider account metadata. Tokens are intentionally absent.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ProviderRecord {
    pub id: String,
    pub provider_type: String,
    pub account_id: Option<String>,
    pub display_name: Option<String>,
    pub metadata: Option<Value>,
    pub enabled: bool,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct MediaFilter<'a> {
    pub account_id: Option<&'a str>,
    pub library_id: Option<&'a str>,
    pub kind: Option<&'a str>,
}

pub struct Database {
    connection: Mutex<Connection>,
}

impl std::fmt::Debug for Database {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Database").finish_non_exhaustive()
    }
}

/// Probe the cwd-relative locations where `.ttv-data` databases have historically
/// landed, so migration finds the library no matter where the app is launched from.
fn legacy_database_candidates(data_dir: &Path) -> Vec<std::path::PathBuf> {
    let target = data_dir.join("ttv.db");
    let mut roots: Vec<std::path::PathBuf> = Vec::new();
    if let Ok(mut dir) = std::env::current_dir() {
        // Walk up a few levels and also probe the `src-tauri` layout at each, which
        // covers launching from the project root, from `src-tauri`, and from inside
        // the various `target` directories.
        for _ in 0..4 {
            roots.push(dir.clone());
            roots.push(dir.join("src-tauri"));
            if !dir.pop() {
                break;
            }
        }
    }
    let mut candidates = Vec::new();
    for root in roots {
        for sub in [
            std::path::PathBuf::from(".ttv-data"),
            std::path::PathBuf::from("target/debug/.ttv-data"),
            std::path::PathBuf::from("target/release/.ttv-data"),
        ] {
            let candidate = root.join(&sub).join("ttv.db");
            if candidate != target {
                candidates.push(candidate);
            }
        }
    }
    candidates
}

/// Count `media` rows in a database file, used to pick the richest legacy library.
fn count_media_rows(path: &Path) -> Result<i64, rusqlite::Error> {
    let connection = Connection::open(path)?;
    connection.query_row("SELECT COUNT(*) FROM media", [], |row| row.get(0))
}

impl Database {
    pub fn open(path: impl AsRef<Path>) -> Result<Self, AppError> {
        let path = path.as_ref();
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent)?;
            }
        }
        let database = Self {
            connection: Mutex::new(Connection::open(path)?),
        };
        database.configure()?;
        database.migrate()?;
        Ok(database)
    }

    pub fn open_in_memory() -> Result<Self, AppError> {
        let database = Self {
            connection: Mutex::new(Connection::open_in_memory()?),
        };
        database.configure()?;
        database.migrate()?;
        Ok(database)
    }

    /// Migrate the most-populated legacy `.ttv-data/ttv.db` into `data_dir`.
    ///
    /// Intended to run once, before [`Database::open`], when the data directory
    /// moved away from the old cwd-relative `.ttv-data`. It is a no-op as soon as
    /// the stable data directory already contains a database. The source file is
    /// copied with `VACUUM INTO` (a consistent snapshot even for a live WAL
    /// database) and left untouched in place, so it doubles as a backup.
    pub fn migrate_legacy_into(data_dir: &Path) {
        let target = data_dir.join("ttv.db");
        if target.exists() {
            return;
        }
        let mut best: Option<(std::path::PathBuf, i64)> = None;
        for candidate in legacy_database_candidates(data_dir) {
            if !candidate.is_file() {
                continue;
            }
            match count_media_rows(&candidate) {
                Ok(count) if count > 0 => {
                    if best.as_ref().map_or(true, |(_, current)| count > *current) {
                        best = Some((candidate, count));
                    }
                }
                Ok(_) => {}
                Err(error) => {
                    tracing::debug!(
                        error = %error,
                        path = %candidate.display(),
                        "skipping unreadable legacy database during migration"
                    );
                }
            }
        }
        let Some((source, count)) = best else {
            tracing::info!(
                "no legacy TTV database found to migrate; starting with an empty library"
            );
            return;
        };
        if let Err(error) = std::fs::create_dir_all(data_dir) {
            tracing::warn!(error = %error, "migration: could not create the stable data directory");
            return;
        }
        let copied = Connection::open(&source).and_then(|connection| {
            let target_path = target.to_string_lossy().into_owned();
            connection.execute("VACUUM INTO ?", params![target_path])
        });
        match copied {
            Ok(_) => tracing::info!(
                source = %source.display(),
                target = %target.display(),
                records = count,
                "migrated legacy TTV database into the stable data directory"
            ),
            Err(error) => {
                tracing::warn!(error = %error, "migration failed; starting with an empty library");
                let _ = std::fs::remove_file(&target);
            }
        }
    }

    fn lock(&self) -> Result<MutexGuard<'_, Connection>, AppError> {
        self.connection
            .lock()
            .map_err(|_| AppError::Storage("database mutex poisoned".to_owned()))
    }

    fn configure(&self) -> Result<(), AppError> {
        let connection = self.lock()?;
        connection.pragma_update(None, "journal_mode", "WAL")?;
        connection.pragma_update(None, "synchronous", "NORMAL")?;
        connection.pragma_update(None, "foreign_keys", "ON")?;
        connection.pragma_update(None, "busy_timeout", 30_000i64)?;
        connection.pragma_update(None, "temp_store", "MEMORY")?;
        Ok(())
    }

    fn migrate(&self) -> Result<(), AppError> {
        let mut connection = self.lock()?;
        connection.execute_batch("CREATE TABLE IF NOT EXISTS schema_migrations (version INTEGER PRIMARY KEY, applied_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP);")?;
        let mut current: i64 = connection.query_row(
            "SELECT COALESCE(MAX(version), 0) FROM schema_migrations",
            [],
            |row| row.get(0),
        )?;
        if current < 1 {
            let tx = connection.transaction()?;
            tx.execute_batch(
                "CREATE TABLE IF NOT EXISTS media (
                    id TEXT PRIMARY KEY, account_id TEXT, library_id TEXT, kind TEXT NOT NULL,
                    title TEXT NOT NULL, original_title TEXT, sort_key TEXT, year INTEGER,
                    art_url TEXT, backdrop_url TEXT, rating REAL, duration_seconds INTEGER,
                    source_type TEXT NOT NULL DEFAULT 'local', remote_path TEXT, payload TEXT,
                    created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
                    updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
                );
                CREATE INDEX IF NOT EXISTS idx_media_page ON media(account_id, library_id, kind, sort_key, id);
                CREATE INDEX IF NOT EXISTS idx_media_source_path ON media(source_type, remote_path);
                CREATE VIRTUAL TABLE IF NOT EXISTS media_fts USING fts5(title, original_title, id UNINDEXED, tokenize='unicode61');
                CREATE TRIGGER IF NOT EXISTS trg_media_insert AFTER INSERT ON media BEGIN
                    INSERT INTO media_fts(id, title, original_title) VALUES (new.id, new.title, new.original_title);
                END;
                CREATE TRIGGER IF NOT EXISTS trg_media_delete AFTER DELETE ON media BEGIN
                    DELETE FROM media_fts WHERE id = old.id;
                END;
                CREATE TRIGGER IF NOT EXISTS trg_media_update AFTER UPDATE ON media BEGIN
                    DELETE FROM media_fts WHERE id = old.id;
                    INSERT INTO media_fts(rowid, title, original_title, id)
                    VALUES (new.rowid, new.title, new.original_title, new.id);
                END;
                CREATE TABLE IF NOT EXISTS watch_history (
                    id INTEGER PRIMARY KEY AUTOINCREMENT, media_id TEXT NOT NULL,
                    position_seconds REAL NOT NULL, duration_seconds REAL NOT NULL,
                    completed INTEGER NOT NULL DEFAULT 0 CHECK (completed IN (0, 1)),
                    watched_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
                    FOREIGN KEY (media_id) REFERENCES media(id) ON DELETE CASCADE
                );
                CREATE INDEX IF NOT EXISTS idx_history_media ON watch_history(media_id, watched_at DESC);
                CREATE TABLE IF NOT EXISTS favorites (
                    media_id TEXT PRIMARY KEY, created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
                    FOREIGN KEY (media_id) REFERENCES media(id) ON DELETE CASCADE
                );
                CREATE TABLE IF NOT EXISTS providers (
                    id TEXT PRIMARY KEY, provider_type TEXT NOT NULL, account_id TEXT,
                    display_name TEXT, metadata TEXT, enabled INTEGER NOT NULL DEFAULT 1 CHECK (enabled IN (0, 1)),
                    created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
                    updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
                );
                CREATE TABLE IF NOT EXISTS kv (
                    key TEXT PRIMARY KEY, value TEXT NOT NULL,
                    updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
                );
                INSERT INTO schema_migrations(version) VALUES (1);",
            )?;
            tx.commit()?;
            current = 1;
        }
        if current < 2 {
            let tx = connection.transaction()?;
            // Match the library ordering expression so SQLite can satisfy the
            // first-page query from the index instead of sorting the full table.
            tx.execute_batch(
                "CREATE INDEX IF NOT EXISTS idx_media_order ON media(COALESCE(sort_key, title), id);
                 CREATE INDEX IF NOT EXISTS idx_media_page_order ON media(account_id, library_id, kind, COALESCE(sort_key, title), id);
                 INSERT INTO schema_migrations(version) VALUES (2);",
            )?;
            tx.commit()?;
            current = 2;
        }
        if current < 3 {
            let tx = connection.transaction()?;
            tx.execute_batch(
                "CREATE TABLE IF NOT EXISTS metadata_cache (
                    cache_key TEXT PRIMARY KEY,
                    provider TEXT NOT NULL,
                    payload TEXT NOT NULL,
                    expires_at INTEGER NOT NULL,
                    created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
                );
                CREATE INDEX IF NOT EXISTS idx_metadata_cache_expiry ON metadata_cache(expires_at);
                INSERT INTO schema_migrations(version) VALUES (3);",
            )?;
            tx.commit()?;
            current = 3;
        }
        if current > LATEST_SCHEMA_VERSION {
            return Err(AppError::Storage(format!("database schema version {current} is newer than supported version {LATEST_SCHEMA_VERSION}")));
        }
        Ok(())
    }

    pub fn schema_version(&self) -> Result<i64, AppError> {
        let connection = self.lock()?;
        Ok(connection.query_row(
            "SELECT COALESCE(MAX(version), 0) FROM schema_migrations",
            [],
            |row| row.get(0),
        )?)
    }

    pub fn upsert_media(&self, media: &MediaRecord) -> Result<(), AppError> {
        let connection = self.lock()?;
        upsert_media_on_connection(&connection, media)?;
        Ok(())
    }

    pub fn upsert_media_batch(&self, media_items: &[MediaRecord]) -> Result<(), AppError> {
        if media_items.is_empty() {
            return Ok(());
        }
        let mut connection = self.lock()?;
        let transaction = connection.transaction()?;
        for media in media_items {
            upsert_media_on_connection(&transaction, media)?;
        }
        transaction.commit()?;
        Ok(())
    }

    pub fn get_media(&self, id: &str) -> Result<Option<MediaRecord>, AppError> {
        let connection = self.lock()?;
        connection.query_row(
            "SELECT id, account_id, library_id, kind, title, original_title, sort_key, year, art_url, backdrop_url, rating, duration_seconds, source_type, remote_path, payload FROM media WHERE id = ?1",
            [id], row_to_media,
        ).optional().map_err(AppError::from)
    }

    pub fn list_media(
        &self,
        filter: MediaFilter<'_>,
        limit: u32,
        offset: u32,
    ) -> Result<Vec<MediaRecord>, AppError> {
        let connection = self.lock()?;
        let mut sql = String::from(
            "SELECT id, account_id, library_id, kind, title, original_title, sort_key, year, art_url, backdrop_url, rating, duration_seconds, source_type, remote_path, payload FROM media",
        );
        let mut predicates = Vec::with_capacity(4);
        let mut values: Vec<rusqlite::types::Value> = Vec::with_capacity(5);
        // 连续两轮刮削全败的条目（payload.scraped=false）在界面隐藏：
        // 文件保留在库中，后台低优先级重试命中后自动重新出现。
        predicates.push(
            "(payload IS NULL OR json_extract(payload, '$.scraped') IS NULL OR json_extract(payload, '$.scraped') != 0)"
                .to_string(),
        );
        predicates.push(
            "(payload IS NULL OR json_extract(payload, '$.promotional') IS NULL OR json_extract(payload, '$.promotional') != 1)"
                .to_string(),
        );
        if let Some(account_id) = filter.account_id {
            predicates.push("account_id = ?".to_string());
            values.push(rusqlite::types::Value::Text(account_id.to_owned()));
        }
        if let Some(library_id) = filter.library_id {
            predicates.push("library_id = ?".to_string());
            values.push(rusqlite::types::Value::Text(library_id.to_owned()));
        }
        if let Some(kind) = filter.kind {
            predicates.push("kind = ?".to_string());
            values.push(rusqlite::types::Value::Text(kind.to_owned()));
        }
        if !predicates.is_empty() {
            sql.push_str(" WHERE ");
            sql.push_str(&predicates.join(" AND "));
        }
        sql.push_str(" ORDER BY COALESCE(sort_key, title), id LIMIT ? OFFSET ?");
        values.push(rusqlite::types::Value::Integer(i64::from(limit)));
        values.push(rusqlite::types::Value::Integer(i64::from(offset)));
        let mut statement = connection.prepare(&sql)?;
        let rows = statement.query_map(rusqlite::params_from_iter(values.iter()), row_to_media)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(AppError::from)
    }

    /// Page through media that have not been scraped yet (no `payload.scrapedBy`).
    ///
    /// Used by the scraper when `overwrite` is false so repeated runs chip away
    /// at the unscraped backlog instead of re-processing the same first page of
    /// already-scraped rows. Ordered by `id` for stable offset pagination.
    pub fn list_media_unscraped(
        &self,
        limit: u32,
        offset: u32,
    ) -> Result<Vec<MediaRecord>, AppError> {
        let connection = self.lock()?;
        let sql = "SELECT id, account_id, library_id, kind, title, original_title, sort_key, year, art_url, backdrop_url, rating, duration_seconds, source_type, remote_path, payload FROM media WHERE (payload IS NULL OR json_extract(payload, '$.scrapedBy') IS NULL) AND (payload IS NULL OR json_extract(payload, '$.promotional') IS NULL OR json_extract(payload, '$.promotional') != 1) ORDER BY id LIMIT ? OFFSET ?";
        let mut statement = connection.prepare(sql)?;
        let rows = statement.query_map(
            rusqlite::params_from_iter(
                [
                    rusqlite::types::Value::Integer(i64::from(limit)),
                    rusqlite::types::Value::Integer(i64::from(offset)),
                ]
                .iter(),
            ),
            row_to_media,
        )?;
        rows.collect::<Result<Vec<_>, _>>().map_err(AppError::from)
    }

    pub fn search_media(
        &self,
        query: &str,
        limit: u32,
        offset: u32,
    ) -> Result<Vec<MediaRecord>, AppError> {
        let connection = self.lock()?;
        let mut statement = connection.prepare(
            "SELECT m.id, m.account_id, m.library_id, m.kind, m.title, m.original_title, m.sort_key, m.year, m.art_url, m.backdrop_url, m.rating, m.duration_seconds, m.source_type, m.remote_path, m.payload
             FROM media_fts f JOIN media m ON m.id = f.id WHERE media_fts MATCH ?1
               AND (m.payload IS NULL OR json_extract(m.payload, '$.scraped') IS NULL OR json_extract(m.payload, '$.scraped') != 0)
               AND (m.payload IS NULL OR json_extract(m.payload, '$.promotional') IS NULL OR json_extract(m.payload, '$.promotional') != 1)
             ORDER BY bm25(media_fts), COALESCE(m.sort_key, m.title), m.id LIMIT ?2 OFFSET ?3",
        )?;
        let rows = statement.query_map(params![query, limit, offset], row_to_media)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(AppError::from)
    }

    pub fn media_count(&self) -> Result<u64, AppError> {
        let connection = self.lock()?;
        Ok(
            connection.query_row("SELECT COUNT(*) FROM media WHERE payload IS NULL OR json_extract(payload, '$.promotional') IS NULL OR json_extract(payload, '$.promotional') != 1", [], |row| row.get::<_, i64>(0))?
                as u64,
        )
    }

    /// Read all media rows for maintenance jobs. Unlike the user-facing
    /// listing, this intentionally includes hidden and failed records.
    pub fn list_media_raw(&self, limit: u32, offset: u32) -> Result<Vec<MediaRecord>, AppError> {
        let connection = self.lock()?;
        let mut statement = connection.prepare(
            "SELECT id, account_id, library_id, kind, title, original_title, sort_key, year, art_url, backdrop_url, rating, duration_seconds, source_type, remote_path, payload FROM media ORDER BY id LIMIT ?1 OFFSET ?2",
        )?;
        let rows = statement.query_map(params![limit, offset], row_to_media)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(AppError::from)
    }

    pub fn save_watch_history(
        &self,
        media_id: &str,
        position_seconds: f64,
        duration_seconds: f64,
        completed: bool,
    ) -> Result<i64, AppError> {
        if media_id.trim().is_empty() {
            return Err(AppError::InvalidInput(
                "media_id cannot be empty".to_owned(),
            ));
        }
        if !position_seconds.is_finite()
            || !duration_seconds.is_finite()
            || position_seconds.is_sign_negative()
            || duration_seconds.is_sign_negative()
        {
            return Err(AppError::InvalidInput(
                "playback positions must be finite and non-negative".to_owned(),
            ));
        }
        let connection = self.lock()?;
        connection.execute("INSERT INTO watch_history(media_id, position_seconds, duration_seconds, completed) VALUES (?1, ?2, ?3, ?4)", params![media_id, position_seconds, duration_seconds, completed])?;
        Ok(connection.last_insert_rowid())
    }

    pub fn latest_watch_history(&self, media_id: &str) -> Result<Option<WatchHistory>, AppError> {
        let connection = self.lock()?;
        connection.query_row(
            "SELECT id, media_id, position_seconds, duration_seconds, completed, watched_at FROM watch_history WHERE media_id = ?1 ORDER BY watched_at DESC, id DESC LIMIT 1",
            [media_id], |row| Ok(WatchHistory { id: row.get(0)?, media_id: row.get(1)?, position_seconds: row.get(2)?, duration_seconds: row.get(3)?, completed: row.get::<_, i64>(4)? != 0, watched_at: row.get(5)? }),
        ).optional().map_err(AppError::from)
    }

    pub fn clear_media(&self) -> Result<u64, AppError> {
        let connection = self.lock()?;
        let deleted = connection.execute("DELETE FROM media", [])?;
        Ok(deleted as u64)
    }

    pub fn delete_media(&self, media_id: &str) -> Result<bool, AppError> {
        if media_id.trim().is_empty() {
            return Err(AppError::InvalidInput(
                "media_id cannot be empty".to_owned(),
            ));
        }
        let connection = self.lock()?;
        let deleted = connection.execute("DELETE FROM media WHERE id = ?1", [media_id])?;
        Ok(deleted > 0)
    }

    pub fn delete_media_by_source(&self, source_type: &str) -> Result<u64, AppError> {
        let source_type = source_type.trim();
        if source_type.is_empty() {
            return Err(AppError::InvalidInput(
                "source_type cannot be empty".to_owned(),
            ));
        }
        let connection = self.lock()?;
        let deleted =
            connection.execute("DELETE FROM media WHERE source_type = ?1", [source_type])?;
        Ok(deleted as u64)
    }

    /// 删除来源时先统计总量，给前端的进度条提供分母。
    pub fn count_media_by_source(&self, source_type: &str) -> Result<u64, AppError> {
        let source_type = source_type.trim();
        if source_type.is_empty() {
            return Err(AppError::InvalidInput(
                "source_type cannot be empty".to_owned(),
            ));
        }
        let connection = self.lock()?;
        let count: i64 = connection.query_row(
            "SELECT COUNT(*) FROM media WHERE source_type = ?1",
            [source_type],
            |row| row.get(0),
        )?;
        Ok(count.max(0) as u64)
    }

    /// 只取 id 和 art_url，不反序列化 payload。整库 list_media 会把每条
    /// payload（可达数 KB）都读进内存，大库上会长时间占住连接锁，
    /// 把刷新时的同步分页查询全部堵在主线程上（表现为窗口"未响应"）。
    pub fn list_media_art_by_source(
        &self,
        source_type: &str,
    ) -> Result<Vec<(String, Option<String>)>, AppError> {
        let source_type = source_type.trim();
        if source_type.is_empty() {
            return Err(AppError::InvalidInput(
                "source_type cannot be empty".to_owned(),
            ));
        }
        let connection = self.lock()?;
        let mut statement =
            connection.prepare("SELECT id, art_url FROM media WHERE source_type = ?1")?;
        let rows = statement.query_map([source_type], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?))
        })?;
        let mut result = Vec::new();
        for row in rows {
            result.push(row?);
        }
        Ok(result)
    }

    /// 分批删除一个来源的媒体记录。单条大 DELETE 会一次性持有写锁删完全部
    /// 行；分批让每次锁的窗口都很短，期间同步的 library_page 命令能插进来，
    /// 删除进行中刷新页面不会卡死窗口。
    pub fn delete_media_by_source_batch(
        &self,
        source_type: &str,
        limit: i64,
    ) -> Result<u64, AppError> {
        let source_type = source_type.trim();
        if source_type.is_empty() {
            return Err(AppError::InvalidInput(
                "source_type cannot be empty".to_owned(),
            ));
        }
        let connection = self.lock()?;
        let deleted = connection.execute(
            "DELETE FROM media WHERE id IN (SELECT id FROM media WHERE source_type = ?1 LIMIT ?2)",
            params![source_type, limit],
        )?;
        Ok(deleted as u64)
    }

    pub fn move_media(&self, media_id: &str, library_id: Option<&str>) -> Result<bool, AppError> {
        if media_id.trim().is_empty() {
            return Err(AppError::InvalidInput(
                "media_id cannot be empty".to_owned(),
            ));
        }
        let normalized = library_id.map(str::trim).filter(|value| !value.is_empty());
        let connection = self.lock()?;
        let updated = connection.execute(
            "UPDATE media SET library_id = ?1, updated_at = CURRENT_TIMESTAMP WHERE id = ?2",
            params![normalized, media_id],
        )?;
        Ok(updated > 0)
    }

    pub fn set_media_art_url(&self, media_id: &str, art_url: &str) -> Result<bool, AppError> {
        if media_id.trim().is_empty() || art_url.trim().is_empty() {
            return Err(AppError::InvalidInput(
                "media_id and art_url are required".to_owned(),
            ));
        }
        let connection = self.lock()?;
        let updated = connection.execute(
            "UPDATE media SET art_url = ?1, updated_at = CURRENT_TIMESTAMP WHERE id = ?2",
            params![art_url.trim(), media_id],
        )?;
        Ok(updated > 0)
    }

    /// Store the wide/high-resolution artwork used by the home hero without
    /// replacing the original card artwork in `art_url`.
    pub fn set_media_backdrop_url(
        &self,
        media_id: &str,
        backdrop_url: &str,
    ) -> Result<bool, AppError> {
        if media_id.trim().is_empty() || backdrop_url.trim().is_empty() {
            return Err(AppError::InvalidInput(
                "media_id and backdrop_url are required".to_owned(),
            ));
        }
        let connection = self.lock()?;
        let updated = connection.execute(
            "UPDATE media SET backdrop_url = ?1, updated_at = CURRENT_TIMESTAMP WHERE id = ?2",
            params![backdrop_url.trim(), media_id],
        )?;
        Ok(updated > 0)
    }

    pub fn set_media_preview(
        &self,
        media_id: &str,
        art_url: &str,
        duration_seconds: Option<i64>,
    ) -> Result<bool, AppError> {
        if media_id.trim().is_empty() || art_url.trim().is_empty() {
            return Err(AppError::InvalidInput(
                "media_id and art_url are required".to_owned(),
            ));
        }
        let duration_seconds = duration_seconds.filter(|value| *value > 0);
        let connection = self.lock()?;
        let updated = connection.execute(
            "UPDATE media SET art_url = ?1, duration_seconds = COALESCE(?2, duration_seconds), updated_at = CURRENT_TIMESTAMP WHERE id = ?3",
            params![art_url.trim(), duration_seconds, media_id],
        )?;
        Ok(updated > 0)
    }

    pub fn set_favorite(&self, media_id: &str, favorite: bool) -> Result<(), AppError> {
        let connection = self.lock()?;
        if favorite {
            connection.execute(
                "INSERT OR IGNORE INTO favorites(media_id) VALUES (?1)",
                [media_id],
            )?;
        } else {
            connection.execute("DELETE FROM favorites WHERE media_id = ?1", [media_id])?;
        }
        Ok(())
    }

    pub fn is_favorite(&self, media_id: &str) -> Result<bool, AppError> {
        let connection = self.lock()?;
        Ok(connection
            .query_row(
                "SELECT 1 FROM favorites WHERE media_id = ?1",
                [media_id],
                |_row| Ok(()),
            )
            .optional()?
            .is_some())
    }

    pub fn list_favorite_media(&self, limit: u32) -> Result<Vec<MediaRecord>, AppError> {
        let connection = self.lock()?;
        let mut statement = connection.prepare(
            "SELECT m.id, m.account_id, m.library_id, m.kind, m.title, m.original_title, m.sort_key, m.year, m.art_url, m.backdrop_url, m.rating, m.duration_seconds, m.source_type, m.remote_path, m.payload
             FROM favorites f JOIN media m ON m.id = f.media_id
             WHERE m.payload IS NULL OR json_extract(m.payload, '$.promotional') IS NULL OR json_extract(m.payload, '$.promotional') != 1
             ORDER BY f.created_at DESC, m.title, m.id LIMIT ?1",
        )?;
        let rows = statement.query_map([limit.max(1)], row_to_media)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(AppError::from)
    }

    pub fn favorite_count(&self) -> Result<u64, AppError> {
        let connection = self.lock()?;
        Ok(
            connection.query_row("SELECT COUNT(*) FROM favorites", [], |row| {
                row.get::<_, i64>(0)
            })? as u64,
        )
    }

    pub fn watched_seconds(&self) -> Result<f64, AppError> {
        let connection = self.lock()?;
        Ok(connection.query_row(
            "SELECT COALESCE(SUM(CASE WHEN h.completed = 1 THEN h.duration_seconds ELSE h.position_seconds END), 0.0)
             FROM watch_history h
             WHERE h.id = (SELECT latest.id FROM watch_history latest WHERE latest.media_id = h.media_id ORDER BY latest.watched_at DESC, latest.id DESC LIMIT 1)",
            [],
            |row| row.get::<_, f64>(0),
        )?)
    }

    pub fn upsert_provider(&self, provider: &ProviderRecord) -> Result<(), AppError> {
        let connection = self.lock()?;
        let metadata = provider
            .metadata
            .as_ref()
            .map(serde_json::to_string)
            .transpose()
            .map_err(|error| AppError::Storage(format!("invalid provider metadata: {error}")))?;
        connection.execute(
            "INSERT INTO providers(id, provider_type, account_id, display_name, metadata, enabled) VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(id) DO UPDATE SET provider_type=excluded.provider_type, account_id=excluded.account_id,
                display_name=excluded.display_name, metadata=excluded.metadata, enabled=excluded.enabled, updated_at=CURRENT_TIMESTAMP",
            params![provider.id, provider.provider_type, provider.account_id, provider.display_name, metadata, provider.enabled],
        )?;
        Ok(())
    }

    pub fn list_providers(&self) -> Result<Vec<ProviderRecord>, AppError> {
        let connection = self.lock()?;
        let mut statement = connection.prepare("SELECT id, provider_type, account_id, display_name, metadata, enabled FROM providers ORDER BY id")?;
        let rows = statement.query_map([], |row| {
            let metadata: Option<String> = row.get(4)?;
            let metadata = metadata
                .map(|value| serde_json::from_str(&value))
                .transpose()
                .map_err(|error| {
                    rusqlite::Error::FromSqlConversionFailure(
                        4,
                        rusqlite::types::Type::Text,
                        Box::new(error),
                    )
                })?;
            Ok(ProviderRecord {
                id: row.get(0)?,
                provider_type: row.get(1)?,
                account_id: row.get(2)?,
                display_name: row.get(3)?,
                metadata,
                enabled: row.get::<_, i64>(5)? != 0,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>().map_err(AppError::from)
    }

    pub fn kv_set(&self, key: &str, value: &str) -> Result<(), AppError> {
        if key.trim().is_empty() {
            return Err(AppError::InvalidInput("key cannot be empty".to_owned()));
        }
        let connection = self.lock()?;
        connection.execute("INSERT INTO kv(key, value) VALUES (?1, ?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value, updated_at=CURRENT_TIMESTAMP", params![key, value])?;
        Ok(())
    }

    pub fn kv_get(&self, key: &str) -> Result<Option<String>, AppError> {
        let connection = self.lock()?;
        connection
            .query_row("SELECT value FROM kv WHERE key = ?1", [key], |row| {
                row.get(0)
            })
            .optional()
            .map_err(AppError::from)
    }

    pub fn kv_delete(&self, key: &str) -> Result<bool, AppError> {
        let connection = self.lock()?;
        Ok(connection.execute("DELETE FROM kv WHERE key = ?1", [key])? > 0)
    }

    pub fn metadata_cache_get(
        &self,
        key: &str,
        now_seconds: i64,
    ) -> Result<Option<(String, String)>, AppError> {
        let connection = self.lock()?;
        let row = connection
            .query_row(
                "SELECT provider, payload, expires_at FROM metadata_cache WHERE cache_key = ?1",
                [key],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, i64>(2)?,
                    ))
                },
            )
            .optional()?;
        let Some((provider, payload, expires_at)) = row else {
            return Ok(None);
        };
        if expires_at <= now_seconds {
            connection.execute("DELETE FROM metadata_cache WHERE cache_key = ?1", [key])?;
            return Ok(None);
        }
        Ok(Some((provider, payload)))
    }

    pub fn metadata_cache_set(
        &self,
        key: &str,
        provider: &str,
        payload: &str,
        expires_at: i64,
    ) -> Result<(), AppError> {
        let connection = self.lock()?;
        connection.execute(
            "INSERT INTO metadata_cache(cache_key, provider, payload, expires_at) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(cache_key) DO UPDATE SET provider=excluded.provider, payload=excluded.payload, expires_at=excluded.expires_at, created_at=CURRENT_TIMESTAMP",
            params![key, provider, payload, expires_at],
        )?;
        Ok(())
    }

    pub fn metadata_cache_clear_expired(&self, now_seconds: i64) -> Result<u64, AppError> {
        let connection = self.lock()?;
        Ok(connection.execute(
            "DELETE FROM metadata_cache WHERE expires_at <= ?1",
            [now_seconds],
        )? as u64)
    }
}

fn row_to_media(row: &rusqlite::Row<'_>) -> rusqlite::Result<MediaRecord> {
    let payload: Option<String> = row.get(14)?;
    let payload = payload
        .map(|value| serde_json::from_str(&value))
        .transpose()
        .map_err(|error| {
            rusqlite::Error::FromSqlConversionFailure(
                14,
                rusqlite::types::Type::Text,
                Box::new(error),
            )
        })?;
    Ok(MediaRecord {
        id: row.get(0)?,
        account_id: row.get(1)?,
        library_id: row.get(2)?,
        kind: row.get(3)?,
        title: row.get(4)?,
        original_title: row.get(5)?,
        sort_key: row.get(6)?,
        year: row.get(7)?,
        art_url: row.get(8)?,
        backdrop_url: row.get(9)?,
        rating: row.get(10)?,
        duration_seconds: row.get(11)?,
        source_type: row.get(12)?,
        remote_path: row.get(13)?,
        payload,
    })
}

fn upsert_media_on_connection(
    connection: &rusqlite::Connection,
    media: &MediaRecord,
) -> Result<(), AppError> {
    let payload = media
        .payload
        .as_ref()
        .map(serde_json::to_string)
        .transpose()
        .map_err(|error| AppError::Storage(format!("invalid media payload: {error}")))?;
    connection.execute(
        "INSERT INTO media (id, account_id, library_id, kind, title, original_title, sort_key, year, art_url, backdrop_url, rating, duration_seconds, source_type, remote_path, payload)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)
         ON CONFLICT(id) DO UPDATE SET account_id=excluded.account_id, library_id=excluded.library_id,
            kind=excluded.kind,
             -- Re-importing a cloud folder must not wipe a completed scrape.
             title=CASE WHEN json_extract(media.payload, '$.scrapedBy') IS NOT NULL AND json_extract(excluded.payload, '$.scrapedBy') IS NULL THEN media.title ELSE excluded.title END,
             original_title=CASE WHEN json_extract(media.payload, '$.scrapedBy') IS NOT NULL AND json_extract(excluded.payload, '$.scrapedBy') IS NULL THEN media.original_title ELSE excluded.original_title END,
             sort_key=CASE WHEN json_extract(media.payload, '$.scrapedBy') IS NOT NULL AND json_extract(excluded.payload, '$.scrapedBy') IS NULL THEN media.sort_key ELSE excluded.sort_key END,
             year=CASE WHEN json_extract(media.payload, '$.scrapedBy') IS NOT NULL AND json_extract(excluded.payload, '$.scrapedBy') IS NULL THEN media.year ELSE excluded.year END,
             -- A scan/import record may not carry derived artwork yet. Keep
             -- the cached card/home images produced by the local video probe
             -- instead of clearing them on the next refresh.
             art_url=COALESCE(NULLIF(excluded.art_url, ''), media.art_url),
             backdrop_url=COALESCE(NULLIF(excluded.backdrop_url, ''), media.backdrop_url),
             rating=CASE WHEN json_extract(media.payload, '$.scrapedBy') IS NOT NULL AND json_extract(excluded.payload, '$.scrapedBy') IS NULL THEN media.rating ELSE excluded.rating END,
             duration_seconds=COALESCE(excluded.duration_seconds, media.duration_seconds),
            source_type=excluded.source_type,
            remote_path=COALESCE(NULLIF(excluded.remote_path, ''), media.remote_path),
            payload=CASE WHEN json_extract(media.payload, '$.scrapedBy') IS NOT NULL AND json_extract(excluded.payload, '$.scrapedBy') IS NULL THEN media.payload ELSE excluded.payload END,
            updated_at=CURRENT_TIMESTAMP",
        params![media.id, media.account_id, media.library_id, media.kind, media.title, media.original_title,
            media.sort_key, media.year, media.art_url, media.backdrop_url, media.rating, media.duration_seconds,
            media.source_type, media.remote_path, payload],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migrates_and_round_trips_media_and_fts() {
        let database = Database::open_in_memory().unwrap();
        assert_eq!(database.schema_version().unwrap(), LATEST_SCHEMA_VERSION);
        let mut media = MediaRecord::new("m1", "movie", "The Long Road");
        media.original_title = Some("Long Road".to_owned());
        media.payload = Some(serde_json::json!({"source": "test"}));
        database.upsert_media(&media).unwrap();
        assert!(database
            .set_media_backdrop_url("m1", "covers/home-wide.jpg")
            .unwrap());
        media.backdrop_url = Some("covers/home-wide.jpg".to_owned());
        assert_eq!(database.get_media("m1").unwrap(), Some(media.clone()));
        assert_eq!(
            database
                .list_media(MediaFilter::default(), 10, 0)
                .unwrap()
                .len(),
            1
        );
        assert_eq!(database.search_media("Long", 10, 0).unwrap().len(), 1);
        media.title = "Updated Road".to_owned();
        database.upsert_media(&media).unwrap();
        assert_eq!(database.search_media("Long", 10, 0).unwrap().len(), 1);
        assert_eq!(database.search_media("Updated", 10, 0).unwrap().len(), 1);
    }

    #[test]
    fn preview_update_persists_artwork_and_duration_together() {
        let database = Database::open_in_memory().unwrap();
        database
            .upsert_media(&MediaRecord::new("preview-1", "video", "Preview"))
            .unwrap();
        assert!(database
            .set_media_preview("preview-1", "data:image/jpeg;base64,preview", Some(3720))
            .unwrap());
        let media = database.get_media("preview-1").unwrap().unwrap();
        assert_eq!(
            media.art_url.as_deref(),
            Some("data:image/jpeg;base64,preview")
        );
        assert_eq!(media.duration_seconds, Some(3720));
    }

    #[test]
    fn upsert_without_derived_artwork_keeps_cached_card_and_home_posters() {
        let database = Database::open_in_memory().unwrap();
        let mut cached = MediaRecord::new("poster-1", "video", "Poster");
        cached.art_url = Some("data:image/jpeg;base64,card".to_owned());
        cached.backdrop_url = Some("data:image/jpeg;base64,home".to_owned());
        database.upsert_media(&cached).unwrap();

        let mut scanned = MediaRecord::new("poster-1", "video", "Poster");
        scanned.remote_path = Some("C:/Videos/poster.mp4".to_owned());
        database.upsert_media(&scanned).unwrap();

        let media = database.get_media("poster-1").unwrap().unwrap();
        assert_eq!(media.art_url, cached.art_url);
        assert_eq!(media.backdrop_url, cached.backdrop_url);
        assert_eq!(media.remote_path, scanned.remote_path);
    }

    #[test]
    fn batch_upsert_round_trips_all_media_and_fts() {
        let database = Database::open_in_memory().unwrap();
        let media = vec![
            MediaRecord::new("batch-1", "video", "Batch Alpha"),
            MediaRecord::new("batch-2", "video", "Batch Beta"),
            MediaRecord::new("batch-3", "video", "Batch Gamma"),
        ];
        database.upsert_media_batch(&media).unwrap();

        assert_eq!(
            database
                .list_media(MediaFilter::default(), 10, 0)
                .unwrap()
                .len(),
            media.len()
        );
        assert_eq!(
            database.search_media("Batch", 10, 0).unwrap().len(),
            media.len()
        );
        assert_eq!(
            database.get_media("batch-2").unwrap(),
            Some(media[1].clone())
        );
    }

    #[test]
    fn list_media_applies_optional_filters_without_changing_order() {
        let database = Database::open_in_memory().unwrap();
        let mut first = MediaRecord::new("filter-1", "video", "Zulu");
        first.library_id = Some("library-a".to_owned());
        first.sort_key = Some("001".to_owned());
        let mut second = MediaRecord::new("filter-2", "movie", "Alpha");
        second.library_id = Some("library-a".to_owned());
        second.sort_key = Some("002".to_owned());
        let mut third = MediaRecord::new("filter-3", "video", "Beta");
        third.library_id = Some("library-b".to_owned());
        database
            .upsert_media_batch(&[first, second, third])
            .unwrap();

        let rows = database
            .list_media(
                MediaFilter {
                    library_id: Some("library-a"),
                    kind: Some("video"),
                    ..MediaFilter::default()
                },
                10,
                0,
            )
            .unwrap();
        assert_eq!(
            rows.iter()
                .map(|media| media.id.as_str())
                .collect::<Vec<_>>(),
            ["filter-1"]
        );
    }

    #[test]
    fn user_facing_queries_exclude_promotional_rows() {
        let database = Database::open_in_memory().unwrap();
        let normal = MediaRecord::new("normal", "video", "Normal Video");
        let mut ad = MediaRecord::new("ad", "video", "Ad Video");
        ad.payload = Some(serde_json::json!({"promotional": true}));
        database.upsert_media_batch(&[normal, ad]).unwrap();
        database.set_favorite("ad", true).unwrap();

        assert_eq!(
            database
                .list_media(MediaFilter::default(), 10, 0)
                .unwrap()
                .len(),
            1
        );
        assert_eq!(database.search_media("Video", 10, 0).unwrap().len(), 1);
        assert_eq!(database.list_favorite_media(10).unwrap().len(), 0);
        assert_eq!(database.media_count().unwrap(), 1);
        assert!(database.get_media("ad").unwrap().is_some());
    }

    #[test]
    fn history_favorites_providers_and_kv_work() {
        let database = Database::open_in_memory().unwrap();
        database
            .upsert_media(&MediaRecord::new("m1", "video", "Video"))
            .unwrap();
        database.save_watch_history("m1", 2.0, 10.0, false).unwrap();
        assert_eq!(
            database
                .latest_watch_history("m1")
                .unwrap()
                .unwrap()
                .position_seconds,
            2.0
        );
        database.set_favorite("m1", true).unwrap();
        assert!(database.is_favorite("m1").unwrap());
        let favorites = database.list_favorite_media(10).unwrap();
        assert_eq!(favorites.len(), 1);
        assert_eq!(favorites[0].id, "m1");
        assert_eq!(database.favorite_count().unwrap(), 1);
        assert_eq!(database.media_count().unwrap(), 1);
        assert_eq!(database.watched_seconds().unwrap(), 2.0);
        database.set_favorite("m1", false).unwrap();
        assert!(!database.is_favorite("m1").unwrap());
        database
            .upsert_provider(&ProviderRecord {
                id: "guangya".to_owned(),
                provider_type: "cloud".to_owned(),
                account_id: Some("a".to_owned()),
                display_name: None,
                metadata: Some(serde_json::json!({"endpoint": "example"})),
                enabled: true,
            })
            .unwrap();
        assert_eq!(database.list_providers().unwrap().len(), 1);
        database.kv_set("theme", "dark").unwrap();
        assert_eq!(database.kv_get("theme").unwrap().as_deref(), Some("dark"));
    }

    #[test]
    fn metadata_cache_round_trips_and_expires() {
        let database = Database::open_in_memory().unwrap();
        assert_eq!(database.schema_version().unwrap(), LATEST_SCHEMA_VERSION);
        database
            .metadata_cache_set("tmdb:matrix:false", "tmdb", "null", 20)
            .unwrap();
        assert_eq!(
            database
                .metadata_cache_get("tmdb:matrix:false", 19)
                .unwrap()
                .unwrap()
                .0,
            "tmdb"
        );
        assert!(database
            .metadata_cache_get("tmdb:matrix:false", 20)
            .unwrap()
            .is_none());
        assert_eq!(database.metadata_cache_clear_expired(20).unwrap(), 0);
    }

    #[test]
    fn clear_media_cascades_history_and_favorites() {
        let database = Database::open_in_memory().unwrap();
        database
            .upsert_media(&MediaRecord::new("m1", "video", "Video"))
            .unwrap();
        database.save_watch_history("m1", 1.0, 10.0, false).unwrap();
        database.set_favorite("m1", true).unwrap();
        assert_eq!(database.clear_media().unwrap(), 1);
        assert!(database.latest_watch_history("m1").unwrap().is_none());
        assert!(!database.is_favorite("m1").unwrap());
    }
    #[test]
    fn deletes_only_requested_media_source() {
        let database = Database::open_in_memory().unwrap();
        for (id, source) in [
            ("guangya-1", "provider:guangya"),
            ("guangya-2", "provider:guangya"),
            ("local-1", "local"),
        ] {
            let mut media = MediaRecord::new(id, "video", id);
            media.source_type = source.to_owned();
            database.upsert_media(&media).unwrap();
        }
        assert_eq!(
            database.delete_media_by_source("provider:guangya").unwrap(),
            2
        );
        assert!(database.get_media("guangya-1").unwrap().is_none());
        assert!(database.get_media("guangya-2").unwrap().is_none());
        assert!(database.get_media("local-1").unwrap().is_some());
    }

    #[test]
    fn counts_lists_art_and_deletes_source_in_batches() {
        let database = Database::open_in_memory().unwrap();
        for (id, source, art) in [
            ("guangya-1", "provider:guangya", Some("covers/a.jpg")),
            ("guangya-2", "provider:guangya", None),
            ("guangya-3", "provider:guangya", Some("covers/b.jpg")),
            ("local-1", "local", Some("covers/c.jpg")),
        ] {
            let mut media = MediaRecord::new(id, "video", id);
            media.source_type = source.to_owned();
            media.art_url = art.map(str::to_owned);
            database.upsert_media(&media).unwrap();
        }
        assert_eq!(
            database.count_media_by_source("provider:guangya").unwrap(),
            3
        );
        let art = database
            .list_media_art_by_source("provider:guangya")
            .unwrap();
        assert_eq!(art.len(), 3);
        assert!(art.contains(&("guangya-1".to_owned(), Some("covers/a.jpg".to_owned()))));
        // 每批最多删 2 条：2 + 1 + 0 收尾，分批与一次性删除结果一致。
        assert_eq!(
            database
                .delete_media_by_source_batch("provider:guangya", 2)
                .unwrap(),
            2
        );
        assert_eq!(
            database
                .delete_media_by_source_batch("provider:guangya", 2)
                .unwrap(),
            1
        );
        assert_eq!(
            database
                .delete_media_by_source_batch("provider:guangya", 2)
                .unwrap(),
            0
        );
        assert_eq!(
            database.count_media_by_source("provider:guangya").unwrap(),
            0
        );
        assert!(database
            .list_media_art_by_source("provider:guangya")
            .unwrap()
            .is_empty());
        assert!(database.get_media("guangya-1").unwrap().is_none());
        assert!(database.get_media("local-1").unwrap().is_some());
    }

    #[test]
    fn history_rejects_empty_and_non_finite_values() {
        let database = Database::open_in_memory().unwrap();
        assert!(database.save_watch_history("", 1.0, 2.0, false).is_err());
        assert!(database
            .save_watch_history("m1", f64::NAN, 2.0, false)
            .is_err());
        assert!(database
            .save_watch_history("m1", 1.0, f64::INFINITY, false)
            .is_err());
    }
}
