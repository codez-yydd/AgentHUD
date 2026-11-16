/**
 * 页④「快捷操作」：全局热键状态（只读）+ 三个立即执行的动作。
 */
import { openMainWindow, quitApp, setIslandEnabled } from "../../../../island/api";
import { Button, Note, PageHeader, Row, Section, Tag } from "../controls";
import { EyeOffIcon, OpenIcon, PowerIcon } from "../icons";
import { useHotkeyStatus } from "../useHotkeyStatus";
import type { PageProps } from "./common";

/** 三个全局热键的展示名（与全局热键页一致） */
const HOTKEY_BADGES = [
  { key: "hotkeys.toggle", label: "展开 / 收起", status: "toggle" as const },
  { key: "hotkeys.translate", label: "翻译剪贴板", status: "translate" as const },
  { key: "hotkeys.clipboard", label: "打开剪贴板", status: "clipboard" as const },
];

/** 命令失败时的提示走页脚状态栏（dirty / 保存提示会被这条临时盖掉，属预期） */
function reportError(draft: PageProps["draft"], prefix: string, error: unknown) {
  const reason = typeof error === "string" ? error : error instanceof Error ? error.message : String(error);
  draft.setStatus(`${prefix}：${reason}`, "danger", true);
}

export function ActionsPage({ draft, saveTick }: PageProps) {
  const { status, loading } = useHotkeyStatus(saveTick);

  return (
    <>
      <PageHeader
        title="快捷操作"
        desc="全局热键状态与常用动作；这里的按钮立即执行，不受「保存并应用」影响。"
      />

      <Section title="全局热键">
        <Row
          wide
          title="注册状态"
          desc="配置里的热键是否已在系统注册成功；未生效通常是组合被占用或留空，换一个组合保存即可。"
        >
          <div className="wset-badges">
            {HOTKEY_BADGES.map((row) => {
              const value = String(draft.value(row.key) ?? "").trim();
              if (loading) {
                return (
                  <Tag key={row.key} tone="hint">
                    {row.label} · 正在检查…
                  </Tag>
                );
              }
              if (!value) {
                return (
                  <Tag key={row.key} tone="hint">
                    {row.label} · 未启用
                  </Tag>
                );
              }
              if (!status) {
                return (
                  <Tag key={row.key} tone="hint">
                    {row.label} · 状态未知
                  </Tag>
                );
              }
              return (
                <Tag key={row.key} tone={status[row.status] ? "ok" : "danger"}>
                  {row.label} · {status[row.status] ? "已注册生效" : "未生效"}
                </Tag>
              );
            })}
          </div>
        </Row>
        <Note>热键本身在「全局热键」页修改；这里只读显示当前是否生效。</Note>
      </Section>

      <Section title="常用动作">
        <Row
          title="打开主窗口"
          desc="打开用量与额度管理大窗（用量总览 / 请求日志 / 价格设置等）。"
        >
          <Button
            icon={<OpenIcon size={12} />}
            onClick={() => {
              void openMainWindow().catch((error) =>
                reportError(draft, "打开主窗口失败", error)
              );
            }}
          >
            打开主窗口
          </Button>
        </Row>
        <Row
          title="关闭灵动岛"
          desc="从屏幕上收起灵动岛（等同于托盘 / 右键菜单里的关闭），本设置窗也会一并收起。"
        >
          <Button
            icon={<EyeOffIcon size={12} />}
            onClick={() => {
              // 成功时 Rust 会随即隐藏本窗，提示来不及被看到，因此只在失败时报错
              void setIslandEnabled(false).catch((error) =>
                reportError(draft, "关闭灵动岛失败", error)
              );
            }}
          >
            关闭灵动岛
          </Button>
        </Row>
        <Row
          title="退出 AgentHUD"
          desc="完全退出应用（含灵动岛、托盘与后台采集）；之后需重新启动才能恢复。"
        >
          <Button
            variant="danger"
            icon={<PowerIcon size={12} />}
            onClick={() => {
              void quitApp().catch((error) => reportError(draft, "退出失败", error));
            }}
          >
            退出 AgentHUD
          </Button>
        </Row>
      </Section>

      <p className="wset-footnote">
        配置保存在 <b>settings.json</b>（{draft.path || "读取中…"}），数据目录{" "}
        {draft.dataDir || "读取中…"}。这里的按钮立即执行，不受草稿影响。
      </p>
    </>
  );
}
