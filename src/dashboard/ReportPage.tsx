/**
 * 用量报告页（主窗口大窗版，300px 窄窗 ReportContent 的大窗宿主）。
 *
 * 原悬浮窗「报表页」是双 tab 壳（ReportsPanel：tab1 周额度对比 → 已由
 * ComparePage 独立承接；tab2 用量报告 → 本页承接）。壳随悬浮窗「报表」
 * 菜单入口一起移除，本页直接挂载 ReportPanel.tsx 导出的 ReportContent
 * 本体（数据拉取 / KPI / 热力图 / 趋势 / 模型表 / 导出 / 分享卡片逻辑
 * 全部保留在组件内，零改动复用）。
 *
 * 数据依赖与 ReportsPanel 壳的现用法一致：
 * - pricing：挂载时 fetchPricing 一次（价格表更新经面板/定价页保存后，
 *   下次进入本页重挂载即取新值；报告页不长期驻留，不做事件跟随）；
 * - currency：主窗口全局币种偏好（useCurrencyPref，设置页手动指定 /
 *   跟随语言，跨窗事件自动跟随）；
 * - agentVisibility：由 DashboardApp 传入（useAgentVisibilitySync 持有，
 *   与设置页 / 悬浮窗 tab 显隐同源）。
 *
 * 版面：对齐请求日志页的先例（.db-logs-page）——本页不整页滚动，
 * .db-report-page 撑满主区高度，ReportContent 自带 PageBody 内滚
 * （长报告只滚内容区，工具行 / Agent 筛选行的位置关系与窄窗一致）。
 */
import { useEffect, useState } from "react";
import { useI18n } from "../i18n";
import { fetchPricing } from "../api";
import type { PricingConfig } from "../types";
import type { AgentVisibility } from "../agentVisibility";
import { ReportContent } from "../ReportPanel";
import { useCurrencyPref } from "./settings/currencyPref";

export function ReportPage({
  agentVisibility,
}: {
  agentVisibility: AgentVisibility;
}) {
  const { t } = useI18n();
  // 全局币种（设置页可手动指定；默认跟随语言）。ReportContent 据此折算
  // 花费口径，切换设置页币种后经 zbar://currency-changed 自动跟随
  const { currency } = useCurrencyPref();
  // 价格表：ReportContent 按模型单价估算花费。初值给空表（组件内各处
  // 对缺失单价已按 0 处理），挂载后拉取替换
  const [pricing, setPricing] = useState<PricingConfig>({ usd: {} });

  useEffect(() => {
    let cancelled = false;
    fetchPricing()
      .then((config) => {
        if (!cancelled) setPricing(config);
      })
      .catch(() => {
        /* 静默：花费按 0 单价展示，报告主数据不受影响 */
      });
    return () => {
      cancelled = true;
    };
  }, []);

  return (
    <div className="db-page db-report-page">
      <header className="db-page__header">
        <div>
          <h1 className="db-page__title">{t("dashboard.report")}</h1>
          <p className="db-page__desc">{t("dashboard.report.desc")}</p>
        </div>
      </header>
      {/* ReportContent 根节点自带 report-scope + flex-1 min-h-0 flex-col
          （工具行 / 告警条 shrink + PageBody 内滚），由 .db-report-embed
          给出确定的剩余高度与宽度约束 */}
      <div className="db-report-embed">
        <ReportContent
          pricing={pricing}
          currency={currency}
          agentVisibility={agentVisibility}
        />
      </div>
    </div>
  );
}
