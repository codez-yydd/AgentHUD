/**
 * 页③「全局热键」：三个可录制的组合键 + 格式说明 + 注册状态徽标。
 */
import { HotkeyField, Note, PageHeader, PageResetBar, Row, Section, Tag } from "../controls";
import { PAGE_FIELDS } from "../schema";
import { useHotkeyStatus } from "../useHotkeyStatus";
import { UnsupportedTag, type PageProps } from "./common";

/** 三个热键的展示名（顺序 = 行顺序 = 徽标顺序） */
const HOTKEY_ROWS = [
  {
    key: "hotkeys.toggle",
    title: "展开 / 收起",
    desc: "例如 Ctrl+Alt+I",
    status: "toggle" as const,
  },
  {
    key: "hotkeys.translate",
    title: "翻译剪贴板",
    desc: "读取剪贴板文字并翻译",
    status: "translate" as const,
  },
  {
    key: "hotkeys.clipboard",
    title: "打开剪贴板",
    desc: "展开灵动岛并切到剪贴板页",
    status: "clipboard" as const,
  },
];

export function HotkeysPage({ draft, saveTick, onRecordingChange }: PageProps) {
  const { status, loading } = useHotkeyStatus(saveTick);

  return (
    <>
      <PageHeader
        title="全局热键"
        desc="在任何窗口下都能触发的快捷键。修改后立即重新注册。"
      />

      <Section title="快捷键">
        {HOTKEY_ROWS.map((row) => (
          <Row
            key={row.key}
            title={row.title}
            desc={row.desc}
            error={draft.errors[row.key]}
            tag={<UnsupportedTag draft={draft} keyName={row.key} />}
            rowKey={row.key}
          >
            <HotkeyField
              value={String(draft.value(row.key) ?? "")}
              ariaLabel={row.title}
              onRecordingChange={onRecordingChange}
              onChange={(next) => draft.setValue(row.key, next)}
            />
          </Row>
        ))}
        <Note>
          需要至少一个 Ctrl / Alt / Shift / Win 修饰键（F1–F24 可以单独使用）。
          留空表示不启用；热键被其他软件占用时会注册失败，换一个组合即可。
        </Note>
      </Section>

      <Section title="注册状态">
        <Row
          wide
          title="系统注册"
          desc="保存后由系统重新注册；未生效时换一个组合再保存，配置本身已经写入 settings.json。"
        >
          <div className="wset-badges">
            {HOTKEY_ROWS.map((row) => {
              const value = String(draft.value(row.key) ?? "").trim();
              const state = loading ? "loading" : !status ? "unknown" : status[row.status] ? "ok" : "warn";
              if (state === "loading") {
                return (
                  <Tag key={row.key} tone="hint">
                    {row.title} · 正在检查…
                  </Tag>
                );
              }
              if (!value) {
                return (
                  <Tag key={row.key} tone="hint">
                    {row.title} · 未启用
                  </Tag>
                );
              }
              if (state === "unknown") {
                return (
                  <Tag key={row.key} tone="hint">
                    {row.title} · 状态未知
                  </Tag>
                );
              }
              return (
                <Tag key={row.key} tone={state === "ok" ? "ok" : "danger"}>
                  {row.title} · {state === "ok" ? "已注册生效" : "未生效"}
                </Tag>
              );
            })}
          </div>
        </Row>
      </Section>

      <PageResetBar onReset={() => draft.resetPage(PAGE_FIELDS.hotkeys)} />
    </>
  );
}
