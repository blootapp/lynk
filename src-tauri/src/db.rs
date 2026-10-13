use rusqlite::{params, Connection, OptionalExtension, Result, Row};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};

use crate::model::Download;

const SCHEMA_VERSION: i32 = 3;

/// Single shared SQLite connection. Every access goes through `conn()`, so
/// writes from download threads and commands are serialized and ordered.
pub struct Db(Mutex<Connection>);

impl Db {
    pub fn open() -> Result<Self> {
        let conn = Connection::open(data_dir().join("lynk.db"))?;
        conn.execute_batch(
            "PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL; PRAGMA busy_timeout=3000;",
        )?;
        migrate(&conn)?;
        Ok(Db(Mutex::new(conn)))
    }

    pub fn conn(&self) -> MutexGuard<'_, Connection> {
        // A panic while holding the lock must not brick the database for the
        // rest of the session.
        self.0.lock().unwrap_or_else(|e| e.into_inner())
    }

    // ── Preferences ───────────────────────────────────────────────────────────

    pub fn prefs(&self) -> Result<HashMap<String, String>> {
        let conn = self.conn();
        let mut stmt = conn.prepare("SELECT key, value FROM preferences")?;
        let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
        rows.collect()
    }

    pub fn pref(&self, key: &str) -> Option<String> {
        self.conn()
            .query_row("SELECT value FROM preferences WHERE key = ?1", [key], |r| r.get(0))
            .optional()
            .ok()
            .flatten()
    }

    pub fn set_pref(&self, key: &str, value: &str) -> Result<()> {
        self.conn().execute(
            "INSERT OR REPLACE INTO preferences (key, value) VALUES (?1, ?2)",
            params![key, value],
        )?;
        Ok(())
    }

    // ── Downloads ─────────────────────────────────────────────────────────────

    pub fn list(&self) -> Result<Vec<Download>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(&format!(
            "SELECT {COLUMNS} FROM downloads WHERE status != 'playlist' ORDER BY created_at DESC"
        ))?;
        let rows = stmt.query_map([], row_to_download)?;
        rows.collect()
    }

    pub fn get(&self, id: &str) -> Result<Option<Download>> {
        self.conn()
            .query_row(
                &format!("SELECT {COLUMNS} FROM downloads WHERE id = ?1"),
                [id],
                row_to_download,
            )
            .optional()
    }

    /// Insert or fully replace a row. Transient fields (speed, eta, stage)
    /// are not persisted.
    pub fn save(&self, d: &Download) -> Result<()> {
        save_row(&self.conn(), d)
    }

    /// Saves many rows in one transaction (playlists can have hundreds).
    pub fn save_many(&self, items: &[Download]) -> Result<()> {
        let conn = self.conn();
        let tx = conn.unchecked_transaction()?;
        for d in items {
            save_row(&tx, d)?;
        }
        tx.commit()
    }

    /// (done, failed) counts for a playlist group.
    pub fn group_counts(&self, group_id: &str) -> (i64, i64) {
        self.conn()
            .query_row(
                "SELECT
                    COALESCE(SUM(status = 'done'), 0),
                    COALESCE(SUM(status = 'error'), 0)
                 FROM downloads WHERE group_id = ?1",
                [group_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap_or((0, 0))
    }

    pub fn delete(&self, id: &str) -> Result<()> {
        self.conn().execute("DELETE FROM downloads WHERE id = ?1", [id])?;
        Ok(())
    }

    /// Removes every finished row and returns what was removed.
    pub fn clear_finished(&self) -> Result<Vec<Download>> {
        let conn = self.conn();
        let rows: Vec<Download> = {
            let mut stmt = conn.prepare(&format!(
                "SELECT {COLUMNS} FROM downloads WHERE status IN ('done', 'error', 'cancelled')"
            ))?;
            let rows = stmt.query_map([], row_to_download)?;
            rows.collect::<Result<_>>()?
        };
        conn.execute(
            "DELETE FROM downloads WHERE status IN ('done', 'error', 'cancelled')",
            [],
        )?;
        Ok(rows)
    }

    /// Anything that was in flight when the app last exited can't still be
    /// running, so mark it interrupted (it can be retried) and drop playlist
    /// prompts nobody answered.
    pub fn recover_interrupted(&self) -> Result<()> {
        let conn = self.conn();
        conn.execute(
            "UPDATE downloads SET status = 'error', error_msg = 'Interrupted', error_detail = NULL
             WHERE status IN ('queued', 'fetching', 'downloading', 'processing')",
            [],
        )?;
        conn.execute("DELETE FROM downloads WHERE status = 'playlist'", [])?;
        Ok(())
    }
}

fn save_row(conn: &Connection, d: &Download) -> Result<()> {
    conn.execute(
        "INSERT OR REPLACE INTO downloads (
            id, url, title, thumbnail, duration, site, format, quality, audio_format,
            prefer_h264, ext, save_path, file_path, status, progress, size_bytes,
            error_msg, error_detail, created_at, finished_at, group_id, group_title,
            group_index, group_count, name_prefix, video_format
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16,
                  ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24, ?25, ?26)",
        params![
            d.id, d.url, d.title, d.thumbnail, d.duration, d.site, d.format, d.quality,
            d.audio_format, d.prefer_h264, d.ext, d.save_path, d.file_path, d.status,
            d.progress, d.size_bytes, d.error_msg, d.error_detail, d.created_at,
            d.finished_at, d.group_id, d.group_title, d.group_index, d.group_count,
            d.name_prefix, d.video_format,
        ],
    )?;
    Ok(())
}

