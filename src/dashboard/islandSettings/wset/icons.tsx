/**
 * 灵动岛设置窗（wset）图标库：14 个导航图标 + 控件用的小图标。
 */
import type { ReactNode, SVGProps } from "react";

export interface WsetIconProps extends Omit<SVGProps<SVGSVGElement>, "children"> {
  /** 边长（逻辑 px），默认 16 */
  size?: number;
}

/** 统一外壳：16 viewBox、描边取 currentColor、不接收 fill */
function Glyph({
  size = 16,
  children,
  ...rest
}: WsetIconProps & { children: ReactNode }) {
  return (
    <svg
      width={size}
      height={size}
      viewBox="0 0 16 16"
      fill="none"
      stroke="currentColor"
      strokeWidth={1.5}
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden="true"
      focusable="false"
      {...rest}
    >
      {children}
    </svg>
  );
}

// ===== 导航图标（顺序与 PAGES 一致）=====

/* 外观与交互：星芒（原版用「换肤」语义，眼睛容易被读成「显示 / 隐藏」） */
export const LookIcon = (p: WsetIconProps) => (
  <Glyph {...p}>
    <path d="M7.3 3.3 8.5 7.5 12.7 8.7 8.5 9.9 7.3 14.1 6.1 9.9 1.9 8.7 6.1 7.5Z" />
    <path d="M12.9 1.1 13.5 2.6 15 3.2 13.5 3.8 12.9 5.3 12.3 3.8 10.8 3.2 12.3 2.6Z" />
  </Glyph>
);

export const OrderIcon = (p: WsetIconProps) => (
  <Glyph {...p}>
    <path d="M5 3v10M5 13 2.8 10.8M5 13l2.2-2.2" />
    <path d="M11 13V3M11 3 8.8 5.2M11 3l2.2 2.2" />
  </Glyph>
);

export const HotkeyIcon = (p: WsetIconProps) => (
  <Glyph {...p}>
    <path d="M9.1 1.6 4 9h3.6L6.9 14.4 12 7H8.4Z" />
  </Glyph>
);

export const ActionsIcon = (p: WsetIconProps) => (
  <Glyph {...p}>
    <path d="M3.1 2.6 12.4 7l-4.1 1.5-1.5 4.1Z" />
    <path d="M12.6 11.4h1.8M13.5 10.5v1.8" />
  </Glyph>
);

export const MediaIcon = (p: WsetIconProps) => (
  <Glyph {...p}>
    <path d="M6.4 12.2V4.4l6.2-1.6v7.8" />
    <circle cx="4.7" cy="12" r="1.7" />
    <circle cx="10.9" cy="10.6" r="1.7" />
  </Glyph>
);

/* 消息与邮件：气泡（尾角更大 + 三个点，14px 下不会读成文档） */
export const MessageIcon = (p: WsetIconProps) => (
  <Glyph {...p}>
    <path d="M13.5 9.5a1.6 1.6 0 0 1-1.6 1.6H6.2L2.9 13.7V4.1a1.6 1.6 0 0 1 1.6-1.6h7.4a1.6 1.6 0 0 1 1.6 1.6Z" />
    <circle cx="5.7" cy="6.9" r=".72" fill="currentColor" stroke="none" />
    <circle cx="8" cy="6.9" r=".72" fill="currentColor" stroke="none" />
    <circle cx="10.3" cy="6.9" r=".72" fill="currentColor" stroke="none" />
  </Glyph>
);

export const StockIcon = (p: WsetIconProps) => (
  <Glyph {...p}>
    <path d="M2 13.2h12" />
    <path d="M3.4 10.6 6.6 7l2.4 2 3.6-4.4" />
    <path d="M10.6 4.6h2v2" />
  </Glyph>
);

export const WeatherIcon = (p: WsetIconProps) => (
  <Glyph {...p}>
    <path d="M4.4 11.8a2.7 2.7 0 0 1 .3-5.4 3.4 3.4 0 0 1 6.5.7 2.4 2.4 0 0 1-.6 4.7Z" />
  </Glyph>
);

