import { useEffect, useMemo, useRef, useState } from "react";
import type { PricingConfig } from "./types";
import { saveShareImage } from "./api";
import { formatCost, formatCountLocale, formatTokensLocale } from "./format";
import { BtnPrimary } from "./layout";
import { useI18n } from "./i18n";
import {
  rangeAgentRows,
  rangeModelRows,
  type ShareReportData,
} from "./ReportPanel";

// ============================================================
// 用量分享卡片：纯 Canvas 2D 手绘（零第三方依赖）。
// 数据复用报告页已聚合结果（经 props 传入，不再自行拉取数据源——
// 打开即出图），内容自上而下：品牌头 + KPI 四项 + Agent 分布 +
// 模型 TOP5 + Token 活动热力图（与页面热力图同口径：列=周、7 行、
// 非零样本分位四档蓝色）。
// ============================================================

/** 画布逻辑宽度（实际导出为 ×2 物理像素） */
const CARD_W = 1080;
const CARD_PAD = 64;
/** 导出缩放：固定 2x，与设备 devicePixelRatio 无关，跨设备一致清晰 */
const CARD_SCALE = 2;

const DARK_BG_TOP = "#0c1526";
const DARK_BG_MID = "#101f36";
const DARK_BG_BOTTOM = "#0a1220";
const TEXT_PRIMARY = "#f1f5f9";
const TEXT_SECONDARY = "#94a3b8";
const TEXT_FAINT = "#64748b";
const ACCENT = "#38bdf8";
const BAR_TRACK = "rgba(148, 163, 184, 0.14)";
/** 热力图分档色：与 index.css .dark .heat-level-1~4 同源（ZCode
 *  usage-heatmap token：sky-400/300 按 24/42/62/78% 混卡片底的 sRGB
 *  线性近似——canvas 无法用 CSS color-mix，分享卡为固定深底图片）。
 *  空档与暗色 0 档一致（白 5%）。 */
const HEAT_COLORS = ["#244b69", "#29668b", "#2e8ebf", "#68aed2"];
const HEAT_EMPTY = "rgba(255, 255, 255, 0.05)";

const FONT_STACK =
  "-apple-system, BlinkMacSystemFont, 'PingFang SC', 'Hiragino Sans GB', 'Microsoft YaHei', 'Segoe UI', sans-serif";

function startOfLocalDay(ms: number): number {
  const d = new Date(ms);
  d.setHours(0, 0, 0, 0);
  return d.getTime();
}

/** ms 所在自然周（本地时区）的周一 0 点（与报告页同款实现）。 */
function weekStartOf(ms: number): number {
  const d = new Date(ms);
  const day = d.getDay();
  d.setDate(d.getDate() - (day === 0 ? 6 : day - 1));
  d.setHours(0, 0, 0, 0);
  return d.getTime();
}

/** 本地日期加 n 天（Date 日期运算，规避毫秒减法的 DST 偏移）。 */
function addDays(ms: number, days: number): number {
  const d = new Date(ms);
  d.setDate(d.getDate() + days);
  return d.getTime();
}

/** 本地日期 MM-DD（卡片日期范围展示用） */
function mdLabel(ms: number): string {
  const d = new Date(ms);
  const p = (n: number) => String(n).padStart(2, "0");
  return `${p(d.getMonth() + 1)}-${p(d.getDate())}`;
}

function ymdLabel(ms: number): string {
  const d = new Date(ms);
  const p = (n: number) => String(n).padStart(2, "0");
  return `${d.getFullYear()}${p(d.getMonth() + 1)}${p(d.getDate())}`;
}

// ===== 展示视图（从报告页数据派生，纯同步计算） =====

interface ShareAgentRow {
  name: string;
  color: string;
  tokens: number;
  /** 占全部 token 的比例 0~1 */
  share: number;
}

interface ShareModelRow {
  name: string;
  color: string;
  tokens: number;
  /** 无有效价格时为 null（不标注花费） */
  cost: number | null;
}

interface ShareView {
  fromMs: number;
  toMs: number;
  currency: ShareReportData["currency"];
  tokens: number;
  requests: number;
  cost: number;
  activeDays: number;
  agents: ShareAgentRow[];
  models: ShareModelRow[];
  /** 热力图：列=周（周一起），cells 固定 7 槽（周一~周日），范围外为 null */
  heat: { cells: ({ dayMs: number; tokens: number } | null)[] }[];
}

