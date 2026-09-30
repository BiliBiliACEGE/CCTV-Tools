/** 悬浮详情卡片：鼠标停在列表行上时跟随光标显示完整信息。 */

import type { CSSProperties } from 'react';

export interface HoverField {
  label: string;
  value: string;
}

export interface HoverInfo {
  title: string;
  /** 缩略图地址（可选） */
  image?: string;
  fields: HoverField[];
  /** 底部补充说明（可选） */
  note?: string;
}

interface Props {
  info: HoverInfo;
  x: number;
  y: number;
}

const CARD_WIDTH = 340;
const CARD_MAX_HEIGHT = 360;
const GAP = 14;

export default function HoverCard({ info, x, y }: Props) {
  const viewportWidth = typeof window === 'undefined' ? 1280 : window.innerWidth;
  const viewportHeight = typeof window === 'undefined' ? 800 : window.innerHeight;

  // 贴近右边缘时翻到光标左侧；贴近下边缘时整体上移，避免被裁掉
  const flipX = x + GAP + CARD_WIDTH > viewportWidth;
  const flipY = y + GAP + CARD_MAX_HEIGHT > viewportHeight && y > CARD_MAX_HEIGHT;

  const style: CSSProperties = {
    left: flipX ? Math.max(8, x - GAP - CARD_WIDTH) : x + GAP,
    top: flipY ? y - GAP : y + GAP,
    width: CARD_WIDTH,
    maxHeight: CARD_MAX_HEIGHT,
    transform: flipY ? 'translateY(-100%)' : undefined,
  };

  return (
    <div className="hover-card" style={style}>
      <div className="hover-card-title">{info.title || '（无标题）'}</div>
      {info.image && (
        <img
          className="hover-card-image"
          src={info.image}
          alt=""
          referrerPolicy="no-referrer"
          onError={(event) => {
            event.currentTarget.style.display = 'none';
          }}
        />
      )}
      <div className="hover-card-fields">
        {info.fields
          .filter((field) => field.value !== '')
          .map((field, index) => (
            <div className="hover-card-field" key={`${field.label}-${index}`}>
              <span className="k">{field.label}</span>
              <span className="v" title={field.value}>
                {field.value}
              </span>
            </div>
          ))}
      </div>
      {info.note && <div className="hover-card-note">{info.note}</div>}
    </div>
  );
}
