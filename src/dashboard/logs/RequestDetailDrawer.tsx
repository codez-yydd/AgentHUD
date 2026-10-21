/**
 * 请求详情抽屉：点击明细表任意一行后从右侧滑入（自实现 fixed 面板 + 遮罩，
 * 不引 UI 库），分四节——基本信息 / Token 明细 / 花费 / 性能。
 *
 * 交互：遮罩点击、右上「关闭」按钮、Esc 均可关闭（面板打开期间挂 window 级
 * Esc 监听，卸载即摘）；同时给 .db-main 加 --locked 类阻断背景滚动（面板
 * 自身的正文区仍可滚）。可访问语义：role="dialog" + aria-modal，关闭按钮
 * 自动聚焦，键盘用户可直接回车关闭。
 *
 * 配色只用 --db-* 变量（抽屉底 --db-drawer-bg、遮罩 --db-mask）：本项目
 * .dark 会整体反转 slate 色板，弹层/抽屉绝不能写 bg-slate-900 这类工具类。
 *
 * 数值口径：token 紧凑值沿用表格的 locale 格式（万/亿、K/M/B），精确整数放
 * title（Kv 的 title 参数）；首字延迟 2 位小数秒（百毫秒量级），耗时 1 位；
 * 耗时只显示来源库的真实值，缺失时与速度同为「—」（不显示估算态）。
 * 速度与表格同走 model.speedOf 两层结构（后端 token_speed 真源优先、本地
 * 公式回退），近似态带 ≈ 前缀并以 title 说明原因。
 */
import { useEffect } from "react";
import { useI18n } from "../../i18n";
import { formatCost, formatTokensLocale, formatTps } from "../../format";
import { costOf } from "../overview/derive";
import type { Currency, RequestLogRow } from "../../types";
import {
  durationText,
  exactCount,
  logTimeFull,
  sourceLabel,
  speedApproxHintKey,
  speedOf,
  ttftText,
} from "./model";

/** 一列键值（值单行截断 + title 给全文，长模型名 / 会话 id 不撑破面板；
 *  title 缺省用 value，token 等有紧凑格式的字段显式传精确整数） */
function Kv({
  label,
  value,
  title,
  mono,
}: {
  label: string;
  value: string;
  title?: string;
  mono?: boolean;
}) {
  return (
    <div className="db-drawer__kv">
      <span className="db-drawer__k">{label}</span>
      <span
        className={`db-drawer__v ${mono ? "num" : ""}`}
        title={title ?? value}
      >
        {value}
      </span>
    </div>
  );
}