function buildView(data: ShareReportData, pricing: PricingConfig): ShareView {
  const { daily, currency } = data;
  // 汇总取报告范围切片口径（daily 即切片结果，与页面 KPI 一致）
  let tokens = 0;
  let requests = 0;
  let cost = 0;
  let activeDays = 0;
  for (const d of daily) {
    tokens += d.tokens;
    requests += d.requests;
    cost += currency === "cny" ? d.costCny : d.costUsd;
    if (d.tokens > 0) activeDays += 1;
  }

  const agentRows: ShareAgentRow[] = rangeAgentRows(data).map((a) => ({
    name: a.name,
    color: a.color,
    tokens: a.tokens,
    share: a.share,
  }));

  const colorOf = new Map(data.agents.map((a) => [a.id, a.color]));
  const modelRows: ShareModelRow[] = rangeModelRows(
    data,
    pricing,
    currency,
    5
  ).map((m) => ({
    name: m.modelId,
    color: colorOf.get(m.agentId) ?? "#94a3b8",
    tokens: m.tokens,
    cost: m.cost,
  }));

  // 热力图：固定一年视图（52 周前的周一 → 今天，约 53 列），与页面一致；
  // 数据保留期（89 天）之外的日期 token 为 0，绘制为空档色
  const dayTokens = new Map(daily.map((d) => [d.dayMs, d.tokens]));
  const last = startOfLocalDay(Date.now());
  const anchor = new Date(last);
  anchor.setDate(anchor.getDate() - 52 * 7);
  const first = weekStartOf(anchor.getTime());
  const weeks: { cells: ({ dayMs: number; tokens: number } | null)[] }[] = [];
  let cursor = first;
  while (cursor <= last) {
    const cells: ({ dayMs: number; tokens: number } | null)[] = [];
    for (let i = 0; i < 7; i++) {
      const day = addDays(cursor, i);
      if (day < first || day > last) {
        cells.push(null);
      } else {
        cells.push({ dayMs: day, tokens: dayTokens.get(day) ?? 0 });
      }
    }
    weeks.push({ cells });
    cursor = addDays(cursor, 7);
  }

  return {
    fromMs: data.fromMs,
    toMs: data.toMs,
    currency,
    tokens,
    requests,
    cost,
    activeDays,
    agents: agentRows,
    models: modelRows,
    heat: weeks,
  };
}

// ===== Canvas 绘制 =====

