/**
 * 价格设置页（主窗口大窗版，300px 窄窗 PricingPanel 的大窗形态）。
 *
 * 功能与窄窗对齐：币种切换（本页查看口径，人民币按汇率折算展示，编辑恒为
 * 美元价）、「检查价格更新」（内置参考表差异 → 勾选应用 / 收起）、
 * models.dev 在线同步、模型价格编辑（草稿 + 失焦提交 + 统一保存）、按归一
 * 名合并的删除（即时落盘 + 隐藏名单）、隐藏模型恢复。大窗把窄窗的「每模型
 * 一张卡片」改成全宽宽表：吸顶表头、行内编辑、搜索过滤、来源标识、新增模型
 * 表单；页头（含保存按钮）吸顶，长表滚动时保存动作始终可见。
 *
 * 币种口径：默认按语言推导（中文=人民币、英文=美元，与总览/对比页同规则），
 * 页内切换只改本页展示、不写偏好也不改全局币种——币种持久化（localStorage
 * 偏好键 + 后端偏好）由面板 App.tsx 独占（启动/切语言强制回写，币种实际从属
 * 语言），主窗口侧不再读写该键。
 *
 * 草稿（draft）放模块级单例：切侧栏页面会卸载本组件，放组件 state 里草稿
 * 会随卸载丢失；单例在窗口会话内存活，重新挂载时恢复。
 *
 * 纯逻辑（canonicalModelId / lookupPrice / 金额格式化 / 草稿解析 / 行合并）
 * 与窄窗同源：模型层在 ./pricing/helpers（复制改造），窄窗原件不动。
 */
import {
  useCallback,
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
} from "react";
import type {
  ApplyPriceItem,
  Currency,
  ModelInfo,
  ModelPrice,
  PricingConfig,
  PricingDiff,
} from "../types";
import {
  applyPricingUpdates,
  checkPricingUpdates,
  fetchFxRate,
  fetchModels,
  fetchPricing,
  getCursorConfig,
  savePricing,
  syncModelsdevPricing,
} from "../api";
import { canonicalModelId, lookupPrice } from "../modelName";
import { AlertBanner } from "../layout";
import { useI18n } from "../i18n";
import { Card, Skeleton } from "./primitives";
import { currencyOf } from "./overview/derive";
import { DiffPanel } from "./pricing/DiffPanel";
import { PriceTable } from "./pricing/PriceTable";
import {
  PRICE_FIELDS,
  ZERO_PRICE,
  buildPriceRows,
  parseDraft,
  type PriceRow,
} from "./pricing/helpers";

/**
 * 模块级草稿单例：切侧栏页面会卸载组件，草稿放在组件 state 里会丢；
 * 提到模块级（同一窗口会话内存活），组件挂载时从这里初始化、每次变更写回。
 */
let pricingDraftStore: Record<string, string> = {};