export const TranslateIcon = (p: WsetIconProps) => (
  <Glyph {...p}>
    <path d="M2 3.6h5.6M4.8 3.6v1.2c0 2.4-1.1 4.4-2.8 5.6" />
    <path d="M2.9 6.4c.7 1.6 2.1 2.8 3.6 3.3" />
    <path d="M9.2 13.4 11.4 7l2.2 6.4M10 11.4h2.8" />
  </Glyph>
);

export const LiveIcon = (p: WsetIconProps) => (
  <Glyph {...p}>
    <rect x="6.2" y="1.8" width="3.6" height="6.6" rx="1.8" />
    <path d="M4.2 7.4a3.8 3.8 0 0 0 7.6 0M8 11.2v3" />
  </Glyph>
);

export const LauncherIcon = (p: WsetIconProps) => (
  <Glyph {...p}>
    <rect x="4.2" y="2.8" width="7.6" height="11" rx="1.4" />
    <path d="M6.4 2.8V2a.6.6 0 0 1 .6-.6h2a.6.6 0 0 1 .6.6v.8" />
    <path d="M6.2 6.4h3.6M6.2 9h2.4" />
  </Glyph>
);

/* 系统与控制：滑杆（两条轨道 + 实心滑块；左右箭头容易读成「排序」） */
export const SystemIcon = (p: WsetIconProps) => (
  <Glyph {...p}>
    <path d="M2.2 5.6h2.5M8.6 5.6h5.2" />
    <circle cx="6.6" cy="5.6" r="1.9" fill="currentColor" stroke="none" />
    <path d="M2.2 10.4h5.2M11.4 10.4h2.4" />
    <circle cx="9.4" cy="10.4" r="1.9" fill="currentColor" stroke="none" />
  </Glyph>
);

export const GearIcon = (p: WsetIconProps) => (
  <Glyph {...p}>
    <circle cx="8" cy="8" r="2.4" />
    <path d="M8 1.4v1.9M8 12.7v1.9M1.4 8h1.9M12.7 8h1.9M3.3 3.3l1.4 1.4M11.3 11.3l1.4 1.4M12.7 3.3l-1.4 1.4M4.7 11.3l-1.4 1.4" />
    <circle cx="8" cy="8" r="6.1" />
  </Glyph>
);

export const InfoIcon = (p: WsetIconProps) => (
  <Glyph {...p}>
    <circle cx="8" cy="8" r="6.2" />
    <path d="M8 7.1v4.2" />
    <path d="M8 4.9h.01" />
  </Glyph>
);

// ===== 控件 / 操作图标 =====

export const ChevronDownIcon = (p: WsetIconProps) => (
  <Glyph {...p}>
    <path d="M4 6.2 8 10.2l4-4" />
  </Glyph>
);

/**
 * 下拉箭头：带竖线的 ↓（对齐原版 Segoe Fluent 9px 的 DOWN 字形 —— 8×8 字形里
 * 有一条短竖线，不是 4×3 的细 chevron）。字形在 16 网格内居中，右侧留白由
 * `.wset-select__chev` 的负边距抵消，保持「字形右缘距边框 10px」。
 */
export const DownArrowIcon = (p: WsetIconProps) => (
  <Glyph {...p}>
    <path d="M8 4.4V12" />
    <path d="M4.9 8.7 8 11.9l3.1-3.2" />
  </Glyph>
);

/** 胶囊勾：左臂短、右臂长，1x 下也能读成 ✓（10px 全勾会糊成一团） */
export const ChipCheckIcon = (p: WsetIconProps) => (
  <Glyph {...p}>
    <path d="M3.6 8.6 6.6 11.4 12.4 4.4" />
  </Glyph>
);

export const CheckIcon = (p: WsetIconProps) => (
  <Glyph {...p}>
    <path d="M3.2 8.4 6.4 11.6 12.8 4.6" />
  </Glyph>
);