function roundRectPath(
  ctx: CanvasRenderingContext2D,
  x: number,
  y: number,
  w: number,
  h: number,
  r: number
) {
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

/** 超宽文本截断加省略号 */
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

function drawSectionTitle(
  ctx: CanvasRenderingContext2D,
  text: string,
  y: number
): void {
  setFont(ctx, 26, 600);
  ctx.fillStyle = TEXT_SECONDARY;
  ctx.fillText(text, CARD_PAD, y);
}

function drawBarRow(
  ctx: CanvasRenderingContext2D,
  y: number,
  rowH: number,
  name: string,
  barColor: string,
  dotColor: string,
  share: number,
  valueText: string
): void {
  const nameW = 330;
  const barX = CARD_PAD + nameW;
  const barW = CARD_W - CARD_PAD * 2 - nameW - 260;
  const valueX = CARD_W - CARD_PAD;

  // 名称（圆点 + 文本）
  ctx.fillStyle = dotColor;
  ctx.beginPath();
  ctx.arc(CARD_PAD + 7, y + rowH / 2, 7, 0, Math.PI * 2);
  ctx.fill();
  setFont(ctx, 26, 500);
  ctx.fillStyle = TEXT_PRIMARY;
  ctx.fillText(fitText(ctx, name, nameW - 30), CARD_PAD + 26, y + rowH / 2 + 9);

  // 条形轨道 + 占比条
  const barY = y + rowH / 2 - 9;
  roundRectPath(ctx, barX, barY, barW, 18, 9);
  ctx.fillStyle = BAR_TRACK;
  ctx.fill();
  if (share > 0) {
    roundRectPath(
      ctx,
      barX,
      barY,
      Math.max(barW * Math.min(share, 1), 18),
      18,
      9
    );
    const grad = ctx.createLinearGradient(barX, 0, barX + barW, 0);
    grad.addColorStop(0, barColor);
    grad.addColorStop(1, barColor + "aa");
    ctx.fillStyle = grad;
    ctx.fill();
  }

  // 右侧数值
  setFont(ctx, 24, 600);
  ctx.fillStyle = TEXT_SECONDARY;
  ctx.textAlign = "right";
  ctx.fillText(valueText, valueX, y + rowH / 2 + 9);
  ctx.textAlign = "left";
}

/** token 量 → 亮度档位：相对峰值比例阈值（≥75% 峰值、≥40% 高活、
 *  ≥15% 日常、其余低活），与页面热力图同口径。 */
function heatLevel(tokens: number, max: number): number {
  if (tokens <= 0 || max <= 0) return 0;
  if (tokens >= max * 0.75) return 4;
  if (tokens >= max * 0.4) return 3;
  if (tokens >= max * 0.15) return 2;
  return 1;
}

/** 热力图月份短标签：zh "10月"，en "Oct"。 */
function monthShortLabel(month: number, locale: "zh" | "en"): string {
  if (locale === "zh") return `${month + 1}月`;
  return new Date(2020, month, 1).toLocaleString("en-US", { month: "short" });
}

/** canvas 上的文案（跟随 UI 语言，渲染时由 t 汇出） */
interface ShareTexts {
  title: string;
  totalCost: string;
  totalTokens: string;
  totalRequests: string;
  activeDays: string;
  activeDaysUnit: string;
  agentsTitle: string;
  topModels: string;
  heatTitle: string;
  footer: string;
}

/**
 * 绘制分享卡片。按内容计算高度并一次性设定画布尺寸（2x 物理像素），
 * 返回卡片逻辑高度。
 */
function drawShareCard(
  canvas: HTMLCanvasElement,
  view: ShareView,
  texts: ShareTexts,
  locale: "zh" | "en"
): number {
  const ctx = canvas.getContext("2d");
  if (!ctx) return 0;

  const showAgents = view.agents.length > 0;
  const showModels = view.models.length > 0;
  const showHeat = view.heat.some((week) =>
    week.cells.some((c) => c != null && c.tokens > 0)
  );

  // 先按内容算高度，再一次性设定画布尺寸（重设尺寸会清空画布）
  const HEADER_H = 168;
  const METRICS_H = 188;
  const ROW_H = 62;
  const SECTION_GAP = 44;
  const HEAT_CELL_H = 18;
  const HEAT_GAP = 8;
  const heatRows = view.heat.length;
  const heatGridH = 7 * HEAT_CELL_H + 6 * HEAT_GAP;
  let contentH = CARD_PAD + HEADER_H + METRICS_H;
  if (showAgents)
    contentH += SECTION_GAP + 44 + view.agents.length * ROW_H + 18;
  if (showModels)
    contentH += SECTION_GAP + 44 + view.models.length * ROW_H + 18;
  if (showHeat) contentH += SECTION_GAP + 44 + heatGridH + 44;
  const H = contentH + 96;

  canvas.width = CARD_W * CARD_SCALE;
  canvas.height = Math.round(H * CARD_SCALE);
  ctx.setTransform(CARD_SCALE, 0, 0, CARD_SCALE, 0, 0);

  // 背景：深底渐变 + 顶部天蓝色氛围光（canvas 上模拟深色毛玻璃，不用 backdrop-filter）
  const bg = ctx.createLinearGradient(0, 0, CARD_W, H);
  bg.addColorStop(0, DARK_BG_TOP);
  bg.addColorStop(0.55, DARK_BG_MID);
  bg.addColorStop(1, DARK_BG_BOTTOM);
  ctx.fillStyle = bg;
  ctx.fillRect(0, 0, CARD_W, H);
  const glow = ctx.createRadialGradient(
    CARD_W * 0.18,
    -60,
    40,
    CARD_W * 0.18,
    -60,
    620
  );
  glow.addColorStop(0, "rgba(56, 189, 248, 0.22)");
  glow.addColorStop(1, "rgba(56, 189, 248, 0)");
  ctx.fillStyle = glow;
  ctx.fillRect(0, 0, CARD_W, H * 0.5);

  ctx.textBaseline = "alphabetic";

  // ===== 顶部：品牌 + 标题 + 日期范围 =====
  const headY = CARD_PAD;

  // 品牌徽标：渐变圆角方块 + Z 字
  const markSize = 52;
  roundRectPath(ctx, CARD_PAD, headY, markSize, markSize, 14);
  const markGrad = ctx.createLinearGradient(
    CARD_PAD,
    headY,
    CARD_PAD + markSize,
    headY + markSize
  );
  markGrad.addColorStop(0, "#0ea5e9");
  markGrad.addColorStop(1, "#6366f1");
  ctx.fillStyle = markGrad;
  ctx.fill();
  setFont(ctx, 32, 800);
  ctx.fillStyle = "#ffffff";
  ctx.textAlign = "center";
  ctx.fillText("Z", CARD_PAD + markSize / 2, headY + markSize / 2 + 11);
  ctx.textAlign = "left";

  setFont(ctx, 30, 700);
  ctx.fillStyle = TEXT_PRIMARY;
  ctx.fillText("ZBar", CARD_PAD + markSize + 18, headY + 35);

  setFont(ctx, 54, 700);
  ctx.fillStyle = TEXT_PRIMARY;
  ctx.fillText(texts.title, CARD_PAD, headY + 116);

  setFont(ctx, 24, 400);
  ctx.fillStyle = TEXT_SECONDARY;
  ctx.fillText(
    `${mdLabel(view.fromMs).replace("-", ".")} – ${mdLabel(view.toMs).replace("-", ".")}`,
    CARD_PAD,
    headY + 152
  );

  // ===== KPI 四项：总 Token / 总花费 / 总请求 / 活跃天数 =====
  let y = headY + HEADER_H;
  const metricY = y;
  const metricCols = 4;
  const colW = (CARD_W - CARD_PAD * 2) / metricCols;

  roundRectPath(
    ctx,
    CARD_PAD,
    metricY,
    CARD_W - CARD_PAD * 2,
    METRICS_H - 30,
    24
  );
  ctx.fillStyle = "rgba(148, 163, 184, 0.08)";
  ctx.fill();

  const metricCell = (col: number, label: string, value: string) => {
    const cx = CARD_PAD + col * colW;
    setFont(ctx, 20, 500);
    ctx.fillStyle = TEXT_SECONDARY;
    ctx.fillText(label, cx + 24, metricY + 46);
    setFont(ctx, 46, 700);
    ctx.fillStyle = col === 1 ? ACCENT : TEXT_PRIMARY;
    ctx.fillText(fitText(ctx, value, colW - 44), cx + 24, metricY + 102);
  };
  metricCell(0, texts.totalTokens, formatTokensLocale(view.tokens, locale));
  metricCell(1, texts.totalCost, formatCost(view.cost, view.currency));
  metricCell(
    2,
    texts.totalRequests,
    formatCountLocale(view.requests, locale)
  );
  metricCell(
    3,
    texts.activeDays,
    `${view.activeDays}${texts.activeDaysUnit}`
  );
  y += METRICS_H;

  // ===== Agent 分布横条 =====
  if (showAgents) {
    y += SECTION_GAP;
    drawSectionTitle(ctx, texts.agentsTitle, y + 30);
    y += 44;
    view.agents.forEach((a) => {
      drawBarRow(
        ctx,
        y,
        ROW_H,
        a.name,
        a.color,
        a.color,
        a.share,
        `${formatTokensLocale(a.tokens, locale)} · ${Math.round(a.share * 100)}%`
      );
      y += ROW_H;
    });
    y += 18;
  }

  // ===== 模型 TOP5 横条（无有效价格的模型不标花费） =====
  if (showModels) {
    y += SECTION_GAP;
    drawSectionTitle(ctx, texts.topModels, y + 30);
    y += 44;
    const maxTokens = Math.max(...view.models.map((m) => m.tokens), 1);
    view.models.forEach((m) => {
      const valueText =
        m.cost != null
          ? `${formatTokensLocale(m.tokens, locale)} · ${formatCost(m.cost, view.currency)}`
          : formatTokensLocale(m.tokens, locale);
      drawBarRow(
        ctx,
        y,
        ROW_H,
        m.name,
        m.color,
        m.color,
        m.tokens / maxTokens,
        valueText
      );
      y += ROW_H;
    });
    y += 18;
  }

  // ===== Token 活动热力图（列=周、7 行、分位四档蓝色） =====
  if (showHeat) {
    y += SECTION_GAP;
    drawSectionTitle(ctx, texts.heatTitle, y + 30);
    y += 44;

    let heatMax = 0;
    for (const week of view.heat) {
      for (const cell of week.cells) {
        if (cell && cell.tokens > heatMax) heatMax = cell.tokens;
      }
    }

    const gridW = CARD_W - CARD_PAD * 2;
    const cellW = (gridW - HEAT_GAP * (heatRows - 1)) / heatRows;
    view.heat.forEach((week, ci) => {
      week.cells.forEach((cell, ri) => {
        if (!cell) return;
        const cx = CARD_PAD + ci * (cellW + HEAT_GAP);
        const cy = y + ri * (HEAT_CELL_H + HEAT_GAP);
        const level = heatLevel(cell.tokens, heatMax);
        roundRectPath(ctx, cx, cy, cellW, HEAT_CELL_H, 4);
        ctx.fillStyle = level === 0 ? HEAT_EMPTY : HEAT_COLORS[level - 1];
        ctx.fill();
      });
    });
    y += heatGridH;

    // 月份标签：锚在月份首列左缘（与页面热力图同规则）
    ctx.font = `500 18px ${FONT_STACK}`;
    ctx.fillStyle = TEXT_FAINT;
    let prevMonth = -1;
    view.heat.forEach((week, ci) => {
      const firstCell = week.cells.find((c) => c != null);
      if (!firstCell) return;
      const month = new Date(firstCell.dayMs).getMonth();
      if (month !== prevMonth) {
        ctx.fillText(
          monthShortLabel(month, locale),
          CARD_PAD + ci * (cellW + HEAT_GAP),
          y + 26
        );
      }
      prevMonth = month;
    });
    y += 44;
  }

  // ===== 底部落款 =====
  ctx.strokeStyle = "rgba(148, 163, 184, 0.16)";
  ctx.lineWidth = 1;
  ctx.beginPath();
  ctx.moveTo(CARD_PAD, H - CARD_PAD + 8);
  ctx.lineTo(CARD_W - CARD_PAD, H - CARD_PAD + 8);
  ctx.stroke();

  setFont(ctx, 22, 500);
  ctx.fillStyle = TEXT_FAINT;
  ctx.textAlign = "right";
  ctx.fillText(texts.footer, CARD_W - CARD_PAD, H - CARD_PAD + 46);
  ctx.textAlign = "left";

  return H;
}

// ===== 弹层组件 =====

interface ShareCardModalProps {
  onClose: () => void;
  /** 报告页价格表（切片后模型花费重算用） */
  pricing: PricingConfig;
  /** 报告页已聚合数据（agents/按日序列/范围/币种），打开即出图 */
  report: ShareReportData;
}

/**
 * 分享卡片弹层：复用报告页已聚合结果实时 canvas 预览，
 * 「保存图片」经系统对话框写盘（用户取消返回 null，静默处理）。
 */
export function ShareCardModal({
  onClose,
  pricing,
  report,
}: ShareCardModalProps) {
  const { t, locale } = useI18n();
  const [saving, setSaving] = useState(false);
  const [status, setStatus] = useState<string | null>(null);
  const canvasRef = useRef<HTMLCanvasElement | null>(null);

  const view = useMemo(() => buildView(report, pricing), [report, pricing]);

  const texts = useMemo<ShareTexts>(
    () => ({
      title: t("report.title"),
      totalCost: t("share.totalCost"),
      totalTokens: t("share.totalTokens"),
      totalRequests: t("share.totalRequests"),
      activeDays: t("report.kpi.activeDays"),
      activeDaysUnit: t("report.act.daysUnit"),
      agentsTitle: t("report.agentDist"),
      topModels: t("share.topModels"),
      heatTitle: t("share.heatTitle"),
      footer: t("share.footer"),
    }),
    [t]
  );

  // 数据/文案就绪后重绘（数据同步来自 props，打开即出图）
  useEffect(() => {
    if (canvasRef.current) {
      drawShareCard(canvasRef.current, view, texts, locale);
    }
  }, [view, texts, locale]);

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
        const name = `zbar-share-${ymdLabel(Date.now())}.png`;
        const path = await saveShareImage(bytes, name);
        // 用户取消（返回 null）静默处理；成功提示保存路径
        if (path) setStatus(t("share.saved", { path }));
      } catch (e) {
        setStatus(t("share.saveFail", { msg: String(e) }));
      } finally {
        setSaving(false);
      }
    }, "image/png");
  };

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/40">
      <div className="bg-elevated border border-slate-900/10 rounded-2xl shadow-xl p-3 w-[min(560px,94vw)] max-h-[92vh] flex flex-col">
        <div className="flex items-center justify-between mb-2 shrink-0">
          <span className="text-[12px] font-semibold text-slate-900">
            {t("share.title")}
          </span>
          <button
            onClick={onClose}
            className="toolbar-btn"
            title={t("share.close")}
          >
            ×
          </button>
        </div>
        <div className="flex-1 min-h-0 overflow-y-auto rounded-xl bg-slate-950/85 p-2 overscroll-contain">
          <canvas
            ref={canvasRef}
            className="w-full h-auto rounded-lg"
            aria-label={t("share.title")}
          />
        </div>
        <div className="flex items-center gap-1.5 mt-2 shrink-0">
          <span className="flex-1 min-w-0 text-[9px] text-slate-600 truncate">
            {status}
          </span>
          <BtnPrimary
            onClick={handleSave}
            disabled={saving}
            className="text-[10px]!"
          >
            {saving ? t("share.saving") : t("share.save")}
          </BtnPrimary>
        </div>
      </div>
    </div>
  );
}
