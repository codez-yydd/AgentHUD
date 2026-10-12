/** 统计页的 Agent 展示偏好，仅影响界面展示，不影响本地采集和同步。 */
import type { MessageKey } from "./i18n";

export type AgentId =
  // 首批 5 个：本地会话/额度采集已接入
  | "zai"
  | "codex"
  | "claude"
  | "cursor"
  | "kimi"
  // 凭证驱动的新 provider（配额数据接入逐步上线；tab 需「已启用或有凭证」）
  | "gemini"
  | "grok"
  | "qoder"
  | "opencodego"
  | "minimax"
  | "moonshot"
  | "deepseek"
  | "longcat"
  | "mimo"
  | "alibaba"
  | "alibabatoken"
  | "stepfun"
  | "doubao"
  // 纯本地采集型（读取本地 ~/.dsh 会话数据，无凭证可录、无订阅额度窗口），
  // 与 claude/codex/kimi 同级：tab 纯偏好控制，默认隐藏
  | "dsh";

/**
 * 凭证驱动的新 provider 集合（tab 显示门槛 = 已启用或已有凭证）。
 * kimi 属首批 5 个（默认开启、tab 行为不变），但其凭证体系与其他新
 * provider 完全一致（~/.zbar/credentials/kimi.json，支持 OAuth 网页
 * 登录/手动粘贴），故同样纳入本集合参与「有凭证自动显示」联动。
 *
 * dsh（DeepSeek Harness）与 zai/codex/claude/cursor 同列：纯本地采集，
 * 既无凭证体系也无「有凭证自动显示」联动——必须显式排除，否则会因
 * Exclude 不完整被本类型吸收，进而强迫 CREDENTIAL_AGENT_KIND 为它编造
 * 一个凭证类型。
 */
export type CredentialAgentId = Exclude<
  AgentId,
  "zai" | "codex" | "claude" | "cursor" | "dsh"
>;

/**
 * 凭证体系名单。
 *
 * **为什么没有 moonshot / deepseek**（产品决定，别再补回来）：
 * 两者都是「独立余额/额度查询」型 provider，已被对应的本地采集型 agent 覆盖——
 * moonshot 是月之暗面开放平台余额，而 Kimi Code（本地 `~/.kimi-code` 会话 + 订阅
 * 额度）已经在了；deepseek 是平台余额，而本地 dsh（DeepSeek Harness 会话即 DeepSeek
 * 用量）已经在了。留着它们等于同一件事在「本地采集」里出现两遍（之前的重命名
 * 「Moonshot → Kimi」就是这么暴露出来的），故整项下架：不进设置列表、不进 tab、
 * 不参与 presence 探测与 120s 额度轮询。
 *
 * 只下架 UI 与轮询，不动历史数据：`AgentId` 联合类型、`AGENT_COLOR`、`AGENT_GROUP`、
 * `DEFAULT_VISIBILITY`、`CREDENTIAL_AGENT_KIND` 里仍保留这两个 id（那几张表都是
 * `Record<AgentId, …>`，删键会连带改类型），这样已写入 localStorage 的可见性键与
 * 已存在的凭证文件都还能被安全读到，只是不再有任何界面入口。
 */
export const CREDENTIAL_AGENTS: readonly CredentialAgentId[] = [
  "kimi",
  "gemini",
  "grok",
  "qoder",
  "opencodego",
  "minimax",
  "longcat",
  "mimo",
  "alibaba",
  "alibabatoken",
  "stepfun",
  "doubao",
];

