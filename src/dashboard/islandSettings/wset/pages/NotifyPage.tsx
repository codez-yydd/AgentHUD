/**
 * 页⑥「消息与邮件」：消息模块 / 正文预览、DeepSeek 桌面版读取、桌面弹窗
 */
import { NOTIFY_BRAND_LABELS } from "../../../../island/notifyBrand";
import {
  Chips,
  Note,
  PageHeader,
  PageResetBar,
  RecordEditor,
  Row,
  Section,
  Select,
  Stepper,
  Switch,
  TextInput,
} from "../controls";
import {
  NOTIFY_BODY_KEYS,
  NOTIFY_MODULE_KEYS,
  PAGE_FIELDS,
  type PageKey,
} from "../schema";
import {
  browsePath,
  readFlag,
  readNumber,
  readRecords,
  readStringList,
  readText,
  UnsupportedTag,
  type PageProps,
} from "./common";

const PAGE: PageKey = "notify";

const MODULE_OPTIONS = NOTIFY_MODULE_KEYS.map((value) => ({
  value,
  label: NOTIFY_BRAND_LABELS[value],
}));

/** 正文预览只对读 toast 的来源有意义 */
const BODY_OPTIONS = MODULE_OPTIONS.filter((item) =>
  NOTIFY_BODY_KEYS.includes(item.value as (typeof NOTIFY_BODY_KEYS)[number])
);

const POPUP_OPTIONS = [
  { value: "center_bottom", label: "中心底部" },
  { value: "center_top", label: "中心顶部" },
  { value: "bottom_left", label: "左下角" },
  { value: "bottom_right", label: "右下角" },
  { value: "top_left", label: "左上角" },
  { value: "top_right", label: "右上角" },
];

/** 「打开应用」六行：点路径 / 标题 / 占位顺序固定 */
const OPEN_FIELDS = [
  { key: "notify.open.qq", label: "QQ" },
  { key: "notify.open.wechat", label: "微信" },
  { key: "notify.open.mail", label: "邮件" },
  { key: "notify.open.feishu", label: "飞书" },
  { key: "notify.open.dingtalk", label: "钉钉" },
  { key: "notify.open.dsh", label: "DeepSeek" },
];

const IMAP_FIELDS = [
  { key: "host", placeholder: "服务器，如 imap.qq.com", stretch: 2 },
  { key: "port", kind: "int" as const, width: 78, default: 993 },
  { key: "user", placeholder: "用户名 / 邮箱", stretch: 2 },
  { key: "password", secret: true, placeholder: "授权码", stretch: 2 },
  { key: "enabled", kind: "switch" as const, default: true },
];

const IMAP_NEW_ITEM = { host: "", port: 993, user: "", password: "", enabled: true };
const DEFAULT_DSH_SESSIONS_DIR = "%USERPROFILE%\\.dsh\\sessions";

