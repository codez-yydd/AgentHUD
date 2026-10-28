/**
 * 模型用量表：每个模型一行——模型名、请求数、输入侧 / 输出 Token、总 Token、
 * 范围内花费、Token 占比（数字 + 细进度条），按范围内总 Token 降序。
 *
 * 口径与数据来源（派生见 derive.buildModelRows / derive.modelRowKey）：
 *  - 由各 Agent 日聚合的 by_model 按当前时间范围切片后按**模型折叠键**跨 Agent
 *    合并：canonicalModelId(displayModelName(model_id))——既折叠「GLM-5.3 /
 *    glm-5.3」这类字节差异写法，也把带 provider 前缀的 id 与裸名视为同一模型
 *    （zhipuai-coding-plan/glm-5.3-flash 与 glm-5.3-flash 合并；本表是跨来源
 *    合并视图，显示名相同的行不应分成两行）；
 *  - 模型名渲染走 displayModelName（与报告页 / 模型徽标同口径：第三方 CLI 拼进
 *    id 的 provider 前缀不在界面上显示），单元格 title 给出完整写法；该行由多个
 *    写法折叠而来时 title 追加各写法及请求数（含带前缀的写法，可看出是折叠结果）；
 *  - 成本按写法分别折算：非 Cursor 写法用本机价格表（与菜单栏标题同源，非账单
 *    金额），Cursor 写法用其后端算好的 cost_usd；价格表未覆盖的写法成本按 0
 *    显示「未定价」；
 *  - 「未拆分」行 = 旧数据 / 无逐模型拆分的来源（Cursor daily 除外，其模型数据
 *    走 by_model 旁路），用量计入输入侧且不计花费；
 *  - 输入侧列表头注明「含缓存读」（与指标卡「输入侧」同口径：总量 − 输出）。
 *
 * 行数多时默认只展示前 COLLAPSED_ROWS 行，「展开全部 N 个模型 / 收起」由
 * 文本按钮切换（纯前端状态，不重新取数）。空数据显示 db-empty 空态。
 */
import { formatCost, formatTokensLocale } from "../../format";
import { useI18n, type TFn } from "../../i18n";
import { dateLocale } from "../../i18n/locale";
import { displayModelName } from "../../modelName";
import type { Currency } from "../../types";
import { costOf, type ModelRow } from "./derive";
import { Card } from "./primitives";

/** 折叠时默认展示的行数（按总 Token 降序取前 N 行） */
const COLLAPSED_ROWS = 10;

/** 占比文案：≥0.1% 显示一位小数，更小但非零显示 "<0.1%"（避免整列 0.0%），
 *  0 显示 "0%"（分母为 0 时也不会出现负值 / NaN） */
function shareText(share: number): string {
  const pct = share * 100;
  if (!(pct > 0)) return "0%";
  if (pct < 0.1) return "<0.1%";
  return `${pct.toFixed(1)}%`;
}

/**
 * 模型名单元格 tooltip：完整 model_id（rep 写法；可见标签是去前缀后的名字，
 * 完整 id 只在悬停时给出）；该行由多个写法折叠而来时追加一行列出各原写法与
 * 请求数（分隔符用与总览页失败名单同款的 " · "，与语言无关）。
 */
function modelCellTitle(row: ModelRow, t: TFn, intlLocale: string): string {
  if (!row.variants || row.variants.length === 0) return row.modelId;
  const list = row.variants
    .map((variant) =>
      t("dashboard.overview.modelVariantItem", {
        model: variant.model_id,
        count: variant.requests.toLocaleString(intlLocale),
      })
    )
    .join(" · ");
  return `${row.modelId}\n${t("dashboard.overview.modelVariantsHint", { list })}`;
}