/**
 * presence 探测名单：CREDENTIAL_AGENTS 之外追加 claude/cursor——两者的
 * 「其他账号」额度区需要按「是否有凭证」决定补刷与 120s 轮询（探测结果
 * 进同一 credentialPresence state，DataCache 轮询自动覆盖；kimi 的「其他
 * 账号」区同理，由 CREDENTIAL_AGENTS 覆盖）。
 * tab 显隐门槛：CREDENTIAL_AGENTS 中除 PURE_PREFERENCE_AGENTS（kimi）外
 * 均为「已启用或有凭证」；kimi 属首批 5 个，与 claude/cursor 一样纯偏好
 * 控制——presence 只驱动「其他账号」区的补刷/轮询，不影响 tab 显隐。
 *
 * dsh 也在此列：它是纯本地采集型（无凭证体系，故不进 CREDENTIAL_AGENTS），
 * 但后端 has_provider_credentials 对其特判为「~/.dsh 会话目录是否存在」。
 * 于是「装了 DSH 才出现 tab」与 claude/codex 的默认可见语义一致，
 * 又不至于让没装的用户看到空 tab。
 */
export const PRESENCE_PROVIDERS: readonly string[] = [
  ...CREDENTIAL_AGENTS,
  "claude",
  "cursor",
  "dsh",
];

/**
 * tab 展示纯偏好控制的首批凭证型 provider（kimi）：虽已纳入凭证体系
 * （有「其他账号」区、presence 补刷与 120s 轮询），但 tab 显隐不对
 * presence 让步——与 claude/cursor 同语义。否则会出现两个问题：有凭证时
 * 设置页的 kimi 开关失效（presence 恒让 tab 显示）；删光凭证会连带隐藏
 * 本地 CLI 主额度卡（kimi tab 承载的是 CLI 用量主面板）。tab 恒由
 * DEFAULT_VISIBILITY / 设置页开关控制。
 */
export const PURE_PREFERENCE_AGENTS: readonly string[] = ["kimi"];

/** id 是否为「tab 纯偏好控制」的凭证型 provider（presence 不参与门槛）。 */
export function isPurePreferenceAgent(id: string): boolean {
  return PURE_PREFERENCE_AGENTS.includes(id);
}

/** id 是否属于凭证驱动的新 provider（用于 tab 渲染分支与类型收窄）。 */
export function isCredentialAgent(id: string): id is CredentialAgentId {
  return (CREDENTIAL_AGENTS as readonly string[]).includes(id);
}

/**
 * 本地型 provider 集合：数据由后端直读本地文件/数据库（无凭证也能出数据），
 * tab 门槛 = 本地数据存在；凭证体系保持可用但为「可选」（不做强引导）。
 * gemini（Gemini CLI 的 OAuth 登录态）与 opencodego 同属本地直读型；
 * minimax 自合并 MiniMax Code 本地用量（~/.minimax）后也归此类——用量来自本地
 * 库，凭证只决定「有没有套餐额度卡」。
 */
export const LOCAL_AGENTS: readonly CredentialAgentId[] = [
  "opencodego",
  "gemini",
  "minimax",
];

/** id 是否属于本地型 provider（GenericQuotaPanel 据此切换凭证卡空态文案）。 */
export function isLocalAgent(id: string): boolean {
  return (LOCAL_AGENTS as readonly string[]).includes(id);
}

/**
 * 双数据源 provider：本地数据与手动凭证是**两条并列来源**，任一可得即可出
 * 数据（后端 presence 为 OR 语义）。与纯本地型（gemini：只有本地 CLI 登录态
 * 一条路，凭证无意义）的区别在于「凭证是否真的参与查询」——
 * - opencodego：本地 opencode.db 读的是本机 CLI 用量；用户没装 CLI 时，
 *   配官方 API Key 走 opencode.ai/zen/go/v1/usage 同样能拿到官方口径数据。
 * - minimax：本地 ~/.minimax 读的是 MiniMax Code 的逐次调用用量；套餐额度
 *   （5h + 周窗口）是另一条路，由已存的 sk-cp- Key 查官方接口得到——两条
 *   链路在面板里并列展示，缺任一条都不影响另一条。
 * 这类 provider 的**双源能力**保留（后端仍按 OR 语义查询，已存在的凭证照旧
 * 参与），但录入入口已按产品决定收口为「像 agent 一样不提供凭证管理」，
 * 见 NO_CREDENTIAL_ENTRY_AGENTS。
 */
export const DUAL_MODE_AGENTS: readonly CredentialAgentId[] = [
  "opencodego",
  "minimax",
];

