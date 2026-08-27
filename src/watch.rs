//! 文件夹监听：检测缓存目录变化，通知主线程增量导入。

use notify::{RecursiveMode, Watcher};
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, Sender};
use std::thread;
use std::time::Duration;

/// 监听句柄：主线程通过 `has_change` 检查是否有变化，通过 `set_path` 更新监听目录。
pub struct WatchHandle {
    rx: Receiver<()>,
    cmd: Sender<PathBuf>,
}

impl WatchHandle {
    /// 启动后台监听线程（监听失败则静默降级为「无增量导入」，不影响手动导入）。
    pub fn start(path: PathBuf) -> Self {
        let (event_tx, event_rx) = std::sync::mpsc::channel::<()>();
        let (cmd_tx, cmd_rx) = std::sync::mpsc::channel::<PathBuf>();

        thread::spawn(move || {
            let mut current = path;

            let mut watcher = match notify::recommended_watcher(
                move |res: notify::Result<notify::Event>| {
                    if res.is_ok() {
                        let _ = event_tx.send(());
                    }
                },
            ) {
                Ok(w) => w,
                Err(_) => return,
            };
            if watcher.watch(&current, RecursiveMode::Recursive).is_err() {
                return;
            }

            loop {
                // 主线程请求更新监听目录
                if let Ok(new_path) = cmd_rx.try_recv() {
                    let _ = watcher.unwatch(&current);
                    let _ = watcher.watch(&new_path, RecursiveMode::Recursive);
                    current = new_path;
                }
                thread::sleep(Duration::from_millis(500));
            }
        });

        Self {
            rx: event_rx,
            cmd: cmd_tx,
        }
    }

    /// 更新监听目录（用户更改缓存路径后调用）。
    pub fn set_path(&self, path: PathBuf) {
        let _ = self.cmd.send(path);
    }

    /// 非阻塞检查是否有文件变化通知。
    pub fn has_change(&self) -> bool {
        self.rx.try_recv().is_ok()
    }
}
