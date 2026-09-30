//! 数据模型。全部走 camelCase 序列化，直接喂给前端。

use serde::{Deserialize, Serialize};

use crate::util::safe_int;

/// 片库中的一部剧集 / 节目。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Album {
    pub title: String,
    /// VIDA...，用于查询剧集列表
    pub album_id: String,
    /// VSET...
    pub vsetid: String,
    pub url: String,
    pub image: String,
    /// 集数
    pub count: i64,
    pub year: String,
    pub area: String,
    /// 类型，如 都市 / 谍战
    pub sc: String,
    /// 分类，如 电视剧
    pub fc: String,
    pub actors: String,
    pub brief: String,
    pub first_video_id: String,
    pub first_guid: String,
}

impl Album {
    /// 从片库接口的一条原始记录构造。
    pub fn from_library_item(item: &serde_json::Value) -> Self {
        let video = item.get("video").cloned().unwrap_or(serde_json::Value::Null);
        let album_id = {
            let direct = crate::util::json_str(item, "id");
            if direct.is_empty() {
                crate::util::json_str(&video, "id")
            } else {
                direct
            }
        };
        let vsetid = {
            let a = crate::util::json_str(item, "vsetid");
            if a.is_empty() {
                crate::util::json_str(item, "vsetids")
            } else {
                a
            }
        };
        Album {
            title: crate::util::json_str(item, "title"),
            album_id,
            vsetid,
            url: crate::util::json_str(item, "url"),
            image: crate::util::json_str(item, "image"),
            count: safe_int(item.get("count").unwrap_or(&serde_json::Value::Null)),
            year: crate::util::json_str(item, "year"),
            area: crate::util::json_str(item, "area"),
            sc: crate::util::json_str(item, "sc"),
            fc: crate::util::json_str(item, "fc"),
            actors: crate::util::json_str(item, "actors"),
            brief: crate::util::json_str(item, "brief"),
            first_video_id: crate::util::json_str(&video, "id"),
            first_guid: String::new(),
        }
    }

    /// 从 4K 专区接口的一条记录构造（`last_video` 里直接带 guid）。
    pub fn from_fourk_item(item: &serde_json::Value) -> Self {
        let last = item
            .get("last_video")
            .cloned()
            .unwrap_or(serde_json::Value::Null);
        Album {
            title: crate::util::json_str(item, "title"),
            album_id: crate::util::json_str(item, "id"),
            vsetid: crate::util::json_str(item, "vsetid"),
            url: crate::util::json_str(&last, "url"),
            image: crate::util::json_str(item, "image"),
            count: safe_int(item.get("ep_count").unwrap_or(&serde_json::Value::Null)),
            fc: "4K专区".to_string(),
            sc: crate::util::json_str(item, "sc"),
            first_guid: crate::util::json_str(&last, "guid"),
            ..Default::default()
        }
    }
}

/// 专辑中的一集（`part == 0` 表示非正片，如短看点）。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Episode {
    pub title: String,
    pub video_id: String,
    pub guid: String,
    pub url: String,
    pub image: String,
    pub length: String,
    pub part: i64,
    pub sc: String,
    pub brief: String,
    /// 4K 专区等专辑无集号，`part` 是按顺序补出来的序号
    pub part_fabricated: bool,
    /// 播出时间 `YYYY-MM-DD HH:MM:SS`（栏目页列表返回）
    pub date: String,
}

impl Episode {
    pub fn is_main(&self) -> bool {
        self.part > 0
    }

    /// 从专辑剧集接口的一条记录构造。
    pub fn from_album_item(item: &serde_json::Value) -> Self {
        Episode {
            title: crate::util::json_str(item, "title"),
            video_id: crate::util::json_str(item, "id"),
            guid: crate::util::json_str(item, "guid"),
            url: crate::util::json_str(item, "url"),
            image: crate::util::json_str(item, "image"),
            length: crate::util::json_str(item, "length"),
            part: safe_int(item.get("part").unwrap_or(&serde_json::Value::Null)),
            sc: crate::util::json_str(item, "sc"),
            brief: crate::util::json_str(item, "brief"),
            ..Default::default()
        }
    }

