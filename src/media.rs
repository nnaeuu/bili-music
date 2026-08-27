//! 媒体处理模块：解析 m4s (fMP4) 分片，提取 AAC 音频为 ADTS .aac；跳过 DRM 加密内容。

use anyhow::{Context, Result};
use rusqlite::Connection;
use std::path::{Path, PathBuf};

/// 处理统计。
#[derive(Default, Debug)]
pub struct ProcessReport {
    pub processed: usize,   // 成功提取
    pub skipped_drm: usize, // 检测到 DRM 加密而跳过
    pub failed: usize,      // 处理失败
}

/// 遍历所有 tracks，将原始音频 m4s 提取为 .aac 文件并更新 file_path。
pub fn process_all(conn: &Connection, audio_dir: &Path) -> Result<ProcessReport> {
    std::fs::create_dir_all(audio_dir).context("创建音频目录失败")?;
    let mut report = ProcessReport::default();

    let mut stmt = conn.prepare("SELECT id, cid, title, file_path FROM tracks")?;
    let rows: Vec<(i64, i64, String, String)> = stmt
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?
        .collect::<rusqlite::Result<_>>()?;

    for (id, cid, title, file_path) in rows {
        let src = crate::db::resolve_path(&file_path);

        // 只处理未提取的 m4s，跳过已提取的 .aac（保持幂等）
        if src.extension().and_then(|s| s.to_str()) != Some("m4s") {
            continue;
        }

        let data = match std::fs::read(&src) {
            Ok(d) => d,
            Err(_) => {
                report.failed += 1;
                continue;
            }
        };

        // DRM 加密内容的文件头会被打乱，无法识别为合法 MP4 box
        if !looks_like_mp4(&data) {
            report.skipped_drm += 1;
            continue;
        }

        match extract_aac_to_file(&data, audio_dir, &title, cid) {
            Ok(out) => {
                let size = std::fs::metadata(&out).map(|m| m.len() as i64).unwrap_or(0);
                // 存相对路径（相对于 exe 目录），便于便携分发
                let rel = out
                    .strip_prefix(crate::db::data_dir())
                    .unwrap_or(&out)
                    .to_string_lossy()
                    .into_owned();
                conn.execute(
                    "UPDATE tracks SET file_path = ?1, file_size = ?2, updated_at = ?3 WHERE id = ?4",
                    rusqlite::params![rel, size, crate::db::now_unix(), id],
                )?;
                report.processed += 1;
            }
            Err(_) => report.failed += 1,
        }
    }

    Ok(report)
}

/// 检测是否为合法 MP4（B 站 m4s 文件头有 "000000000" 前缀，需跳过再识别）。
fn looks_like_mp4(data: &[u8]) -> bool {
    find_mp4_start(data).is_some()
}

/// 定位 MP4 box 的起始偏移（跳过文件头的 "0" 填充前缀等非 box 字节）。
fn find_mp4_start(data: &[u8]) -> Option<usize> {
    let mut pos = 0usize;
    while pos + 8 <= data.len() {
        let size = u32::from_be_bytes(data[pos..pos + 4].try_into().unwrap()) as usize;
        let ty = std::str::from_utf8(&data[pos + 4..pos + 8]).unwrap_or("");
        let known = matches!(ty, "ftyp" | "styp" | "moov" | "free" | "skip" | "wide");
        if known && size >= 8 && pos + size <= data.len() {
            return Some(pos);
        }
        pos += 1;
    }
    None
}