export function RequestDetailDrawer({
  row,
  currency,
  deviceLabel,
  onClose,
}: {
  row: RequestLogRow;
  currency: Currency;
  /** 设备 id → 展示名（页面统一维护）：设备行显示「设备名 + 完整 id」 */
  deviceLabel: (deviceId: string) => string;
  onClose: () => void;
}) {
  const { locale, t } = useI18n();

  useEffect(() => {
    function onKey(e: KeyboardEvent) {
      if (e.key === "Escape") onClose();
    }
    document.addEventListener("keydown", onKey);
    return () => document.removeEventListener("keydown", onKey);
  }, [onClose]);

  // 打开期间阻断背景滚动：主区滚到底也不会把抽屉外的内容带着滚（body 本身
  // 在 dashboard.css 里已是 overflow: hidden，锁住 .db-main 即可）
  useEffect(() => {
    const main = document.querySelector(".db-main");
    if (!main) return;
    main.classList.add("db-main--locked");
    return () => main.classList.remove("db-main--locked");
  }, []);

  const speed = speedOf(row);
  const duration = durationText(row.duration_ms);
  const speedHintKey = speedApproxHintKey(speed?.approximate === true);
  const ttft = ttftText(row.ttft_ms);
  const mainCost = costOf(row.cost_usd, row.cost_cny, currency);
  const otherCurrency: Currency = currency === "cny" ? "usd" : "cny";
  const otherCost = costOf(row.cost_usd, row.cost_cny, otherCurrency);
  // token 明细：紧凑值 + 精确整数 title
  const tokens: { label: string; value: number }[] = [
    { label: t("dashboard.logs.detailInput"), value: row.input_tokens },
    { label: t("dashboard.logs.detailOutput"), value: row.output_tokens },
    { label: t("dashboard.logs.detailCacheRead"), value: row.cache_read_tokens },
    { label: t("dashboard.logs.detailCacheWrite"), value: row.cache_write_tokens },
    { label: t("dashboard.logs.detailReasoning"), value: row.reasoning_tokens },
    { label: t("dashboard.logs.detailTotal"), value: row.total_tokens },
  ];

  return (
    <>
      <div className="db-drawer-mask" onClick={onClose} aria-hidden />
      <aside
        className="db-drawer"
        role="dialog"
        aria-modal="true"
        aria-label={t("dashboard.logs.detailTitle")}
      >
        <header className="db-drawer__head">
          <div className="db-drawer__headline">
            <h2 className="db-drawer__title">{t("dashboard.logs.detailTitle")}</h2>
            {/* 完整时间只在头部副行展示一次（基本信息节不再重复一行时间） */}
            <span className="db-drawer__sub num">{logTimeFull(row.started_at)}</span>
          </div>
          <button
            type="button"
            className="db-icon-btn"
            onClick={onClose}
            aria-label={t("dashboard.logs.close")}
            title={t("dashboard.logs.close")}
            autoFocus
          >
            ✕
          </button>
        </header>

        <div className="db-drawer__body">
          {/* ---- 基本信息 ---- */}
          <section className="db-drawer__section">
            <h3 className="db-drawer__section-title">
              {t("dashboard.logs.detailBasic")}
            </h3>
            <Kv label={t("dashboard.logs.detailSource")} value={sourceLabel(row.source)} />
            {/* 设备归属：仅远端行有（远端口径从服务端明细带回 device_id，
                名称由页面的设备映射补上——「设备名（完整 id）」同行展示，
                超长截断、title 给全文）；本机行不显示该行 */}
            {row.device != null && row.device !== "" && (
              <Kv
                label={t("dashboard.logs.detailDevice")}
                value={t("common.deviceOption", {
                  name: deviceLabel(row.device),
                  id: row.device,
                })}
              />
            )}
            <Kv label={t("dashboard.logs.detailModel")} value={row.model_id || "—"} />
            <Kv
              label={t("dashboard.logs.detailProvider")}
              value={row.provider_id || "—"}
            />
            <Kv
              label={t("dashboard.logs.detailSession")}
              value={row.session_id ?? "—"}
              mono
            />
            {row.project != null && row.project !== "" && (
              <Kv label={t("dashboard.logs.detailProject")} value={row.project} />
            )}
          </section>

          {/* ---- Token 明细（紧凑值 + 精确整数 title） ---- */}
          <section className="db-drawer__section">
            <h3 className="db-drawer__section-title">
              {t("dashboard.logs.detailTokens")}
            </h3>
            {tokens.map((item) => (
              <Kv
                key={item.label}
                label={item.label}
                value={formatTokensLocale(item.value, locale)}
                title={exactCount(item.value, locale)}
                mono
              />
            ))}
          </section>

          {/* ---- 花费（本机币种主显、另一币种次显） ---- */}
          <section className="db-drawer__section">
            <h3 className="db-drawer__section-title">
              {t("dashboard.logs.detailCost")}
            </h3>
            {mainCost > 0 ? (
              <div className="db-drawer__money">
                <span className="db-drawer__money-main num">
                  {formatCost(mainCost, currency)}
                </span>
                <span className="db-drawer__money-sub num">
                  {formatCost(otherCost, otherCurrency)}
                </span>
              </div>
            ) : (
              <div className="db-drawer__money">
                <span className="db-drawer__money-main db-drawer__money-main--dim">
                  {row.total_tokens > 0
                    ? t("dashboard.logs.unpriced")
                    : "—"}
                </span>
                {row.total_tokens > 0 && (
                  <span className="db-drawer__money-sub">
                    {t("dashboard.logs.unpricedHint")}
                  </span>
                )}
              </div>
            )}
          </section>

          {/* ---- 性能（耗时 / 首字全缺失时整节省略） ---- */}
          {row.duration_ms != null || row.ttft_ms != null ? (
            <section className="db-drawer__section">
              <h3 className="db-drawer__section-title">
                {t("dashboard.logs.detailPerf")}
              </h3>
              <Kv
                label={t("dashboard.logs.detailDuration")}
                value={duration ?? "—"}
                mono
              />
              <Kv label={t("dashboard.logs.detailTtft")} value={ttft ?? "—"} mono />
              <Kv
                label={t("dashboard.logs.detailSpeed")}
                value={
                  speed
                    ? `${speed.approximate ? "≈" : ""}${formatTps(speed.tps)} t/s`
                    : "—"
                }
                title={speedHintKey ? t(speedHintKey) : undefined}
                mono
              />
            </section>
          ) : null}
        </div>
      </aside>
    </>
  );
}

