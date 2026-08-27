//! 播放器模块：基于 rodio（内部用 symphonia 解码）播放音频。

use anyhow::{Context, Result};
use rodio::{Decoder, OutputStream, OutputStreamHandle, Sink};
use std::io::BufReader;
use std::path::Path;
use std::time::Duration;

/// 播放模式。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PlayMode {
    Sequential, // 顺序：列表播完即停
    Loop,       // 循环：列表播完回到开头
    Random,     // 随机
}

pub struct Player {
    _stream: OutputStream,
    stream_handle: OutputStreamHandle,
    sink: Option<Sink>,
    volume: f32,
    mode: PlayMode,
}

impl Player {
    pub fn new() -> Result<Self> {
        let (stream, stream_handle) =
            OutputStream::try_default().context("创建音频输出失败（请检查音频设备）")?;
        Ok(Self {
            _stream: stream,
            stream_handle,
            sink: None,
            volume: 0.7,
            mode: PlayMode::Sequential,
        })
    }

    /// 播放指定文件。
    pub fn play_file(&mut self, path: &Path) -> Result<()> {
        let file =
            std::fs::File::open(path).with_context(|| format!("打开音频失败：{}", path.display()))?;
        let source = Decoder::new(BufReader::new(file))
            .with_context(|| format!("解码音频失败：{}", path.display()))?;

        let sink = Sink::try_new(&self.stream_handle)?;
        sink.set_volume(self.volume);
        sink.append(source);
        sink.play();
        self.sink = Some(sink);
        Ok(())
    }

    /// 暂停/继续。
    pub fn toggle_pause(&mut self) {
        if let Some(s) = &self.sink {
            if s.is_paused() {
                s.play();
            } else {
                s.pause();
            }
        }
    }

    pub fn is_paused(&self) -> bool {
        self.sink.as_ref().map(|s| s.is_paused()).unwrap_or(true)
    }

    /// 当前曲目是否已播放完毕。
    pub fn is_finished(&self) -> bool {
        self.sink.as_ref().map(|s| s.empty()).unwrap_or(true)
    }

    /// 是否已加载曲目。
    pub fn has_loaded(&self) -> bool {
        self.sink.is_some()
    }

    pub fn set_volume(&mut self, v: f32) {
        self.volume = v.clamp(0.0, 1.0);
        if let Some(s) = &self.sink {
            s.set_volume(self.volume);
        }
    }

    pub fn volume(&self) -> f32 {
        self.volume
    }

    pub fn mode(&self) -> PlayMode {
        self.mode
    }

    pub fn set_mode(&mut self, m: PlayMode) {
        self.mode = m;
    }

    /// 当前播放位置。
    pub fn position(&self) -> Duration {
        self.sink
            .as_ref()
            .map(|s| s.get_pos())
            .unwrap_or(Duration::ZERO)
    }

    /// 跳转到指定位置。
    pub fn seek(&mut self, pos: Duration) {
        if let Some(s) = &self.sink {
            let _ = s.try_seek(pos);
        }
    }

}
