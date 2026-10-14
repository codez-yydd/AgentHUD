/**
 * 主窗口共享的卡片原语（原落在 overview/primitives，对比页 / 价格页也要用，
 * 故上移一层为纯搬移；overview/primitives 保留再导出，既有 import 不变）。
 * 卡片视觉对齐 dashboard.css 的 --db-* 变量体系（亮/暗两套），
 * 不用面板的 .card-base（那份依赖悬浮窗的毛玻璃底，普通窗会穿帮）。
 */
import { useEffect, useState } from "react";
import type { ReactNode } from "react";

/**
 * 当前是否暗色主题：观察 <html> 的 .dark 类（DashboardApp 的 applyDomTheme 与
 * 面板 applyTheme 都只切这个类）。用于需要按主题换色的内联样式（如对比页的
 * 系列色阶）；纯 CSS 能表达的翻转仍交给 CSS 变量，不走这里。
 */
export function useDarkTheme(): boolean {
  const [dark, setDark] = useState(
    () =>
      typeof document !== "undefined" &&
      document.documentElement.classList.contains("dark")
  );
  useEffect(() => {
    const target = document.documentElement;
    const observer = new MutationObserver(() => {
      setDark(target.classList.contains("dark"));
    });
    observer.observe(target, { attributes: true, attributeFilter: ["class"] });
    return () => observer.disconnect();
  }, []);
  return dark;
}

/** 区块卡片：标题行（左标题+副标题，右操作区）+ 内容区 */
export function Card({
  title,
  sub,
  action,
  children,
  className = "",
}: {
  title?: string;
  sub?: string;
  action?: ReactNode;
  children: ReactNode;
  className?: string;
}) {
  return (
    <section className={`db-card ${className}`}>
      {(title || action) && (
        <header className="db-card__head">
          <div className="db-card__headline">
            {title && <h2 className="db-card__title">{title}</h2>}
            {sub && <span className="db-card__sub">{sub}</span>}
          </div>
          {action && <div className="db-card__action">{action}</div>}
        </header>
      )}
      <div className="db-card__body">{children}</div>
    </section>
  );
}

/** 骨架块（animate-pulse 由 CSS 负责，宽高由调用方给） */
export function Skeleton({
  className = "",
  style,
}: {
  className?: string;
  style?: React.CSSProperties;
}) {
  return <div className={`db-skeleton ${className}`} style={style} />;
}
