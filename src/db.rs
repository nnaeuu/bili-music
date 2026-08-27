//! 数据库模块：SQLite 连接、表结构初始化与基础查询。

use anyhow::{Context, Result};
use rusqlite::Connection;
use std::path::PathBuf;

/// 数据库文件名
const DB_FILE: &str = "bili_music.db";

/// 建表 SQL（幂等，可重复执行）。
///
/// 时间字段统一使用 Unix 时间戳（秒）；时长使用秒（REAL）。
const SCHEMA_SQL: &str = r#"
CREATE TABLE IF NOT EXISTS tracks (
    id           INTEGER PRIMARY KEY AUTOINCREMENT,
    bvid         TEXT    NOT NULL,                 -- BV 号（同一稿件多个分 P 会重复）
    cid          INTEGER NOT NULL UNIQUE,          -- 分 P 的 cid（缓存条目唯一标识）
    title        TEXT    NOT NULL,                 -- 稿件/分 P 标题
    uploader     TEXT,                             -- UP 主
    publish_time INTEGER,                          -- 发布时间（Unix 秒）
    duration     REAL,                             -- 时长（秒）
    file_path    TEXT    NOT NULL,                 -- 音频文件绝对路径
    file_size    INTEGER,                          -- 文件大小（字节）
    created_at   INTEGER NOT NULL,                 -- 导入时间（Unix 秒）
    updated_at   INTEGER NOT NULL                  -- 最后更新时间（Unix 秒）
);

CREATE TABLE IF NOT EXISTS tags (
    id   INTEGER PRIMARY KEY AUTOINCREMENT,
    name TEXT NOT NULL UNIQUE
);

CREATE TABLE IF NOT EXISTS track_tags (
    track_id INTEGER NOT NULL REFERENCES tracks(id) ON DELETE CASCADE,
    tag_id   INTEGER NOT NULL REFERENCES tags(id)   ON DELETE CASCADE,
    PRIMARY KEY (track_id, tag_id)
);

CREATE TABLE IF NOT EXISTS playlists (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    name       TEXT    NOT NULL UNIQUE,
    created_at INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS playlist_tracks (
    playlist_id INTEGER NOT NULL REFERENCES playlists(id) ON DELETE CASCADE,
    track_id    INTEGER NOT NULL REFERENCES tracks(id)    ON DELETE CASCADE,
    position    INTEGER NOT NULL,
    PRIMARY KEY (playlist_id, track_id)
);

CREATE TABLE IF NOT EXISTS play_history (
    id        INTEGER PRIMARY KEY AUTOINCREMENT,
    track_id  INTEGER NOT NULL REFERENCES tracks(id) ON DELETE CASCADE,
    played_at INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS settings (
    key   TEXT PRIMARY KEY,
    value TEXT
);

CREATE INDEX IF NOT EXISTS idx_tracks_bvid     ON tracks(bvid);
CREATE INDEX IF NOT EXISTS idx_tracks_uploader ON tracks(uploader);
CREATE INDEX IF NOT EXISTS idx_tracks_publish  ON tracks(publish_time);
CREATE INDEX IF NOT EXISTS idx_history_track   ON play_history(track_id);
"#;

/// 获取默认数据库路径。
pub fn default_db_path() -> PathBuf {
    data_dir().join(DB_FILE)
}

/// 数据目录（便携模式：可执行文件所在目录）。
///
/// 数据库与音频都放在程序目录下，删除程序目录即完全清除，不往系统其他位置写文件。
pub fn data_dir() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.to_path_buf()))
        .unwrap_or_else(|| PathBuf::from("."))
}

/// 提取出的音频文件存放目录。
pub fn audio_dir() -> PathBuf {
    data_dir().join("audio")
}

/// 解析文件路径：相对路径相对于数据目录（exe 目录），绝对路径原样返回。
pub fn resolve_path(path: &str) -> PathBuf {
    let p = std::path::Path::new(path);
    if p.is_absolute() {
        p.to_path_buf()
    } else {
        data_dir().join(p)
    }
}

/// 当前 Unix 时间戳（秒）。
pub fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// 打开（必要时创建）数据库并初始化表结构。
///
/// 注意：`PRAGMA foreign_keys` 是「每个连接」级别的开关，
/// 因此每次打开连接都需要设置一次（此处已设置）。
pub fn init_db(path: &PathBuf) -> Result<Connection> {
    let conn = Connection::open(path)
        .with_context(|| format!("无法打开数据库：{}", path.display()))?;

    conn.execute_batch("PRAGMA foreign_keys = ON;")
        .context("开启外键约束失败")?;

    conn.execute_batch(SCHEMA_SQL)
        .context("初始化表结构失败")?;

    Ok(conn)
}

/// 读取设置值。
pub fn get_setting(conn: &Connection, key: &str) -> Option<String> {
    conn.query_row("SELECT value FROM settings WHERE key = ?1", [key], |r| r.get(0))
        .ok()
}

/// 写入设置值（键已存在则更新）。
pub fn set_setting(conn: &Connection, key: &str, value: &str) -> Result<()> {
    conn.execute(
        "INSERT INTO settings(key, value) VALUES (?1, ?2) \
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        [key, value],
    )?;
    Ok(())
}
