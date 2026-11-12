/** 灵动岛共享 UI：图标、天气图形、品牌标记、额度条、开关、滑杆等 */

import {
  createContext,
  useContext,
  useEffect,
  useMemo,
  useState,
  type CSSProperties,
  type ReactNode,
} from "react";
import { BrandIcon, type BrandIconName } from "../BrandIcon";
import type { HudBarQuota } from "./types";

// ============================================================
// 时间上下文：整岛共用一个 1s 时钟，倒计时 / 相对时间不再各起定时器
// ============================================================

const NowContext = createContext(Date.now());

export function NowProvider({ children }: { children: ReactNode }) {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    const timer = window.setInterval(() => setNow(Date.now()), 1000);
    return () => window.clearInterval(timer);
  }, []);
  return <NowContext.Provider value={now}>{children}</NowContext.Provider>;
}

export function useNow(): number {
  return useContext(NowContext);
}

// ============================================================
// 图标
// ============================================================

export type IconName =
  | "home"
  | "message"
  | "chart"
  | "weather"
  | "sliders"
  | "clipboard"
  | "settings"
  | "refresh"
  | "chevron"
  | "close"
  | "play"
  | "pause"
  | "prev"
  | "next"
  | "copy"
  | "check"
  | "plus"
  | "trash"
  | "wifi"
  | "bluetooth"
  | "volume"
  | "volumeOff"
  | "brightness"
  | "folder"
  | "browser"
  | "task"
  | "note"
  | "terminal"
  | "external"
  | "pin"
  | "eye"
  | "eyeOff"
  | "power"
  | "search"
  | "clock"
  | "music"
  | "quota"
  | "spark"
  | "arrowUp"
  | "arrowDown"
  | "mapPin"
  | "wind"
  | "drop"
  | "sunrise"
  | "sunset"
  | "cpu"
  | "globe"
  | "camera"
  | "window"
  | "monitor"
  | "scroll"
  | "timer"
  | "image"
  | "edit"
  | "mic"
  | "toolbox";

