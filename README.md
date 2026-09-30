# 央视工具箱
> **许可证**：PolyForm Noncommercial License 1.0.0 — 允许个人学习、研究、修改，**禁止任何商业用途**。详见 [LICENSE](LICENSE)。
通过分析央视网（CCTV）的接口，实现 **片库视频下载 / 栏目往期 / 听音音频 / 视频预览** 的一站式桌面工具。

技术栈：**Tauri v2 + React 19 + Vite 8 + Rust 2021**（后端全部为 Rust，前端为 React + TypeScript）。

| 模块 | 支持的玩法 |
| --- | --- |
| **片库视频** | 关键词搜索、片库分类浏览、栏目页往期（内置 346 个栏目）、4K 专区、直接粘贴链接解析 |
| **听音音频** | [听音专区](https://tv.cctv.com/ty/m/index.shtml) 9 个分区的播客 / 评书 / 戏曲 / 文化节目，纯音频直连下载 |
| **视频预览** | 下载前先看一眼：明文档位窗口内嵌播放（hls.js 直连 CDN）+ 抓帧出图（可换帧、另存）+ 一键交给 ffplay / mpv / PotPlayer / VLC |

视频下载提供三条通道：

- **标准通道**（默认，快）：直接并发下载 HLS 分片后 ffmpeg 无损转封装为 MP4。
  - **4K 专区视频有明文 1080P（br=4000）**，标准通道即可下载，1920×1080；
  - 普通片库 / 栏目 / 搜索视频的明文通道最高只有 360P（850 档，且 1200/2000 路径会被 CDN 回退成低档假流）。
- **高清通道**：enc/h5e 高清流（720P）分片是**央视私有加密**的（CDRM），解密器为官方播放页内的
  WASM 模块。本工具用无头 Chromium（CDP 协议）驱动真实播放页完成解密，拦截解密后的 fMP4 流
  合成为 MP4，产物 **1280×720**。速度约为播放速度的数倍。
- **听音通道**：音频是明文的，无需解密，产物为 `.m4a`（AAC 48kHz）。

清晰度选择逻辑：加密档（enc/h5e）在界面上统一折叠为「高清通道」一项，避免误走标准通道下载出花屏文件。

---

## 一、快速开始

### 方式 1：直接运行编译好的程序

双击运行: 

```bat
rem release 版（7.5 MB，自带前端资源，可独立运行）
src-tauri\target\release\cctv-toolbox.exe
```

> 注意：**debug 版的 exe 不能单独运行**。Tauri 的 debug 构建走 `devUrl`，
> 必须有活着的 Vite dev server，所以 debug 只能通过 `npm run app` 启动。

### 方式 2：开发模式

```bat
npm install
npm run app          rem = tauri dev，起 Vite dev server + 编译并启动 Rust 后端
```

首次编译 Rust 依赖约 5 分钟，之后增量编译很快。

### 方式 3：自己打包

```bat
npm run dist         rem = tauri build
```

产出 `src-tauri\target\release\cctv-toolbox.exe`。若还要 NSIS 安装包，
`tauri build` 需要联网下载 `nsis-3.11.zip`；网络不通该步骤会以
`failed to bundle project: timeout: global` 失败，**但 exe 已经产出且可正常使用**
（安装包失败不影响程序本身）。

### 界面用法

界面顶部共有**五种**来源模式（**切换模式会清空上一个模式已加载的列表与选择**）：

1. **关键词搜索**：输入剧名 / 节目名，点「搜索」，左侧列出结果（单击选中，双击直接下载第 1 集）。
2. **片库浏览**：选分类（电视剧 / 动画片 / 纪录片 / 特别节目），可再填题材、年份，点「加载」。
3. **栏目页**：**输入栏目名即可**（如 `今日说法`，无需网址），模糊匹配时左侧会列出候选栏目供点选；
   也可以粘贴栏目页地址。默认勾选「全部往期」—— 会自动逐年拉取该栏目**全部年份**的往期
   （今日说法可回溯到 2008 年，共 5000+ 期），也可用年份范围缩小；按年过滤时只取当年。
   本模式还带 **「栏目大全」筛选行**（对应 <https://tv.cctv.com/lm/index.shtml>）：
   选分类（新闻 / 影视 / 纪实…）与播出频道（CCTV-1…CCTV-17）后点「浏览栏目大全」，
   左侧列出 346+ 个央视栏目，**单击任意一条即加载它的往期**，不用记栏目名。
4. **听音**：选分区（首页 / 热听榜 / 历史 / 电视剧 / 全部 / 文化 / 健康课堂 / 戏曲 / 听书社区），
   点「加载听音」→ 左侧列出音频节目（单击即加载、双击即下载该条）。
5. **4K专区**：点「浏览 4K 专辑」→ 左侧列出全部 4K 专辑（记住乡愁、军武零距离、考古公开课…），
   单击专辑 → 右侧列出剧集 → 下载即得**蓝光 1080P**（明文直连，速度快，无需浏览器解密）。

通用操作：

- 左侧 **单击** 资源 → 右侧列出剧集，并自动探测可用清晰度（**双击** 资源可直接下载第 1 集）。
- 右侧 **单击勾选** 要下载的集（支持「全选 / 取消全选 / 反选」），选好清晰度与输出目录后点「下载选中集」。
- **预览**：选中条目后点「预览选中」（听音模式下是「试听选中」），弹出预览窗口——
  自动解析明文最高档：窗口内嵌 hls.js 直接播放（央视 CDN 返回 `Access-Control-Allow-Origin: *`，
  无需本地代理）；「取一帧」把当前帧抓成图，可换时间点重取、可「另存为图片」；
  「外部播放器」下拉可把流交给 ffplay / mpv / PotPlayer / VLC 播放；也能复制流地址、打开官方页面。
  只有加密档的视频会如实提示并引导「打开视频页」在官网观看。
- 片库 / 栏目 / 4K 模式下的 **「直接链接」** 框：把视频页链接粘进去点「解析链接」，
  解析结果落到右侧列表，勾选即可下载 / 预览。
- 右上角可切换深色 / 浅色主题。

---

## 二、项目结构

```
VideoDownloader/
├── src/                           React 前端
│   ├── App.tsx                    主界面：五种模式 + 双列表 + 下载队列 + 日志
│   ├── components/
│   │   └── PreviewDialog.tsx      预览弹窗（内嵌 hls.js / 取帧 / 外部播放器）
│   ├── lib/
│   │   ├── api.ts                 与后端的所有交互（invoke + 事件订阅）
│   │   ├── harden.ts              产物加固：禁右键菜单与开发者工具快捷键
│   │   └── types.ts               与 Rust 模型一一对应的类型
│   ├── styles.css                 深色 / 浅色主题
│   └── main.tsx
├── src-tauri/                     Rust 后端
│   ├── src/
│   │   ├── lib.rs                 应用入口、命令注册
│   │   ├── commands.rs            Tauri 命令层（把后端能力暴露给前端）
│   │   ├── http.rs                浏览器一致性请求头、重试、并发映射、EcoQoS 关闭
│   │   ├── cctv/                  央视网接口客户端
│   │   │   ├── search.rs          全站关键词搜索
│   │   │   ├── library.rs         片库分类浏览
│   │   │   ├── album.rs           专辑剧集列表
│   │   │   ├── column.rs          栏目页往期（含跨年全量拉取）
│   │   │   ├── columns_dir.rs     栏目大全与内置栏目清单
│   │   │   ├── fourk.rs           4K 专区
│   │   │   ├── ting.rs            听音专区
│   │   │   └── video.rs           视频源、清晰度、分片解析（vdn 签名）
│   │   ├── download.rs            HLS 并发分片下载、合并与 ffmpeg 转封装
│   │   ├── hd.rs                  高清通道：CDP 驱动浏览器解密 720P
│   │   ├── cdp.rs                 极简 Chrome DevTools Protocol 客户端
│   │   ├── preview.rs             视频预览：明文档位解析、取帧、外部播放器调度
│   │   ├── ffmpeg.rs              ffmpeg / ffplay 探测与调用
│   │   ├── model.rs               数据模型
│   │   ├── util.rs                通用小工具
│   │   └── smoke.rs               联网冒烟测试（`cargo test`）
│   ├── data/columns.json          内置栏目清单快照（346 个栏目）
│   ├── capabilities/default.json  权限声明
│   └── tauri.conf.json            窗口 / 打包配置
├── docs/
│   └── CCTV片库接口分析.md         逆向分析全过程与接口清单
├── downloads/                     默认输出目录
└── README.md
```

---

## 三、功能与参数

| 能力 | 说明 |
| --- | --- |
| 关键词搜索 | 全站视频搜索，返回标题、时长、频道、播放页 |
| 片库分类浏览 | 电视剧 / 动画片 / 纪录片 / 特别节目，支持题材、年份筛选与分页 |
| 栏目往期 | **内置 346 个栏目清单**（244 个支持往期接口），输入栏目名即用；「全部往期」逐年分块拉取（最早 2008 年），突破接口「不指定年份深翻页只能到约 30 页」的限制 |
| 栏目大全浏览 | `columnSearch` 接口按**分类 / 播出频道**筛选 346+ 个栏目，单击即加载往期 |
| 听音专区 | tvty 服务：9 个分区音频节目，纯音频明文下载为 m4a |
| 视频预览 | 窗口内嵌 hls.js 播放明细流 + 取帧出图（可换帧 / 另存 1280 宽截图）+ 一键交给 ffplay / mpv / PotPlayer / VLC；只有加密档的视频自动引导打开官方页 |
| 4K 专区 | `getLastVideoList4K` 列出全部专辑 → 剧集 → **明文蓝光 1080P** 直连下载 |
| 直接链接 | 粘贴视频页地址即可解析成可下载条目 |
| 剧集列表 | 自动过滤「短看点」，只保留正片并按集号排序 |
| 清晰度选择 | 加密档（enc/h5e，最高 720P）统一折叠为「高清通道」；明文档（270P/360P/1080P）可直接下载 |
| 并发下载 | 并发分片下载，默认 8 并发，失败自动重试；支持随时取消 |
| 输出格式 | 视频：分片合并为 TS，再调用 ffmpeg 无损转封装为 MP4（无 ffmpeg 时保留 TS）；音频：m4a |
| 批量下载 | 勾选多条批量下载，单条失败不影响其他；进度与日志实时推送到界面 |

---

## 四、技术说明

央视网的点播视频走 **HLS** 分发，下载链路为：

```
视频页 --(guid)--> getHttpVideoInfo.do --(hls_url)--> 主播放列表
       --(码率变体)--> 媒体播放列表 --(N 个 .ts 分片)--> 合并 --(ffmpeg)--> MP4
```

需要注意的坑（详见 `docs/CCTV片库接口分析.md`）：

1. **`getHttpVideoInfo.do` 需要签名**：播放器把签名算法写在混淆过的 `vodplayer.js` 里，
   参数为 `tsp`（10 位秒级时间戳）、`vn=2049`、`vc = MD5(tsp + vn + 盐 + uid).upper()`。
   盐值已从混淆代码中还原并固化在 `cctv/video.rs`。
2. **接口要的是 guid，不是页面 URL 里的 id**：页面 URL 中的 `VIDE…` 是内容 ID，
   而视频源接口需要 32 位十六进制的 guid。专辑 / 栏目剧集接口会直接给出 guid；
   搜索结果只有页面地址，需要抓页面解析 `var guid = "…"`。
3. **enc / h5e 通道是私有加密流**：`manifest.hls_enc_url`、`hls_h5e_url` 声明了完整档位
   （450/850/1200/2000），但其分片的 H264 负载是密文（无 `#EXT-X-KEY`，解码报
   `top block unavailable`，画面花屏）。解密器是播放页 WASM 模块，因此 720P 必须
   走 `hd.rs` 的浏览器解密通道。明文通道 `/asp/hls/` 的 1200/2000 路径会被 CDN
   回退成 480×270 假流，不可信。
4. **4K 专区例外：存在明文 1080P**：4K 专区视频（如《记住乡愁》《军武零距离》）的
   plain master 被 `?maxbr=` 参数压档，但 CDN 上 `/asp/hls/4000/` 路径实际存在、
   未加密、实测 1920×1080 解码无花屏。代码直接按路径探测该档，命中后作为
   「蓝光 1080P」参与清晰度排序。enc2 通道在所有已测 CDN 主机上均返回 403，暂不可用。
5. **栏目「只有最近一年」的真相**：`getVideoListByColumn` 不指定 `d` 参数时深翻页
   有硬上限（约 30 页），且 `total` 超过 1000 会被封顶成 1000（假值）。要拿全量往期
   必须带 `d=YYYY`（年）或 `d=YYYYMM`（月）分块请求 —— 年份过滤可一路回溯到 2008 年。
   `cctv/column.rs` 先用约 6 次请求二分定位最早年份，再逐年（每年超 1000 期时细分为月）
   并发拉取，按 guid 去重排序。栏目 ID 藏在栏目页内联脚本里，变量名有
   `lmtopId / topicID / topicId` 等写法，用一条正则统一匹配；内置栏目清单来自
   `api.cntv.cn/lanmu/columnSearch`（快照在 `src-tauri/data/columns.json`）。
6. **栏目大全只给 EPGM id，不给往期用的 TOPC id**：`columnSearch` 的 `doc` 里只有
   `column_website`（栏目页地址），往期接口要的是 TOPC…。`columns_dir::resolve_column_topic()`
   先查内置清单，查不到就抓栏目页解析。**「浏览栏目大全」会在返回前对没命中的条目并发补解析**，
   否则界面里双击这些栏目会因为拿不到 TOPC id 而拉不到往期。
7. **片库的 `fc` 必须传中文分类名**：`fc=纪录片` 有数据，`fc=jlp` 会返回
   `{"list":[],"total":0}`。`dsj / dhp / jlp / tbjm` 是央视网内部页面用的缩写，接口不认。
8. **听音（tvty）有两条取数路径**：带 `var param = "PAGE…"` 的分区走
   `getVideoListByPageIdTvty?serviceId=tvty&id=<PAGE…>`，直接给 guid；
   **该接口不接受 `p` 参数**（带上会返回 `errcode 1001 url error`，表现为「分区 0 条」），
   只能靠 `n` 控制条数。首页 / 热听榜是静态 HTML，需抓 `<a href="…/VIDA….shtml">` 再用
   `getVideoListByAlbumIdNew?serviceId=tvty` 把 VIDA 换成 guid。
   音频流取 `manifest.hls_audio_url`（主表三档，需先落到具体档位）；
   `manifest.audio_mp3` 路径已下线（404），仅作兜底。
   「全部」分区是纯 JS 渲染页（既无 `var param` 也无 VIDA 链接），静态解析拿不到条目 —— 这是固化行为，不是 bug。
9. **预览为什么只能走明文档位**：点播的 720P 及以上档位只存在于 enc/h5e 加密通道，
   ffmpeg / ffplay 拿到的 H264 负载是密文，解码即花屏。因此预览只在明文通道里选档
   （450/850，4K 专区另有明文 4000）。**央视 CDN 返回 `Access-Control-Allow-Origin: *`**，
   所以窗口内嵌播放由前端 hls.js 直连 CDN 即可，不需要本地代理；取帧则是
   `ffmpeg -headers "Referer: …"` 直接解码明文 HLS。
   （曾尝试过的直播提取 / 录制因直播流同为 CDRM 加密、且自动化环境无法触发网页解密器，
   已在 v1.2 移除，逆向记录保留在 docs 第九章。）
10. **WAF 会校验「浏览器一致性」请求头**：`api.cntv.cn` 现在要求 UA 声称是 Chrome 的
    请求必须带齐 `sec-ch-ua` / `Sec-Fetch-*` 系列头，否则一律返回
    **302 且 Location 指向自身**（等于拒绝，表现为整站接口全挂）。
    只换 UA 或加 Accept 都没用，必须带上完整的一套（见 `http.rs` 的 `BASE_HEADERS`）。
    这种自指 302 也被当作瞬时限流信号做退避重试。
11. **Windows 会把「后台进程」降到 EcoQoS 慢档**：窗口不在前台时，同一个 HTTPS
    请求实测从 ~280ms 涨到 ~650ms（2.3 倍，开关对照 3 轮结论一致），整个下载器都变「卡」。
    `http::disable_power_throttling()` 在启动时显式关闭该节流
    （`SetProcessInformation(ProcessPowerThrottling)`，直接 extern kernel32，不引额外 crate）。
12. **加载链路全程并发**：单次请求约 190ms，任何「要打多个同构请求」的地方都走
    `http::parallel_map()`（保序、单任务失败不拖垮整批）——年份探测 / 时间块组装 /
    分块翻页 / 各通道 master 解析 / 明文加档与 1080P 探测 / 听音 guid 批量解析 /
    栏目大全 TOPC 补解析，全部并发。同一年份的条数探测有缓存，避免重复提问。
    今日说法 5449 期全量往期实测 **1.4s**。
13. **Rust 依赖走镜像**：本网络直连 `index.crates.io` 会出现 <10 B/s 的超时，
    `src-tauri/.cargo/config.toml` 已把索引指向 `rsproxy-sparse` 稀疏镜像
    （项目级配置，不动全局 `~/.cargo`）。
14. **应用图标与滚动条跟随主题**：图标取自 tv.cctv.com 的官方站点图标
    （apple-touch-icon，200×200，Lanczos 放大到 1024 后做圆角透明），
    `npx tauri icon` 生成全套（ico / icns / png）。要换图标只需替换
    `src-tauri/icons/` 下的文件（或重新跑 `npx tauri icon <图>`）。
    滚动条用 `::-webkit-scrollbar` 系列按主题变量着色
    （深色 `#333c4d` / 浅色 `#c6cfdc`，悬停加深，按下为主题色）；
    **不要**改用标准的 `scrollbar-width` / `scrollbar-color` —— Chromium 121+
    里标准属性非 auto 时 webkit 规则会被整段忽略，深色主题下会退回白色系统滚动条。

---

## 五、测试

后端带一组**联网冒烟测试**，覆盖每个接口的真实返回（含一次真实的短视频下载）：

```bat
cd src-tauri
cargo test --lib -- --nocapture --test-threads=1
```

会打印各接口拿到的条数、解析出的清晰度、下载产物大小等，接口契约一变就会报红。

---

## 六、产物加固

分三层，任何一层单独失效都还有兜底：

| 层 | 位置 | 做法 | 效果 |
| --- | --- | --- | --- |
| 引擎层 | `src-tauri/Cargo.toml` | `tauri` **不开** `devtools` feature | release 构建下 `tauri-runtime-wry` 里 `with_devtools(...)` 被 `cfg` 整个编译掉，落到 wry 默认值 `devtools: false` → WebView2 `AreDevToolsEnabled(FALSE)`，**F12 / Ctrl+Shift+I 在内核层就无效** |
| 权限层 | `src-tauri/capabilities/default.json` | 显式 `core:webview:deny-internal-toggle-devtools` | `core:default` 自带 `allow-internal-toggle-devtools`；这里显式 deny（deny 优先），即便将来误开了 feature，前端 IPC 也调不动 |
| 界面层 | `src/lib/harden.ts` | 拦 `contextmenu` + 开发者工具/重载类快捷键 + 拖放 | 右键不再弹原生菜单；F12、Ctrl+Shift+I/J/C、Ctrl+U/R/P/S、F5 全部吞掉；拖文件进窗口不再顶掉界面 |

界面层只在**非开发构建**生效（`import.meta.env.DEV` 为真时直接返回），所以
`npm run app` 调试时右键"检查元素"照常可用；打包产物里 Vite 会把守卫折叠掉，逻辑变成无条件生效。

注意两点：

- `Ctrl+C` / `Ctrl+V` / `Ctrl+A` **不受影响**，输入框里正常复制粘贴（只挡了带 Shift 的开发者工具组合键）。
- 右键菜单被禁后，输入框里不能用右键粘贴，请用 `Ctrl+V`。

做法上刻意**没有**在 `tauri.conf.json` 里写 `"devtools": false`：那个字段在 debug 构建下同样会被读取，
会把开发模式的调试能力一起关掉。靠"release 不开 feature"来区分，开发与交付互不影响。

---

## 七、许可证与合规提醒

# 许可证
本项目采用 PolyForm Noncommercial License 1.0.0（SPDX: PolyForm-Noncommercial-1.0.0）。

这是一份源码可见（Source-Available）许可，不是开源协议。它授予你：

修改：可以基于本项目进行更改、创建衍生作品

非商业分发：可以向他人分享原版或修改后的副本

禁止商业用途：不得用于任何商业目的，包括但不限于：

  将本工具或其衍生品用于盈利性服务
  
  二次打包、转售、或作为付费产品的一部分分发
  
  在公司/商业组织内部作为生产工具使用
  
  任何超出上述范围的用途，需事先获得作者的书面商业授权。

# 合规提醒
本工具通过分析公开网页接口实现功能，**仅供个人学习、研究与技术交流**。使用时请务必遵守：

央视网用户协议：
本工具不对抗、不绕过央视网的访问控制与付费/会员机制。请勿使用本工具获取任何需要付费、登录或授权才能观看的内容。

著作权规定：
下载的音视频内容版权归央视及相关权利人所有，请勿二次传播、公开发布或用于任何商业用途。个人下载后请自行妥善保管。

接口稳定性：
所有接口均为逆向分析所得，央视可能随时变更。本项目不承诺长期可用，也不提供任何形式的担保。

不鼓励批量抓取：
请控制请求频率，避免对央视服务器造成压力。工具内置的并发策略已针对正常使用场景优化，请勿改写用于大规模爬取。

责任自负：
使用者应自行承担因使用本工具而产生的一切法律风险。作者不对任何滥用行为负责。

如果你认可上述条款，欢迎学习、fork、改进；如果你希望将其用于商业场景，请先联系作者获取授权。

