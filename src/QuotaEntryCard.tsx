/**
 * 单条凭证的配额卡（共享组件）：从 GenericQuotaPanel 提取，供通用额度
 * 面板与 Claude 面板的「其他账号」区两处复用，避免组件复制。
 * 渲染结构：备注名 + 套餐/状态徽章 + 窗口进度条 + 按量余额 + 错误信息。
 *
 * 额度窗口统一按「剩余」口径展示（剩 X% + 剩余长度的条 + 按剩余着色），
 * 与 Z.ai / Codex / Claude / Cursor 及汇总页的额度条完全一致；绝对值另起
 * 一行标「已用 N / 总量」。详见 QuotaWindowBar 的注释。
 */
import { useState } from "react";
import type { MessageKey } from "./i18n";
import { useI18n } from "./i18n";
import { useDataCache } from "./DataCache";
import type { ProviderQuotaEntry } from "./types";
import { resetProviderCredentials } from "./api";
import { notifyCredentialsChanged } from "./agentVisibility";
import { ProgressBar, remainingGradient, remainingTextColor } from "./widgets";
import { formatCountdownCore, formatResetStamp } from "./format";

/** 凭证状态 → 徽章文案与配色（ok 为正常态，不渲染徽章） */
function statusBadge(
  status: ProviderQuotaEntry["status"]
): { cls: string; key: MessageKey } | null {
  switch (status) {
    case "expired":
      return { cls: "bg-amber-500/12 text-amber-700", key: "credentials.entryExpired" };
    case "error":
      return { cls: "bg-rose-500/12 text-rose-600", key: "credentials.entryError" };
    case "pending":
      return { cls: "bg-slate-900/8 text-slate-600", key: "credentials.entryPending" };
    default:
      return null;
  }
}

/**
 * 额度窗口 key → i18n 标题键映射：Rust 各 provider 模块下发的 window.title
 * 为硬编码中文，英文界面会混入中文，渲染时按 key 优先取 i18n 文案。
 * 用 Record 收窄动态键类型（MessageKey 为静态联合类型，不能模板字符串直传
 * t()）；未收录的 key（新增 provider / 新窗口形态）回落 Rust 下发的 title，
 * 保证不渲染空标题。key 清单 = 全仓 Rust 侧 ProviderQuotaWindow 的 key 参数。
 */
const WINDOW_TITLE_KEYS: Record<string, MessageKey> = {
  hour5: "credentials.windowTitle.hour5",
  weekly: "credentials.windowTitle.weekly",
  monthly: "credentials.windowTitle.monthly",
  interval: "credentials.windowTitle.interval",
  credits: "credentials.windowTitle.credits",
  quota: "credentials.windowTitle.quota",
  fuel: "credentials.windowTitle.fuel",
  sub_credits: "credentials.windowTitle.sub_credits",
  topup_credits: "credentials.windowTitle.topup_credits",
  opus_weekly: "credentials.windowTitle.opus_weekly",
  sonnet_weekly: "credentials.windowTitle.sonnet_weekly",
  extra_usage: "credentials.windowTitle.extra_usage",
  pro: "credentials.windowTitle.pro",
  flash: "credentials.windowTitle.flash",
  auto: "credentials.windowTitle.auto",
  api: "credentials.windowTitle.api",
};

