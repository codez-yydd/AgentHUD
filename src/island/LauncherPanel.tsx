/**
 * 灵动岛「启动器」子页：一个搜索框找得到所有能去的地方。
 *
 * 四类目标（顺序即默认排序）：
 *  - 岛内页面：直接切标签（主页 / 消息 / 剪贴板 / 截图 / 工具 …）
 *  - 主窗口页面：调 open_main_window(page) 跳页（用量总览 / 请求日志 / 额度驾驶舱 …）
 *  - 快捷动作：截图、录屏、打开截图文件夹、打开主窗口
 *  - 项目目录：近 30 天有会话的项目，回车用系统默认方式打开目录
 *
 * 键盘：↑↓ 选择、回车执行、Esc 清空搜索。搜索是大小写不敏感的子串匹配，
 * 命中 label / 关键词任一即可（关键词里放了拼音首字母与英文名，方便盲打）。
 */
import { useEffect, useMemo, useRef, useState } from "react";
import { launcherProjects } from "./api";
import { openShotFolder, startShot } from "../shot/api";
import type { LauncherProject, TabKey } from "./types";
import { Icon, type IconName } from "./ui";

interface LauncherEntry {
  id: string;
  label: string;
  hint: string;
  /** 参与搜索的额外关键词（空格分隔） */
  keywords: string;
  icon: IconName;
  run: () => void;
}

const ISLAND_TABS: { key: TabKey; label: string; icon: IconName; keywords: string }[] = [
  { key: "home", label: "主页", icon: "home", keywords: "home zhuye" },
  { key: "msg", label: "消息", icon: "message", keywords: "message xiaoxi notify" },
  { key: "stock", label: "股票", icon: "chart", keywords: "stock gupiao" },
  { key: "weather", label: "天气", icon: "weather", keywords: "weather tianqi" },
  { key: "ctl", label: "控制", icon: "sliders", keywords: "control kongzhi volume brightness" },
  { key: "clip", label: "剪贴板", icon: "clipboard", keywords: "clipboard jiantieban copy" },
  { key: "shot", label: "截图", icon: "camera", keywords: "screenshot jietu record" },
  { key: "tool", label: "工具", icon: "toolbox", keywords: "tool gongju qr image snippet" },
  { key: "settings", label: "设置", icon: "settings", keywords: "settings shezhi" },
];

/** 主窗口可跳页（与 DashboardApp / main_window 白名单同源，缺页只会安全失败） */
const DASHBOARD_PAGES: { page: string; label: string; keywords: string }[] = [
  { page: "overview", label: "用量总览", keywords: "overview zonglan usage" },
  { page: "logs", label: "请求日志", keywords: "logs rizhi requests" },
  { page: "sessions", label: "会话", keywords: "sessions huihua live" },
  { page: "speed", label: "模型速度", keywords: "speed sudu tps" },
  { page: "compare", label: "周额度对比", keywords: "compare duibi weekly" },
  { page: "quota", label: "额度驾驶舱", keywords: "quota edu cockpit" },
  { page: "sync", label: "同步与设备", keywords: "sync tongbu devices" },
  { page: "accounts", label: "凭证与账号", keywords: "accounts zhanghao credential" },
  { page: "pricing", label: "价格设置", keywords: "pricing jiage price" },
  { page: "agents", label: "Agent 与 Skill", keywords: "agents skill assets" },
  { page: "settings", label: "设置", keywords: "settings shezhi main" },
];