const PATHS: Record<IconName, ReactNode> = {
  home: <path d="M3.5 10.6 12 3.5l8.5 7.1V20a1 1 0 0 1-1 1h-5v-6h-5v6h-5a1 1 0 0 1-1-1z" />,
  message: (
    <>
      <path d="M21 11.5a8.4 8.4 0 0 1-8.5 8.3 9 9 0 0 1-3.2-.6L4 21l1.2-4.1A8 8 0 0 1 3.5 12 8.4 8.4 0 0 1 12 3.7a8.4 8.4 0 0 1 9 7.8Z" />
      <path d="M8.5 12h.01M12 12h.01M15.5 12h.01" />
    </>
  ),
  chart: (
    <>
      <path d="M4 20V4M4 20h16" />
      <path d="m7.5 15 3.6-4 3 2.4L20 7" />
    </>
  ),
  weather: (
    <>
      <circle cx="9" cy="9" r="3.2" />
      <path d="M9 2.8v1.4M9 13.8v1.4M2.8 9h1.4M13.8 9h1.4M4.6 4.6l1 1M12.4 12.4l1 1M4.6 13.4l1-1M12.4 5.6l1-1" />
      <path d="M9 20h9.2a3.3 3.3 0 0 0 .2-6.6A4.6 4.6 0 0 0 9.6 13 3.6 3.6 0 0 0 9 20Z" />
    </>
  ),
  sliders: (
    <>
      <path d="M4 7h9M17 7h3M4 17h3M11 17h9" />
      <circle cx="15" cy="7" r="2" />
      <circle cx="9" cy="17" r="2" />
    </>
  ),
  clipboard: (
    <>
      <rect x="5" y="4.5" width="14" height="17" rx="2.2" />
      <path d="M9 4.5V3.6A1.6 1.6 0 0 1 10.6 2h2.8A1.6 1.6 0 0 1 15 3.6v.9" />
      <path d="M8.5 9.5h7M8.5 13h7M8.5 16.5h4.4" />
    </>
  ),
  settings: (
    <>
      <circle cx="12" cy="12" r="3.1" />
      <path d="M19.4 15a1.7 1.7 0 0 0 .34 1.87l.06.06a2 2 0 1 1-2.83 2.83l-.06-.06a1.7 1.7 0 0 0-1.87-.34 1.7 1.7 0 0 0-1 1.55V21a2 2 0 1 1-4 0v-.09a1.7 1.7 0 0 0-1-1.55 1.7 1.7 0 0 0-1.87.34l-.06.06a2 2 0 1 1-2.83-2.83l.06-.06a1.7 1.7 0 0 0 .34-1.87 1.7 1.7 0 0 0-1.55-1H3a2 2 0 1 1 0-4h.09a1.7 1.7 0 0 0 1.55-1 1.7 1.7 0 0 0-.34-1.87l-.06-.06a2 2 0 1 1 2.83-2.83l.06.06a1.7 1.7 0 0 0 1.87.34h.08a1.7 1.7 0 0 0 1-1.55V3a2 2 0 1 1 4 0v.09a1.7 1.7 0 0 0 1 1.55 1.7 1.7 0 0 0 1.87-.34l.06-.06a2 2 0 1 1 2.83 2.83l-.06.06a1.7 1.7 0 0 0-.34 1.87v.08a1.7 1.7 0 0 0 1.55 1H21a2 2 0 1 1 0 4h-.09a1.7 1.7 0 0 0-1.55 1Z" />
    </>
  ),
  refresh: (
    <>
      <path d="M20 11a8 8 0 1 0-2.3 5.7" />
      <path d="M20 4.5V11h-6.5" />
    </>
  ),
  chevron: <path d="m7 9.5 5 5 5-5" />,
  close: <path d="m6 6 12 12M18 6 6 18" />,
  play: <path d="M8.5 5.6v12.8a.8.8 0 0 0 1.22.68l10-6.4a.8.8 0 0 0 0-1.36l-10-6.4A.8.8 0 0 0 8.5 5.6Z" />,
  pause: <path d="M8 5.5h2.6v13H8zM13.4 5.5H16v13h-2.6z" />,
  prev: <path d="M18.5 5.6v12.8a.8.8 0 0 1-1.22.68l-9-5.72V18a1 1 0 0 1-2 0V6a1 1 0 0 1 2 0v4.64l9-5.72a.8.8 0 0 1 1.22.68Z" />,
  next: <path d="M5.5 5.6v12.8a.8.8 0 0 0 1.22.68l9-5.72V18a1 1 0 0 0 2 0V6a1 1 0 0 0-2 0v4.64l-9-5.72a.8.8 0 0 0-1.22.68Z" />,
  copy: (
    <>
      <rect x="8.5" y="8.5" width="11.5" height="11.5" rx="2" />
      <path d="M15.5 5.5A2 2 0 0 0 13.5 3.5H5.8a2 2 0 0 0-2 2v8.1a2 2 0 0 0 2 2" />
    </>
  ),
  check: <path d="m4.5 12.5 5 5 10-11" />,
  plus: <path d="M12 4.5v15M4.5 12h15" />,
  trash: (
    <>
      <path d="M4.5 7h15M9 7V4.5h6V7M6.5 7l.8 13.5h9.4L17.5 7" />
      <path d="M10 11v5.5M14 11v5.5" />
    </>
  ),
  wifi: (
    <>
      <path d="M2.5 8.5a15 15 0 0 1 19 0M5.5 12a10.5 10.5 0 0 1 13 0M8.8 15.5a6 6 0 0 1 6.4 0" />
      <circle cx="12" cy="19" r="1.2" fill="currentColor" stroke="none" />
    </>
  ),
  bluetooth: <path d="m7 7 10 10-5 4V3l5 4L7 17" />,
  volume: (
    <>
      <path d="M4 9.5h4l5-4v13l-5-4H4z" />
      <path d="M16.5 8.5a5 5 0 0 1 0 7M19 6a8.5 8.5 0 0 1 0 12" />
    </>
  ),
  volumeOff: (
    <>
      <path d="M4 9.5h4l5-4v13l-5-4H4z" />
      <path d="m17 9.5 5 5M22 9.5l-5 5" />
    </>
  ),
  brightness: (
    <>
      <circle cx="12" cy="12" r="3.6" />
      <path d="M12 2.5v2M12 19.5v2M2.5 12h2M19.5 12h2M5.2 5.2l1.4 1.4M17.4 17.4l1.4 1.4M5.2 18.8l1.4-1.4M17.4 6.6l1.4-1.4" />
    </>
  ),
  folder: <path d="M3.5 6.5a2 2 0 0 1 2-2h4l2 2.5h7a2 2 0 0 1 2 2V18a2 2 0 0 1-2 2h-13a2 2 0 0 1-2-2z" />,
  browser: (
    <>
      <circle cx="12" cy="12" r="8.5" />
      <path d="M3.5 12h17M12 3.5c2.3 2.4 3.4 5.3 3.4 8.5s-1.1 6.1-3.4 8.5c-2.3-2.4-3.4-5.3-3.4-8.5S9.7 5.9 12 3.5Z" />
    </>
  ),
  task: (
    <>
      <rect x="4" y="4" width="16" height="16" rx="2.5" />
      <path d="M8 12.5l2.5 2.5L16 9.5" />
    </>
  ),
  note: (
    <>
      <path d="M6 3.5h9l4 4V20a1 1 0 0 1-1 1H6a1 1 0 0 1-1-1V4.5a1 1 0 0 1 1-1Z" />
      <path d="M14.5 3.8V8h4.2M9 12h6M9 15.5h6" />
    </>
  ),
  terminal: (
    <>
      <rect x="3.5" y="4.5" width="17" height="15" rx="2.2" />
      <path d="m7.5 9.5 3 2.5-3 2.5M12.5 15h4" />
    </>
  ),
  external: (
    <>
      <path d="M13.5 4.5H20V11M20 4.5 11 13.5" />
      <path d="M18 14v5a1.5 1.5 0 0 1-1.5 1.5H5A1.5 1.5 0 0 1 3.5 19V7.5A1.5 1.5 0 0 1 5 6h5" />
    </>
  ),
  pin: <path d="M9 4h6l-1 5 4 4-1.5 1.5L12 11l-4.5 3.5L6 13l4-4zM12 11v9" />,
  eye: (
    <>
      <path d="M2.5 12S6 5.5 12 5.5 21.5 12 21.5 12 18 18.5 12 18.5 2.5 12 2.5 12Z" />
      <circle cx="12" cy="12" r="3" />
    </>
  ),
  eyeOff: (
    <>
      <path d="M3 3l18 18M10.6 6.1A9.7 9.7 0 0 1 12 6c6 0 9.5 6 9.5 6a17 17 0 0 1-3.4 4M6.2 6.8A17 17 0 0 0 2.5 12S6 18 12 18a9.6 9.6 0 0 0 3.3-.6" />
      <path d="M9.9 9.9a3 3 0 0 0 4.2 4.2" />
    </>
  ),
  power: (
    <>
      <path d="M12 3v9" />
      <path d="M6.3 6.8a8 8 0 1 0 11.4 0" />
    </>
  ),
  search: (
    <>
      <circle cx="10.8" cy="10.8" r="6.5" />
      <path d="m15.7 15.7 4.8 4.8" />
    </>
  ),
  clock: (
    <>
      <circle cx="12" cy="12" r="8.5" />
      <path d="M12 7v5.3l3.4 2" />
    </>
  ),
  music: (
    <>
      <path d="M9 18.2V6.5l10-2v11.7" />
      <circle cx="6.5" cy="18.2" r="2.5" />
      <circle cx="16.5" cy="16.2" r="2.5" />
    </>
  ),
  quota: (
    <>
      <path d="M4 18V9M9.3 18V4M14.7 18v-6M20 18V7" />
    </>
  ),
  spark: <path d="M12 2.8c.6 5.4 3.8 8.6 9.2 9.2-5.4.6-8.6 3.8-9.2 9.2-.6-5.4-3.8-8.6-9.2-9.2 5.4-.6 8.6-3.8 9.2-9.2Z" />,
  arrowUp: <path d="M12 19V5M6.5 10.5 12 5l5.5 5.5" />,
  arrowDown: <path d="M12 5v14M6.5 13.5 12 19l5.5-5.5" />,
  mapPin: (
    <>
      <path d="M12 21s7-5.7 7-11a7 7 0 1 0-14 0c0 5.3 7 11 7 11Z" />
      <circle cx="12" cy="10" r="2.5" />
    </>
  ),
  wind: <path d="M3 8h11a3 3 0 1 0-3-3M3 12h15a3 3 0 1 1-3 3M3 16h8" />,
  drop: <path d="M12 3s6 6.2 6 10.2A6 6 0 0 1 6 13.2C6 9.2 12 3 12 3Z" />,
  sunrise: (
    <>
      <path d="M3 18h18M6.5 18a5.5 5.5 0 0 1 11 0" />
      <path d="M12 3v4M4.8 8.3l1.4 1.4M19.2 8.3l-1.4 1.4" />
    </>
  ),
  sunset: (
    <>
      <path d="M3 18h18M6.5 18a5.5 5.5 0 0 1 11 0" />
      <path d="M12 7V3M4.8 11.3l1.4-1.4M19.2 11.3l-1.4-1.4" />
    </>
  ),
  cpu: (
    <>
      <rect x="6" y="6" width="12" height="12" rx="2" />
      <rect x="9.5" y="9.5" width="5" height="5" rx="1" />
      <path d="M9 2.5V6M15 2.5V6M9 18v3.5M15 18v3.5M2.5 9H6M2.5 15H6M18 9h3.5M18 15h3.5" />
    </>
  ),
  globe: (
    <>
      <circle cx="12" cy="12" r="8.5" />
      <path d="M3.5 12h17M12 3.5c2.3 2.4 3.4 5.3 3.4 8.5s-1.1 6.1-3.4 8.5c-2.3-2.4-3.4-5.3-3.4-8.5S9.7 5.9 12 3.5Z" />
    </>
  ),
  camera: (
    <>
      <path d="M4.5 7.5h3l1.5-2h6l1.5 2h3a1.5 1.5 0 0 1 1.5 1.5v8a1.5 1.5 0 0 1-1.5 1.5h-15A1.5 1.5 0 0 1 3 17V9a1.5 1.5 0 0 1 1.5-1.5Z" />
      <circle cx="12" cy="13" r="3.2" />
    </>
  ),
  window: (
    <>
      <rect x="3.5" y="4.5" width="17" height="15" rx="2" />
      <path d="M3.5 9h17M7 6.8h.01M10 6.8h.01" />
    </>
  ),
  monitor: (
    <>
      <rect x="3" y="4.5" width="18" height="12.5" rx="1.8" />
      <path d="M9 21h6M12 17v4" />
    </>
  ),
  scroll: (
    <>
      <rect x="5.5" y="3.5" width="13" height="17" rx="2.2" />
      <path d="M12 7v7M9.5 11.5 12 14l2.5-2.5" />
      <path d="M9 17.5h6" />
    </>
  ),
  timer: (
    <>
      <circle cx="12" cy="13" r="7.5" />
      <path d="M12 9.5V13l2.4 1.6M9 2.5h6M18.8 6.2l1.4-1.4" />
    </>
  ),
  image: (
    <>
      <rect x="3.5" y="4.5" width="17" height="15" rx="2" />
      <circle cx="8.5" cy="9.5" r="1.6" />
      <path d="m5 18 5.2-5.2 3.3 3.3 2.4-2.4L20.5 18" />
    </>
  ),
  edit: (
    <>
      <path d="m4 20 4.4-1L19 8.4a1.9 1.9 0 0 0 0-2.7l-.7-.7a1.9 1.9 0 0 0-2.7 0L5 15.6 4 20Z" />
      <path d="m14 6.4 3.6 3.6" />
    </>
  ),
  /* 工具箱：带提手的工具盒（二维码 / 图片小工具都收在这里） */
  toolbox: (
    <>
      <rect x="2.8" y="8.2" width="18.4" height="12" rx="2.4" />
      <path d="M8.6 8.2V5.9a1.7 1.7 0 0 1 1.7-1.7h3.4a1.7 1.7 0 0 1 1.7 1.7v2.3" />
      <path d="M2.8 12.6h18.4M10 12.6v2.1h4v-2.1" />
    </>
  ),
  /* 会议录音：麦克风（正面话筒头 + 支架弧线） */
  mic: (
    <>
      <path d="M12 3.2a2.9 2.9 0 0 1 2.9 2.9v5.4a2.9 2.9 0 0 1-5.8 0V6.1A2.9 2.9 0 0 1 12 3.2Z" />
      <path d="M6.3 11.2v.5a5.7 5.7 0 0 0 11.4 0v-.5M12 17.4v3.4M8.8 20.8h6.4" />
    </>
  ),
};