/** id 是否属于双数据源 provider（凭证入口据此放行）。 */
export function isDualModeAgent(id: string): boolean {
  return (DUAL_MODE_AGENTS as readonly string[]).includes(id);
}

/**
 * 纯本地型 provider：**只有本地数据一条来源**，凭证根本不参与查询（gemini
 * 读的是 Gemini CLI 的 OAuth 登录态，没有可录入的 Key）。这类 provider 在
 * 「添加服务」浮层与设置页里都不该出现——列出来也无凭证可录。
 *
 * 注意与 LOCAL_AGENTS 的区别：LOCAL_AGENTS 表达的是「本地数据也算数据来源」
 * （用于空态文案与轮询门槛），双数据源型（opencodego）也在其中；而本集合
 * 表达的是「**只能**靠本地数据」，是判断「能否录入凭证」的静态依据。
 */
export const PURE_LOCAL_AGENTS: readonly CredentialAgentId[] = ["gemini"];

/** id 是否属于「只能靠本地数据、无凭证可录」的 provider。 */
export function isPureLocalAgent(id: string): boolean {
  return (PURE_LOCAL_AGENTS as readonly string[]).includes(id);
}

/**
 * 界面不提供凭证录入入口的 provider（与 PURE_LOCAL_AGENTS 是两回事，见下）：
 * - opencodego：OpenCode 与 Kimi Code、ZCode 一样是自带 CLI 与订阅的 agent，
 *   数据来自自身 CLI 的本地库（`~/.local/share/opencode/opencode.db`）；
 * - minimax：MiniMax Code 同理（本地 ~/.minimax 采集用量 + 自家 Coding Plan
 *   订阅额度），按同一「agent 口径」收口；
 * - moonshot：月之暗面开放平台余额，按同一「agent 口径」一并收口（产品决定）；
 * - deepseek：已有 dsh（DeepSeek Harness，本地会话即 DeepSeek 用量），
 *   不再单独提供平台余额查询的录入入口（产品决定）。
 *
 * 收口的是**入口**，不是能力：后端查询链路与已存在的凭证完全不动——
 * 已经配过 Key 的机器照旧出数据（minimax 的套餐额度卡就靠它），只是界面
 * 不再提供「添加凭证 / 多账号」入口，因此无法新增。与 PURE_LOCAL_AGENTS
 * 的区别：那个是「技术上没有可录的凭证」（gemini 只有 CLI 登录态），
 * 这个是「产品上不提供入口」。
 */
export const NO_CREDENTIAL_ENTRY_AGENTS: readonly CredentialAgentId[] = [
  "opencodego",
  "minimax",
  "moonshot",
  "deepseek",
];

/** id 是否在界面上提供凭证录入入口（静态判定）。 */
export function hasCredentialEntry(id: string): boolean {
  return !(NO_CREDENTIAL_ENTRY_AGENTS as readonly string[]).includes(id);
}

/**
 * 该 provider 是否可以录入凭证（静态属性，与当前是否已配凭证无关）。
 * 凭证录入入口（添加服务浮层、设置页快捷按钮、凭证卡添加按钮）统一用它判断，
 * 避免把「当前只有本地数据」的运行时状态误当作「不能录入凭证」。
 * 两类为 false：技术上无凭证可录（gemini），产品上不提供入口
 * （opencodego / minimax / moonshot，见 NO_CREDENTIAL_ENTRY_AGENTS）。
 */
export function canAddCredential(id: string): boolean {
  return !isPureLocalAgent(id) && hasCredentialEntry(id);
}

/**
 * 该 provider 当前是否需要「只有本地数据源」的表达（空态文案与凭证入口收口）。
 * 纯本地型恒为 true；双数据源型仅在没有 API Key 时才用本地口径——一旦配了
 * Key，凭证卡应回到常规的凭证列表形态（可加/可删/可看校验结论）。
 */
export function localOnlyMode(id: string, hasCredentials: boolean): boolean {
  if (!isLocalAgent(id)) return false;
  return !isDualModeAgent(id) || !hasCredentials;
}

