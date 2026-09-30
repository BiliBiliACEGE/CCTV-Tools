//! Tauri 命令层：把后端能力暴露给前端，并把长任务的进度用事件推出去。

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager, State};

use crate::cctv::{album, column, columns_dir, fourk, search, ting, video};
use crate::error::{Error, Result};
use crate::model::{Album, Column, Episode, Paged, Quality, SearchItem};
use crate::preview::{self, PlayerInfo, PreviewTarget};
use crate::{cdp, download, ffmpeg, hd};

/// 应用全局状态。
pub struct AppState {
    /// 当前长任务的取消开关
    pub cancel: Mutex<Option<Arc<AtomicBool>>>,
    pub default_output: String,
}

impl AppState {
    pub fn new(default_output: String) -> Self {
        AppState {
            cancel: Mutex::new(None),
            default_output,
        }
    }

    fn begin_task(&self) -> Arc<AtomicBool> {
        let flag = Arc::new(AtomicBool::new(false));
        if let Ok(mut guard) = self.cancel.lock() {
            if let Some(previous) = guard.take() {
                previous.store(true, Ordering::SeqCst);
            }
            *guard = Some(Arc::clone(&flag));
        }
        flag
    }

    fn request_cancel(&self) {
        if let Ok(guard) = self.cancel.lock() {
            if let Some(flag) = guard.as_ref() {
                flag.store(true, Ordering::SeqCst);
            }
        }
    }
}

fn state_of(app: &AppHandle) -> std::sync::Arc<AppState> {
    app.state::<std::sync::Arc<AppState>>().inner().clone()
}

// --------------------------------------------------------------------------- //
// 环境自检
// --------------------------------------------------------------------------- //
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EnvInfo {
    pub ffmpeg: String,
    pub ffplay: String,
    pub browser: String,
    pub default_output: String,
    pub version: String,
}

#[tauri::command]
pub async fn env_info(state: State<'_, Arc<AppState>>) -> Result<EnvInfo> {
    Ok(EnvInfo {
        ffmpeg: ffmpeg::find_ffmpeg().unwrap_or_default(),
        ffplay: ffmpeg::find_ffplay().unwrap_or_default(),
        browser: cdp::find_browser().unwrap_or_default(),
        default_output: state.default_output.clone(),
        version: env!("CARGO_PKG_VERSION").to_string(),
    })
}

// --------------------------------------------------------------------------- //
// 搜索 / 片库 / 专辑
// --------------------------------------------------------------------------- //
#[tauri::command]
pub async fn search_videos(
    keyword: String,
    page: Option<i64>,
    page_size: Option<i64>,
) -> Result<Paged<SearchItem>> {
    search::search_videos(
        &keyword,
        page.unwrap_or(1),
        page_size.unwrap_or(30),
        "relevance",
        0,
        "",
        "-1",
    )
    .await
}

#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn list_albums(
    fc: String,
    sc: Option<String>,
    area: Option<String>,
    year: Option<String>,
    letter: Option<String>,
    page: Option<i64>,
    page_size: Option<i64>,
) -> Result<Paged<Album>> {
    // `fc` 直接用中文分类名（接口不认 dsj / jlp 这类缩写）
    let fc_name = fc.as_str();
    crate::cctv::library::list_albums(
        fc_name,
        &sc.unwrap_or_default(),
        &area.unwrap_or_default(),
        &year.unwrap_or_default(),
        &letter.unwrap_or_default(),
        "",
        page.unwrap_or(1),
        page_size.unwrap_or(30),
    )
    .await
}

#[tauri::command]
pub async fn list_album_episodes(album_id: String, max_pages: Option<i64>) -> Result<Vec<Episode>> {
    album::list_album_main_episodes(&album_id, max_pages.unwrap_or(6)).await
}

/// 片库可选分类（前端下拉直接用这份，避免前后端各写一遍）。
#[tauri::command]
pub async fn library_categories() -> Result<Vec<String>> {
    Ok(crate::cctv::LIBRARY_CATEGORIES
        .iter()
        .map(|name| (*name).to_string())
        .collect())
}

