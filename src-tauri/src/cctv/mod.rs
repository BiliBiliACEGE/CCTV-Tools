//! 央视网（CCTV）接口客户端。
//!
//! 模块划分：
//! * [`search`]       —— 全站关键词搜索
//! * [`library`]      —— 片库分类浏览（剧集 / 动画 / 纪录片 / 特别节目）
//! * [`album`]        —— 专辑剧集列表
//! * [`column`]       —— 栏目页往期（含全量跨年拉取）
//! * [`columns_dir`]  —— 栏目大全与内置栏目清单
//! * [`fourk`]        —— 4K 专区
//! * [`video`]        —— 视频源、清晰度、分片解析（vdn 签名）
//! * [`ting`]         —— 听音专区音频节目

pub mod album;
pub mod column;
pub mod columns_dir;
pub mod fourk;
pub mod library;
pub mod search;
pub mod ting;
pub mod video;

/// 片库频道分类（`fc` 参数）。
///
/// **注意：这里必须传中文分类名本身**，不能传 dsj / jlp 这类缩写——
/// 实测 `fc=jlp` 会返回 `{"list":[],"total":0}`，`fc=纪录片` 才有数据。
/// 缩写码是央视网内部页面用的，接口不认。
pub const LIBRARY_CATEGORIES: &[&str] = &["电视剧", "动画片", "纪录片", "特别节目"];
