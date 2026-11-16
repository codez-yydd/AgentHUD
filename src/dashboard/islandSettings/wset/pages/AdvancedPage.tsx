/**
 * 页⑬「高级」：配置文件位置、重新加载、导入 / 导出、恢复所有默认值。
 */
import { useState } from "react";
import { Button, Confirm, Note, PageHeader, Row, Section } from "../controls";
import { OpenIcon } from "../icons";
import {
  IS_TAURI,
  exportSettings,
  importSettings,
  openSettingsFile,
  resetSettings,
  revealSettingsFile,
} from "../settingsApi";
import type { Flat } from "../useSettingsDraft";
import { errorText, type PageProps } from "./common";

/** 只取文件名（状态栏 / 确认框里显示，不暴露完整路径） */
function fileName(path: string): string {
  const parts = path.split(/[\\/]/);
  return parts[parts.length - 1] || path;
}

/** 扁平点路径 → 嵌套对象（浏览器预览导出 JSON 时用，与后端文档同形） */
function unflatten(flat: Flat): Record<string, unknown> {
  const root: Record<string, unknown> = {};
  for (const [key, value] of Object.entries(flat)) {
    const parts = key.split(".");
    let cursor: Record<string, unknown> = root;
    for (const part of parts.slice(0, -1)) {
      const next = cursor[part];
      if (!next || typeof next !== "object" || Array.isArray(next)) cursor[part] = {};
      cursor = cursor[part] as Record<string, unknown>;
    }
    cursor[parts[parts.length - 1]] = value;
  }
  return root;
}

/** 浏览器预览的导出兜底：把当前草稿写成 AgentHUD-settings.json 下载 */
function downloadPreview(flat: Flat): void {
  try {
    const blob = new Blob([JSON.stringify(unflatten(flat), null, 2)], {
      type: "application/json",
    });
    const url = URL.createObjectURL(blob);
    const anchor = document.createElement("a");
    anchor.href = url;
    anchor.download = "AgentHUD-settings.json";
    anchor.rel = "noopener";
    document.body.appendChild(anchor);
    anchor.click();
    anchor.remove();
    window.setTimeout(() => URL.revokeObjectURL(url), 1000);
    console.info("[island mock] export settings (preview download)", "AgentHUD-settings.json");
  } catch (error) {
    console.info("[island mock] export settings (fallback)", error);
  }
}

