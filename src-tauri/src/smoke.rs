//! 联网冒烟测试：验证央视网接口层在当前网络环境下真的能跑通。
//!
//! 这些测试**依赖外网**，建议这样跑（串行、打印明细）：
//!
//! ```text
//! cargo test --lib -- --nocapture --test-threads=1
//! ```
//!
//! 之所以把它们放进 crate 内部（而不是 `tests/`），是因为 `cctv` 等模块
//! 是私有模块，只有单元测试能直接调到。接口契约一旦变化，这里应当第一时间
//! 报红——这正是这些测试存在的意义。

use crate::cctv::{album, column, columns_dir, fourk, library, search, ting, video};
use crate::preview;

fn rt() -> tokio::runtime::Runtime {
    tokio::runtime::Runtime::new().expect("创建 tokio runtime 失败")
}

/// 搜索接口：`search_videos`
#[test]
fn smoke_search() {
    rt().block_on(async {
        let page = search::search_videos("新闻联播", 1, 5, "relevance", 0, "", "-1")
            .await
            .expect("搜索接口失败");
        println!("搜索『新闻联播』: total={} 返回 {} 条", page.total, page.list.len());
        for item in page.list.iter().take(3) {
            println!("  · {} | id={} | url={}", item.title, item.id, item.url);
        }
        assert!(!page.list.is_empty(), "搜索结果为空，接口契约可能变了");
    });
}

/// 片库接口：`list_albums`（`fc` 传中文分类名，不是 dsj / jlp 这类缩写）
#[test]
fn smoke_library() {
    rt().block_on(async {
        let page = library::list_albums("纪录片", "", "", "", "", "", 1, 6)
            .await
            .expect("片库接口失败");
        println!("片库『纪录片』: total={} 返回 {} 条", page.total, page.list.len());
        assert!(!page.list.is_empty(), "片库返回为空");
        for item in page.list.iter().take(3) {
            println!(
                "  · {} | album_id={} | 首集 guid={}",
                item.title, item.album_id, item.first_guid
            );
        }
    });
}

/// 专辑剧集：`list_album_main_episodes`
#[test]
fn smoke_album_episodes() {
    rt().block_on(async {
        let page = library::list_albums("纪录片", "", "", "", "", "", 1, 4)
            .await
            .expect("片库接口失败");
        let album_id = page
            .list
            .iter()
            .map(|item| item.album_id.clone())
            .find(|id| !id.is_empty())
            .expect("片库没有返回可用 album_id");

        let episodes = album::list_album_main_episodes(&album_id, 3)
            .await
            .expect("剧集接口失败");
        println!("专辑 {album_id}: 拿到 {} 集", episodes.len());
        assert!(!episodes.is_empty(), "剧集列表为空");
        println!("  首集: {} | guid={}", episodes[0].title, episodes[0].guid);
    });
}

/// 4K 专区：`list_4k_albums`
#[test]
fn smoke_fourk() {
    rt().block_on(async {
        let page = fourk::list_4k_albums(1, 6).await.expect("4K 专区接口失败");
        println!("4K 专区: total={} 返回 {} 条", page.total, page.list.len());
        for item in page.list.iter().take(3) {
            println!("  · {} | 首集 guid={}", item.title, item.first_guid);
        }
        assert!(!page.list.is_empty(), "4K 专区返回为空");

        // 4K 专区独有明文 1080P（br=4000）：预览档位切换的关键场景
        let guid = page
            .list
            .iter()
            .map(|item| item.first_guid.as_str())
            .find(|guid| !guid.is_empty())
            .expect("4K 专辑都没有首集 guid");
        let target = preview::resolve(guid, None, false, "")
            .await
            .expect("4K 视频预览解析失败");
        let plain_brs: Vec<i64> = target
            .qualities
            .iter()
            .filter(|quality| !quality.encrypted)
            .map(|quality| quality.br)
            .collect();
        println!("4K 视频明文档位: {plain_brs:?}");
        assert!(
            plain_brs.contains(&4000),
            "4K 专区视频没有探测到明文 1080P（br=4000）：{plain_brs:?}"
        );
        // 默认选中最高明文档位；切到最低明文档位再切回来，验证切换链路
        let lowest = *plain_brs.last().expect("明文档位列表为空");
        let switched = preview::resolve(guid, Some(lowest), false, "")
            .await
            .expect("4K 视频切换档位失败");
        let chosen = switched.quality.expect("切换后没有选中档位");
        assert_eq!(chosen.br, lowest, "4K 视频切换档位未生效");
        println!("4K 视频切到 br={lowest}: {}", switched.summary);
    });
}

