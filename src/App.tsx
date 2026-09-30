/** 央视工具箱主界面：五种来源模式 + 双列表 + 下载与预览。 */

import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import type { MouseEvent as ReactMouseEvent } from 'react';
import { open as openDialog } from '@tauri-apps/plugin-dialog';

import PreviewDialog from './components/PreviewDialog';
import HoverCard from './components/HoverCard';
import type { HoverInfo } from './components/HoverCard';
import { api, events, toError } from './lib/api';
import { resolutionOf } from './lib/types';
import type {
  Album,
  Column,
  DownloadChannel,
  Episode,
  EnvInfo,
  Mode,
  PreviewTarget,
  Quality,
  ResourceRow,
  TaskLog,
} from './lib/types';

const MODES: { key: Mode; label: string }[] = [
  { key: 'search', label: '关键词搜索' },
  { key: 'library', label: '片库浏览' },
  { key: 'column', label: '栏目页' },
  { key: 'ting', label: '听音' },
  { key: 'fourk', label: '4K专区' },
];

/** 片库分类以后端下发为准，这份只是后端没起来时的兜底。 */
const FALLBACK_LIBRARY_CATEGORIES = ['电视剧', '动画片', '纪录片', '特别节目'];
const FALLBACK_TING_SECTIONS = ['首页', '热听榜', '历史', '电视剧', '全部', '文化', '健康课堂', '戏曲', '听书社区'];
const YEARS = ['', ...Array.from({ length: 12 }, (_, index) => String(new Date().getFullYear() - index))];

/** 栏目往期的年份范围（`all` 表示从最早一期开始）。 */
const COLUMN_RANGES: { value: string; label: string }[] = [
  { value: 'all', label: '全部往期' },
  { value: '10', label: '最近 10 年' },
  { value: '5', label: '最近 5 年' },
  { value: '3', label: '最近 3 年' },
  { value: '1', label: '最近 1 年' },
];

/** 期数展示：有真实期号就用期号，否则用列表序号。 */
function episodeNumber(episode: Episode, index: number): string {
  return episode.part > 0 ? String(episode.part) : String(index + 1);
}

/** 剧集（右列表）的悬浮详情。 */
function episodeHover(episode: Episode, index: number): HoverInfo {
  const brief = (episode.brief || '').replace(/\s+/g, ' ').trim();
  return {
    title: episode.title || '（无标题）',
    image: episode.image || undefined,
    fields: [
      { label: '期数', value: `第 ${episodeNumber(episode, index)} 期${episode.partFabricated ? '（推算）' : ''}` },
      { label: '播出', value: episode.date || '' },
      { label: '时长', value: episode.length || '' },
      { label: '频道', value: episode.sc || '' },
      { label: 'guid', value: episode.guid || '（待解析）' },
      { label: '视频页', value: episode.url || '' },
    ],
    note: brief ? (brief.length > 160 ? `${brief.slice(0, 160)}…` : brief) : undefined,
  };
}

/** 左列表条目的悬浮详情。 */
function resourceHover(row: ResourceRow, mode: Mode): HoverInfo {
  const payload = row.payload as Record<string, unknown>;
  const text = (key: string) => {
    const value = payload?.[key];
    return value === undefined || value === null ? '' : String(value);
  };
  if (mode === 'library' || mode === 'fourk') {
    const count = text('count');
    return {
      title: row.title,
      image: text('image') || undefined,
      fields: [
        { label: '分类', value: text('fc') },
        { label: '年份 / 地区', value: [text('year'), text('area')].filter(Boolean).join(' · ') },
        { label: '主题', value: text('sc') },
        { label: '集数', value: count ? `${count} 集` : '' },
        { label: '主演', value: text('actors') },
        { label: '首集 guid', value: text('firstGuid') },
      ],
      note: text('brief') || undefined,
    };
  }
  if (mode === 'column') {
    return {
      title: row.title,
      fields: [
        { label: '往期 ID', value: text('topicId') || '（该栏目没有往期接口）' },
        { label: '频道', value: text('channel') },
        { label: '分类', value: [text('fc'), text('sc')].filter(Boolean).join(' · ') },
        { label: '栏目页', value: text('site') },
      ],
      note: text('topicId') ? '双击加载全部往期' : '没有 TOPC 往期 ID，无法按栏目拉取',
    };
  }
  // 搜索 / 听音条目本身就是视频
  return {
    title: row.title,
    image: text('image') || undefined,
    fields: [
      { label: '频道', value: text('channel') || text('sc') },
      { label: '时长', value: text('duration') || text('length') },
      { label: '发布时间', value: text('uploadTime') || text('date') },
      { label: 'videoId', value: text('id') || text('videoId') },
      { label: 'guid', value: text('guid') || '（待解析）' },
      { label: '视频页', value: text('url') },
    ],
  };
}


/** 把专辑转成左列表行。 */
function albumRow(album: Album, index: number): ResourceRow {
  const parts = [album.year, album.area, album.sc].filter(Boolean);
  if (album.count) parts.push(`${album.count} 集`);
  return {
    key: `album-${album.albumId || index}`,
    title: album.title,
    subtitle: parts.join(' · '),
    payload: album,
    badge: album.fc === '4K专区' ? '4K' : undefined,
  };
}

