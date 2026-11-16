/**
 * 页⑧「天气」：数据源（可选的 OpenWeather Key / 单位 / 刷新间隔）、绑定城市
 */
import {
  PageHeader,
  PageResetBar,
  RecordEditor,
  Row,
  Section,
  Select,
  Stepper,
  TextInput,
} from "../controls";
import { PAGE_FIELDS, type PageKey } from "../schema";
import {
  readNumber,
  readRecords,
  readText,
  UnsupportedTag,
  type PageProps,
} from "./common";

const PAGE: PageKey = "weather";

const UNIT_OPTIONS = [
  { value: "metric", label: "摄氏度 °C" },
  { value: "imperial", label: "华氏度 °F" },
];

const CITY_FIELDS = [
  { key: "name", placeholder: "城市名，如 上海", width: 170 },
  { key: "lat", kind: "number" as const, placeholder: "纬度（可留空）", width: 130 },
  { key: "lon", kind: "number" as const, placeholder: "经度（可留空）", width: 130 },
];

const CITY_NEW_ITEM = { name: "", lat: null, lon: null };

export function WeatherPage({ draft }: PageProps) {
  return (
    <>
      <PageHeader title="天气" desc="天气数据源、温度单位、刷新间隔与城市列表。" />

      <Section title="数据源">
        <Row
          title="OpenWeather API Key"
          desc="填了 Key 走 OpenWeather（当前天气 + 3 小时 / 5 天预报），网络或 Key 失败自动回退 Open-Meteo；留空则只用免费、无需 Key 的 Open-Meteo。"
          tag={<UnsupportedTag draft={draft} keyName="weather.api_key" />}
          rowKey="weather.api_key"
        >
          <TextInput
            value={readText(draft, "weather.api_key")}
            width={250}
            secret
            ariaLabel="OpenWeather API Key"
            onChange={(next) => draft.setValue("weather.api_key", next)}
          />
        </Row>
        <Row
          title="温度单位"
          desc="对 OpenWeather 与 Open-Meteo 都生效：选华氏度时天气页按 °F 显示，摄氏度时按 ° 显示。"
          error={draft.errors["weather.units"]}
          tag={<UnsupportedTag draft={draft} keyName="weather.units" />}
          rowKey="weather.units"
        >
          <Select
            value={readText(draft, "weather.units", "metric")}
            width={150}
            options={UNIT_OPTIONS}
            ariaLabel="温度单位"
            onChange={(next) => draft.setValue("weather.units", next)}
          />
        </Row>
        <Row
          title="刷新间隔"
          desc="对所有数据源生效：后台按此间隔自动取数（5–180 分钟），天气变化慢，不建议少于 5 分钟。"
          error={draft.errors["weather.interval_min"]}
          tag={<UnsupportedTag draft={draft} keyName="weather.interval_min" />}
          rowKey="weather.interval_min"
        >
          <Stepper
            value={readNumber(draft, "weather.interval_min", 15)}
            min={5}
            max={180}
            step={5}
            suffix="分钟"
            ariaLabel="天气刷新间隔"
            onChange={(next) => draft.setValue("weather.interval_min", next)}
          />
        </Row>
      </Section>

      <Section title="城市">
        <Row
          title="绑定城市（留空跟随 IP）"
          desc="留空时主城市跟随公网 IP；填写城市名即把它绑定为主城市，天气页顶部固定显示它。"
          rowKey="weather.city"
        >
          <TextInput
            value={readText(draft, "weather.city")}
            width={230}
            placeholder="留空跟随 IP，如 上海"
            ariaLabel="绑定城市"
            onChange={(next) => draft.setValue("weather.city", next)}
          />
        </Row>
        <Row
          wide
          title="城市列表"
          desc="这里只维护你自己添加的城市。天气页顶部会显示主城市 / 当前 IP 城市 / 自定义城市，右键胶囊可绑定主城市或删除；坐标留空时按城市名自动查询，也可以在天气页点 + 搜索添加。"
          error={draft.errors["weather.cities"]}
          rowKey="weather.cities"
        >
          <RecordEditor
            items={readRecords(draft, "weather.cities")}
            fields={CITY_FIELDS}
            newItem={CITY_NEW_ITEM}
            addLabel="添加城市"
            onChange={(next) => draft.setValue("weather.cities", next)}
          />
        </Row>
      </Section>

      <PageResetBar onReset={() => draft.resetPage(PAGE_FIELDS[PAGE])} />
    </>
  );
}