/// 听音专区：静态页解析 + `getVideoListByPageIdTvty`
#[test]
fn smoke_ting() {
    rt().block_on(async {
        let mut ok_sections = 0usize;
        for name in ["首页", "全部"] {
            match ting::list_ting_items(name, 1, 8).await {
                Ok(result) => {
                    println!("听音『{name}』: url={} 返回 {} 条", result.url, result.list.len());
                    for item in result.list.iter().take(3) {
                        println!(
                            "  · {} | video_id={} | guid={}",
                            item.title,
                            item.video_id,
                            if item.guid.is_empty() { "（待解析）" } else { &item.guid }
                        );
                    }
                    if !result.list.is_empty() {
                        ok_sections += 1;
                    }
                }
                Err(exc) => println!("听音『{name}』失败：{exc}"),
            }
        }
        assert!(ok_sections > 0, "听音所有分区都拿不到条目（检查 p 参数是否被误传）");
    });
}

/// 栏目大全：`columnSearch` + 内置清单兜底解析 TOPC id
#[test]
fn smoke_browse_columns() {
    rt().block_on(async {
        let page = columns_dir::browse_columns("", "", 1, 8)
            .await
            .expect("栏目大全接口失败");
        println!("栏目大全: total={} 返回 {} 条", page.total, page.list.len());
        assert!(!page.list.is_empty(), "栏目大全返回为空");
        for item in page.list.iter().take(5) {
            println!(
                "  · {} | channel={} | fc={} | topic_id={}",
                item.name,
                item.channel,
                item.fc,
                if item.topic_id.is_empty() {
                    "（未解析）"
                } else {
                    &item.topic_id
                }
            );
        }
        let resolved = page.list.iter().filter(|item| !item.topic_id.is_empty()).count();
        println!("  其中 {resolved}/{} 条拿到了 TOPC id", page.list.len());
    });
}

/// 栏目往期：按年分块拉取（只测当年，避免测试跑太久）
#[test]
fn smoke_column_all() {
    rt().block_on(async {
        let topic = columns_dir::search_columns("新闻联播", true)
            .into_iter()
            .map(|item| item.topic_id)
            .find(|id| !id.is_empty())
            .expect("内置清单里没有可用的『新闻联播』TOPC id");

        let result = column::list_column_all_videos(&topic, "2026", "", 6, None, None)
            .await
            .expect("栏目往期接口失败");
        println!(
            "栏目往期 {topic}（2026）: total={} blocks={} truncated={} years={:?}",
            result.total, result.blocks, result.truncated, result.years
        );
        for episode in result.list.iter().take(3) {
            println!("  · {} | guid={}", episode.title, episode.guid);
        }
        assert!(!result.list.is_empty(), "栏目往期为空");

        // 单窗口上限 1000 条：高产栏目（日均上百条）必须细化到「日」才不会被截断。
        // 百家讲坛 2026 年约 2700 条，若仍是 1000 说明细化逻辑退化。
        let dense = column::list_column_all_videos("TOPC1451557052519584", "2026", "", 8, None, None)
            .await
            .expect("高产栏目往期接口失败");
        println!(
            "高产栏目（百家讲坛 2026）: total={} blocks={} truncated={}",
            dense.total, dense.blocks, dense.truncated
        );
        assert!(
            dense.total > 1000,
            "封顶年份没有细化（total={}），往期会被单窗口 1000 条的上限截断",
            dense.total
        );
    });
}

/// 视频源 + 清晰度 + 预览解析（明文档位取帧/内嵌播放的前提）
#[test]
fn smoke_video_and_preview() {
    rt().block_on(async {
        let page = search::search_videos("新闻联播", 1, 4, "relevance", 0, "", "-1")
            .await
            .expect("搜索接口失败");
        let page_url = page
            .list
            .iter()
            .map(|item| item.url.clone())
            .find(|url| url.starts_with("http"))
            .expect("搜索结果里没有可用的视频页地址");

        let guid = video::resolve_guid(&page_url)
            .await
            .expect("从视频页地址解析 guid 失败");
        let info = video::get_video_info(&guid).await.expect("视频源接口失败");
        println!(
            "视频 {guid}: 标题={} 时长={} 频道={}",
            info.title, info.duration, info.channel
        );

        let qualities = video::list_qualities(info.into(), false)
            .await
            .expect("清晰度解析失败");
        assert!(!qualities.is_empty(), "没有解析出任何清晰度");
        for quality in qualities.iter() {
            println!(
                "  · {:<12} br={:<5} {}x{} channel={:<6} encrypted={}",
                quality.label,
                quality.br,
                quality.width,
                quality.height,
                quality.channel,
                quality.encrypted
            );
        }

        let target = preview::resolve(&guid, None, false, &page_url)
            .await
            .expect("预览解析失败");
        println!(
            "预览: {} | media_url={}",
            target.summary,
            if target.media_url.is_empty() {
                "（无，只有加密档）"
            } else {
                &target.media_url
            }
        );

        // 档位切换：明文档位 ≥2 个时，切到最低档并确认生效（预览下拉的切换链路）
        let plain_brs: Vec<i64> = target
            .qualities
            .iter()
            .filter(|quality| !quality.encrypted)
            .map(|quality| quality.br)
            .collect();
        if plain_brs.len() >= 2 {
            let other = *plain_brs.last().expect("明文档位列表为空");
            let switched = preview::resolve(&guid, Some(other), false, &page_url)
                .await
                .expect("切换档位失败");
            let chosen = switched.quality.expect("切换后没有选中档位");
            assert_eq!(chosen.br, other, "切换档位未生效（仍选中 {}）", chosen.br);
            assert!(!switched.media_url.is_empty(), "切换后 media_url 为空");
            println!(
                "切换档位 br={other}: {} | media_url={}",
                switched.summary, switched.media_url
            );
        } else {
            println!("明文档位只有 {plain_brs:?}，无切换可验证");
        }
    });
}