// --------------------------------------------------------------------------- //
// 栏目
// --------------------------------------------------------------------------- //
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ColumnResolved {
    pub column_id: String,
    pub title: String,
    pub site: String,
    /// 同名/模糊匹配到多个时给出候选，由前端让用户选
    pub candidates: Vec<Column>,
}

/// 把「栏目名 / 栏目页地址 / TOPC id」统一解析成可用的栏目。
#[tauri::command]
pub async fn resolve_column(input: String) -> Result<ColumnResolved> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Err(Error::cctv("请输入栏目名或栏目页地址"));
    }

    if trimmed.starts_with("http") {
        let page = column::parse_column_page(trimmed).await?;
        return Ok(ColumnResolved {
            column_id: page.column_id,
            title: page.title,
            site: trimmed.to_string(),
            candidates: Vec::new(),
        });
    }

    if trimmed.starts_with("TOPC") {
        let column = columns_dir::find_column_by_topic(trimmed);
        return Ok(ColumnResolved {
            column_id: trimmed.to_string(),
            title: column.map(|c| c.name).unwrap_or_default(),
            site: String::new(),
            candidates: Vec::new(),
        });
    }

    let hits = columns_dir::search_columns(trimmed, true);
    if hits.is_empty() {
        return Err(Error::cctv(format!(
            "未在内置栏目清单中找到「{trimmed}」，可改用栏目页地址，或从「栏目大全」里选"
        )));
    }
    if let Some(exact) = hits.iter().find(|column| column.name == trimmed) {
        return Ok(ColumnResolved {
            column_id: exact.topic_id.clone(),
            title: exact.name.clone(),
            site: exact.site.clone(),
            candidates: Vec::new(),
        });
    }
    let first = &hits[0];
    if hits.len() == 1 {
        return Ok(ColumnResolved {
            column_id: first.topic_id.clone(),
            title: first.name.clone(),
            site: first.site.clone(),
            candidates: Vec::new(),
        });
    }
    Ok(ColumnResolved {
        column_id: String::new(),
        title: String::new(),
        site: String::new(),
        candidates: hits,
    })
}

/// 拉取栏目全部往期（跨年分块），进度通过 `column:progress` 事件推送。
#[tauri::command]
pub async fn list_column_all(
    app: AppHandle,
    column_id: String,
    since: Option<String>,
    until: Option<String>,
    workers: Option<usize>,
) -> Result<column::ColumnAllResult> {
    let app_for_progress = app.clone();
    let progress: column::ProgressFn = Arc::new(move |done, total, text| {
        let _ = app_for_progress.emit(
            "column:progress",
            serde_json::json!({ "done": done, "total": total, "text": text }),
        );
    });

    let state = state_of(&app);
    let cancel = state.begin_task();
    column::list_column_all_videos(
        &column_id,
        &since.unwrap_or_default(),
        &until.unwrap_or_default(),
        workers.unwrap_or(6),
        Some(progress),
        Some(cancel),
    )
    .await
}

#[tauri::command]
pub async fn search_columns(keyword: String, only_usable: Option<bool>) -> Result<Vec<Column>> {
    Ok(columns_dir::search_columns(&keyword, only_usable.unwrap_or(false)))
}

#[tauri::command]
pub async fn column_filters() -> Result<serde_json::Value> {
    Ok(serde_json::json!({
        "categories": columns_dir::COLUMN_CATEGORIES
            .iter()
            .map(|(name, _)| *name)
            .collect::<Vec<_>>(),
        "channels": columns_dir::COLUMN_CHANNELS
            .iter()
            .map(|(name, _)| *name)
            .collect::<Vec<_>>(),
    }))
}

#[tauri::command]
pub async fn browse_columns(
    category: Option<String>,
    channel: Option<String>,
    page: Option<i64>,
    page_size: Option<i64>,
) -> Result<Paged<Column>> {
    columns_dir::browse_columns(
        &category.unwrap_or_default(),
        &channel.unwrap_or_default(),
        page.unwrap_or(1),
        page_size.unwrap_or(40),
    )
    .await
}

// --------------------------------------------------------------------------- //
// 4K 专区 / 听音
// --------------------------------------------------------------------------- //
#[tauri::command]
pub async fn list_4k_albums(page: Option<i64>, page_size: Option<i64>) -> Result<Paged<Album>> {
    fourk::list_4k_albums(page.unwrap_or(1), page_size.unwrap_or(60)).await
}

