/**
 * 设置窗的草稿态数据层（负责绑定 / 脏值 / 保存语义）。
 */
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { onEvent } from "../../../island/api";
import {
  getSettingsDoc,
  reloadSettings,
  saveSettings,
  type SettingsDoc,
  type SettingsSaveResult,
} from "./settingsApi";
import { validateDraft, type ValidationError } from "./schema";

/** 控件改动到自动写盘的防抖时长（毫秒）：连续拖动滑动条只在停手后提交一次 */
const AUTO_SAVE_DELAY_MS = 400;

/** 扁平点路径 map */
export type Flat = Record<string, unknown>;

export type StatusRole = "hint" | "accent" | "ok" | "danger";

export interface SettingsStatus {
  text: string;
  role: StatusRole;
}

/** 默认状态文案（无改动时显示） */
export const DEFAULT_STATUS_TEXT = "修改实时生效，自动保存";

export interface SettingsDraft {
  loading: boolean;
  loadError: string | null;
  doc: SettingsDoc | null;
  draft: Flat;
  defaults: Flat;
  snapshot: Flat;
  dirtyCount: number;
  patch: Flat;
  errors: Record<string, string>;
  status: SettingsStatus;
  path: string;
  dataDir: string;
  unsupported: string[];
  /** 每次保存成功 +1：热键状态等「保存后复查」的页面据此刷新 */
  saveTick: number;
  value: (key: string) => unknown;
  isUnsupported: (key: string) => boolean;
  setValue: (key: string, value: unknown) => void;
  setMany: (entries: Record<string, unknown>) => void;
  resetPage: (keys: string[]) => void;
  setStatus: (text: string, role: StatusRole, sticky?: boolean) => void;
  validate: () => ValidationError[];
  /** 内部提交函数：校验 → island_settings_save(patch)。自动保存与手动 reload 复用 */
  save: () => Promise<boolean>;
  /** 冲刷尚未到期的防抖保存（关窗 / 卸载前尽力调用一次） */
  flushPendingSave: () => void;
  reload: () => Promise<void>;
  /** 初次读取失败后的重试（重新读 island_settings_doc） */
  retry: () => Promise<void>;
  /** 放弃未保存的修改：草稿回到 snapshot（高级页重载确认框用） */
  discard: () => void;
}

/** 深拷贝（文档里全是 JSON 值） */
function clone<T>(value: T): T {
  return JSON.parse(JSON.stringify(value)) as T;
}

/** 稳定的值比较：JSON 序列化（数组 / 对象整体比较，与后端 patch 口径一致） */
function canon(value: unknown): string {
  return JSON.stringify(value ?? null);
}

/** 嵌套对象 → 扁平点路径 map；数组 / 空值整体作为叶子 */
export function flatten(value: unknown, prefix = "", out: Flat = {}): Flat {
  if (value === null || typeof value !== "object" || Array.isArray(value)) {
    if (prefix) out[prefix] = value;
    return out;
  }
  for (const [key, child] of Object.entries(value as Record<string, unknown>)) {
    flatten(child, prefix ? `${prefix}.${key}` : key, out);
  }
  return out;
}

/** 脏值数量 */
export function countDirty(draft: Flat, snapshot: Flat): number {
  let count = 0;
  for (const [key, value] of Object.entries(draft)) {
    if (!(key in snapshot) || canon(value) !== canon(snapshot[key])) count += 1;
  }
  return count;
}

/** 只含差异键的 patch */
export function buildPatch(draft: Flat, snapshot: Flat): Flat {
  const patch: Flat = {};
  for (const [key, value] of Object.entries(draft)) {
    if (!(key in snapshot) || canon(value) !== canon(snapshot[key])) {
      patch[key] = clone(value);
    }
  }
  return patch;
}

/** 本地时间 HH:MM:SS（保存成功文案用） */
function nowHms(): string {
  const now = new Date();
  const pad = (n: number) => String(n).padStart(2, "0");
  return `${pad(now.getHours())}:${pad(now.getMinutes())}:${pad(now.getSeconds())}`;
}

