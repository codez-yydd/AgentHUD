/**
 * 主窗口（Dashboard）图标集：手写内联 SVG（项目无 lucide 等图标依赖，
 * 面板 toolbar 也是手写 SVG / 字符图标，此处沿用同一做法）。
 * 统一 24 视口 + currentColor 描边，尺寸由调用方 className 决定。
 */

/** 图标公共 props：尺寸/颜色全部交给 className（如 "h-4 w-4"） */
export interface IconProps {
  className?: string;
}

/** 用量总览：坐标轴 + 三根柱（用量/趋势的通用语义） */
export function OverviewIcon({ className }: IconProps) {
  return (
    <svg
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth="1.8"
      strokeLinecap="round"
      strokeLinejoin="round"
      className={className}
      aria-hidden
    >
      <path d="M3 3v18h18" />
      <path d="M7.5 16v-6" />
      <path d="M12 16V6.5" />
      <path d="M16.5 16v-3.5" />
    </svg>
  );
}

/** 请求日志：列表线条（每行 = 一次请求的日志语义） */
export function LogsIcon({ className }: IconProps) {
  return (
    <svg
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth="1.8"
      strokeLinecap="round"
      strokeLinejoin="round"
      className={className}
      aria-hidden
    >
      <path d="M9 6h11" />
      <path d="M9 12h11" />
      <path d="M9 18h11" />
      <path d="M4.5 6h.01" />
      <path d="M4.5 12h.01" />
      <path d="M4.5 18h.01" />
    </svg>
  );
}

/** 周额度对比：双向箭头（两个口径对照的通用语义） */
export function CompareIcon({ className }: IconProps) {
  return (
    <svg
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth="1.8"
      strokeLinecap="round"
      strokeLinejoin="round"
      className={className}
      aria-hidden
    >
      <path d="M8 3 4 7l4 4" />
      <path d="M4 7h16" />
      <path d="m16 21 4-4-4-4" />
      <path d="M20 17H4" />
    </svg>
  );
}

/** 价格设置：价签（单价/价格表语义） */
export function PricingIcon({ className }: IconProps) {
  return (
    <svg
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth="1.8"
      strokeLinecap="round"
      strokeLinejoin="round"
      className={className}
      aria-hidden
    >
      <path d="M12.586 2.586A2 2 0 0 0 11.172 2H4a2 2 0 0 0-2 2v7.172a2 2 0 0 0 .586 1.414l8.704 8.704a2.426 2.426 0 0 0 3.42 0l6.58-6.58a2.426 2.426 0 0 0 0-3.42z" />
      <circle cx="7.5" cy="7.5" r="1.5" fill="currentColor" stroke="none" />
    </svg>
  );
}

/** 刷新：环形箭头（用量总览页刷新按钮；加载中由调用方加旋转类） */
export function RefreshIcon({ className }: IconProps) {
  return (
    <svg
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth="1.8"
      strokeLinecap="round"
      strokeLinejoin="round"
      className={className}
      aria-hidden
    >
      <path d="M21 12a9 9 0 1 1-2.64-6.36" />
      <path d="M21 3.5V9h-5.5" />
    </svg>
  );
}
