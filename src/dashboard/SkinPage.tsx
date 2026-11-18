/**
 * 皮肤中心页（主窗口大窗版，悬浮窗皮肤域三层结构的迁入口）。
 *
 * 结构（沿用悬浮窗 skinHub → skinStandalone / theme 的三层语义，子页切换
 * 收进本页组件内 state，不新增侧栏项）：
 * - hub：两张入口卡（免注入独立窗口 / 需注入 ZCode），主窗口骨架自绘
 *   （文案与图标沿用 SkinHubPanel 的 theme.* 词条与调色板/窗口图形）；
 * - standalone：复用 SkinStandalonePanel 本体（会话悬浮窗 + 桌面宠物）；
 * - injected：复用 ThemePanel 本体（ZCode 动态壁纸）。
 *
 * 复用组件不做改动：两者按 300px 窄窗版式编写（PageShell 全高 + PageBody
 * 内滚 + SettingsCard 卡片流），由 .db-skin-embed 限宽居中承载（
 * dashboard.css：高度撑满主区、内容列 860px、字号按共享组件大窗适配
 * 的既有思路整体提升），子页头部「返回」回到本页 hub 视图——皮肤子页
 * 间的导航语义与悬浮窗一致，功能入口无删减。
 *
 * 子页记忆放模块级单例（对齐 PricingPage 的草稿单例做法）：切侧栏页面
 * 会卸载本组件，放组件 state 里子页位置会丢；单例在窗口会话内存活，
 * 重新挂载时恢复。
 *
 * 硬性约束继承自 SkinStandalonePanel：hub 视图不发起任何后端调用（尤其
 * 不做注入状态检测），扫描 ZCode 安装目录的开销仍由用户显式进入需注入
 * 子页时才发生。
 */
import { useEffect, useState } from "react";
import type { ReactNode } from "react";
import { useI18n } from "../i18n";
import { SkinStandalonePanel } from "../SkinStandalonePanel";
import { ThemePanel } from "../ThemePanel";

/** 皮肤域子页：hub 分类页 / 免注入子页 / 需注入子页（与悬浮窗 View 同义） */
type SkinSub = "hub" | "standalone" | "injected";

/**
 * 模块级子页单例：切侧栏页面卸载本组件后保留最后一次所在子页，
 * 重新挂载时恢复（同一窗口会话内有效）。
 */
let skinSubStore: SkinSub = "hub";

export function SkinPage() {
  const { t } = useI18n();
  const [sub, setSub] = useState<SkinSub>(() => skinSubStore);
  // 每次变更写回单例（卸载后保留）
  useEffect(() => {
    skinSubStore = sub;
  }, [sub]);

  // 子页：复用悬浮窗组件本体，返回箭头回到本页 hub（不出皮肤域）
  if (sub === "standalone") {
    return (
      <SkinEmbed>
        <SkinStandalonePanel onBack={() => setSub("hub")} />
      </SkinEmbed>
    );
  }
  if (sub === "injected") {
    return (
      <SkinEmbed>
        <ThemePanel onBack={() => setSub("hub")} />
      </SkinEmbed>
    );
  }

  // hub 分类页：主窗口骨架自绘两张入口卡（大窗双列网格，不再沿用
  // SkinHubPanel 的 300px 窄窗单列卡——该组件强绑面板 PageHeader 外壳，
  // 大窗形态按 ComparePage 先例就地实现，文案词条与图形完全一致）
  return (
    <div className="db-page db-skin-hub-page">
      <header className="db-page__header">
        <div>
          <h1 className="db-page__title">{t("dashboard.skin")}</h1>
          <p className="db-page__desc">{t("dashboard.skin.desc")}</p>
        </div>
      </header>

      <div className="db-skin-hub">
        <SkinEntryCard
          icon={
            <svg
              viewBox="0 0 24 24"
              fill="none"
              stroke="currentColor"
              strokeWidth="2"
              strokeLinecap="round"
              strokeLinejoin="round"
              className="h-5 w-5"
              aria-hidden
            >
              {/* 独立窗口：窗体 + 标题栏 + 两个窗口控制点（SkinHubPanel 同款） */}
              <rect x="3" y="4" width="18" height="16" rx="2" />
              <path d="M3 9h18" />
              <circle cx="6" cy="6.5" r=".5" fill="currentColor" />
              <circle cx="8.5" cy="6.5" r=".5" fill="currentColor" />
            </svg>
          }
          title={t("theme.sectionStandalone")}
          desc={t("theme.hubStandaloneDesc")}
          onClick={() => setSub("standalone")}
        />
        <SkinEntryCard
          icon={
            // 调色板：与窄窗 SkinHubPanel / 侧栏 SkinIcon 同款图形（放大版）
            <svg
              viewBox="0 0 24 24"
              fill="none"
              stroke="currentColor"
              strokeWidth="2"
              strokeLinecap="round"
              strokeLinejoin="round"
              className="h-5 w-5"
              aria-hidden
            >
              <circle cx="13.5" cy="6.5" r=".5" fill="currentColor" />
              <circle cx="17.5" cy="10.5" r=".5" fill="currentColor" />
              <circle cx="8.5" cy="7.5" r=".5" fill="currentColor" />
              <circle cx="6.5" cy="12.5" r=".5" fill="currentColor" />
              <path d="M12 2C6.5 2 2 6.5 2 12s4.5 10 10 10c.926 0 1.648-.746 1.648-1.688 0-.437-.18-.835-.437-1.125-.29-.289-.438-.652-.438-1.125a1.64 1.64 0 0 1 1.668-1.668h1.996c3.051 0 5.555-2.503 5.555-5.554C21.965 6.012 17.461 2 12 2z" />
            </svg>
          }
          title={t("theme.sectionInjected")}
          desc={t("theme.hubInjectedDesc")}
          onClick={() => setSub("injected")}
        />
      </div>
    </div>
  );
}

/** hub 入口卡：图标 + 标题 + 说明 + 右侧箭头（语义沿窄窗 EntryCard，
 *  尺寸与配色对齐主窗口卡片体系：db-card 面 + hover 提亮 + 主区按钮惯例） */
function SkinEntryCard({
  icon,
  title,
  desc,
  onClick,
}: {
  icon: ReactNode;
  title: string;
  desc: string;
  onClick: () => void;
}) {
  return (
    <button type="button" onClick={onClick} className="db-skin-entry">
      <span className="db-skin-entry__icon">{icon}</span>
      <span className="db-skin-entry__text">
        <span className="db-skin-entry__title">{title}</span>
        <span className="db-skin-entry__desc">{desc}</span>
      </span>
      <svg
        viewBox="0 0 24 24"
        fill="none"
        stroke="currentColor"
        strokeWidth="2"
        strokeLinecap="round"
        strokeLinejoin="round"
        className="db-skin-entry__arrow"
        aria-hidden
      >
        <path d="m9 18 6-6-6-6" />
      </svg>
    </button>
  );
}

/**
 * 子页嵌入容器：SkinStandalonePanel / ThemePanel 是 PageShell 全高版式，
 * 直接作为 db-main 子元素渲染（不套 db-page，避免百分比高度在
 * min-height 容器里解析不确定），宽度约束与留白由本容器承担。
 */
function SkinEmbed({ children }: { children: ReactNode }) {
  return <div className="db-skin-embed">{children}</div>;
}
