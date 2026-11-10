# 调研报告：请求日志页 ZCode 与 DeepSeek Harness 的 Token 速度计算对比

> 调研日期：2026-11-09。调研范围：主窗口「请求日志」页（`src/dashboard/logs/`）展示的
> 单请求速度 `speed_tps` / `speed_quality` 的后端计算链路，对比 `zcode` 与 `dsh`
> （DeepSeek Harness）两个来源。方法：只读代码走查（Rust 后端为主），全部结论附文件与行号。
> 口径层面的既有总结见 `doc/token-speed-accounting.md`，本报告是代码级补充，两者互不替代。

## 1. 一句话结论

**两个来源的最终速度语义一致**——都是「扣掉 TTFT 的纯流式生成窗口」速度
（`SpeedQuality::Generation` 精确池），共享同一套样本有效性边界（输出 > 0、
窗口 ∈ [200ms, 1h]、越界整体拒收）。**但分母的产生路径、降级行为、跨设备传输方式
三个维度实质不同**，详见第 5 节对比表。

## 2. 术语与核心模块

| 名称 | 位置 | 职责 |
| --- | --- | --- |
| `SpeedQuality` | `src-tauri/src/token_speed.rs:23-28` | 两档质量标记：`Generation`（精确流式窗口）/ `RequestAverage`（整请求近似） |
| `SpeedEvidence` | `src-tauri/src/token_speed.rs:30-43` | 跨设备传输的「来源设备已验证计时」信封（version=1 + span_ms + quality）；**信封存在但 span 为空 = 明确不可用，消费端永不复活 fallback** |
| `SpeedTotals` | `src-tauri/src/token_speed.rs:266-304` | 聚合池：两档样本不混算，存在 Generation 样本时 RequestAverage 样本整体不进池 |
| `RequestTiming` | `src-tauri/src/token_speed.rs:100-111` | 一条完成请求的计时原料（started/first_token/completed/duration 四要素 + 元数据） |
| `request_speed_at` | `src-tauri/src/token_speed.rs:122-219` | ZCode 路径的速度判定函数（查询时现算） |
| `duration_speed` | `src-tauri/src/token_speed.rs:224-235` | 非 ZCode 导入源的速度函数（除法 + 边界） |
| `duration_quality` | `src-tauri/src/token_speed.rs:238-240` | 按来源决定 duration 的语义档位：**`dsh` → Generation，其余 → RequestAverage** |
| `row_to_log` | `src-tauri/src/request_logs.rs:401-450` | 请求日志页本地行的构造器，**两来源分叉点在这里** |
| `remote_row_to_log` | `src-tauri/src/request_logs.rs:1025-1055` | 「全部设备」口径的远端行构造器 |
| `zcode_first_token_expr` | `src-tauri/src/db.rs:171-184` | ZCode 首 token 时刻的 SQL 取值表达式（含事件流覆盖解析） |
| `zcode_speed_evidence.rs` | `src-tauri/src/zcode_speed_evidence.rs` | 挂载 `agenthud_zcode_first_content` SQL 函数；从 ZCode 主库事件日志推导首个内容里程碑，维护 AgentHUD 自己的派生缓存 |
| `stream_bounds` / `derive_timing` | `src-tauri/src/dsh.rs:194-239` | DSH 导入时把事件日志 stream 数组物化成 `duration_ms`/`ttft_ms` 存库 |

样本窗口边界常量（两路径共用，`token_speed.rs:77,83`）：

- `MIN_GEN_MS = 200`：< 200ms 视为计时抖动/缓存命中伪请求，**整体拒收**（不产出速度、不造 1 秒地板、不截断分母）；
- `MAX_GEN_MS = 3_600_000`（1 小时）：视为挂死/计时泄漏，整体拒收；边界含端点（恰 200ms 与恰 1h 有效）。

完成状态过滤（`token_speed.rs:88-92`）：`status` 为空/NULL（旧 schema 兼容）、
`completed`、`success` 才算完成态；显式非终态永不产速度。请求日志页在
`row_to_log`（`request_logs.rs:408`）先做这道过滤再进速度分支。

## 3. 本地路径（请求日志页默认「本机」数据）

### 3.1 分叉点

`request_logs.rs:406-428`（`row_to_log`）：

