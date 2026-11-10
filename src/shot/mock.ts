import type {
  OcrResult,
  PinData,
  QrResult,
  RecordStarted,
  RecordState,
  ScrollResult,
  ShotHistoryItem,
  ShotMonitor,
  ShotSession,
  ShotSettings,
  ShotWindow,
} from "./types";

export const mockMonitor: ShotMonitor = {
  index: 0,
  name: "DISPLAY1",
  x: 0,
  y: 0,
  width: 1600,
  height: 1000,
  scaleFactor: 1,
  primary: true,
};

export const mockShotSettings: ShotSettings = {
  autoCopy: true,
  autoPin: false,
  playSound: false,
  format: "png",
  delaySec: 0,
  hotkey: "Ctrl+Alt+A",
};

/** 浏览器预览用的假窗口（和 mockDesktop 里画的窗口对齐，前→后顺序）。 */
export const mockWindows: ShotWindow[] = [
  { x: 900, y: 690, width: 560, height: 264, app: "Code", title: "shotOverlay.tsx" },
  { x: 170, y: 96, width: 980, height: 720, app: "Safari", title: "AgentHUD 截图工作台" },
];

let cachedDesktop = "";

/** 仅浏览器预览使用：画一张有桌面、浏览器和代码窗口的假截图。 */
export function mockDesktop(): string {
  if (cachedDesktop) return cachedDesktop;
  const canvas = document.createElement("canvas");
  canvas.width = 1600;
  canvas.height = 1000;
  const ctx = canvas.getContext("2d");
  if (!ctx) return "";
  const bg = ctx.createLinearGradient(0, 0, 1600, 1000);
  bg.addColorStop(0, "#101827");
  bg.addColorStop(0.44, "#1c2444");
  bg.addColorStop(1, "#3c2348");
  ctx.fillStyle = bg;
  ctx.fillRect(0, 0, 1600, 1000);
  const glow = ctx.createRadialGradient(240, 110, 20, 240, 110, 620);
  glow.addColorStop(0, "rgba(102, 147, 255, 0.55)");
  glow.addColorStop(1, "rgba(102, 147, 255, 0)");
  ctx.fillStyle = glow;
  ctx.fillRect(0, 0, 1200, 760);
  const glow2 = ctx.createRadialGradient(1350, 810, 20, 1350, 810, 500);
  glow2.addColorStop(0, "rgba(226, 128, 255, 0.28)");
  glow2.addColorStop(1, "rgba(226, 128, 255, 0)");
  ctx.fillStyle = glow2;
  ctx.fillRect(800, 300, 800, 700);

  // 桌面图标
  ctx.fillStyle = "rgba(255,255,255,.24)";
  for (let i = 0; i < 4; i += 1) {
    const y = 54 + i * 92;
    rounded(ctx, 34, y, 46, 46, 11);
    ctx.fill();
    ctx.fillStyle = "rgba(255,255,255,.15)";
    ctx.fillRect(43, y + 58, 28, 4);
    ctx.fillStyle = "rgba(255,255,255,.24)";
  }

  // 浏览器窗口
  ctx.save();
  ctx.shadowColor = "rgba(0,0,0,.48)";
  ctx.shadowBlur = 50;
  ctx.shadowOffsetY = 26;
  rounded(ctx, 170, 96, 980, 720, 18);
  ctx.fillStyle = "#f4f6fb";
  ctx.fill();
  ctx.restore();
  rounded(ctx, 170, 96, 980, 720, 18);
  ctx.clip();
  ctx.fillStyle = "#e9edf6";
  ctx.fillRect(170, 96, 980, 56);
  ctx.fillStyle = "#dfe4ef";
  for (let i = 0; i < 3; i += 1) {
    ctx.beginPath();
    ctx.arc(198 + i * 21, 124, 5.5, 0, Math.PI * 2);
    ctx.fill();
  }
  rounded(ctx, 270, 110, 580, 28, 8);
  ctx.fillStyle = "#fff";
  ctx.fill();
  ctx.fillStyle = "#6b7488";
  ctx.font = "500 13px 'Segoe UI', sans-serif";
  ctx.fillText("https://agenthud.local/screenshot-workbench", 288, 129);
  ctx.fillStyle = "#fff";
  ctx.fillRect(170, 152, 980, 664);

  // 浏览器内容：左侧导航
  ctx.fillStyle = "#f7f8fb";
  ctx.fillRect(170, 152, 224, 664);
  ctx.fillStyle = "#252b3a";
  ctx.font = "650 17px 'Segoe UI', sans-serif";
  ctx.fillText("AgentHUD", 198, 196);
  ctx.fillStyle = "#8a92a6";
  ctx.font = "500 12px 'Segoe UI', sans-serif";
  ctx.fillText("WORKBENCH", 198, 226);
  const nav = ["概览", "截图", "标注", "历史记录", "设置"];
  nav.forEach((label, i) => {
    const y = 264 + i * 45;
    if (i === 1) {
      rounded(ctx, 188, y - 20, 186, 36, 9);
      ctx.fillStyle = "#e9eefc";
      ctx.fill();
      ctx.fillStyle = "#4e6ff0";
    } else {
      ctx.fillStyle = "#778096";
    }
    ctx.font = i === 1 ? "650 13px 'Segoe UI', sans-serif" : "500 13px 'Segoe UI', sans-serif";
    ctx.fillText(label, 214, y + 2);
    ctx.beginPath();
    ctx.arc(201, y - 2, 4, 0, Math.PI * 2);
    ctx.fill();
  });

  // 浏览器内容：主区域
  ctx.fillStyle = "#252b3a";
  ctx.font = "700 30px 'Segoe UI', sans-serif";
  ctx.fillText("截图工作台", 438, 214);
  ctx.fillStyle = "#8a92a6";
  ctx.font = "400 13px 'Segoe UI', sans-serif";
  ctx.fillText("框选、标注、保存，一次完成。", 438, 241);
  rounded(ctx, 438, 270, 672, 150, 16);
  const card = ctx.createLinearGradient(438, 270, 1110, 420);
  card.addColorStop(0, "#eef2ff");
  card.addColorStop(1, "#f9efff");
  ctx.fillStyle = card;
  ctx.fill();
  ctx.fillStyle = "#5864a8";
  ctx.font = "650 14px 'Segoe UI', sans-serif";
  ctx.fillText("快速开始", 464, 304);
  ctx.fillStyle = "#222a3e";
  ctx.font = "700 26px 'Segoe UI', sans-serif";
  ctx.fillText("Ctrl + Alt + A", 464, 342);
  ctx.fillStyle = "#7a83a3";
  ctx.font = "400 12px 'Segoe UI', sans-serif";
  ctx.fillText("区域截图  ·  窗口  ·  全屏  ·  滚动长截图", 464, 369);
  rounded(ctx, 464, 386, 112, 22, 11);
  ctx.fillStyle = "#5c78ef";
  ctx.fill();
  ctx.fillStyle = "#fff";
  ctx.font = "600 11px 'Segoe UI', sans-serif";
  ctx.fillText("开始截图", 489, 401);

  const tools = [
    ["区域", "#6c8bff"],
    ["窗口", "#b18cff"],
    ["全屏", "#52c7a5"],
    ["滚动", "#f0a35e"],
  ];
  tools.forEach(([label, color], i) => {
    const x = 438 + i * 172;
    rounded(ctx, x, 450, 154, 112, 15);
    ctx.fillStyle = "#f7f8fc";
    ctx.fill();
    rounded(ctx, x + 16, 468, 38, 38, 11);
    ctx.fillStyle = `${color}22`;
    ctx.fill();
    ctx.fillStyle = color;
    ctx.beginPath();
    ctx.arc(x + 35, 487, 7, 0, Math.PI * 2);
    ctx.fill();
    ctx.fillStyle = "#2d3446";
    ctx.font = "650 14px 'Segoe UI', sans-serif";
    ctx.fillText(label, x + 16, 531);
    ctx.fillStyle = "#9aa1b4";
    ctx.font = "400 11px 'Segoe UI', sans-serif";
    ctx.fillText(i === 3 ? "长网页 / 聊天记录" : "点击立即开始", x + 16, 549);
  });

  // 最近截图列表
  ctx.fillStyle = "#2d3446";
  ctx.font = "650 15px 'Segoe UI', sans-serif";
  ctx.fillText("最近截图", 438, 612);
  for (let i = 0; i < 3; i += 1) {
    const x = 438 + i * 224;
    rounded(ctx, x, 628, 206, 140, 13);
    ctx.fillStyle = "#f5f6fa";
    ctx.fill();
    rounded(ctx, x + 8, 636, 190, 86, 9);
    const thumb = ctx.createLinearGradient(x + 8, 636, x + 198, 722);
    thumb.addColorStop(0, i === 0 ? "#dce6ff" : i === 1 ? "#e8ddff" : "#d8f3e9");
    thumb.addColorStop(1, i === 0 ? "#9bb6ff" : i === 1 ? "#c6a9ff" : "#92ddc6");
    ctx.fillStyle = thumb;
    ctx.fill();
    ctx.fillStyle = "rgba(255,255,255,.7)";
    rounded(ctx, x + 28, 656, 82, 7, 3.5);
    ctx.fill();
    rounded(ctx, x + 28, 673, 132, 7, 3.5);
    ctx.fill();
    rounded(ctx, x + 28, 690, 104, 7, 3.5);
    ctx.fill();
    ctx.fillStyle = "#4c5468";
    ctx.font = "600 11px 'Segoe UI', sans-serif";
    ctx.fillText(`Shot_20261110_10${18 + i}.png`, x + 9, 745);
    ctx.fillStyle = "#a0a7b9";
    ctx.font = "400 9.5px 'Segoe UI', sans-serif";
    ctx.fillText("1600 × 1000  ·  刚刚", x + 9, 759);
  }

  ctx.restore();

  // 右侧代码窗口
  ctx.save();
  ctx.shadowColor = "rgba(0,0,0,.5)";
  ctx.shadowBlur = 48;
  ctx.shadowOffsetY = 24;
  rounded(ctx, 900, 690, 560, 264, 15);
  ctx.fillStyle = "#141923";
  ctx.fill();
  ctx.restore();
  rounded(ctx, 900, 690, 560, 264, 15);
  ctx.clip();
  ctx.fillStyle = "#1b212d";
  ctx.fillRect(900, 690, 560, 38);
  ctx.fillStyle = "#f7b267";
  ctx.beginPath();
  ctx.arc(922, 709, 5, 0, Math.PI * 2);
  ctx.fill();
  ctx.fillStyle = "#7a8498";
  ctx.font = "500 11px 'Consolas', monospace";
  ctx.fillText("shotOverlay.tsx", 942, 713);
  const code = [
    ["const", "#ff9e64", " capture ", "#e6edf7", "= ", "#e6edf7", "await", "#bb9af7"],
    ["  ", "#e6edf7", "screen.", "#7dcfff", "capture", "#7aa2f7", "({", "#e6edf7"],
    ["    ", "#e6edf7", "mode", "#7dcfff", ": ", "#e6edf7", "'region'", "#9ece6a"],
    ["  ", "#e6edf7", "})", "#e6edf7"],
  ];
  code.forEach((line, row) => {
    let x = 922;
    const y = 754 + row * 24;
    for (let i = 0; i < line.length; i += 2) {
      ctx.fillStyle = line[i + 1] as string;
      ctx.font = "400 12px 'Consolas', monospace";
      ctx.fillText(line[i] as string, x, y);
      x += ctx.measureText(line[i] as string).width;
    }
  });
  ctx.fillStyle = "#424c61";
  ctx.font = "400 12px 'Consolas', monospace";
  ctx.fillText("...", 922, 850);
  cachedDesktop = canvas.toDataURL("image/png");
  return cachedDesktop;
}

