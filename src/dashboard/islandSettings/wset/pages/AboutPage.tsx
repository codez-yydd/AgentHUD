/**
 * 页⑭「关于」：应用卡片 + 当前版本 + 开源信息。
 */
import { useEffect, useState } from "react";
import { getVersion } from "@tauri-apps/api/app";
import { openUrl } from "@tauri-apps/plugin-opener";
import logoMark from "../../../../assets/logo-mark.png";
import { Button, PageHeader, Row, Section, Tag } from "../controls";
import type { PageProps } from "./common";

/** AgentHUD 仓库 / 反馈 / 协议地址（与主窗口「关于」页同源） */
const REPO_URL = "https://github.com/codez-yydd/AgentHUD";
const AUTHOR_URL = "https://github.com/codez-yydd";
const ISSUES_URL = `${REPO_URL}/issues`;
const LICENSE_URL = `${REPO_URL}/blob/main/LICENSE`;

/** 用系统浏览器打开链接：openUrl 失败（浏览器预览 / ACL）退化 window.open */
function openExternal(url: string) {
  void openUrl(url).catch(() => window.open(url, "_blank", "noopener,noreferrer"));
}

export function AboutPage(_props: PageProps) {
  const [version, setVersion] = useState("2.0.1");

  useEffect(() => {
    let alive = true;
    getVersion()
      .then((value) => {
        if (alive && value) setVersion(value);
      })
      .catch(() => {
        /* 浏览器预览 / 读取失败：回退到 package.json 的版本号，不再显示「—」 */
      });
    return () => {
      alive = false;
    };
  }, []);

  const versionLabel = `v${version}`;

  return (
    <>
      <PageHeader title="关于" desc="应用信息、当前版本与开源说明。" />

      <div className="wset-about-hero">
        <img className="wset-about-hero__logo" src={logoMark} alt="" draggable={false} />
        <div className="wset-about-hero__titles">
          <div className="wset-about-hero__name">AgentHUD</div>
          <div className="wset-about-hero__tagline">AI 用量与额度监控 · 灵动岛 HUD</div>
        </div>
        <Tag tone="accent">{versionLabel}</Tag>
      </div>

      <Section title="版本更新">
        <Row
          title="当前版本"
          desc="更新由主窗口「关于」页统一检查与安装；设置窗不重复持有更新器状态。"
        >
          <Tag tone="accent">{versionLabel}</Tag>
        </Row>
      </Section>

      <Section title="开源信息">
        <Row title="作者">
          <div className="wset-rowbtns">
            <Tag tone="hint">codez-yydd</Tag>
            <Button onClick={() => openExternal(AUTHOR_URL)}>打开主页</Button>
          </div>
        </Row>
        <Row title="仓库" desc={REPO_URL}>
          <Button onClick={() => openExternal(REPO_URL)}>打开仓库</Button>
        </Row>
        <Row title="Issues" desc={ISSUES_URL}>
          <Button onClick={() => openExternal(ISSUES_URL)}>打开 Issues</Button>
        </Row>
        <Row title="开源协议" desc="MIT License">
          <div className="wset-rowbtns">
            <Tag tone="hint">MIT License</Tag>
            <Button onClick={() => openExternal(LICENSE_URL)}>打开 MIT License</Button>
          </div>
        </Row>
      </Section>
    </>
  );
}