/// 从 m4s 数据提取 AAC，写为中文命名的 ADTS .aac 文件，返回输出路径。
fn extract_aac_to_file(data: &[u8], audio_dir: &Path, title: &str, cid: i64) -> Result<PathBuf> {
    let track = extract_aac(data)?;

    let mut out = Vec::with_capacity(track.samples.len() * 1024);
    for s in &track.samples {
        out.extend_from_slice(&adts_header(track.sample_rate, track.channels, s.len()));
        out.extend_from_slice(s);
    }

    // 中文文件名（清洗非法字符），重名时加 cid 后缀
    let base = sanitize_filename(title);
    let mut path = audio_dir.join(format!("{base}.aac"));
    if path.exists() {
        path = audio_dir.join(format!("{base}_{cid}.aac"));
    }

    std::fs::write(&path, &out).with_context(|| format!("写入失败：{}", path.display()))?;
    Ok(path)
}

/// 清洗文件名中的非法字符，截断超长标题。
fn sanitize_filename(name: &str) -> String {
    let mut s: String = name
        .chars()
        .map(|c| if "\\/:*?\"<>|".contains(c) { '_' } else { c })
        .collect();
    s = s.trim().to_string();
    if s.chars().count() > 80 {
        s = s.chars().take(80).collect();
    }
    if s.is_empty() {
        s = "unknown".to_string();
    }
    s
}

// ---- MP4 box 解析 ----

struct AacTrack {
    sample_rate: u32,
    channels: u8,
    samples: Vec<Vec<u8>>,
}

/// 从 fMP4 (m4s) 数据中提取 AAC sample。
fn extract_aac(data: &[u8]) -> Result<AacTrack> {
    let start = find_mp4_start(data).context("无法识别 MP4 结构（可能为 DRM 加密）")?;
    let data = &data[start..];

    let mut sample_rate = 0u32;
    let mut channels = 0u8;
    let mut pending_sizes: Vec<u32> = Vec::new();
    let mut samples: Vec<Vec<u8>> = Vec::new();

    let mut pos = 0usize;
    while pos + 8 <= data.len() {
        let (ty, content, next) = read_box(data, pos)?;
        match ty {
            "moov" => {
                let (sr, ch) = parse_moov(content)?;
                sample_rate = sr;
                channels = ch;
            }
            "moof" => {
                pending_sizes = parse_moof(content)?;
            }
            "mdat" => {
                // B 站 m4s 的 mdat 内容直接就是 sample 数据
                let mut cursor = 0usize;
                for &sz in &pending_sizes {
                    let end = cursor + sz as usize;
                    if end > content.len() {
                        break;
                    }
                    samples.push(content[cursor..end].to_vec());
                    cursor = end;
                }
                pending_sizes.clear();
            }
            _ => {}
        }
        pos = next;
    }

    if sample_rate == 0 || samples.is_empty() {
        anyhow::bail!("未提取到有效音频数据");
    }

    Ok(AacTrack {
        sample_rate,
        channels,
        samples,
    })
}

/// 从 moov 中读取音频采样率与声道数（mp4a sample entry）。
fn parse_moov(moov: &[u8]) -> Result<(u32, u8)> {
    let mp4a = find_box(moov, "mp4a").context("未找到 mp4a sample entry")?;
    if mp4a.len() < 28 {
        anyhow::bail!("mp4a 结构不完整");
    }
    let channels = u16::from_be_bytes(mp4a[16..18].try_into().unwrap()) as u8;
    let sample_rate = u32::from_be_bytes(mp4a[24..28].try_into().unwrap()) >> 16;
    Ok((sample_rate, channels))
}