function rounded(
  ctx: CanvasRenderingContext2D,
  x: number,
  y: number,
  w: number,
  h: number,
  r: number
) {
  ctx.beginPath();
  ctx.roundRect(x, y, w, h, r);
}

export function mockSession(mode: "region" | "window" | "edit" = "region"): ShotSession {
  const desktop = mockDesktop();
  return {
    id: 1,
    mode,
    image: desktop,
    backdrop: desktop,
    width: 1600,
    height: 1000,
    origin: { x: 0, y: 0, width: 1600, height: 1000 },
    monitor: mockMonitor,
    title: mode === "edit" ? "窗口截图" : mode === "window" ? "窗口截图" : "区域截图",
    createdAt: Date.now(),
  };
}

export function mockHistory(): ShotHistoryItem[] {
  const desktop = mockDesktop();
  return [18, 12].map((minute, index) => ({
    name: `Shot_20261110_${String(10 - Math.floor(index / 2)).padStart(2, "0")}${String(
      58 - minute
    ).padStart(2, "0")}_118.png`,
    path: `C:\\Users\\admin\\Pictures\\AgentHUD Screenshots\\Shot_${index}.png`,
    width: 1600,
    height: 1000,
    createdAt: Date.now() - minute * 60_000,
    thumb: desktop,
    size: 2_840_000 - index * 320_000,
  }));
}