interface IconProps {
  name: IconName;
  size?: number;
  strokeWidth?: number;
  className?: string;
  style?: CSSProperties;
}

export function Icon({ name, size = 18, strokeWidth = 1.8, className, style }: IconProps) {
  const filled = name === "play" || name === "pause" || name === "prev" || name === "next" || name === "spark";
  return (
    <svg
      className={className}
      style={style}
      width={size}
      height={size}
      viewBox="0 0 24 24"
      fill={filled ? "currentColor" : "none"}
      stroke={filled ? "none" : "currentColor"}
      strokeWidth={strokeWidth}
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden="true"
    >
      {PATHS[name]}
    </svg>
  );
}

// ============================================================
// 天气图形（比通用图标更细致，颜色由 CSS currentColor / 固定色控制）
// ============================================================

export function WeatherGlyph({
  icon,
  size = 28,
  className,
}: {
  icon: string;
  size?: number;
  className?: string;
}) {
  const common = {
    width: size,
    height: size,
    viewBox: "0 0 48 48",
    fill: "none",
    strokeLinecap: "round" as const,
    strokeLinejoin: "round" as const,
    className,
    "aria-hidden": true,
  };
  if (icon === "sun") {
    return (
      <svg {...common}>
        <circle cx="24" cy="24" r="8.5" className="wg-sun-core" />
        <g className="wg-sun-ray" strokeWidth="2.4">
          {Array.from({ length: 8 }, (_, i) => {
            const a = (i * Math.PI) / 4;
            return (
              <line
                key={i}
                x1={24 + Math.cos(a) * 13}
                y1={24 + Math.sin(a) * 13}
                x2={24 + Math.cos(a) * 18}
                y2={24 + Math.sin(a) * 18}
              />
            );
          })}
        </g>
      </svg>
    );
  }
  if (icon === "moon") {
    return (
      <svg {...common}>
        <path
          d="M31.5 29.8A12.5 12.5 0 0 1 18.2 16.5a12.8 12.8 0 1 0 13.3 13.3Z"
          className="wg-moon"
          strokeWidth="2.4"
        />
        <path d="M35 11.5v4M33 13.5h4" className="wg-star" strokeWidth="2" />
      </svg>
    );
  }
  const cloud = (
    <path
      d="M13.5 35h20.2a7 7 0 0 0 .4-13.9A9.3 9.3 0 0 0 15.8 23a6.2 6.2 0 0 0-2.3 12Z"
      className="wg-cloud"
      strokeWidth="2.4"
    />
  );
  if (icon === "partly") {
    return (
      <svg {...common}>
        <circle cx="30" cy="16" r="6" className="wg-sun-core" strokeWidth="2.2" />
        <g className="wg-sun-ray" strokeWidth="2">
          <line x1="30" y1="5.5" x2="30" y2="8" />
          <line x1="39" y1="10" x2="37.5" y2="12" />
          <line x1="21" y1="10" x2="22.5" y2="12" />
          <line x1="40" y1="22" x2="37.8" y2="22" />
        </g>
        <path
          d="M10 37h21a6.6 6.6 0 0 0 .3-13.2A9 9 0 0 0 13 28.5 5.8 5.8 0 0 0 10 37Z"
          className="wg-cloud"
          strokeWidth="2.4"
        />
      </svg>
    );
  }
  if (icon === "fog") {
    return (
      <svg {...common}>
        {cloud}
        <g className="wg-fog" strokeWidth="2.2">
          <line x1="11" y1="40" x2="37" y2="40" />
          <line x1="15" y1="44" x2="33" y2="44" />
        </g>
      </svg>
    );
  }
  if (icon === "drizzle" || icon === "rain" || icon === "thunder" || icon === "storm") {
    return (
      <svg {...common}>
        {cloud}
        {icon === "thunder" || icon === "storm" ? (
          <path d="M25 36 20 44h5l-2 4 8-9h-5l2-3Z" className="wg-bolt" strokeWidth="2" />
        ) : (
          <g className="wg-rain" strokeWidth="2.2">
            <line x1="17" y1="39" x2="15.5" y2="43" />
            <line x1="24" y1="39" x2="22.5" y2="43" />
            <line x1="31" y1="39" x2="29.5" y2="43" />
          </g>
        )}
      </svg>
    );
  }
  if (icon === "snow") {
    return (
      <svg {...common}>
        {cloud}
        <g className="wg-snow" strokeWidth="2">
          <path d="M17 39v6M14 41l6 2M20 41l-6 2" />
          <path d="M31 39v6M28 41l6 2M34 41l-6 2" />
        </g>
      </svg>
    );
  }
  return (
    <svg {...common}>
      <path
        d="M13.5 33h20.2a7 7 0 0 0 .4-13.9A9.3 9.3 0 0 0 15.8 21a6.2 6.2 0 0 0-2.3 12Z"
        className="wg-cloud"
        strokeWidth="2.4"
      />
    </svg>
  );
}

