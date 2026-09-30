/** 与 Rust 后端的所有交互都集中在这里。 */

import { invoke } from '@tauri-apps/api/core';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';

import type {
  Album,
  Column,
  ColumnAllResult,
  ColumnProgress,
  ColumnResolved,
  DownloadRequest,
  EnvInfo,
  Episode,
  Paged,
  PlayerInfo,
  PreviewTarget,
  Quality,
  SearchItem,
  TaskDone,
  TaskLog,
  TaskProgress,
  TingResult,
} from './types';

export const api = {
  envInfo: () => invoke<EnvInfo>('env_info'),

  // 搜索 / 片库 / 专辑
  searchVideos: (keyword: string, page = 1, pageSize = 30) =>
    invoke<Paged<SearchItem>>('search_videos', { keyword, page, pageSize }),
  listAlbums: (
    fc: string,
    options: { sc?: string; area?: string; year?: string; letter?: string; page?: number; pageSize?: number } = {},
  ) =>
    invoke<Paged<Album>>('list_albums', {
      fc,
      sc: options.sc ?? '',
      area: options.area ?? '',
      year: options.year ?? '',
      letter: options.letter ?? '',
      page: options.page ?? 1,
      pageSize: options.pageSize ?? 30,
    }),
  listAlbumEpisodes: (albumId: string, maxPages = 6) =>
    invoke<Episode[]>('list_album_episodes', { albumId, maxPages }),
  libraryCategories: () => invoke<string[]>('library_categories'),

  // 栏目
  resolveColumn: (input: string) => invoke<ColumnResolved>('resolve_column', { input }),
  listColumnAll: (columnId: string, since = '', until = '', workers = 6) =>
    invoke<ColumnAllResult>('list_column_all', { columnId, since, until, workers }),
  searchColumns: (keyword: string, onlyUsable = false) =>
    invoke<Column[]>('search_columns', { keyword, onlyUsable }),
  columnFilters: () => invoke<{ categories: string[]; channels: string[] }>('column_filters'),
  browseColumns: (category = '', channel = '', page = 1, pageSize = 40) =>
    invoke<Paged<Column>>('browse_columns', { category, channel, page, pageSize }),

  // 4K / 听音
  list4kAlbums: (page = 1, pageSize = 60) =>
    invoke<Paged<Album>>('list_4k_albums', { page, pageSize }),
  listTingItems: (section: string, page = 1, pageSize = 60) =>
    invoke<TingResult>('list_ting_items', { section, page, pageSize }),
  resolveTingGuids: (items: Episode[]) => invoke<Episode[]>('resolve_ting_guids', { items }),
  tingSections: () => invoke<string[]>('ting_sections'),

  // 清晰度 / 预览
  listQualities: (guid: string) => invoke<Quality[]>('list_qualities', { guid }),
  previewResolve: (guid: string, br: number | null, audio: boolean, pageUrl: string) =>
    invoke<PreviewTarget>('preview_resolve', { guid, br, audio, pageUrl }),
  previewPlayers: () => invoke<PlayerInfo[]>('preview_players'),
  previewPlay: (url: string, title: string, player = '') =>
    invoke<number>('preview_play', { url, title, player }),
  previewStop: () => invoke<void>('preview_stop'),
  previewFrame: (mediaUrl: string, guid: string, at: number, width = 640) =>
    invoke<string>('preview_frame', { mediaUrl, guid, at, width }),
  previewSaveFrame: (mediaUrl: string, at: number, width: number, outPath: string) =>
    invoke<string>('preview_save_frame', { mediaUrl, at, width, outPath }),
  openUrl: (url: string) => invoke<void>('open_url', { url }),

  // 下载
  startDownload: (request: DownloadRequest) => invoke<void>('start_download', { request }),
  cancelTask: () => invoke<void>('cancel_task'),
  defaultOutputDir: () => invoke<string>('default_output_dir'),
};

export const events = {
  onDownloadProgress: (callback: (payload: TaskProgress) => void): Promise<UnlistenFn> =>
    listen<TaskProgress>('download:progress', (event) => callback(event.payload)),
  onDownloadLog: (callback: (payload: TaskLog) => void): Promise<UnlistenFn> =>
    listen<TaskLog>('download:log', (event) => callback(event.payload)),
  onDownloadDone: (callback: (payload: TaskDone) => void): Promise<UnlistenFn> =>
    listen<TaskDone>('download:done', (event) => callback(event.payload)),
  onColumnProgress: (callback: (payload: ColumnProgress) => void): Promise<UnlistenFn> =>
    listen<ColumnProgress>('column:progress', (event) => callback(event.payload)),
};

/** 把后端返回的错误（字符串）统一成 Error。 */
export function toError(value: unknown): Error {
  if (value instanceof Error) return value;
  if (typeof value === 'string') return new Error(value);
  return new Error(String(value));
}