const COLUMNS: &str ="id, url, title, thumbnail, duration, site, format, quality, audio_format,
    prefer_h264, ext, save_path, file_path, status, progress, size_bytes, error_msg,
    error_detail, created_at, finished_at, group_id, group_title, group_index, group_count,
    name_prefix, video_format";

fn row_to_download(r: &Row) -> Result<Download> {
    Ok(Download {
        id: r.get(0)?,
        url: r.get(1)?,
        title: r.get(2)?,
        thumbnail: r.get(3)?,
        duration: r.get(4)?,
        site: r.get(5)?,
        format: r.get(6)?,
        quality: r.get(7)?,
        audio_format: r.get::<_, Option<String>>(8)?.unwrap_or_else(|| "mp3".into()),
        prefer_h264: r.get::<_, Option<bool>>(9)?.unwrap_or(true),
        ext: r.get(10)?,
        save_path: r.get(11)?,
        file_path: r.get(12)?,
        status: r.get(13)?,
        progress: r.get::<_, Option<f64>>(14)?.unwrap_or(0.0),
        size_bytes: r.get(15)?,
        error_msg: r.get(16)?,
        error_detail: r.get(17)?,
        created_at: r.get(18)?,
        finished_at: r.get(19)?,
        group_id: r.get(20)?,
        group_title: r.get(21)?,
        group_index: r.get(22)?,
        group_count: r.get(23)?,
        name_prefix: r.get(24)?,
        video_format: r.get::<_, Option<String>>(25)?.unwrap_or_else(|| "mp4".into()),
        ..Default::default()
    })
}

// ── Schema ───────────────────────────────────────────────────────────────────

fn migrate(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS downloads (
            id          TEXT PRIMARY KEY,
            url         TEXT NOT NULL,
            title       TEXT,
            format      TEXT NOT NULL,
            quality     TEXT NOT NULL,
            ext         TEXT,
            save_path   TEXT,
            status      TEXT NOT NULL DEFAULT 'queued',
            progress    REAL DEFAULT 0,
            speed       TEXT,
            size_total  TEXT,
            error_msg   TEXT,
            created_at  INTEGER NOT NULL
        );
        CREATE TABLE IF NOT EXISTS preferences (
            key   TEXT PRIMARY KEY,
            value TEXT NOT NULL
        );",
    )?;

    let version: i32 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    if version < 2 {
        migrate_v2(conn)?;
    }
    if version < 3 && !has_column(conn, "video_format")? {
        conn.execute_batch("ALTER TABLE downloads ADD COLUMN video_format TEXT")?;
    }
    if version < SCHEMA_VERSION {
        conn.execute_batch(&format!("PRAGMA user_version = {SCHEMA_VERSION}"))?;
    }
    Ok(())
}

fn has_column(conn: &Connection, name: &str) -> Result<bool> {
    let mut stmt = conn.prepare("PRAGMA table_info(downloads)")?;
    let cols = stmt.query_map([], |r| r.get::<_, String>(1))?;
    for c in cols {
        if c? == name {
            return Ok(true);
        }
    }
    Ok(false)
}

