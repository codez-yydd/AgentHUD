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
 * 耗时显示来源库的真实值。速度与表格共用后端校验过的数值及其计时口径。
 */
import { useEffect, useMemo, useRef, useState } from "react";
import { useI18n } from "../../i18n";
import { formatCost, formatTokensLocale } from "../../format";
import { getEquivalentCosts, getReferenceTable } from "../../api";
import { displayModelName } from "../../modelName";
import { ModelSearchPicker } from "./ModelSearchPicker";
import { costOf } from "../overview/derive";
import type {
  Currency,
  EquivalentCost,
  ReferenceTable,
  RequestLogRow,
} from "../../types";
import {
  durationText,
  exactCount,
  formatLogSpeed,
  logTimeFull,
  sourceLabel,
  speedHintKey,
  speedLabelKey,
  speedOf,
  ttftText,
} from "./model";

/** 「等价花费」对比模型选择的持久化键（与日志页其它偏好同款用 localStorage） */
const EQUIVALENT_KEY = "zbar-equivalent-models";
/** 对比模型数量上限：再多，详情抽屉就变成价目表了 */
const EQUIVALENT_MAX = 6;

/** 读取已保存的对比模型；没存过 → null（由「当前范围最常用」的默认值接管） */
function loadEquivalentSelection(): string[] | null {
  try {
    const raw = localStorage.getItem(EQUIVALENT_KEY);
    if (raw == null) return null;
    const parsed: unknown = JSON.parse(raw);
    return Array.isArray(parsed)
      ? parsed.filter((item): item is string => typeof item === "string")
      : null;
  } catch {
    return null;
  }
}

/** 持久化对比模型：空数组也写入——表示用户明确清空，不再回落到默认值 */
function saveEquivalentSelection(ids: string[]): void {
  try {
    localStorage.setItem(EQUIVALENT_KEY, JSON.stringify(ids));
  } catch {
    /* 忽略：隐私模式 / 配额满，选择只在本次会话生效 */
  }
}

/** 时间戳 → 本地 "MM-DD HH:mm"（参考价表的同步时刻，只到分钟足够） */
function syncTimeText(ms: number): string {
  const date = new Date(ms);
  const pad = (value: number) => String(value).padStart(2, "0");
  return `${pad(date.getMonth() + 1)}-${pad(date.getDate())} ${pad(
    date.getHours()
  )}:${pad(date.getMinutes())}`;
}

