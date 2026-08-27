//! 导入模块：扫描 B 站客户端缓存目录，解析 videoInfo.json，提取元数据入库。

use anyhow::{Context, Result};
use rusqlite::Connection;
use serde::Deserialize;
use std::path::{Path, PathBuf};

/// 一次导入的统计结果。
#[derive(Default, Debug)]
pub struct ImportReport {
    pub scanned: usize,  // 扫描到的缓存目录数
    pub imported: usize, // 新导入的曲目数
    pub skipped: usize,  // 已存在而跳过的数量
    pub failed: usize,   // 解析/校验失败的数量
}

/// videoInfo.json 中我们关心的字段（其余字段自动忽略）。
#[derive(Deserialize)]
struct VideoInfo {
    bvid: Option<String>,
    cid: Option<i64>,
    title: Option<String>,
    uname: Option<String>,
    pubdate: Option<i64>,
    duration: Option<f64>,
    status: Option<String>,
}

/// 扫描缓存根目录，导入其中所有完整的缓存条目。
///
/// 目录结构：`<root>/<cid>/videoInfo.json`，每个 cid 目录对应一个分 P（一首歌）。
pub fn import_from_dir(conn: &Connection, root: &Path) -> Result<ImportReport> {
    let mut report = ImportReport::default();
    let now = crate::db::now_unix();

    let dirs: Vec<PathBuf> = std::fs::read_dir(root)
        .with_context(|| format!("无法读取缓存目录：{}", root.display()))?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();

    for dir in dirs {
        let info_path = dir.join("videoInfo.json");
        if !info_path.is_file() {
            continue;
        }
        report.scanned += 1;

        match import_one(conn, &dir, &info_path, now) {
            Ok(true) => report.imported += 1,
            Ok(false) => report.skipped += 1,
            Err(_) => report.failed += 1,
        }
    }

    Ok(report)
}

/// 导入单个缓存目录；返回是否为新插入（true=新增，false=已存在跳过）。
fn import_one(conn: &Connection, dir: &Path, info_path: &Path, now: i64) -> Result<bool> {
    let text = std::fs::read_to_string(info_path)
        .with_context(|| format!("读取失败：{}", info_path.display()))?;
    let info: VideoInfo = serde_json::from_str(&text)
        .with_context(|| format!("解析失败：{}", info_path.display()))?;

    let bvid = info.bvid.filter(|s| !s.is_empty()).context("缺少 bvid")?;
    let cid = info.cid.filter(|c| *c > 0).context("缺少 cid")?;
    let title = info.title.filter(|s| !s.is_empty()).context("缺少标题")?;

    // 只导入下载完成的缓存（status 缺失视为完成，兼容旧版）
    if info.status.as_deref().map_or(false, |s| s != "completed") {
        return Ok(false);
    }

    // 音频流 m4s：codecid 302xx（30280=192k、30232=128k、30216=64k）
    let audio = find_audio_file(dir).context("未找到音频文件（缓存不完整或为纯视频）")?;
    let file_path = audio.to_string_lossy().into_owned();
    let file_size = std::fs::metadata(&audio).map(|m| m.len() as i64).ok();

    // cid 唯一：INSERT OR IGNORE 在 cid 冲突时静默跳过，返回影响行数 0
    let inserted = conn.execute(
        "INSERT OR IGNORE INTO tracks
         (bvid, cid, title, uploader, publish_time, duration, file_path, file_size, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        rusqlite::params![
            bvid,
            cid,
            title,
            info.uname,
            info.pubdate,
            info.duration,
            file_path,
            file_size,
            now,
            now
        ],
    )?;

    Ok(inserted > 0)
}

/// 在缓存目录中查找音频 m4s 文件，存在多个时取码率最高（codecid 最大）者。
fn find_audio_file(dir: &Path) -> Option<PathBuf> {
    let mut best: Option<(u32, PathBuf)> = None;

    for entry in std::fs::read_dir(dir).ok()?.flatten() {
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) != Some("m4s") {
            continue;
        }
        let Some(codecid) = file_codecid(&path) else { continue; };
        if (30200..30300).contains(&codecid) && best.as_ref().map_or(true, |(c, _)| codecid > *c) {
            best = Some((codecid, path));
        }
    }

    best.map(|(_, p)| p)
}

/// 从文件名 `{cid}-{index}-{codecid}.m4s` 提取 codecid。
fn file_codecid(path: &Path) -> Option<u32> {
    path.file_stem()?.to_str()?.rsplit('-').next()?.parse().ok()
}