export const CloseIcon = (p: WsetIconProps) => (
  <Glyph {...p}>
    <path d="M4.2 4.2 11.8 11.8M11.8 4.2 4.2 11.8" />
  </Glyph>
);

export const ArrowUpIcon = (p: WsetIconProps) => (
  <Glyph {...p}>
    <path d="M8 12.4V3.6M4.4 7.2 8 3.6l3.6 3.6" />
  </Glyph>
);

export const ArrowDownIcon = (p: WsetIconProps) => (
  <Glyph {...p}>
    <path d="M8 3.6v8.8M4.4 8.8 8 12.4l3.6-3.6" />
  </Glyph>
);

export const SearchIcon = (p: WsetIconProps) => (
  <Glyph {...p}>
    <circle cx="7" cy="7" r="4.1" />
    <path d="M10.1 10.1 13.6 13.6" />
  </Glyph>
);

export const EyeIcon = (p: WsetIconProps) => (
  <Glyph {...p}>
    <path d="M1.6 8S4.2 3.6 8 3.6 14.4 8 14.4 8 11.8 12.4 8 12.4 1.6 8 1.6 8Z" />
    <circle cx="8" cy="8" r="2.1" />
  </Glyph>
);

export const EyeOffIcon = (p: WsetIconProps) => (
  <Glyph {...p}>
    <path d="M6.6 4a5.8 5.8 0 0 1 1.4-.2c3.8 0 6.4 4.2 6.4 4.2a11 11 0 0 1-1.8 2.2" />
    <path d="M4.1 5.1A11.3 11.3 0 0 0 1.6 8S4.2 12.2 8 12.2a5.9 5.9 0 0 0 2.3-.45" />
    <path d="M2.4 2.4 13.6 13.6" />
  </Glyph>
);

export const OpenIcon = (p: WsetIconProps) => (
  <Glyph {...p}>
    <path d="M9.4 2.6h4v4M13.4 2.6 8.2 7.8" />
    <path d="M12.4 9.6v2.8a1 1 0 0 1-1 1H3.6a1 1 0 0 1-1-1V4.6a1 1 0 0 1 1-1h2.8" />
  </Glyph>
);

export const PowerIcon = (p: WsetIconProps) => (
  <Glyph {...p}>
    <path d="M8 1.8v5.4" />
    <path d="M4.5 4.1a5.1 5.1 0 1 0 7 0" />
  </Glyph>
);

export const PlusIcon = (p: WsetIconProps) => (
  <Glyph {...p}>
    <path d="M8 3.2v9.6M3.2 8h9.6" />
  </Glyph>
);

export const TrashIcon = (p: WsetIconProps) => (
  <Glyph {...p}>
    <path d="M3.2 5h9.6M5.6 5V3.4h4.8V5" />
    <path d="M4.4 5l.6 8h6l.6-8" />
  </Glyph>
);

export const FolderIcon = (p: WsetIconProps) => (
  <Glyph {...p}>
    <path d="M2 5.2h4.2l1.3 1.6h6.5v6.4H2Z" />
  </Glyph>
);

export const MonitorIcon = (p: WsetIconProps) => (
  <Glyph {...p}>
    <rect x="1.8" y="3" width="12.4" height="8" rx="1.2" />
    <path d="M5.6 13.4h4.8M8 11v2.4" />
  </Glyph>
);

export const FontIcon = (p: WsetIconProps) => (
  <Glyph {...p}>
    <path d="M2.8 12.6 6.4 3.4l3.6 9.2M4 9.8h4.8" />
    <path d="M11 12.6V7.4M11 9.4c.5-.7 1.2-1 1.9-1" />
  </Glyph>
);

export const ChipsIcon = (p: WsetIconProps) => (
  <Glyph {...p}>
    <rect x="1.8" y="5.4" width="5.4" height="5.2" rx="2.6" />
    <rect x="8.8" y="5.4" width="5.4" height="5.2" rx="2.6" />
  </Glyph>
);
