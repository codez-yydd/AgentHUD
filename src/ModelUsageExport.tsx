/**
 * 模型用量「完整图片导出」（阶段四）。
 *
 * 与 ShareCard 的模型 TOP5 分享卡不同：这里导出**全部模型行**，不受卡片滚动
 * 位置 / 手动高度限制，也不写任何布局配置（临时画布与日常布局完全解耦）。
 *
 * 版式：1080+ 宽深色卡片，头部展示时间范围 / 筛选条件，KPI 四项，完整表格
 * （请求 / 输入 / 输出 / 总 Token / 花费 / 占比），底部给出口径说明。
 * 内容过长时按 MODELS_PER_PAGE 行分页，每页一张图片，避免生成不可用的超长图。
 * 预览弹层只渲染 canvas 与翻页控件，不包含拖动手柄、折叠/编辑按钮。
 */
import { useEffect, useMemo, useRef, useState } from "react";
import type { Currency } from "./types";
import { saveShareImage } from "./api";
import { formatCost, formatCountLocale, formatTokensLocale } from "./format";
import { useI18n } from "./i18n";

/** 单行模型数据（面板 / 主窗口两套来源归一化后的最小公共集） */
export interface ExportModelRow {
  key: string;
  name: string;
  /** 来源名（主窗口跨来源合并表可为空，空则不出该列） */
  source?: string;
  requests: number;
  /** 输入 / 输出 Token；面板模型榜没有拆分数据时传 null */
  input?: number | null;
  output?: number | null;
  tokens: number;
  /** 当前币种花费；null = 无有效价格 / 未拆分（按 unpriced/unsplit 显示） */
  cost: number | null;
  unpriced?: boolean;
  unsplit?: boolean;
  /** 占比 0~1（分母 = 全部行 total 之和） */
  share: number;
}

export interface ModelUsageExportData {
  rows: ExportModelRow[];
  currency: Currency;
  /** 时间范围文案（含自定义区间 / 今日 / 近 7 天等） */
  rangeLabel: string;
  /** 筛选条件（设备范围 / Agent / 来源等），渲染为一行 chips 文本 */
  filters: string[];
}

/** 每页行数：20 行 ≈ 一张 1240×1600 逻辑 px 的清晰图片 */
const MODELS_PER_PAGE = 20;
const CARD_W = 1240;
const CARD_PAD = 56;
const CARD_SCALE = 2;
const ROW_H = 46;
const TABLE_HEAD_H = 38;
const KPI_H = 92;
const HEADER_H = 206;
const FOOTER_H = 154;

const BG_TOP = "#0c1526";
const BG_MID = "#101f36";
const BG_BOTTOM = "#0a1220";
const TEXT = "#f1f5f9";
const TEXT_DIM = "#94a3b8";
const TEXT_FAINT = "#64748b";
const ACCENT = "#38bdf8";
const LINE = "rgba(148, 163, 184, 0.14)";
const ROW_BG = "rgba(148, 163, 184, 0.05)";

const FONT_STACK =
  "-apple-system, BlinkMacSystemFont, 'PingFang SC', 'Hiragino Sans GB', 'Microsoft YaHei', 'Segoe UI', sans-serif";

type TFn = ReturnType<typeof useI18n>["t"];

function roundRect(
  ctx: CanvasRenderingContext2D,
  x: number,
  y: number,
  w: number,
  h: number,
  r: number
): void {
  const radius = Math.min(r, w / 2, h / 2);
  ctx.beginPath();
  ctx.moveTo(x + radius, y);
  ctx.lineTo(x + w - radius, y);
  ctx.arcTo(x + w, y, x + w, y + radius, radius);
  ctx.lineTo(x + w, y + h - radius);
  ctx.arcTo(x + w, y + h, x + w - radius, y + h, radius);
  ctx.lineTo(x + radius, y + h);
  ctx.arcTo(x, y + h, x, y + h - radius, radius);
  ctx.lineTo(x, y + radius);
  ctx.arcTo(x, y, x + radius, y, radius);
  ctx.closePath();
}

