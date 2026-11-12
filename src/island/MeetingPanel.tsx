/**
 * 灵动岛「会议录音」子页：cpal 采集 → 本机 WAV + JSON 侧车，纯本机。
 *
 * 状态机：空闲（设备选择 + 最近列表）→ 录制中（计时 / 暂停继续 / 打标记 /
 * 结束）→ 回列表。转写明确不做，放「即将支持」置灰占位。
 *
 * - 首次开始前弹隐私确认（「录音会保存到本机磁盘 + 路径」），确认结果由
 *   后端持久化（island.db），之后不再弹。
 * - 计时用本地墙钟（暂停累计扣除），权威时长以结束后端返回的 duration_ms
 *   为准（按实际写入采样数算，天然扣除暂停段）。
 * - 子页重挂载（切走再切回）时通过 meeting_state() 恢复录制视图，后端
 *   会话不受前端影响。
 * - 浏览器预览（!IS_TAURI）走 api.ts 的 mock 会话状态机，列表用假数据。
 */
import { useCallback, useEffect, useRef, useState } from "react";
import type { KeyboardEvent as ReactKeyboardEvent } from "react";
import {
  meetingAddMark,
  meetingAudio,
  meetingConfirmPrivacy,
  meetingDelete,
  meetingDevices,
  meetingList,
  meetingOpenFolder,
  meetingPause,
  meetingPrivacy,
  meetingRename,
  meetingResume,
  meetingSetAutoGain,
  meetingSettings,
  meetingStart,
  meetingState,
  meetingStop,
} from "./api";
import type {
  MeetingAudioDevice,
  MeetingItem,
  MeetingMark,
  MeetingPrivacy,
  MeetingSessionBrief,
} from "./types";
import { fmtDuration, Icon, Toggle } from "./ui";

const PENDING_RESET_MS = 2400;

/** 录制视图的本地会话态（从后端 brief 或开始结果构建） */
interface LiveMeeting {
  name: string;
  device: string;
  sampleRate: number;
  dir: string;
  marks: MeetingMark[];
  paused: boolean;
  /** 计时基准：本地墙钟起点 + 已暂停累计（后端 brief 带 effective_ms 时换算） */
  startWall: number;
  pausedAccumMs: number;
  pauseAt: number | null;
}