/** 单条凭证的配额卡：备注名 + 套餐/状态徽章 + 窗口进度条 + 余额 + 错误信息 */
export function QuotaEntryCard({
  entry,
  accent,
  resetDisplay,
  now,
}: {
  entry: ProviderQuotaEntry;
  accent: string;
  resetDisplay: { countdown: boolean; datetime: boolean };
  now: number;
}) {
  const { t } = useI18n();
  const badge = statusBadge(entry.status);
  const isError = entry.status === "error" || entry.status === "expired";
  // 状态点语义与徽章一致：error 红 / expired amber，正常态保持品牌色
  const dotColor =
    entry.status === "error"
      ? "#f43f5e"
      : entry.status === "expired"
        ? "#f59e0b"
        : accent;
  // error/expired 时错误行已是主要信息，更新时间降级为 title 属性
  const updatedAtTitle = t("credentials.updatedAt", {
    time: formatResetStamp(entry.updatedAt),
  });

  return (
    <div className="card-base rounded-2xl px-3 py-2.5">
      {/* 标题行：备注名 + 套餐徽章 + 状态徽章 + 更新时间 */}
      <div
        className="flex items-center justify-between gap-2 mb-1.5"
        title={isError ? updatedAtTitle : undefined}
      >
        <div className="flex items-center gap-1.5 min-w-0">
          <span
            className="w-1.5 h-1.5 rounded-full shrink-0"
            style={{ background: dotColor }}
          />
          <span className="text-[10px] font-medium text-slate-900/85 truncate">
            {entry.label}
          </span>
          {entry.planName && (
            <span
              className="shrink-0 px-1 py-px rounded text-[8px] font-semibold"
              style={{ background: `${accent}1f`, color: accent }}
            >
              {entry.planName}
            </span>
          )}
          {badge && (
            <span className={`shrink-0 px-1.5 py-px rounded-full text-[8px] font-medium ${badge.cls}`}>
              {t(badge.key)}
            </span>
          )}
        </div>
        {!isError && (
          <span className="num text-[9px] text-slate-500/70 shrink-0">
            {formatResetStamp(entry.updatedAt)}
          </span>
        )}
      </div>

      {/* 错误 / 过期：展示原因行 */}
      {isError && entry.message && (
        <p
          className="text-[10px] text-rose-600/90 leading-relaxed mb-1.5 break-all"
          title={entry.message}
        >
          ⚠ {entry.message}
        </p>
      )}

      {/* 正常态（ok / pending）的说明行：中性色，不当失败展示。
          例：本地估算条目的「本地用量估算，非官方额度数据」（非官方口径需要告知，
          否则用户会把估算值当官方额度）、本地登录条目的「当前账号没有生效的订阅」。
          这类 message 以前只在 error/expired 时渲染，等于静默丢弃了这些说明。 */}
      {!isError && entry.message && (
        <p
          className="text-[10px] text-slate-500 leading-relaxed mb-1.5 break-all"
          title={entry.message}
        >
          {entry.message}
        </p>
      )}

      {/* 用量窗口（标题优先取 key 映射的 i18n 文案，未收录回落 Rust title） */}
      {entry.windows.length > 0 && (
        <div className="space-y-1.5">
          {entry.windows.map((w) => {
            const titleKey = WINDOW_TITLE_KEYS[w.key];
            return (
              <QuotaWindowBar
                key={w.key}
                title={titleKey ? t(titleKey) : w.title}
                usedPercent={w.usedPercent}
                used={w.used}
                total={w.total}
                unit={w.unit}
                resetsAt={w.resetsAt}
                resetDisplay={resetDisplay}
                now={now}
              />
            );
          })}
        </div>
      )}

      {/* 按量余额（充值型 provider） */}
      {entry.balance && (
        <div className="mt-1.5 pt-1.5 border-t border-slate-900/6 flex items-baseline gap-1.5 flex-wrap">
          <span className="text-[9px] text-slate-500">
            {t("credentials.balance")}
          </span>
          <span className="num text-[13px] font-semibold text-slate-900/85">
            {entry.balance.currency}
            {entry.balance.amount.toFixed(2)}
          </span>
          {(entry.balance.granted != null || entry.balance.toppedUp != null) && (
            <span className="num text-[9px] text-slate-500/80">
              {entry.balance.granted != null &&
                t("credentials.balanceGranted", {
                  amount: `${entry.balance.currency}${entry.balance.granted.toFixed(2)}`,
                })}
              {entry.balance.granted != null && entry.balance.toppedUp != null && " · "}
              {entry.balance.toppedUp != null &&
                t("credentials.balanceToppedUp", {
                  amount: `${entry.balance.currency}${entry.balance.toppedUp.toFixed(2)}`,
                })}
            </span>
          )}
        </div>
      )}
    </div>
  );
}

/**
 * 凭证文件损坏类错误的判定：Rust provider_credentials.rs 的 load_provider
 * 有两条同类错误前缀（均为硬编码中文、不受界面语言影响，includes 匹配可靠）：
 * - 「凭证文件解析失败（{provider}）: …」——合法 UTF-8 但 JSON 解析失败
 *   （典型：全 NUL 坏文件）；
 * - 「读取凭证文件失败（{provider}）: …」——非 UTF-8 乱码等读盘失败。
 * 两者都是「文件在但读不出」的同一故障形态，自愈入口相同，一并匹配。
 */
function isCredResettableError(message: string): boolean {
  return (
    message.includes("凭证文件解析失败") || message.includes("读取凭证文件失败")
  );
}

