/** 预览弹窗：窗口内取帧 / 内嵌播放 + 外部播放器。 */

import { useCallback, useEffect, useRef, useState } from 'react';
import Hls from 'hls.js';
import { save } from '@tauri-apps/plugin-dialog';

import { api, toError } from '../lib/api';
import { resolutionOf } from '../lib/types';
import type { PlayerInfo, PreviewTarget } from '../lib/types';

/** 取帧时间点（秒）：1s 起跳过片头黑场，依次往后找有画面的位置。 */
const FRAME_POINTS = [1, 15, 45, 90, 180, 300];

interface Props {
  target: PreviewTarget;
  /** 切换档位时由父组件重新解析 */
  onResolveQuality: (br: number | null) => Promise<void>;
  loading: boolean;
  onClose: () => void;
}

export default function PreviewDialog({ target, onResolveQuality, loading, onClose }: Props) {
  const [frame, setFrame] = useState<string | null>(null);
  const [frameAt, setFrameAt] = useState(FRAME_POINTS[0]);
  const [grabbing, setGrabbing] = useState(false);
  const [note, setNote] = useState('');
  const [error, setError] = useState('');
  const [players, setPlayers] = useState<PlayerInfo[]>([]);
  const [player, setPlayer] = useState('');
  const [playingInline, setPlayingInline] = useState(false);
  const [inlineError, setInlineError] = useState('');

  const videoRef = useRef<HTMLVideoElement | null>(null);
  const hlsRef = useRef<Hls | null>(null);

  const mediaUrl = target.mediaUrl;
  const canPlay = !target.encryptedOnly && mediaUrl !== '';
  // 档位分两组：明文档位可直接预览；加密档（720P+）是央视私有 CDRM 流，
  // hls.js / ffmpeg 都无法解码，只能走「高清通道」下载或官方页面观看
  const plainQualities = target.qualities.filter((quality) => !quality.encrypted);
  const encryptedQualities = target.qualities.filter((quality) => quality.encrypted);

  // 可用播放器
  useEffect(() => {
    api
      .previewPlayers()
      .then((list) => {
        setPlayers(list);
        if (list.length > 0) setPlayer(list[0].path);
      })
      .catch(() => undefined);
  }, []);

  // 取帧
  const grab = useCallback(
    async (at: number) => {
      if (!canPlay) return;
      setGrabbing(true);
      setError('');
      setNote('');
      try {
        const dataUrl = await api.previewFrame(mediaUrl, target.guid, at, 640);
        setFrame(dataUrl);
        setFrameAt(at);
      } catch (exc) {
        setError(toError(exc).message);
      } finally {
        setGrabbing(false);
      }
    },
    [canPlay, mediaUrl, target.guid],
  );

  useEffect(() => {
    if (canPlay) {
      void grab(FRAME_POINTS[0]);
    }
  }, [canPlay, grab]);

  // 内嵌播放
  useEffect(() => {
    if (!playingInline || !canPlay) return;
    const video = videoRef.current;
    if (!video) return;
    setInlineError('');

    if (Hls.isSupported()) {
      const hls = new Hls({ enableWorker: true, lowLatencyMode: false });
      hlsRef.current = hls;
      hls.loadSource(mediaUrl);
      hls.attachMedia(video);
      hls.on(Hls.Events.ERROR, (_event, data) => {
        if (data.fatal) {
          setInlineError(`内嵌播放失败：${data.details}（可改用外部播放器）`);
        }
      });
      void video.play().catch(() => undefined);
      return () => {
        hls.destroy();
        hlsRef.current = null;
      };
    }

    if (video.canPlayType('application/vnd.apple.mpegurl')) {
      video.src = mediaUrl;
      void video.play().catch(() => undefined);
      return () => {
        video.removeAttribute('src');
      };
    }
    setInlineError('当前环境不支持内嵌播放，请使用外部播放器');
    return undefined;
  }, [playingInline, canPlay, mediaUrl]);

  const stopInline = () => {
    hlsRef.current?.destroy();
    hlsRef.current = null;
    const video = videoRef.current;
    if (video) {
      video.pause();
      video.removeAttribute('src');
    }
    setPlayingInline(false);
  };

  useEffect(() => {
    return () => {
      hlsRef.current?.destroy();
      hlsRef.current = null;
    };
  }, []);

  const cycleFrame = () => {
    const current = FRAME_POINTS.indexOf(frameAt);
    const next = FRAME_POINTS[(current + 1) % FRAME_POINTS.length];
    void grab(next);
  };

  const playExternal = async () => {
    try {
      await api.previewPlay(mediaUrl, target.title, player);
      setNote('已交给外部播放器打开');
    } catch (exc) {
      setError(toError(exc).message);
    }
  };

  const copyUrl = async () => {
    try {
      await navigator.clipboard.writeText(mediaUrl);
      setNote('流地址已复制到剪贴板');
    } catch {
      setNote(mediaUrl);
    }
  };

  const saveFrame = async () => {
    try {
      const path = await save({
        defaultPath: `${target.title || target.guid}.jpg`,
        filters: [{ name: 'JPEG 图片', extensions: ['jpg'] }],
      });
      if (!path) return;
      await api.previewSaveFrame(mediaUrl, frameAt, 1280, path);
      setNote(`已保存 1280 宽截图：${path}`);
    } catch (exc) {
      setError(toError(exc).message);
    }
  };

  return (
    <div className="modal-mask" onMouseDown={(event) => event.target === event.currentTarget && onClose()}>
      <div className="modal">
        <div className="modal-head">
          <span className="name" title={target.title}>
            {target.title || target.guid}
          </span>
          {target.duration && <span className="badge">{target.duration}</span>}
          <div className="spacer" />
          {loading && <span className="spin" />}
          <button className="btn sm ghost" onClick={onClose}>
            关闭
          </button>
        </div>

        <div className="modal-body">
          <div>
            <div className="stage">
              {playingInline && canPlay ? (
                <video ref={videoRef} controls playsInline />
              ) : frame ? (
                <img src={frame} alt="预览帧" />
              ) : target.encryptedOnly ? (
                <div className="placeholder">
                  该视频只有加密档（720P 及以上）
                  <br />
                  软件内无法解码预览
                  <br />
                  <span style={{ color: 'var(--text-mute)' }}>请用浏览器打开视频页，或走「高清通道」下载</span>
                </div>
              ) : grabbing ? (
                <div className="placeholder">
                  <span className="spin" /> 正在取帧…
                </div>
              ) : (
                <div className="placeholder">暂无预览图</div>
              )}
            </div>

            {(error || inlineError) && (
              <div className="hint-box warn" style={{ marginTop: 10 }}>
                {error || inlineError}
              </div>
            )}
            {note && (
              <div className="hint-box" style={{ marginTop: 10, wordBreak: 'break-all' }}>
                {note}
              </div>
            )}
          </div>

          <div className="side">
            <div className="block">
              <span className="label">档位</span>
              <select
                value={target.quality?.br ?? ''}
                onChange={(event) => {
                  const value = event.target.value;
                  stopInline();
                  void onResolveQuality(value === '' ? null : Number(value));
                }}
                disabled={loading}
              >
                {plainQualities.length === 0 && <option value="">无明文档位</option>}
                {plainQualities.length > 0 && (
                  <optgroup label={target.audio ? '可预览' : '可预览（明文）'}>
                    {plainQualities.map((quality) => (
                      <option key={`${quality.channel}-${quality.br}-${quality.url}`} value={quality.br}>
                        {quality.label}
                        {resolutionOf(quality) ? ` · ${resolutionOf(quality)}` : ''}
                      </option>
                    ))}
                  </optgroup>
                )}
                {encryptedQualities.length > 0 && (
                  <optgroup label="加密档（仅下载，无法预览）">
                    {encryptedQualities.map((quality) => (
                      <option
                        key={`${quality.channel}-${quality.br}-${quality.url}`}
                        value={quality.br}
                        disabled
                      >
                        {quality.label}
                        {resolutionOf(quality) ? ` · ${resolutionOf(quality)}` : ''}
                      </option>
                    ))}
                  </optgroup>
                )}
              </select>
              {encryptedQualities.length > 0 && (
                <div className="hint-box" style={{ marginTop: 8, fontSize: 12 }}>
                  720P 及以上是央视私有加密流（CDRM），软件内无法解码播放。
                  需要高清请走主界面「高清通道」下载解密，或打开官方视频页在线观看。
                </div>
              )}
            </div>

            <div className="block">
              <span className="label">当前流</span>
              <div className="summary">{target.summary || '—'}</div>
            </div>

            {canPlay ? (
              <>
                <div className="block">
                  <span className="label">画面</span>
                  <div style={{ display: 'flex', gap: 6, flexWrap: 'wrap' }}>
                    {playingInline ? (
                      <button className="btn sm" onClick={stopInline}>
                        ⏹ 停止
                      </button>
                    ) : (
                      <button className="btn sm primary" onClick={() => setPlayingInline(true)}>
                        ▶ 窗口内播放
                      </button>
                    )}
                    <button className="btn sm" onClick={cycleFrame} disabled={grabbing || playingInline}>
                      {grabbing ? '取帧中…' : '换一帧'}
                    </button>
                  </div>
                </div>

                <div className="block">
                  <span className="label">外部播放器</span>
                  <select value={player} onChange={(event) => setPlayer(event.target.value)}>
                    {players.length === 0 && <option value="">未检测到播放器</option>}
                    {players.map((item) => (
                      <option key={item.path} value={item.path}>
                        {item.name}
                      </option>
                    ))}
                  </select>
                  <div style={{ display: 'flex', gap: 6, flexWrap: 'wrap' }}>
                    <button className="btn sm" onClick={playExternal} disabled={players.length === 0}>
                      ▶ 用它播放
                    </button>
                    <button className="btn sm" onClick={() => api.previewStop().catch(() => undefined)}>
                      ⏹ 结束
                    </button>
                  </div>
                </div>

                <div className="block">
                  <span className="label">保存 / 复制</span>
                  <div style={{ display: 'flex', gap: 6, flexWrap: 'wrap' }}>
                    <button className="btn sm" onClick={saveFrame} disabled={!frame}>
                      另存图片
                    </button>
                    <button className="btn sm" onClick={copyUrl}>
                      复制流地址
                    </button>
                  </div>
                </div>
              </>
            ) : (
              <div className="hint-box warn">
                预览只覆盖明文档位。720P 及以上是央视私有加密流（CDRM），
                需要走「高清通道」用浏览器解密下载。
              </div>
            )}

            <div className="block">
              <button
                className="btn sm"
                onClick={() => target.pageUrl && api.openUrl(target.pageUrl)}
                disabled={!target.pageUrl}
              >
                打开官方视频页
              </button>
            </div>
          </div>
        </div>

        <div className="modal-foot">
          <span style={{ color: 'var(--text-mute)', fontSize: 12 }}>
            截图时间点：{frameAt}s · 明文档位由 CDN 直连，窗口内播放无需解密
          </span>
        </div>
      </div>
    </div>
  );
}