// ============================================================
// 品牌标记
// ============================================================

const BRAND_NAMES: Record<string, BrandIconName> = {
  zai: "zai",
  codex: "codex",
  claude: "claude",
  cursor: "cursor",
  kimi: "kimi",
  dsh: "dsh",
  gemini: "gemini",
  grok: "grok",
  qoder: "qoder",
  opencodego: "opencodego",
  minimax: "minimax",
  moonshot: "moonshot",
  deepseek: "deepseek",
  longcat: "longcat",
  mimo: "mimo",
  alibaba: "alibaba",
  alibabatoken: "alibabatoken",
  stepfun: "stepfun",
  doubao: "doubao",
};

const AGENT_LABELS: Record<string, string> = {
  zai: "ZCode",
  codex: "Codex",
  claude: "Claude",
  cursor: "Cursor",
  kimi: "Kimi",
  dsh: "DeepSeek",
  gemini: "Gemini",
  grok: "Grok",
  qoder: "Qoder",
  opencodego: "OpenCode",
  minimax: "MiniMax",
  moonshot: "Moonshot",
  deepseek: "DeepSeek",
  longcat: "LongCat",
  mimo: "MiMo",
  alibaba: "阿里云",
  alibabatoken: "百炼",
  stepfun: "StepFun",
  doubao: "豆包",
};

