/**
 * 主窗口设置页「数据来源」分区：各 Agent 的统计展示开关（自悬浮窗设置
 * 面板迁移）。分组按**数据来源**（AGENT_GROUP）切分而非数组下标、也非
 * 「是否凭证驱动」——kimi 虽有独立凭证体系（属 CREDENTIAL_AGENTS），但
 * 用量数据来自本地 ~/.kimi-code/sessions，仍属本地采集（详见
 * agentVisibility.ts 的 AGENT_GROUP 注释）。
 *
 * 与悬浮窗版的差异：凭证型服务的行内「添加」快捷按钮不迁——主窗口的
 * 凭证管理在「凭证与账号」页，这里改为文案引导 + 跳转按钮，避免两处
 * 入口。开关写入由 useAgentVisibilitySync 的 setAgentVisible 承接
 * （manually-disabled 标记 + localStorage + Rust 镜像 + 跨窗广播）。
 */
import { useI18n } from "../../i18n";
import { BrandIcon } from "../../BrandIcon";
import {
  AGENT_VISIBILITY_OPTIONS,
  isLocalCollectAgent,
  type AgentId,
  type AgentVisibility,
} from "../../agentVisibility";
import { ToggleSwitch } from "./controls";

export function SourcesSettings({
  agentVisibility,
  onAgentVisibilityChange,
  onOpenAccounts,
}: {
  agentVisibility: AgentVisibility;
  onAgentVisibilityChange: (id: AgentId, visible: boolean) => void;
  /** 跳转主窗口「凭证与账号」页（凭证添加/管理的统一入口） */
  onOpenAccounts: () => void;
}) {
  const { t } = useI18n();

  return (
    <section className="db-set-row db-set-row--block">
      <div className="db-set-row__text">
        <h2 className="db-set-row__title">{t("settings.sources")}</h2>
        <p className="db-set-row__desc">{t("settings.sourcesHint")}</p>
      </div>
      <div className="db-set-blockbody">
        {(
          [
            [
              "settings.groupLocal",
              AGENT_VISIBILITY_OPTIONS.filter((a) => isLocalCollectAgent(a.id)),
            ],
            [
              "settings.groupCredential",
              AGENT_VISIBILITY_OPTIONS.filter((a) => !isLocalCollectAgent(a.id)),
            ],
          ] as const
        ).map(([groupKey, agents]) => (
          <div key={groupKey} className="db-set-agent__group">
            <div className="db-set-agent__group-title">{t(groupKey)}</div>
            {agents.map((agent) => (
              <div key={agent.id} className="db-set-agent">
                <span className="db-set-agent__text">
                  <BrandIcon
                    brand={agent.id}
                    className="db-set-agent__icon"
                  />
                  <span className="db-set-agent__names">
                    <span className="db-set-agent__name">{agent.label}</span>
                    <span className="db-set-agent__desc">
                      {t(agent.descriptionKey)}
                    </span>
                  </span>
                </span>
                <ToggleSwitch
                  checked={agentVisibility[agent.id]}
                  title={agent.label}
                  onChange={(next) => onAgentVisibilityChange(agent.id, next)}
                />
              </div>
            ))}
            {/* 凭证接入组的引导：悬浮窗版行内「添加」快捷按钮不迁，凭证
                添加/管理统一到本窗「凭证与账号」页 */}
            {groupKey === "settings.groupCredential" && (
              <div className="db-set-agent__cta">
                <span className="db-set-note">
                  {t("dashboard.settings.sourcesAddHint")}
                </span>
                <button type="button" className="db-btn" onClick={onOpenAccounts}>
                  {t("dashboard.settings.sourcesGoAccounts")}
                </button>
              </div>
            )}
          </div>
        ))}
      </div>
    </section>
  );
}