/// 从 moof 中解析 trun，得到每个 sample 的大小。
fn parse_moof(moof: &[u8]) -> Result<Vec<u32>> {
    let trun = find_box(moof, "trun").context("未找到 trun")?;
    if trun.len() < 8 {
        anyhow::bail!("trun 结构不完整");
    }
    let flags = u32::from_be_bytes([0, trun[1], trun[2], trun[3]]);
    let sample_count = u32::from_be_bytes(trun[4..8].try_into().unwrap()) as usize;

    let mut cursor = 8usize;
    if flags & 0x1 != 0 {
        cursor += 4; // data_offset
    }
    if flags & 0x4 != 0 {
        cursor += 4; // first_sample_flags
    }

    let mut sizes = Vec::with_capacity(sample_count);
    for _ in 0..sample_count {
        if flags & 0x100 != 0 {
            cursor += 4; // duration
        }
        if flags & 0x200 != 0 {
            // size
            if cursor + 4 > trun.len() {
                break;
            }
            sizes.push(u32::from_be_bytes(trun[cursor..cursor + 4].try_into().unwrap()));
            cursor += 4;
        }
        if flags & 0x400 != 0 {
            cursor += 4; // flags
        }
        if flags & 0x800 != 0 {
            cursor += 4; // composition time offset
        }
    }

    if sizes.is_empty() {
        anyhow::bail!("trun 未提供 sample size");
    }
    Ok(sizes)
}

/// 递归查找指定类型的子 box。
fn find_box<'a>(content: &'a [u8], target: &str) -> Option<&'a [u8]> {
    let mut pos = 0usize;
    while pos + 8 <= content.len() {
        let (ty, sub, next) = read_box(content, pos).ok()?;
        if ty == target {
            return Some(sub);
        }
        if let Some(found) = find_box(sub, target) {
            return Some(found);
        }
        pos = next;
    }
    None
}

/// 读取一个 box：返回 (类型, 内容切片, 下一个 box 起始)。
fn read_box<'a>(data: &'a [u8], pos: usize) -> Result<(&'a str, &'a [u8], usize)> {
    if pos + 8 > data.len() {
        anyhow::bail!("box 头越界");
    }
    let size32 = u32::from_be_bytes(data[pos..pos + 4].try_into().unwrap()) as usize;
    let ty = std::str::from_utf8(&data[pos + 4..pos + 8]).unwrap_or("????");

    let (total, header) = match size32 {
        0 => (data.len() - pos, 8), // box 延伸到文件尾
        1 => {
            if pos + 16 > data.len() {
                anyhow::bail!("64 位 box 头越界");
            }
            let s64 = u64::from_be_bytes(data[pos + 8..pos + 16].try_into().unwrap()) as usize;
            (s64, 16)
        }
        n if n < 8 => anyhow::bail!("非法 box 大小：{n}"),
        n => (n, 8),
    };

    let content_start = pos + header;
    let content_end = pos + total;
    if content_end > data.len() {
        anyhow::bail!("box 超出文件范围");
    }
    Ok((ty, &data[content_start..content_end], content_end))
}

// ---- ADTS 封装 ----

/// 生成 7 字节 ADTS 头（AAC-LC，无 CRC）。
fn adts_header(sample_rate: u32, channels: u8, frame_len: usize) -> [u8; 7] {
    let profile = 1u8; // AAC-LC
    let sf_index = sample_rate_index(sample_rate);
    let ch = channels & 0x07;
    let total = (frame_len + 7) as u32;

    let mut h = [0u8; 7];
    h[0] = 0xFF;
    h[1] = 0xF1; // syncword + ID(MPEG-4) + layer(0) + protection_absent(1)
    h[2] = (profile << 6) | (sf_index << 2) | (ch >> 2);
    h[3] = ((ch & 0x03) << 6) | (((total >> 11) & 0x03) as u8);
    h[4] = ((total >> 3) & 0xFF) as u8;
    h[5] = (((total & 0x07) << 5) as u8) | 0x1F;
    h[6] = 0xFC; // buffer_fullness = 0x7FF (VBR)，num_blocks = 0
    h
}

/// 采样率 → ADTS sampling_frequency_index 映射。
fn sample_rate_index(sr: u32) -> u8 {
    match sr {
        96000 => 0,
        88200 => 1,
        64000 => 2,
        48000 => 3,
        44100 => 4,
        32000 => 5,
        24000 => 6,
        22050 => 7,
        16000 => 8,
        12000 => 9,
        11025 => 10,
        8000 => 11,
        7350 => 12,
        _ => 4, // 默认 44100
    }
}