/**
 * provider 额度查询整体失败的公共错误卡（get_provider_quota 返回 Err）：
 * 与单条目 status="error"（QuotaEntryCard 内部渲染）不同，本卡表达的是
 * command 级失败——后端读不到任何条目（典型：凭证文件损坏，第一步读凭证
 * 就 Err）。从 DataCache 读错误记录，无错误渲染 null，面板可无条件插入。
 * 错误叠加时旧缓存条目照常渲染（保旧不丢数据，由面板负责），本卡只负责
 * 把失败本身告知用户，样式与 QuotaEntryCard 的 error 行保持一致
 * （⚠ + rose 文字 + 徽章）。
 *
 * 凭证文件损坏场景（isCredResettableError）内嵌「重置凭证文件」自愈入口
 * （二次确认后调用后端 reset_provider：删除坏文件并重建空骨架）。入口必须
 * 放在 banner 内而非引导用户去凭证管理区：坏文件时凭证列表永远读取失败
 * （DataCache 只在成功时写 credentials 缓存），凭证卡区块在多数面板整体
 * 不渲染，重置按钮不可达（2026-11-03 opencodego 事故的真实路径）。重置
 * 成功后广播 credentials-changed：DataCache 失效刷新该 provider 凭证列表
 * 并 force 补刷额度（查询成功即清除本错误卡），App 重查 presence 联动
 * tab 显隐——整个闭环复用既有事件机制，banner 内无需额外刷新调用。
 *
 * className：调用点用它与相邻卡片对齐间距（各面板卡片间距口径不一：
 * page-stack 容器自带 gap 无需外边距，其余面板传 mt-1.5 / mb-1.5），
 * 组件自身不带任何外边距。
 */