#[tauri::command]
pub async fn list_ting_items(
    section: String,
    page: Option<i64>,
    page_size: Option<i64>,
) -> Result<ting::TingResult> {
    ting::list_ting_items(&section, page.unwrap_or(1), page_size.unwrap_or(60)).await
}

/// 并发补齐听音条目的 guid（首页 / 热听榜是静态页，只有视频页 ID）。
#[tauri::command]
pub async fn resolve_ting_guids(items: Vec<Episode>) -> Result<Vec<Episode>> {
    let mut items = items;
    let mut pending: Vec<Episode> = items
        .iter()
        .filter(|item| item.guid.is_empty())
        .cloned()
        .collect();
    if !pending.is_empty() {
        ting::resolve_guids(&mut pending, 8).await;
        for resolved in pending {
            if let Some(slot) = items
                .iter_mut()
                .find(|item| item.guid.is_empty() && item.video_id == resolved.video_id)
            {
                slot.guid = resolved.guid;
            }
        }
    }
    Ok(items)
}

#[tauri::command]
pub async fn ting_sections() -> Result<Vec<String>> {
    Ok(ting::TING_SECTIONS
        .iter()
        .map(|(name, _)| (*name).to_string())
        .collect())
}

// --------------------------------------------------------------------------- //
// 清晰度 / 预览
// --------------------------------------------------------------------------- //
#[tauri::command]
pub async fn list_qualities(guid: String) -> Result<Vec<Quality>> {
    let info = video::get_video_info(&guid).await?;
    video::list_qualities(info.into(), false).await
}

#[tauri::command]
pub async fn preview_resolve(
    guid: String,
    br: Option<i64>,
    audio: Option<bool>,
    page_url: Option<String>,
) -> Result<PreviewTarget> {
    let page_url = page_url.unwrap_or_default();
    // 搜索结果、听音条目常常只有视频页地址，这里补一次 guid 解析
    let guid = if guid.trim().is_empty() {
        if page_url.trim().is_empty() {
            return Err(Error::cctv("缺少视频标识（guid 与视频页地址都为空）"));
        }
        video::resolve_guid(&page_url).await?
    } else {
        guid
    };
    preview::resolve(&guid, br, audio.unwrap_or(false), &page_url).await
}

#[tauri::command]
pub async fn preview_players() -> Result<Vec<PlayerInfo>> {
    Ok(preview::list_players())
}

#[tauri::command]
pub async fn preview_play(url: String, title: String, player: Option<String>) -> Result<u32> {
    preview::play(&url, &title, &player.unwrap_or_default())
}

#[tauri::command]
pub async fn preview_stop() -> Result<()> {
    preview::stop_player();
    Ok(())
}

#[tauri::command]
pub async fn preview_frame(
    media_url: String,
    guid: String,
    at: f64,
    width: Option<u32>,
) -> Result<String> {
    preview::grab_frame_data_url(&media_url, &guid, at, width.unwrap_or(640)).await
}

#[tauri::command]
pub async fn preview_save_frame(
    media_url: String,
    at: f64,
    width: Option<u32>,
    out_path: String,
) -> Result<String> {
    preview::save_frame(&media_url, &out_path, at, width.unwrap_or(1280)).await
}

#[tauri::command]
pub async fn open_url(url: String) -> Result<()> {
    preview::open_in_browser(&url)
}