    /// 从栏目往期接口的一条记录构造（带播出时间）。
    pub fn from_column_item(item: &serde_json::Value) -> Self {
        let mut date = crate::util::json_str(item, "time");
        if date.is_empty() {
            date = crate::util::json_str(item, "fdate");
        }
        let brief = crate::util::json_str(item, "brief");
        Episode {
            title: crate::util::strip_tags(&crate::util::json_str(item, "title")),
            video_id: crate::util::json_str(item, "id"),
            guid: crate::util::json_str(item, "guid"),
            url: crate::util::json_str(item, "url"),
            image: crate::util::json_str(item, "image"),
            length: crate::util::json_str(item, "length"),
            sc: crate::util::json_str(item, "sc"),
            brief: brief.trim().chars().take(120).collect(),
            date: date.trim().to_string(),
            ..Default::default()
        }
    }
}

/// 央视网栏目（tv.cctv.com/lm/xxx/）。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Column {
    pub name: String,
    /// TOPC 开头，喂 `getVideoListByColumn`；为空表示暂无往期接口
    pub topic_id: String,
    /// 栏目页地址
    pub site: String,
    /// 频道名，如 CCTV-1 综合
    pub channel: String,
    /// 一级分类
    pub fc: String,
    /// 二级分类
    pub sc: String,
}

impl Column {
    pub fn usable(&self) -> bool {
        !self.topic_id.is_empty()
    }
}

/// 一个可用清晰度。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Quality {
    /// CDN 码率代号：450 / 850 / 1200 / 2000 / 4000
    pub br: i64,
    /// 媒体播放列表（.m3u8）地址
    pub url: String,
    /// 展示名，如「超清 720P」
    pub label: String,
    /// master 声明的带宽（kbps）
    pub kbps: i64,
    pub width: i64,
    pub height: i64,
    /// 来源通道：enc / h5e / plain
    pub channel: String,
    /// enc/h5e 通道为央视私有加密（CDRM），标准通道下载会花屏
    pub encrypted: bool,
    #[serde(default)]
    pub segment_urls: Vec<String>,
}

impl Quality {
    pub fn resolution(&self) -> String {
        if self.width > 0 && self.height > 0 {
            format!("{}×{}", self.width, self.height)
        } else {
            String::new()
        }
    }
}

/// 单个视频的播放信息。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VideoInfo {
    pub guid: String,
    pub title: String,
    pub duration: String,
    /// 明文通道主播放列表（档位通常不全）
    pub hls_url: String,
    /// enc 通道主播放列表（档位最全）
    pub enc_hls_url: String,
    /// h5e 通道主播放列表
    pub h5e_hls_url: String,
    pub image: String,
    pub channel: String,
    pub brief: String,
    pub album_id: String,
    pub vsetid: String,
    /// 官方视频页地址（加密档只能去这里看）
    pub page_url: String,
    /// 原始 manifest（内部用，不下发前端）
    #[serde(skip)]
    pub manifest: serde_json::Map<String, serde_json::Value>,
    /// 原始响应（排错时看，不下发前端）
    #[serde(skip)]
    #[allow(dead_code)]
    pub raw: serde_json::Value,
}

impl VideoInfo {
    /// 按优先级返回 `(通道标识, 主播放列表地址)`。
    pub fn masters(&self) -> Vec<(&'static str, String)> {
        let mut result = Vec::new();
        if !self.enc_hls_url.is_empty() {
            result.push(("enc", self.enc_hls_url.clone()));
        }
        if !self.h5e_hls_url.is_empty() {
            result.push(("h5e", self.h5e_hls_url.clone()));
        }
        if !self.hls_url.is_empty() {
            result.push(("plain", self.hls_url.clone()));
        }
        result
    }

    /// 音频流地址（听音专区）。优先可用的 HLS 音频，其次兜底。
    pub fn audio_stream(&self) -> String {
        for key in ["hls_audio_url", "hls_url_audio", "audio_mp3"] {
            if let Some(value) = self.manifest.get(key).and_then(|v| v.as_str()) {
                if !value.is_empty() {
                    return value.to_string();
                }
            }
        }
        self.hls_url.clone()
    }
}

/// 关键词搜索结果条目。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchItem {
    pub id: String,
    pub title: String,
    pub all_title: String,
    pub url: String,
    pub image: String,
    pub duration: String,
    pub channel: String,
    pub upload_time: String,
}

/// 分页返回的通用外壳。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Paged<T> {
    pub total: i64,
    pub list: Vec<T>,
}
