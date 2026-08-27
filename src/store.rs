//! 数据访问层：曲目查询/更新、标签、歌单。

use anyhow::Result;
use rusqlite::{params, Connection};

#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct Track {
    pub id: i64,
    pub bvid: String,
    pub cid: i64,
    pub title: String,
    pub uploader: String,
    pub publish_time: Option<i64>,
    pub duration: Option<f64>,
    pub file_path: String,
}

#[derive(Debug, Clone)]
pub struct Tag {
    pub id: i64,
    pub name: String,
}

#[derive(Debug, Clone)]
pub struct Playlist {
    pub id: i64,
    pub name: String,
}

const TRACK_COLS: &str = "id, bvid, cid, title, uploader, publish_time, duration, file_path";

fn row_to_track(row: &rusqlite::Row) -> rusqlite::Result<Track> {
    Ok(Track {
        id: row.get(0)?,
        bvid: row.get(1)?,
        cid: row.get(2)?,
        title: row.get(3)?,
        uploader: row.get::<_, Option<String>>(4)?.unwrap_or_default(),
        publish_time: row.get(5)?,
        duration: row.get(6)?,
        file_path: row.get(7)?,
    })
}

/// 查询全部曲目（按导入时间倒序）。
pub fn list_tracks(conn: &Connection) -> Result<Vec<Track>> {
    let mut stmt = conn.prepare(&format!("SELECT {TRACK_COLS} FROM tracks ORDER BY id DESC"))?;
    let tracks = stmt
        .query_map([], |r| row_to_track(r))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(tracks)
}

/// 所有 UP 主（去重，按名称排序）。
pub fn list_uploaders(conn: &Connection) -> Result<Vec<String>> {
    let mut stmt = conn.prepare(
        "SELECT DISTINCT uploader FROM tracks WHERE uploader IS NOT NULL AND uploader != '' ORDER BY uploader",
    )?;
    let v = stmt
        .query_map([], |r| r.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(v)
}

/// 所有标签。
pub fn list_tags(conn: &Connection) -> Result<Vec<Tag>> {
    let mut stmt = conn.prepare("SELECT id, name FROM tags ORDER BY name")?;
    let v = stmt
        .query_map([], |r| {
            Ok(Tag {
                id: r.get(0)?,
                name: r.get(1)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(v)
}

/// 所有歌单。
pub fn list_playlists(conn: &Connection) -> Result<Vec<Playlist>> {
    let mut stmt = conn.prepare("SELECT id, name FROM playlists ORDER BY id")?;
    let v = stmt
        .query_map([], |r| {
            Ok(Playlist {
                id: r.get(0)?,
                name: r.get(1)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(v)
}

/// 更新曲目元数据（标题、UP 主）。
pub fn update_track(conn: &Connection, id: i64, title: &str, uploader: &str) -> Result<()> {
    conn.execute(
        "UPDATE tracks SET title = ?1, uploader = ?2, updated_at = ?3 WHERE id = ?4",
        params![title, uploader, crate::db::now_unix(), id],
    )?;
    Ok(())
}

/// 给曲目打标签（标签不存在则自动创建）。
pub fn add_tag(conn: &Connection, track_id: i64, tag_name: &str) -> Result<()> {
    let name = tag_name.trim();
    if name.is_empty() {
        return Ok(());
    }
    conn.execute("INSERT OR IGNORE INTO tags(name) VALUES (?1)", [name])?;
    let tag_id: i64 = conn.query_row("SELECT id FROM tags WHERE name = ?1", [name], |r| r.get(0))?;
    conn.execute(
        "INSERT OR IGNORE INTO track_tags(track_id, tag_id) VALUES (?1, ?2)",
        params![track_id, tag_id],
    )?;
    Ok(())
}

/// 新建歌单，返回 id（已存在则返回现有 id）。
pub fn create_playlist(conn: &Connection, name: &str) -> Result<i64> {
    let name = name.trim();
    conn.execute(
        "INSERT OR IGNORE INTO playlists(name, created_at) VALUES (?1, ?2)",
        params![name, crate::db::now_unix()],
    )?;
    let id: i64 = conn.query_row("SELECT id FROM playlists WHERE name = ?1", [name], |r| r.get(0))?;
    Ok(id)
}

/// 把曲目加入歌单。
pub fn add_to_playlist(conn: &Connection, playlist_id: i64, track_id: i64) -> Result<()> {
    let pos: i64 = conn.query_row(
        "SELECT COALESCE(MAX(position), 0) + 1 FROM playlist_tracks WHERE playlist_id = ?1",
        [playlist_id],
        |r| r.get(0),
    )?;
    conn.execute(
        "INSERT OR IGNORE INTO playlist_tracks(playlist_id, track_id, position) VALUES (?1, ?2, ?3)",
        params![playlist_id, track_id, pos],
    )?;
    Ok(())
}

/// 曲目的所有标签。
pub fn track_tags(conn: &Connection, track_id: i64) -> Result<Vec<Tag>> {
    let mut stmt = conn.prepare(
        "SELECT tg.id, tg.name FROM tags tg \
         JOIN track_tags tt ON tg.id = tt.tag_id \
         WHERE tt.track_id = ?1 ORDER BY tg.name",
    )?;
    let v = stmt
        .query_map([track_id], |r| {
            Ok(Tag {
                id: r.get(0)?,
                name: r.get(1)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(v)
}

/// 按条件筛选曲目（按 UP 主或标签，二者互斥）。
pub fn filter_tracks(
    conn: &Connection,
    uploader: Option<&str>,
    tag_id: Option<i64>,
) -> Result<Vec<Track>> {
    if let Some(tag) = tag_id {
        let mut stmt = conn.prepare(&format!(
            "SELECT {TRACK_COLS} FROM tracks t \
             JOIN track_tags tt ON t.id = tt.track_id \
             WHERE tt.tag_id = ?1 ORDER BY t.id DESC"
        ))?;
        let v = stmt
            .query_map([tag], |r| row_to_track(r))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(v)
    } else if let Some(up) = uploader {
        let mut stmt = conn.prepare(&format!(
            "SELECT {TRACK_COLS} FROM tracks WHERE uploader = ?1 ORDER BY id DESC"
        ))?;
        let v = stmt
            .query_map([up], |r| row_to_track(r))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(v)
    } else {
        list_tracks(conn)
    }
}

/// 清理 file_path 已失效（文件不存在）的曲目记录，返回清理数量。
/// 删除曲目会级联删除其标签、歌单、播放记录关联。
pub fn clean_missing(conn: &Connection) -> Result<usize> {
    let mut stmt = conn.prepare("SELECT id, file_path FROM tracks")?;
    let rows: Vec<(i64, String)> = stmt
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<rusqlite::Result<_>>()?;

    let mut removed = 0;
    for (id, path) in rows {
        if !crate::db::resolve_path(&path).exists() {
            conn.execute("DELETE FROM tracks WHERE id = ?1", [id])?;
            removed += 1;
        }
    }
    Ok(removed)
}