```rust
let speed = if !crate::token_speed::is_completed_status(status.as_deref()) { None }
    else if source != "zcode" {
        duration_ms.and_then(|duration| crate::token_speed::duration_speed(
            output_tokens, duration, crate::token_speed::duration_quality(source), started_at))
    } else { crate::token_speed::request_speed_at(
        &crate::token_speed::RequestTiming {
            output_tokens,
            started_at: Some(started_at),
            first_token_at: row.get(13)?,   // ← zcode_first_token_expr 的产物
            completed_at: row.get(14)?,
            // 真实耗时只作 request_average 近似窗口的兜底，绝不参与生成口径分母
            duration_ms: duration_ms.filter(|v| *v > 0.0).map(|v| *v as i64),
            ...
        },
        chrono::Utc::now().timestamp_millis(),  // observed_at：拒绝未来时刻
    ) };
```

SQL 侧（`request_logs.rs:369-398` `select_list`）按 `has_column` 动态探测各来源库结构：
`duration_ms` 只听**来源库真实列**且 > 0（`duration_expr`，356-365 行），没有该列（如 codex）
或值为 NULL/0 就按无耗时下发，**不做任何估算回退**；`first_token` 列即
`zcode_first_token_expr(conn)` 的返回（383 行）。

### 3.2 ZCode 路径：查询时实时推导分母

**第一步：解析 first_token_at**（`db.rs:171-184` `zcode_first_token_expr`）：

- 库里只有裸 `first_token_at` 列时：若同时存在工具调用痕迹
  （`tool_call_count > 0` 或 `finish_reason IN ('tool-calls','tool_calls','function_call')`）
  则置 NULL（工具轮的数据库首字不可信），否则用裸列；
- 库里 trace_id/span_id/session_id/started_at/completed_at 齐全且
  `zcode_speed_evidence::install` 成功时：升级为
  `agenthud_zcode_first_content(trace_id, span_id, session_id, started_at, completed_at, first, (tools OR finish))`
  ——该 SQL 函数从 ZCode 主库的 `ttft.model.first_content` 事件日志（覆盖非空正文、
  思考与**工具参数**的首内容里程碑）按 `(trace_id, span_id)` 精确定位
  `model.request.completed`，再以 `(traceId, sessionId, requestId, attempt)` 定位首内容
  事件；重复冲突、重试串线、时序错误均不采用。核验结果缓存在 AgentHUD 自己的
  派生缓存里，日志轮转不丢证据（背景详见 `doc/token-speed-accounting.md` 2026-11-01 节）。

**第二步：`request_speed_at` 判定**（`token_speed.rs:122-219`），优先级从上到下：

1. 基础校验：`output_tokens > 0`；三个时间戳出现则必须 > 0；first/completed 不得晚于
   观测时刻（`observed_at_ms` = now）；
2. 时序校验：`first < started` 或 `completed <= started` → None；
3. **first 与 completed 都有 → Generation**：`span = completed − first`（显式扣除 TTFT），
   `value = output × 1000 / span`，窗口过 `finite_rate` 的 [200ms, 1h] 边界；
4. 有 first 无 completed → **None，不降级**（半截证据不硬造平均）；
5. 无 first → **RequestAverage 降级**：duration 取 `completed − started`（两绝对端点优先），
   否则用显式 `duration_ms` 兜底；两时间戳都没有 → None。

所以 ZCode 行在请求日志页可能呈现三种结果：精确速度（Generation）、近似速度
（RequestAverage，前端带 ≈）、无速度（—）。

### 3.3 DSH 路径：分母在导入时已物化，查询时只做除法

**导入时**（`dsh.rs`，数据流 `~/.dsh/sessions/**/*.jsonl.zstd → ~/.agenthud/dsh.sqlite`）：

- `stream_bounds`（194-210 行）：从事件日志的 stream 数组取**首个/末个带 `time` 的元素时刻**。
  首个带 time 的元素即 `block-start`（开始吐字时刻），字段注释见 187-190 行；
- `derive_timing`（212-239 行）：
  - `duration_ms = 末 chunk − 首 chunk`，`≤ 0` 或时刻缺失 → None（绝不用 0 冒充）；
  - `ttft_ms = 首 chunk − step 起始时刻`，且要求首 chunk 落在
    `[step 起始, step 结束]` 内，计时异常 → None；
