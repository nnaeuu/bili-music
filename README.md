# Bili Music

轻量级 B 站缓存音频管理播放器 —— 用 Rust 打造，单文件、零依赖、纯本地。

## ✨ 功能特性

- 🎵 读取本地 B 站客户端缓存，提取音频为 AAC 格式
- 📋 音乐列表展示（标题 / UP主 / BV号 / 时长）
- ✏️ 元数据编辑、自定义标签、自建歌单
- 🔍 按 UP主 / 标签筛选
- ▶️ 完整播放器：播放 / 暂停 / 上下曲 / 进度拖拽 / 音量 / 顺序·循环·随机
- 🔄 文件夹增量监听，新缓存自动导入
- 🧹 失效文件校验清理
- 🚫 自动跳过 DRM 加密内容

## 🛠️ 技术栈

- [Rust](https://www.rust-lang.org/) + [egui](https://github.com/emilk/egui)（GUI）
- [SQLite](https://www.sqlite.org/)（本地数据库，通过 rusqlite）
- [symphonia](https://github.com/pdeljanov/Symphonia) + [rodio](https://github.com/RustAudio/rodio)（音频解码与播放）
- [lofty](https://github.com/Serial-ATA/lofty-rs)（音频元数据）
- [notify](https://github.com/notify-rs/notify)（文件夹监听）

## 📦 构建

```bash
# 需先安装 Rust 工具链与 MSVC 构建工具
cargo build --release
# 产物在 target/release/bili-music.exe（单文件，静态链接 CRT，免安装）
```

> 首次构建若下载依赖慢，可配置 [rsproxy.cn](https://rsproxy.cn) 国内镜像。

## 🚀 使用

1. 双击 `bili-music.exe` 启动
2. 顶部填入 B 站缓存路径（默认 `C:\Users\<用户名>\Videos\bilibili`），点「导入」
3. 双击列表歌曲即可播放

所有数据（数据库 + 提取的音频）都保存在程序所在目录，绿色便携，删除文件夹即彻底清除，不留痕迹。

## 📁 项目结构

```
src/
├── main.rs    # 入口与 UI（egui）
├── db.rs      # 数据库（SQLite 表结构与路径管理）
├── import.rs  # 导入模块（解析 videoInfo.json）
├── media.rs   # 媒体处理（m4s → AAC 提取，跳过 DRM）
├── player.rs  # 播放器（rodio 封装）
├── store.rs   # 数据访问层（曲目/标签/歌单）
└── watch.rs   # 文件夹监听（notify）
```

## ⚠️ 免责声明

本项目仅供学习交流使用，请勿用于商业用途。提取的音频版权归原作者及 B 站所有，请尊重版权，仅处理你自己有权处理的内容。

## 📄 许可证

[MIT](LICENSE)