const AGENT_COLORS: Record<string, string> = {
  zai: "#5f8bff",
  codex: "#19c37d",
  claude: "#e8866b",
  cursor: "#e6e8ef",
  kimi: "#8b7cff",
  dsh: "#6d8dff",
  gemini: "#6f9bff",
  grok: "#f2f4f8",
  qoder: "#5dd4ff",
  opencodego: "#f2b35d",
  minimax: "#ff6d9a",
  minimaxcode: "#ff6d9a",
  moonshot: "#ffb65c",
  deepseek: "#5b7cff",
  longcat: "#67d6a8",
  mimo: "#ff9d6b",
  alibaba: "#ff9a3d",
  alibabatoken: "#ff7b5c",
  stepfun: "#7bd6ff",
  doubao: "#5eb7ff",
};

export function agentLabel(source: string): string {
  return AGENT_LABELS[source] ?? source.replace(/^\w/, (c) => c.toUpperCase());
}

export function agentColor(source: string): string {
  return AGENT_COLORS[source] ?? "#8fa3c8";
}

export function BrandMark({
  source,
  size = 20,
  className,
}: {
  source: string;
  size?: number;
  className?: string;
}) {
  const brand = BRAND_NAMES[source];
  const color = agentColor(source);
  if (brand) {
    return (
      <span
        className={`brand-mark ${className ?? ""}`}
        style={{ color, width: size, height: size }}
      >
        <BrandIcon brand={brand} width={size} height={size} />
      </span>
    );
  }
  return (
    <span
      className={`brand-mark brand-letter ${className ?? ""}`}
      style={{
        background: color,
        width: size,
        height: size,
        fontSize: Math.max(9, size * 0.46),
      }}
    >
      {agentLabel(source).slice(0, 1).toUpperCase()}
    </span>
  );
}