- 结果写入 dsh.sqlite `model_usage.duration_ms`（REAL 列，`dsh.rs:144-145`）。
  **该列的语义就是「stream 首末 chunk 之差」= 纯生成窗口，天然不含 TTFT。**

**查询时**（`row_to_log` 的非 zcode 分支）：直接
`duration_speed(output, duration_ms, duration_quality("dsh"), started_at)`：

- `duration_quality("dsh")` 返回 `Generation`（`token_speed.rs:238-240`，注释原文
  "DSH records a stream span; other importers record whole-request durations"）；
- `duration_speed`（224-235 行）只做：`output > 0`、duration 有限且 ∈ [200ms, 1h]、
  `value = output × 1000 / duration`。**不减 TTFT**（db.rs:188-190 注释：对已存
  stream span 的来源再减 TTFT 会毁掉它）。

注意：这条分支的 `duration_quality` 是**按列语义给档位**，不是算出来的——dsh.sqlite
的 duration 列之所以可信为 Generation，完全因为导入器（`derive_timing`）的产出语义。
DSH 行因此只有两种呈现：精确速度（Generation）或无速度（duration 缺失即 None，
**没有 RequestAverage 层**——DSH 数据里不存在「整请求时长」这个量）。

DSH 的会话 HUD / 聚合侧（`dsh.rs:1133-1135、1245`）用同一列做 `gen_out_tokens / gen_ms`
分子分母同范围累加，口径与请求日志页一致。

## 4. 远端路径（「全部设备」scope）

`remote_row_to_log`（`request_logs.rs:1025-1055`）对两来源**入口统一**：

```rust
let speed = record.speed_evidence.as_ref()
    .map(|e| e.speed(record.output_tokens, record.started_at))
    .unwrap_or_else(|| record.duration_ms.and_then(|duration| crate::token_speed::duration_speed(
        record.output_tokens, duration, crate::token_speed::duration_quality(&record.source), record.started_at)));
```

- 优先消费 `speed_evidence`（同步协议 9 引入，`sync.rs:868-870` 的
  `speed_evidence_version` 迁移）：来源设备用**与本机相同的验证器**生成，远端不自行猜窗口；
- 无 evidence（旧服务端/旧记录）→ 回退 `duration_speed + duration_quality(source)`。

两来源在此恰好互补：

- **DSH 不需要 evidence**：duration 列语义自明（stream span），任何路径用同一列重算
  必然同值，回退路径即精确路径；
- **ZCode 必须走 evidence**：其精确速度依赖查询时对主库事件流的覆盖解析，而
  `first_token_at / completed_at` 原料**远端不上传**（`request_logs.rs:1019-1021` 注释）。
  上传侧由 `annotate_zcode_speed`（`db.rs:705-726`）对增量行用同款
  `RequestTiming + speed_sample` 生成 v1 信封（span + quality）随行带走；
  信封存在但 span 为空时表示「已验证不可用」，消费端不得回退到 duration 重算
  （`token_speed.rs:30-31` 注释）。

更旧的远端记录（连 duration 都没有）由前端 `speedOf` 第二层公式兜底
（生成段 = duration − ttft，带 ≈ 标记），见 `request_logs.rs:1020-1022` 注释，
属于前端展示层，不在本报告的 Rust 链路内。

## 5. 差异对比总表

