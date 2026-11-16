/**
 * 页②「排序」：账号在收起胶囊额度环 / 主页额度卡里的先后顺序。
 */
import { agentLabel, BrandMark } from "../../../../island/ui";
import type { HudBarAgent } from "../../../../island/types";
import { useOrderedAgents } from "../../useAgentUsage";
import { Button, IconButton, Note, PageHeader, Row, Section } from "../controls";
import { ArrowDownIcon, ArrowUpIcon } from "../icons";
import type { PageProps } from "./common";
import { UnsupportedTag } from "./common";

/** 排序表所在点路径 */
const ORDER_KEY = "agenthud.agent_order";

/** 账号副标题：账号名 / 套餐（缺一省略，两个都空返回空串） */
function agentSubtitle(agent: HudBarAgent): string {
  return [agent.account, agent.plan]
    .filter((part): part is string => Boolean(part && part.trim()))
    .join(" · ");
}

export function OrderPage({ draft }: PageProps) {
  const orderValue = draft.value(ORDER_KEY);
  const order = Array.isArray(orderValue)
    ? orderValue.filter((item): item is string => typeof item === "string")
    : [];
  const agents = useOrderedAgents(order);
  const hasCustomOrder = order.length > 0;

  /** 上移 / 下移一位：把当前显示顺序整份写进草稿（含新账号 id，便于稳定复现） */
  const move = (index: number, delta: number) => {
    const to = index + delta;
    if (to < 0 || to >= agents.length) return;
    const ids = agents.map((agent) => agent.id);
    const [moving] = ids.splice(index, 1);
    ids.splice(to, 0, moving);
    draft.setValue(ORDER_KEY, ids);
  };

  return (
    <>
      <PageHeader
        title="排序"
        desc="收起胶囊里的同心额度环和主页额度卡按这里的顺序排列；新出现的账号排到最后。"
      />

      <Section title="账号顺序">
        <Row
          wide
          title="账号顺序"
          desc="用 ↑↓ 调整先后；改动后点「保存并应用」生效，并同步到灵动岛的收起态与主页。"
          tag={<UnsupportedTag draft={draft} keyName={ORDER_KEY} />}
          rowKey={ORDER_KEY}
        >
          {agents.length === 0 ? (
            <p className="wset-empty">
              还没读到账号数据 —— 灵动岛启动并拿到首帧额度快照后，这里会自动出现。
            </p>
          ) : (
            <div className="wset-agents">
              {agents.map((agent, index) => {
                const subtitle = agentSubtitle(agent);
                return (
                  <div className="wset-agent" key={agent.id}>
                    <span className="wset-agent__icon">
                      <BrandMark source={agent.source} size={18} />
                    </span>
                    <span className="wset-agent__text">
                      <b>{agentLabel(agent.source)}</b>
                      {subtitle ? <span>{subtitle}</span> : null}
                    </span>
                    <span className="wset-agent__ops">
                      <IconButton
                        size={24}
                        ariaLabel={`${agentLabel(agent.source)} 上移`}
                        title="上移"
                        disabled={index === 0}
                        onClick={() => move(index, -1)}
                      >
                        <ArrowUpIcon size={12} />
                      </IconButton>
                      <IconButton
                        size={24}
                        ariaLabel={`${agentLabel(agent.source)} 下移`}
                        title="下移"
                        disabled={index === agents.length - 1}
                        onClick={() => move(index, 1)}
                      >
                        <ArrowDownIcon size={12} />
                      </IconButton>
                    </span>
                  </div>
                );
              })}
            </div>
          )}

          <div className="wset-inline-foot">
            <Button
              size="sm"
              disabled={!hasCustomOrder || agents.length === 0}
              onClick={() => draft.setValue(ORDER_KEY, [])}
            >
              恢复默认顺序
            </Button>
          </div>
          <Note>
            顺序保存在 settings.json 的 agenthud.agent_order 里，应用重启后仍然有效；
            「恢复默认顺序」只是清空自定义顺序，不会删除任何账号。
          </Note>
        </Row>
      </Section>
    </>
  );
}