// ============================================================
// 额度 / 格式化
// ============================================================

export function remainingPct(q: HudBarQuota): number {
  return Math.max(0, Math.min(100, 100 - (Number.isFinite(q.usedPct) ? q.usedPct : 0)));
}

export function quotaTone(q: HudBarQuota): "ok" | "warn" | "low" {
  const used = Number.isFinite(q.usedPct) ? q.usedPct : 0;
  if (used >= 90) return "low";
  if (used >= 70) return "warn";
  return "ok";
}

export function quotaLabel(q: HudBarQuota, zh: boolean): string {
  return zh ? q.labelZh : q.labelEn;
}

export function fmtCountdown(resetAt: number | null, now: number): string {
  if (!resetAt || !Number.isFinite(resetAt)) return "";
  const ms = resetAt - now;
  if (ms <= 0) return "已重置";
  const totalMin = Math.ceil(ms / 60_000);
  if (totalMin < 60) return `${totalMin}m`;
  const totalHours = Math.floor(totalMin / 60);
  const days = Math.floor(totalHours / 24);
  const hours = totalHours % 24;
  const mins = totalMin % 60;
  if (days > 0) return hours > 0 ? `${days}d ${hours}h` : `${days}d`;
  return mins > 0 ? `${hours}h${String(mins).padStart(2, "0")}m` : `${hours}h`;
}

