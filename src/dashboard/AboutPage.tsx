/**
 * 关于页（主窗口第十一页）：品牌信息 / 版本 / 检查更新 / 项目链接与许可。
 *
 * 软件更新区块已提取为共享组件（settings/UpdateSection.tsx，本页原样
 * 渲染、行为零变）：更新的执行全部委托面板（面板是唯一更新执行者：
 * Update 资源存于面板 webview 资源表，rid 在主窗口无效；主窗口值导入
 * updater.ts 还会清掉面板写入的红点标记）——组件只经跨窗口事件桥
 * （updaterEvents.ts，宿主见 updaterBridge.ts）订阅面板广播的状态、
 * 发送命令，设置页「关于与更新」分区也复用同一组件。链接行经 opener
 * 插件在本机浏览器打开（capability 见 src-tauri/capabilities/
 * dashboard.json 的 opener:default）。
 */
import { useEffect, useState } from "react";
import { getVersion } from "@tauri-apps/api/app";
import { openUrl } from "@tauri-apps/plugin-opener";
// 软件 logo：与 TitleBar.tsx 同源（Vite 打包为带哈希的资源 URL）
import logo from "../assets/logo.png";
import { useI18n, type MessageKey } from "../i18n";
import { ExternalLinkIcon } from "./icons";
import { UpdateSection } from "./settings/UpdateSection";
import { PageTitleWithDesc } from "./primitives";

/** 链接行：副文本用去掉协议的路径（同域名行可区分，且无 i18n 负担） */
const LINKS: { titleKey: MessageKey; url: string; sub: string }[] = [
  {
    titleKey: "dashboard.about.linkGitHub",
    url: "https://github.com/codez-yydd/AgentHUD",
    sub: "github.com/codez-yydd/AgentHUD",
  },
  {
    titleKey: "dashboard.about.linkGitee",
    url: "https://gitee.com/codezwx/AgentHUD",
    sub: "gitee.com/codezwx/AgentHUD",
  },
  {
    titleKey: "dashboard.about.linkReleases",
    url: "https://gitee.com/codezwx/AgentHUD/releases",
    sub: "gitee.com/codezwx/AgentHUD/releases",
  },
  {
    titleKey: "dashboard.about.linkIssues",
    url: "https://github.com/codez-yydd/AgentHUD/issues",
    sub: "github.com/codez-yydd/AgentHUD/issues",
  },
];

export function AboutPage() {
  const { t } = useI18n();
  // 当前版本（getVersion 失败显示 "—"，照面板 UpdaterCard 同口径）
  const [version, setVersion] = useState("");

  useEffect(() => {
    getVersion()
      .then(setVersion)
      .catch(() => setVersion(""));
  }, []);

  // 打开外链：opener 插件失败时兜底 window.open（照面板 UpdaterCard 先例）
  const openLink = (url: string) => {
    openUrl(url).catch(() => window.open(url, "_blank"));
  };

  return (
    <div className="db-page db-about-page">
      <header className="db-page__header">
        <PageTitleWithDesc title={t("dashboard.about")} desc={t("dashboard.about.desc")} />
      </header>

      {/* 品牌区：左对齐单行元信息（logo + 品牌名 + 版本胶囊）。标题栏已有
          常驻品牌区（logo + AgentHUD + 副标题），此处刻意收窄不重复副标题，
          版本号全页只在此胶囊出现（logo alt 留空——品牌名已在旁边可见
          文本里，照 TitleBar 的读屏口径） */}
      <div className="db-about-brand">
        <img
          src={logo}
          alt=""
          className="db-about-brand__logo"
          draggable={false}
        />
        <span className="db-about-brand__name">AgentHUD</span>
        <span className="db-about-version num">
          {version ? `v${version}` : "—"}
        </span>
      </div>

      {/* 更新区块：共享组件（settings/UpdateSection），DOM 与行为同提取前
          一致；状态语义与面板 UpdaterCard 对齐（执行在面板，本页只展示） */}
      <UpdateSection />

      {/* 链接与信息区：上半项目链接（整行可点，opener 打开），下半只读信息 */}
      <section className="db-card db-about-card">
        <h2 className="db-about-card__title">
          {t("dashboard.about.linksTitle")}
        </h2>
        <div className="db-about-list">
          {LINKS.map((link) => (
            <button
              key={link.url}
              type="button"
              className="db-about-link"
              onClick={() => openLink(link.url)}
            >
              <span className="db-about-link__text">
                <span className="db-about-link__title">
                  {t(link.titleKey)}
                </span>
                <span className="db-about-link__sub">{link.sub}</span>
              </span>
              <ExternalLinkIcon className="db-about-link__icon" />
            </button>
          ))}

          {/* 只读信息行：与链接行同构（无 hover、无图标） */}
          <div className="db-about-info">
            <span className="db-about-info__title">
              {t("dashboard.about.infoLicense")}
            </span>
            <span className="db-about-info__value">MIT License</span>
          </div>
          <div className="db-about-info">
            <span className="db-about-info__title">
              {t("dashboard.about.infoStack")}
            </span>
            <span className="db-about-info__value">
              Tauri 2 · Rust · React · TypeScript
            </span>
          </div>
          <div className="db-about-info">
            <span className="db-about-info__text">
              <span className="db-about-info__title">
                {t("dashboard.about.infoDataDir")}
              </span>
              <span className="db-about-info__hint">
                {t("dashboard.about.infoDataDirHint")}
              </span>
            </span>
            <span className="db-about-info__value num db-mono">~/.agenthud</span>
          </div>
        </div>
      </section>
    </div>
  );
}
