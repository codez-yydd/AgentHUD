/**
 * 新消息桌面玻璃提示框（独立窗口 label `notify-popup`）。
 *
 * 分工（Rust 侧见 src-tauri/src/island/popup.rs）：
 * - Rust 负责「新消息 → 等 1.2s（岛端先动）→ 同一波次合并 → 广播
 *   `zbar://notify-popup-show`」以及窗口的建/显/隐与贴边几何；
 * - 本组件只负责卡片：堆叠、5 条与可用高度裁剪、180ms 淡入、每条独立 3s、
 *   hover 暂停（丢弃剩余时间）/ 移出重新计满 3s、点击秒关 + 打开来源应用、
 *   260ms 退场、其余条目 180ms 滑入空出的槽位，以及把**实际堆叠尺寸**量出来
 *   调 `notify_popup_resize` 让窗口紧贴卡片（空闲时 `hide`）。
 *
 * 顺序语义（与参考实现一致，容易读反）：数组 `cards[0]` 是**最贴屏幕边缘**
 * 的那条 = 最早到达的那条；新卡永远追加到数组末尾（最靠内侧）。超过上限
 * 时秒删 `cards[0]`（贴边最旧），不播退场。
 *
 * 窗口几何：内容尺寸 = 卡片堆叠 + 贴边侧一条 `EXIT_SHIFT × size` 的透明带
 * （退场朝贴边位移 10px 要住在里面，否则会被窗口自己裁掉），这条带子的宽度
 * 作为 `pad` 一起交给 Rust，Rust 负责让**卡片**距屏幕边仍是 16px。
 */