function columnRow(column: Column, index: number): ResourceRow {
  return {
    key: `column-${column.topicId || column.site || index}`,
    title: column.name,
    subtitle: [column.channel, column.fc, column.sc].filter(Boolean).join(' · '),
    payload: column,
    disabled: !column.topicId,
    badge: column.topicId ? undefined : '无往期接口',
  };
}

function videoRow(episode: Episode, index: number): ResourceRow {
  return {
    key: `video-${episode.guid || episode.videoId || index}`,
    title: episode.title,
    subtitle: [episode.sc, episode.length, episode.date].filter(Boolean).join(' · '),
    payload: episode,
  };
}

export default function App() {
  const [theme, setTheme] = useState<'dark' | 'light'>('dark');
  const [mode, setMode] = useState<Mode>('search');
  const [env, setEnv] = useState<EnvInfo | null>(null);

  // 参数
  const [keyword, setKeyword] = useState('');
  const [libCategory, setLibCategory] = useState(FALLBACK_LIBRARY_CATEGORIES[0]);
  const [libCategoryOptions, setLibCategoryOptions] = useState(FALLBACK_LIBRARY_CATEGORIES);
  const [libYear, setLibYear] = useState('');
  const [libSc, setLibSc] = useState('');
  const [columnInput, setColumnInput] = useState('');
  const [directLink, setDirectLink] = useState('');
  const [colCategory, setColCategory] = useState('全部');
  const [colChannel, setColChannel] = useState('全部');
  const [colFilters, setColFilters] = useState<{ categories: string[]; channels: string[] }>({
    categories: [],
    channels: [],
  });
  const [tingSections, setTingSections] = useState<string[]>(FALLBACK_TING_SECTIONS);
  const [tingSection, setTingSection] = useState(FALLBACK_TING_SECTIONS[0]);
  /** 栏目往期年份范围：all / 10 / 5 / 3 / 1 */
  const [columnRange, setColumnRange] = useState('all');
  /** 右列表筛选与排序（栏目往期用） */
  const [episodeKeyword, setEpisodeKeyword] = useState('');
  const [episodeYear, setEpisodeYear] = useState('all');
  const [episodeOrder, setEpisodeOrder] = useState<'desc' | 'asc'>('desc');
  /** 悬浮详情卡片 */
  const [hover, setHover] = useState<{ info: HoverInfo; x: number; y: number } | null>(null);

  // 列表
  const [resources, setResources] = useState<ResourceRow[]>([]);
  const [resourceHint, setResourceHint] = useState('');
  const [resourceLoading, setResourceLoading] = useState(false);
  const [activeResourceKey, setActiveResourceKey] = useState<string | null>(null);
  const [episodes, setEpisodes] = useState<Episode[]>([]);
  const [episodeTitle, setEpisodeTitle] = useState('');
  const [episodeHint, setEpisodeHint] = useState('');
  const [episodeLoading, setEpisodeLoading] = useState(false);
  const [selectedEpisodes, setSelectedEpisodes] = useState<Set<number>>(new Set());

  // 清晰度
  const [qualities, setQualities] = useState<Quality[]>([]);
  const [qualityBr, setQualityBr] = useState('auto');
  const [probing, setProbing] = useState(false);
  const probeToken = useRef(0);

  // 下载
  const [channel, setChannel] = useState<DownloadChannel>('standard');
  const [outputDir, setOutputDir] = useState('');
  const [workers, setWorkers] = useState(8);
  const [hdSeconds, setHdSeconds] = useState(0);
  const [busy, setBusy] = useState(false);
  const [status, setStatus] = useState('');
  const [percent, setPercent] = useState(0);
  const [logs, setLogs] = useState<TaskLog[]>([]);
  const logRef = useRef<HTMLDivElement | null>(null);

  // 预览
  const [preview, setPreview] = useState<PreviewTarget | null>(null);
  const [previewLoading, setPreviewLoading] = useState(false);
  const [previewAudio, setPreviewAudio] = useState(false);

  const pushLog = useCallback((level: TaskLog['level'], message: string) => {
    setLogs((previous) => [...previous.slice(-300), { level, message }]);
  }, []);

  // 主题
  useEffect(() => {
    document.documentElement.dataset.theme = theme;
  }, [theme]);

  // 初始化
  useEffect(() => {
    api
      .envInfo()
      .then((info) => {
        setEnv(info);
        if (info.defaultOutput) setOutputDir(info.defaultOutput);
        pushLog('info', `ffmpeg：${info.ffmpeg || '未检测到（将保存为 .ts）'}`);
        pushLog('info', `高清通道浏览器：${info.browser || '未检测到（Edge / Chrome 都没有）'}`);
      })
      .catch((exc) => pushLog('error', `环境自检失败：${toError(exc).message}`));

    api.tingSections().then(setTingSections).catch(() => undefined);
    api.columnFilters().then(setColFilters).catch(() => undefined);
    api
      .libraryCategories()
      .then((list) => {
        if (list.length > 0) setLibCategoryOptions(list);
      })
      .catch(() => undefined);
  }, [pushLog]);

  // 事件订阅
  useEffect(() => {
    const unlisteners: Array<() => void> = [];
    void events
      .onDownloadProgress((payload) => {
        setStatus(`${payload.index}/${payload.total} · ${payload.name} · ${payload.message}`);
        if (payload.percent >= 0) setPercent(payload.percent);
        setBusy(true);
      })
      .then((un) => unlisteners.push(un));
    void events
      .onDownloadLog((payload) => pushLog(payload.level, payload.message))
      .then((un) => unlisteners.push(un));
    void events
      .onDownloadDone((payload) => {
        setBusy(false);
        setPercent(payload.total > 0 ? 100 : 0);
        const summary = payload.cancelled
          ? `已取消（成功 ${payload.success} / 失败 ${payload.failed}）`
          : `完成：成功 ${payload.success} 个，失败 ${payload.failed} 个`;
        setStatus(summary);
        pushLog(payload.failed > 0 ? 'error' : 'ok', summary);
      })
      .then((un) => unlisteners.push(un));
    void events
      .onColumnProgress((payload) => {
        setResourceHint(`${payload.text}（${payload.done}/${payload.total}）`);
        if (payload.total > 0) setPercent((payload.done / payload.total) * 100);
      })
      .then((un) => unlisteners.push(un));

    return () => unlisteners.forEach((un) => un());
  }, [pushLog]);

  // 日志自动滚到底
  useEffect(() => {
    if (logRef.current) logRef.current.scrollTop = logRef.current.scrollHeight;
  }, [logs]);

  const clearContent = useCallback(() => {
    setResources([]);
    setEpisodes([]);
    setSelectedEpisodes(new Set());
    setQualities([]);
    setQualityBr('auto');
    setActiveResourceKey(null);
    setResourceHint('');
    setEpisodeTitle('');
    setEpisodeHint('');
    setEpisodeLoading(false);
    setResourceLoading(false);
    setProbing(false);
    setPercent(0);
  }, []);

  const switchMode = useCallback(
    (next: Mode) => {
      if (next === mode) return;
      if (busy) {
        pushLog('error', '有任务正在执行，暂不能切换模式');
        return;
      }
      setMode(next);
      clearContent();
      setHover(null);
      setEpisodeKeyword('');
      setEpisodeYear('all');
      setStatus('');
    },
    [busy, clearContent, mode, pushLog],
  );

  // ------------------------------------------------------------ 探测清晰度 --
  const probeQualities = useCallback(
    async (guid: string) => {
      if (!guid) {
        setQualities([]);
        return;
      }
      const token = ++probeToken.current;
      setProbing(true);
      try {
        const list = await api.listQualities(guid);
        if (token !== probeToken.current) return;
        setQualities(list);
        const plain = list.filter((quality) => !quality.encrypted);
        setQualityBr(plain.length > 0 ? String(plain[0].br) : list.length > 0 ? String(list[0].br) : 'auto');
      } catch (exc) {
        if (token === probeToken.current) {
          setQualities([]);
          setEpisodeHint(`清晰度探测失败：${toError(exc).message}`);
        }
      } finally {
        if (token === probeToken.current) setProbing(false);
      }
    },
    [],
  );

  // ------------------------------------------------------------ 加载左列表 --
  const loadResources = useCallback(async () => {
    setResourceLoading(true);
    setResourceHint('');
    setEpisodes([]);
    setSelectedEpisodes(new Set());
    setQualities([]);
    setActiveResourceKey(null);
    try {
      if (mode === 'search') {
        if (!keyword.trim()) throw new Error('请输入要搜索的关键词');
        const result = await api.searchVideos(keyword.trim());
        const rows = result.list.map((item, index) => ({
          key: `search-${item.id || index}`,
          title: item.title,
          subtitle: [item.channel, item.uploadTime, item.duration].filter(Boolean).join(' · '),
          payload: item,
        }));
        setResources(rows);
        setResourceHint(`共 ${result.total} 条，显示 ${rows.length} 条`);
      } else if (mode === 'library') {
        const result = await api.listAlbums(libCategory, {
          year: libYear,
          sc: libSc,
          pageSize: 40,
        });
        setResources(result.list.map(albumRow));
        setResourceHint(`共 ${result.total} 部`);
      } else if (mode === 'fourk') {
        const result = await api.list4kAlbums();
        setResources(result.list.map(albumRow));
        setResourceHint(`共 ${result.total} 个 4K 专辑`);
      } else if (mode === 'ting') {
        const result = await api.listTingItems(tingSection);
        setResources(result.list.map(videoRow));
        setResourceHint(`${tingSection} · ${result.list.length} 条`);
      } else {
        // 栏目页
        const input = columnInput.trim();
        if (!input) {
          // 没填栏目名 → 按筛选条件浏览栏目大全
          const result = await api.browseColumns(colCategory, colChannel);
          setResources(result.list.map(columnRow));
          setResourceHint(`栏目大全 · 共 ${result.total} 个栏目（双击加载往期）`);
          return;
        }
        const resolved = await api.resolveColumn(input);
        if (resolved.candidates.length > 0) {
          setResources(resolved.candidates.map(columnRow));
          setResourceHint(`匹配到 ${resolved.candidates.length} 个栏目，请选择`);
          return;
        }
        setResources([
          {
            key: `column-${resolved.columnId}`,
            title: resolved.title || resolved.columnId,
            subtitle: resolved.site,
            payload: { topicId: resolved.columnId, name: resolved.title, site: resolved.site } as Column,
          },
        ]);
        setResourceHint('已定位栏目，正在加载往期…');
        await loadColumnEpisodes(resolved.columnId, resolved.title);
      }
    } catch (exc) {
      setResources([]);
      setResourceHint(toError(exc).message);
      pushLog('error', toError(exc).message);
    } finally {
      setResourceLoading(false);
    }
  }, [colCategory, colChannel, columnInput, keyword, libCategory, libSc, libYear, mode, pushLog, tingSection]);

  // ------------------------------------------------------------ 加载往期 --
  const loadColumnEpisodes = useCallback(
    async (topicId: string, title: string) => {
      setEpisodeLoading(true);
      setEpisodeTitle(`${title || topicId} · 往期`);
      setEpisodeHint('正在划分时间块并逐年拉取…');
      setSelectedEpisodes(new Set());
      setEpisodeKeyword('');
      setEpisodeYear('all');
      setPercent(0);
      try {
        const since =
          columnRange === 'all'
            ? ''
            : String(new Date().getFullYear() - Number(columnRange) + 1);
        const result = await api.listColumnAll(topicId, since, '', 8);
        setEpisodes(result.list);
        const [first, last] = result.years;
        setEpisodeHint(
          [
            `共 ${result.total} 段 · ${first ?? '?'}-${last ?? '?'} · ${result.blocks} 个时间块`,
            result.truncated > 0 ? `⚠ ${result.truncated} 个时间块仍超上限` : '',
          ]
            .filter(Boolean)
            .join(' · '),
        );
      } catch (exc) {
        setEpisodes([]);
        setEpisodeHint(toError(exc).message);
      } finally {
        setEpisodeLoading(false);
        setPercent(0);
      }
    },
    [columnRange],
  );

  // ------------------------------------------------------------ 选中资源 --
  const openResource = useCallback(
    async (row: ResourceRow) => {
      if (row.disabled) {
        pushLog('error', `「${row.title}」没有往期接口，无法加载`);
        return;
      }
      setActiveResourceKey(row.key);
      setSelectedEpisodes(new Set());
      setQualities([]);

      if (mode === 'search') {
        const item = row.payload as { id: string; title: string; url: string; image: string; duration: string };
        const episode: Episode = {
          title: item.title,
          videoId: item.id,
          guid: '',
          url: item.url,
          image: item.image,
          length: item.duration,
          part: 1,
          sc: '',
          brief: '',
          partFabricated: false,
          date: '',
        };
        setEpisodes([episode]);
        setEpisodeTitle(item.title);
        setEpisodeHint('搜索结果 · 单集视频');
        return;
      }

      if (mode === 'ting') {
        const item = row.payload as Episode;
        setEpisodeTitle(item.title);
        setEpisodeHint('听音节目 · 正在解析 guid…');
        try {
          const list = await api.resolveTingGuids([item]);
          setEpisodes(list);
          setEpisodeHint('听音节目 · 单条音频');
        } catch (exc) {
          setEpisodes([item]);
          setEpisodeHint(`guid 解析失败：${toError(exc).message}`);
        }
        return;
      }

      if (mode === 'column') {
        const column = row.payload as Column;
        await loadColumnEpisodes(column.topicId, column.name);
        return;
      }

      // 片库 / 4K：列剧集
      const album = row.payload as Album;
      setEpisodeLoading(true);
      setEpisodeTitle(album.title);
      setEpisodeHint('正在加载剧集…');
      try {
        const list = await api.listAlbumEpisodes(album.albumId);
        setEpisodes(list);
        setEpisodeHint(`共 ${list.length} 集`);
        if (list.length > 0) void probeQualities(list[0].guid);
      } catch (exc) {
        setEpisodes([]);
        setEpisodeHint(toError(exc).message);
      } finally {
        setEpisodeLoading(false);
      }
    },
    [loadColumnEpisodes, mode, probeQualities, pushLog],
  );

  // ---------------------------------------------------------- 直接链接 --
  /** 把粘贴的视频页链接解析成一条可下载/可预览的条目，直接落到右列表。 */
  const resolveDirectLink = useCallback(async () => {
    const url = directLink.trim();
    if (!url) {
      pushLog('error', '请先粘贴视频页链接');
      return;
    }
    setEpisodeLoading(true);
    setEpisodeTitle('直接链接');
    setEpisodeHint('正在解析链接…');
    try {
      const target = await api.previewResolve('', null, false, url);
      const item: Episode = {
        title: target.title || url,
        videoId: '',
        guid: target.guid,
        url,
        image: '',
        length: target.duration,
        part: 0,
        sc: target.channel,
        brief: '',
        partFabricated: false,
        date: '',
      };
      setEpisodes([item]);
      setSelectedEpisodes(new Set([0]));
      setEpisodeHint(target.summary || '已解析，勾选后可下载');
      pushLog('ok', `链接解析成功：${item.title}`);
      void probeQualities(target.guid);
    } catch (exc) {
      setEpisodes([]);
      setSelectedEpisodes(new Set());
      setEpisodeHint(toError(exc).message);
      pushLog('error', `链接解析失败：${toError(exc).message}`);
    } finally {
      setEpisodeLoading(false);
    }
  }, [directLink, probeQualities, pushLog]);

  const toggleEpisode = useCallback((index: number) => {
    setSelectedEpisodes((previous) => {
      const next = new Set(previous);
      if (next.has(index)) next.delete(index);
      else next.add(index);
      return next;
    });
  }, []);

  const toggleAll = useCallback(() => {
    setSelectedEpisodes((previous) => {
      if (previous.size === episodes.length) return new Set();
      return new Set(episodes.map((_, index) => index));
    });
  }, [episodes]);

  const invertSelection = useCallback(() => {
    setSelectedEpisodes((previous) => {
      const next = new Set<number>();
      episodes.forEach((_, index) => {
        if (!previous.has(index)) next.add(index);
      });
      return next;
    });
  }, [episodes]);

  // -------------------------------------------------------- 往期筛选排序 --
  /** 右列表的可见条目：保留原始下标，筛选不会打乱勾选状态。 */
  const visibleEpisodes = useMemo(() => {
    const indexed = episodes.map((episode, index) => ({ episode, index }));
    if (mode !== 'column') return indexed;
    const keyword = episodeKeyword.trim().toLowerCase();
    let list = indexed;
    if (keyword) {
      list = list.filter(({ episode }) =>
        [episode.title, episode.date, episode.brief, episode.sc]
          .join(' ')
          .toLowerCase()
          .includes(keyword),
      );
    }
    if (episodeYear !== 'all') {
      list = list.filter(({ episode }) => (episode.date || '').startsWith(episodeYear));
    }
    return episodeOrder === 'asc' ? [...list].reverse() : list;
  }, [episodeKeyword, episodeOrder, episodeYear, episodes, mode]);

  /** 可选的年份（由已有往期推出，倒序）。 */
  const episodeYears = useMemo(() => {
    const counts = new Map<string, number>();
    for (const episode of episodes) {
      const year = (episode.date || '').slice(0, 4);
      if (/^\d{4}$/.test(year)) counts.set(year, (counts.get(year) ?? 0) + 1);
    }
    return [...counts.entries()].sort((a, b) => b[0].localeCompare(a[0]));
  }, [episodes]);

  /** 悬浮详情：位置取鼠标进入时的坐标，避免跟随抖动。 */
  const showHover = useCallback((info: HoverInfo, event: ReactMouseEvent) => {
    setHover({ info, x: event.clientX, y: event.clientY });
  }, []);
  const hideHover = useCallback(() => setHover(null), []);

  // -------------------------------------------------------------- 预览 --
  const previewItem = useCallback((): { guid: string; pageUrl: string; audio: boolean } | null => {
    const selectedIndex = [...selectedEpisodes][0];
    const episode =
      selectedIndex !== undefined ? episodes[selectedIndex] : episodes.length === 1 ? episodes[0] : undefined;
    if (episode) {
      return { guid: episode.guid, pageUrl: episode.url, audio: mode === 'ting' };
    }
    const row = resources.find((item) => item.key === activeResourceKey);
    if (row) {
      const payload = row.payload as { guid?: string; url?: string; firstGuid?: string };
      return {
        guid: payload.guid || payload.firstGuid || '',
        pageUrl: payload.url || '',
        audio: mode === 'ting',
      };
    }
    return null;
  }, [activeResourceKey, episodes, mode, resources, selectedEpisodes]);

  const openPreview = useCallback(async () => {
    const item = previewItem();
    if (!item || (!item.guid && !item.pageUrl)) {
      pushLog('error', '请先选中一个资源或剧集');
      return;
    }
    setPreviewLoading(true);
    setPreviewAudio(item.audio);
    try {
      let guid = item.guid;
      if (!guid) {
        // 搜索结果只有视频页地址，交给后端的预览解析处理
        pushLog('info', '正在解析视频 guid…');
      }
      const target = await api.previewResolve(guid, null, item.audio, item.pageUrl);
      setPreview(target);
    } catch (exc) {
      pushLog('error', `预览失败：${toError(exc).message}`);
    } finally {
      setPreviewLoading(false);
    }
  }, [previewItem, pushLog]);

  const resolvePreviewQuality = useCallback(
    async (br: number | null) => {
      if (!preview) return;
      setPreviewLoading(true);
      try {
        const target = await api.previewResolve(preview.guid, br, previewAudio, preview.pageUrl);
        setPreview(target);
      } catch (exc) {
        pushLog('error', `切换档位失败：${toError(exc).message}`);
      } finally {
        setPreviewLoading(false);
      }
    },
    [preview, previewAudio, pushLog],
  );

  // -------------------------------------------------------------- 下载 --
  const plainQualities = useMemo(() => qualities.filter((quality) => !quality.encrypted), [qualities]);
  const encQualities = useMemo(() => qualities.filter((quality) => quality.encrypted), [qualities]);

  const currentChannelQualities = channel === 'hd' ? encQualities : plainQualities;

  const startDownload = useCallback(
    async (targets: Episode[]) => {
      if (busy) {
        pushLog('error', '已有任务在执行');
        return;
      }
      if (targets.length === 0) {
        pushLog('error', '没有要下载的条目');
        return;
      }
      if (!outputDir.trim()) {
        pushLog('error', '请先选择输出目录');
        return;
      }
      const albumTitle = mode === 'library' || mode === 'fourk' ? episodeTitle : '';
      const items = targets.map((episode) => ({
        guid: episode.guid,
        title: episode.title,
        albumTitle: targets.length > 1 ? albumTitle : '',
        pageUrl: episode.url,
      }));
      const br = channel === 'hd' || qualityBr === 'auto' ? null : Number(qualityBr);

      setBusy(true);
      setPercent(0);
      setStatus('正在启动…');
      setLogs([]);
      try {
        await api.startDownload({
          items,
          outputDir: outputDir.trim(),
          br,
          channel,
          workers,
          makeMp4: true,
          keepTs: false,
          hdSeconds,
        });
      } catch (exc) {
        setBusy(false);
        pushLog('error', toError(exc).message);
      }
    },
    [busy, channel, episodeTitle, hdSeconds, mode, outputDir, pushLog, qualityBr, workers],
  );

  const downloadSelected = useCallback(() => {
    const list = episodes.filter((_, index) => selectedEpisodes.has(index));
    void startDownload(list);
  }, [episodes, selectedEpisodes, startDownload]);

  const downloadAll = useCallback(() => {
    void startDownload(episodes);
  }, [episodes, startDownload]);

  const cancel = useCallback(async () => {
    await api.cancelTask().catch(() => undefined);
    pushLog('info', '已请求取消任务');
  }, [pushLog]);

  const pickOutputDir = useCallback(async () => {
    try {
      const picked = await openDialog({ directory: true, defaultPath: outputDir });
      if (typeof picked === 'string') setOutputDir(picked);
    } catch (exc) {
      pushLog('error', toError(exc).message);
    }
  }, [outputDir, pushLog]);

  // -------------------------------------------------------------- 渲染 --
  const resourcePanelTitle = useMemo(() => {
    switch (mode) {
      case 'search':
        return '搜索结果';
      case 'library':
        return '片库专辑';
      case 'column':
        return '栏目 / 栏目大全';
      case 'ting':
        return '听音节目';
      default:
        return '4K 专辑';
    }
  }, [mode]);

  const episodePanelTitle = useMemo(() => {
    if (episodeTitle) return episodeTitle;
    switch (mode) {
      case 'search':
        return '视频';
      case 'ting':
        return '音频';
      case 'column':
        return '往期';
      default:
        return '剧集';
    }
  }, [episodeTitle, mode]);

  return (
    <div className="app">
      <div className="topbar">
        <div className="brand">
          <h1>央视工具箱</h1>
          <span className="ver">
            v{env?.version ?? '2.0.0'} · {env?.runtime ?? 'Tauri + React + Rust'}
          </span>
        </div>
        <div className="spacer" />
        <div className="segments">
          {MODES.map((item) => (
            <button
              key={item.key}
              className={item.key === mode ? 'active' : ''}
              disabled={busy}
              onClick={() => switchMode(item.key)}
            >
              {item.label}
            </button>
          ))}
        </div>
        <button className="btn sm" onClick={() => setTheme(theme === 'dark' ? 'light' : 'dark')}>
          {theme === 'dark' ? '☀ 浅色' : '☾ 深色'}
        </button>
      </div>

      {/* 参数区 */}
      <div className="card">
        {mode === 'search' && (
          <div className="param-row">
            <label className="field">关键词</label>
            <input
              type="text"
              style={{ flex: 1, minWidth: 240 }}
              placeholder="剧名 / 节目名，例如：今日说法"
              value={keyword}
              onChange={(event) => setKeyword(event.target.value)}
              onKeyDown={(event) => event.key === 'Enter' && void loadResources()}
            />
            <button className="btn primary" onClick={() => void loadResources()} disabled={resourceLoading || busy}>
              {resourceLoading ? '搜索中…' : '搜索'}
            </button>
          </div>
        )}

        {mode === 'library' && (
          <div className="param-row">
            <label className="field">分类</label>
            <select value={libCategory} onChange={(event) => setLibCategory(event.target.value)}>
              {libCategoryOptions.map((item) => (
                <option key={item}>{item}</option>
              ))}
            </select>
            <label className="field">题材</label>
            <input
              type="text"
              style={{ width: 120 }}
              placeholder="如 都市 / 谍战"
              value={libSc}
              onChange={(event) => setLibSc(event.target.value)}
            />
            <label className="field">年份</label>
            <select value={libYear} onChange={(event) => setLibYear(event.target.value)}>
              {YEARS.map((item) => (
                <option key={item || 'all'} value={item}>
                  {item || '全部'}
                </option>
              ))}
            </select>
            <button className="btn primary" onClick={() => void loadResources()} disabled={resourceLoading || busy}>
              {resourceLoading ? '加载中…' : '浏览片库'}
            </button>
          </div>
        )}

        {mode === 'column' && (
          <>
            <div className="param-row">
              <label className="field">栏目</label>
              <input
                type="text"
                style={{ flex: 1, minWidth: 240 }}
                placeholder="栏目名（如 今日说法）或栏目页地址，留空则浏览栏目大全"
                value={columnInput}
                onChange={(event) => setColumnInput(event.target.value)}
                onKeyDown={(event) => event.key === 'Enter' && void loadResources()}
              />
              <label className="field">时间范围</label>
              <select value={columnRange} onChange={(event) => setColumnRange(event.target.value)}>
                {COLUMN_RANGES.map((item) => (
                  <option key={item.value} value={item.value}>
                    {item.label}
                  </option>
                ))}
              </select>
              <button className="btn primary" onClick={() => void loadResources()} disabled={resourceLoading || busy}>
                {resourceLoading ? '加载中…' : '加载'}
              </button>
            </div>
            <div className="param-row">
              <label className="field">栏目大全</label>
              <select value={colCategory} onChange={(event) => setColCategory(event.target.value)}>
                {(colFilters.categories.length > 0 ? colFilters.categories : ['全部']).map((item) => (
                  <option key={item}>{item}</option>
                ))}
              </select>
              <select value={colChannel} onChange={(event) => setColChannel(event.target.value)}>
                {(colFilters.channels.length > 0 ? colFilters.channels : ['全部']).map((item) => (
                  <option key={item}>{item}</option>
                ))}
              </select>
              <button className="btn" onClick={() => void loadResources()} disabled={resourceLoading || busy}>
                浏览栏目大全
              </button>
            </div>
          </>
        )}

        {mode === 'ting' && (
          <div className="param-row">
            <label className="field">分区</label>
            <select value={tingSection} onChange={(event) => setTingSection(event.target.value)}>
              {tingSections.map((item) => (
                <option key={item}>{item}</option>
              ))}
            </select>
            <button className="btn primary" onClick={() => void loadResources()} disabled={resourceLoading || busy}>
              {resourceLoading ? '加载中…' : '加载听音'}
            </button>
            <span style={{ color: 'var(--text-mute)' }}>音频明文流，可直接下载为 m4a / mp3</span>
          </div>
        )}

        {mode === 'fourk' && (
          <div className="param-row">
            <button className="btn primary" onClick={() => void loadResources()} disabled={resourceLoading || busy}>
              {resourceLoading ? '加载中…' : '浏览 4K 专辑'}
            </button>
            <span style={{ color: 'var(--text-mute)' }}>
              4K 专区最高 1080P：1080P（br=4000）有明文档位可直接下载/预览，720P 及以上另有加密档
            </span>
          </div>
        )}

        {(mode === 'library' || mode === 'column' || mode === 'fourk') && (
          <div className="param-row">
            <label className="field">直接链接</label>
            <input
              type="text"
              style={{ flex: 1, minWidth: 260 }}
              placeholder="粘贴视频页地址（https://tv.cctv.com/…/VIDE….shtml），解析后落右侧列表"
              value={directLink}
              onChange={(event) => setDirectLink(event.target.value)}
              onKeyDown={(event) => event.key === 'Enter' && void resolveDirectLink()}
            />
            <button className="btn" onClick={() => void resolveDirectLink()} disabled={episodeLoading}>
              {episodeLoading ? '解析中…' : '解析链接'}
            </button>
          </div>
        )}

        {mode === 'column' && episodes.length > 0 && (
          <div className="param-row">
            <label className="field">筛选往期</label>
            <input
              type="text"
              style={{ flex: 1, minWidth: 200 }}
              placeholder="按标题 / 日期 / 简介 过滤…"
              value={episodeKeyword}
              onChange={(event) => setEpisodeKeyword(event.target.value)}
            />
            <select value={episodeYear} onChange={(event) => setEpisodeYear(event.target.value)}>
              <option value="all">全部年份</option>
              {episodeYears.map(([year, count]) => (
                <option key={year} value={year}>
                  {year}（{count}）
                </option>
              ))}
            </select>
            <label className="field">排序</label>
            <select
              value={episodeOrder}
              onChange={(event) => setEpisodeOrder(event.target.value as 'desc' | 'asc')}
            >
              <option value="desc">最新在上</option>
              <option value="asc">最早在上</option>
            </select>
            <span style={{ color: 'var(--text-mute)' }}>
              命中 {visibleEpisodes.length} / {episodes.length}
            </span>
            <button
              className="btn sm"
              disabled={visibleEpisodes.length === 0}
              onClick={() => setSelectedEpisodes(new Set(visibleEpisodes.map((item) => item.index)))}
            >
              选中筛选结果
            </button>
            {(episodeKeyword || episodeYear !== 'all') && (
              <button
                className="btn sm ghost"
                onClick={() => {
                  setEpisodeKeyword('');
                  setEpisodeYear('all');
                }}
              >
                清空筛选
              </button>
            )}
          </div>
        )}
      </div>

      {/* 双列表 */}
      <div className="lists">
        <div className="list-panel">
          <div className="list-head">
            <span className="title">{resourcePanelTitle}</span>
            {resourceLoading && <span className="spin" />}
            <span className="hint">{resourceHint}</span>
          </div>
          <div className="list-body">
            {resources.length === 0 && !resourceLoading && (
              <div className="empty">
                暂无内容
                <br />
                先在上方输入条件并点击加载
              </div>
            )}
            {resources.map((row, index) => (
              <div
                key={row.key}
                className={`row ${row.key === activeResourceKey ? 'active' : ''} ${row.disabled ? 'disabled' : ''}`}
                onClick={() => {
                  hideHover();
                  void openResource(row);
                }}
                onMouseEnter={(event) => showHover(resourceHover(row, mode), event)}
                onMouseLeave={hideHover}
              >
                <span className="idx">{index + 1}</span>
                <span className="info">
                  <div className="t1" title={row.title}>
                    {row.title}
                  </div>
                  {row.subtitle && <div className="t2">{row.subtitle}</div>}
                </span>
                {row.badge && <span className="badge">{row.badge}</span>}
              </div>
            ))}
          </div>
        </div>

        <div className="list-panel">
          <div className="list-head">
            <span className="title">{episodePanelTitle}</span>
            {episodeLoading && <span className="spin" />}
            {episodes.length > 0 && (
              <>
                <button className="btn sm ghost" onClick={toggleAll}>
                  {selectedEpisodes.size === episodes.length ? '取消全选' : '全选'}
                </button>
                <button className="btn sm ghost" onClick={invertSelection}>
                  反选
                </button>
              </>
            )}
            <span className="hint">{episodeHint}</span>
          </div>
          <div className="list-body">
            {episodes.length === 0 && !episodeLoading && (
              <div className="empty">
                点击左侧条目加载内容
                <br />
                勾选后可下载或预览
              </div>
            )}
            {visibleEpisodes.length === 0 && episodes.length > 0 && (
              <div className="empty">没有符合筛选条件的往期，试试放宽关键词或年份</div>
            )}
            {visibleEpisodes.map(({ episode, index }) => (
              <div
                key={`${episode.guid || episode.videoId || index}-${index}`}
                className={`row ${selectedEpisodes.has(index) ? 'active' : ''}`}
                onClick={() => {
                  hideHover();
                  toggleEpisode(index);
                  if (episode.guid) void probeQualities(episode.guid);
                }}
                onMouseEnter={(event) => showHover(episodeHover(episode, index), event)}
                onMouseLeave={hideHover}
              >
                <span className={`checkbox ${selectedEpisodes.has(index) ? 'on' : ''}`}>✓</span>
                <span className="idx">{episodeNumber(episode, index)}</span>
                <span className="info">
                  <div className="t1" title={episode.title}>
                    {episode.title}
                  </div>
                  {(episode.date || episode.length || episode.brief) && (
                    <div className="t2">{[episode.date, episode.length, episode.brief].filter(Boolean).join(' · ')}</div>
                  )}
                </span>
              </div>
            ))}
          </div>
        </div>
      </div>

      {/* 底部操作 */}
      <div className="footer">
        <div className="action-bar">
          <label className="field">通道</label>
          <select
            value={channel}
            onChange={(event) => setChannel(event.target.value as DownloadChannel)}
            disabled={busy}
          >
            <option value="standard">明文档位（快）</option>
            <option value="hd">高清 720P（浏览器解密）</option>
            {mode === 'ting' && <option value="audio">音频（听音）</option>}
          </select>

          <label className="field">清晰度</label>
          <select value={qualityBr} onChange={(event) => setQualityBr(event.target.value)} disabled={busy}>
            <option value="auto">自动（最高）</option>
            {currentChannelQualities.map((quality) => (
              <option key={`${quality.channel}-${quality.br}`} value={quality.br}>
                {quality.label}
                {resolutionOf(quality) ? ` · ${resolutionOf(quality)}` : ''}
              </option>
            ))}
          </select>
          {probing && <span className="spin" />}

          {channel === 'hd' && (
            <>
              <label className="field">只取前</label>
              <input
                type="number"
                style={{ width: 70 }}
                value={hdSeconds}
                onChange={(event) => setHdSeconds(Number(event.target.value) || 0)}
                disabled={busy}
              />
              <span style={{ color: 'var(--text-mute)' }}>秒（0=全部）</span>
            </>
          )}

          <button className="btn primary" onClick={downloadSelected} disabled={busy || selectedEpisodes.size === 0}>
            下载选中（{selectedEpisodes.size}）
          </button>
          <button className="btn" onClick={downloadAll} disabled={busy || episodes.length === 0}>
            下载全部（{episodes.length}）
          </button>
          <button className="btn" onClick={() => void openPreview()} disabled={previewLoading}>
            {previewLoading ? '解析中…' : mode === 'ting' ? '试听选中' : '预览选中'}
          </button>
          {(busy || (episodeLoading && mode === 'column')) && (
            <button className="btn danger" onClick={() => void cancel()}>
              取消任务
            </button>
          )}
        </div>

        <div className="action-bar">
          <label className="field">输出目录</label>
          <input
            type="text"
            style={{ flex: 1, minWidth: 220 }}
            value={outputDir}
            onChange={(event) => setOutputDir(event.target.value)}
            disabled={busy}
          />
          <button className="btn" onClick={() => void pickOutputDir()} disabled={busy}>
            选择
          </button>
          <label className="field">并发</label>
          <input
            type="number"
            style={{ width: 62 }}
            value={workers}
            onChange={(event) => setWorkers(Math.max(1, Number(event.target.value) || 8))}
            disabled={busy}
          />
        </div>

        <div className="progress-track">
          <div className="progress-fill" style={{ width: `${Math.min(100, Math.max(0, percent))}%` }} />
        </div>
        <div className="status-line">
          {busy && <span className="spin" />}
          <span>{status || '就绪'}</span>
        </div>
      </div>

      {/* 日志 */}
      <div className="logs" ref={logRef}>
        {logs.length === 0 && <div className="info">日志会显示在这里</div>}
        {logs.map((log, index) => (
          <div key={index} className={log.level}>
            {log.message}
          </div>
        ))}
      </div>

      {preview && (
        <PreviewDialog
          target={preview}
          loading={previewLoading}
          onResolveQuality={resolvePreviewQuality}
          onClose={() => {
            void api.previewStop().catch(() => undefined);
            setPreview(null);
          }}
        />
      )}

      {hover && <HoverCard info={hover.info} x={hover.x} y={hover.y} />}
    </div>
  );
}