export function LauncherPanel({
  onOpenTab,
  onOpenMainWindow,
  onOpenPath,
  onToast,
}: {
  onOpenTab: (tab: TabKey) => void;
  onOpenMainWindow: (page?: string) => void;
  onOpenPath: (path: string) => void;
  onToast: (message: string) => void;
}) {
  const [query, setQuery] = useState("");
  const [active, setActive] = useState(0);
  const [projects, setProjects] = useState<LauncherProject[]>([]);
  const inputRef = useRef<HTMLInputElement | null>(null);

  useEffect(() => {
    inputRef.current?.focus();
    void launcherProjects()
      .then(setProjects)
      .catch(() => {
        /* 项目列表取不到不影响其它入口 */
      });
  }, []);

  const entries = useMemo<LauncherEntry[]>(() => {
    const islandEntries: LauncherEntry[] = ISLAND_TABS.map((tab) => ({
      id: `tab:${tab.key}`,
      label: tab.label,
      hint: "跳到岛内页面",
      keywords: tab.keywords,
      icon: tab.icon,
      run: () => onOpenTab(tab.key),
    }));

    const dashboardEntries: LauncherEntry[] = DASHBOARD_PAGES.map((page) => ({
      id: `page:${page.page}`,
      label: page.label,
      hint: "在主窗口打开",
      keywords: page.keywords,
      icon: "external",
      run: () => onOpenMainWindow(page.page),
    }));

    const actionEntries: LauncherEntry[] = [
      {
        id: "action:shot",
        label: "截个图",
        hint: "框选区域截图",
        keywords: "screenshot jietu capture",
        icon: "camera",
        run: () => {
          void startShot("region", 0).catch((error) => onToast(String(error)));
        },
      },
      {
        id: "action:record",
        label: "开始录屏",
        hint: "框选区域录屏",
        keywords: "record luping video",
        icon: "monitor",
        run: () => {
          void startShot("record", 0).catch((error) => onToast(String(error)));
        },
      },
      {
        id: "action:folder",
        label: "打开截图文件夹",
        hint: "在文件管理器里查看历史截图",
        keywords: "folder wenjianjia screenshots",
        icon: "folder",
        run: () => {
          void openShotFolder().catch((error) => onToast(String(error)));
        },
      },
      {
        id: "action:main",
        label: "打开主窗口",
        hint: "展开完整的用量仪表盘",
        keywords: "main window zhu chuangkou dashboard",
        icon: "external",
        run: () => onOpenMainWindow(),
      },
    ];

    const projectEntries: LauncherEntry[] = projects
      .filter((project) => project.display_path)
      .map((project) => ({
        id: `project:${project.key}`,
        label: project.display_path ?? project.key,
        hint: `${project.sessions} 个会话 · 回车打开目录`,
        keywords: project.key,
        icon: "task",
        run: () => {
          if (project.display_path) onOpenPath(project.display_path);
        },
      }));

    // 默认顺序：动作 → 项目 → 主窗口页 → 岛内页（前两类更有「启动器」的味道）
    return [...actionEntries, ...projectEntries, ...dashboardEntries, ...islandEntries];
  }, [projects, onOpenTab, onOpenMainWindow, onOpenPath, onToast]);

  const keyword = query.trim().toLowerCase();
  const visible = useMemo(() => {
    if (!keyword) return entries.slice(0, 24);
    const hits = entries.filter(
      (entry) =>
        entry.label.toLowerCase().includes(keyword) ||
        entry.keywords.toLowerCase().includes(keyword)
    );
    return hits.slice(0, 24);
  }, [entries, keyword]);

  // 搜索词变了就回到第一项，避免回车执行到上一次高亮的东西
  useEffect(() => {
    setActive(0);
  }, [keyword]);

  const runEntry = (entry: LauncherEntry) => {
    entry.run();
    setQuery("");
  };

  const onKeyDown = (event: React.KeyboardEvent<HTMLInputElement>) => {
    if (event.key === "ArrowDown") {
      event.preventDefault();
      setActive((index) => (visible.length ? (index + 1) % visible.length : 0));
    } else if (event.key === "ArrowUp") {
      event.preventDefault();
      setActive((index) =>
        visible.length ? (index - 1 + visible.length) % visible.length : 0
      );
    } else if (event.key === "Enter") {
      event.preventDefault();
      const entry = visible[active];
      if (entry) runEntry(entry);
    } else if (event.key === "Escape") {
      setQuery("");
    }
  };

  return (
    <div className="launch-page">
      <label className="launch-search">
        <Icon name="search" size={12} />
        <input
          ref={inputRef}
          value={query}
          placeholder="搜页面、动作或项目…"
          onChange={(event) => setQuery(event.target.value)}
          onKeyDown={onKeyDown}
        />
      </label>

      {visible.length === 0 ? (
        <div className="tool-empty">没有匹配项，换个词试试</div>
      ) : (
        <div className="launch-list" role="listbox" aria-label="启动项">
          {visible.map((entry, index) => (
            <button
              key={entry.id}
              type="button"
              role="option"
              aria-selected={index === active}
              className={`launch-item ${index === active ? "active" : ""}`}
              onMouseEnter={() => setActive(index)}
              onClick={() => runEntry(entry)}
            >
              <span className="launch-icon">
                <Icon name={entry.icon} size={13} />
              </span>
              <span className="launch-main">
                <span className="launch-label">{entry.label}</span>
                <span className="launch-hint">{entry.hint}</span>
              </span>
              {index === active ? <span className="launch-enter">↵</span> : null}
            </button>
          ))}
        </div>
      )}

      <p className="launch-foot">↑↓ 选择 · 回车执行 · Esc 清空</p>
    </div>
  );
}