export type AgentVisibility = Record<AgentId, boolean>;

/** 各 Agent 的品牌展示色（汇总页与对比页共用，保证跨页视觉一致）。 */
export const AGENT_COLOR: Record<AgentId, string> = {
  zai: "#0ea5e9",
  codex: "#10a37f",
  claude: "#d97757",
  cursor: "#8b5cf6",
  kimi: "#4338ca",
  gemini: "#4285F4",
  grok: "#1d9bf0",
  qoder: "#6c5ce7",
  opencodego: "#f97316",
  minimax: "#00b96b",
  moonshot: "#4540d6",
  deepseek: "#4D6BFE",
  // 亮黄在暗色主题下文字对比度不足，正文色用深一档
  longcat: "#ffd100",
  mimo: "#ff6900",
  alibaba: "#ff6a00",
  alibabatoken: "#ff9240",
  stepfun: "#3b82f6",
  doubao: "#00d4aa",
  dsh: "#4D6BFE",
};

/** 同一 Agent 的多账号系列用同色系色阶区分（明度递增，4 档，超出取最末档）。
 *  不用透明度方案：细柱上不可辨，且低透明度有"弱化"的语义误导。 */
export const AGENT_COLOR_SCALE: Record<AgentId, readonly string[]> = {
  zai: ["#0ea5e9", "#38bdf8", "#7dd3fc", "#bae6fd"],
  codex: ["#10a37f", "#34d399", "#6ee7b7", "#a7f3d0"],
  claude: ["#d97757", "#fb923c", "#fdba74", "#fed7aa"],
  cursor: ["#8b5cf6", "#a78bfa", "#c4b5fd", "#ddd6fe"],
  kimi: ["#4338ca", "#6366f1", "#818cf8", "#a5b4fc"],
  gemini: ["#2f6fe0", "#4285F4", "#7aa7f8", "#aec9fb"],
  grok: ["#0f7ecb", "#1d9bf0", "#57b3f4", "#8ccaf8"],
  qoder: ["#5a4bd0", "#6c5ce7", "#8d7ded", "#ae9ff2"],
  opencodego: ["#d96110", "#f97316", "#fb8f47", "#fcaa74"],
  minimax: ["#009657", "#00b96b", "#3fcf90", "#78dcae"],
  moonshot: ["#3833b4", "#4540d6", "#6b66e0", "#918ce9"],
  deepseek: ["#3a53d0", "#4D6BFE", "#7a90fe", "#a6b4fe"],
  longcat: ["#c9a400", "#e6bd00", "#ffd100", "#ffdb4d"],
  mimo: ["#d95700", "#ff6900", "#ff8733", "#ffa466"],
  alibaba: ["#d95700", "#ff6a00", "#ff8833", "#ffa666"],
  alibabatoken: ["#e5772c", "#ff9240", "#ffa866", "#ffbd8c"],
  stepfun: ["#2d68cc", "#3b82f6", "#6ba1f9", "#96bffb"],
  doubao: ["#00a486", "#00d4aa", "#33debb", "#66e6cc"],
  // DeepSeek 蓝同色系 4 档（与 deepseek 同款色阶）
  dsh: ["#3a53d0", "#4D6BFE", "#7a90fe", "#a6b4fe"],
};