export function mockPin(image: string, width: number, height: number): PinData {
  return { id: 7, image, width, height };
}

export function mockOcr(): OcrResult {
  return {
    ok: true,
    text: "AgentHUD 截图工作台\n框选、标注、保存，一次完成。\nCtrl + Alt + A 区域截图",
    lines: ["AgentHUD 截图工作台", "框选、标注、保存，一次完成。", "Ctrl + Alt + A 区域截图"],
    error: "",
  };
}

export function mockQr(): QrResult {
  return {
    ok: true,
    texts: ["https://agenthud.local/screenshot-workbench"],
    error: "",
  };
}

export function mockScroll(_image: string, width: number, height: number): ScrollResult {
  return { width, height: Math.round(height * 1.55), frames: 5 };
}

// ------------------------------------------------------------
// 录屏（浏览器预览用假状态机）
// ------------------------------------------------------------

interface MockRecord {
  startedAt: number;
  running: boolean;
  format: string;
}

const mockRecords = new Map<number, MockRecord>();
let mockRecordSeq = 1;

export function mockRecordStart(format: string): Promise<RecordStarted> {
  const id = mockRecordSeq++;
  mockRecords.set(id, { startedAt: performance.now(), running: true, format });
  return new Promise((resolve) =>
    window.setTimeout(
      () =>
        resolve({
          recordId: id,
          format,
          width: 960,
          height: 600,
          maxSeconds: 300,
        }),
      120
    )
  );
}

