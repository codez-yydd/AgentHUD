/** 消息来源品牌图标：PNG 资源位于 assets/notify。
 *
 * 视觉规范：
 * 白色圆角底 + 1px 半透明描边，图标四边内缩约 9%。
 */
import qqIcon from "../assets/notify/qq.png";
import wechatIcon from "../assets/notify/wechat.png";
import mailIcon from "../assets/notify/mail.png";
import feishuIcon from "../assets/notify/feishu.png";
import dingtalkIcon from "../assets/notify/dingtalk.png";
import deepseekIcon from "../assets/notify/deepseek.png";

/** 消息模块显示顺序，与 Rust 的 NOTIFY_KINDS 一致。 */
export const NOTIFY_MODULE_KEYS = ["qq", "wechat", "mail", "feishu", "dingtalk", "dsh"] as const;

/** 通知来源 -> 品牌图。 */
export const NOTIFY_BRAND_ICONS: Record<string, string> = {
  qq: qqIcon,
  wechat: wechatIcon,
  mail: mailIcon,
  feishu: feishuIcon,
  dingtalk: dingtalkIcon,
  dsh: deepseekIcon,
};

/** 品牌中文名：未读 chip 的 title / aria-label 用。 */
export const NOTIFY_BRAND_LABELS: Record<string, string> = {
  qq: "QQ",
  wechat: "微信",
  mail: "邮件",
  feishu: "飞书",
  dingtalk: "钉钉",
  dsh: "DeepSeek",
};

export const KIND_COLORS: Record<string, string> = {
  qq: "#12b7f5",
  wechat: "#07c160",
  mail: "#ff9f0a",
  feishu: "#00c2a8",
  dingtalk: "#1e9bff",
  dsh: "#4d6bfe",
  stock: "#ff5a52",
  other: "#8e8e93",
};

/** 品牌图标 chip；size 默认 30（模块磁贴），历史行传 18。 */
export function NotifyBrandIcon({ kind, size = 30 }: { kind: string; size?: number }) {
  const src = NOTIFY_BRAND_ICONS[kind];
  if (!src) return null;
  return (
    <span className="msg-brand-chip" style={{ width: size, height: size }} aria-hidden="true">
      <img src={src} alt="" draggable={false} />
    </span>
  );
}
