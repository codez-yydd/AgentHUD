/**
 * localStorage 持久化缓存：用于面板冷启动首屏秒开。
 *
 * DataProvider 把数据常驻在 React state（内存）里，热重显（切页面再切回）
 * 能瞬时恢复；但进程重启后内存清空，冷启动仍需等首次请求。这里把上次结果
 * 同步落盘，冷启动时先用缓存渲染、后台请求刷新后覆盖 —— 与内存缓存互补。
 *
 * 所有操作 try-catch：配额超限、序列化失败、隐私模式等情况下静默降级，
 * 绝不影响正常的数据加载流程（缓存只是锦上添花）。
 */

/** 读取缓存；不存在或解析失败返回 null。 */
export function loadCache<T>(key: string): T | null {
  try {
    const raw = localStorage.getItem(key);
    return raw ? (JSON.parse(raw) as T) : null;
  } catch {
    return null;
  }
}

/** 写入缓存；失败静默（不抛错，避免影响主流程）。 */
export function saveCache<T>(key: string, data: T): void {
  try {
    localStorage.setItem(key, JSON.stringify(data));
  } catch {
    // 忽略：QuotaExceededError、隐私模式、序列化失败等
  }
}

/** 删除指定缓存键；失败静默（缓存键结构升级时清理旧版本数据用）。 */
export function removeCache(key: string): void {
  try {
    localStorage.removeItem(key);
  } catch {
    // 忽略：隐私模式等
  }
}

/**
 * 按前缀聚合读取缓存子键（拆 key 持久化的读路径）。
 *
 * 拆 key 持久化把一个大缓存（原整键全量 JSON）拆成
 * `{前缀}{条目key}` 独立子键，冷启动时用本函数一次遍历 localStorage
 * 聚合恢复成完整 Record。单键损坏（JSON 解析失败）跳过该键不拖垮整体；
 * 隐私模式等异常整体静默降级为空对象，与三件套同款兜底口径。
 */
export function loadCacheByPrefix<T>(prefix: string): Record<string, T> {
  const out: Record<string, T> = {};
  try {
    // 快照长度：遍历中不会新增键，但防御性缓存上限，避免依赖实现细节
    const len = localStorage.length;
    for (let i = 0; i < len; i++) {
      const key = localStorage.key(i);
      if (!key || !key.startsWith(prefix)) continue;
      const raw = localStorage.getItem(key);
      if (!raw) continue;
      try {
        out[key.slice(prefix.length)] = JSON.parse(raw) as T;
      } catch {
        // 单键损坏：跳过，等后台刷新轮重拉覆盖
      }
    }
  } catch {
    // 忽略：隐私模式等，返回已聚合部分（或空对象）
  }
  return out;
}
