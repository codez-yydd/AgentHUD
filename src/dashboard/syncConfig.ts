/**
 * 总览页 / 请求日志页共用的同步配置快路径（模块级缓存 + 超时兜底）。
 *
 * 两页都要等「同步配置」才能确定数据范围口径（本机 / 全部设备），而配置是
 * 本地文件读取、两页会先后或反复读取：
 *  - 模块级缓存：任一页读到有效配置后记下，后续读取命中即立即 resolve，
 *    不再等一轮 IPC（重进页面不重复等待）；
 *  - 读取失败 / 超时按 null（未启用同步）处理，不卡首屏；null 不进缓存
 *    （下次读取仍会重试），瞬时的读取失败不会抹掉已知的有效配置；
 *  - refresh = true：跳过缓存快路径，强制走一轮 IPC 并刷新缓存（总览页每次
 *    取数都用它：面板里刚改过同步开关的话，范围控件不该停在旧状态）。
 *
 * 缓存只存在于本窗口的 JS 模块内存里，不落盘；进程退出即失效。
 */
import { getSyncConfig } from "../api";
import type { SyncConfig } from "../types";

/** 配置等待上限：超时按「未启用同步」处理（不卡首屏） */
const SYNC_CONFIG_TIMEOUT_MS = 5000;

/** 最近一次读到的有效配置（模块级记忆；null = 尚未读到过） */
let cachedSyncConfig: SyncConfig | null = null;

/** 同步 peek（不发 IPC）：供懒初始化 useState 用——命中缓存的页面首帧即为
 *  「配置就绪」状态，范围控件不短暂禁用、数据范围不回落本机口径。 */
export function peekSyncConfigCached(): SyncConfig | null {
  return cachedSyncConfig;
}

/** 读取同步配置：默认命中缓存立即返回（不发 IPC）；refresh = true 强制走
 *  一轮 IPC 并刷新缓存。读取失败 / 超时（SYNC_CONFIG_TIMEOUT_MS）resolve
 *  null，调用方按未启用同步处理（本 Promise 不 reject）。 */
export function readSyncConfigCached(
  options: { refresh?: boolean } = {}
): Promise<SyncConfig | null> {
  if (!options.refresh && cachedSyncConfig != null) {
    return Promise.resolve(cachedSyncConfig);
  }
  return new Promise((resolve) => {
    const timer = window.setTimeout(() => resolve(null), SYNC_CONFIG_TIMEOUT_MS);
    getSyncConfig()
      .catch(() => null)
      .then((config) => {
        window.clearTimeout(timer);
        if (config != null) cachedSyncConfig = config;
        resolve(config);
      });
  });
}