export function mockRecordState(recordId: number): Promise<RecordState> {
  const entry = mockRecords.get(recordId);
  if (!entry) {
    return Promise.resolve({
      running: false,
      elapsedMs: 0,
      format: "gif",
      result: null,
      error: "录屏会话已失效",
      gifFps: 10,
      progress: 0,
    });
  }
  const elapsed = performance.now() - entry.startedAt;
  if (elapsed > 3200) entry.running = false;
  const duration = Math.min(3200, elapsed);
  const state: RecordState = {
    running: entry.running,
    elapsedMs: Math.round(duration),
    format: entry.format,
    result: entry.running
      ? null
      : {
          path: `/tmp/AgentHUD-Recording-preview.${entry.format}`,
          width: 960,
          height: 600,
          frames: 64,
          durationMs: 3200,
          size: 1_820_000,
          format: entry.format,
          recordId,
        },
    error: null,
    gifFps: 10,
    progress: 0,
  };
  return new Promise((resolve) => window.setTimeout(() => resolve(state), 40));
}

export function mockRecordExport(action: "copy" | "save"): Promise<string | null> {
  return new Promise((resolve) =>
    window.setTimeout(() => resolve(action === "save" ? "/tmp/Recording_trimmed.gif" : null), 240)
  );
}

/** 1×1 透明 GIF：浏览器预览里够占位就行。 */
export async function mockRecordMedia(): Promise<ArrayBuffer> {
  const base64 =
    "R0lGODlhAQABAIAAAAAAAP///yH5BAEAAAAALAAAAAABAAEAAAIBRAA7";
  const binary = atob(base64);
  const bytes = new Uint8Array(binary.length);
  for (let i = 0; i < binary.length; i += 1) bytes[i] = binary.charCodeAt(i);
  return bytes.buffer;
}