/**
 * 设置页「统计展示来源」的分组归属（纯界面分组，不影响采集与同步）。
 *
 * 判据是**数据从哪来**，不是「有没有凭证」：
 * - local：「本地采集」——解析本地 CLI 会话日志 → 派生自有 sqlite → 参与多设备
 *   同步上传。与后端同步白名单 SYNC_SOURCE_WHITELIST（server/app.py）及
 *   projects.rs 的来源列表完全一致（此处的 zai 对应 source 名 "zcode"）。
 * - credential：「凭证接入」——数据来自 API Key / Cookie 等凭证查询。
 *
 * 特意用 Record<AgentId, ...> 而非数组：新增 agent 时 TypeScript 会强制在此
 * 表态，不会「漏登记 → 静默归错组」。此处曾两次踩坑，都是同一类错误：
 * 1. 原实现按 AGENT_VISIBILITY_OPTIONS 的 slice(0,5)/slice(5) 位置魔法数切分，
 *    新 agent 插到中间即静默归错组；
 * 2. 随后改按 isCredentialAgent 判定 —— 依然错：kimi 另有凭证体系
 *    （~/.zbar/credentials/kimi.json、网页 OAuth 登录、汇总页「其他账号」区）
 *    故属于 CREDENTIAL_AGENTS，于是被当成凭证型挪出「本地采集」，而它的用量
 *    数据其实来自本地 ~/.kimi-code/sessions 会话日志。
 * 两个集合回答的是不同问题，不可互相替代：CREDENTIAL_AGENTS 管「凭证体系与
 * tab 显示门槛」，AGENT_GROUP 管「数据来源归类」。
 *
 * gemini 虽也有本地读取路径（见 LOCAL_AGENTS），但只有 CLI 登录态、不解析会话
 * 日志，故仍归 credential（维持既有展示行为）。
 * opencodego 归 local：自接入 opencode 来源起，它的用量就是解析本地库得来的
 * （并进了同步白名单）。minimax 同理——合并 MiniMax Code 后用量确实解析本地库
 * （~/.minimax），与 kimi 一样是「本地采集为主、订阅额度为辅」的形态。
 */
export type AgentGroupKind = "local" | "credential";

export const AGENT_GROUP: Record<AgentId, AgentGroupKind> = {
  zai: "local",
  codex: "local",
  claude: "local",
  cursor: "local",
  kimi: "local",
  dsh: "local",
  gemini: "credential",
  grok: "credential",
  qoder: "credential",
  opencodego: "local",
  minimax: "local",
  moonshot: "local",
  deepseek: "credential",
  longcat: "credential",
  mimo: "credential",
  alibaba: "credential",
  alibabatoken: "credential",
  stepfun: "credential",
  doubao: "credential",
};

/** id 是否属于「本地采集」分组（未知 id 回退 false，不抛错）。 */
export function isLocalCollectAgent(id: string): boolean {
  return AGENT_GROUP[id as AgentId] === "local";
}

// 模式 A：label 是品牌名不进词典；description 存词典键，渲染时查（跟随 UI 语言）
export const AGENT_VISIBILITY_OPTIONS: ReadonlyArray<{
  id: AgentId;
  label: string;
  descriptionKey: MessageKey;
}> = [
  {
    id: "zai",
    label: "Z.ai",
    descriptionKey: "settings.agentZaiDesc",
  },
  {
    id: "codex",
    label: "Codex",
    descriptionKey: "settings.agentCodexDesc",
  },
  {
    id: "claude",
    label: "Claude",
    descriptionKey: "settings.agentClaudeDesc",
  },
  {
    id: "cursor",
    label: "Cursor",
    descriptionKey: "settings.agentCursorDesc",
  },
  {
    id: "kimi",
    label: "Kimi Code",
    descriptionKey: "settings.agentKimiDesc",
  },
  // 紧随首批 5 个本地采集型之后：分组由 AGENT_GROUP 语义决定（与数组下标
  // 无关），此处位置只决定「本地采集」组内的显示顺序
  {
    id: "dsh",
    label: "DeepSeek Harness",
    descriptionKey: "settings.agentDshDesc",
  },
  {
    // 合并 MiniMax Code 后仍是既有的 minimax（凭证、可见性、额度卡都不动），
    // 只是显示名与分组随「本地用量为主」的形态调整
    id: "minimax",
    label: "MiniMax Code",
    descriptionKey: "settings.agentMinimaxDesc",
  },
  {
    id: "gemini",
    label: "Gemini",
    descriptionKey: "settings.agentGeminiDesc",
  },
  {
    id: "grok",
    label: "Grok",
    descriptionKey: "settings.agentGrokDesc",
  },
  {
    id: "qoder",
    label: "Qoder",
    descriptionKey: "settings.agentQoderDesc",
  },
  {
    id: "opencodego",
    label: "OpenCode",
    descriptionKey: "settings.agentOpencodegoDesc",
  },
  {
    id: "longcat",
    label: "LongCat",
    descriptionKey: "settings.agentLongcatDesc",
  },
  {
    id: "mimo",
    label: "MiMo",
    descriptionKey: "settings.agentMimoDesc",
  },
  {
    id: "alibaba",
    label: "通义灵码",
    descriptionKey: "settings.agentAlibabaDesc",
  },
  {
    id: "alibabatoken",
    label: "百炼Token包",
    descriptionKey: "settings.agentAlibabatokenDesc",
  },
  {
    id: "stepfun",
    label: "StepFun",
    descriptionKey: "settings.agentStepfunDesc",
  },
  {
    id: "doubao",
    label: "火山引擎",
    descriptionKey: "settings.agentDoubaoDesc",
  },
];