export function fmtClock(ts: number): string {
  return new Date(ts).toLocaleTimeString("zh-CN", {
    hour: "2-digit",
    minute: "2-digit",
    hour12: false,
  });
}

export function fmtDate(ts: number): string {
  return new Date(ts).toLocaleDateString("zh-CN", {
    month: "long",
    day: "numeric",
    weekday: "long",
  });
}

export function timeAgo(ts: number, now: number): string {
  const diff = Math.max(0, now - ts);
  const min = Math.floor(diff / 60_000);
  if (min < 1) return "刚刚";
  if (min < 60) return `${min} 分钟前`;
  const h = Math.floor(min / 60);
  if (h < 24) return `${h} 小时前`;
  const d = Math.floor(h / 24);
  if (d < 7) return `${d} 天前`;
  return new Date(ts).toLocaleDateString("zh-CN", { month: "2-digit", day: "2-digit" });
}

export function fmtDuration(seconds: number): string {
  const s = Math.max(0, Math.round(seconds));
  const m = Math.floor(s / 60);
  const r = s % 60;
  return `${m}:${String(r).padStart(2, "0")}`;
}

export function fmtMoney(amount: number, currency: string): string {
  const symbol =
    currency === "CNY" || currency === "¥" ? "¥" : currency === "USD" || currency === "$" ? "$" : currency;
  return `${symbol}${amount.toFixed(2)}`;
}

// ============================================================
// 歌词
// ============================================================

export interface LyricLine {
  time: number;
  text: string;
  trans: string;
}

function parseLrcText(raw: string): { time: number; text: string }[] {
  const out: { time: number; text: string }[] = [];
  if (!raw) return out;
  for (const line of raw.split(/\r?\n/)) {
    const times: number[] = [];
    const re = /\[(\d{1,3}):(\d{2})(?:[.:](\d{1,3}))?\]/g;
    let match: RegExpExecArray | null;
    let lastEnd = 0;
    while ((match = re.exec(line)) !== null) {
      const min = Number(match[1]);
      const sec = Number(match[2]);
      const frac = match[3] ? Number(`0.${match[3]}`) : 0;
      times.push(min * 60 + sec + frac);
      lastEnd = re.lastIndex;
    }
    const text = line.slice(lastEnd).trim();
    if (!text || times.length === 0) continue;
    for (const time of times) out.push({ time, text });
  }
  return out.sort((a, b) => a.time - b.time);
}

