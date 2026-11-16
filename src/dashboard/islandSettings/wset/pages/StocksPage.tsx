/**
 * 页⑦「股票」：自选股、行情刷新间隔与涨跌幅提醒。
 */
import {
  Note,
  PageHeader,
  PageResetBar,
  Row,
  Section,
  Stepper,
  StringList,
} from "../controls";
import { PAGE_FIELDS, type PageKey } from "../schema";
import {
  readNumber,
  readStringList,
  UnsupportedTag,
  type PageProps,
} from "./common";

const PAGE: PageKey = "stocks";

export function StocksPage({ draft }: PageProps) {
  return (
    <>
      <PageHeader title="股票" desc="自选股、行情刷新间隔与涨跌幅提醒。" />

      <Section title="自选">
        <Row
          wide
          title="自选股"
          desc="A 股 sh / sz / bj + 6 位；港股 hk + 5 位；美股 us + 代码。也可以在股票页点 + 搜索添加。"
          error={draft.errors["stocks.watchlist"]}
          tag={<UnsupportedTag draft={draft} keyName="stocks.watchlist" />}
          rowKey="stocks.watchlist"
        >
          <StringList
            values={readStringList(draft, "stocks.watchlist")}
            placeholder="例如 sh600519"
            addLabel="添加"
            onChange={(next) => draft.setValue("stocks.watchlist", next)}
          />
        </Row>
      </Section>

      <Section title="刷新与提醒">
        <Row
          title="行情刷新间隔"
          desc="交易时段内的轮询间隔；非交易时段会自动放慢。"
          error={draft.errors["stocks.interval"]}
          tag={<UnsupportedTag draft={draft} keyName="stocks.interval" />}
          rowKey="stocks.interval"
        >
          <Stepper
            value={readNumber(draft, "stocks.interval", 5)}
            min={2}
            max={60}
            suffix="秒"
            ariaLabel="行情刷新间隔"
            onChange={(next) => draft.setValue("stocks.interval", next)}
          />
        </Row>
        <Row
          title="涨跌幅提醒"
          desc="单只股票涨跌幅超过它时弹出提醒（每只每天每个方向一次）；0 = 关闭。"
          error={draft.errors["stocks.pct_alert"]}
          tag={<UnsupportedTag draft={draft} keyName="stocks.pct_alert" />}
          rowKey="stocks.pct_alert"
        >
          <Stepper
            value={readNumber(draft, "stocks.pct_alert", 3)}
            min={0}
            max={20}
            step={0.5}
            decimals={1}
            suffix="%"
            ariaLabel="涨跌幅提醒"
            onChange={(next) => draft.setValue("stocks.pct_alert", next)}
          />
        </Row>
        <Note>
          到价提醒（stocks.alerts）和自定义提醒文案目前只能在 settings.json 里编辑：高级 → 打开 settings.json。
        </Note>
      </Section>

      <PageResetBar onReset={() => draft.resetPage(PAGE_FIELDS[PAGE])} />
    </>
  );
}