/** 凭证驱动 provider 的录入形态：凭证类型（决定添加弹层的输入引导）。
 *  grok 并入本地 ~/.grok/auth.json 读取，手动补充的凭证为访问 Token。
 *  kimi 的添加表单提供类型选择（默认 OAuth 令牌：网页登录/手动粘贴
 *  refresh_token 存 kind=token；可切 API Key 存 kind=apiKey），服务端
 *  同一鉴权接口，两种形态都能查额度。 */
export const CREDENTIAL_AGENT_KIND: Record<CredentialAgentId, "apiKey" | "cookie" | "token"> = {
  // kimi 默认形态为 OAuth 令牌（refresh_token）；API Key 为表单内可选形态
  kimi: "token",
  gemini: "apiKey",
  grok: "token",
  qoder: "cookie",
  opencodego: "apiKey",
  minimax: "apiKey",
  moonshot: "apiKey",
  deepseek: "apiKey",
  longcat: "cookie",
  // MiMo 凭证是浏览器复制的 Cookie（需含 api-platform_serviceToken 与 userId）
  mimo: "cookie",
  alibaba: "apiKey",
  // 阿里 Token Plan（百炼 Token 包）凭证是浏览器复制的控制台 Cookie
  //（Team 订阅摘要与 Personal/Solo 滚动窗口共用，region 分中国站/国际站）
  alibabatoken: "cookie",
  // StepFun 凭证是浏览器复制的 Oasis-Token（platform.stepfun.com 登录态 JWT）
  stepfun: "token",
  doubao: "apiKey",
};

const STORAGE_KEY = "zbar-agent-visibility";

/** 「有凭证自动显示」的显式关闭标记（独立 localStorage 键，向后兼容：
 *  不改 visibility 结构，旧数据无需迁移）。用户在设置页手动关闭某
 *  凭证型 agent 后记录，此后 presence 无→有不再自动开启该 agent；
 *  再次手动开启时清除标记。 */
const MANUALLY_DISABLED_KEY = "zbar-agent-manually-disabled";

type ManuallyDisabledMap = Partial<Record<AgentId, boolean>>;

function loadManuallyDisabled(): ManuallyDisabledMap {
  try {
    const raw = localStorage.getItem(MANUALLY_DISABLED_KEY);
    if (!raw) return {};
    const parsed = JSON.parse(raw) as ManuallyDisabledMap;
    if (parsed && typeof parsed === "object") return parsed;
  } catch {
    // 存储不可用/内容损坏时视为无标记，仅影响自动开启行为
  }
  return {};
}

function saveManuallyDisabled(map: ManuallyDisabledMap): void {
  try {
    localStorage.setItem(MANUALLY_DISABLED_KEY, JSON.stringify(map));
  } catch {
    // 隐私模式或存储配额不足时静默降级（仅丢失「不再自动开启」记忆）
  }
}

/** 设置页手动关闭时记录显式关闭标记（幂等）。 */
export function markAgentManuallyDisabled(id: AgentId): void {
  const map = loadManuallyDisabled();
  if (map[id]) return;
  saveManuallyDisabled({ ...map, [id]: true });
}