function setFont(
  ctx: CanvasRenderingContext2D,
  size: number,
  weight = 400
): void {
  ctx.font = `${weight} ${size}px ${FONT_STACK}`;
}

function fitText(
  ctx: CanvasRenderingContext2D,
  text: string,
  maxWidth: number
): string {
  if (ctx.measureText(text).width <= maxWidth) return text;
  let out = text;
  while (out.length > 1 && ctx.measureText(out + "…").width > maxWidth) {
    out = out.slice(0, -1);
  }
  return out + "…";
}

function dateTimeLabel(ms: number): string {
  const d = new Date(ms);
  const p = (n: number) => String(n).padStart(2, "0");
  return `${d.getFullYear()}-${p(d.getMonth() + 1)}-${p(d.getDate())} ${p(
    d.getHours()
  )}:${p(d.getMinutes())}`;
}

function ymd(ms: number): string {
  const d = new Date(ms);
  const p = (n: number) => String(n).padStart(2, "0");
  return `${d.getFullYear()}${p(d.getMonth() + 1)}${p(d.getDate())}`;
}

interface Column {
  key: "rank" | "model" | "source" | "requests" | "input" | "output" | "tokens" | "cost" | "share";
  width: number;
  align: "left" | "right";
}

function buildColumns(hasSource: boolean, hasSplit: boolean): Column[] {
  if (hasSource && hasSplit) {
    return [
      { key: "rank", width: 38, align: "left" },
      { key: "model", width: 334, align: "left" },
      { key: "source", width: 132, align: "left" },
      { key: "requests", width: 86, align: "right" },
      { key: "input", width: 106, align: "right" },
      { key: "output", width: 106, align: "right" },
      { key: "tokens", width: 126, align: "right" },
      { key: "cost", width: 116, align: "right" },
      { key: "share", width: 84, align: "right" },
    ];
  }
  if (hasSource) {
    return [
      { key: "rank", width: 38, align: "left" },
      { key: "model", width: 360, align: "left" },
      { key: "source", width: 180, align: "left" },
      { key: "requests", width: 100, align: "right" },
      { key: "tokens", width: 170, align: "right" },
      { key: "cost", width: 160, align: "right" },
      { key: "share", width: 120, align: "right" },
    ];
  }
  if (hasSplit) {
    return [
      { key: "rank", width: 38, align: "left" },
      { key: "model", width: 466, align: "left" },
      { key: "requests", width: 86, align: "right" },
      { key: "input", width: 106, align: "right" },
      { key: "output", width: 106, align: "right" },
      { key: "tokens", width: 126, align: "right" },
      { key: "cost", width: 116, align: "right" },
      { key: "share", width: 84, align: "right" },
    ];
  }
  return [
    { key: "rank", width: 38, align: "left" },
    { key: "model", width: 470, align: "left" },
    { key: "requests", width: 110, align: "right" },
    { key: "tokens", width: 190, align: "right" },
    { key: "cost", width: 180, align: "right" },
    { key: "share", width: 140, align: "right" },
  ];
}

function columnLabel(key: Column["key"], t: TFn): string {
  switch (key) {
    case "rank":
      return t("export.colRank");
    case "model":
      return t("export.colModel");
    case "source":
      return t("export.colSource");
    case "requests":
      return t("export.colRequests");
    case "input":
      return t("export.colInput");
    case "output":
      return t("export.colOutput");
    case "tokens":
      return t("export.colTokens");
    case "cost":
      return t("export.colCost");
    case "share":
      return t("export.colShare");
  }
}