fn migrate_v2(conn: &Connection) -> Result<()> {
    let existing: Vec<String> = {
        let mut stmt = conn.prepare("PRAGMA table_info(downloads)")?;
        let rows = stmt.query_map([], |r| r.get::<_, String>(1))?;
        rows.collect::<Result<_>>()?
    };
    let add = [
        ("thumbnail", "TEXT"),
        ("duration", "REAL"),
        ("site", "TEXT"),
        ("audio_format", "TEXT"),
        ("prefer_h264", "INTEGER"),
        ("file_path", "TEXT"),
        ("size_bytes", "INTEGER"),
        ("error_detail", "TEXT"),
        ("finished_at", "INTEGER"),
        ("group_id", "TEXT"),
        ("group_title", "TEXT"),
        ("group_index", "INTEGER"),
        ("group_count", "INTEGER"),
        ("name_prefix", "TEXT"),
    ];
    for (col, ty) in add {
        if !existing.iter().any(|c| c == col) {
            conn.execute_batch(&format!("ALTER TABLE downloads ADD COLUMN {col} {ty}"))?;
        }
    }

    // v1 stored created_at in seconds; everything is milliseconds now.
    conn.execute(
        "UPDATE downloads SET created_at = created_at * 1000 WHERE created_at < 100000000000",
        [],
    )?;
    conn.execute(
        "UPDATE downloads SET audio_format = ext WHERE format = 'audio' AND audio_format IS NULL",
        [],
    )?;

    // v1 overwrote save_path with the final file path on completion (and a
    // path-parsing bug often left only a fragment of it). Split it into a
    // real directory + file path where the file can still be found.
    let rows: Vec<(String, Option<String>)> = {
        let mut stmt = conn.prepare(
            "SELECT id, save_path FROM downloads WHERE status = 'done' AND file_path IS NULL",
        )?;
        let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
        rows.collect::<Result<_>>()?
    };
    for (id, save_path) in rows {
        let p = save_path.map(PathBuf::from);
        let (dir, file) = match p {
            Some(p) if p.is_file() => (
                p.parent().map(|d| d.to_string_lossy().to_string()),
                Some(p.to_string_lossy().to_string()),
            ),
            Some(p) if p.is_dir() => (Some(p.to_string_lossy().to_string()), None),
            _ => (None, None),
        };
        conn.execute(
            "UPDATE downloads SET save_path = ?1, file_path = ?2 WHERE id = ?3",
            params![dir, file, id],
        )?;
    }

    // v1 seeded `autostart_initialized = 'false'`, which made the first-run
    // check think autostart had already been set up. Drop the bogus seed so
    // the first-run logic can do its job.
    conn.execute(
        "DELETE FROM preferences WHERE key = 'autostart_initialized' AND value = 'false'",
        [],
    )?;
    Ok(())
}

/// Seeds defaults for any missing preference keys.
pub fn seed_preferences(db: &Db, default_save_path: &Path) -> Result<()> {
    let save = default_save_path.to_string_lossy().to_string();
    let defaults: [(&str, &str); 11] = [
        ("default_quality", "best"),
        ("default_format", "video"),
        ("audio_format", "mp3"),
        ("video_format", "mp4"),
        ("ask_save_path", "false"),
        ("prefer_h264", "true"),
        ("max_concurrent", "3"),
        ("notifications", "true"),
        ("ytdlp_auto_update", "true"),
        ("cookies_browser", ""),
        ("save_path", &save),
    ];
    let conn = db.conn();
    for (k, v) in defaults {
        conn.execute(
            "INSERT OR IGNORE INTO preferences (key, value) VALUES (?1, ?2)",
            params![k, v],
        )?;
    }
    Ok(())
}

/// App data directory. Kept identical to v1 so existing history survives.
pub fn data_dir() -> PathBuf {
    #[cfg(target_os = "windows")]
    let p = PathBuf::from(std::env::var("APPDATA").unwrap_or_else(|_| ".".into())).join("Lynk");
    #[cfg(not(target_os = "windows"))]
    let p = PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| ".".into())).join(".lynk");
    std::fs::create_dir_all(&p).ok();
    p
}
