/** 与 Rust 侧 serde 模型一一对应的类型（全部 camelCase）。 */

export interface Album {
  title: string;
  albumId: string;
  vsetid: string;
  url: string;
  image: string;
  count: number;
  year: string;
  area: string;
  sc: string;
  fc: string;
  actors: string;
  brief: string;
  firstVideoId: string;
  firstGuid: string;
}

export interface Episode {
  title: string;
  videoId: string;
  guid: string;
  url: string;
  image: string;
  length: string;
  part: number;
  sc: string;
  brief: string;
  partFabricated: boolean;
  date: string;
}

export interface Column {
  name: string;
  topicId: string;
  site: string;
  channel: string;
  fc: string;
  sc: string;
}

export interface Quality {
  br: number;
  url: string;
  label: string;
  kbps: number;
  width: number;
  height: number;
  channel: string;
  encrypted: boolean;
  segmentUrls: string[];
}

export interface SearchItem {
  id: string;
  title: string;
  allTitle: string;
  url: string;
  image: string;
  duration: string;
  channel: string;
  uploadTime: string;
}

export interface Paged<T> {
  total: number;
  list: T[];
}

export interface TingResult {
  section: string;
  page: number;
  url: string;
  list: Episode[];
}

export interface ColumnAllResult {
  total: number;
  list: Episode[];
  years: [number | null, number | null];
  blocks: number;
  /** 仍然超上限（封顶 1000 条）的时间块数 */
  truncated: number;
}

export interface ColumnResolved {
  columnId: string;
  title: string;
  site: string;
  candidates: Column[];
}

export interface PreviewTarget {
  guid: string;
  title: string;
  duration: string;
  channel: string;
  pageUrl: string;
  qualities: Quality[];
  quality: Quality | null;
  mediaUrl: string;
  encryptedOnly: boolean;
  audio: boolean;
  summary: string;
}

export interface PlayerInfo {
  name: string;
  path: string;
}

export interface EnvInfo {
  ffmpeg: string;
  ffplay: string;
  browser: string;
  defaultOutput: string;
  version: string;
  /** 运行环境标识（桌面版留空；后端可填自定义串，用于在界面上标注宿主环境） */
  runtime?: string;
}

export interface DownloadItem {
  guid: string;
  title: string;
  albumTitle: string;
  pageUrl: string;
}

export type DownloadChannel = 'standard' | 'hd' | 'audio';

export interface DownloadRequest {
  items: DownloadItem[];
  outputDir: string;
  br: number | null;
  channel: DownloadChannel;
  workers: number;
  makeMp4: boolean;
  keepTs: boolean;
  hdSeconds: number;
}

export interface TaskProgress {
  index: number;
  total: number;
  name: string;
  percent: number;
  message: string;
}

export interface TaskLog {
  level: 'ok' | 'error' | 'info';
  message: string;
}

export interface TaskDone {
  total: number;
  success: number;
  failed: number;
  cancelled: boolean;
  outputs: string[];
}

export interface ColumnProgress {
  done: number;
  total: number;
  text: string;
}

/** 界面上的模式。 */
export type Mode = 'search' | 'library' | 'column' | 'ting' | 'fourk';

/** 左列表里的一行（不同模式共享渲染逻辑）。 */
export interface ResourceRow {
  key: string;
  title: string;
  subtitle: string;
  /** 原始对象，供点击时取用 */
  payload: unknown;
  /** 是否可点击加载（例如没有 TOPC id 的栏目不可用） */
  disabled?: boolean;
  badge?: string;
}

/** 档位分辨率的展示文本（如 `1280×720`）。 */
export function resolutionOf(quality: Quality): string {
  return quality.width > 0 && quality.height > 0 ? `${quality.width}×${quality.height}` : '';
}