/** 手动开启时清除显式关闭标记（此后恢复「有凭证自动显示」联动）。 */
export function clearAgentManuallyDisabled(id: AgentId): void {
  const map = loadManuallyDisabled();
  if (!map[id]) return;
  const next = { ...map };
  delete next[id];
  saveManuallyDisabled(next);
}

/** 该 agent 是否被用户显式关闭过（enableAgentByCredential 的触发前置检查）。 */
export function isAgentManuallyDisabled(id: AgentId): boolean {
  return !!loadManuallyDisabled()[id];
}

/** 首批 5 个 agent 默认开启（升级后行为不变）；新 provider 默认隐藏，
 *  由「有凭证自动开启」或设置页手动开启。 */
const DEFAULT_VISIBILITY: AgentVisibility = {
  zai: true,
  codex: true,
  claude: true,
  cursor: true,
  kimi: true,
  gemini: false,
  grok: false,
  qoder: false,
  opencodego: false,
  minimax: false,
  moonshot: false,
  deepseek: false,
  longcat: false,
  mimo: false,
  alibaba: false,
  alibabatoken: false,
  stepfun: false,
  doubao: false,
  // 纯本地采集型：无凭证联动，默认隐藏，由设置页开关开启
  dsh: false,
};

/** 读取展示偏好；缺失或损坏字段回退默认值（首批 5 个保持「缺省即开」，
 *  新 provider「缺省即关」），保证升级后行为不变。 */
export function loadAgentVisibility(): AgentVisibility {
  const next = { ...DEFAULT_VISIBILITY };
  try {
    const raw = localStorage.getItem(STORAGE_KEY);
    if (!raw) return next;
    const parsed = JSON.parse(raw) as Partial<Record<AgentId, unknown>>;
    for (const id of Object.keys(DEFAULT_VISIBILITY) as AgentId[]) {
      if (typeof parsed[id] === "boolean") {
        next[id] = parsed[id] as boolean;
      }
    }
  } catch {
    // 存储不可用/内容损坏时全量回退默认值
  }
  return next;
}

/** 设置页切换后立即保存，异常时静默降级为当前会话内状态。 */
export function saveAgentVisibility(visibility: AgentVisibility): void {
  try {
    localStorage.setItem(STORAGE_KEY, JSON.stringify(visibility));
  } catch {
    // 隐私模式或存储配额不足不应阻断设置页操作。
  }
}

// ============================================================
// 跨层联动事件：visibility 状态提升在 App 层，localStorage 写入可能
// 发生在任意组件层（设置页开关 / 凭证添加自动开启），统一用 window
// 自定义事件通知 App 重读——最小侵入，不引入新的状态管理。
// ============================================================

/** visibility 变化广播（enableAgentByCredential 触发，App 监听后重读 localStorage） */
export const AGENT_VISIBILITY_EVENT = "zbar-agent-visibility-changed";

/** 凭证增删改广播（detail = { provider }）：DataCache 刷新凭证缓存、
 *  App 重查 presence 决定是否自动开启对应 agent 的展示。 */
export const CREDENTIALS_CHANGED_EVENT = "zbar-credentials-changed";

/**
 * 「有凭证自动显示」：把该 provider 的 visibility 置 true 并持久化。
 * 仅在当前为 false 时写入（幂等）。用户在设置页显式关闭过的 agent
 * （见 MANUALLY_DISABLED_KEY 标记）不再自动开启——凭证从无到有仅在
 * 用户未表达过「不要它」时才视为需要展示；重新手动开启即恢复联动。
 */
export function enableAgentByCredential(id: AgentId): void {
  if (isAgentManuallyDisabled(id)) return;
  const current = loadAgentVisibility();
  if (current[id]) return;
  saveAgentVisibility({ ...current, [id]: true });
  window.dispatchEvent(new Event(AGENT_VISIBILITY_EVENT));
}

/**
 * 删除某凭证型 agent 的最后一条凭证后回退展示偏好：visibility 置 false
 * 并持久化（与 enableAgentByCredential 对称）。仅在当前为 true 时写入
 * （幂等）；设置页可随时手动重开，不受标记影响。
 */
