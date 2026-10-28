/**
 * 简洁方案的核心卡片（悬浮面板汇总页）：当前速度 + Token 用量。
 * 只消费已取到的真实数据：没有速度样本时显示真实空态，不填 0 / 占位数值；
 * 用量卡的时间范围与顶部 RangePicker 同步，卡内提供 今日/7天/30天 快捷切换。
 */
import type { CurrentModel, Currency, RangePreset, SpeedQuality } from "./types";
import { formatCost, formatMs, formatTokens, formatTps } from "./format";
import { displayModelName } from "./modelName";
import { useI18n } from "./i18n";
import { SectionCard } from "./layout";

/** 速度卡需要的来源摘要（SummaryTab 的 AgentSummary 子集） */
export interface SpeedAgentView {
  id: string;
  name: string;
  color: string;
  currentModel: CurrentModel | null;
  avgTps: number | null;
  speedQuality: SpeedQuality | null;
  avgTtftMs: number | null;
}

const SPEED_RANGES: { value: RangePreset; labelKey: "range.today" | "range.7d" | "range.30d" }[] = [
  { value: "today", labelKey: "range.today" },
  { value: "7d", labelKey: "range.7d" },
  { value: "30d", labelKey: "range.30d" },
];

export function SimpleSpeedCard({
  agents,
  speedLocal,
}: {
  agents: SpeedAgentView[];
  /** 设备筛选合并远端时速度只来自本机，标签显式标注 */
  speedLocal: boolean;
}) {
  const { t } = useI18n();
  const withSpeed = agents
    .filter((a) => a.avgTps != null && Number.isFinite(a.avgTps))
    .sort(
      (a, b) =>
        (b.currentModel?.last_used_ms ?? 0) - (a.currentModel?.last_used_ms ?? 0)
    );
  const top = withSpeed[0] ?? null;
  const approx = top?.speedQuality === "request_average" ? "≈" : "";
  const speedText =
    top?.avgTps != null ? `${approx}${formatTps(top.avgTps)} t/s` : null;
  const speedLabel = speedLocal
    ? t("customize.speed.local")
    : t("customize.speed.label");
  const summary = top
    ? `${top.currentModel ? displayModelName(top.currentModel.model_id) : top.name} · ${
        speedText ?? t("customize.speed.none")
      }`
    : t("customize.speed.none");

  return (
    <SectionCard
      title={t("customize.card.speed")}
      card={{ window: "panel", page: "summary", id: "summary.speed" }}
      cardSummary={summary}
    >
      {({ detail }) =>
        top ? (
          <div className="min-w-0">
            <div className="flex items-baseline justify-between gap-2 min-w-0">
              <span
                className="num text-[22px] font-bold leading-none text-sky-700 truncate"
                title={speedText ?? undefined}
              >
                {speedText}
              </span>
              <span className="text-[9px] text-slate-500 shrink-0">
                {t("customize.speed.today")}
              </span>
            </div>
            <div className="mt-1.5 flex items-center gap-1 min-w-0 text-[10px] text-slate-600">
              <span className="shrink-0">{t("customize.speed.model")}</span>
              <span className="font-medium text-slate-900/85 truncate">
                {top.currentModel
                  ? displayModelName(top.currentModel.model_id)
                  : "—"}
              </span>
            </div>
            <div className="mt-0.5 flex items-center gap-1 text-[9px] text-slate-500 min-w-0">
              <span
                className="w-1.5 h-1.5 rounded-full shrink-0"
                style={{ background: top.color }}
              />
              <span className="truncate">{top.name}</span>
              <span className="shrink-0">· {speedLabel}</span>
            </div>
            {detail === "full" && top.avgTtftMs != null && (
              <div className="num mt-1 text-[9px] text-slate-500">
                {t("common.ttft")} {formatMs(top.avgTtftMs)}
              </div>
            )}
            {detail === "full" && withSpeed.length > 1 && (
              <div className="mt-1.5 pt-1.5 border-t border-slate-900/6 flex flex-col gap-0.5">
                {withSpeed.slice(1, 3).map((agent) => (
                  <div
                    key={agent.id}
                    className="flex items-center justify-between gap-2 text-[9px] text-slate-500 min-w-0"
                  >
                    <span className="truncate">{agent.name}</span>
                    <span className="num shrink-0">
                      {agent.speedQuality === "request_average" ? "≈" : ""}
                      {formatTps(agent.avgTps!)} t/s
                    </span>
                  </div>
                ))}
              </div>
            )}
          </div>
        ) : (
          <div className="py-1">
            <div className="text-[11px] font-medium text-slate-600">
              {t("customize.speed.none")}
            </div>
            <div className="mt-0.5 text-[9px] leading-relaxed text-slate-500">
              {t("customize.speed.noneHint")}
            </div>
          </div>
        )
      }
    </SectionCard>
  );
}

export function SimpleUsageCard({
  tokens,
  cost,
  requests,
  currency,
  rangeLabel,
  rangePreset,
  onRangeChange,
}: {
  tokens: number;
  cost: number;
  requests: number;
  currency: Currency;
  rangeLabel: string;
  rangePreset?: RangePreset;
  onRangeChange?: (preset: RangePreset) => void;
}) {
  const { t } = useI18n();
  const summary = `${rangeLabel} · ${formatTokens(tokens)} Token`;

  return (
    <SectionCard
      title={t("customize.card.usage")}
      card={{ window: "panel", page: "summary", id: "summary.usage" }}
      cardSummary={summary}
      action={
        onRangeChange ? (
          <div className="flex gap-0.5 p-0.5 rounded-lg bg-slate-900/4">
            {SPEED_RANGES.map((item) => (
              <button
                key={item.value}
                type="button"
                className={`px-1.5 py-0.5 rounded-md text-[9px] transition-all duration-150 ${
                  rangePreset === item.value
                    ? "bg-sky-500 text-white shadow-sm"
                    : "text-slate-500 hover:text-slate-700"
                }`}
                aria-pressed={rangePreset === item.value}
                onClick={() => onRangeChange(item.value)}
              >
                {t(item.labelKey)}
              </button>
            ))}
          </div>
        ) : undefined
      }
    >
      {({ detail }) => (
        <div className="min-w-0">
          <div className="text-[9px] text-slate-500">
            {t("customize.usage.rangeTokens")} · {rangeLabel}
          </div>
          <div className="num mt-0.5 text-[24px] font-bold leading-none text-slate-900/90 truncate">
            {formatTokens(tokens)}
          </div>
          <div className="mt-0.5 text-[9px] text-slate-500">
            {t("customize.usage.rangeHint")}
          </div>
          {detail === "full" && (
            <div className="mt-2 pt-2 border-t border-slate-900/6 grid grid-cols-2 gap-2">
              <div className="min-w-0">
                <div className="text-[8px] text-slate-500">
                  {t("customize.usage.rangeCost")}
                </div>
                <div className="num text-[11px] font-semibold text-slate-800 truncate">
                  {formatCost(cost, currency)}
                </div>
              </div>
              <div className="min-w-0">
                <div className="text-[8px] text-slate-500">
                  {t("customize.usage.requests")}
                </div>
                <div className="num text-[11px] font-semibold text-slate-800 truncate">
                  {formatTokens(requests)}
                </div>
              </div>
            </div>
          )}
        </div>
      )}
    </SectionCard>
  );
}