export function NotifyPage({ draft }: PageProps) {
  const modules = readStringList(draft, "notify.modules");
  const bodyPreview = readStringList(draft, "notify.body_preview");

  return (
    <>
      <PageHeader
        title="消息与邮件"
        desc="消息页显示的模块、提醒正文、桌面弹窗，以及邮件未读检查。"
      />

      <Section title="消息模块">
        <Row
          wide
          title="启用的模块"
          desc="消息页显示的模块；也可以在消息页点 + 添加。"
          error={draft.errors["notify.modules"]}
          tag={<UnsupportedTag draft={draft} keyName="notify.modules" />}
          rowKey="notify.modules"
        >
          <Chips
            options={MODULE_OPTIONS}
            value={modules}
            ariaLabel="启用的消息模块"
            onChange={(next) => draft.setValue("notify.modules", next)}
          />
        </Row>
        <Row
          wide
          title="显示正文预览"
          desc="飞书 / 钉钉可以从右下角通知浮窗读取发送人与正文；其他应用只有写进 Windows 通知中心才有正文。"
          error={draft.errors["notify.body_preview"]}
          tag={<UnsupportedTag draft={draft} keyName="notify.body_preview" />}
          rowKey="notify.body_preview"
        >
          <Chips
            options={BODY_OPTIONS}
            value={bodyPreview}
            ariaLabel="显示正文预览的来源"
            onChange={(next) => draft.setValue("notify.body_preview", next)}
          />
        </Row>
      </Section>

      <Section title="DeepSeek 桌面版">
        <Row
          title="读取 DeepSeek 桌面版完成通知"
          desc="读取 DSH 的本地会话日志，一轮回答完成、失败或被截断时提醒。请同时启用上方的「DeepSeek」消息模块。"
          tag={<UnsupportedTag draft={draft} keyName="dsh.enabled" />}
          rowKey="dsh.enabled"
        >
          <Switch
            checked={readFlag(draft, "dsh.enabled", true)}
            ariaLabel="读取 DeepSeek 桌面版完成通知"
            onChange={(next) => draft.setValue("dsh.enabled", next)}
          />
        </Row>
        <Row
          title="同时提醒子任务"
          desc="子代理会话（delegationDepth > 0）也提醒；默认只看主会话。"
          tag={<UnsupportedTag draft={draft} keyName="dsh.include_subagents" />}
          rowKey="dsh.include_subagents"
        >
          <Switch
            checked={readFlag(draft, "dsh.include_subagents")}
            ariaLabel="同时提醒子任务"
            onChange={(next) => draft.setValue("dsh.include_subagents", next)}
          />
        </Row>
        <Row
          title="DeepSeek 在前台时不弹窗"
          desc="Windows 上，DeepSeek 窗口在前台时仍会记录消息。"
          tag={<UnsupportedTag draft={draft} keyName="dsh.skip_when_active" />}
          rowKey="dsh.skip_when_active"
        >
          <Switch
            checked={readFlag(draft, "dsh.skip_when_active", true)}
            ariaLabel="DeepSeek 在前台时不弹窗"
            onChange={(next) => draft.setValue("dsh.skip_when_active", next)}
          />
        </Row>
        <Row
          title="正文预览长度"
          desc="设置消息列表和弹窗中可显示的回答字数。"
          error={draft.errors["dsh.body_chars"]}
          tag={<UnsupportedTag draft={draft} keyName="dsh.body_chars" />}
          rowKey="dsh.body_chars"
        >
          <Stepper
            value={readNumber(draft, "dsh.body_chars", 120)}
            min={40}
            max={240}
            step={10}
            suffix="字"
            ariaLabel="正文预览长度"
            onChange={(next) => draft.setValue("dsh.body_chars", next)}
          />
        </Row>
        <Row
          title="会话目录"
          desc={`留空时读取 ${DEFAULT_DSH_SESSIONS_DIR}。`}
          tag={<UnsupportedTag draft={draft} keyName="dsh.sessions_dir" />}
          rowKey="dsh.sessions_dir"
        >
          <TextInput
            value={readText(draft, "dsh.sessions_dir")}
            width={280}
            placeholder={DEFAULT_DSH_SESSIONS_DIR}
            ariaLabel="会话目录"
            onBrowse={() => void browsePath(draft, "dsh.sessions_dir", { directory: true })}
            browseTitle="选择会话目录"
            onChange={(next) => draft.setValue("dsh.sessions_dir", next)}
          />
        </Row>
        <Note>
          只读会话日志，不会修改 dsh-pet 或任何其它应用的设置；如果同时装了 dsh-pet
          并开着系统通知，两边会各提醒一次。
        </Note>
      </Section>

      <Section title="桌面弹窗">
        <Row
          title="新消息弹出提示框"
          desc="在屏幕指定位置显示圆角提示框，淡入淡出；每条消息独立显示 3 秒，多条会按位置堆叠；鼠标移入暂停倒计时，点击提示框打开对应应用并切到前台。"
          tag={<UnsupportedTag draft={draft} keyName="notify.popup.enabled" />}
          rowKey="notify.popup.enabled"
        >
          <Switch
            checked={readFlag(draft, "notify.popup.enabled", true)}
            ariaLabel="新消息弹出提示框"
            onChange={(next) => draft.setValue("notify.popup.enabled", next)}
          />
        </Row>
        <Row
          title="弹出位置"
          desc="多条消息会从边缘向中间排列。"
          error={draft.errors["notify.popup.position"]}
          tag={<UnsupportedTag draft={draft} keyName="notify.popup.position" />}
          rowKey="notify.popup.position"
        >
          <Select
            value={readText(draft, "notify.popup.position", "bottom_right")}
            width={165}
            options={POPUP_OPTIONS}
            ariaLabel="弹出位置"
            onChange={(next) => draft.setValue("notify.popup.position", next)}
          />
        </Row>
        <Row
          title="弹窗大小"
          desc="100% 为当前默认尺寸（约 360 × 84）；可调 70%–160%。"
          error={draft.errors["notify.popup.size"]}
          tag={<UnsupportedTag draft={draft} keyName="notify.popup.size" />}
          rowKey="notify.popup.size"
        >
          <Stepper
            value={readNumber(draft, "notify.popup.size", 100)}
            min={70}
            max={160}
            step={5}
            suffix="%"
            ariaLabel="弹窗大小"
            onChange={(next) => draft.setValue("notify.popup.size", next)}
          />
        </Row>
      </Section>

      <Section title="打开应用">
        {OPEN_FIELDS.map((row) => (
          <Row
            key={row.key}
            title={row.label}
            tag={<UnsupportedTag draft={draft} keyName={row.key} />}
            rowKey={row.key}
          >
            <TextInput
              value={readText(draft, row.key)}
              width={280}
              placeholder="程序路径（留空按消息来源打开）"
              ariaLabel={`${row.label}程序路径`}
              onBrowse={() => void browsePath(draft, row.key, { title: `选择 ${row.label}` })}
              browseTitle="选择程序"
              onChange={(next) => draft.setValue(row.key, next)}
            />
          </Row>
        ))}
        <Note>
          每次启动 AgentHUD（含开机自启）都会自动探测本机程序路径，填入从未设置过的空项；手动保存过（包括清空）的项不会再被自动覆盖。留空时按消息来源打开。
        </Note>
      </Section>

      <Section title="邮件检查">
        <Row
          title="检查间隔"
          desc="定期检查 IMAP 收件箱中的新未读邮件。首次成功检查建立基线，之后发现新未读邮件时通知。"
          error={draft.errors["mail.interval_s"]}
          tag={<UnsupportedTag draft={draft} keyName="mail.interval_s" />}
          rowKey="mail.interval_s"
        >
          <Stepper
            value={readNumber(draft, "mail.interval_s", 120)}
            min={30}
            max={3600}
            step={30}
            suffix="秒"
            ariaLabel="邮件检查间隔"
            onChange={(next) => draft.setValue("mail.interval_s", next)}
          />
        </Row>
        <Row
          wide
          title="IMAP 账户"
          desc="填写 IMAP 服务器、端口（默认 993）、用户名和授权码或应用专用密码，开启账户后保存并应用。空账户列表可从 Thunderbird 自动填入服务器和用户名。"
          error={draft.errors["mail.imap"]}
          tag={<UnsupportedTag draft={draft} keyName="mail.imap" />}
          rowKey="mail.imap"
        >
          <RecordEditor
            items={readRecords(draft, "mail.imap")}
            fields={IMAP_FIELDS}
            newItem={IMAP_NEW_ITEM}
            addLabel="添加账户"
            emptyLabel="还没有 IMAP 账户。"
            onChange={(next) => draft.setValue("mail.imap", next)}
          />
        </Row>
        <Note>
          IMAP 通过 SSL/TLS 检查新增未读数量。Thunderbird 运行时，其中已配置的同一账号使用 Thunderbird 的系统提醒。Windows「邮件」和新版 Outlook 的系统通知可在 Windows 设置中单独开启。
        </Note>
      </Section>

      <PageResetBar onReset={() => draft.resetPage(PAGE_FIELDS[PAGE])} />
    </>
  );
}