// --------------------------------------------------------------------------- //
// 下载
// --------------------------------------------------------------------------- //
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DownloadItem {
    pub guid: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub album_title: String,
    #[serde(default)]
    pub page_url: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DownloadRequest {
    pub items: Vec<DownloadItem>,
    pub output_dir: String,
    /// 目标码率代号；留空表示自动选最高明文档
    pub br: Option<i64>,
    /// `standard` | `hd` | `audio`
    pub channel: String,
    pub workers: Option<usize>,
    pub make_mp4: Option<bool>,
    pub keep_ts: Option<bool>,
    /// 高清通道只取前 N 秒（0 = 全部）
    pub hd_seconds: Option<f64>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct TaskProgress {
    index: usize,
    total: usize,
    name: String,
    /// 完成百分比 0-100（不确定时为 -1）
    percent: f64,
    message: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct TaskLog {
    level: String,
    message: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct TaskDone {
    total: usize,
    success: usize,
    failed: usize,
    cancelled: bool,
    outputs: Vec<String>,
}

/// 开始批量下载。立即返回，进度通过事件推送：
/// `download:progress` / `download:log` / `download:done`
#[tauri::command]
pub async fn start_download(app: AppHandle, request: DownloadRequest) -> Result<()> {
    if request.items.is_empty() {
        return Err(Error::cctv("没有要下载的条目"));
    }
    if request.output_dir.trim().is_empty() {
        return Err(Error::cctv("请先选择输出目录"));
    }

    let state = state_of(&app);
    let cancel = state.begin_task();
    let workers = request.workers.unwrap_or(8);
    let make_mp4 = request.make_mp4.unwrap_or(true);
    let keep_ts = request.keep_ts.unwrap_or(false);

    tauri::async_runtime::spawn(async move {
        let total = request.items.len();
        let mut success = 0usize;
        let mut failed = 0usize;
        let mut outputs: Vec<String> = Vec::new();
        let mut cancelled = false;

        for (index, item) in request.items.iter().enumerate() {
            if cancel.load(Ordering::SeqCst) {
                cancelled = true;
                break;
            }

            let name = if item.title.is_empty() {
                item.guid.clone()
            } else {
                item.title.clone()
            };
            let app_for_log = app.clone();
            let log = |level: &str, message: String| {
                let _ = app_for_log.emit(
                    "download:log",
                    TaskLog {
                        level: level.to_string(),
                        message,
                    },
                );
            };

            let _ = app.emit(
                "download:progress",
                TaskProgress {
                    index: index + 1,
                    total,
                    name: name.clone(),
                    percent: 0.0,
                    message: "开始".to_string(),
                },
            );

            // 搜索结果 / 听音条目可能只有视频页地址，先补 guid
            let guid = if !item.guid.is_empty() {
                item.guid.clone()
            } else if item.page_url.starts_with("http") {
                match video::resolve_guid(&item.page_url).await {
                    Ok(resolved) => resolved,
                    Err(exc) => {
                        failed += 1;
                        log("error", format!("✘ {name}：无法解析视频标识（{exc}）"));
                        continue;
                    }
                }
            } else {
                String::new()
            };

            let result = match request.channel.as_str() {
                "hd" => {
                    run_hd_task(
                        &app,
                        item,
                        &request,
                        index,
                        total,
                        &name,
                        cancel.clone(),
                    )
                    .await
                }
                "audio" => {
                    let app_for_progress = app.clone();
                    let name_for_progress = name.clone();
                    let progress: download::ProgressFn = Arc::new(move |done, total_seg, text| {
                        let percent = if total_seg > 0 {
                            done as f64 / total_seg as f64 * 100.0
                        } else {
                            -1.0
                        };
                        let _ = app_for_progress.emit(
                            "download:progress",
                            TaskProgress {
                                index: index + 1,
                                total,
                                name: name_for_progress.clone(),
                                percent,
                                message: text,
                            },
                        );
                    });
                    download::download_audio(
                        &guid,
                        &request.output_dir,
                        &item.title,
                        workers,
                        Some(progress),
                        Some(cancel.clone()),
                        keep_ts,
                    )
                    .await
                }
                _ => {
                    let app_for_progress = app.clone();
                    let name_for_progress = name.clone();
                    let progress: download::ProgressFn = Arc::new(move |done, total_seg, text| {
                        let percent = if total_seg > 0 {
                            done as f64 / total_seg as f64 * 100.0
                        } else {
                            -1.0
                        };
                        let _ = app_for_progress.emit(
                            "download:progress",
                            TaskProgress {
                                index: index + 1,
                                total,
                                name: name_for_progress.clone(),
                                percent,
                                message: text,
                            },
                        );
                    });
                    let target_dir = if item.album_title.is_empty() {
                        request.output_dir.clone()
                    } else {
                        std::path::Path::new(&request.output_dir)
                            .join(crate::util::sanitize_filename(&item.album_title, 120))
                            .to_string_lossy()
                            .into_owned()
                    };
                    download::download_video(
                        &guid,
                        &target_dir,
                        &item.title,
                        request.br,
                        workers,
                        Some(progress),
                        Some(cancel.clone()),
                        make_mp4,
                        keep_ts,
                    )
                    .await
                }
            };

            if result.success {
                success += 1;
                outputs.push(result.output_path.clone());
                log("ok", format!("✔ {name} → {}", result.output_path));
            } else if matches!(result.message.as_str(), "已取消") {
                cancelled = true;
                break;
            } else {
                failed += 1;
                log("error", format!("✘ {name}：{}", result.message));
            }

            let _ = app.emit(
                "download:progress",
                TaskProgress {
                    index: index + 1,
                    total,
                    name: name.clone(),
                    percent: 100.0,
                    message: if result.success {
                        "完成".to_string()
                    } else {
                        result.message.clone()
                    },
                },
            );
        }

        let _ = app.emit(
            "download:done",
            TaskDone {
                total,
                success,
                failed,
                cancelled,
                outputs,
            },
        );
    });

    Ok(())
}

async fn run_hd_task(
    app: &AppHandle,
    item: &DownloadItem,
    request: &DownloadRequest,
    index: usize,
    total: usize,
    name: &str,
    cancel: Arc<AtomicBool>,
) -> download::DownloadResult {
    // 高清通道需要视频页地址
    let page_url = if !item.page_url.is_empty() {
        item.page_url.clone()
    } else {
        match video::page_url_for_guid(&item.guid).await {
            Ok(url) => url,
            Err(exc) => return download::DownloadResult {
                success: false,
                message: format!("无法确定视频页地址：{exc}"),
                ..Default::default()
            },
        }
    };

    let target_dir = if item.album_title.is_empty() {
        std::path::PathBuf::from(&request.output_dir)
    } else {
        std::path::PathBuf::from(&request.output_dir)
            .join(crate::util::sanitize_filename(&item.album_title, 120))
    };
    if let Err(exc) = tokio::fs::create_dir_all(&target_dir).await {
        return download::DownloadResult {
            success: false,
            message: format!("创建输出目录失败：{exc}"),
            ..Default::default()
        };
    }
    let file_name = if item.title.is_empty() {
        item.guid.clone()
    } else {
        item.title.clone()
    };
    let out_path = target_dir.join(format!(
        "{}.mp4",
        crate::util::sanitize_filename(&file_name, 120)
    ));

    let app_for_progress = app.clone();
    let name_for_progress = name.to_string();
    let progress: hd::ProgressFn = Arc::new(move |state| {
        let percent = if state.target > 0.0 {
            (state.buf_end / state.target * 100.0).min(100.0)
        } else {
            -1.0
        };
        let _ = app_for_progress.emit(
            "download:progress",
            TaskProgress {
                index: index + 1,
                total,
                name: name_for_progress.clone(),
                percent,
                message: format!("浏览器解密中 · {} · 已落盘 {} 段", state.resolution, state.saved),
            },
        );
    });

    let seconds = request.hd_seconds.unwrap_or(0.0);
    match hd::download_hd(
        &page_url,
        &out_path.to_string_lossy(),
        seconds,
        Some(progress),
        Some(cancel),
    )
    .await
    {
        Ok(path) => download::DownloadResult {
            success: true,
            output_path: path,
            message: "高清 720P".to_string(),
            quality_label: "高清 720P".to_string(),
            ..Default::default()
        },
        Err(exc) => download::DownloadResult {
            success: false,
            message: exc.to_string(),
            ..Default::default()
        },
    }
}

/// 取消当前任务（下载 / 录制 / 栏目全量拉取）。
#[tauri::command]
pub async fn cancel_task(state: State<'_, Arc<AppState>>) -> Result<()> {
    state.request_cancel();
    Ok(())
}

/// 用系统对话框挑选输出目录（前端也可用 dialog 插件，这里留一个后端实现）。
#[tauri::command]
pub async fn default_output_dir(state: State<'_, Arc<AppState>>) -> Result<String> {
    Ok(state.default_output.clone())
}