import { useEffect, useLayoutEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { copyText } from "./api";
import { KIND_COLORS, NOTIFY_BRAND_ICONS } from "./notifyBrand";

const POPUP_SHOW_EVENT = "zbar://notify-popup-show";
const POPUP_CONFIG_EVENT = "zbar://notify-popup-config";

/** 设计稿卡片尺寸（逻辑 px，size = 100%）与卡间距（不随 size 缩放） */
const CARD_W = 360;
const CARD_H = 84;
/** 同屏硬上限（Rust 侧 `POPUP_MAX` 同值） */
const MAX_CARDS = 5;
/** 每条卡片自己的停留时间；hover 暂停、移出后重新计满 */
const HOLD_MS = 3000;
/** 出现淡入 */
const FADE_IN_MS = 180;
/** 到期退场（位移 + 缩放 / 透明度两段并行） */
const EXIT_MS = 260;
/** 退场朝贴边方向的位移（设计稿 px，随 size 缩放） */
const EXIT_SHIFT = 10;
/** 其余条目滑入空出的槽位 */
const SLIDE_MS = 180;
const OUT_CUBIC = "cubic-bezier(0.215, 0.61, 0.355, 1)";
const IN_CUBIC = "cubic-bezier(0.55, 0.055, 0.675, 0.19)";

/** Rust `NotifyItem`（camelCase wire）。 */
interface PopupItem {
  id: number;
  kind: string;
  name: string;
  sender: string;
  body: string;
  /** 邮件验证码（仅 IMAP 抽取成功时有值） */
  code?: string;
  time: number;
  read: boolean;
  exe: string;
}

/** Rust `PopupConfig`（`islandEnabled` = 岛总开关，任一为 false 都不出卡） */
interface PopupConfig {
  enabled: boolean;
  /** 岛总开关：关岛时浮窗一并作废（页面要清掉手上的卡片） */
  islandEnabled: boolean;
  position: string;
  size: number;
  theme: string;
  modules: string[];
  maxHeight: number;
  maxCards: number;
}

interface PopupShow {
  items: PopupItem[];
  config: PopupConfig;
}

interface PopupReady {
  config: PopupConfig;
  pending: PopupItem[];
}

interface CardState {
  /** 本窗口内自增，只用于 DOM key 与计时器索引（消息 id 可能重复推送） */
  key: number;
  item: PopupItem;
  /** 淡入是否已开始（窗口尺寸摆好之后才置 true，避免被窗口裁掉半帧） */
  revealed: boolean;
}

const POSITION_KEYS = [
  "center_bottom",
  "center_top",
  "bottom_left",
  "bottom_right",
  "top_left",
  "top_right",
] as const;

function normalizeConfig(raw: PopupConfig | null): PopupConfig {
  const position =
    raw && POSITION_KEYS.includes(raw.position as (typeof POSITION_KEYS)[number])
      ? raw.position
      : "bottom_right";
  const size = Math.min(160, Math.max(70, Math.round(Number(raw?.size) || 100)));
  const maxCards = Math.min(
    MAX_CARDS,
    Math.max(1, Math.round(Number(raw?.maxCards) || MAX_CARDS))
  );
  return {
    // 岛总开关关掉时浮窗也视作关闭：event 到达时立刻清卡 + 隐藏
    enabled: raw?.enabled !== false && raw?.islandEnabled !== false,
    islandEnabled: raw?.islandEnabled !== false,
    position,
    size,
    theme: raw?.theme || "dark",
    modules: Array.isArray(raw?.modules) ? raw.modules : [],
    maxHeight: Number(raw?.maxHeight) || 0,
    maxCards,
  };
}

/** 卡片标题 / 副标题：沿用消息页口径（有正文预览 → sender + body，否则用 name）。
 *  邮件抽到验证码时副标题优先显示验证码，卡片右侧同时给一个复制按钮。 */
function cardText(item: PopupItem): { title: string; sub: string } {
  const kind = item.kind || "other";
  const sender = (item.sender ?? "").trim();
  const body = (item.body ?? "").trim();
  const name = (item.name ?? "").trim();
  const code = (item.code ?? "").trim();
  if (code) return { title: sender || name || "新消息", sub: `验证码 ${code}` };
  if (body) return { title: sender || name || "新消息", sub: body };
  const title = name || sender || "新消息";
  return { title, sub: kind === "mail" ? "新邮件" : "新消息" };
}

/** auto 主题按系统深浅色落地；其余四套直接用设置里的主题键。 */
function resolveTheme(theme: string): string {
  if (theme === "light" || theme === "dark" || theme === "rouge" || theme === "sky") {
    return theme;
  }
  const dark =
    typeof window.matchMedia === "function"
      ? window.matchMedia("(prefers-color-scheme: dark)").matches
      : true;
  return dark ? "dark" : "light";
}

function clearTimer(map: Map<number, number>, key: number): void {
  const timer = map.get(key);
  if (timer != null) {
    window.clearTimeout(timer);
    map.delete(key);
  }
}

export default function NotifyPopup() {
  const [cards, setCards] = useState<CardState[]>([]);
  const [config, setConfig] = useState<PopupConfig>(() => normalizeConfig(null));
  /** 验证码复制反馈：卡片 key + 是否成功，就地显示约 1.2s */
  const [copyState, setCopyState] = useState<{ key: number; ok: boolean } | null>(null);
  const copyTimer = useRef<number | null>(null);

  const cardsRef = useRef<CardState[]>([]);
  const configRef = useRef<PopupConfig>(config);
  const stackRef = useRef<HTMLDivElement | null>(null);
  const nodes = useRef(new Map<number, HTMLElement>());
  /** 每张卡片上一帧「距堆叠贴边侧的距离」（FLIP 让位动画用） */
  const tops = useRef(new Map<number, number>());
  /** 每条卡片自己的 3s 停留计时：timer + 绝对截止时刻（visibilitychange 校正用） */
  const holds = useRef(new Map<number, { timer: number; deadline: number }>());
  const exits = useRef(new Map<number, number>());
  const hovered = useRef(new Set<number>());
  /** 已入队但还没开始淡入的卡片（等窗口尺寸摆好） */
  const pendingReveal = useRef(new Set<number>());
  const syncTimer = useRef<number | null>(null);
  /** 上一次交给 Rust 的窗口内容高度（让位动画期间不允许缩窗） */
  const lastHeight = useRef(0);
  /** 在此之前不允许缩窗：卡片此刻的视觉位置可能还在新窗口之外 */
  const shrinkGuard = useRef(0);
  const seq = useRef(0);

  const scaleNow = () =>
    Math.min(1.6, Math.max(0.7, configRef.current.size / 100));
  /** 贴边侧透明带宽度（退场位移 + 缩放后取整） */
  const padNow = () => Math.ceil(EXIT_SHIFT * scaleNow());

  const commit = (next: CardState[]) => {
    cardsRef.current = next;
    setCards(next);
  };

  const applyConfig = (raw: PopupConfig | null) => {
    const next = normalizeConfig(raw);
    const previous = configRef.current;
    configRef.current = next;
    setConfig(next);
    document.documentElement.dataset.theme = resolveTheme(next.theme);
    if (!next.enabled) {
      // 开关被关掉：立刻清空并隐藏（不等让位动画）
      dropCards(
        cardsRef.current.map((card) => card.key),
        0
      );
      return;
    }
    // size 变 → 卡片缩放与贴边透明带都变；position 变 → 贴边方向要重算
    if (previous.size !== next.size || previous.position !== next.position) {
      syncWindow(0);
    }
    const cap = Math.max(1, Math.min(next.maxCards, MAX_CARDS));
    if (cardsRef.current.length > cap) {
      dropCards(
        cardsRef.current.slice(0, cardsRef.current.length - cap).map((card) => card.key)
      );
    }
  };

  const reveal = (key: number) => {
    const card = cardsRef.current.find((entry) => entry.key === key);
    if (!card) return;
    const node = nodes.current.get(key);
    node?.animate([{ opacity: 0 }, { opacity: 1 }], {
      duration: FADE_IN_MS,
      easing: OUT_CUBIC,
    });
    commit(
      cardsRef.current.map((entry) =>
        entry.key === key ? { ...entry, revealed: true } : entry
      )
    );
    // 淡入开始即开始计时（与参考实现一致：计时不叠加动画时长）
    armHold(key);
  };

  /** 丢弃剩余时间（hover 进入 / 卡片被移除时调用） */
  const suspendHold = (key: number) => {
    const hold = holds.current.get(key);
    if (hold) {
      window.clearTimeout(hold.timer);
      holds.current.delete(key);
    }
  };

  const armHold = (key: number) => {
    suspendHold(key);
    holds.current.set(key, {
      deadline: window.performance.now() + HOLD_MS,
      timer: window.setTimeout(() => {
        holds.current.delete(key);
        beginExit(key);
      }, HOLD_MS),
    });
  };

  /**
   * 计时器被后台压过 / 系统休眠后，按 `performance.now()` 的绝对截止时刻校正：
   * 已经到点的立刻退场，没到点的用剩余时间重排。
   *
   * Windows 上 Tauri 的 background_throttling 是 no-op（Rust 侧注释已订正），
   * 不能假定 3s 一定准，所以只在 visibilitychange 这个时机补一次，不做轮询。
   */
  const resyncHolds = () => {
    const now = window.performance.now();
    for (const [key, hold] of Array.from(holds.current)) {
      if (exits.current.has(key)) {
        suspendHold(key);
        continue;
      }
      window.clearTimeout(hold.timer);
      const remaining = hold.deadline - now;
      if (remaining <= 0) {
        holds.current.delete(key);
        beginExit(key);
      } else {
        holds.current.set(key, {
          deadline: hold.deadline,
          timer: window.setTimeout(() => {
            holds.current.delete(key);
            beginExit(key);
          }, remaining),
        });
      }
    }
  };

  const beginExit = (key: number) => {
    if (exits.current.has(key)) return;
    const card = cardsRef.current.find((entry) => entry.key === key);
    if (!card) return;
    suspendHold(key);
    hovered.current.delete(key);
    const node = nodes.current.get(key);
    if (node) {
      // 朝贴边方向位移（随 size 缩放）+ 缩到 86%（锚点在贴边一侧，见 popup.css），
      // OutCubic；透明度 → 0 用 InCubic，两段并行
      const dir = configRef.current.position.includes("bottom") ? 1 : -1;
      const shift = EXIT_SHIFT * scaleNow();
      node.animate(
        [
          { transform: "translateY(0px) scale(1)" },
          { transform: `translateY(${dir * shift}px) scale(0.86)` },
        ],
        { duration: EXIT_MS, easing: OUT_CUBIC, fill: "forwards" }
      );
      node.animate([{ opacity: 1 }, { opacity: 0 }], {
        duration: EXIT_MS,
        easing: IN_CUBIC,
        fill: "forwards",
      });
    }
    // 槽位留到退场结束（与参考实现一致），其余条目随后滑入
    exits.current.set(
      key,
      window.setTimeout(() => {
        exits.current.delete(key);
        removeCard(key);
      }, EXIT_MS)
    );
  };

  const removeCard = (key: number) => {
    nodes.current.delete(key);
    tops.current.delete(key);
    pendingReveal.current.delete(key);
    hovered.current.delete(key);
    suspendHold(key);
    clearTimer(exits.current, key);
    shrinkGuard.current = window.performance.now() + SLIDE_MS;
    commit(cardsRef.current.filter((card) => card.key !== key));
    syncWindow(SLIDE_MS);
  };

  /** 秒删（超上限 / 超过可用高度 / 点击关闭）：不播退场，其余条目 180ms 让位 */
  const dropCards = (keys: number[], delay = SLIDE_MS) => {
    if (keys.length === 0) return;
    const drop = new Set(keys);
    for (const key of keys) {
      nodes.current.delete(key);
      tops.current.delete(key);
      pendingReveal.current.delete(key);
      hovered.current.delete(key);
      suspendHold(key);
      clearTimer(exits.current, key);
    }
    shrinkGuard.current = window.performance.now() + SLIDE_MS;
    commit(cardsRef.current.filter((card) => !drop.has(card.key)));
    syncWindow(delay);
  };

  const syncWindow = (delay: number) => {
    if (syncTimer.current != null) window.clearTimeout(syncTimer.current);
    syncTimer.current = window.setTimeout(() => {
      syncTimer.current = null;
      void applyWindowSize();
    }, delay);
  };

  /** 量出当前堆叠的真实尺寸交给 Rust：空堆叠 → 隐藏；否则贴边定位并显示。 */
  const applyWindowSize = async () => {
    const stack = stackRef.current;
    const list = cardsRef.current;
    if (!configRef.current.enabled || !stack || list.length === 0) {
      try {
        await invoke("notify_popup_hide");
      } catch {
        /* 浏览器预览 / 后端未就绪：忽略 */
      }
      return;
    }
    const pad = padNow();
    const rect = stack.getBoundingClientRect();
    const width = Math.ceil(rect.width);
    let height = Math.ceil(rect.height) + pad;
    if (width < 1 || height < 1) return;
    // 只淡入这次测量时已挂载的卡片。等待 Rust 调整窗口时到达的消息会单独测量。
    const revealKeys = list
      .filter((card) => pendingReveal.current.has(card.key) && nodes.current.has(card.key))
      .map((card) => card.key);
    // 让位 / 退场动画期间只许涨不许缩：卡片此刻的视觉位置可能还在新窗口外，
    // 立刻缩窗会把它们裁掉一条。等动画跑完（guard 过期）再收敛。
    const guarded = window.performance.now() < shrinkGuard.current;
    if (guarded) {
      height = Math.max(height, lastHeight.current);
      if (height > Math.ceil(rect.height) + pad) {
        syncWindow(Math.max(20, shrinkGuard.current - window.performance.now() + 20));
      }
    }
    lastHeight.current = height;
    try {
      await invoke("notify_popup_resize", { width, height, pad });
    } catch {
      /* 定位失败也要把卡片显示出来，总比什么都不出现好 */
    }
    // 竞态收尾：resize 往返期间卡片可能已经被清空（点最后一张 / 关开关），
    // 这时不能再淡入，而且要补一次 hide，别留一块可见的透明置顶窗
    if (cardsRef.current.length === 0 || !configRef.current.enabled) {
      pendingReveal.current.clear();
      try {
        await invoke("notify_popup_hide");
      } catch {
        /* 忽略 */
      }
      return;
    }
    for (const key of revealKeys) {
      if (pendingReveal.current.delete(key)) reveal(key);
    }
  };

  const enqueue = (items: PopupItem[]) => {
    const cfg = configRef.current;
    if (!cfg.enabled || items.length === 0) return;
    const modules = new Set(cfg.modules);
    const known = new Set(cardsRef.current.map((card) => card.item.id));
    let next = [...cardsRef.current];
    let added = 0;
    for (const item of items) {
      // 后端已按 notify.modules 过滤，前端再兜一层；同 id 也不重复建卡
      if (!modules.has(item.kind) || known.has(item.id)) continue;
      known.add(item.id);
      seq.current += 1;
      pendingReveal.current.add(seq.current);
      next.push({ key: seq.current, item, revealed: false });
      added += 1;
    }
    if (added === 0) return;
    // 超过上限：直接秒删贴边最旧的那条（数组头），不播退场
    const cap = Math.max(1, Math.min(cfg.maxCards, MAX_CARDS));
    while (next.length > cap) {
      const oldest = next[0];
      if (!oldest) break;
      next = next.slice(1);
      nodes.current.delete(oldest.key);
      tops.current.delete(oldest.key);
      pendingReveal.current.delete(oldest.key);
      hovered.current.delete(oldest.key);
      suspendHold(oldest.key);
      clearTimer(exits.current, oldest.key);
    }
    shrinkGuard.current = window.performance.now() + SLIDE_MS;
    commit(next);
  };

  const openSource = (item: PopupItem) => {
    const kind = item.kind || "other";
    // 复用既有的「打开来源应用」命令（聚焦已运行窗口 → 常见安装路径），
    // exe 为空时传 null，交给 Rust 走内置逻辑
    void invoke("island_notify_open", { kind, exe: item.exe || null }).catch(() => {});
  };

  /** 复制验证码：复用岛内 `copyText`（shot_copy_text），不引入剪贴板插件。
   *  就地反馈 1.2s，点击不再冒泡去打开来源应用。 */
  const copyCode = (card: CardState) => {
    const code = (card.item.code ?? "").trim();
    if (!code) return;
    if (copyTimer.current != null) window.clearTimeout(copyTimer.current);
    const settle = (ok: boolean) => {
      setCopyState({ key: card.key, ok });
      if (copyTimer.current != null) window.clearTimeout(copyTimer.current);
      copyTimer.current = window.setTimeout(() => setCopyState(null), 1200);
    };
    copyText(code).then(() => settle(true), () => settle(false));
  };

  const onCardClick = (card: CardState) => {
    if (exits.current.has(card.key)) return;
    suspendHold(card.key);
    hovered.current.delete(card.key);
    // 点击立刻关闭（不播退场），其余条目同时让位
    dropCards([card.key]);
    openSource(card.item);
  };

  const onCardEnter = (key: number) => {
    if (exits.current.has(key)) return;
    hovered.current.add(key);
    // 暂停 = 丢弃剩余时间（移出后重新计满 3s）
    suspendHold(key);
  };

  const onCardLeave = (key: number) => {
    if (exits.current.has(key)) return;
    if (!hovered.current.delete(key)) return;
    armHold(key);
  };

  // 让位动画（FLIP）：卡片换槽位时，把它拉回上一帧的位置，再 180ms OutCubic
  // 滑回 0。参照系必须取「堆叠贴边那一侧到卡片的距离」：
  // - offsetTop 不行：堆叠本身贴着窗口边，缩高时整块会跟着移，位移被吃掉；
  // - 视口 top 也不行：窗口是按贴边侧锚定重新定位的，视口原点会跟着动。
  // 堆叠的贴边侧相对屏幕是静止的，所以量它的距离最稳。
  useLayoutEffect(() => {
    const stack = stackRef.current;
    if (!stack) return;
    const stackRect = stack.getBoundingClientRect();
    const bottom = (configRef.current.position || "").includes("bottom");
    for (const card of cards) {
      const node = nodes.current.get(card.key);
      if (!node) continue;
      const rect = node.getBoundingClientRect();
      // 0 = 最贴边的那一格，数值越大越靠屏幕内侧
      const inward = bottom ? stackRect.bottom - rect.bottom : rect.top - stackRect.top;
      const previous = tops.current.get(card.key);
      tops.current.set(card.key, inward);
      if (previous == null || Math.abs(previous - inward) < 0.5) continue;
      if (exits.current.has(card.key)) continue;
      const delta = previous - inward;
      const shift = bottom ? -delta : delta;
      node.animate(
        [{ transform: `translateY(${shift}px)` }, { transform: "translateY(0px)" }],
        { duration: SLIDE_MS, easing: OUT_CUBIC }
      );
    }
    const alive = new Set(cards.map((card) => card.key));
    for (const key of Array.from(tops.current.keys())) {
      if (!alive.has(key)) tops.current.delete(key);
    }
    // 等新卡写入 DOM 再量尺寸，尤其是第一张卡片，空堆叠的宽度是 0。
    if (cards.some((card) => pendingReveal.current.has(card.key))) syncWindow(0);
  }, [cards]);

  useEffect(() => {
    let disposed = false;
    const unlisteners: Array<() => void> = [];
    const boot = async () => {
      try {
        // 先注册监听再握手：Rust 只在收到 ready 之后才广播 show 事件
        const offShow = await listen<PopupShow>(POPUP_SHOW_EVENT, (event) => {
          applyConfig(event.payload.config);
          enqueue(event.payload.items ?? []);
        });
        const offConfig = await listen<PopupConfig>(POPUP_CONFIG_EVENT, (event) => {
          applyConfig(event.payload);
        });
        if (disposed) {
          offShow();
          offConfig();
          return;
        }
        unlisteners.push(offShow, offConfig);
        const ready = await invoke<PopupReady>("notify_popup_ready");
        if (disposed) return;
        applyConfig(ready.config);
        if (ready.pending?.length) enqueue(ready.pending);
      } catch {
        /* 浏览器预览（没有 Tauri 后端）或后端异常：保持空窗口即可 */
      }
    };
    void boot();
    // 窗口/系统从后台回来时校正 3s 停留计时（Windows 上节流开关是 no-op）
    const onVisibility = () => {
      if (document.visibilityState === "visible") resyncHolds();
    };
    document.addEventListener("visibilitychange", onVisibility);
    return () => {
      disposed = true;
      document.removeEventListener("visibilitychange", onVisibility);
      for (const off of unlisteners) off();
      for (const hold of holds.current.values()) window.clearTimeout(hold.timer);
      for (const timer of exits.current.values()) window.clearTimeout(timer);
      if (syncTimer.current != null) window.clearTimeout(syncTimer.current);
      if (copyTimer.current != null) window.clearTimeout(copyTimer.current);
      holds.current.clear();
      exits.current.clear();
      nodes.current.clear();
      tops.current.clear();
    };
    // 只在挂载时跑一次：内部全部通过 ref 读最新状态
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const scale = scaleNow();
  const pad = Math.ceil(EXIT_SHIFT * scale);
  const side = config.position.includes("bottom") ? "bottom" : "top";
  const align = config.position.endsWith("_right") ? "right" : "left";

  return (
    <div
      className="np-stack"
      data-side={side}
      data-align={align}
      // 贴边一侧让出 pad（退场位移住在这里），卡片本体仍贴 16px 屏幕边距
      style={side === "bottom" ? { bottom: pad } : { top: pad }}
      ref={stackRef}
    >
      {cards.map((card) => {
        const text = cardText(card.item);
        const code = (card.item.code ?? "").trim();
        const accent = KIND_COLORS[card.item.kind] ?? KIND_COLORS.other;
        const icon = NOTIFY_BRAND_ICONS[card.item.kind];
        return (
          <article
            key={card.key}
            className="np-card"
            style={{
              width: CARD_W * scale,
              height: CARD_H * scale,
              // 淡入由 Web Animations 播，未开始前保持 0（窗口刚显示时不闪）
              opacity: card.revealed ? 1 : 0,
            }}
            ref={(node) => {
              if (node) nodes.current.set(card.key, node);
            }}
            onPointerEnter={() => onCardEnter(card.key)}
            onPointerLeave={() => onCardLeave(card.key)}
            onClick={() => onCardClick(card)}
            onContextMenu={(event) => event.preventDefault()}
          >
            <div
              className="np-card__inner"
              data-code={code ? "1" : undefined}
              style={{ transform: `scale(${scale})` }}
            >
              <span className="np-card__plate" />
              <span className="np-card__rim" />
              <span className="np-card__accent" style={{ background: accent }} />
              <span className="np-card__icon">
                {icon ? (
                  <img src={icon} alt="" draggable={false} />
                ) : (
                  <span className="np-card__icon-fallback" style={{ background: accent }}>
                    {(card.item.name || "·").slice(0, 1)}
                  </span>
                )}
              </span>
              <span className="np-card__title">{text.title}</span>
              <span className="np-card__sub">{text.sub}</span>
              {code ? (
                <button
                  type="button"
                  className="np-card__copy"
                  data-copied={
                    copyState?.key === card.key ? (copyState.ok ? "1" : "0") : undefined
                  }
                  title="点击复制验证码"
                  aria-label={`复制验证码 ${code}`}
                  onClick={(event) => {
                    event.stopPropagation();
                    copyCode(card);
                  }}
                >
                  {copyState?.key === card.key ? (copyState.ok ? "已复制" : "复制失败") : "复制"}
                </button>
              ) : null}
            </div>
          </article>
        );
      })}
    </div>
  );
}