/// 真实下载一小段明文视频，验证「解析 → 并发分片 → 合并 → ffmpeg 转封装」整条链路。
///
/// 会自动挑 150 秒以内的短节目，避免测试跑太久；真挑不到就跳过（不算失败）。
#[test]
fn smoke_download_short_clip() {
    rt().block_on(async {
        let page = search::search_videos("国际联播快讯", 1, 10, "relevance", 0, "", "-1")
            .await
            .expect("搜索接口失败");

        let mut picked: Option<(String, String, f64)> = None;
        for item in page.list.iter() {
            if !item.url.starts_with("http") {
                continue;
            }
            let Ok(guid) = video::resolve_guid(&item.url).await else {
                continue;
            };
            let Ok(info) = video::get_video_info(&guid).await else {
                continue;
            };
            let seconds: f64 = info.duration.parse().unwrap_or(0.0);
            if seconds > 0.0 && seconds <= 150.0 {
                picked = Some((guid, info.title.clone(), seconds));
                break;
            }
        }

        let Some((guid, title, seconds)) = picked else {
            println!("没找到 150 秒以内的短节目，跳过下载实测");
            return;
        };
        println!("下载实测：{title}（{seconds:.0} 秒）guid={guid}");

        let dir = std::env::temp_dir().join("cctv_smoke_dl");
        let _ = tokio::fs::remove_dir_all(&dir).await;

        let result = crate::download::download_video(
            &guid,
            &dir.to_string_lossy(),
            "smoke",
            None,
            8,
            None,
            None,
            true,
            false,
        )
        .await;
        println!(
            "  结果：success={} 分片={} 耗时={:.1}s | {}",
            result.success, result.segment_count, result.elapsed, result.message
        );
        assert!(result.success, "下载失败：{}", result.message);

        let meta = tokio::fs::metadata(&result.output_path)
            .await
            .expect("下载报告成功，但产物文件不存在");
        println!(
            "  产物：{}（{:.1} KB，清晰度 {}）",
            result.output_path,
            meta.len() as f64 / 1024.0,
            result.quality_label
        );
        assert!(meta.len() > 50_000, "产物过小（{} 字节），可能是坏流", meta.len());

        let _ = tokio::fs::remove_dir_all(&dir).await;
    });
}

/// 预览取帧：ffmpeg 直连 CDN（验证 CORS / Referer 都对）拿到一张 JPEG。
#[test]
fn smoke_grab_frame() {
    rt().block_on(async {
        let guid = "962fa271986e437eb6617aa8597569db"; // 《国际联播快讯》短节目
        let target = preview::resolve(guid, None, false, "")
            .await
            .expect("预览解析失败");
        if target.media_url.is_empty() {
            println!("该节目只有加密档，无法取帧，跳过");
            return;
        }
        let data_url = preview::grab_frame_data_url(&target.media_url, guid, 5.0, 480)
            .await
            .expect("取帧失败");
        println!(
            "取帧成功：{} | data URL 长度 = {} 字节",
            target.summary,
            data_url.len()
        );
        assert!(data_url.starts_with("data:image/jpeg;base64,"), "取帧返回格式不对");
        assert!(data_url.len() > 2_000, "取到的图太小，可能是坏帧");
    });
}

/// 本地环境自检：ffmpeg / ffplay / 浏览器 / EcoQoS
#[test]
fn smoke_local_env() {
    println!("ffmpeg  = {:?}", crate::ffmpeg::find_ffmpeg());
    println!("ffplay  = {:?}", crate::ffmpeg::find_ffplay());
    println!("browser = {:?}", crate::cdp::find_browser());
    println!(
        "关闭 Windows 速度节流 = {}",
        crate::http::disable_power_throttling()
    );
}
