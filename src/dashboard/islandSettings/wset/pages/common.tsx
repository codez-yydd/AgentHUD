/**
 * 页面公共道具：14 个已接线页面共用的入参、只读取值器、路径浏览与「未接入」标签。
 */
import type { ReactNode } from "react";
import { pickFile } from "../settingsApi";
import type { SettingsDraft } from "../useSettingsDraft";
import { Tag } from "../controls";

export interface PageProps {
  draft: SettingsDraft;
  /** 保存成功计数（热键注册状态等「保存后复查」的页面用） */
  saveTick: number;
  /** 热键录制状态上报：录制中窗口的 Esc 只取消录制 */
  onRecordingChange?: (recording: boolean) => void;
}

/**
 * 行标题后的「未接入」标签：doc payload 的 unsupported 命中了这个点路径时显示。
 * 未命中返回 null（Row 的 tag 接受 undefined / null）。
 */
export function UnsupportedTag({
  draft,
  keyName,
}: {
  draft: SettingsDraft;
  keyName: string;
}): ReactNode {
  if (!draft.isUnsupported(keyName)) return null;
  return <Tag tone="accent">未接入</Tag>;
}

/** 字符串值（缺键 / 类型不符时回退） */
export function readText(draft: SettingsDraft, key: string, fallback = ""): string {
  const value = draft.value(key);
  return typeof value === "string" ? value : fallback;
}

/** 布尔值（缺键按 false） */
export function readFlag(draft: SettingsDraft, key: string, fallback = false): boolean {
  const value = draft.value(key);
  return typeof value === "boolean" ? value : fallback;
}

/** 数字值（缺键 / NaN 时回退；小数保留原值） */
export function readNumber(draft: SettingsDraft, key: string, fallback: number): number {
  const value = Number(draft.value(key));
  return Number.isFinite(value) ? value : fallback;
}

/** 字符串数组（过滤非字符串项，列表控件不接受 unknown） */
export function readStringList(draft: SettingsDraft, key: string): string[] {
  const value = draft.value(key);
  if (!Array.isArray(value)) return [];
  return value.filter((item): item is string => typeof item === "string");
}

/** 结构化列表（过滤非对象项；字段顺序 / 额外字段原样保留） */
export function readRecords(draft: SettingsDraft, key: string): Record<string, unknown>[] {
  const value = draft.value(key);
  if (!Array.isArray(value)) return [];
  return value.filter(
    (item): item is Record<string, unknown> =>
      Boolean(item) && typeof item === "object" && !Array.isArray(item)
  );
}

/** 事件 / 异常 → 一句可读原因（状态栏用） */
export function errorText(error: unknown): string {
  if (typeof error === "string") return error;
  if (error instanceof Error) return error.message;
  try {
    return JSON.stringify(error);
  } catch {
    return String(error);
  }
}

/**
 * 系统对话框选路径并写进草稿（dsh.sessions_dir / 启动器 / 翻译配置等路径行用）。
 * 浏览器预览会拿到 mock 假路径；取消或失败时保持原值，失败原因走页脚状态。
 */
export async function browsePath(
  draft: SettingsDraft,
  key: string,
  options: { directory?: boolean; title?: string } = {}
): Promise<void> {
  try {
    const picked = await pickFile({
      mode: "open",
      title: options.title,
      directory: options.directory,
    });
    if (picked) draft.setValue(key, picked);
  } catch (error) {
    draft.setStatus(`选择路径失败：${errorText(error)}`, "danger", true);
  }
}
