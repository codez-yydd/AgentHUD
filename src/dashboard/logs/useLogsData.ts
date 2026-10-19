/**
 * 请求日志页取数 hook：分页明细 + 全范围汇总 + 模型筛选项。
 *
 * 设计要点（对齐 useOverviewData 的既有做法，但刻意更轻）：
 *  - 筛选 / 页码变化即重新请求；**不做模块级缓存**——翻页场景缓存命中率低，
 *    又会让「刷新按钮」与「筛选切换」的语义变复杂；
 *  - summary（指标卡）随明细同一次请求返回：同一 WHERE 下的全范围聚合，
 *    因此永远与当前筛选条件一致（翻页时数值不变，只是重新下发）；
 *  - 请求序号守卫（reqIdRef）：并发触发时只有最新一次请求的结果落 state，
 *    旧请求（含刚被筛掉的来源）的结果一律丢弃；
 *  - reload 支持**静默刷新**（reload({ silent: true })，页面自动刷新用）：
 *    不置 loading——刷新按钮不转圈、表格与指标不压暗、不闪骨架；失败也不
 *    改写当前视图（保留上次成功的数据与「上次刷新」时刻），避免后台的偶发
 *    失败把用户正在看的表格清空成错误态（手动刷新仍是完整语义）；
 *  - 未安装 / 未用过来源由后端按 0 条返回，这里静默（total 0 → 页面空态），
 *    只有真正的错误才进 error；
 *  - 模型清单与明细分成两个请求：清单只依赖「来源 + 时间范围」，翻页不重取；
 *    清单连同它所属的 key 一起存，页面据此只消费「当前来源 + 范围」的清单
 *    （切条件后旧清单不会被拿来当作新条件的选项；选中值本身不自动清空，
 *    由页面的空态提示 + 「清除模型筛选」按钮交给用户决定）。
 *  - 时间范围支持自定义区间（custom 非空即生效，优先于预设档 range）：起止
 *    已在页面侧折算为本地自然日半开区间 [fromMs, toMs)，这里直接下发，
 *    不再按 rangeWindow 的「此刻」口径推算；
 *  - 数据范围（scope = local | all）参与请求键与 effect 依赖：切到「全部
 *    设备」即重新取数（后端并入远端其他设备明细），远端不可达 / 未启用同步 /
 *    旧服务端 404 由后端降级为纯本机口径（failedSources 里给 "remote"），
 *    这里照常渲染数据，不清空表格；
 *  - 首发请求挂起在同步配置就绪（ready）之后：scope 偏好缺省「全部设备」，
 *    配置未读到时无法确定实际口径，先按本机查再全量重取会双取（总览页
 *    useOverviewData 的反 pattern 注记），等待期骨架本就在展示。
 */
import { useCallback, useEffect, useRef, useState } from "react";
import { getRequestLogModels, getRequestLogs } from "../../api";
import type {
  RequestLogModelCount,
  RequestLogRow,
  RequestLogScope,
  RequestLogSource,
  RequestLogSummary,
} from "../../types";
import type { CustomRangeValue } from "../CustomRangeMenu";
import { LOG_PAGE_SIZE, rangeWindow, type LogsRange } from "./model";

export interface LogsQuery {
  range: LogsRange;
  /** 自定义区间（非空 = 自定义模式，优先于 range；[fromMs, toMs) 本地自然日
   *  半开区间，toMs = 结束日次日 0 点）；预设档选中时页面应清为 null */
  custom: CustomRangeValue | null;
  /** 后端来源名；空串 = 全部来源 */
  source: string;
  /** 模型 id 精确筛选；空串 = 不筛 */
  model: string;
  /** 数据范围：local = 仅本机；all = 本机 + 其他设备（后端合并远端明细，
   *  远端不可达自动降级为本机口径并在 failedSources 里给 "remote"） */
  scope: RequestLogScope;
  /** 同步配置是否就绪：未就绪时挂起首发请求（明细 + 清单都不发）。scope
   *  偏好可能是「全部设备」且缺省即为 all——配置到位前无法确定实际口径，
   *  先按本机查一遍、配置到达后再按全部设备全量重取会双取 7 库并闪一次
   *  数据（总览页 useOverviewData 的既有反 pattern 注记）；等待期骨架本就
   *  在展示，getSyncConfig 是本地文件读取，挂起成本可忽略 */
  ready: boolean;
  /** 页码（1 基，页面语义；转成后端的 0 基） */
  page: number;
}

export interface LogsDataState {
  rows: RequestLogRow[];
  /** 上面这批 rows 生效的页码（1 基）：页码文案与分页按钮以此为准，
   *  避免「第 N 页」领先于尚未返回的数据（翻页等待期仍显示旧页） */
  rowsPage: number;
  /** 当前筛选条件下的总条数（后端各源 COUNT 之和） */
  total: number;
  /** 当前筛选条件下的全范围汇总（指标卡）；尚未取到 / 出错时为 null */
  summary: RequestLogSummary | null;
  /** 本次查询打开 / 查询失败被跳过的来源名（库不存在不算）；非空且 total > 0
   *  时页面弱提示「数据可能不完整」 */
  failedSources: string[];
  loading: boolean;
  error: string | null;
  lastUpdatedAt: number | null;
  /** 当前「来源 + 范围」的模型清单；尚未取到 / 出错时为 null */
  models: RequestLogModelCount[] | null;
  /** 重取明细 + 清单（不做模块级缓存）。silent = true 走静默刷新：不置
   *  loading、失败不改写当前视图，供页面的自动刷新定时器使用 */
  reload: (options?: { silent?: boolean }) => void;
}