export function AdvancedPage({ draft }: PageProps) {
  const [confirmReload, setConfirmReload] = useState(false);
  const [confirmReset, setConfirmReset] = useState(false);
  const [pendingImport, setPendingImport] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  /** 动作统一包一层：busy 期间禁用按钮，失败写中文状态 */
  const run = async (prefix: string, action: () => Promise<void>) => {
    setBusy(true);
    try {
      await action();
    } catch (error) {
      draft.setStatus(`${prefix}：${errorText(error)}`, "danger", true);
    } finally {
      setBusy(false);
    }
  };

  const doReload = () => {
    void run("重新加载失败", async () => {
      await draft.reload();
      draft.setStatus("已从磁盘重新加载", "ok", true);
    });
  };

  const requestReload = () => {
    if (draft.dirtyCount > 0) {
      setConfirmReload(true);
      return;
    }
    doReload();
  };

  const doExport = () => {
    void run("导出失败", async () => {
      // 先校验：有错的项导出出去也加载不回来，跳转到首个错误行（SettingsWindow 负责）
      const found = draft.validate();
      if (found.length) {
        const more = found.length > 1 ? `，另外还有 ${found.length - 1} 项` : "";
        draft.setStatus(`${found[0].message}${more}`, "danger", true);
        return;
      }
      if (!IS_TAURI) {
        downloadPreview(draft.draft);
        draft.setStatus("已导出（预览）", "ok", true);
        return;
      }
      const { save } = await import("@tauri-apps/plugin-dialog");
      const path = await save({
        defaultPath: "AgentHUD-settings.json",
        filters: [{ name: "JSON 文件", extensions: ["json"] }],
      });
      if (!path) return;
      await exportSettings(path);
      draft.setStatus(`已导出到 ${fileName(path)}`, "ok", true);
    });
  };

  const doImport = (path: string) => {
    void run("导入失败", async () => {
      await importSettings(path);
      await draft.reload();
      draft.setStatus(`已导入 ${fileName(path)}`, "ok", true);
    });
  };

  const requestImport = () => {
    void run("导入失败", async () => {
      if (!IS_TAURI) {
        draft.setStatus("预览模式不支持导入", "hint", true);
        return;
      }
      const { open } = await import("@tauri-apps/plugin-dialog");
      const picked = await open({
        multiple: false,
        directory: false,
        filters: [{ name: "JSON 文件", extensions: ["json"] }],
      });
      if (typeof picked === "string") setPendingImport(picked);
    });
  };

  const doReset = () => {
    void run("恢复默认失败", async () => {
      await resetSettings();
      await draft.reload();
      draft.setStatus("已恢复所有默认值", "ok", true);
    });
  };

  return (
    <>
      <PageHeader title="高级" desc="配置文件的位置、导入导出，以及恢复默认设置。" />

      <Section title="配置文件">
        <Row title="settings.json" desc={draft.path || "读取中…"} rowKey="advanced.path">
          <div className="wset-rowbtns">
            <Button
              icon={<OpenIcon size={12} />}
              disabled={busy}
              onClick={() => {
                void run("打开配置文件失败", () => openSettingsFile());
              }}
            >
              打开
            </Button>
            <Button
              disabled={busy}
              onClick={() => {
                void run("定位配置文件失败", () => revealSettingsFile());
              }}
            >
              在文件夹中显示
            </Button>
          </div>
        </Row>
        <Row
          title="重新加载"
          desc="从磁盘重新读取 settings.json 并应用到灵动岛；未保存的修改会丢失。"
          rowKey="advanced.reload"
        >
          <Button disabled={busy} onClick={requestReload}>
            重新加载
          </Button>
        </Row>
      </Section>

      <Section title="导入 / 导出">
        <Row
          title="导出配置"
          desc="把当前所有设置（含尚未保存的修改）写成一份 JSON 备份。"
          rowKey="advanced.export"
        >
          <Button disabled={busy} onClick={doExport}>
            导出…
          </Button>
        </Row>
        <Row
          title="导入配置"
          desc="用一份 JSON 配置覆盖当前设置；缺的项用默认值补齐。"
          rowKey="advanced.import"
        >
          <Button disabled={busy} onClick={requestImport}>
            导入…
          </Button>
        </Row>
      </Section>

      <Section title="恢复默认">
        <Row
          title="恢复所有默认值"
          desc="主题、热键、自选股、启动器、API Key 等全部恢复为出厂默认，然后立即保存。"
          rowKey="advanced.reset"
        >
          <Button variant="danger" disabled={busy} onClick={() => setConfirmReset(true)}>
            恢复默认…
          </Button>
        </Row>
      </Section>

      <Note>
        少数高级字段（notify.toast_apps、notify.flash_processes、apps.restore_hotkey、stocks.alerts）
        仍然需要在 settings.json 里编辑，可以点上面的「打开」。
      </Note>
      <Note>数据目录：{draft.dataDir || "读取中…"}</Note>

      <Confirm
        open={confirmReload}
        title="放弃未保存的修改？"
        body={`重新加载会用磁盘上的 settings.json 覆盖 ${draft.dirtyCount} 项未保存的修改。`}
        okText="重新加载"
        danger
        onOk={() => {
          setConfirmReload(false);
          doReload();
        }}
        onCancel={() => setConfirmReload(false)}
      />

      <Confirm
        open={pendingImport !== null}
        title="导入配置"
        body={`用 ${pendingImport ? fileName(pendingImport) : ""} 覆盖当前配置？`}
        okText="导入"
        onOk={() => {
          const path = pendingImport;
          setPendingImport(null);
          if (path) doImport(path);
        }}
        onCancel={() => setPendingImport(null)}
      />

      <Confirm
        open={confirmReset}
        title="恢复所有默认值？"
        body="主题、热键、自选股、启动器、API Key 等都会清空并恢复为出厂设置。"
        okText="恢复默认"
        danger
        onOk={() => {
          setConfirmReset(false);
          doReset();
        }}
        onCancel={() => setConfirmReset(false)}
      />
    </>
  );
}