export function parseLyrics(lrc: string, translation: string): LyricLine[] {
  const lines = parseLrcText(lrc);
  const trans = parseLrcText(translation);
  const transByTime = new Map<number, string>();
  for (const t of trans) transByTime.set(Math.round(t.time * 10) / 10, t.text);
  return lines.map((line) => ({
    time: line.time,
    text: line.text,
    trans: transByTime.get(Math.round(line.time * 10) / 10) ?? "",
  }));
}

export function currentLyricLine(
  lines: LyricLine[],
  position: number
): LyricLine | null {
  if (lines.length === 0) return null;
  let current = lines[0];
  for (const line of lines) {
    if (line.time <= position + 0.15) current = line;
    else break;
  }
  return current;
}

// ============================================================
// 小组件
// ============================================================

export function QuotaBar({
  quota,
  compact = false,
}: {
  quota: HudBarQuota;
  compact?: boolean;
}) {
  const pct = remainingPct(quota);
  const tone = quotaTone(quota);
  return (
    <div className={`quota-bar ${compact ? "is-compact" : ""}`} data-tone={tone}>
      <div className="quota-bar-track">
        <span className="quota-bar-fill" style={{ width: `${pct}%` }} />
      </div>
    </div>
  );
}

export function Toggle({
  checked,
  onChange,
  disabled,
  label,
}: {
  checked: boolean;
  onChange: (next: boolean) => void;
  disabled?: boolean;
  label?: string;
}) {
  return (
    <button
      type="button"
      role="switch"
      aria-checked={checked}
      aria-label={label}
      disabled={disabled}
      className={`toggle ${checked ? "on" : ""} ${disabled ? "disabled" : ""}`}
      onClick={() => onChange(!checked)}
    >
      <span className="toggle-thumb" />
    </button>
  );
}

export function Slider({
  value,
  onChange,
  disabled,
  min = 0,
  max = 100,
  step = 1,
  ariaLabel,
}: {
  value: number;
  onChange: (next: number) => void;
  disabled?: boolean;
  min?: number;
  max?: number;
  step?: number;
  ariaLabel?: string;
}) {
  const pct = ((value - min) / Math.max(1, max - min)) * 100;
  return (
    <input
      className="island-range"
      type="range"
      min={min}
      max={max}
      step={step}
      value={value}
      disabled={disabled}
      aria-label={ariaLabel}
      style={{ "--range-pct": `${pct}%` } as CSSProperties}
      onChange={(e) => onChange(Number(e.target.value))}
    />
  );
}

export function SectionHead({
  title,
  hint,
  action,
}: {
  title: string;
  hint?: string;
  action?: ReactNode;
}) {
  return (
    <div className="section-head">
      <div>
        <div className="section-title">{title}</div>
        {hint ? <div className="section-hint">{hint}</div> : null}
      </div>
      {action}
    </div>
  );
}

export function EmptyState({
  icon = "spark",
  title,
  hint,
  action,
}: {
  icon?: IconName;
  title: string;
  hint?: string;
  action?: ReactNode;
}) {
  return (
    <div className="empty-state">
      <div className="empty-icon">
        <Icon name={icon} size={24} />
      </div>
      <div className="empty-title">{title}</div>
      {hint ? <div className="empty-hint">{hint}</div> : null}
      {action}
    </div>
  );
}

export function useDebouncedCallback<T extends (...args: never[]) => void>(
  callback: T,
  delayMs: number
): T {
  const [timer, setTimer] = useState<number | null>(null);
  const debounced = useMemo(() => {
    const fn = ((...args: never[]) => {
      if (timer != null) window.clearTimeout(timer);
      const id = window.setTimeout(() => callback(...args), delayMs);
      setTimer(id);
    }) as T;
    return fn;
  }, [callback, delayMs, timer]);
  useEffect(
    () => () => {
      if (timer != null) window.clearTimeout(timer);
    },
    [timer]
  );
  return debounced;
}