export function ModelTable({
  rows,
  currency,
  rangeLabel,
  agentName,
  onExport,
}: {
  rows: ModelRow[];
  currency: Currency;
  /** 自定义区间摘要（MM-DD ~ MM-DD；预设档不传）：作为卡片副标题前缀，
   *  「范围内」口径由此可读（与 AgentTable 同款惯例） */
  rangeLabel?: string;
  /** Agent 筛选生效时的品牌名（未筛选不传）：与本表的 rangeLabel 一样走
   *  独立 prop（本表跟随筛选、AgentTable 不跟随，共用 rangeLabel 会反向
   *  污染 Agent 表的口径说明），拼进副标题前缀链 */
  agentName?: string;
  /** 「导出完整图片」入口（全部模型行，不受折叠/卡片高度限制） */
  onExport?: () => void;
}) {
  const { locale, t } = useI18n();
  const intlLocale = dateLocale(locale);
  const totalTokens = rows.reduce((sum, row) => sum + row.total, 0);

  return (
    <Card
      title={t("dashboard.overview.modelTitle")}
      frame={{ window: "dashboard", page: "overview", id: "overview.models" }}
      summary={t("customize.summaryModels", {
        count: rows.length,
        tokens: formatTokensLocale(totalTokens, locale),
      })}
      list={{ total: rows.length, collapsedCount: COLLAPSED_ROWS }}
      action={
        onExport && rows.length > 0 ? (
          <button
            type="button"
            className="db-linkbtn"
            onClick={onExport}
            title={t("export.entry")}
          >
            <span aria-hidden="true">⤓</span> {t("export.entry")}
          </button>
        ) : undefined
      }
      // 副标题前缀链：自定义区间摘要 · Agent 名 · 既有排序说明（各段独立
      //  可选，" · " 分隔与本页失败名单等惯例一致；与 TrendCard 的词条分支
      //  不同，这里末段是固定短语，直接拼接即可，无需专用词条）
      sub={
        [rangeLabel, agentName, t("dashboard.overview.modelSub")]
          .filter((part) => part != null)
          .join(" · ")
      }
    >
      {({ listExpanded }) => {
        const visible = listExpanded ? rows : rows.slice(0, COLLAPSED_ROWS);
        return rows.length === 0 ? (
          <div className="db-empty">{t("dashboard.overview.modelEmpty")}</div>
        ) : (
          <>
            <div className="db-table-wrap">
              <table className="db-table">
                <thead>
                  <tr>
                    <th>{t("dashboard.overview.colModel")}</th>
                    <th className="db-table__num">
                      {t("dashboard.overview.colRequests")}
                    </th>
                    <th
                      className="db-table__num"
                      title={t("dashboard.overview.colModelInputHint")}
                    >
                      {t("dashboard.overview.colModelInput")}
                    </th>
                    <th className="db-table__num">
                      {t("dashboard.overview.colModelOutput")}
                    </th>
                    <th className="db-table__num">
                      {t("dashboard.overview.colModelTotal")}
                    </th>
                    <th className="db-table__num">
                      {t("dashboard.overview.colCost")}
                    </th>
                    <th className="db-table__share">
                      {t("dashboard.overview.colShare")}
                    </th>
                  </tr>
                </thead>
                <tbody>
                  {visible.map((row) => (
                    <tr key={row.unsplit ? "__unsplit__" : row.key}>
                      <td>
                        {row.unsplit ? (
                          <span
                            className="db-model__unsplit"
                            title={t("dashboard.overview.modelUnsplitHint")}
                          >
                            {t("dashboard.overview.modelUnsplit")}
                          </span>
                        ) : (
                          <span
                            className="db-model__name"
                            title={modelCellTitle(row, t, intlLocale)}
                          >
                            {displayModelName(row.modelId)}
                          </span>
                        )}
                      </td>
                      <td className="db-table__num num">
                        {row.requests > 0
                          ? row.requests.toLocaleString(intlLocale)
                          : "—"}
                      </td>
                      {/* 未拆分行：token 无输入/输出拆分，整份计入输入侧（输出列
                          显示占位符，不把未知伪装成 0） */}
                      <td className="db-table__num num">
                        {formatTokensLocale(row.input, locale)}
                      </td>
                      <td className="db-table__num num">
                        {row.unsplit || row.output <= 0
                          ? "—"
                          : formatTokensLocale(row.output, locale)}
                      </td>
                      <td className="db-table__num num">
                        {formatTokensLocale(row.total, locale)}
                      </td>
                      <td className="db-table__num num">
                        {costCellText(row, currency, t)}
                      </td>
                      <td className="db-table__share">
                        <div className="db-share">
                          <span className="db-share__pct num">
                            {shareText(row.share)}
                          </span>
                          <div className="db-share__track">
                            <div
                              className="db-share__fill"
                              style={{ width: `${Math.min(100, row.share * 100)}%` }}
                            />
                          </div>
                        </div>
                      </td>
                    </tr>
                  ))}
                </tbody>
              </table>
            </div>
            <p className="db-modeltable__foot">
              {t("dashboard.overview.modelFootnote")}
            </p>
          </>
        );
      }}
    </Card>
  );
}

/** 成本单元格：有成本显示金额；有用量但价格表未覆盖时与 Agent 表同款显示
 *  「未定价」；「未拆分」行不参与成本折算，显示占位符——避免把"没配价"或
 *  "无法折算"伪装成"不花钱"。 */
function costCellText(
  row: ModelRow,
  currency: Currency,
  t: (key: "dashboard.overview.unpriced") => string
): string {
  if (row.unsplit) return "—";
  const cost = costOf(row.costUsd, row.costCny, currency);
  if (cost > 0) return formatCost(cost, currency);
  if (row.total > 0) return t("dashboard.overview.unpriced");
  return "—";
}
