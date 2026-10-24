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

/** 同步与设备：云 + 双向箭头（多设备上云同步的通用语义） */
export function SyncIcon({ className }: IconProps) {
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
      {/* 云轮廓（底部敞开，让位给内部箭头的视觉重心） */}
      <path d="M17.5 15.5a4 4 0 0 0 0-8 6 6 0 0 0-11.63 1.5A4.25 4.25 0 0 0 6.5 15.5" />
      {/* 云内双向箭头（⇄）：上传 / 下载双向同步 */}
      <path d="M8.5 12h7" />
      <path d="m10.5 10-2 2 2 2" />
      <path d="m13.5 10 2 2-2 2" />
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

/** 凭证与账号：人形 + 下方卡片（账号身份与登录凭证的通用语义） */
export function AccountsIcon({ className }: IconProps) {
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
      {/* 账号人形：头部圆 + 肩线 */}
      <circle cx="10" cy="7.5" r="3.2" />
      <path d="M4 18.2c0-2.9 2.7-4.4 6-4.4 1 0 2 .16 2.8.47" />
      {/* 右下角凭证卡：圆角矩形 + 两行信息位 */}
      <rect x="13" y="14" width="8.5" height="7" rx="1.6" />
      <path d="M15.2 17h4.1" />
      <path d="M15.2 19h2.6" />
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

/** 额度驾驶舱：半圆仪表盘 + 指针（额度余量仪表的通用语义；指针指向
    刻度中段，与页面「剩余额度」的仪表盘隐喻一致） */
export function QuotaIcon({ className }: IconProps) {
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
      {/* 仪表盘外廓：上半圆弧 + 底部封口 */}
      <path d="M4.5 17.5a8.5 8.5 0 1 1 15 0" />
      {/* 底部封口横线 */}
      <path d="M3 20.5h18" />
      {/* 指针：从圆心附近指向右上中段刻度 */}
      <path d="m12 15 4.2-4.2" />
      <circle cx="12" cy="15" r="1.1" fill="currentColor" stroke="none" />
    </svg>
  );
}

/** 模型速度：折线上升 + 箭头（窄浮窗速度页 SpeedGlyph 同一形态，输出
    速度/排行的语义；图形与窄窗一致，两处入口可对应认知） */
export function SpeedIcon({ className }: IconProps) {
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
      <path d="M4 17.5 9.2 12l3.2 3.1L20 7.5" />
      <path d="M15.5 7.5H20V12" />
    </svg>
  );
}

/** 设置：齿轮（通用设置语义，第八页「设置」的侧栏导航与页面标识） */
export function SettingsIcon({ className }: IconProps) {
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
      {/* 外齿轮：中心圆 + 八齿（等角分布的短辐条） */}
      <circle cx="12" cy="12" r="3.2" />
      <path d="M12 2.8v2.4M12 18.8v2.4M21.2 12h-2.4M5.2 12H2.8M18.5 5.5l-1.7 1.7M7.2 16.8l-1.7 1.7M18.5 18.5l-1.7-1.7M7.2 7.2 5.5 5.5" />
    </svg>
  );
}

/* ---- 自绘标题栏的窗口控制图标（最小化 / 最大化 / 还原 / 关闭） ----
   沿用上方 IconProps 模式，几何铺满 24 视口（描边外沿留约 1.5 安全边，
   strokeWidth 2 不裁切），12px 渲染尺寸下（TitleBar 用 h-3 w-3）字形约
   9.5px、有效线宽 1px，三键图标清晰可辨；"最大化"方框与"还原"后层共用
   同一光学上沿（y=2.5）与右沿（x=21.5），还原的前层方框缩小一号错位到
   左下，保持 Windows 标题栏的字形惯例。 */

/** 最小化：单根横线（横向占满，与最大化方框同宽） */
export function MinimizeIcon({ className }: IconProps) {
  return (
    <svg
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth="2"
      strokeLinecap="round"
      strokeLinejoin="round"
      className={className}
      aria-hidden
    >
      <path d="M2.5 12h19" />
    </svg>
  );
}

/** 最大化：单个方框 */
export function MaximizeIcon({ className }: IconProps) {
  return (
    <svg
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth="2"
      strokeLinecap="round"
      strokeLinejoin="round"
      className={className}
      aria-hidden
    >
      <rect x="2.5" y="2.5" width="19" height="19" rx="1.5" />
    </svg>
  );
}

/** 还原（两层错位方框）：前层方框 + 后层方框露出的上缘。后层折线两端都
    搭在前层边框上（起点落在前层顶边、经右沿下折后横线收在前层右边），
    不悬空；后层顶边 / 右沿与最大化字形对齐同一光学线 */
export function RestoreIcon({ className }: IconProps) {
  return (
    <svg
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth="2"
      strokeLinecap="round"
      strokeLinejoin="round"
      className={className}
      aria-hidden
    >
      <path d="M9 8.5V2.5h12.5v9H16" />
      <rect x="2.5" y="8.5" width="13.5" height="13.5" rx="1.5" />
    </svg>
  );
}

/** 关闭：×（对角线略短于最大化方框对角，Windows 惯例比例） */
export function CloseIcon({ className }: IconProps) {
  return (
    <svg
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth="2"
      strokeLinecap="round"
      strokeLinejoin="round"
      className={className}
      aria-hidden
    >
      <path d="M3 3l18 18" />
      <path d="M21 3L3 21" />
    </svg>
  );
}