export function ProviderQuotaErrorBanner({
  provider,
  className = "",
}: {
  provider: string;
  /** 与相邻卡片对齐间距（如 mt-1.5 / mb-1.5），由调用点按面板口径传入 */
  className?: string;
}) {
  const { t } = useI18n();
  const { providerQuotaErrors } = useDataCache();
  const err = providerQuotaErrors[provider];
  // 重置流程本地态：二次确认 / 进行中 / 结果反馈。挂载在早退之前声明
  //（hooks 顺序稳定；无错误时这些态不会被用到）
  const [confirming, setConfirming] = useState(false);
  const [busy, setBusy] = useState(false);
  const [result, setResult] = useState<{ ok: boolean; text: string } | null>(
    null
  );
  if (!err) return null;
  const isCredParseError = isCredResettableError(err.message);

  // 重置凭证文件（后端删坏文件重建空骨架）：成功后广播凭证变更，刷新与
  // 额度补刷走 DataCache 的既有事件链路（见组件注释）；失败在按钮旁透出
  const handleReset = async () => {
    setBusy(true);
    setResult(null);
    try {
      await resetProviderCredentials(provider);
      setConfirming(false);
      notifyCredentialsChanged(provider);
      setResult({ ok: true, text: t("credentials.resetDone") });
    } catch (e) {
      setResult({
        ok: false,
        text: t("credentials.resetFail", { msg: String(e) }),
      });
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className={`card-base rounded-2xl px-3 py-2.5 relative ${className}`}>
      {/* 标题行：查询失败徽章（复用条目级文案与配色）+ 最近失败时刻 */}
      <div className="flex items-center justify-between gap-2 mb-1.5">
        <span className="shrink-0 px-1.5 py-px rounded-full text-[8px] font-medium bg-rose-500/12 text-rose-600">
          {t("credentials.entryError")}
        </span>
        <span className="num text-[9px] text-slate-500/70 shrink-0">
          {formatResetStamp(err.ts)}
        </span>
      </div>
      {/* 后端错误原文（与 QuotaEntryCard 的条目错误行同款样式） */}
      <p
        className="text-[10px] text-rose-600/90 leading-relaxed break-all"
        title={err.message}
      >
        ⚠ {err.message}
      </p>
      {/* 凭证文件损坏场景的自愈入口：后果说明 + 重置按钮（入口就在本卡内，
          不引导用户去凭证管理区——坏文件时那片区域整体不可达，见组件注释） */}
      {isCredParseError && (
        <>
          <p className="text-[10px] text-amber-700 leading-relaxed mt-1">
            {t("credentials.quotaCredResetHint")}
          </p>
          <div className="flex items-center gap-2 mt-1.5">
            <button
              onClick={() => {
                setResult(null);
                setConfirming(true);
              }}
              disabled={busy}
              className="text-[9px] px-2 py-0.5 rounded bg-red-500/10 text-red-600 hover:bg-red-500/20 transition-colors disabled:opacity-40 disabled:cursor-not-allowed"
            >
              {t("credentials.resetFile")}
            </button>
            {result && (
              <span
                className={`text-[9px] leading-relaxed break-all ${
                  result.ok ? "text-emerald-600" : "text-rose-600"
                }`}
              >
                {result.text}
              </span>
            )}
          </div>
          {/* 二次确认弹层（样式对齐 CredentialsCard 的 ConfirmResetOverlay；
              覆盖本卡而非全屏，busy 保持挂载） */}
          {confirming && (
            <div className="absolute inset-0 z-50 flex items-center justify-center bg-black/30 rounded-2xl">
              <div className="mx-3 w-full rounded-lg bg-elevated border border-slate-900/10 p-3 shadow-xl">
                <div className="text-[12px] font-semibold text-slate-900 mb-1">
                  {t("credentials.resetTitle")}
                </div>
                <p className="text-[10px] text-slate-700/65 leading-relaxed mb-2">
                  {t("credentials.resetConfirm")}
                </p>
                <div className="flex gap-1.5">
                  <button
                    onClick={() => setConfirming(false)}
                    disabled={busy}
                    className="flex-1 text-[11px] py-1 rounded-md bg-slate-900/5 text-slate-700/70 hover:bg-slate-900/10 transition-colors disabled:opacity-40 disabled:cursor-not-allowed"
                  >
                    {t("common.cancel")}
                  </button>
                  <button
                    onClick={() => void handleReset()}
                    disabled={busy}
                    className="flex-1 text-[11px] py-1 rounded-md bg-red-500 text-white hover:bg-red-600 transition-colors disabled:opacity-40 disabled:cursor-not-allowed"
                  >
                    {busy ? t("common.saving") : t("credentials.resetFile")}
                  </button>
                </div>
              </div>
            </div>
          )}
        </>
      )}
    </div>
  );
}

/** 单个用量窗口：标题 + 进度条 + 数值 + 重置倒计时（展示偏好跟随设置页）。
 *
 *  **口径统一（重要）**：与 Z.ai / Codex / Claude / Cursor 的额度条一致，
 *  一律按「剩余」展示——右侧数字是「剩 X%」，进度条长度 = 剩余百分比，
 *  配色也按剩余分段（remainingGradient / remainingTextColor）。
 *  后端 ProviderQuotaWindow.usedPercent 是**已用**百分比（各 provider 模块
 *  按同一口径产出，如 kimi 的 pct_from_limit_remaining、minimax 的
 *  100 - remaining_percent），故此处在渲染层取补数，不改后端契约。
 *  修复前这里直接把 usedPercent 当数字与条长展示（「70%」+ 70% 长的条），
 *  方向与其它面板相反：同一账号在 Kimi 主面板显示「剩 30%」、在凭证卡里
 *  却显示「70%」。
 *
 *  绝对值信息保留：有 used/total 时在条下给一行「已用 N / 总量」（与 Z.ai 的
 *  MCP 月度条同款版式），避免与右侧的「剩 X%」混淆。 */
function QuotaWindowBar({
  title,
  usedPercent,
  used,
  total,
  unit,
  resetsAt,
  resetDisplay,
  now,
}: {
  title: string;
  usedPercent?: number;
  used?: number;
  total?: number;
  unit?: string;
  resetsAt?: number;
  resetDisplay: { countdown: boolean; datetime: boolean };
  now: number;
}) {
  const { t } = useI18n();
  // 已用百分比（缺 usedPercent 时用 used/total 兜底）→ 取补数得剩余
  const usedPct =
    usedPercent ?? (total ? ((used ?? 0) / total) * 100 : undefined);
  const hasBar = usedPct != null && isFinite(usedPct);
  const remain = hasBar
    ? Math.max(0, 100 - Math.min(Math.max(usedPct, 0), 100))
    : 0;

  // 数值文本：有百分比 → 「剩 X%」（与其它面板同款）；无百分比只有绝对值时
  // 回落展示绝对值本身（此前行为）
  const numText = hasBar
    ? t("common.remaining", { pct: Math.round(remain) })
    : used != null
      ? unit
        ? `${used}${total != null ? `/${total}` : ""} ${unit}`
        : String(used)
      : "";
  // 绝对值行（已用口径；总量与单位来自后端，单位可能缺省）
  const absText =
    used != null && total != null
      ? t("credentials.windowUsedAbs", {
          amount: unit ? `${used} ${unit}` : String(used),
          total: String(total),
        })
      : "";

  return (
    <div>
      <div className="flex items-center justify-between gap-2 mb-0.5">
        <span className="text-[9px] text-slate-600/70">{title}</span>
        {numText && (
          <span
            className="num text-[10px] font-semibold shrink-0"
            style={{ color: remainingTextColor(remain) }}
          >
            {numText}
          </span>
        )}
      </div>
      {hasBar && (
        <ProgressBar pct={remain / 100} gradient={remainingGradient(remain)} />
      )}
      {/* 绝对值行：已用 / 总量（与右侧「剩 X%」互为补充，语义在文案里写明） */}
      {absText && (
        <div className="num text-[8px] text-slate-500/70 mt-0.5">{absText}</div>
      )}
      {/* 重置时间：倒计时 / 具体时间点（两项可同时开，跟随设置页偏好） */}
      {resetsAt != null && resetsAt > now && (
        <div className="num text-[8px] text-slate-500/70 mt-0.5">
          {resetDisplay.countdown &&
            t("common.refreshIn", {
              time: formatCountdownCore(resetsAt - now),
            })}
          {resetDisplay.countdown && resetDisplay.datetime && " · "}
          {resetDisplay.datetime && formatResetStamp(resetsAt)}
        </div>
      )}
    </div>
  );
}