/** 一列键值（值单行截断 + title 给全文，长模型名 / 会话 id 不撑破面板；
 *  title 缺省用 value，token 等有紧凑格式的字段显式传精确整数） */function Kv({
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
  modelIds = [],
  onClose,
}: {
  row: RequestLogRow;
  currency: Currency;
  /** 设备 id → 展示名（页面统一维护）：设备行显示「设备名 + 完整 id」 */
  deviceLabel: (deviceId: string) => string;
  /** 当前日志范围里出现过的模型 id（按请求数降序）：等价花费的默认对比集。
   *  页面不传就给空数组，此时只能由用户手动挑模型。 */
  modelIds?: string[];
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
  const ttft = ttftText(row.ttft_ms);
  const mainCost = costOf(row.cost_usd, row.cost_cny, currency);
  const otherCurrency: Currency = currency === "cny" ? "usd" : "cny";
  const otherCost = costOf(row.cost_usd, row.cost_cny, otherCurrency);
  /**
   * 等价花费（参考价估算）。
   *
   * 对比哪些模型**不写死**：默认取当前日志范围里最常用的几个（页面通过
   * `modelIds` 传进来，已按请求数降序），并排除本行自己的模型；用户还可以
   * 在下拉里从内置参考表任意增删，选择存 localStorage 下次沿用（清空也算
   * 明确选择，不再回落到默认值）。
   */
  const [storedSelection] = useState(() => loadEquivalentSelection());
  const selectionTouched = useRef(storedSelection != null);
  const [compareIds, setCompareIds] = useState<string[]>(storedSelection ?? []);
  const [equivalents, setEquivalents] = useState<EquivalentCost[]>([]);
  /** 参考价表（模型的候选集 + 在线表新鲜度）：抽屉打开时取一次 */
  const [referenceTable, setReferenceTable] = useState<ReferenceTable | null>(
    null
  );

  /** 默认对比集：当前范围内最常用的 3 个模型（不含本行模型） */
  const candidates = useMemo(
    () => modelIds.filter((id) => id !== row.model_id).slice(0, 3),
    [modelIds, row.model_id]
  );

  // 没存过选择时跟随候选默认值（用户手动改过之后就不再覆盖）
  useEffect(() => {
    if (!selectionTouched.current) setCompareIds(candidates);
  }, [candidates]);

  // 参考价表：选择器候选 + 在线表抓取时间，抽屉打开时取一次
  useEffect(() => {
    let cancelled = false;
    getReferenceTable()
      .then((table) => {
        if (!cancelled) setReferenceTable(table);
      })
      .catch(() => {
        /* 静默：选择器退化为只能移除已选项，不影响已选模型的折算 */
      });
    return () => {
      cancelled = true;
    };
  }, []);

  // 折算：当前行或对比集变化就重算（对比集为空则不请求）
  useEffect(() => {
    if (compareIds.length === 0) {
      setEquivalents([]);
      return;
    }
    let cancelled = false;
    getEquivalentCosts(row, compareIds)
      .then((list) => {
        if (!cancelled) setEquivalents(list);
      })
      .catch(() => {
        // 参考表缺失 / IPC 异常：静默降级为不显示，详情其它信息照常
        if (!cancelled) setEquivalents([]);
      });
    return () => {
      cancelled = true;
    };
  }, [row, compareIds]);

  /** 改选对比模型：去重 + 截断上限，并落盘（清空同样落盘，表示"就要空"） */
  const applySelection = (next: string[]) => {
    const unique = next
      .filter((id, index) => next.indexOf(id) === index)
      .slice(0, EQUIVALENT_MAX);
    selectionTouched.current = true;
    setCompareIds(unique);
    saveEquivalentSelection(unique);
  };
  // token 明细：紧凑值 + 精确整数 title
  const tokens: { label: string; value: number }[] = [
    { label: t("dashboard.logs.detailInput"), value: row.input_tokens },
    { label: t("dashboard.logs.detailOutput"), value: row.output_tokens },
    { label: t("dashboard.logs.detailCacheRead"), value: row.cache_read_tokens },
    { label: t("dashboard.logs.detailCacheWrite"), value: row.cache_write_tokens },
    { label: t("dashboard.logs.detailReasoning"), value: row.reasoning_tokens },
    { label: t("dashboard.logs.detailTotal"), value: row.total_tokens },
  ];

  /** 选择器候选：合并表（快照 ← models.dev ← 本地价表）里的全部模型 */
  const referenceModels = referenceTable?.models ?? [];

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

          {/* ---- 等价花费（参考价估算）：对比模型由用户决定，不写死 ---- */}
          {row.total_tokens > 0 && (
            <section className="db-drawer__section">
              <h3
                className="db-drawer__section-title"
                title={t("dashboard.logs.detailEquivalentHint")}
              >
                {t("dashboard.logs.detailEquivalent")}
              </h3>
              {equivalents.map((item) => (
                <div className="db-drawer__kv" key={item.model_id}>
                  <span className="db-drawer__k db-drawer__eq-k">
                    <button
                      type="button"
                      className="db-drawer__eq-x"
                      title={t("dashboard.logs.detailEquivalentRemove")}
                      aria-label={t("dashboard.logs.detailEquivalentRemove")}
                      onClick={() =>
                        applySelection(
                          compareIds.filter((id) => id !== item.model_id)
                        )
                      }
                    >
                      ×
                    </button>
                    <span className="db-drawer__eq-name" title={item.model_id}>
                      {displayModelName(item.model_id)}
                    </span>
                  </span>
                  <span
                    className="db-drawer__v num"
                    title={formatCost(item.usd, "usd")}
                  >
                    {formatCost(costOf(item.usd, item.cny, currency), currency)}
                  </span>
                </div>
              ))}
              {compareIds.length === 0 && (
                <p className="db-drawer__eq-empty">
                  {t("dashboard.logs.detailEquivalentEmpty")}
                </p>
              )}
              {referenceModels.length > 0 && compareIds.length < EQUIVALENT_MAX && (
                <div className="db-drawer__eq-add">
                  {/* 候选来自运行时合并表（models.dev 每天后台刷新），按输入价降序；
                      几百个模型靠滚动找不现实，用带模糊搜索的选择器 */}
                  <ModelSearchPicker
                    label={t("dashboard.logs.detailEquivalentAdd")}
                    options={referenceModels
                      .filter((model) => !compareIds.includes(model.model_id))
                      .map((model) => ({
                        value: model.model_id,
                        label: displayModelName(model.model_id),
                        hint: `$${model.input}/M`,
                      }))}
                    onPick={(id) => applySelection([...compareIds, id])}
                  />
                </div>
              )}
              {/* 数据新鲜度：在线表同步过就报时间，没有就说清是内置快照 */}
              <p className="db-drawer__eq-note">
                {referenceTable?.fetched_at != null
                  ? t("dashboard.logs.detailEquivalentFresh", {
                      time: syncTimeText(referenceTable.fetched_at),
                    })
                  : t("dashboard.logs.detailEquivalentStale")}
              </p>
            </section>
          )}

          {duration != null || ttft != null || speed ? (
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
                label={t(speed ? speedLabelKey(speed.basis) : "dashboard.logs.colSpeed")}
                value={
                  speed
                    ? `${formatLogSpeed(speed.tps, speed.basis)} t/s`
                    : "—"
                }
                title={t(speed ? speedHintKey(speed.basis, row.source) : "dashboard.logs.noSpeedTitle")}
                mono
              />
            </section>
          ) : null}
        </div>
      </aside>
    </>
  );
}

