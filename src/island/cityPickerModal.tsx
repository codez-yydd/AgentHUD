/**
 * 共享城市选择弹层：省市联动 + 搜索 + 使用当前位置 + 手动输入兜底。
 *
 * 岛内天气页（src/island/pages.tsx）与灵动岛设置窗天气页
 * （src/dashboard/islandSettings/wset/pages/WeatherPage.tsx）复用同一份交互。
 * 两份入口样式表各自定义同名类的外观（island.css / island-settings.css），
 * 组件本身不内联样式，避免与任何一边的主题变量打架。
 */

import { useEffect, useMemo, useRef, useState } from "react";
import { weatherLocateNow } from "./api";
import { CHINA_PROVINCES, type ChinaCity, type ChinaProvince } from "./chinaCities";
import { Icon } from "./ui";

/** 带坐标的城市（省市联动 / 当前位置直选），与后端 island_weather_add_city_exact 入参一致 */
export interface CityPick {
  name: string;
  province: string;
  lat: number;
  lon: number;
}

export function CityPickerModal({
  onPick,
  onManual,
  onClose,
  busy = false,
  title = "添加城市",
  manualPlaceholder = "城市名或 纬度,经度",
}: {
  /** 省市联动 / 当前位置选中城市后的回调（带坐标） */
  onPick: (city: CityPick) => void | Promise<void>;
  /** 底部手动输入（海外 / 兜底）提交后的回调；不传则隐藏手动区 */
  onManual?: (query: string) => void | Promise<void>;
  /** 关闭弹层（同时负责清掉调用方的弹层开关） */
  onClose: () => void;
  /** 外部忙碌态（保存中 / 直加中）：禁用所有提交按钮 */
  busy?: boolean;
  title?: string;
  manualPlaceholder?: string;
}) {
  const [province, setProvince] = useState<ChinaProvince | null>(null);
  const [cityQuery, setCityQuery] = useState("");
  const [manualOpen, setManualOpen] = useState(false);
  const [query, setQuery] = useState("");
  const [locating, setLocating] = useState(false);
  const [locateError, setLocateError] = useState("");
  const [manualError, setManualError] = useState("");
  const [localBusy, setLocalBusy] = useState(false);
  const addInputRef = useRef<HTMLInputElement | null>(null);
  const working = busy || localBusy;

  // 手动输入框展开后自动聚焦（面板展开动画期间也能立即输入）。
  useEffect(() => {
    if (!manualOpen) return;
    const timer = window.setTimeout(() => addInputRef.current?.focus(), 0);
    return () => window.clearTimeout(timer);
  }, [manualOpen]);

  // Esc 关闭弹层。用捕获阶段 + stopImmediatePropagation 抢在设置窗「Esc 关窗」
  // 的 window 监听器之前，避免按 Esc 连弹层和整个设置窗一起关掉。
  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if (event.key !== "Escape") return;
      event.preventDefault();
      event.stopImmediatePropagation();
      onClose();
    };
    window.addEventListener("keydown", onKey, true);
    return () => window.removeEventListener("keydown", onKey, true);
  }, [onClose]);

  // 省级列表 / 选中省的市列表：按搜索文本离线 contains 过滤（与抽取前一致）。
  const provinceQuery = cityQuery.trim().toLowerCase();
  const shownProvinces = useMemo(
    () =>
      provinceQuery
        ? CHINA_PROVINCES.filter(
            (p) =>
              p.name.toLowerCase().includes(provinceQuery) ||
              p.cities.some((c) => c.name.toLowerCase().includes(provinceQuery)),
          )
        : CHINA_PROVINCES,
    [provinceQuery],
  );
  const shownCities = useMemo(() => {
    if (!province) return [];
    if (province.name.toLowerCase().includes(provinceQuery)) return province.cities;
    return province.cities.filter((c) => c.name.toLowerCase().includes(provinceQuery));
  }, [province, provinceQuery]);

  /** 带坐标直选 / 手动提交共用的忙碌包裹（外部回调自己负责提示与关闭弹层）。 */
  const runSubmit = async (action: () => void | Promise<void>) => {
    if (working) return;
    setLocalBusy(true);
    try {
      await action();
    } finally {
      setLocalBusy(false);
    }
  };

  /** 「使用当前位置」：手动定位一次，成功后以返回的省市与经纬度回调。 */
  const useCurrentLocation = async () => {
    if (locating || working) return;
    setLocating(true);
    setLocateError("");
    try {
      const place = await weatherLocateNow();
      if (!place.ok || (!place.city && !place.province)) {
        setLocateError(place.error || "定位失败，请手动选择城市");
        return;
      }
      const name = place.city.trim() || place.province.trim();
      await onPick({
        name,
        province: place.province,
        lat: place.lat,
        lon: place.lon,
      });
    } catch (error) {
      // 定位失败不关闭弹层，仅行内提示。
      setLocateError(String(error));
    } finally {
      setLocating(false);
    }
  };

  const pickCity = (provinceName: string, city: ChinaCity) => {
    void runSubmit(async () => {
      await onPick({ name: city.name, province: provinceName, lat: city.lat, lon: city.lon });
    });
  };

  /** 底部折叠的手动输入（海外城市 / 兜底）：由调用方决定走在线 geocode 还是写草稿。 */
  const submitManual = async () => {
    const value = query.trim();
    if (!value || working || !onManual) return;
    setManualError("");
    await runSubmit(async () => {
      try {
        await onManual(value);
        setQuery("");
      } catch (error) {
        setManualError(String(error));
      }
    });
  };

  return (
    <>
      <div className="msg-menu-backdrop" onPointerDown={onClose} />
      <div className="city-picker" onPointerDown={(event) => event.stopPropagation()}>
        <div className="city-picker-head">
          <b>{title}</b>
          <button
            type="button"
            className="city-picker-close"
            onClick={onClose}
            title="关闭"
            aria-label="关闭城市选择"
          >
            <Icon name="close" size={13} />
          </button>
        </div>
        <button
          type="button"
          className="city-picker-locate"
          disabled={locating || working}
          onClick={() => void useCurrentLocation()}
        >
          <Icon name="mapPin" size={13} />
          {locating ? "正在定位…" : "使用当前位置"}
        </button>
        {locateError ? <div className="city-picker-locate-error">{locateError}</div> : null}
        <div className="city-picker-search">
          <Icon name="search" size={13} />
          <input
            value={cityQuery}
            placeholder="搜索省 / 市名"
            aria-label="搜索城市"
            onChange={(event) => setCityQuery(event.target.value)}
          />
        </div>
        <div className="city-picker-columns">
          <div className="city-picker-col">
            <div className="city-picker-col-title">省份</div>
            <div className="city-picker-list">
              {shownProvinces.map((p) => (
                <button
                  key={p.name}
                  type="button"
                  className={`city-picker-item ${province?.name === p.name ? "is-active" : ""}`}
                  onClick={() => setProvince(p)}
                >
                  <span>{p.name}</span>
                  <Icon name="chevron" size={11} />
                </button>
              ))}
              {shownProvinces.length === 0 ? <div className="city-picker-empty">无匹配省份</div> : null}
            </div>
          </div>
          <div className="city-picker-col">
            <div className="city-picker-col-title">{province ? province.name : "城市"}</div>
            <div className="city-picker-list">
              {shownCities.map((city) => (
                <button
                  key={city.name}
                  type="button"
                  className="city-picker-item"
                  disabled={working}
                  onClick={() => pickCity(province!.name, city)}
                  title={`${city.name} ${city.lat}, ${city.lon}`}
                >
                  <span>{city.name}</span>
                  <Icon name="plus" size={11} />
                </button>
              ))}
              {!province ? <div className="city-picker-empty">请先选择省份</div> : null}
              {province && shownCities.length === 0 ? <div className="city-picker-empty">无匹配城市</div> : null}
            </div>
          </div>
        </div>
        {onManual ? (
          <div className="city-picker-manual">
            <button
              type="button"
              className="city-picker-manual-toggle"
              onClick={() => setManualOpen((v) => !v)}
            >
              <Icon name={manualOpen ? "chevron" : "globe"} size={12} />
              手动输入城市名（海外 / 兜底）
              <Icon name="chevron" size={11} className={manualOpen ? "is-open" : ""} />
            </button>
            {manualOpen ? (
              <>
                <div className="weather-add-box city-picker-manual-box">
                  <input
                    ref={addInputRef}
                    value={query}
                    disabled={working}
                    placeholder={manualPlaceholder}
                    aria-label="添加天气城市"
                    onChange={(event) => setQuery(event.target.value)}
                    onKeyDown={(event) => {
                      if (event.nativeEvent.isComposing) return;
                      // Esc 由弹层的 window 捕获监听统一处理（关闭弹层、且阻止
                      // 设置窗的「Esc 关窗」全局监听误触发）。
                      if (event.key === "Enter") {
                        event.preventDefault();
                        void submitManual();
                      }
                    }}
                  />
                  <button
                    type="button"
                    className="weather-add-submit"
                    disabled={working || !query.trim()}
                    onClick={() => void submitManual()}
                    title="添加城市"
                    aria-label="添加城市"
                  >
                    <Icon name="check" size={13} />
                  </button>
                </div>
                {manualError ? <div className="city-picker-locate-error">{manualError}</div> : null}
              </>
            ) : null}
          </div>
        ) : null}
      </div>
    </>
  );
}