export function disableAgentByCredential(id: AgentId): void {
  const current = loadAgentVisibility();
  if (!current[id]) return;
  saveAgentVisibility({ ...current, [id]: false });
  window.dispatchEvent(new Event(AGENT_VISIBILITY_EVENT));
}

/** 凭证增删改后广播（CredentialsCard 在操作成功后调用）。 */
export function notifyCredentialsChanged(provider: string): void {
  window.dispatchEvent(
    new CustomEvent(CREDENTIALS_CHANGED_EVENT, { detail: { provider } })
  );
}

// ============================================================
// 数据来源字符串（后端 source 字段："zcode" | "codex" | ...）的展示元数据。
// 项目浏览器等按 source 字符串工作的界面使用，保证与
// AGENT_COLOR / AGENT_VISIBILITY_OPTIONS 的品牌色与名称一致。
// ============================================================

/** 未知来源的兜底展示：中性灰 */
const UNKNOWN_SOURCE_META = { label: "Unknown", color: "#94a3b8" };

const SOURCE_META: Record<string, { label: string; color: string }> = {
  zcode: { label: "Z.ai", color: AGENT_COLOR.zai },
  codex: { label: "Codex", color: AGENT_COLOR.codex },
  claude: { label: "Claude", color: AGENT_COLOR.claude },
  cursor: { label: "Cursor", color: AGENT_COLOR.cursor },
  kimi: { label: "Kimi Code", color: AGENT_COLOR.kimi },
  gemini: { label: "Gemini", color: AGENT_COLOR.gemini },
  grok: { label: "Grok", color: AGENT_COLOR.grok },
  qoder: { label: "Qoder", color: AGENT_COLOR.qoder },
  opencodego: { label: "OpenCode", color: AGENT_COLOR.opencodego },
  minimax: { label: "MiniMax", color: AGENT_COLOR.minimax },
  moonshot: { label: "Kimi", color: AGENT_COLOR.moonshot },
  deepseek: { label: "DeepSeek", color: AGENT_COLOR.deepseek },
  longcat: { label: "LongCat", color: AGENT_COLOR.longcat },
  mimo: { label: "MiMo", color: AGENT_COLOR.mimo },
  alibaba: { label: "通义灵码", color: AGENT_COLOR.alibaba },
  alibabatoken: { label: "百炼Token包", color: AGENT_COLOR.alibabatoken },
  stepfun: { label: "StepFun", color: AGENT_COLOR.stepfun },
  doubao: { label: "火山引擎", color: AGENT_COLOR.doubao },
  dsh: { label: "DeepSeek Harness", color: AGENT_COLOR.dsh },
  // OpenCode 是首个「UI id 与后端来源名不同字面」的 agent：后端来源名固定
  // "opencode"（本地 opencode.db 导入库、项目页会话、悬浮窗分组、远端上传
  // 都用它），而面板 id / 颜色 / 可见性键是 "opencodego"。两个域都查这张表
  // ——项目页会话来源、报告图例按来源名查（"opencode"，缺了会显示 Unknown），
  // 面板与设置项按 agent id 查（"opencodego"）——故两条都登记、指向同一品牌。
  opencode: { label: "OpenCode", color: AGENT_COLOR.opencodego },
  // MiniMax 同为两个域异名（UI id `minimax` / 后端来源名 `minimaxcode`，
  // 见 StatsPanel 的 minimax 分支与 MiniMaxPanel）：项目页会话来源、报告图例、
  // 悬浮窗分组都按来源名查，缺了会显示 Unknown；品牌色与图标复用 minimax。
  minimaxcode: { label: "MiniMax Code", color: AGENT_COLOR.minimax },
};

/** source 字符串 → 品牌名 + 品牌色（未知来源回退中性灰，不抛错） */
export function sourceMeta(source: string): { label: string; color: string } {
  return SOURCE_META[source] ?? UNKNOWN_SOURCE_META;
}