export function PricingPage() {
  const { locale, t } = useI18n();
  const [pricing, setPricing] = useState<PricingConfig | null>(null);
  const [models, setModels] = useState<ModelInfo[]>([]);
  // 已落盘快照（JSON 串）：dirty 判定「内存 pricing ≠ 最后一次成功落盘的内容」
  const [savedJson, setSavedJson] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);
  const [savedFlash, setSavedFlash] = useState(false);
  // 应用价格更新成功后的轻提示（「已应用 N 项」，2s 后自动消失）
  const [appliedCount, setAppliedCount] = useState<number | null>(null);
  const [error, setError] = useState<string | null>(null);
  // USD→CNY 汇率：¥ 视图的折算展示用（价格只存美元，人民币一律实时折算）
  const [fxRate, setFxRate] = useState(7.2);
  const [fxLoading, setFxLoading] = useState(false);
  // 币种：默认按语言推导（中文=人民币、英文=美元），页内切换只改本页展示
  const [currency, setCurrency] = useState<Currency>(() => currencyOf(locale));
  // 草稿：把每个输入框的编辑值作为字符串暂存，避免小数点输入被 parseFloat 吞掉。
  // key = `${modelId}|${field}`；初值取模块级单例（跨页面切换不丢草稿）
  const [draft, setDraft] = useState<Record<string, string>>(
    () => pricingDraftStore
  );
  // draft 的 ref 镜像：onBlur 提交时读最新值，避免把回调绑进 draft 依赖导致整表重渲染。
  // 用 useLayoutEffect 同步：渲染后立刻刷新，保证任何后续事件（onBlur）读到的是最新草稿
  const draftRef = useRef<Record<string, string>>({});
  useLayoutEffect(() => {
    draftRef.current = draft;
  }, [draft]);
  // 草稿写回模块级单例：任何 setDraft（含清空、删除/新增模型的裁剪）都同步，
  // 页面卸载后单例保留最后一次内容，重新挂载时初始化 state 恢复
  useEffect(() => {
    pricingDraftStore = draft;
  }, [draft]);
  // pricing 的 ref 镜像：删除/恢复是即时落盘动作（不走草稿+保存），回调以稳定引用
  // 绑进 memo 行与恢复列表，点击时经 ref 读最新配置，避免回调依赖 pricing
  const pricingRef = useRef<PricingConfig | null>(null);
  useLayoutEffect(() => {
    pricingRef.current = pricing;
  }, [pricing]);

  // 表格工具条状态：搜索过滤、新增模型表单、隐藏名单展开
  const [query, setQuery] = useState("");
  const [newId, setNewId] = useState("");
  const [newPrice, setNewPrice] = useState<Record<keyof ModelPrice, string>>({
    input: "",
    output: "",
    cache_read: "",
  });
  const [hiddenOpen, setHiddenOpen] = useState(false);
  // 待确认删除的模型 id：行内 × 只进确认浮层（窄窗面板 ConfirmDialog 的大窗
  // 同构形态），确认后才执行 handleRemove 即时落盘；null = 未在确认中
  const [removeTarget, setRemoveTarget] = useState<string | null>(null);

  // ===== 价格同步（内置参考表 diff 提示）=====
  // updateCount：待应用的差异总数（用于按钮红点）；diffPanel：是否展开差异面板
  const [updateCount, setUpdateCount] = useState(0);
  const [diffPanel, setDiffPanel] = useState(false);
  const [diff, setDiff] = useState<PricingDiff | null>(null);
  // 差异参考源：builtin = 编译期内置表（检查更新）；modelsdev = models.dev 在线官方价。
  // 两个来源互斥展示：点哪个按钮就用哪个结果覆盖当前 diff 状态
  const [diffSource, setDiffSource] = useState<"builtin" | "modelsdev">(
    "builtin"
  );
  // diffSource 的 ref 镜像（与 draftRef 同模式）：晚到的异步结果落地前校验当前来源
  const diffSourceRef = useRef<"builtin" | "modelsdev">("builtin");
  useLayoutEffect(() => {
    diffSourceRef.current = diffSource;
  }, [diffSource]);
  const [syncing, setSyncing] = useState(false);
  const [modelsdevMeta, setModelsdevMeta] = useState<{
    fetchedAt: number;
    fromCache: boolean;
  } | null>(null);
  // 用户勾选的模型 id 集合（差异条目为模型级）
  const [selected, setSelected] = useState<Set<string>>(new Set());
  const [applying, setApplying] = useState(false);

  useEffect(() => {
    Promise.all([fetchPricing(), fetchModels()])
      .then(([config, list]) => {
        setPricing(config);
        setSavedJson(JSON.stringify(config));
        setModels(list);
      })
      .catch((e) => setError(String(e)));
    // 汇率与 Cursor 设置共用同一来源（设置页可改、每日自动更新）
    getCursorConfig()
      .then((config) => {
        if (config.usd_cny_rate > 0) setFxRate(config.usd_cny_rate);
      })
      .catch(() => {});
  }, []);

  // 语言切换：币种回到语言对应的默认口径（与总览/对比页同规则）。
  // 本页不做任何持久化——全局币种从属语言，写盘入口在面板 App.tsx
  useEffect(() => {
    setCurrency(currencyOf(locale));
  }, [locale]);

  // 把某个来源的 diff 结果写入共享状态（更新差异数 + 默认勾选全部新增项，
  // 变动项默认不勾以保护用户自定义）
  const applyDiffState = (next: PricingDiff) => {
    setDiff(next);
    setUpdateCount(next.new_models.length + next.changed.length);
    setSelected(new Set(next.new_models.map((item) => item.model_id)));
  };

  // 进入价格设置时静默检查一次差异，有差异则在按钮显示红点（纯本地对比，秒回不联网）。
  // 差异判定只看 USD 原始价（人民币按汇率折算展示，不参与判定）。
  useEffect(() => {
    checkPricingUpdates()
      .then((result) => {
        // 内置检查内部含 codex 导入冷启动、可能晚到数秒：期间用户若已切到
        // models.dev 来源，丢弃这份过期结果，避免来源标注与数据错位
        if (diffSourceRef.current !== "builtin") return;
        applyDiffState(result);
      })
      .catch(() => {});
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  // 切换某条差异的勾选状态
  const toggleSelect = (key: string) => {
    setSelected((prev) => {
      const next = new Set(prev);
      if (next.has(key)) next.delete(key);
      else next.add(key);
      return next;
    });
  };

  // 应用勾选的价格更新：把勾选项的美元价合并进 pricing 并保存，绝不自动覆盖未勾选项
  const handleApplyUpdates = async () => {
    if (!diff) return;
    setApplying(true);
    setError(null);
    try {
      const all = [...diff.new_models, ...diff.changed];
      const items: ApplyPriceItem[] = all
        .filter((item) => selected.has(item.model_id))
        .map((item) => ({
          model_id: item.model_id,
          currency: "usd" as const,
          price: item.default,
        }));
      if (items.length === 0) {
        setDiffPanel(false);
        return;
      }
      const updated = await applyPricingUpdates(items);
      setPricing(updated);
      setSavedJson(JSON.stringify(updated));
      setDraft({});
      // 轻提示：应用成功给一次「已应用 N 项」的确认（2s 自动消失）
      setAppliedCount(items.length);
      window.setTimeout(() => setAppliedCount(null), 2000);
      // 重新检查差异：与当前展示的参考源保持一致（若应用 models.dev 价后
      // 改用内置表复查，会把刚应用的官方价误报为「变动」）
      if (diffSource === "modelsdev") {
        const result = await syncModelsdevPricing();
        setModelsdevMeta({
          fetchedAt: result.fetched_at,
          fromCache: result.from_cache,
        });
        applyDiffState(result.diff);
        if (result.diff.new_models.length + result.diff.changed.length === 0) {
          setDiffPanel(false);
        }
      } else {
        const result = await checkPricingUpdates();
        applyDiffState(result);
        if (result.new_models.length + result.changed.length === 0) {
          setDiffPanel(false);
        }
      }
    } catch (e) {
      setError(String(e));
    } finally {
      setApplying(false);
    }
  };

  // 对比并展示差异（纯本地对比内置参考表，无网络请求）
  const runDiff = async () => {
    setError(null);
    try {
      const result = await checkPricingUpdates();
      setDiffSource("builtin");
      setModelsdevMeta(null);
      applyDiffState(result);
      setDiffPanel(true);
    } catch (e) {
      setError(String(e));
    }
  };

  // 从 models.dev 在线同步官方厂商价格（网络失败自动降级本地缓存）。
  // 结果与内置表来源互斥：直接覆盖当前 diff 状态
  const runModelsdevSync = async () => {
    setError(null);
    setSyncing(true);
    try {
      const result = await syncModelsdevPricing();
      setDiffSource("modelsdev");
      setModelsdevMeta({
        fetchedAt: result.fetched_at,
        fromCache: result.from_cache,
      });
      applyDiffState(result.diff);
      setDiffPanel(true);
    } catch (e) {
      setError(`${String(e)}（${t("dashboard.pricing.syncFailHint")}）`);
    } finally {
      setSyncing(false);
    }
  };

  // 手动刷新汇率（多源容错，成功写回后端配置）：仅影响 ¥ 折算展示
  const refreshFxRate = async () => {
    setFxLoading(true);
    setError(null);
    try {
      const [rate] = await fetchFxRate();
      if (rate > 0) setFxRate(rate);
    } catch (e) {
      setError(String(e));
    } finally {
      setFxLoading(false);
    }
  };

  // 草稿变更（稳定引用）：单个输入框的键入只重渲染对应的一行（memo 行组件）
  const handleDraftChange = useCallback(
    (id: string, key: keyof ModelPrice, value: string) => {
      setDraft((prev) => ({ ...prev, [`${id}|${key}`]: value }));
    },
    []
  );

  // 某输入框失焦：把草稿写回 pricing 数值（读 draftRef 最新值）。只编辑美元价
  const commitDraft = useCallback(
    (modelId: string, key: keyof ModelPrice) => {
      const raw = draftRef.current[`${modelId}|${key}`];
      if (raw === undefined) return; // 没编辑过，跳过
      const num = parseDraft(raw);
      setPricing((prev) => {
        if (!prev) return prev;
        const cur = prev.usd ?? {};
        // 表单按归一名去重只显示一行；写入时把同归一键（大小写/隐藏字符变体）
        // 的残留条目合并掉，避免残留条目被兜底查找优先命中，让编辑看起来不生效。
        // 合并域用 canonicalModelId（与表单行分组同域）
        const target = canonicalModelId(modelId);
        const next: Record<string, ModelPrice> = {};
        for (const [k, v] of Object.entries(cur)) {
          if (k !== modelId && canonicalModelId(k) === target) continue;
          next[k] = v;
        }
        next[modelId] = {
          // 与窄窗同基底：优先沿用该行现值，其次沿用兜底命中的既有三元组
          // （展示 id 与价格表键名不一致时，避免把未编辑的字段清零）
          ...(cur[modelId] ?? lookupPrice(cur, modelId) ?? ZERO_PRICE),
          [key]: num,
        };
        return { ...prev, usd: next };
      });
    },
    []
  );

  const handleSave = async () => {
    if (!pricing) return;
    // 保存前把所有未 commit 的草稿落盘（只存美元价）
    const merged = { ...pricing };
    const cur = { ...(merged.usd ?? {}) };
    for (const [draftKey, raw] of Object.entries(draft)) {
      const [modelId, key] = draftKey.split("|") as [string, keyof ModelPrice];
      cur[modelId] = {
        // 与 commitDraft 同基底：优先沿用兜底命中的既有三元组，避免丢其他字段
        ...(cur[modelId] ?? lookupPrice(cur, modelId) ?? ZERO_PRICE),
        [key]: parseDraft(raw),
      };
    }
    // 与 commitDraft 同域（canonicalModelId）：只合并本次被编辑展示键的
    // 大小写/隐藏字符残留写法；两者都未编辑的条目保持原样，不做静默折叠
    const edited = new Set(Object.keys(draft).map((dk) => dk.split("|")[0]));
    const consolidated: Record<string, ModelPrice> = {};
    for (const [k, v] of Object.entries(cur)) {
      const target = canonicalModelId(k);
      const kept = Object.keys(consolidated).find(
        (c) => canonicalModelId(c) === target
      );
      if (kept === undefined) {
        consolidated[k] = v;
      } else if (edited.has(k) && !edited.has(kept)) {
        consolidated[k] = v;
        delete consolidated[kept];
      }
    }
    merged.usd = consolidated;

    setSaving(true);
    setError(null);
    try {
      await savePricing(merged);
      setPricing(merged);
      setSavedJson(JSON.stringify(merged));
      setDraft({});
      setSavedFlash(true);
      setTimeout(() => setSavedFlash(false), 1500);
    } catch (e) {
      setError(String(e));
    } finally {
      setSaving(false);
    }
  };

  // 从列表移除模型（明确动作，即时落盘，不走草稿+保存的按钮模式）：
  // 1) 删 usd 价格条目（按归一键删该模型全部大小写/变体写法）；
  // 2) 记入隐藏名单（阻止它因数据库出现过再次出现在列表与差异提醒中）；
  // 3) 清该模型全部草稿（防止后续「保存」把已删模型经残留草稿写回）。
  // 落盘失败报错并回滚本地状态
  const handleRemove = useCallback(async (id: string) => {
    const prev = pricingRef.current;
    if (!prev) return;
    const target = canonicalModelId(id);
    const usd: Record<string, ModelPrice> = {};
    for (const [k, v] of Object.entries(prev.usd ?? {})) {
      if (canonicalModelId(k) === target) continue;
      usd[k] = v;
    }
    const next: PricingConfig = {
      ...prev,
      usd,
      hidden: [...new Set([...(prev.hidden ?? []), id])],
    };
    const prevDraft = draftRef.current;
    const nextDraft: Record<string, string> = {};
    for (const [draftKey, v] of Object.entries(prevDraft)) {
      // 草稿键前缀按归一匹配：同模型任何写法的残留草稿都不会再被 handleSave 写回
      if (canonicalModelId(draftKey.split("|")[0]) === target) continue;
      nextDraft[draftKey] = v;
    }
    setDraft(nextDraft);
    setPricing(next);
    setError(null);
    try {
      await savePricing(next);
      // 即时落盘动作也是「已保存」状态：快照一并推进，避免误报未保存
      setSavedJson(JSON.stringify(next));
    } catch (e) {
      setPricing(prev);
      setDraft(prevDraft);
      setError(String(e));
    }
  }, []);

  // 恢复隐藏模型（即时落盘）：从隐藏名单移除后重新出现在列表——价格已删的
  // 以 0 占位，走现有编辑逻辑。按归一移除，同模型多种写法的隐藏条目一并清除。
  // 落盘失败报错并回滚本地状态
  const handleRestore = useCallback(async (id: string) => {
    const prev = pricingRef.current;
    if (!prev) return;
    const target = canonicalModelId(id);
    const next: PricingConfig = {
      ...prev,
      hidden: (prev.hidden ?? []).filter((h) => canonicalModelId(h) !== target),
    };
    setPricing(next);
    setError(null);
    try {
      await savePricing(next);
      setSavedJson(JSON.stringify(next));
    } catch (e) {
      setPricing(prev);
      setError(String(e));
    }
  }, []);

  // 币种切换（本页查看口径）：只影响本页 ¥/$ 展示——既不写本地偏好键，也不调
  // saveCurrency；币种从属语言，持久化入口在面板 App.tsx 的 handleCurrencyChange
  const handleCurrencyChange = (next: Currency) => {
    setCurrency(next);
  };

  // 合并：数据库里的模型 + 价格表里手动加的模型（必须位于下方 if (!pricing)
  // 早退之前：条件早退会跳过 hooks，导致前后渲染 hooks 数不一致而崩溃）
  const rows = useMemo<PriceRow[]>(
    () => (pricing ? buildPriceRows(pricing, models) : []),
    [pricing, models]
  );
  // 搜索过滤：模型 id 与展示名都可命中（大小写不敏感）
  const visibleRows = useMemo(() => {
    const keyword = query.trim().toLowerCase();
    if (!keyword) return rows;
    return rows.filter(
      (row) =>
        row.id.toLowerCase().includes(keyword) ||
        row.displayName.toLowerCase().includes(keyword)
    );
  }, [rows, query]);

  // dirty 派生（必须位于下方 if (!pricing) 早退之前）：草稿非空，或内存配置与
  // 已落盘快照不一致（行内编辑/新增模型都改内存等「保存」统一落盘）。
  // pricingJson 只在 pricing 变化时重算，不受草稿键入影响
  const pricingJson = useMemo(
    () => (pricing ? JSON.stringify(pricing) : null),
    [pricing]
  );
  const draftCount = Object.keys(draft).length;
  const pricingChanged =
    savedJson != null && pricingJson != null && pricingJson !== savedJson;
  const dirty = draftCount > 0 || pricingChanged;
  // 未保存处数：以草稿条数为准；仅配置差异（如新增模型）时至少报 1 处
  const dirtyCount = draftCount > 0 ? draftCount : pricingChanged ? 1 : 0;

  // 新增模型定价：写入内存配置（与行内编辑一样等「保存」统一落盘）；
  // 若该 id 在隐藏名单里（曾被删除）则一并恢复，避免「加了却看不到」
  const handleAddModel = () => {
    if (!pricing) return;
    const id = newId.trim();
    if (!id) {
      setError(t("dashboard.pricing.addEmptyId"));
      return;
    }
    const target = canonicalModelId(id);
    if (rows.some((row) => canonicalModelId(row.id) === target)) {
      setError(t("dashboard.pricing.addExists"));
      return;
    }
    setError(null);
    // 与 commitDraft 同域：同归一名的残留写法先合并掉，保证新条目能被精确命中
    const usd: Record<string, ModelPrice> = {};
    for (const [k, v] of Object.entries(pricing.usd)) {
      if (k !== id && canonicalModelId(k) === target) continue;
      usd[k] = v;
    }
    usd[id] = {
      input: parseDraft(newPrice.input),
      output: parseDraft(newPrice.output),
      cache_read: parseDraft(newPrice.cache_read),
    };
    setPricing({
      ...pricing,
      usd,
      hidden: (pricing.hidden ?? []).filter(
        (h) => canonicalModelId(h) !== target
      ),
    });
    setDraft((prev) => {
      const next: Record<string, string> = {};
      for (const [draftKey, v] of Object.entries(prev)) {
        if (canonicalModelId(draftKey.split("|")[0]) === target) continue;
        next[draftKey] = v;
      }
      return next;
    });
    setNewId("");
    setNewPrice({ input: "", output: "", cache_read: "" });
  };

  if (!pricing) {
    return (
      <div className="db-page">
        <header>
          <h1 className="db-page__title">{t("dashboard.pricing")}</h1>
          <p className="db-page__desc">{t("dashboard.pricing.desc")}</p>
        </header>
        <PricingSkeleton />
      </div>
    );
  }

  const hiddenList = pricing.hidden ?? [];
  const upToDateVisible = diff != null && updateCount === 0 && !diffPanel;

  return (
    <div className="db-page">
      {/* 吸顶页头：长价格表滚动时「保存」始终可见（见 dashboard.css 的
          .db-page__header--sticky） */}
      <header className="db-page__header db-page__header--sticky">
        <div>
          <h1 className="db-page__title">{t("dashboard.pricing")}</h1>
          <p className="db-page__desc">{t("dashboard.pricing.desc")}</p>
        </div>
        <div className="db-page__actions">
          <div
            className="db-seg"
            role="group"
            aria-label={t("dashboard.pricing.currencyLabel")}
          >
            {(["cny", "usd"] as Currency[]).map((code) => (
              <button
                key={code}
                type="button"
                onClick={() => handleCurrencyChange(code)}
                className={`db-seg__btn ${
                  currency === code ? "db-seg__btn--active" : ""
                }`}
                aria-pressed={currency === code}
              >
                {code === "cny"
                  ? t("dashboard.pricing.cny")
                  : t("dashboard.pricing.usd")}
              </button>
            ))}
          </div>
          {/* 临时口径标注：本页的 ¥/$ 切换不回写全局币种（全局币种从属语言） */}
          <span
            className="db-pricing__viewonly"
            title={t("dashboard.pricing.currencyViewOnlyHint")}
          >
            {t("dashboard.pricing.currencyViewOnly")}
          </span>
          <button
            type="button"
            className="db-btn db-btn--badge"
            onClick={runDiff}
          >
            {t("dashboard.pricing.checkUpdates")}
            {/* 红点只对内置表来源展示：models.dev 的差异数在差异卡内呈现 */}
            {diffSource === "builtin" && updateCount > 0 && (
              <span className="db-dot">{updateCount}</span>
            )}
          </button>
          <button
            type="button"
            className="db-btn"
            onClick={runModelsdevSync}
            disabled={syncing}
          >
            {syncing
              ? t("dashboard.pricing.syncing")
              : t("dashboard.pricing.syncModelsdev")}
          </button>
          {dirty && (
            <span className="db-pricing__dirty">
              {t("dashboard.pricing.dirtyHint", { count: dirtyCount })}
            </span>
          )}
          <button
            type="button"
            className={`db-btn db-btn--primary ${dirty ? "db-btn--dirty" : ""}`}
            onClick={handleSave}
            disabled={saving}
          >
            {saving
              ? t("common.saving")
              : savedFlash
                ? t("common.saved")
                : dirty
                  ? t("dashboard.pricing.saveDirty")
                  : t("common.save")}
          </button>
        </div>
      </header>

      {/* 口径行：单位与汇率（可手动刷新）+ 差异检查结论 + 当前模型数 */}
      <div className="db-pricing__meta">
        <span>{t("dashboard.pricing.unitHint", { rate: fxRate })}</span>
        <button
          type="button"
          className="db-linkbtn"
          onClick={refreshFxRate}
          disabled={fxLoading}
        >
          {fxLoading
            ? t("dashboard.pricing.fxRefreshing")
            : t("dashboard.pricing.fxRefresh")}
        </button>
        {upToDateVisible && diff && (
          <span
            className={`db-pricing__status ${
              diff.missing.length > 0
                ? "db-pricing__status--warn"
                : "db-pricing__status--ok"
            }`}
          >
            {diff.missing.length > 0
              ? t("dashboard.pricing.missingCount", {
                  count: diff.missing.length,
                })
              : t("dashboard.pricing.upToDate")}
          </span>
        )}
        {/* 应用更新的轻提示：与「已是最新」状态位并列，2s 后自动消失 */}
        {appliedCount != null && (
          <span className="db-pricing__flash">
            {t("dashboard.pricing.appliedCount", { count: appliedCount })}
          </span>
        )}
        <span className="db-pricing__count num">
          {t("dashboard.pricing.count", { n: rows.length })}
        </span>
      </div>

      {error && <AlertBanner type="error">{error}</AlertBanner>}

      {diffPanel && diff && (
        <DiffPanel
          diff={diff}
          source={diffSource}
          meta={modelsdevMeta}
          selected={selected}
          fxRate={fxRate}
          applying={applying}
          onToggle={toggleSelect}
          onApply={handleApplyUpdates}
          onCollapse={() => setDiffPanel(false)}
        />
      )}

      <Card
        title={t("dashboard.pricing.tableTitle")}
        sub={t("dashboard.pricing.tableSub")}
        action={
          <input
            type="search"
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            placeholder={t("dashboard.pricing.searchPlaceholder")}
            aria-label={t("dashboard.pricing.searchPlaceholder")}
            className="db-search"
          />
        }
      >
        <PriceTable
          rows={visibleRows}
          total={rows.length}
          currency={currency}
          fxRate={fxRate}
          draft={draft}
          onDraftChange={handleDraftChange}
          onCommit={commitDraft}
          onRemove={setRemoveTarget}
        />
      </Card>

      {/* 新增模型定价：写入内存配置，点右上「保存」统一落盘（与行内编辑同一条链路） */}
      <Card title={t("dashboard.pricing.addTitle")}>
        <div className="db-addmodel">
          <label className="db-addmodel__id">
            <span className="db-addmodel__label">
              {t("dashboard.pricing.addIdLabel")}
            </span>
            <input
              type="text"
              value={newId}
              onChange={(e) => setNewId(e.target.value)}
              placeholder={t("dashboard.pricing.addPlaceholder")}
              className="db-addmodel__input num"
            />
          </label>
          {PRICE_FIELDS.map((field) => (
            <label key={field.key} className="db-addmodel__price">
              <span className="db-addmodel__label">{t(field.labelKey)}</span>
              <span className="db-addmodel__box">
                <span className="db-addmodel__symbol">$</span>
                <input
                  type="text"
                  inputMode="decimal"
                  value={newPrice[field.key]}
                  placeholder="0"
                  onChange={(e) => {
                    const value = e.target.value.replace(/[^\d.]/g, "");
                    setNewPrice((prev) => ({ ...prev, [field.key]: value }));
                  }}
                  className="num db-addmodel__num"
                />
              </span>
            </label>
          ))}
          <button
            type="button"
            className="db-btn db-btn--primary db-addmodel__submit"
            onClick={handleAddModel}
          >
            {t("dashboard.pricing.add")}
          </button>
        </div>
      </Card>

      {/* 已隐藏模型（删除过的）：折叠入口 + 恢复列表（N=0 时不渲染） */}
      {hiddenList.length > 0 && (
        <Card title={t("dashboard.pricing.hiddenTitle")}>
          <button
            type="button"
            className="db-linkbtn db-linkbtn--block"
            onClick={() => setHiddenOpen((open) => !open)}
            aria-expanded={hiddenOpen}
          >
            {hiddenOpen ? "▾" : "▸"}{" "}
            {t("dashboard.pricing.hiddenCount", { count: hiddenList.length })}
          </button>
          {hiddenOpen && (
            <ul className="db-hiddenlist">
              {hiddenList.map((hidden) => (
                <li key={hidden} className="db-hiddenlist__item">
                  <span className="num db-hiddenlist__name">{hidden}</span>
                  <button
                    type="button"
                    className="db-linkbtn"
                    onClick={() => handleRestore(hidden)}
                  >
                    {t("dashboard.pricing.restore")}
                  </button>
                </li>
              ))}
            </ul>
          )}
        </Card>
      )}

      {/* 口径脚注：落盘行为与费用估算的含义（与窄窗提示口径一致） */}
      <p className="db-page__footnote">{t("dashboard.pricing.footnote")}</p>

      {/* 删除确认浮层（窄窗面板 ConfirmDialog 的大窗同构形态，红键）：
          行内 × 只进确认，确认后才执行 handleRemove 即时落盘 */}
      {removeTarget != null && (
        <div className="db-confirm-mask">
          <div
            className="db-confirm"
            role="alertdialog"
            aria-modal="true"
            aria-labelledby="db-confirm-title"
          >
            <div id="db-confirm-title" className="db-confirm__title">
              {t("common.delete")} · {removeTarget}
            </div>
            <p className="db-confirm__desc">
              {t("dashboard.pricing.removeConfirmDesc")}
            </p>
            <div className="db-confirm__actions">
              <button
                type="button"
                className="db-btn"
                onClick={() => setRemoveTarget(null)}
              >
                {t("common.cancel")}
              </button>
              <button
                type="button"
                className="db-btn db-btn--danger"
                onClick={() => {
                  const id = removeTarget;
                  setRemoveTarget(null);
                  void handleRemove(id);
                }}
              >
                {t("common.delete")}
              </button>
            </div>
          </div>
        </div>
      )}
    </div>
  );
}

/** 首屏骨架：表格 + 新增表单，形状与真实内容一致。
 *  行高按真实价格行给（输入框 26px + ¥ 折算副行，见 .db-price-skeleton__row）：
 *  ¥ 口径约 61px、$ 口径约 46px（没有折算副行），行块紧贴排布，
 *  避免加载完成后整表高度跳动。 */
function PricingSkeleton() {
  const { locale } = useI18n();
  const rowClass =
    currencyOf(locale) === "cny"
      ? "db-price-skeleton__row"
      : "db-price-skeleton__row db-price-skeleton__row--usd";
  return (
    <>
      <div className="db-card">
        <div className="db-card__head">
          <Skeleton className="h-3.5 w-24" />
        </div>
        <div className="db-card__body">
          {[0, 1, 2, 3, 4, 5].map((i) => (
            <Skeleton key={i} className={rowClass} />
          ))}
        </div>
      </div>
      <div className="db-card">
        <div className="db-card__head">
          <Skeleton className="h-3.5 w-20" />
        </div>
        <div className="db-card__body">
          <Skeleton className="h-8 w-full" />
        </div>
      </div>
    </>
  );
}