/** 错误信息截断（与 useOverviewData.shortError 同款：后端错误可能是长中文串） */
function shortError(error: unknown): string {
  const text = String(error).replace(/^Error:\s*/, "");
  return text.length > 120 ? text.slice(0, 117) + "…" : text;
}

export function useLogsData({
  range,
  custom,
  source,
  model,
  scope,
  ready,
  page,
}: LogsQuery): LogsDataState {
  const [rows, setRows] = useState<RequestLogRow[]>([]);
  const [rowsPage, setRowsPage] = useState(1);
  const [total, setTotal] = useState(0);
  const [summary, setSummary] = useState<RequestLogSummary | null>(null);
  const [failedSources, setFailedSources] = useState<string[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [lastUpdatedAt, setLastUpdatedAt] = useState<number | null>(null);
  const [models, setModels] = useState<{
    key: string;
    list: RequestLogModelCount[];
  } | null>(null);
  const [nonce, setNonce] = useState(0);
  // 明细 / 清单各自的请求序号（互不干扰：清单慢于明细时不该丢弃清单结果）
  const rowsReqRef = useRef(0);
  const modelsReqRef = useRef(0);
  // 下一次请求是否静默（reload({silent:true}) 置位）：由明细 effect 开头消费并
  // 复位；清单 effect 没有 loading 语义，不受该标记影响
  const silentRef = useRef(false);

  // 明细：筛选、页码与手动刷新触发；窗口在 effect 内取「此刻」计算，
  // 不进依赖（Date.now() 每次渲染都变，进依赖会把 effect 变成每次渲染都跑）；
  // 自定义区间直接使用（起止已是本地自然日边界，不再按「此刻」推算）。
  // ready = false（同步配置未读到）时挂起请求：silent 标记照常消费（挂起期
  // 自动刷新定时器置位的静默标记直接丢弃，配置到位后的首射永远是非静默的
  // 完整语义），loading 保持初值 true，骨架继续展示
  useEffect(() => {
    const silent = silentRef.current;
    silentRef.current = false;
    if (!ready) return;
    const reqId = ++rowsReqRef.current;
    const isCurrent = () => reqId === rowsReqRef.current;
    const { fromMs, toMs } = custom ?? rangeWindow(range, Date.now());
    // 静默刷新保持现有画面：不置 loading（按钮不转圈、表格不压暗）
    if (!silent) setLoading(true);
    getRequestLogs(fromMs, toMs, page - 1, LOG_PAGE_SIZE, {
      agent: source ? (source as RequestLogSource) : undefined,
      model: model || undefined,
      scope,
    })
      .then((result) => {
        if (!isCurrent()) return;
        setRows(result.rows);
        setRowsPage(page);
        setTotal(result.total);
        setSummary(result.summary ?? null);
        setFailedSources(result.failed_sources ?? []);
        setError(null);
        setLastUpdatedAt(Date.now());
        setLoading(false);
      })
      .catch((e) => {
        if (!isCurrent()) return;
        // 本次已是最新请求：先把 loading 收掉（静默刷新未置位，这里幂等；
        // 若上一笔被本笔取代且仍挂着 loading，也不会把转圈态留在界面上）
        setLoading(false);
        // 静默刷新失败：保留当前视图（上次成功的数据），由下一次定时或用户
        // 手动刷新再试，不因后台偶发失败清空表格 / 闪错误态
        if (silent) return;
        // 失败时清空列表与汇总：筛选刚变过，留着上一条件的数据会误导
        setRows([]);
        setRowsPage(page);
        setTotal(0);
        setSummary(null);
        setFailedSources([]);
        setError(shortError(e));
      });
  }, [custom, range, source, model, scope, ready, page, nonce]);

  // 模型清单：只随来源 / 范围 / 数据范围（与手动刷新）重取；成功才发布（失败
  // 保持 null，页面不做失效校正，下拉退化为「全部模型」而不是给出错误清单）。
  // 键含自定义区间的起止毫秒：不同自定义范围不共用同一份清单；ready 门控同明细
  const modelsKey = `${range}|${source}|${scope}|${
    custom ? `${custom.fromMs}-${custom.toMs}` : ""
  }`;
  useEffect(() => {
    if (!ready) return;
    const reqId = ++modelsReqRef.current;
    const isCurrent = () => reqId === modelsReqRef.current;
    const { fromMs, toMs } = custom ?? rangeWindow(range, Date.now());
    getRequestLogModels(fromMs, toMs, {
      agent: source ? (source as RequestLogSource) : undefined,
      scope,
    })
      .then((list) => {
        if (isCurrent()) setModels({ key: modelsKey, list });
      })
      .catch(() => {
        /* 静默：清单失败不影响明细展示 */
      });
  }, [range, source, scope, ready, modelsKey, nonce]);

  const reload = useCallback((options?: { silent?: boolean }) => {
    silentRef.current = options?.silent === true;
    setNonce((n) => n + 1);
  }, []);

  return {
    rows,
    rowsPage,
    total,
    summary,
    failedSources,
    loading,
    error,
    lastUpdatedAt,
    // 只把「属于当前来源 + 范围」的清单交给页面
    models: models && models.key === modelsKey ? models.list : null,
    reload,
  };
}