function errorText(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

/** 字节数 → 人读（B / KB / MB / GB） */
function fmtBytes(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(bytes < 10_240 ? 1 : 0)} KB`;
  if (bytes < 1024 * 1024 * 1024)
    return `${(bytes / 1024 / 1024).toFixed(1)} MB`;
  return `${(bytes / 1024 / 1024 / 1024).toFixed(2)} GB`;
}

function fmtWhen(ts: number): string {
  const diff = Math.max(0, Date.now() - ts);
  if (diff < 60_000) return "刚刚";
  if (diff < 3_600_000) return `${Math.floor(diff / 60_000)} 分钟前`;
  if (diff < 86_400_000) return `${Math.floor(diff / 3_600_000)} 小时前`;
  const date = new Date(ts);
  return `${date.getMonth() + 1}/${date.getDate()} ${String(date.getHours()).padStart(2, "0")}:${String(date.getMinutes()).padStart(2, "0")}`;
}

/** 从后端快照恢复本地计时态：effective_ms 是此刻有效时长，向前推基准 */
function briefToLive(brief: MeetingSessionBrief): LiveMeeting {
  const startWall = Date.now() - brief.effective_ms;
  return {
    name: brief.name,
    device: brief.device,
    sampleRate: brief.sample_rate,
    dir: brief.dir,
    marks: brief.marks,
    paused: brief.paused,
    startWall,
    pausedAccumMs: 0,
    pauseAt: brief.paused ? Date.now() : null,
  };
}

/** 设备重扫间隔：插拔耳机后不用手动刷新，列表自己长出来 / 消失 */
const DEVICE_POLL_MS = 4000;

/** 「刚插上」提示的停留时间 */
const DEVICE_NEW_HINT_MS = 6000;

interface DeviceOption {
  id: string;
  label: string;
}

/** 选项表：第一项恒为「默认麦克风」（跟随系统默认输入设备） */
function deviceOptions(devices: MeetingAudioDevice[]): DeviceOption[] {
  return [
    { id: "", label: "默认麦克风" },
    ...devices.map((device) => ({ id: device.id, label: device.name })),
  ];
}

/**
 * 音频输入设备下拉（自建，不用原生 `<select>`）。
 *
 * 为什么不用原生：弹层由 WebView2 画成独立窗口，在岛的「透明 + 置顶 + 圆角
 * 裁剪」窗口里真机上会变成白底白字、且被窗口边缘裁掉半截（552px 宽的面板里
 * 完全看不清）。这里用 DOM 绝对定位列表，配色走岛的玻璃变量，底色比右键菜单
 * 更实（列表压在按钮/列表文字上，必须不透字），下方空间不足时向上翻。
 *
 * 交互：点击展开并顺带重扫设备（热插拔）；↑↓ 移动、Enter 选中、Esc / 点外部
 * 收起；触发器 `aria-haspopup` + `aria-expanded`，列表 `role="listbox"`、
 * 行 `role="option"`。
 */
function DevicePicker({
  devices,
  value,
  onChange,
  scanning,
  onRefresh,
}: {
  devices: MeetingAudioDevice[];
  value: string;
  onChange: (id: string) => void;
  scanning: boolean;
  onRefresh: () => void;
}) {
  const [open, setOpen] = useState(false);
  /** 弹层方向：下方放不下就向上翻（列表最高 ~168px） */
  const [dir, setDir] = useState<"down" | "up">("down");
  const [active, setActive] = useState(0);
  const wrapRef = useRef<HTMLDivElement | null>(null);
  const listRef = useRef<HTMLDivElement | null>(null);
  const triggerRef = useRef<HTMLButtonElement | null>(null);

  const options = deviceOptions(devices);
  const current = options.find((option) => option.id === value) ?? options[0];
  // 重扫后选项可能变少：高亮下标夹到有效范围，避免 Enter 选到 undefined
  const activeIndex = Math.min(active, options.length - 1);

  const openList = useCallback(() => {
    const rect = wrapRef.current?.getBoundingClientRect();
    if (rect) {
      const below = window.innerHeight - rect.bottom;
      const above = rect.top;
      setDir(below < 190 && above > below ? "up" : "down");
    }
    setActive(Math.max(0, options.findIndex((option) => option.id === value)));
    setOpen(true);
    onRefresh();
  }, [onRefresh, options, value]);

  const close = useCallback((refocus: boolean) => {
    setOpen(false);
    if (refocus) triggerRef.current?.focus();
  }, []);

  const pick = useCallback(
    (id: string) => {
      onChange(id);
      close(true);
    },
    [close, onChange]
  );

  // 展开后把焦点交给列表（键盘可用），收起/点外部就关
  useEffect(() => {
    if (!open) return;
    listRef.current?.focus({ preventScroll: true });
    const onPointerDown = (event: MouseEvent) => {
      if (!wrapRef.current?.contains(event.target as Node)) setOpen(false);
    };
    document.addEventListener("mousedown", onPointerDown);
    return () => document.removeEventListener("mousedown", onPointerDown);
  }, [open]);

  // 键盘移动时让高亮行滚进可视区（列表自身 overflow 滚动）
  useEffect(() => {
    if (!open) return;
    listRef.current
      ?.querySelector<HTMLElement>(`[data-index="${activeIndex}"]`)
      ?.scrollIntoView({ block: "nearest" });
  }, [activeIndex, open]);

  const onListKeyDown = (event: ReactKeyboardEvent<HTMLDivElement>) => {
    const last = options.length - 1;
    if (event.key === "Escape") {
      event.preventDefault();
      close(true);
    } else if (event.key === "Tab") {
      setOpen(false);
    } else if (event.key === "ArrowDown" || event.key === "ArrowUp") {
      event.preventDefault();
      const step = event.key === "ArrowDown" ? 1 : -1;
      setActive((index) => {
        const from = Math.min(index, last);
        return (from + step + options.length) % options.length;
      });
    } else if (event.key === "Home" || event.key === "End") {
      event.preventDefault();
      setActive(event.key === "Home" ? 0 : last);
    } else if (event.key === "Enter" || event.key === " ") {
      event.preventDefault();
      const option = options[activeIndex];
      if (option) pick(option.id);
    }
  };

  return (
    <div className="meet-picker" ref={wrapRef}>
      <button
        ref={triggerRef}
        type="button"
        className="meet-select"
        aria-haspopup="listbox"
        aria-expanded={open}
        aria-label="音频输入设备"
        title={current.label}
        onClick={() => (open ? close(true) : openList())}
        onKeyDown={(event) => {
          if (event.key === "ArrowDown" || event.key === "ArrowUp") {
            event.preventDefault();
            openList();
          }
        }}
      >
        <span className="meet-select-text">{current.label}</span>
        <Icon name="chevron" size={11} className={`meet-caret${open ? " open" : ""}`} />
      </button>

      {open ? (
        <div
          ref={listRef}
          className="meet-picker-pop"
          data-dir={dir}
          role="listbox"
          aria-label="音频输入设备"
          tabIndex={-1}
          onKeyDown={onListKeyDown}
        >
          {options.map((option, index) => (
            <div
              key={option.id || "__default"}
              data-index={index}
              data-active={index === activeIndex ? "true" : undefined}
              role="option"
              aria-selected={option.id === value}
              className="meet-option"
              onMouseEnter={() => setActive(index)}
              onClick={() => pick(option.id)}
            >
              <span className="meet-option-text">{option.label}</span>
              {option.id === value ? <Icon name="check" size={11} /> : null}
            </div>
          ))}
          <div className="meet-picker-foot">
            <span>{devices.length > 0 ? `${devices.length} 个输入设备` : "未检测到输入设备"}</span>
            <button
              type="button"
              className="mini-text-btn"
              disabled={scanning}
              onClick={onRefresh}
            >
              {scanning ? "扫描中…" : "重新扫描"}
            </button>
          </div>
        </div>
      ) : null}
    </div>
  );
}

export function MeetingPanel({ onToast }: { onToast: (message: string) => void }) {
  // ---- 静态数据：设备 / 隐私状态 ----
  const [devices, setDevices] = useState<MeetingAudioDevice[]>([]);
  const [deviceId, setDeviceId] = useState("");
  /** 设备枚举真失败（驱动异常 / 无权限）；「一台设备都没有」走空态，不算错误 */
  const [deviceError, setDeviceError] = useState("");
  /** 所选设备被拔掉的一句话说明（自动回落到默认麦克风） */
  const [deviceGone, setDeviceGone] = useState("");
  /** 刚插上的设备提示（几秒后自动消失） */
  const [deviceNew, setDeviceNew] = useState("");
  const [scanning, setScanning] = useState(false);
  /** 重扫是定时器驱动的：回调里读不到最新 deviceId，用 ref 取当前选中值 */
  const deviceIdRef = useRef("");
  const deviceNamesRef = useRef<string[]>([]);
  const newHintTimer = useRef<number | null>(null);
  const [privacy, setPrivacy] = useState<MeetingPrivacy | null>(null);
  const [confirmPending, setConfirmPending] = useState(false);
  /** 保存时自动归一化音量（默认开；设置持久化在 island.db） */
  const [autoGain, setAutoGain] = useState(true);
  const [gainSaving, setGainSaving] = useState(false);

  // ---- 播放（一次只播一条；audio 元素在空闲视图里常驻）----
  const audioRef = useRef<HTMLAudioElement | null>(null);
  const [trackId, setTrackId] = useState("");
  const [playing, setPlaying] = useState(false);
  const [audioPos, setAudioPos] = useState(0);
  const [audioDur, setAudioDur] = useState(0);
  const [audioLoading, setAudioLoading] = useState("");
  const [audioError, setAudioError] = useState("");

  // ---- 录制会话 ----
  const [live, setLive] = useState<LiveMeeting | null>(null);
  const [elapsedMs, setElapsedMs] = useState(0);
  const [markLabel, setMarkLabel] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");

  // ---- 列表 ----
  const [items, setItems] = useState<MeetingItem[]>([]);
  const [listLoading, setListLoading] = useState(true);
  /** 列表加载失败单独记：加载失败时不再渲染「还没有录音」空态（互相矛盾） */
  const [listError, setListError] = useState("");
  const [renaming, setRenaming] = useState<{ id: string; value: string } | null>(null);
  /** 重命名错误就近显示在该行输入框下方，而不是列表顶部 */
  const [renameError, setRenameError] = useState("");
  const [renameSaving, setRenameSaving] = useState(false);
  /** 正在删除的条目 id：按钮禁用防重复提交 */
  const [deleteBusyId, setDeleteBusyId] = useState("");
  /** 内联展开标记明细的条目 id（"" = 全部收起） */
  const [marksOpen, setMarksOpen] = useState("");
  const [pendingDelete, setPendingDelete] = useState("");
  const deleteTimer = useRef<number | null>(null);

  const reloadList = useCallback(() => {
    return meetingList()
      .then((list) => {
        setItems(list);
        setListError("");
      })
      .catch((e) => setListError(errorText(e)));
  }, []);

  useEffect(() => {
    deviceIdRef.current = deviceId;
  }, [deviceId]);

  /**
   * 重扫输入设备（热插拔）：cpal 每次 `input_devices()` 都重新
   * `EnumAudioEndpoints(ACTIVE)` 取当前快照，插上 / 拔掉耳机后再调一次就能
   * 看到变化，不需要设备变更通知。
   *
   * - `showBusy` 只在用户展开下拉 / 点「重新扫描」时为 true；4s 轮询走静默，
   *   不闪「扫描中…」。
   * - 枚举失败保留旧列表（拔掉最后一个设备时不该把选择器清成「读取失败」）；
   *   只有真异常才记 `deviceError`，空列表由空态提示。
   * - 选中的设备被拔掉：回落到系统默认并说明是哪个设备掉了（后端开始录音时
   *   还会再校验一次，这里只是让 UI 不撒谎）。
   */
  const refreshDevices = useCallback(async (showBusy = false) => {
    if (showBusy) setScanning(true);
    try {
      const list = await meetingDevices();
      // 同型号设备可能同名（cpal 用名字当 id），重复项只留一条
      const unique: MeetingAudioDevice[] = [];
      for (const device of list) {
        if (device.id && !unique.some((item) => item.id === device.id)) unique.push(device);
      }
      setDevices(unique);
      setDeviceError("");

      const picked = deviceIdRef.current;
      if (picked && !unique.some((item) => item.id === picked)) {
        setDeviceId("");
        setDeviceGone(`「${picked}」已断开，改用默认麦克风`);
      } else if (picked) {
        setDeviceGone("");
      }

      // 首轮不算「新插入」；之后多出来的设备给一句提示（插耳机时用户往往在
      // 别的窗口，回来能看见列表已经更新）
      const previous = deviceNamesRef.current;
      const fresh =
        previous.length > 0
          ? unique.filter((device) => !previous.includes(device.name))
          : [];
      deviceNamesRef.current = unique.map((device) => device.name);
      if (fresh.length > 0) {
        setDeviceNew(`已检测到新设备「${fresh[0].name}」`);
        if (newHintTimer.current) window.clearTimeout(newHintTimer.current);
        newHintTimer.current = window.setTimeout(() => setDeviceNew(""), DEVICE_NEW_HINT_MS);
      }
    } catch (e) {
      setDeviceError(errorText(e));
    } finally {
      if (showBusy) setScanning(false);
    }
  }, []);

  // 首次挂载：恢复可能的录制会话 + 拉隐私 / 列表（设备由下面的轮询负责）
  useEffect(() => {
    void meetingState()
      .then((brief) => {
        if (brief) setLive(briefToLive(brief));
      })
      .catch(() => {
        /* 恢复失败按空闲处理 */
      });
    void meetingPrivacy()
      .then(setPrivacy)
      .catch(() => {
        /* 读不到按未确认处理 */
      });
    void meetingSettings()
      .then((settings) => setAutoGain(settings.autoGain))
      .catch(() => {
        /* 读不到按默认「开」处理 */
      });
    void reloadList().finally(() => setListLoading(false));
    return () => {
      if (deleteTimer.current) window.clearTimeout(deleteTimer.current);
      if (newHintTimer.current) window.clearTimeout(newHintTimer.current);
    };
  }, [reloadList]);

  // 设备热插拔：进页面先扫一次，之后每 DEVICE_POLL_MS 静默重扫；窗口重新拿到
  // 焦点时也扫（插耳机时用户常在别的窗口）。录制中不扫——选择器本来就不在。
  useEffect(() => {
    if (live) return;
    void refreshDevices();
    const timer = window.setInterval(() => void refreshDevices(), DEVICE_POLL_MS);
    const onFocus = () => void refreshDevices();
    window.addEventListener("focus", onFocus);
    return () => {
      window.clearInterval(timer);
      window.removeEventListener("focus", onFocus);
    };
  }, [live, refreshDevices]);

  // ---- 播放：audio 元素随空闲视图挂载，事件回写进度 / 状态 ----
  // 依赖 live：录制中空闲视图不渲染（没有 audio 元素），回到空闲时重新绑定
  useEffect(() => {
    const audio = audioRef.current;
    if (!audio) return;
    const onTime = () => setAudioPos(audio.currentTime);
    const onMeta = () => setAudioDur(Number.isFinite(audio.duration) ? audio.duration : 0);
    const onPlay = () => setPlaying(true);
    const onPause = () => setPlaying(false);
    const onEnded = () => {
      setPlaying(false);
      setAudioPos(0);
      audio.currentTime = 0;
    };
    const onError = () =>
      setAudioError("无法播放该录音（文件可能已被移动或删除）");
    audio.addEventListener("timeupdate", onTime);
    audio.addEventListener("loadedmetadata", onMeta);
    audio.addEventListener("play", onPlay);
    audio.addEventListener("pause", onPause);
    audio.addEventListener("ended", onEnded);
    audio.addEventListener("error", onError);
    return () => {
      audio.removeEventListener("timeupdate", onTime);
      audio.removeEventListener("loadedmetadata", onMeta);
      audio.removeEventListener("play", onPlay);
      audio.removeEventListener("pause", onPause);
      audio.removeEventListener("ended", onEnded);
      audio.removeEventListener("error", onError);
      audio.pause();
    };
  }, [live]);

  const resetPlayer = useCallback(() => {
    const audio = audioRef.current;
    if (audio) audio.pause();
    setTrackId("");
    setPlaying(false);
    setAudioPos(0);
    setAudioDur(0);
    setAudioError("");
  }, []);

  /** 播放 / 暂停某条录音；点另一条会切换音源 */
  const toggleTrack = useCallback(
    async (item: MeetingItem) => {
      const audio = audioRef.current;
      if (!audio) return;
      setAudioError("");
      if (trackId === item.id) {
        if (audio.paused) {
          try {
            await audio.play();
          } catch {
            setAudioError("播放失败，请检查系统音频输出设备");
          }
        } else {
          audio.pause();
        }
        return;
      }
      audio.pause();
      setAudioLoading(item.id);
      setPlaying(false);
      setAudioPos(0);
      // 先用列表里的时长占位，audio 元数据到了再覆盖
      setAudioDur(item.duration_ms / 1000);
      setTrackId(item.id);
      try {
        const url = await meetingAudio(item.id);
        audio.src = url;
        audio.load();
        await audio.play();
      } catch (e) {
        // 快速切歌 / 停止会把上一段 play() 打断成 AbortError，不是真失败
        const aborted = e instanceof DOMException && e.name === "AbortError";
        if (!aborted) setAudioError(errorText(e));
      } finally {
        setAudioLoading("");
      }
    },
    [trackId]
  );

  const seekTrack = useCallback((value: number) => {
    const audio = audioRef.current;
    if (!audio) return;
    audio.currentTime = value;
    setAudioPos(value);
  }, []);

  const changeAutoGain = useCallback(
    (next: boolean) => {
      setAutoGain(next);
      setGainSaving(true);
      void meetingSetAutoGain(next)
        .then((settings) => setAutoGain(settings.autoGain))
        .catch((e) => {
          setAutoGain(!next);
          onToast(`保存音量设置失败：${errorText(e)}`);
        })
        .finally(() => setGainSaving(false));
    },
    [onToast]
  );

  // 计时：500ms 一拍，本地墙钟减去已暂停累计
  useEffect(() => {
    if (!live) return;
    const tick = () => {
      setLive((current) => {
        if (!current) return current;
        const pausedNow =
          current.pauseAt != null ? Date.now() - current.pauseAt : 0;
        setElapsedMs(
          Math.max(0, Date.now() - current.startWall - current.pausedAccumMs - pausedNow)
        );
        return current;
      });
    };
    tick();
    const timer = window.setInterval(tick, 500);
    return () => window.clearInterval(timer);
  }, [live]);

  const reallyStart = useCallback(async () => {
    setBusy(true);
    setError("");
    try {
      const info = await meetingStart(deviceId || null);
      setLive({
        name: info.name,
        device: info.device,
        sampleRate: info.sample_rate,
        dir: info.dir,
        marks: [],
        paused: false,
        startWall: Date.now(),
        pausedAccumMs: 0,
        pauseAt: null,
      });
      setElapsedMs(0);
      setConfirmPending(false);
      // 开始录音前停掉播放：外放会串进麦克风
      resetPlayer();
    } catch (e) {
      setError(errorText(e));
      // 开始失败多半是设备没了 / 被占用：顺手重扫，把下拉列表拉回真实状态
      void refreshDevices();
    } finally {
      setBusy(false);
    }
  }, [deviceId, refreshDevices, resetPlayer]);

  const start = useCallback(async () => {
    setError("");
    const state = privacy ?? (await meetingPrivacy().catch(() => null));
    if (state) setPrivacy(state);
    if (!state?.confirmed) {
      setConfirmPending(true);
      return;
    }
    await reallyStart();
  }, [privacy, reallyStart]);

  const confirmAndStart = useCallback(async () => {
    setBusy(true);
    setError("");
    try {
      await meetingConfirmPrivacy();
      setPrivacy((current) => (current ? { ...current, confirmed: true } : current));
      await reallyStart();
    } catch (e) {
      setError(errorText(e));
      setBusy(false);
    }
  }, [reallyStart]);

  const togglePause = useCallback(async () => {
    if (!live) return;
    setError("");
    try {
      if (live.paused) {
        await meetingResume();
        setLive((current) =>
          current
            ? {
                ...current,
                paused: false,
                pausedAccumMs: current.pausedAccumMs + (current.pauseAt ? Date.now() - current.pauseAt : 0),
                pauseAt: null,
              }
            : current
        );
      } else {
        await meetingPause();
        setLive((current) => (current ? { ...current, paused: true, pauseAt: Date.now() } : current));
      }
    } catch (e) {
      setError(errorText(e));
    }
  }, [live]);

  const addMark = useCallback(async () => {
    if (!live) return;
    setError("");
    try {
      const mark = await meetingAddMark(markLabel.trim() || null);
      setLive((current) => (current ? { ...current, marks: [...current.marks, mark] } : current));
      setMarkLabel("");
      onToast(`已在 ${fmtDuration(mark.ms / 1000)} 打标记`);
    } catch (e) {
      setError(errorText(e));
    }
  }, [live, markLabel, onToast]);

  const stop = useCallback(async () => {
    if (!live) return;
    setBusy(true);
    setError("");
    try {
      const result = await meetingStop();
      setLive(null);
      // 已经回到空闲视图：busy 要立刻复位，否则「开始录音」会短暂显示
      // 「启动中…」（列表刷新在下面，别让按钮文案跟着等）
      setBusy(false);
      setElapsedMs(0);
      // warning 非 null = 文件已保存但录制期间发生过写入告警：toast 告警
      // 内容并正常进列表，不打断流程
      onToast(
        result.warning ??
          `已保存「${result.name}」 · ${fmtDuration(result.duration_ms / 1000)} · ${fmtBytes(result.size)}`
      );
      await reloadList();
    } catch (e) {
      // 真失败：后端 stop 在收尾前就已清掉会话槽、停了流，留在录制视图是
      // 死路（再点结束只会报「没有正在进行的会议录音」）。退出到空闲视图，
      // 把错误就近显示在开始按钮附近；可能已落盘的部分靠刷新列表呈现。
      const text = errorText(e);
      setLive(null);
      setBusy(false);
      setElapsedMs(0);
      setError(`结束录音失败：${text}`);
      await reloadList();
    } finally {
      setBusy(false);
    }
  }, [live, onToast, reloadList]);

  const remove = useCallback(
    (item: MeetingItem) => {
      if (deleteBusyId) return;
      if (pendingDelete !== item.id) {
        setPendingDelete(item.id);
        onToast("再点一次删除这条录音");
        if (deleteTimer.current) window.clearTimeout(deleteTimer.current);
        deleteTimer.current = window.setTimeout(() => setPendingDelete(""), PENDING_RESET_MS);
        return;
      }
      setPendingDelete("");
      setDeleteBusyId(item.id);
      void meetingDelete(item.id)
        .then(() => {
          // 正在播放的录音被删：立刻停掉，别让播放器挂着已不存在的文件
          if (trackId === item.id) resetPlayer();
          onToast("录音已删除");
          return reloadList();
        })
        .catch((e) => onToast(`删除失败：${errorText(e)}`))
        .finally(() => setDeleteBusyId(""));
    },
    [deleteBusyId, pendingDelete, onToast, reloadList, resetPlayer, trackId]
  );

  const commitRename = useCallback(async () => {
    if (!renaming || renameSaving) return;
    const value = renaming.value.trim();
    if (!value || value === renaming.id) {
      setRenaming(null);
      setRenameError("");
      return;
    }
    setError("");
    setRenameSaving(true);
    try {
      await meetingRename(renaming.id, value);
      setRenaming(null);
      setRenameError("");
      onToast("已重命名");
      await reloadList();
    } catch (e) {
      // 重命名失败就显示在该行输入框下方（列表顶部的 error 在视野外）
      setRenameError(errorText(e));
    } finally {
      setRenameSaving(false);
    }
  }, [renaming, renameSaving, onToast, reloadList]);

  // ===== 录制中 =====
  if (live) {
    const lastMark = live.marks.length > 0 ? live.marks[live.marks.length - 1] : null;
    return (
      <div className="meet-page">
        <div className="meet-rec">
          <span className="meet-dot" data-paused={live.paused} />
          <span className="meet-timer">{fmtDuration(elapsedMs / 1000)}</span>
          <span className="meet-rec-meta">
            {live.device}
            <br />
            {live.sampleRate > 0 ? `${(live.sampleRate / 1000).toFixed(live.sampleRate % 1000 === 0 ? 0 : 1)}kHz 单声道` : ""}
            {live.paused ? " · 已暂停" : ""}
          </span>
        </div>

        <div className="meet-rec-actions">
          <button type="button" className="tool-btn" onClick={() => void togglePause()}>
            <Icon name={live.paused ? "play" : "pause"} size={12} />
            {live.paused ? "继续" : "暂停"}
          </button>
          <button
            type="button"
            className="tool-btn"
            disabled={busy}
            onClick={() => void addMark()}
          >
            打标记
          </button>
          <button
            type="button"
            className="tool-btn primary"
            disabled={busy}
            onClick={() => void stop()}
          >
            {busy ? "保存中…" : "结束录音"}
          </button>
        </div>

        <div className="meet-mark-row">
          <input
            value={markLabel}
            placeholder="标记备注（可选），如「结论」「行动项」"
            maxLength={60}
            onChange={(event) => setMarkLabel(event.target.value)}
            onKeyDown={(event) => {
              if (event.key === "Enter") {
                event.preventDefault();
                void addMark();
              }
            }}
          />
        </div>

        <div className="meet-marks-info">
          标记 {live.marks.length} 个
          {lastMark ? ` · 最近：${fmtDuration(lastMark.ms / 1000)}${lastMark.label ? `「${lastMark.label}」` : ""}` : ""}
          {" · 保存在 "}
          {live.dir}
        </div>

        <button type="button" className="tool-btn" disabled title="转写功能即将支持">
          转写（即将支持）
        </button>

        {error ? <div className="tool-error">{error}</div> : null}
      </div>
    );
  }

  // ===== 空闲：设备选择 + 隐私确认 + 最近列表 =====
  // 设备区提示只占一行，优先级：枚举失败 > 选中设备被拔 > 刚插上新设备 > 空列表
  const deviceNote = deviceError
    ? `读取设备失败：${deviceError}`
    : deviceGone ||
      deviceNew ||
      (devices.length === 0 ? "未检测到麦克风，插上后会自动出现在这里" : "");

  return (
    <div className="meet-page">
      {/* 播放器本体：不显示原生控件（岛里样式不可控），进度条自己画 */}
      <audio ref={audioRef} preload="metadata" />

      <div className="meet-setup">
        <div className="tool-row">
          <span className="tool-label">设备</span>
          <DevicePicker
            devices={devices}
            value={deviceId}
            scanning={scanning}
            onChange={(id) => {
              setDeviceId(id);
              setDeviceGone("");
              setDeviceNew("");
            }}
            onRefresh={() => void refreshDevices(true)}
          />
        </div>
        {deviceNote ? (
          <div className={`meet-device-note${deviceError ? " error" : ""}`}>{deviceNote}</div>
        ) : null}

        <div className="tool-row">
          <span className="tool-label">音量</span>
          <span className="meet-gain-hint">自动提升（保存时按峰值调响，不削波）</span>
          <Toggle
            checked={autoGain}
            disabled={gainSaving}
            onChange={changeAutoGain}
            label="保存时自动提升音量"
          />
        </div>

        {confirmPending ? (
          <div className="meet-privacy">
            <p>
              录音会保存到本机磁盘（{privacy?.dir ?? "~/.agenthud/meetings"}），不上传。
              请确认麦克风已连接，并遵守当地法律与他人隐私。
            </p>
            <div className="meet-privacy-actions">
              <button type="button" className="tool-btn" onClick={() => setConfirmPending(false)}>
                取消
              </button>
              <button
                type="button"
                className="tool-btn primary"
                disabled={busy}
                onClick={() => void confirmAndStart()}
              >
                确认并开始录音
              </button>
            </div>
          </div>
        ) : (
          <button
            type="button"
            className="tool-btn primary"
            disabled={busy}
            onClick={() => void start()}
          >
            {busy ? "启动中…" : "开始录音"}
          </button>
        )}

        {error ? <div className="tool-error">{error}</div> : null}
      </div>

      <div className="meet-list-head">
        <span className="tool-hint">
          {items.length > 0 ? `${items.length} 条 · 存于 ~/.agenthud/meetings` : "还没有录音"}
        </span>
        <button type="button" className="mini-text-btn" onClick={() => void reloadList()}>
          刷新
        </button>
      </div>

      {listError ? (
        <div className="tool-error">{listError}</div>
      ) : listLoading ? (
        <div className="tool-empty">读取中…</div>
      ) : items.length === 0 ? (
        <div className="tool-empty">
          录音会保存为本机 WAV 文件
          <br />
          结束后可打开文件夹、重命名或删除
        </div>
      ) : (
        <div className="meet-list">
          {items.map((item) => (
            <div className="meet-item" key={item.id}>
              {renaming?.id === item.id ? (
                <div className="meet-rename-wrap">
                  <div className="meet-rename">
                    <input
                      value={renaming.value}
                      autoFocus
                      maxLength={100}
                      onChange={(event) =>
                        setRenaming((current) =>
                          current ? { ...current, value: event.target.value } : current
                        )
                      }
                      onKeyDown={(event) => {
                        if (event.key === "Enter") {
                          event.preventDefault();
                          void commitRename();
                        }
                        if (event.key === "Escape") {
                          setRenaming(null);
                          setRenameError("");
                        }
                      }}
                    />
                    <button
                      type="button"
                      className="tool-btn primary"
                      disabled={renameSaving}
                      onClick={() => void commitRename()}
                    >
                      {renameSaving ? "保存中…" : "保存"}
                    </button>
                    <button
                      type="button"
                      className="tool-btn"
                      disabled={renameSaving}
                      onClick={() => {
                        setRenaming(null);
                        setRenameError("");
                      }}
                    >
                      取消
                    </button>
                  </div>
                  {renameError ? <div className="tool-error meet-inline-error">{renameError}</div> : null}
                </div>
              ) : (
                <>
                  <div className="meet-main">
                    <span className="meet-name">{item.name}</span>
                    <span className="meet-meta">
                      {fmtWhen(item.started_at)} · {fmtDuration(item.duration_ms / 1000)}
                      {item.marks.length > 0 ? (
                        <>
                          {" · "}
                          <button
                            type="button"
                            className="meet-marks-toggle"
                            aria-expanded={marksOpen === item.id}
                            title={marksOpen === item.id ? "收起标记明细" : "查看标记明细"}
                            onClick={() =>
                              setMarksOpen((current) => (current === item.id ? "" : item.id))
                            }
                          >
                            标记 {item.marks.length}
                          </button>
                        </>
                      ) : null}
                      {" · "}
                      {fmtBytes(item.size)}
                      {item.sample_rate > 0
                        ? ` · ${(item.sample_rate / 1000).toFixed(item.sample_rate % 1000 === 0 ? 0 : 1)}kHz`
                        : ""}
                    </span>
                    {marksOpen === item.id && item.marks.length > 0 ? (
                      <div className="meet-marks-list">
                        {item.marks.map((mark, index) => (
                          <div className="meet-mark-line" key={`${item.id}-${index}`}>
                            <span className="meet-mark-at">{fmtDuration(mark.ms / 1000)}</span>
                            <span className="meet-mark-label">{mark.label || "标记"}</span>
                          </div>
                        ))}
                      </div>
                    ) : null}
                  </div>
                  <div className="meet-actions">
                    <button
                      type="button"
                      className={`clip-action ${trackId === item.id && playing ? "on" : ""}`}
                      title={trackId === item.id && playing ? "暂停播放" : "播放录音"}
                      aria-label={trackId === item.id && playing ? "暂停播放" : "播放录音"}
                      disabled={audioLoading === item.id}
                      onClick={() => void toggleTrack(item)}
                    >
                      <Icon name={trackId === item.id && playing ? "pause" : "play"} size={12} />
                    </button>
                    <button
                      type="button"
                      className="clip-action"
                      title="打开所在文件夹"
                      aria-label="打开所在文件夹"
                      onClick={() =>
                        void meetingOpenFolder(item.id).catch((e) =>
                          onToast(`打开失败：${errorText(e)}`)
                        )
                      }
                    >
                      <Icon name="folder" size={12} />
                    </button>
                    <button
                      type="button"
                      className="clip-action"
                      title="重命名"
                      aria-label="重命名"
                      onClick={() => {
                        setRenameError("");
                        setRenaming({ id: item.id, value: item.name });
                      }}
                    >
                      <Icon name="edit" size={12} />
                    </button>
                    <button
                      type="button"
                      className={`clip-action danger ${pendingDelete === item.id ? "on" : ""}`}
                      title={pendingDelete === item.id ? "再点一次删除" : "删除"}
                      aria-label="删除"
                      disabled={deleteBusyId === item.id}
                      onClick={() => remove(item)}
                    >
                      <Icon name="trash" size={12} />
                    </button>
                  </div>
                  {/* 播放器独占一整行（flex-basis:100% 换行），不吃动作列的宽度 */}
                  {trackId === item.id ? (
                    <div className="meet-player">
                      <button
                        type="button"
                        className="meet-play-btn"
                        aria-label={playing ? "暂停" : "播放"}
                        onClick={() => void toggleTrack(item)}
                      >
                        <Icon name={playing ? "pause" : "play"} size={11} />
                      </button>
                      <span className="meet-player-time">
                        {fmtDuration(Math.min(audioPos, audioDur || audioPos))}
                      </span>
                      <input
                        type="range"
                        className="meet-player-seek"
                        aria-label="播放进度"
                        min={0}
                        max={Math.max(1, audioDur)}
                        step={0.1}
                        value={Math.min(audioPos, Math.max(1, audioDur))}
                        onChange={(event) => seekTrack(Number(event.target.value))}
                      />
                      <span className="meet-player-time">{fmtDuration(audioDur)}</span>
                      <button
                        type="button"
                        className="clip-action"
                        title="停止播放"
                        aria-label="停止播放"
                        onClick={resetPlayer}
                      >
                        <Icon name="close" size={11} />
                      </button>
                    </div>
                  ) : null}
                  {trackId === item.id && audioError ? (
                    <div className="tool-error meet-inline-error">{audioError}</div>
                  ) : null}
                </>
              )}
            </div>
          ))}
        </div>
      )}
    </div>
  );
}
