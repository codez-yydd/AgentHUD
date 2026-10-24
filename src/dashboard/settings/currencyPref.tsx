/**
 * 主窗口币种偏好上下文：把「币种完全跟随语言」的纯函数（currencyOf）
 * 升级为「读设置」的版本——设置页选了具体币种（cny/usd）后，主窗口全部
 * 页面（总览 / 日志 / 定价 / 骨架屏）的花费折算口径跟随；默认 follow
 * （跟随语言）时行为与升级前**逐字节一致**（同一份 currencyOf 纯函数）。
 *
 * 状态真相在 Rust（~/.agenthud/currency.json 的 mode 字段，原子写）：
 * - 挂载时主动拉一次 get_currency_setting 校正（首帧按 follow 渲染，
 *   与升级前的语言推导值一致，无跳变）；
 * - 监听 zbar://currency-changed（set_currency_mode 落盘后广播）：
 *   本窗/面板任何一处改设置，各窗口同步；
 * - setMode 乐观更新 + 失败回滚（照 hudBar.ts 开关的先例），Rust 广播
 *   回来后幂等同值。
 */
import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useMemo,
  useRef,
  useState,
} from "react";
import type { ReactNode } from "react";
import { listen } from "@tauri-apps/api/event";
import type { Currency, CurrencyMode } from "../../types";
import { getCurrencySetting, setCurrencyMode } from "../../api";
import { useI18n } from "../../i18n";
import { currencyOf } from "../overview/derive";

/** 币种变化事件（Rust lib.rs set_currency_mode 落盘后广播）：
 *  payload = { mode, currency } */
const CURRENCY_CHANGED_EVENT = "zbar://currency-changed";

/** 事件 payload（Rust serde camelCase） */
interface CurrencyChangedPayload {
  mode?: string;
  currency?: string;
}

/** 把事件/命令回执里的字符串收敛为合法三态（非法值回 follow） */
function normalizeMode(value: unknown): CurrencyMode {
  return value === "cny" || value === "usd" ? value : "follow";
}

interface CurrencyPrefValue {
  /** 当前来源三态（首帧 follow，启动读取后校正） */
  mode: CurrencyMode;
  /** 生效币种：follow → currencyOf(locale)（升级前同款纯函数），否则手动值 */
  currency: Currency;
  /** 设置页切换来源：乐观更新 + 落盘（失败回滚） */
  setMode: (mode: CurrencyMode) => void;
}

const Ctx = createContext<CurrencyPrefValue | null>(null);

export function CurrencyPrefProvider({ children }: { children: ReactNode }) {
  const { locale } = useI18n();
  // 首帧 follow：与升级前的语言推导渲染零差异；启动读取后校正
  const [mode, setModeState] = useState<CurrencyMode>("follow");

  // 启动读取 + 事件跟随。读取失败静默（保持 follow，与现状一致）。
  useEffect(() => {
    let cancelled = false;
    // 时序守卫：启动读取的响应可能在 currency-changed 事件之后才返回
    //（读到的是事件前的旧值）；事件已到达后晚到的旧响应直接丢弃，
    // 避免旧值覆盖事件带来的新值（显示短暂回退，与 App.tsx 同款守卫）
    let eventSeen = false;
    getCurrencySetting()
      .then((setting) => {
        if (!cancelled && !eventSeen) setModeState(normalizeMode(setting.mode));
      })
      .catch(() => {
        /* 静默：保持 follow（= 升级前行为） */
      });
    let dispose: (() => void) | undefined;
    listen<CurrencyChangedPayload>(CURRENCY_CHANGED_EVENT, (event) => {
      eventSeen = true;
      setModeState(normalizeMode(event.payload?.mode));
    })
      .then((unlisten) => {
        if (cancelled) unlisten();
        else dispose = unlisten;
      })
      .catch(() => {
        /* 静默：只失去跨窗实时性，下次挂载重读 */
      });
    return () => {
      cancelled = true;
      dispose?.();
    };
  }, []);

  // 乐观切换 + 失败回滚（照 hudBar 开关先例）。落盘由 Rust 完成并广播，
  // 事件回来后同值幂等；期间本窗已按新值渲染，体感即时。
  const modeRef = useRef(mode);
  modeRef.current = mode;
  const setMode = useCallback((next: CurrencyMode) => {
    const prev = modeRef.current;
    setModeState(next);
    setCurrencyMode(next).catch(() => {
      // 落盘失败（磁盘/IPC 异常）回滚，界面与配置保持一致
      if (modeRef.current === next) setModeState(prev);
    });
  }, []);

  const currency: Currency = mode === "follow" ? currencyOf(locale) : mode;

  const value = useMemo(
    () => ({ mode, currency, setMode }),
    [mode, currency, setMode]
  );

  return <Ctx.Provider value={value}>{children}</Ctx.Provider>;
}

/** 读取币种偏好上下文。必须在 <CurrencyPrefProvider> 内使用。 */
export function useCurrencyPref(): CurrencyPrefValue {
  const v = useContext(Ctx);
  if (!v) throw new Error("useCurrencyPref must be used within a CurrencyPrefProvider");
  return v;
}
