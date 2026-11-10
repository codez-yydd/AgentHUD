/** 浏览器预览用的假数据（无 Tauri 环境下 island.html 仍可完整渲染/截图） */

import type {
  ClipItem,
  HudBarAgent,
  HudBarUsage,
  IslandConfig,
  MediaState,
  NotifyState,
  StockMinute,
  StocksState,
  SystemState,
  WeatherState,
} from "./types";

const now = Date.now();
const HOUR = 3_600_000;

function isoLocal(d: Date): string {
  const pad = (n: number) => String(n).padStart(2, "0");
  return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())}T${pad(
    d.getHours()
  )}:00`;
}

const weatherCodes = [
  { code: 2, icon: "partly", desc: "多云" },
  { code: 1, icon: "sun", desc: "晴" },
  { code: 3, icon: "cloudy", desc: "阴" },
  { code: 61, icon: "rain", desc: "小雨" },
  { code: 0, icon: "sun", desc: "晴" },
];

export const mockWeather: WeatherState = {
  ok: true,
  error: "",
  city: "上海",
  updatedAt: now,
  temp: 23,
  feelsLike: 24,
  humidity: 61,
  wind: 12,
  code: 2,
  desc: "多云",
  icon: "partly",
  isDay: true,
  sunrise: "06:18",
  sunset: "17:02",
  hourly: Array.from({ length: 12 }, (_, i) => {
    const d = new Date(now + i * HOUR);
    const w = weatherCodes[(i + 2) % weatherCodes.length];
    return {
      time: isoLocal(d),
      label: `${String(d.getHours()).padStart(2, "0")}:00`,
      temp: Math.round(22 + Math.sin(i / 2.2) * 4 + (i > 7 ? -2 : 0)),
      code: w.code,
      icon: w.icon,
      desc: w.desc,
      precip: i % 4 === 3 ? 35 : i % 3 === 0 ? 10 : 0,
    };
  }),
  daily: Array.from({ length: 6 }, (_, i) => {
    const d = new Date(now + i * 86_400_000);
    const w = weatherCodes[(i + 1) % weatherCodes.length];
    const names = ["周日", "周一", "周二", "周三", "周四", "周五", "周六"];
    return {
      date: d.toISOString().slice(0, 10),
      weekday: i === 0 ? "今天" : names[d.getDay()],
      code: w.code,
      icon: w.icon,
      desc: w.desc,
      max: [25, 24, 22, 20, 23, 26][i],
      min: [17, 16, 15, 14, 16, 18][i],
    };
  }),
};

export const mockAgents: HudBarAgent[] = [
  {
    id: "zai:work",
    source: "zai",
    account: "工作号",
    plan: "MAX",
    quotas: [
      { key: "hour5", labelZh: "5h", labelEn: "5h", usedPct: 4, resetAt: now + 3.1 * HOUR },
      { key: "weekly", labelZh: "每周", labelEn: "wk", usedPct: 18, resetAt: now + 6 * 86_400_000 },
    ],
  },
  {
    id: "zai:personal",
    source: "zai",
    account: "个人号",
    plan: "Pro",
    quotas: [
      { key: "hour5", labelZh: "5h", labelEn: "5h", usedPct: 38, resetAt: now + 2.8 * HOUR },
      { key: "weekly", labelZh: "每周", labelEn: "wk", usedPct: 51, resetAt: now + 5 * 86_400_000 },
    ],
  },
  {
    id: "codex",
    source: "codex",
    plan: "plus",
    quotas: [
      { key: "hour5", labelZh: "5h", labelEn: "5h", usedPct: 35, resetAt: now + 2.4 * HOUR },
      { key: "weekly", labelZh: "每周", labelEn: "wk", usedPct: 42, resetAt: now + 4.2 * 86_400_000 },
    ],
  },
  {
    id: "dsh",
    source: "dsh",
    quotas: [],
    balance: { amount: 23.5, currency: "¥", granted: 5, toppedUp: 18.5 },
  },
  {
    id: "opencodego:main",
    source: "opencodego",
    account: "主账号",
    plan: "Go",
    quotas: [
      { key: "hour5", labelZh: "5h", labelEn: "5h", usedPct: 72, resetAt: now + 3.7 * HOUR },
      { key: "weekly", labelZh: "每周", labelEn: "wk", usedPct: 36, resetAt: now + 3.2 * 86_400_000 },
      { key: "monthly", labelZh: "每月", labelEn: "mo", usedPct: 28, resetAt: now + 18 * 86_400_000 },
    ],
  },
  {
    id: "opencodego:backup",
    source: "opencodego",
    account: "备用 Key",
    plan: "Go",
    quotas: [
      { key: "hour5", labelZh: "5h", labelEn: "5h", usedPct: 12, resetAt: now + 4.1 * HOUR },
      { key: "weekly", labelZh: "每周", labelEn: "wk", usedPct: 8, resetAt: now + 6.7 * 86_400_000 },
    ],
  },
];

export const mockUsage: HudBarUsage = { ts: now, agents: mockAgents };

export const mockStocks: StocksState = {
  ok: true,
  error: "",
  updatedAt: now,
  quotes: [
    {
      code: "sh000001",
      name: "上证指数",
      price: 3386.24,
      prevClose: 3352.11,
      open: 3355.2,
      chg: 34.13,
      pct: 1.02,
      high: 3390.1,
      low: 3348.5,
      time: "20261110150000",
      market: "sh",
      status: "已收盘",
    },
    {
      code: "sz399001",
      name: "深证成指",
      price: 10820.42,
      prevClose: 10891.2,
      open: 10885.1,
      chg: -70.78,
      pct: -0.65,
      high: 10902.3,
      low: 10780.2,
      time: "20261110150000",
      market: "sz",
      status: "已收盘",
    },
    {
      code: "hk00700",
      name: "腾讯控股",
      price: 468.2,
      prevClose: 461.4,
      open: 462,
      chg: 6.8,
      pct: 1.47,
      high: 471.8,
      low: 460.2,
      time: "20261110160000",
      market: "hk",
      status: "已收盘",
    },
    {
      code: "usAAPL",
      name: "苹果",
      price: 228.42,
      prevClose: 226.1,
      open: 226.8,
      chg: 2.32,
      pct: 1.03,
      high: 229.1,
      low: 225.9,
      time: "20261110040000",
      market: "us",
      status: "已收盘",
    },
    {
      code: "usNVDA",
      name: "英伟达",
      price: 196.74,
      prevClose: 201.2,
      open: 200.8,
      chg: -4.46,
      pct: -2.22,
      high: 202.4,
      low: 195.8,
      time: "20261110040000",
      market: "us",
      status: "已收盘",
    },
  ],
};

export const mockMedia: MediaState = {
  hasSession: true,
  appId: "Spotify.exe",
  appName: "Spotify",
  title: "Sample Song",
  artist: "wisland",
  album: "Ambient Works",
  playing: true,
  position: 82,
  duration: 213,
  canPrev: true,
  canNext: true,
  cover: "",
  source: "mock",
};

export const mockSystem: SystemState = {
  volume: 0.62,
  muted: false,
  brightness: 80,
  brightnessSupported: true,
  wifi: true,
  wifiAvailable: true,
  bluetooth: false,
  bluetoothAvailable: true,
  error: "",
  updatedAt: now,
};

export const mockClipboard: ClipItem[] = [
  {
    id: 1,
    kind: "text",
    text: "pnpm tauri dev -- --features island",
    preview: "pnpm tauri dev -- --features island",
    image: "",
    time: now - 120_000,
    width: 0,
    height: 0,
  },
  {
    id: 2,
    kind: "text",
    text: "https://github.com/Misaka273/wisland",
    preview: "https://github.com/Misaka273/wisland",
    image: "",
    time: now - 540_000,
    width: 0,
    height: 0,
  },
  {
    id: 3,
    kind: "text",
    text: "Agent 额度与天气直接放在岛里，默认不要宠物表情。",
    preview: "Agent 额度与天气直接放在岛里，默认不要宠物表情。",
    image: "",
    time: now - 1_800_000,
    width: 0,
    height: 0,
  },
];

export const mockNotify: NotifyState = {
  unread: 2,
  items: [
    {
      id: 4,
      kind: "wechat",
      name: "微信",
      sender: "产品群",
      body: "",
      time: now - 300_000,
      read: false,
    },
    {
      id: 3,
      kind: "mail",
      name: "邮件",
      sender: "GitHub",
      body: "[AgentHUD] New issue #128: HUD island request",
      time: now - 1_200_000,
      read: false,
    },
    {
      id: 2,
      kind: "feishu",
      name: "飞书",
      sender: "Release Bot",
      body: "v1.8.1 已发布到 Windows / macOS",
      time: now - 3_600_000,
      read: true,
    },
    {
      id: 1,
      kind: "qq",
      name: "QQ",
      sender: "文件传输助手",
      body: "",
      time: now - 7_200_000,
      read: true,
    },
  ],
};

export const mockConfig: IslandConfig = {
  enabled: true,
  transparency: 40,
  fontSize: 13,
  expandMode: "hover",
  compactView: "quota",
  replaceLegacyBar: true,
  weatherCity: "上海",
  weatherLabel: "上海 · 徐汇区",
  weatherLat: 31.22,
  weatherLon: 121.46,
  edge: "top",
  posFrac: 0.5,
  agentOrder: [],
  stocks: mockStocks.quotes.map((q) => q.code),
  stockIntervalSec: 5,
  stockAlertPct: 3,
  hotkeyToggle: "Ctrl+Alt+I",
  hideFullscreen: true,
  hiddenTabs: [],
  launcher: [
    { name: "资源管理器", path: "explorer.exe", icon: "folder" },
    { name: "浏览器", path: "https://www.bing.com", icon: "browser" },
    { name: "设置", path: "ms-settings:", icon: "settings" },
    { name: "任务管理器", path: "taskmgr.exe", icon: "task" },
    { name: "记事本", path: "notepad.exe", icon: "note" },
  ],
};

export const mockMinute: StockMinute = {
  code: "sh000001",
  prevClose: 3352.11,
  points: Array.from({ length: 120 }, (_, i) => {
    const base = 3352.11 + Math.sin(i / 9) * 10 + i * 0.28;
    return Number(base.toFixed(2));
  }),
};