/** 表格一行的显示文本（按列取） */
function cellText(
  row: ExportModelRow,
  key: Column["key"],
  rank: number,
  locale: "zh" | "en",
  currency: Currency,
  t: TFn
): string {
  switch (key) {
    case "rank":
      return String(rank);
    case "model":
      return row.name;
    case "source":
      return row.source ?? "—";
    case "requests":
      return row.requests > 0 ? formatCountLocale(row.requests, locale) : "—";
    case "input":
      return row.input != null && row.input > 0
        ? formatTokensLocale(row.input, locale)
        : "—";
    case "output":
      return row.output != null && row.output > 0
        ? formatTokensLocale(row.output, locale)
        : "—";
    case "tokens":
      return formatTokensLocale(row.tokens, locale);
    case "cost":
      if (row.unsplit) return "—";
      if (row.cost == null || row.cost <= 0) {
        return row.tokens > 0 ? t("export.unpriced") : "—";
      }
      return formatCost(row.cost, currency);
    case "share":
      return `${(row.share * 100).toFixed(row.share < 0.001 ? 2 : 1)}%`;
  }
}

/** 绘制一页；返回画布逻辑高度 */
function drawPage(
  canvas: HTMLCanvasElement,
  data: ModelUsageExportData,
  pageRows: ExportModelRow[],
  pageIndex: number,
  pageCount: number,
  locale: "zh" | "en",
  t: TFn
): number {
  const ctx = canvas.getContext("2d");
  if (!ctx) return 0;
  const hasSource = data.rows.some((row) => !!row.source);
  const hasSplit = data.rows.some(
    (row) => row.input != null || row.output != null
  );
  const columns = buildColumns(hasSource, hasSplit);

  const H =
    CARD_PAD * 2 +
    HEADER_H +
    KPI_H +
    20 +
    TABLE_HEAD_H +
    Math.max(pageRows.length, 1) * ROW_H +
    18 +
    FOOTER_H;

  canvas.width = CARD_W * CARD_SCALE;
  canvas.height = Math.round(H * CARD_SCALE);
  ctx.setTransform(CARD_SCALE, 0, 0, CARD_SCALE, 0, 0);

  // 背景与顶部光晕
  const bg = ctx.createLinearGradient(0, 0, CARD_W, H);
  bg.addColorStop(0, BG_TOP);
  bg.addColorStop(0.55, BG_MID);
  bg.addColorStop(1, BG_BOTTOM);
  ctx.fillStyle = bg;
  ctx.fillRect(0, 0, CARD_W, H);
  const glow = ctx.createRadialGradient(
    CARD_W * 0.16,
    -50,
    30,
    CARD_W * 0.16,
    -50,
    560
  );
  glow.addColorStop(0, "rgba(56, 189, 248, 0.2)");
  glow.addColorStop(1, "rgba(56, 189, 248, 0)");
  ctx.fillStyle = glow;
  ctx.fillRect(0, 0, CARD_W, H * 0.45);

  // ===== 头部 =====
  let y = CARD_PAD;
  const mark = 46;
  roundRect(ctx, CARD_PAD, y, mark, mark, 12);
  const markGrad = ctx.createLinearGradient(
    CARD_PAD,
    y,
    CARD_PAD + mark,
    y + mark
  );
  markGrad.addColorStop(0, "#0ea5e9");
  markGrad.addColorStop(1, "#6366f1");
  ctx.fillStyle = markGrad;
  ctx.fill();
  setFont(ctx, 28, 800);
  ctx.fillStyle = "#fff";
  ctx.textAlign = "center";
  ctx.fillText("A", CARD_PAD + mark / 2, y + 33);
  ctx.textAlign = "left";

  setFont(ctx, 26, 700);
  ctx.fillStyle = TEXT;
  ctx.fillText("AgentHUD", CARD_PAD + mark + 16, y + 31);

  setFont(ctx, 38, 700);
  ctx.fillStyle = TEXT;
  ctx.fillText(t("export.title"), CARD_PAD, y + 96);

  // 页码（右上）
  setFont(ctx, 20, 500);
  ctx.fillStyle = TEXT_DIM;
  ctx.textAlign = "right";
  ctx.fillText(
    t("export.page", { page: pageIndex + 1, total: pageCount }),
    CARD_W - CARD_PAD,
    y + 31
  );
  ctx.textAlign = "left";

  // 范围与筛选
  setFont(ctx, 21, 500);
  ctx.fillStyle = TEXT_DIM;
  const rangeLine = `${t("export.range")}: ${data.rangeLabel}`;
  ctx.fillText(fitText(ctx, rangeLine, 760), CARD_PAD, y + 132);
  const filterLine = `${t("export.filters")}: ${
    data.filters.length > 0
      ? data.filters.join(" · ")
      : t("export.filterAllSources")
  }`;
  ctx.fillStyle = TEXT_FAINT;
  setFont(ctx, 19, 400);
  ctx.fillText(
    fitText(ctx, filterLine, CARD_W - CARD_PAD * 2),
    CARD_PAD,
    y + 160
  );
  y += HEADER_H;

  // ===== KPI =====
  const totalTokens = data.rows.reduce((sum, row) => sum + row.tokens, 0);
  const totalRequests = data.rows.reduce((sum, row) => sum + row.requests, 0);
  const totalCost = data.rows.reduce((sum, row) => sum + (row.cost ?? 0), 0);
  const kpiGap = 14;
  const kpiW = (CARD_W - CARD_PAD * 2 - kpiGap * 3) / 4;
  const kpis = [
    { label: t("export.kpiModels"), value: t("export.allModels", { count: data.rows.length }) },
    { label: t("export.kpiTokens"), value: formatTokensLocale(totalTokens, locale) },
    { label: t("export.kpiRequests"), value: formatCountLocale(totalRequests, locale) },
    { label: t("export.kpiCost"), value: formatCost(totalCost, data.currency) },
  ];
  kpis.forEach((kpi, index) => {
    const x = CARD_PAD + index * (kpiW + kpiGap);
    roundRect(ctx, x, y, kpiW, KPI_H - 10, 14);
    ctx.fillStyle = "rgba(148, 163, 184, 0.08)";
    ctx.fill();
    ctx.strokeStyle = LINE;
    ctx.stroke();
    setFont(ctx, 17, 500);
    ctx.fillStyle = TEXT_DIM;
    ctx.fillText(kpi.label, x + 16, y + 28);
    setFont(ctx, 26, 700);
    ctx.fillStyle = index === 1 ? ACCENT : TEXT;
    ctx.fillText(fitText(ctx, kpi.value, kpiW - 32), x + 16, y + 66);
  });
  y += KPI_H + 20;

  // ===== 表头 =====
  let x = CARD_PAD;
  setFont(ctx, 17, 600);
  ctx.fillStyle = TEXT_FAINT;
  for (const col of columns) {
    if (col.align === "right") {
      ctx.textAlign = "right";
      ctx.fillText(columnLabel(col.key, t), x + col.width - 6, y + 25);
      ctx.textAlign = "left";
    } else {
      ctx.fillText(columnLabel(col.key, t), x, y + 25);
    }
    x += col.width;
  }
  y += TABLE_HEAD_H;
  ctx.strokeStyle = LINE;
  ctx.beginPath();
  ctx.moveTo(CARD_PAD, y - 8);
  ctx.lineTo(CARD_W - CARD_PAD, y - 8);
  ctx.stroke();

  // ===== 数据行 =====
  if (pageRows.length === 0) {
    setFont(ctx, 24, 500);
    ctx.fillStyle = TEXT_DIM;
    ctx.fillText(t("export.empty"), CARD_PAD, y + 40);
  }
  pageRows.forEach((row, index) => {
    const rowY = y + index * ROW_H;
    if (index % 2 === 1) {
      ctx.fillStyle = ROW_BG;
      ctx.fillRect(CARD_PAD - 8, rowY, CARD_W - CARD_PAD * 2 + 16, ROW_H);
    }
    let cx = CARD_PAD;
    const rank = pageIndex * MODELS_PER_PAGE + index + 1;
    for (const col of columns) {
      const text = cellText(row, col.key, rank, locale, data.currency, t);
      if (col.key === "rank") {
        setFont(ctx, 18, 500);
        ctx.fillStyle = TEXT_FAINT;
      } else if (col.key === "model") {
        setFont(ctx, 21, 600);
        ctx.fillStyle = TEXT;
      } else if (col.key === "cost" || col.key === "tokens") {
        setFont(ctx, 19, 500);
        ctx.fillStyle = col.key === "cost" ? TEXT_DIM : TEXT;
      } else {
        setFont(ctx, 18, 400);
        ctx.fillStyle = TEXT_DIM;
      }
      if (col.align === "right") {
        ctx.textAlign = "right";
        ctx.fillText(
          fitText(ctx, text, col.width - 14),
          cx + col.width - 6,
          rowY + 29
        );
        ctx.textAlign = "left";
      } else {
        ctx.fillText(
          fitText(
            ctx,
            text,
            col.key === "model" ? col.width - 20 : col.width - 10
          ),
          cx,
          rowY + 29
        );
      }
      cx += col.width;
    }
  });
  y += Math.max(pageRows.length, 1) * ROW_H + 18;

  // ===== 脚注 =====
  ctx.strokeStyle = LINE;
  ctx.beginPath();
  ctx.moveTo(CARD_PAD, y);
  ctx.lineTo(CARD_W - CARD_PAD, y);
  ctx.stroke();
  y += 26;
  setFont(ctx, 17, 400);
  ctx.fillStyle = TEXT_FAINT;
  const notes = [
    t("export.noteTokens"),
    t("export.noteCost"),
    t("export.noteUnsplit"),
  ];
  notes.forEach((note, index) => {
    ctx.fillText(
      fitText(ctx, note, CARD_W - CARD_PAD * 2),
      CARD_PAD,
      y + index * 25
    );
  });
  y += notes.length * 25 + 10;
  setFont(ctx, 16, 500);
  ctx.fillStyle = TEXT_FAINT;
  ctx.textAlign = "right";
  ctx.fillText(
    `${t("export.page", {
      page: pageIndex + 1,
      total: pageCount,
    })} · ${t("export.generated", { time: dateTimeLabel(Date.now()) })}`,
    CARD_W - CARD_PAD,
    y + 12
  );
  ctx.textAlign = "left";

  return H;
}

