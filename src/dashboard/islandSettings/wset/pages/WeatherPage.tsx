/**
 * 页⑧「天气」：数据源（可选的 OpenWeather Key / 单位 / 刷新间隔）、绑定城市
 */
import { useState } from "react";
import { CityPickerModal } from "../../../../island/cityPickerModal";
import { addWeatherCity, addWeatherCityExact, weatherLocateNow } from "../../../../island/api";
import {
  Button,
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
  errorText,
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

/** 弹层来源：区分「绑定城市」(写草稿) 与「城市列表」(直加命令)，共用一份选择器。 */
type PickerSource = "bind" | "list" | null;

export function WeatherPage({ draft }: PageProps) {
  // 城市选择弹层开关；绑定城市 / 城市列表复用同一个选择器，按来源分派结果。
  const [picker, setPicker] = useState<PickerSource>(null);
  // 页面级提示（定位失败 / 直加失败）。成功由状态栏 / 自动回刷体现。
  const [hint, setHint] = useState<{ text: string; tone: "ok" | "error" } | null>(null);
  // 命令执行中，防止重复提交。
  const [busy, setBusy] = useState(false);

  const flash = (text: string, tone: "ok" | "error") => setHint({ text, tone });

  /** 绑定城市：选中 / 定位成功后只写名字，带坐标时同步写入 lat/lon 草稿。 */
  const applyBind = (name: string, lat?: number, lon?: number) => {
    draft.setValue("weather.city", name);
    if (lat != null && lon != null) {
      draft.setMany({ "weather.lat": lat, "weather.lon": lon });
    }
    setPicker(null);
  };

  /** 城市列表：选中 / 定位成功后调岛内命令直加（不插入空行）。 */
  const addExact = async (city: {
    name: string;
    province: string;
    lat: number;
    lon: number;
  }) => {
    if (busy) return;
    setBusy(true);
    try {
      const message = await addWeatherCityExact(city);
      flash(message, "ok");
      setPicker(null);
      // 命令经 apply_patch 落盘并广播 zbar://island-config，草稿会被静默回刷。
    } catch (error) {
      flash(errorText(error), "error");
      throw error;
    } finally {
      setBusy(false);
    }
  };

  /** 绑定城市行「当前位置」按钮：定位成功后写 binding 草稿（弹层内的定位走
   *  CityPickerModal → onPick 分派，不走这里）。 */
  const locateForBind = async () => {
    try {
      const place = await weatherLocateNow();
      if (!place.ok || (!place.city && !place.province)) {
        flash(place.error || "定位失败，请手动选择城市", "error");
        return;
      }
      const name = place.city.trim() || place.province.trim();
      applyBind(name, place.lat, place.lon);
    } catch (error) {
      flash(errorText(error), "error");
    }
  };

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
          desc="留空时主城市跟随公网 IP；填写城市名即把它绑定为主城市，天气页顶部固定显示它。可用「当前位置」或「选择」快速填入，也可保留手动输入兜底。"
          rowKey="weather.city"
        >
          <div className="wset-city-bind">
            <div className="wset-city-bind__input">
              <TextInput
                value={readText(draft, "weather.city")}
                width={230}
                placeholder="留空跟随 IP，如 上海"
                ariaLabel="绑定城市"
                onChange={(next) => draft.setValue("weather.city", next)}
              />
            </div>
            <Button size="sm" onClick={() => void locateForBind()} title="用当前位置作为绑定城市">
              当前位置
            </Button>
            <Button size="sm" onClick={() => setPicker("bind")} title="从省市列表选择绑定城市">
              选择
            </Button>
          </div>
        </Row>
        <Row
          wide
          title="城市列表"
          desc="这里只维护你自己添加的城市。点「添加城市」用当前位置或省市联动直加，无需手动填坐标；已有行仍可改名 / 改坐标 / 排序 / 删除。坐标留空时按城市名自动查询，也可在岛天气页点 + 添加。"
          error={draft.errors["weather.cities"]}
          rowKey="weather.cities"
        >
          <RecordEditor
            items={readRecords(draft, "weather.cities")}
            fields={CITY_FIELDS}
            newItem={CITY_NEW_ITEM}
            addLabel="添加城市"
            onAdd={() => setPicker("list")}
            onChange={(next) => draft.setValue("weather.cities", next)}
          />
          {hint ? (
            <div
              className={`wset-city-hint ${hint.tone === "error" ? "wset-city-hint--error" : ""}`}
              role="status"
            >
              {hint.text}
            </div>
          ) : null}
        </Row>
      </Section>

      <PageResetBar onReset={() => draft.resetPage(PAGE_FIELDS[PAGE])} />

      {picker ? (
        <CityPickerModal
          title={picker === "bind" ? "选择绑定城市" : "添加城市"}
          busy={busy}
          onClose={() => setPicker(null)}
          onPick={(city) => {
            if (picker === "bind") {
              applyBind(city.name, city.lat, city.lon);
            } else {
              void addExact(city).catch(() => {
                /* 错误已由 addExact 行内 flash 提示 */
              });
            }
          }}
          onManual={
            picker === "list"
              ? async (query) => {
                  // 手动输入兜底（海外城市 / 纬度,经度）：走在线 geocode 命令。
                  try {
                    const message = await addWeatherCity(query);
                    flash(message, "ok");
                    setPicker(null);
                  } catch (error) {
                    flash(errorText(error), "error");
                    throw error;
                  }
                }
              : undefined
          }
        />
      ) : null}
    </>
  );
}