/** 把事件 / 异常转成一句可读的原因 */
function errorText(error: unknown): string {
  if (typeof error === "string") return error;
  if (error instanceof Error) return error.message;
  try {
    return JSON.stringify(error);
  } catch {
    return String(error);
  }
}

export function useSettingsDraft(): SettingsDraft {
  const [doc, setDoc] = useState<SettingsDoc | null>(null);
  const [draft, setDraft] = useState<Flat>({});
  const [defaults, setDefaults] = useState<Flat>({});
  const [snapshot, setSnapshot] = useState<Flat>({});
  const [errors, setErrors] = useState<Record<string, string>>({});
  const [status, setStatusState] = useState<SettingsStatus>({
    text: DEFAULT_STATUS_TEXT,
    role: "hint",
  });
  const [loading, setLoading] = useState(true);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [unsupported, setUnsupported] = useState<string[]>([]);
  const [saveTick, setSaveTick] = useState(0);

  // 事件回调 / 保存流程里要读到「最新一次」的值，避免闭包拿到旧 state
  const draftRef = useRef<Flat>({});
  const snapshotRef = useRef<Flat>({});
  const defaultsRef = useRef<Flat>({});
  const stickyRef = useRef<string | null>(null);
  const aliveRef = useRef(true);
  /** 文档请求序号：并发重载时只接受最后一次结果 */
  const requestRef = useRef(0);
  /** 自动保存防抖定时器 */
  const autoSaveTimerRef = useRef<number | null>(null);
  /** 保存函数的最新引用：防抖回调到点时才读取，避免闭包拿到旧 save */
  const saveRef = useRef<() => Promise<boolean>>(() => Promise.resolve(false));

  useEffect(() => {
    aliveRef.current = true;
    return () => {
      aliveRef.current = false;
      // 卸载时若有未提交脏值，尽力冲刷一次（fire-and-forget）
      if (autoSaveTimerRef.current != null) {
        window.clearTimeout(autoSaveTimerRef.current);
        autoSaveTimerRef.current = null;
        void saveRef.current();
      }
    };
  }, []);

  /** 取消尚未到期的自动保存定时器 */
  const clearAutoSaveTimer = useCallback(() => {
    if (autoSaveTimerRef.current != null) {
      window.clearTimeout(autoSaveTimerRef.current);
      autoSaveTimerRef.current = null;
    }
  }, []);

  /** 控件改动后安排一次防抖自动保存：连续拖动只在停手 400ms 后提交一次 */
  const scheduleAutoSave = useCallback(() => {
    clearAutoSaveTimer();
    autoSaveTimerRef.current = window.setTimeout(() => {
      autoSaveTimerRef.current = null;
      void saveRef.current();
    }, AUTO_SAVE_DELAY_MS);
  }, [clearAutoSaveTimer]);

  /** 冲刷未到期的防抖保存（关窗 / 卸载前调用，立即提交当前 patch） */
  const flushPendingSave = useCallback(() => {
    if (autoSaveTimerRef.current == null) return;
    clearAutoSaveTimer();
    void saveRef.current();
  }, [clearAutoSaveTimer]);

  /** 设置状态；sticky=true 时保留到下一次控件改动 / 保存前 */
  const setStatus = useCallback(
    (text: string, role: StatusRole, sticky = false) => {
      stickyRef.current = sticky ? text : null;
      setStatusState({ text, role });
    },
    []
  );

  /** 按给定草稿重算脏值文案（控件改动时先清 sticky） */
  const refreshDirty = useCallback((nextDraft: Flat, clearSticky: boolean) => {
    if (clearSticky) stickyRef.current = null;
    if (stickyRef.current) return;
    const n = countDirty(nextDraft, snapshotRef.current);
    setStatusState(
      n
        ? { text: `${n} 项修改未保存`, role: "accent" }
        : { text: DEFAULT_STATUS_TEXT, role: "hint" }
    );
  }, []);

  const commitDraft = useCallback((next: Flat) => {
    draftRef.current = next;
    setDraft(next);
  }, []);

  const commitSnapshot = useCallback((next: Flat) => {
    snapshotRef.current = next;
    setSnapshot(next);
  }, []);

  /** 用一份 doc payload 重建 baseline；external=外部事件静默刷新，不动状态栏 */
  const applyPayload = useCallback(
    (payload: SettingsDoc, reason: "load" | "reload" | "external") => {
      const nextSnapshot = flatten(payload.data ?? {});
      const nextDefaults = flatten(payload.defaults ?? {});
      commitSnapshot(nextSnapshot);
      defaultsRef.current = nextDefaults;
      setDefaults(nextDefaults);
      commitDraft({ ...nextSnapshot });
      setUnsupported(Array.isArray(payload.unsupported) ? payload.unsupported : []);
      setErrors({});
      setDoc(payload);
      setLoadError(null);
      if (reason === "external") {
        // 岛端配置广播：静默同步基准，保留当前状态栏文案（含「已自动保存」提示）
        return;
      }
      stickyRef.current = null;
      setStatusState({ text: DEFAULT_STATUS_TEXT, role: "hint" });
      if (reason === "reload") {
        // 手动重载：明确告诉用户草稿已被磁盘内容替换
        setStatus("已重新读取设置", "ok", true);
      }
    },
    [commitDraft, commitSnapshot, setStatus]
  );

  /** 首帧 / 重试：读 island_settings_doc */
  const load = useCallback(async () => {
    const seq = ++requestRef.current;
    setLoading(true);
    try {
      const payload = await getSettingsDoc();
      if (!aliveRef.current || seq !== requestRef.current) return;
      applyPayload(payload, "load");
      setLoadError(null);
    } catch (error) {
      if (!aliveRef.current || seq !== requestRef.current) return;
      setLoadError(errorText(error));
    } finally {
      if (aliveRef.current && seq === requestRef.current) setLoading(false);
    }
  }, [applyPayload]);

  useEffect(() => {
    void load();
  }, [load]);

  /** 岛端配置 / 外观变化：静默刷新（有未提交改动时跳过，避免冲掉正在编辑的草稿） */
  useEffect(() => {
    const handleExternal = () => {
      if (!aliveRef.current) return;
      // 有未提交改动时不覆盖草稿；自动保存完成后快照会更新，下一轮事件自然同步
      if (countDirty(draftRef.current, snapshotRef.current) > 0) return;
      getSettingsDoc()
        .then((payload) => {
          if (!aliveRef.current) return;
          // 请求期间用户可能已经动手改了：再判一次，避免把草稿冲掉
          if (countDirty(draftRef.current, snapshotRef.current) > 0) return;
          applyPayload(payload, "external");
        })
        .catch(() => {
          /* 静默：外部事件刷新失败不影响当前草稿 */
        });
    };
    const offConfig = onEvent("zbar://island-config", handleExternal);
    const offAppearance = onEvent("zbar://appearance-changed", handleExternal);
    return () => {
      offConfig();
      offAppearance();
    };
  }, [applyPayload]);

  const dirtyCount = useMemo(() => countDirty(draft, snapshot), [draft, snapshot]);
  const patch = useMemo(() => buildPatch(draft, snapshot), [draft, snapshot]);

  const value = useCallback((key: string) => draft[key], [draft]);

  const isUnsupported = useCallback(
    (key: string) => unsupported.includes(key),
    [unsupported]
  );

  /** 控件改动：写草稿、清该行错误、清 sticky 并刷新脏值文案 */
  const setValue = useCallback(
    (key: string, next: unknown) => {
      const merged = { ...draftRef.current, [key]: next };
      commitDraft(merged);
      setErrors((prev) => {
        if (!(key in prev)) return prev;
        const copy = { ...prev };
        delete copy[key];
        return copy;
      });
      refreshDirty(merged, true);
      scheduleAutoSave();
    },
    [commitDraft, refreshDirty, scheduleAutoSave]
  );

  const setMany = useCallback(
    (entries: Record<string, unknown>) => {
      const merged = { ...draftRef.current, ...entries };
      commitDraft(merged);
      setErrors((prev) => {
        let changed = false;
        const copy = { ...prev };
        for (const key of Object.keys(entries)) {
          if (key in copy) {
            delete copy[key];
            changed = true;
          }
        }
        return changed ? copy : prev;
      });
      refreshDirty(merged, true);
      scheduleAutoSave();
    },
    [commitDraft, refreshDirty, scheduleAutoSave]
  );

  /** 恢复本页默认值：只改草稿，随后由防抖自动写盘 */
  const resetPage = useCallback(
    (keys: string[]) => {
      const next = { ...draftRef.current };
      for (const key of keys) {
        if (key in defaultsRef.current) next[key] = clone(defaultsRef.current[key]);
      }
      commitDraft(next);
      setErrors((prev) => {
        const copy = { ...prev };
        for (const key of keys) delete copy[key];
        return copy;
      });
      refreshDirty(next, true);
      scheduleAutoSave();
    },
    [commitDraft, refreshDirty, scheduleAutoSave]
  );

  /** 全量校验：写入 errors（行内红字），返回按页面顺序排列的错误列表 */
  const validate = useCallback((): ValidationError[] => {
    const found = validateDraft((key) => draftRef.current[key]);
    const map: Record<string, string> = {};
    for (const item of found) {
      if (!(item.key in map)) map[item.key] = item.message;
    }
    setErrors(map);
    return found;
  }, []);

  /** 内部提交：校验 → 无脏值 → island_settings_save(patch)。由自动保存 / 冲刷触发 */
  const save = useCallback(async (): Promise<boolean> => {
    const found = validate();
    if (found.length) {
      const more = found.length > 1 ? `，另外还有 ${found.length - 1} 项` : "";
      setStatus(`${found[0].message}${more}`, "danger", true);
      return false;
    }
    const nextPatch = buildPatch(draftRef.current, snapshotRef.current);
    if (!Object.keys(nextPatch).length) {
      // 防抖到期时改动可能已被改回原值：静默返回，不写无谓的提示
      return true;
    }
    try {
      const result: SettingsSaveResult = await saveSettings(nextPatch);
      const nextSnapshot = flatten(result.data ?? {});
      // 后端没回写的键（未接入 / 被忽略）保留草稿里的值，脏值提示如实反映
      const nextDraft: Flat = { ...nextSnapshot };
      for (const [key, item] of Object.entries(draftRef.current)) {
        if (!(key in nextSnapshot)) nextDraft[key] = item;
      }
      commitSnapshot(nextSnapshot);
      commitDraft(nextDraft);
      setUnsupported(Array.isArray(result.unsupported) ? result.unsupported : []);
      setErrors({});
      setStatus(`已自动保存 · ${nowHms()}`, "ok", true);
      setSaveTick((tick) => tick + 1);
      return true;
    } catch (error) {
      // 保留草稿脏值，下轮控件改动会重新安排自动保存
      setStatus(`保存失败：${errorText(error)}`, "danger", true);
      return false;
    }
  }, [commitDraft, commitSnapshot, setStatus, validate]);

  // 防抖回调到点时才读最新 save（save 定义依赖 validate / commit，引用稳定但避免闭包隐患）
  saveRef.current = save;

  /** 手动重读文件（有脏值时仍然覆盖草稿，调用方负责确认） */
  const reload = useCallback(async () => {
    try {
      const payload = await reloadSettings();
      if (!aliveRef.current) return;
      applyPayload(payload, "reload");
    } catch (error) {
      setStatus(`重新读取失败：${errorText(error)}`, "danger", true);
    }
  }, [applyPayload, setStatus]);

  /** 放弃未保存的修改：草稿回到最后一次保存 / 读取的基准 */
  const discard = useCallback(() => {
    clearAutoSaveTimer();
    commitDraft({ ...snapshotRef.current });
    setErrors({});
    stickyRef.current = null;
    setStatusState({ text: DEFAULT_STATUS_TEXT, role: "hint" });
  }, [clearAutoSaveTimer, commitDraft]);

  return {
    loading,
    loadError,
    doc,
    draft,
    defaults,
    snapshot,
    dirtyCount,
    patch,
    errors,
    status,
    path: doc?.path ?? "",
    dataDir: doc?.data_dir ?? "",
    unsupported,
    saveTick,
    value,
    isUnsupported,
    setValue,
    setMany,
    resetPage,
    setStatus,
    validate,
    save,
    flushPendingSave,
    reload,
    retry: load,
    discard,
  };
}
