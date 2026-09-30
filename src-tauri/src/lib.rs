//! 央视工具箱 —— Tauri v2 后端入口。
//!
//! 模块划分：
//! * [`http`]      —— 浏览器一致性请求头、重试、并发映射、EcoQoS 关闭
//! * [`cctv`]      —— 央视网接口客户端（搜索 / 片库 / 栏目 / 栏目大全 / 4K / 听音 / 视频源）
//! * [`download`]  —— HLS 并发分片下载、合并与 ffmpeg 转封装
//! * [`hd`]        —— 高清（720P）浏览器解密通道
//! * [`preview`]   —— 视频预览（取帧 + 外部播放器）
//! * [`ffmpeg`]    —— ffmpeg / ffplay 探测与调用
//! * [`cdp`]       —— 极简 Chrome DevTools Protocol 客户端

mod cctv;
mod cdp;
mod commands;
mod download;
mod error;
mod ffmpeg;
mod hd;
mod http;
mod model;
mod preview;
#[cfg(test)]
mod smoke;
mod util;

use std::sync::Arc;

use tauri::Manager;

/// 默认输出目录：优先「下载」文件夹，其次是可执行文件同级的 downloads。
fn resolve_default_output() -> String {
    if let Some(home) = std::env::var_os("USERPROFILE") {
        let downloads = std::path::PathBuf::from(home).join("Downloads");
        if downloads.is_dir() {
            return downloads.to_string_lossy().into_owned();
        }
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let target = dir.join("downloads");
            let _ = std::fs::create_dir_all(&target);
            return target.to_string_lossy().into_owned();
        }
    }
    let fallback = std::env::temp_dir().join("cctv_downloads");
    let _ = std::fs::create_dir_all(&fallback);
    fallback.to_string_lossy().into_owned()
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // Windows 会把「自认为在后台」的进程降速 2.3 倍，启动时显式关掉
    http::disable_power_throttling();

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            let default_output = resolve_default_output();
            app.manage(Arc::new(commands::AppState::new(default_output)));
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::env_info,
            commands::search_videos,
            commands::list_albums,
            commands::list_album_episodes,
            commands::library_categories,
            commands::resolve_column,
            commands::list_column_all,
            commands::search_columns,
            commands::column_filters,
            commands::browse_columns,
            commands::list_4k_albums,
            commands::list_ting_items,
            commands::resolve_ting_guids,
            commands::ting_sections,
            commands::list_qualities,
            commands::preview_resolve,
            commands::preview_players,
            commands::preview_play,
            commands::preview_stop,
            commands::preview_frame,
            commands::preview_save_frame,
            commands::open_url,
            commands::start_download,
            commands::cancel_task,
            commands::default_output_dir,
        ])
        .run(tauri::generate_context!())
        .expect("启动央视工具箱失败");
}