function pageDateLabel(): string {
  return ymd(Date.now());
}

/**
 * 导出预览弹层：翻页预览 + 保存当前页。
 * 画布仅由数据绘制，不含拖动手柄 / 折叠 / 编辑控件；不读写布局配置。
 */
export function ModelUsageExportModal({
  data,
  onClose,
}: {
  data: ModelUsageExportData;
  onClose: () => void;
}) {
  const { t, locale } = useI18n();
  const [page, setPage] = useState(0);
  const [fit, setFit] = useState(true);
  const [saving, setSaving] = useState(false);
  const [status, setStatus] = useState<string | null>(null);
  const canvasRef = useRef<HTMLCanvasElement | null>(null);

  const pages = useMemo(() => {
    const out: ExportModelRow[][] = [];
    for (let i = 0; i < data.rows.length; i += MODELS_PER_PAGE) {
      out.push(data.rows.slice(i, i + MODELS_PER_PAGE));
    }
    return out.length > 0 ? out : [[]];
  }, [data.rows]);

  useEffect(() => {
    setPage(0);
  }, [data.rows]);

  useEffect(() => {
    setPage((p) => Math.min(p, pages.length - 1));
  }, [pages.length]);

  useEffect(() => {
    const canvas = canvasRef.current;
    if (!canvas) return;
    drawPage(canvas, data, pages[page] ?? [], page, pages.length, locale, t);
  }, [data, pages, page, locale, t]);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") onClose();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onClose]);

  const handleSave = () => {
    const canvas = canvasRef.current;
    if (!canvas || saving) return;
    setSaving(true);
    setStatus(null);
    canvas.toBlob(async (blob) => {
      if (!blob) {
        setSaving(false);
        return;
      }
      try {
        const buf = await blob.arrayBuffer();
        const bytes = Array.from(new Uint8Array(buf));
        const name = `agenthud-models-${pageDateLabel()}-p${page + 1}.png`;
        const path = await saveShareImage(bytes, name);
        if (path) setStatus(t("export.saved", { path }));
      } catch (e) {
        setStatus(t("export.saveFail", { msg: String(e) }));
      } finally {
        setSaving(false);
      }
    }, "image/png");
  };

  return (
    <div
      className="fixed inset-0 z-[90] flex items-center justify-center bg-black/55 p-2"
      role="dialog"
      aria-modal="true"
      aria-label={t("export.previewTitle")}
    >
      <div className="bg-elevated flex max-h-[94vh] w-[min(1000px,96vw)] flex-col rounded-2xl border border-slate-900/10 shadow-2xl">
        <div className="flex shrink-0 flex-wrap items-center justify-between gap-2 border-b border-slate-900/8 px-3 py-2">
          <div className="min-w-0">
            <div className="truncate text-[12px] font-semibold text-slate-900">
              {t("export.previewTitle")}
            </div>
            <div className="text-[9px] text-slate-500">
              {t("export.page", { page: page + 1, total: pages.length })} ·{" "}
              {t("export.pageHint")}
            </div>
          </div>
          <div className="flex items-center gap-1">
            <button
              type="button"
              className="rounded-lg border border-slate-900/10 px-2 py-1 text-[10px] text-slate-600 hover:bg-slate-900/5"
              onClick={() => setFit((v) => !v)}
              title={fit ? t("export.actualSize") : t("export.fitWidth")}
            >
              {fit ? t("export.actualSize") : t("export.fitWidth")}
            </button>
            <button
              type="button"
              className="rounded-lg border border-slate-900/10 px-2 py-1 text-[10px] text-slate-600 disabled:opacity-35"
              onClick={() => setPage((p) => Math.max(0, p - 1))}
              disabled={page === 0}
            >
              {t("export.prev")}
            </button>
            <button
              type="button"
              className="rounded-lg border border-slate-900/10 px-2 py-1 text-[10px] text-slate-600 disabled:opacity-35"
              onClick={() => setPage((p) => Math.min(pages.length - 1, p + 1))}
              disabled={page >= pages.length - 1}
            >
              {t("export.next")}
            </button>
            <button
              type="button"
              className="rounded-lg border border-slate-900/10 px-2 py-1 text-[10px] text-slate-600 hover:bg-slate-900/5"
              onClick={onClose}
              title={t("export.close")}
            >
              ×
            </button>
          </div>
        </div>

        <div className="min-h-0 flex-1 overflow-auto rounded-b-xl bg-slate-950/90 p-2">
          <canvas
            ref={canvasRef}
            className="h-auto rounded-lg"
            style={{
              width: fit ? "100%" : CARD_W,
              minWidth: fit ? 0 : CARD_W,
              maxWidth: "none",
              display: "block",
            }}
            aria-label={t("export.title")}
          />
        </div>

        <div className="flex shrink-0 flex-wrap items-center gap-2 border-t border-slate-900/8 px-3 py-2">
          <span className="min-w-0 flex-1 truncate text-[9px] text-slate-500">
            {status}
          </span>
          <button
            type="button"
            className="rounded-lg bg-sky-500 px-3 py-1.5 text-[11px] font-medium text-white transition-colors hover:bg-sky-600 disabled:opacity-50"
            onClick={handleSave}
            disabled={saving || data.rows.length === 0}
          >
            {saving ? t("export.saving") : t("export.savePage")}
          </button>
        </div>
      </div>
    </div>
  );
}