| 维度 | ZCode | DSH（DeepSeek Harness） |
| --- | --- | --- |
| 入口分支（`request_logs.rs:408-410`） | `request_speed_at(&RequestTiming{...}, now)` | `duration_speed(output, duration, duration_quality("dsh"), started_at)` |
| 分母产生时机 | **查询时现算**：`completed_at − first_token_at` | **导入时物化**：`derive_timing` 存入 duration_ms 列，查询时直接除 |
| 分母语义 | 首**内容里程碑**（含正文/思考/工具参数首证据，事件流解析）到完成 | stream **首末 chunk 之差**（首 chunk = block-start 吐字起点） |
| TTFT 扣除方式 | 显式减（span = completed − first） | duration 本身不含 TTFT，**不可再减**（再减会毁掉口径） |
| 首 token 缺失时 | 降级 RequestAverage（≈ 近似，started+completed 或 duration 兜底） | 无速度（None），无降级层 |
| 时序校验强度 | 全量：逆序拒、first<started 拒、未来时刻拒（observed_at=now） | 仅窗口边界 [200ms, 1h]（导入时 derive_timing 已拒 ≤0） |
| Generation 档位来源 | `request_speed_at` 内部判定（first+completed 齐备才算） | `duration_quality` 按 dsh 列语义硬编码 |
| 跨设备保真 | 必须走 SpeedEvidence 信封（proto 9，`annotate_zcode_speed` 生成） | 不需要：duration 列自证语义，回退重算即精确 |
| 可能的页面呈现 | 精确（Generation）/ 近似（≈，RequestAverage）/ 无（—） | 精确（Generation）/ 无（—） |

**共同点**（两路径严格一致的部分）：

1. 最终公式同为 `output_tokens × 1000 / 纯生成窗口ms`，TTFT 均不在分母内；
2. 样本边界同源：`output > 0`、窗口 ∈ [200ms, 1h]（`MIN_GEN_MS`/`MAX_GEN_MS`），
   越界整体拒收、不做 1 秒地板、不截断分母、无速度上限裁剪；
3. 完成状态过滤同源（`is_completed_status`）；
4. 均标记 `SpeedQuality::Generation`，进同一精确样本池，聚合时与 request_average
   池不混算（`SpeedTotals::merge`，Generation 存在时丢弃 RequestAverage）。

## 6. 测试佐证（接手后改动的回归基线）

- `token_speed.rs:495-505` `duration_sources_preserve_their_native_timing`：遍历
  7 个来源断言 `duration_quality` 档位（**dsh=Generation，其余 6 个=RequestAverage**），
  并断言 duration 路径的 NaN/过短窗口拒绝——改 `duration_quality` 或边界必挂；
- `token_speed.rs:735-807` `思考段缺started退回首字窗口_时长越界样本整体拒绝`：
  200ms/1h 边界含端点、越界样本不降级为 request_average 重新放行；
- `token_speed.rs:528-543、546-579`：短窗口无地板、RequestAverage 显式近似语义；
- `request_logs.rs:1506-1556`：同步序列化前后 speed 等值、legacy 远端走 evidence、
  **dsh 行窗口过短无速度**（1556 行）、`1060.0/4.2` 数值断言；
- `doc/token-speed-accounting.md` 验证节：`npm run test:speed`、
  `cargo test --lib local_speed_readonly_audit -- --ignored --nocapture`（读各来源真库复算）、
  `cargo test --lib local_zcode_speed_evidence_audit -- --ignored --nocapture`
  （ZCode 首内容证据与传输核验）。

## 7. 接手注意事项（刻意设计，勿当缺陷改掉）

1. **DSH 的 duration 绝不能再减 TTFT**——它已经是 stream 窗口。任何「生成段 =
   duration − ttft」的公式只允许出现在前端旧远端记录的展示兜底，不得进 Rust 侧 dsh 路径。
2. **ZCode 工具轮的数据库首字不可信**（`finish_reason=tool-calls` 等置 NULL），首内容
   证据来自独立事件日志的覆盖解析；这是 `zcode_speed_evidence.rs` 存在的全部理由，
   不要简化成裸读 `first_token_at` 列。
3. **两档样本池永不混算**是产品口径（≈ 标记的信任基础），`SpeedTotals::merge` 的
   「Generation 存在则丢弃 RequestAverage」必须保持与顺序无关。
4. **SpeedEvidence 信封「存在但 span 空 = 不可用」**，消费端不得回退 duration 重算
   （否则来源设备已验证的拒绝会被复活成近似值）。
5. 窗口边界 [200ms, 1h] 是「拒收坏样本」而非「改写分母」，与「不给短请求硬造 1 秒
   地板」的立场是两回事（`token_speed.rs:72-83` 注释），不要加地板或上限裁剪。
6. 所有来源库结构靠 `has_column` 动态探测（旧 schema 兼容），duration 列
   「没有或 ≤0 → 按无耗时下发、不估算回退」是明确决策（`request_logs.rs:356-365`）。
