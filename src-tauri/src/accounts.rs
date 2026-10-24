//! 多智谱账号切换：快照存储 + 切换事务 + ZCode 进程控制。
//!
//! 【重要声明】本模块是整个应用中**唯一**允许写 ZCode 数据目录的位置
//! （默认 `~/.zcode/v2/`，支持 ZCode「更改数据目录」迁移，定位见 zcode_dir），
//! 且只写 `credentials.json` 与 `config.json` 两个文件、只发生在切换事务
//! （switch_account）内部。额度查询（quota.rs）对该目录严格只读，两者互不干扰。
//!
//! 设计要点：
//! - 快照保存在 `~/.agenthud/accounts/<id>.account.json`（更名前 ~/.zbar，启动自动迁移；
//!   目录 0700、文件 0600），
//!   其中 credentials 按原文整串保存——键集由 ZCode 客户端自行演进，原文回写
//!   天然兼容未来的键增删；config 只保存 key 含 `coding-plan` 的 provider。
//! - 切换事务顺序 = 先退出桌面应用后写入：ZCode 运行中写凭证文件会被其
//!   内部状态覆盖或破坏登录态，必须先退出（CLI 进程不动，新调用自然读新配置）。
//! - 切换前先把两文件原文备份到 `~/.agenthud/accounts/.last/`（更名前 ~/.zbar，
//!   启动自动迁移；点开头不被快照扫描命中），任何一步失败走回滚，保证零损坏。
//! - 切换写 credentials.json 时以快照原文为基础，**合并现场新增/更新的
//!   `account-provider:` 前缀键**（ZCode 3.12+ 的账号级套餐凭证键，ZCode 判定
//!   套餐资格依赖它们，且跨账号累积存在）；其余键一律以快照为准。合并不可用
//!   （任一侧解析失败）时降级为原文整串回写，绝不让合并逻辑使切换失败。
//! - 切换前用现场数据刷新"即将被切走账号"的**已存在**快照（不新建条目），
//!   保证被切走账号的快照不会停留在过期键集上。
//! - 指纹（user_id）解密失败时降级为 unknown-id 快照，不阻塞捕获
//!   （仅手动捕获；自动捕获轮询对无指纹轮次整轮跳过，防 unknown 快照
//!   无限堆积，见 upsert_snapshot_at_opts）。

use crate::pricing::config_dir;
use crate::zcode_crypto::{fingerprint_of_credentials, Fingerprint};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use tauri::{AppHandle, Emitter};
// macOS/Windows 退出进程的轮询等待使用（Linux 下避免 unused import）
#[cfg(any(target_os = "macos", windows))]
use std::time::Duration;

// ============================================================
// 数据结构
// ============================================================

/// 账号快照元信息（列表/捕获返回给前端的形态）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AccountMeta {
    pub id: String,
    pub display_name: String,
    pub email: Option<String>,
    pub fingerprint: String,
    pub created_at: i64,
    /// 最近一次捕获/刷新时间（主窗口「凭证与账号管理」页展示用；磁盘快照
    /// 自始有值，default 仅为反序列化旧数据兜底）
    #[serde(default)]
    pub updated_at: i64,
    /// 捕获时选中的 Coding Plan provider（ZCode 登录渠道标识；老快照为 None）
    #[serde(default)]
    pub login_provider: Option<String>,
    #[serde(default)]
    pub is_current: bool,
}

/// 当前实时登录账号（对 ~/.zcode/v2/credentials.json 解密推断而来）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CurrentAccount {
    pub fingerprint: String,
    pub email: Option<String>,
    /// 当前账号匹配到的快照 id（未捕获过时为 None）
    pub matched_snapshot_id: Option<String>,
}

/// list_accounts 返回：当前登录 + 快照列表。
#[derive(Debug, Serialize, Deserialize)]
pub struct AccountsState {
    pub current: Option<CurrentAccount>,
    pub accounts: Vec<AccountMeta>,
}

/// capture_account 返回。
#[derive(Debug, Serialize)]
pub struct CaptureOutcome {
    pub account: AccountMeta,
    pub updated_existing: bool,
}

/// switch_account 返回。
#[derive(Debug, Serialize)]
pub struct SwitchOutcome {
    pub switched_to: String,
    /// ZCode 桌面应用是否自动重启成功（false 时前端提示手动打开）
    pub zcode_relaunched: bool,
}

/// 磁盘上的快照完整内容（~/.agenthud/accounts/<id>.account.json）。
#[derive(Debug, Clone, Serialize, Deserialize)]
struct AccountSnapshot {
    version: i32,
    id: String,
    fingerprint: String,
    display_name: String,
    email: Option<String>,
    created_at: i64,
    updated_at: i64,
    /// credentials.json 原文整串（保留原始格式与全部键，回写零损失）
    credentials_raw: String,
    /// config.json 中 key 含 "coding-plan" 的 provider（含 apiKey 为空的）
    config_providers: Map<String, Value>,
    /// 捕获时按固定顺序选中的 Coding Plan provider key。
    /// 多账号切换后 live config 可能混入其他账号的同前缀 key（切换事务只逐
    /// key 覆盖不清理），额度查询优先按本字段取凭证，保证「快照身份 ↔ 凭证」
    /// 一一对应（老快照无此字段，None 时回退 pick 固定顺序）。
    #[serde(default)]
    login_provider: Option<String>,
    /// 用户是否手动重命名过（true 时重捕获不再覆盖 display_name；
    /// 老快照无此字段，serde 默认 false 即"未锁定"，保持原刷新行为）
    #[serde(default)]
    name_locked: bool,
}

/// 切换前读到的两文件现场（None = 文件不存在）。
#[derive(Debug, Clone, Default)]
struct LiveFiles {
    credentials: Option<String>,
    config: Option<String>,
}

// ============================================================
// 第一节：快照存储（~/.agenthud/accounts/）
// ============================================================

/// 全部写操作（捕获/切换/删除/重命名）共用的互斥锁，
/// 防止并发写快照目录与 ~/.zcode 造成交错损坏。
static ACCOUNTS_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

fn accounts_lock() -> &'static Mutex<()> {
    ACCOUNTS_LOCK.get_or_init(|| Mutex::new(()))
}

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

/// 快照目录（生产路径 ~/.agenthud/accounts/，更名前 ~/.zbar，启动自动迁移）。
fn accounts_dir() -> Result<PathBuf, String> {
    Ok(config_dir()?.join("accounts"))
}

/// 切换前备份目录（~/.agenthud/accounts/.last/，更名前 ~/.zbar，启动自动迁移；点开头不被快照扫描命中）。
fn backup_dir() -> Result<PathBuf, String> {
    Ok(accounts_dir()?.join(".last"))
}

/// ZCode 数据目录与两文件路径。支持 ZCode「更改数据目录」（setting.json 的
/// dataBaseDir，如 D:\app\ZCode-cache）——迁移后凭证写在 {dataBaseDir}/.zcode/v2/，
/// 默认位置只剩旧数据；解析逻辑见 quota::zcode_v2_dir（本应用唯一入口）。
fn zcode_dir() -> Result<PathBuf, String> {
    crate::quota::zcode_v2_dir()
}

fn credentials_path() -> Result<PathBuf, String> {
    Ok(zcode_dir()?.join("credentials.json"))
}

fn zcode_config_path() -> Result<PathBuf, String> {
    Ok(zcode_dir()?.join("config.json"))
}

/// id 只允许 [A-Za-z0-9_-]，防路径穿越。
fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

/// 由指纹生成快照 id（过滤非法字符；解密失败退化 unknown-<时间戳>）。
fn snapshot_id_of(fp: Option<&Fingerprint>) -> String {
    match fp {
        Some(f) => {
            let filtered: String = f
                .user_id
                .chars()
                .filter(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '-')
                .collect();
            if filtered.is_empty() {
                format!("unknown-{}", now_ms())
            } else {
                filtered
            }
        }
        None => format!("unknown-{}", now_ms()),
    }
}

/// 目录权限收紧到 0700（仅 Unix；Windows 走 ACL 不处理）。
fn harden_dir(dir: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(dir, fs::Permissions::from_mode(0o700));
    }
    let _ = dir;
}

/// 文件权限收紧到 0600。
fn harden_file(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o600));
    }
    let _ = path;
}

/// 同目录临时文件 + rename 原子写（快照含凭证，必须避免半截文件）。
fn atomic_write(path: &Path, contents: &str) -> Result<(), String> {
    let dir = path
        .parent()
        .ok_or_else(|| format!("路径缺少父目录: {}", path.display()))?;
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "file".into());
    let tmp = dir.join(format!(".{name}.tmp"));
    fs::write(&tmp, contents).map_err(|e| format!("写入临时文件失败: {e}"))?;
    harden_file(&tmp);
    fs::rename(&tmp, path).map_err(|e| format!("替换文件失败: {e}"))
}

/// 在指定基目录下保存快照（tmp+rename，目录 0700 / 文件 0600）。
fn save_snapshot_at(base: &Path, snap: &AccountSnapshot) -> Result<(), String> {
    let dir = base.join("accounts");
    fs::create_dir_all(&dir).map_err(|e| format!("创建快照目录失败: {e}"))?;
    harden_dir(&dir);
    let data = serde_json::to_string_pretty(snap)
        .map_err(|e| format!("序列化快照失败: {e}"))?;
    atomic_write(&dir.join(format!("{}.account.json", snap.id)), &data)
}

/// 读取指定基目录下的单个快照（不存在/损坏/非法 id 均返回 None）。
fn load_snapshot_at(base: &Path, id: &str) -> Option<AccountSnapshot> {
    if !valid_id(id) {
        return None;
    }
    let path = base.join("accounts").join(format!("{id}.account.json"));
    let raw = fs::read_to_string(path).ok()?;
    serde_json::from_str(&raw).ok()
}

/// 扫描指定基目录下全部快照完整内容（损坏文件跳过并记日志），按创建时间升序。
fn load_snapshots_at(base: &Path) -> Vec<AccountSnapshot> {
    let dir = base.join("accounts");
    let entries = match fs::read_dir(&dir) {
        Ok(e) => e,
        Err(_) => return vec![], // 目录不存在视为空，不报错
    };
    let mut out: Vec<AccountSnapshot> = vec![];
    for entry in entries.flatten() {
        let path = entry.path();
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        if !name.ends_with(".account.json") {
            continue; // .last/ 在子目录里，天然不会命中
        }
        match fs::read_to_string(&path)
            .map_err(|e| e.to_string())
            .and_then(|s| serde_json::from_str::<AccountSnapshot>(&s).map_err(|e| e.to_string()))
        {
            Ok(s) => out.push(s),
            Err(e) => eprintln!("[zbar-accounts] 跳过损坏的快照文件 {name}: {e}"),
        }
    }
    out.sort_by_key(|s| s.created_at);
    out
}

/// 快照元信息列表（复用 load_snapshots_at，按创建时间升序）。
fn load_meta_list_at(base: &Path) -> Vec<AccountMeta> {
    load_snapshots_at(base)
        .iter()
        .map(AccountMeta::from)
        .collect()
}

/// 删除指定基目录下的快照文件。
fn remove_snapshot_at(base: &Path, id: &str) -> Result<(), String> {
    if !valid_id(id) {
        return Err("非法的账号 id".into());
    }
    let path = base.join("accounts").join(format!("{id}.account.json"));
    if !path.exists() {
        return Err("未找到该账号快照".into());
    }
    fs::remove_file(&path).map_err(|e| format!("删除快照失败: {e}"))
}

impl From<&AccountSnapshot> for AccountMeta {
    fn from(s: &AccountSnapshot) -> Self {
        AccountMeta {
            id: s.id.clone(),
            display_name: s.display_name.clone(),
            email: s.email.clone(),
            fingerprint: s.fingerprint.clone(),
            created_at: s.created_at,
            updated_at: s.updated_at,
            login_provider: s.login_provider.clone(),
            is_current: false, // 由 list_accounts 按实时指纹回填
        }
    }
}

// ============================================================
// 第二节：读取现场 / 捕获 / 切换事务（含回滚）
// ============================================================

/// 读取当前两文件原文（None = 确认不存在）。存在但读取失败（权限/IO 故障）
/// 必须返回 Err 中止切换：若误当 None 处理，回滚会按"切换前没有该文件"
/// 删除现场登录态。
fn read_live_files() -> Result<LiveFiles, String> {
    let read = |p: Result<std::path::PathBuf, String>, what: &str| -> Result<Option<String>, String> {
        let p = p.map_err(|e| format!("无法定位 {what} 路径（{e}）"))?;
        match fs::read_to_string(&p) {
            Ok(s) => Ok(Some(s)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(format!("读取 {what} 失败: {e}")),
        }
    };
    Ok(LiveFiles {
        credentials: read(credentials_path(), "credentials.json")?,
        config: read(zcode_config_path(), "config.json")?,
    })
}

/// 解析当前 credentials.json 并提取指纹（文件缺失/格式异常/解密失败均 None）。
/// pub(crate)：quota.rs 的 fetch_quota 写历史采样时需要当前账号指纹。
pub(crate) fn current_fingerprint() -> Option<Fingerprint> {
    let raw = fs::read_to_string(credentials_path().ok()?).ok()?;
    let v: Value = serde_json::from_str(&raw).ok()?;
    fingerprint_of_credentials(&v)
}

/// 从 config.json 原文提取 key 含 "coding-plan" 的 provider
/// （含 apiKey 为空的——是否登录由 credentials 决定，provider 定义保留完整）。
/// config 缺失/解析失败返回空 map，捕获不因此失败。
fn coding_plan_providers(config_raw: Option<&str>) -> Map<String, Value> {
    let mut out = Map::new();
    if let Some(raw) = config_raw {
        if let Ok(v) = serde_json::from_str::<Value>(raw) {
            if let Some(p) = v.get("provider").and_then(|v| v.as_object()) {
                for (k, val) in p {
                    if k.contains("coding-plan") {
                        out.insert(k.clone(), val.clone());
                    }
                }
            }
        }
    }
    out
}

/// 快照默认显示名：昵称 > 邮箱 > 账号-{id前8}。
fn default_display_name(fp: Option<&Fingerprint>, email: Option<&str>, id: &str) -> String {
    if let Some(name) = fp.and_then(|f| f.display_name.as_deref()) {
        if !name.is_empty() {
            return name.to_string();
        }
    }
    email
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .unwrap_or_else(|| format!("账号-{}", &id[..id.len().min(8)]))
}

/// upsert 内核的结果形态。
/// - Nothing：没有可处理的当前账号（credentials.json 缺失/非法 JSON，
///   或 update_only 下指纹无对应快照），由调用方决定报错还是静默跳过；
/// - Unchanged：现场有效内容与既有快照完全一致，幂等跳过未落盘
///   （仅 skip_unchanged=true 的自动捕获路径会出现，见 upsert_snapshot_at_opts）；
/// - Upserted：新建或刷新了快照（updated_existing 区分两种情形）。
#[derive(Debug)]
enum UpsertOutcome {
    Nothing,
    Unchanged,
    Upserted(CaptureOutcome),
}

/// 现场两文件原文 → 快照 upsert 核心（capture_account、"切换到其他账号前
/// 刷新被切走账号快照"、自动捕获轮询三方共用，编译期保证解析与 upsert
/// 语义一致）。base 为快照基目录（生产为 ~/.agenthud；单测注入临时目录）。
///
/// update_only=true 只刷新指纹命中的**已存在**快照（切换路径专用：不在切换
/// 路径产生新账号条目；指纹解密降级成 unknown-* 时自然落空，静默跳过）。
/// update_only=false 为常规捕获：无对应快照则新建（凭证键集异常导致指纹
/// 提取失败时，与升级前一致地退化为 unknown-* 快照，不阻塞捕获）。
///
/// skip_unchanged=true（自动捕获轮询专用）：旧快照存在且现场两文件的有效
/// 内容（凭证原文 + coding-plan providers + 选中 provider）与之完全一致时
/// 返回 Unchanged，不落盘、不刷新 updated_at——60s 一轮的轮询若每轮都
/// 原样重写，快照文件会被无谓刷写。指纹为 None 的轮次在该路径下直接
/// 返回 Nothing 不落盘（防 unknown 快照无限堆积，机理见函数体内注释），
/// unknown 快照只能由手动捕获产生。手动捕获与切换前刷新一律传 false，
/// 行为与短路机制引入前完全一致（手动捕获同内容仍正常返回成功）。
///
/// Err 只表示快照读取/落盘失败。
fn upsert_snapshot_at(
    base: &Path,
    live: &LiveFiles,
    update_only: bool,
) -> Result<Option<CaptureOutcome>, String> {
    match upsert_snapshot_at_opts(base, live, update_only, false)? {
        UpsertOutcome::Upserted(outcome) => Ok(Some(outcome)),
        // skip_unchanged=false 时 Unchanged 分支不可达，归入 None 保持旧签名语义
        _ => Ok(None),
    }
}

/// upsert_snapshot_at 的完整参数版内核（原签名函数是其 skip_unchanged=false
/// 的薄封装，既有调用点与测试零改动）。
fn upsert_snapshot_at_opts(
    base: &Path,
    live: &LiveFiles,
    update_only: bool,
    skip_unchanged: bool,
) -> Result<UpsertOutcome, String> {
    let raw = match live.credentials.as_deref() {
        Some(raw) => raw,
        None => return Ok(UpsertOutcome::Nothing),
    };
    let creds: Value = match serde_json::from_str(raw) {
        Ok(v) => v,
        // 非法 JSON：无法提取指纹（捕获路径另行报"格式异常"，切换路径跳过）
        Err(_) => return Ok(UpsertOutcome::Nothing),
    };
    let fp = fingerprint_of_credentials(&creds); // 解密失败降级 None，不阻塞
    // 自动捕获路径（skip_unchanged=true）的防堆积闸：指纹为 None 时本轮
    // 直接跳过、不落盘。机理：unknown 快照 id 含毫秒时间戳（见
    // snapshot_id_of），每轮都不同，下方按 id 查旧快照（load_snapshot_at）
    // 的幂等短路永远命中不了——现场是合法 JSON 却取不到指纹（登出残留
    // `{}`、解密失败、JWT 缺 user_id/sub 等）时，60s 一轮的自动捕获会
    // 每轮新建一个 unknown-* 快照并广播事件，一天堆积 1440 个文件、把
    // 账号列表灌满。因此 unknown 快照只能由手动捕获产生
    // （skip_unchanged=false，用户点击自然限频，既有语义不变）。切换路径
    // （update_only=true）不受影响：unknown id 在该路径下本就命中不了
    // 任何快照、只会静默 Nothing。
    if skip_unchanged && fp.is_none() {
        return Ok(UpsertOutcome::Nothing);
    }
    let id = snapshot_id_of(fp.as_ref());
    let fingerprint = fp.as_ref().map(|f| f.user_id.clone()).unwrap_or_default();
    let email = fp.as_ref().and_then(|f| f.email.clone());
    let providers = coding_plan_providers(live.config.as_deref());
    // 记录捕获时选中的 provider（与主面板 query_quota 同口径），额度查询
    // 优先按它取凭证，避免后续 config 混入其他账号 key 时张冠李戴
    let login_provider =
        crate::quota::pick_coding_plan_api_key(&providers).map(|(k, _, _)| k);

    let old = load_snapshot_at(base, &id);
    // 幂等短路（仅 skip_unchanged=true）：凭证原文一致则指纹必然一致（同一
    // 解析逻辑），无需另比；display_name/email 变化但凭证未变时同样跳过
    // （低频展示字段，交手动捕获刷新）
    if skip_unchanged {
        if let Some(old) = old.as_ref() {
            if old.credentials_raw == raw
                && old.config_providers == providers
                && old.login_provider == login_provider
            {
                return Ok(UpsertOutcome::Unchanged);
            }
        }
    }

    let now = now_ms();
    let (snapshot, updated_existing) = match old {
        Some(old) => (
            AccountSnapshot {
                version: 1,
                id: old.id,
                fingerprint, // 重捕获刷新（可能从 unknown 空指纹升级为真实指纹）
                // name_locked=true 表示用户手动重命名过，不能覆盖；否则按默认优先级刷新
                display_name: if old.name_locked {
                    old.display_name
                } else {
                    default_display_name(fp.as_ref(), email.as_deref(), &id)
                },
                email,
                created_at: old.created_at,
                updated_at: now,
                credentials_raw: raw.to_string(),
                config_providers: providers,
                login_provider,
                name_locked: old.name_locked,
            },
            true,
        ),
        // 只更新已存在的快照：不新建条目（切换路径不产生新账号）
        None if update_only => return Ok(UpsertOutcome::Nothing),
        None => {
            let display_name = default_display_name(fp.as_ref(), email.as_deref(), &id);
            (
                AccountSnapshot {
                    version: 1,
                    id,
                    fingerprint,
                    display_name,
                    email,
                    created_at: now,
                    updated_at: now,
                    credentials_raw: raw.to_string(),
                    config_providers: providers,
                    login_provider,
                    name_locked: false,
                },
                false,
            )
        }
    };
    let account = AccountMeta::from(&snapshot);
    save_snapshot_at(base, &snapshot)?;
    Ok(UpsertOutcome::Upserted(CaptureOutcome {
        account,
        updated_existing,
    }))
}

/// 捕获路径的"无当前账号可捕获"文案（与升级前逐字一致）。
/// 只在异常路径调用：为保住 upsert 核心单一入口而多解析一次凭证，代价可忽略。
fn capture_without_credentials_error(live: &LiveFiles) -> String {
    match live.credentials.as_deref() {
        Some(raw) => match serde_json::from_str::<Value>(raw) {
            Err(e) => format!("credentials.json 格式异常: {e}"),
            // 可解析的凭证必然产出快照（最差是 unknown-* 降级），正常到不了这里
            Ok(_) => "未找到 ZCode 登录凭证（ZCode 数据目录下 credentials.json 内容异常），请先在 ZCode 客户端登录后再捕获".into(),
        },
        None => "未找到 ZCode 登录凭证（ZCode 数据目录下 credentials.json 不存在），请先在 ZCode 客户端登录后再捕获".into(),
    }
}

/// 捕获当前登录：读两文件原文 → 提取指纹 → upsert 快照。
/// 重复捕获同一账号时保留原 created_at，只刷新凭证数据；
/// display_name 在旧快照未被手动重命名（name_locked=false）时才刷新。
pub fn capture_account() -> Result<CaptureOutcome, String> {
    let _guard = accounts_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());

    let live = read_live_files()?;
    match upsert_snapshot_at(&config_dir()?, &live, false)? {
        Some(outcome) => Ok(outcome),
        None => Err(capture_without_credentials_error(&live)),
    }
}

// ============================================================
// 第三节：ZCode 换账号自动捕获（后台轮询，应用运行期间常驻）
// ============================================================

/// 自动捕获事件名：后台轮询发现新登录（新建快照）或既有快照凭证内容
/// 真的变化时广播，前端账号卡片（SettingsPanel 的 AccountsCard）监听后
/// 刷新账号列表。payload 为 { name: 账号名, created: 是否新建 }。
const ACCOUNTS_AUTO_CAPTURED_EVENT: &str = "zbar://accounts-auto-captured";

/// 自动捕获单轮（spawn_auto_capture 的循环体）：读现场 → 持锁 upsert
/// （幂等短路）。返回 Some 表示捕获到变化（新建快照或凭证内容真的变化），
/// 需要广播事件；其余情况（凭证不存在/非法 JSON/取不到指纹/内容未变）
/// 静默返回 None。取不到指纹的轮次整轮跳过不落盘（防 unknown 快照
/// 无限堆积，见 upsert_snapshot_at_opts 体内注释）。
/// 持 accounts_lock 与手动捕获/切换事务/重命名互斥：切换写回现场后，
/// 本轮 upsert 同一快照幂等，不会与切换事务交错写快照目录。
fn auto_capture_once() -> Option<CaptureOutcome> {
    let _guard = accounts_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    // 路径定位/读现场失败（数据目录异常、权限/IO 故障）：环境问题静默跳过
    // 本轮，下一轮（60s 后）自动重试，绝不 panic、不阻塞任何调用方
    let base = config_dir().ok()?;
    let live = read_live_files().ok()?;
    match upsert_snapshot_at_opts(&base, &live, false, true) {
        Ok(UpsertOutcome::Upserted(outcome)) => Some(outcome),
        // 无凭证/非法 JSON/内容未变：正常路径，本轮无事可做
        Ok(_) => None,
        // 快照落盘失败：低频日志 + 下一轮重试，不让线程退出
        Err(e) => {
            eprintln!("[zbar-accounts] 自动捕获落盘失败（下轮重试）: {e}");
            None
        }
    }
}

/// 自动捕获一轮的门控入口：开关关闭时**在读取任何现场文件之前**直接
/// 返回 None（本轮零读零写，与「凭证不存在」的静默跳过同形）；开启时
/// 行为与开关引入前逐字一致（直接进 auto_capture_once）。
/// 开关由调用方每周期读一次（线程循环里读文件，60s 一次开销可忽略），
/// 因此设置页改开关后下一周期即生效，无需重启。
fn auto_capture_step(enabled: bool) -> Option<CaptureOutcome> {
    if !enabled {
        return None;
    }
    auto_capture_once()
}

/// 后台线程：应用运行期间自动捕获 ZCode 现场登录——用户在 ZCode 客户端
/// 换账号登录后无需手动点「捕获当前登录」。每 60s 一轮，首周期延迟 30s
/// 错开启动高峰；幂等短路保证无变化轮零写盘（updated_at 不动）；只有
/// 新建快照或既有快照凭证内容真的变化时才广播 accounts-auto-captured
/// 事件（打开着的设置页立即看到新账号）。线程随进程退出自然终止。
/// 每周期读一次自动捕获开关（load_app_settings，文件极小）：关掉后
/// 跳过捕获（auto_capture_step 短路），重新开启即时恢复——线程不退出、
/// 不需要重启应用。
pub fn spawn_auto_capture(app: AppHandle) {
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_secs(30));
        loop {
            if let Some(outcome) = auto_capture_step(auto_capture_enabled()) {
                let _ = app.emit(
                    ACCOUNTS_AUTO_CAPTURED_EVENT,
                    serde_json::json!({
                        "name": outcome.account.display_name,
                        "created": !outcome.updated_existing,
                    }),
                );
            }
            std::thread::sleep(std::time::Duration::from_secs(60));
        }
    });
}

// ============================================================
// 第四节：应用级设置（~/.agenthud/app-settings.json，主窗口设置页读写）
// ============================================================

/// 应用设置。当前只有 ZCode 换账号自动捕获开关；后续应用级开关并入本
/// 文件（不另开新文件），读写照 hud-bar.json 的原子写先例（临时文件 +
/// rename，避免半截 JSON 让下次读取整文件失配）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppSettings {
    /// 自动捕获开关（默认开）。关闭后 spawn_auto_capture 线程每周期跳过
    /// 捕获；文件缺失/损坏回退默认开（见 load_app_settings_at）。
    #[serde(default = "default_auto_capture")]
    pub auto_capture: bool,
}

fn default_auto_capture() -> bool {
    true
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            auto_capture: default_auto_capture(),
        }
    }
}

fn app_settings_path() -> Result<PathBuf, String> {
    Ok(config_dir()?.join("app-settings.json"))
}

/// 读取应用设置（路径注入版，单测用 temp 路径）：文件不存在或损坏一律
/// 回退默认值（自动捕获 = 开）——读取失败绝不能把开关静默关掉。
fn load_app_settings_at(path: &Path) -> AppSettings {
    fs::read_to_string(path)
        .ok()
        .and_then(|text| serde_json::from_str::<AppSettings>(&text).ok())
        .unwrap_or_default()
}

/// 保存应用设置（路径注入版）：原子写（先写临时文件再改名）。
fn save_app_settings_at(path: &Path, settings: &AppSettings) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).map_err(|e| format!("创建配置目录失败: {e}"))?;
    }
    let json = serde_json::to_string_pretty(settings)
        .map_err(|e| format!("序列化应用设置失败: {e}"))?;
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, json).map_err(|e| format!("写入应用设置失败: {e}"))?;
    fs::rename(&tmp, path).map_err(|e| format!("保存应用设置失败: {e}"))
}

/// 读取应用设置（真实路径）。
pub fn load_app_settings() -> AppSettings {
    app_settings_path()
        .map(|path| load_app_settings_at(&path))
        .unwrap_or_default()
}

/// 保存应用设置（真实路径，原子写）。调用方（set_auto_capture 命令）
/// 自行保证「先 load 最新 → 只覆盖目标字段 → 整体写回」。
pub fn save_app_settings_raw(settings: &AppSettings) -> Result<(), String> {
    save_app_settings_at(&app_settings_path()?, settings)
}

/// 自动捕获开关当前值（spawn_auto_capture 每周期调用一次）。
pub fn auto_capture_enabled() -> bool {
    load_app_settings().auto_capture
}

/// 列出全部快照 + 实时推断当前登录账号。
pub fn list_accounts() -> Result<AccountsState, String> {
    let base = config_dir()?;
    let mut accounts = load_meta_list_at(&base);
    let current = current_fingerprint().map(|fp| {
        let matched = accounts
            .iter()
            .find(|a| a.fingerprint == fp.user_id)
            .map(|a| a.id.clone());
        CurrentAccount {
            fingerprint: fp.user_id,
            email: fp.email,
            matched_snapshot_id: matched,
        }
    });
    // is_current 只按实时指纹与快照指纹的匹配回填
    if let Some(mid) = current.as_ref().and_then(|c| c.matched_snapshot_id.clone()) {
        for a in accounts.iter_mut() {
            a.is_current = a.id == mid;
        }
    }
    Ok(AccountsState { current, accounts })
}

/// 切换到指定快照账号。事务顺序 = 先退出桌面应用后写入；
/// 任一步失败回滚到切换前状态（详见模块头注释）。
/// expect_fingerprint：调用方（无人值守自动切换）在发起时观察到的当前登录
/// 指纹；持锁读出现场后若与之不符，说明等待锁期间登录态已被其他切换改变
/// （如用户手动切换），直接取消，避免把用户刚切好的账号再切走。
pub fn switch_account(
    id: &str,
    expect_fingerprint: Option<&str>,
) -> Result<SwitchOutcome, String> {
    let _guard = accounts_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());

    // 1. 读目标快照（失败零改动）
    let snapshot =
        load_snapshot_at(&config_dir()?, id).ok_or_else(|| "未找到该账号快照".to_string())?;

    // 2. 记录当前两文件现场（存在但读取失败时中止，零改动）
    let live =
        read_live_files().map_err(|e| format!("读取当前登录态失败（{e}），已取消切换"))?;

    // 2.5 前提校验（零改动）：提供了期望指纹而现场不符（含读不出当前指纹）时
    // 取消。注意错误文案是前端识别"自动切换被取消"的约定，勿改动。
    if let Some(expect) = expect_fingerprint {
        let actual = live.credentials.as_deref().and_then(|raw| {
            serde_json::from_str::<Value>(raw)
                .ok()
                .and_then(|v| fingerprint_of_credentials(&v).map(|fp| fp.user_id))
        });
        if actual.as_deref() != Some(expect) {
            return Err("登录态已变化，已取消本次自动切换".into());
        }
    }

    // 3. 备份到 .last/（失败零改动）
    backup_live(&live).map_err(|e| format!("备份失败（{e}），已取消切换"))?;

    // 4. 当前登录已是目标账号 → 拒绝（零改动）
    if let Some(raw) = &live.credentials {
        if let Ok(v) = serde_json::from_str::<Value>(raw) {
            if let Some(fp) = fingerprint_of_credentials(&v) {
                if fp.user_id == snapshot.fingerprint {
                    return Err("该账号已是当前登录账号".into());
                }
            }
        }
    }

    // 4.5 刷新"即将被切走的当前账号"的已存在快照（不新建条目，见
    // upsert_snapshot_at）。目的：让被切走账号的快照不停留在过期键集上
    // （ZCode 升级新增的 account-provider: 套餐键等）。凭证缺失/不可解析/
    // 指纹无对应快照时静默跳过，不阻断切换；只有快照落盘失败才中止——本步在
    // 退出 ZCode 之前，失败时 ~/.zcode 零改动，符合零改动契约。
    if let Err(e) = upsert_snapshot_at(&config_dir()?, &live, true) {
        return Err(format!("刷新当前账号快照失败（{e}），已取消切换"));
    }

    // 5. 退出 ZCode 桌面应用（未运行直接跳过；失败零改动）
    quit_zcode().map_err(|e| format!("{e}，已取消切换"))?;

    // 6. credentials.json 合并写回：以快照原文为基础，保留现场新增/更新的
    // account-provider: 套餐凭证键（原样整串回写会抹掉 ZCode 3.12+ 的套餐键，
    // 导致 ZCode 端套餐失效）；解析失败时自动降级为原文整串回写
    let merged_credentials =
        merge_credentials_with_live(&snapshot.credentials_raw, live.credentials.as_deref());
    if let Err(e) = atomic_write(&credentials_path()?, &merged_credentials) {
        return rollback(&live, format!("写入 credentials.json 失败（{e}）"));
    }

    // 7. config.json 合并写：coding-plan provider 逐 key 覆盖，其余配置保留
    let config_write = match &live.config {
        Some(raw) => match serde_json::from_str::<Value>(raw) {
            Ok(mut v) => {
                let providers = v.get_mut("provider").and_then(|p| p.as_object_mut());
                match providers {
                    Some(p) => {
                        for (k, val) in &snapshot.config_providers {
                            p.insert(k.clone(), val.clone());
                        }
                    }
                    None => {
                        v["provider"] = Value::Object(snapshot.config_providers.clone());
                    }
                }
                serde_json::to_string_pretty(&v)
                    .map_err(|e| format!("序列化 config.json 失败: {e}"))
                    .and_then(|s| atomic_write(&zcode_config_path()?, &s))
            }
            Err(e) => Err(format!("现有 config.json 无法解析: {e}")),
        },
        None => serde_json::to_string_pretty(&serde_json::json!({
            "provider": snapshot.config_providers
        }))
        .map_err(|e| format!("序列化 config.json 失败: {e}"))
        .and_then(|s| atomic_write(&zcode_config_path()?, &s)),
    };
    if let Err(e) = config_write {
        return rollback(&live, format!("写入 config.json 失败（{e}）"));
    }

    // 8. 重读校验：两文件必须都是合法 JSON；credentials.json 还必须包含快照
    // 原文解析出的全部键（合并写回不得丢键），否则回滚。
    // 快照原文解析失败时跳过附加校验（与合并逻辑的降级保护同口径）。
    let snapshot_keys: Option<Vec<String>> =
        serde_json::from_str::<Value>(&snapshot.credentials_raw)
            .ok()
            .and_then(|v| v.as_object().map(|o| o.keys().cloned().collect()));
    for (name, path) in [
        ("credentials.json", credentials_path()?),
        ("config.json", zcode_config_path()?),
    ] {
        let parsed = match fs::read_to_string(&path) {
            Ok(raw) => match serde_json::from_str::<Value>(&raw) {
                Ok(v) => v,
                Err(_) => {
                    return rollback(
                        &live,
                        format!("写入后校验 {name} 失败（内容不是合法 JSON）"),
                    );
                }
            },
            Err(_) => {
                return rollback(&live, format!("写入后校验 {name} 失败（内容不是合法 JSON）"));
            }
        };
        if name == "credentials.json" {
            if let (Some(expected), Some(obj)) = (&snapshot_keys, parsed.as_object()) {
                if let Some(missing) = expected.iter().find(|k| !obj.contains_key(k.as_str())) {
                    return rollback(
                        &live,
                        format!("写入后校验 credentials.json 失败（丢失快照键 {missing}）"),
                    );
                }
            }
        }
    }

    // 9. 重启 ZCode（失败不算切换失败，仅提示手动打开）
    let relaunched = launch_zcode();
    Ok(SwitchOutcome {
        switched_to: snapshot.display_name,
        zcode_relaunched: relaunched,
    })
}

/// credentials.json 中账号级套餐凭证键的公共前缀（ZCode 3.12+ 新增形态：
/// `account-provider:coding-plan:<providerId>:account:<identity>:api-key`，
/// 本机实测形如 `account-provider:coding-plan:account:bigmodel-individual-coding-plan:account:29701779269890662:api-key`）。
/// ZCode 判定套餐资格（entitled）依赖这些键，且跨账号累积存在。
const ACCOUNT_PROVIDER_KEY_PREFIX: &str = "account-provider:";

/// credentials.json 写回内容：以快照原文为基础，合并现场新增/更新的
/// `account-provider:` 前缀键。
///
/// 动机：升级前捕获的老快照只有旧键集，原文整串回写会把 ZCode 3.12+ 写入的
/// 账号级套餐凭证键抹掉，ZCode 端随即 credential-failed（套餐无法识别）；
/// 而现场文件里这些键是跨账号累积的（含目标账号自己的键），合并写回即可保住
/// 套餐资格。其余键（zcodejwttoken / oauth:* 等身份键）一律以快照为准——
/// 切换本身靠它们生效，绝不能被现场覆盖。
///
/// 降级保护（只返回 String、不返回 Result）：现场不存在、或快照/现场任一侧
/// 不是合法 JSON 对象时，退回"快照原文整串回写"的原行为，绝不让合并逻辑
/// 使切换失败。键值是 `enc:v1:` 密文，只搬运不解密（跨平台 secret 不同，
/// 解密再加密只会破坏凭证）。
fn merge_credentials_with_live(snapshot_raw: &str, live_raw: Option<&str>) -> String {
    let Some(live_raw) = live_raw else {
        return snapshot_raw.to_string(); // 现场无文件：无可合并内容
    };
    let (Ok(Value::Object(mut snap)), Ok(Value::Object(live))) = (
        serde_json::from_str::<Value>(snapshot_raw),
        serde_json::from_str::<Value>(live_raw),
    ) else {
        return snapshot_raw.to_string(); // 任一侧解析失败：降级为原文整串回写
    };
    for (key, value) in live {
        // 快照无此键 → 插入；已有同名键 → 覆盖（现场值更新鲜）
        if key.starts_with(ACCOUNT_PROVIDER_KEY_PREFIX) {
            snap.insert(key, value);
        }
    }
    serde_json::to_string_pretty(&Value::Object(snap)).unwrap_or_else(|_| snapshot_raw.to_string())
}

/// 切换前备份两文件原文到 ~/.agenthud/accounts/.last/（更名前 ~/.zbar，启动自动迁移；tmp+rename+0600）。
/// 切换前不存在的文件会清掉 .last/ 中的旧备份，保证 .last/ 始终精确反映切换前现场。
fn backup_live(live: &LiveFiles) -> Result<(), String> {
    let dir = backup_dir()?;
    fs::create_dir_all(&dir).map_err(|e| format!("创建备份目录失败: {e}"))?;
    harden_dir(&dir);
    match &live.credentials {
        Some(raw) => atomic_write(&dir.join("credentials.json"), raw)?,
        None => {
            let _ = fs::remove_file(dir.join("credentials.json"));
        }
    }
    match &live.config {
        Some(raw) => atomic_write(&dir.join("config.json"), raw)?,
        None => {
            let _ = fs::remove_file(dir.join("config.json"));
        }
    }
    Ok(())
}

/// 回滚：把 .last/ 备份写回 ~/.zcode/v2/（切换前不存在的文件删除目标）。
/// 无论回滚成败都尽力重启 ZCode，让用户能立即看到/处理结果。
fn rollback(live: &LiveFiles, step: String) -> Result<SwitchOutcome, String> {
    let restore = restore_backup(live);
    let _ = launch_zcode();
    match restore {
        Ok(()) => Err(format!("切换失败（{step}），已回滚到切换前的登录状态")),
        Err(e) => {
            // 备份目录随数据目录迁移结果而定（~/.agenthud 或回退态的
            // ~/.zbar），动态取当前真实位置写进指引，与实际落盘一致
            let backup = backup_dir()
                .map(|p| p.to_string_lossy().into_owned())
                .unwrap_or_else(|_| "数据目录下的 accounts/.last".to_string());
            Err(format!(
                "切换失败（{step}），且回滚失败（{e}）：请手动将 {backup} 下的备份文件复制到 ZCode 数据目录（ZCode 设置中可见，默认 ~/.zcode/v2/），或重启 ZCode 重新登录"
            ))
        }
    }
}

fn restore_backup(live: &LiveFiles) -> Result<(), String> {
    let backup = backup_dir()?;
    match &live.credentials {
        Some(_) => {
            let raw = fs::read_to_string(backup.join("credentials.json"))
                .map_err(|e| format!("读取备份 credentials.json 失败: {e}"))?;
            atomic_write(&credentials_path()?, &raw)?;
        }
        None => {
            let _ = fs::remove_file(credentials_path()?);
        }
    }
    match &live.config {
        Some(_) => {
            let raw = fs::read_to_string(backup.join("config.json"))
                .map_err(|e| format!("读取备份 config.json 失败: {e}"))?;
            atomic_write(&zcode_config_path()?, &raw)?;
        }
        None => {
            let _ = fs::remove_file(zcode_config_path()?);
        }
    }
    Ok(())
}

/// 删除快照（只删本应用的快照文件，不影响 ZCode 当前登录）。
pub fn remove_account(id: &str) -> Result<(), String> {
    let _guard = accounts_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    remove_snapshot_at(&config_dir()?, id)
}

/// 重命名快照（只改 display_name，32 字上限由前端约束，后端再截断兜底）。
/// 改名后置 name_locked=true：后续重捕获不再用默认名覆盖手动命名。
pub fn rename_account(id: &str, new_name: &str) -> Result<AccountMeta, String> {
    let _guard = accounts_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let mut snap = load_snapshot_at(&config_dir()?, id)
        .ok_or_else(|| "未找到该账号快照".to_string())?;
    let name = new_name.trim().chars().take(32).collect::<String>();
    if name.is_empty() {
        return Err("账号名称不能为空".into());
    }
    snap.display_name = name;
    snap.name_locked = true;
    snap.updated_at = now_ms();
    let meta = AccountMeta::from(&snap);
    save_snapshot_at(&config_dir()?, &snap)?;
    Ok(meta)
}

// ============================================================
// 第四节：多账号额度查询（只读快照 + 并行 HTTP）
// ============================================================

/// account_quotas 返回：单个账号快照的订阅额度查询结果。
#[derive(Debug, Serialize)]
pub struct AccountQuotaEntry {
    pub id: String,
    pub display_name: String,
    pub email: Option<String>,
    /// 账号指纹（user_id，与 quota_history 快照的 account 同一标识）。
    /// 前端用它把"本机+远端合并计算的各账号今日增量"关联回本条目。
    pub fingerprint: String,
    /// 是否当前登录账号（按实时指纹与快照指纹匹配回填，与 list_accounts 同口径）
    pub is_current: bool,
    /// 额度查询结果（失败为 None，错误见 error）
    pub quota: Option<crate::quota::QuotaResult>,
    /// 该账号今日增量 (增量百分比, 今日采样数)（查询失败为 None）。
    /// 由本函数写入的带指纹采样累积计算，当前账号另由 fetch_quota
    /// 30s 一轮补采样，数据比非当前账号更新。
    pub today_delta: Option<(u32, u32)>,
    /// 查询失败原因（quota 为 Some 时为 None；错误串不含 token，原样透传）
    pub error: Option<String>,
}

/// 快照的额度查询凭证，优先级从高到低（每级失败一律静默降级，新路径绝不报错）：
/// 1. 快照自带 credentials_raw 里的账号级套餐凭证键（ZCode 3.12+）。这是唯一与
///    "该快照身份"严格同源的来源：3.12+ 起 config.json 的 coding-plan apiKey
///    不再随账号登录刷新（本机实测停在另一账号的 key），而切换前刷新快照会把
///    现场 config 刷进被切走账号的快照，只用 config 会让多账号面板两条都显示
///    同一份额度；identity 传快照指纹（与键名 `:account:<identity>` 同源）精确
///    命中本账号的键；
/// 2. 捕获时记录的 login_provider（切换事务逐 key 覆盖不清理，live config/快照
///    可能混入其他账号的同前缀 key，固定序 pick 会错取）；
/// 3. 固定序 pick（老快照无 login_provider 也走这里）。
///
/// allow_config_fallback=false（本机存在 ≥2 个账号快照时由 account_quotas 传入）
/// 只走第 1 级，第 1 级落空直接 None：第 2/3 级读的是快照自身 config_providers，
/// 但上面已说明它无法与快照身份精确对应（切换前刷新会刷进现场 config、3.12+
/// 起 apiKey 不随登录刷新），多账号下取到别的账号的键就会让额度面板张冠李戴
/// （Pro 显示成 Max、未使用账号出现"今日增量"），且错配结果会以本账号指纹
/// 写入 quota_history 并经远端同步传播。单快照（单账号用户）传 true，行为与
/// 收紧前完全一致。
///
/// base_url：第 1 级与 pick_from_config 第 1 级同口径（取 coding-plan
/// provider 的 options.baseURL），但**只从快照自身的 config_providers 取**——
/// 快照查询与当前登录态无关，绝不读现场 config.json；取不到给空串由
/// base_from_provider_url 兜底。第 2/3 级的 base_url 随选中 provider 返回。
fn snapshot_credential(
    snap: &AccountSnapshot,
    allow_config_fallback: bool,
) -> Option<(String, String, String)> {
    // 第 1 级：快照自己的凭证原文（老快照/跨平台搬运时密文解不开属正常，
    // 单账号下静默回退 config_providers；多账号下无精确归属即报"无凭证"）
    if let Some((credential_key, api_key)) = snapshot_plan_credential(snap) {
        return Some((credential_key, api_key, snapshot_base_url(snap)));
    }
    if !allow_config_fallback {
        return None;
    }
    // 第 2 级：捕获时记录的 login_provider
    if let Some(key) = &snap.login_provider {
        if let Some((api_key, base_url)) = snap
            .config_providers
            .get(key)
            .and_then(crate::quota::provider_credential)
        {
            return Some((key.clone(), api_key, base_url));
        }
    }
    // 第 3 级：固定序 pick
    crate::quota::pick_coding_plan_api_key(&snap.config_providers)
}

/// snapshot_credential 第 1 级：从快照自带的 credentials_raw 中挑选该账号的
/// 账号级套餐凭证键（挑选口径与 quota.rs 的 pick_from_config 第 1 级一致），
/// identity 用快照指纹（快照 fingerprint 即该账号 user_id，与键名
/// `:account:<identity>` 同源）。任何一步失败——credentials_raw 非法 JSON/
/// 非对象、解密失败（跨平台 secret 不同）、无匹配键、指纹不命中的多候选
/// 歧义——一律静默 None，由调用方按 allow_config_fallback 决定是否回退
/// config_providers 路径。
fn snapshot_plan_credential(snap: &AccountSnapshot) -> Option<(String, String)> {
    let creds: Value = serde_json::from_str(&snap.credentials_raw).ok()?;
    let secret = crate::zcode_crypto::credential_secret();
    crate::quota::pick_account_provider_plan_key(&creds, Some(&snap.fingerprint), &secret)
}

/// snapshot_credential 第 1 级的 base_url：只从快照自身的 config_providers 取
/// （选中口径沿用 login_provider → 固定序 pick，与第 2/3 级一致），绝不读现场
/// config.json——同一份快照在任何登录态下查询都得到相同端点。取不到给空串，
/// 由 base_from_provider_url 兜底默认端点（bigmodel 系 = open.bigmodel.cn）。
fn snapshot_base_url(snap: &AccountSnapshot) -> String {
    if let Some(key) = &snap.login_provider {
        if let Some((_, base_url)) = snap
            .config_providers
            .get(key)
            .and_then(crate::quota::provider_credential)
        {
            return base_url;
        }
    }
    crate::quota::pick_coding_plan_api_key(&snap.config_providers)
        .map(|(_, _, base_url)| base_url)
        .unwrap_or_default()
}

/// 查询全部账号快照各自的订阅额度。
///
/// 数据来源是各快照捕获时保存的 Coding Plan provider（含 apiKey/baseURL），
/// 与当前登录无关——切换账号后仍能查到其他账号的额度。
/// 凭证必须能与快照身份精确对应：多账号时不走无法判定归属的 config 回退，
/// 落空的账号直接报"无可用 Coding Plan 凭证"，绝不猜（见 snapshot_credential）。
/// 只读快照目录与实时指纹，全程不持 ACCOUNTS_LOCK（与切换/捕获写事务互不阻塞）；
/// 各账号 HTTP 用 std::thread::scope 并行（单账号 15s 超时独立计时，不叠加）。
///
/// 每个查询成功的账号同时写一条带指纹的额度历史采样（quota_history）：
/// 快照的 account 字段使读写按账号隔离，非当前账号由此获得自己的
/// "今日增量"数据源（5 分钟一轮），且不会污染当前账号的任何读路径。
pub fn account_quotas() -> Result<Vec<AccountQuotaEntry>, String> {
    let base = config_dir()?;
    let snapshots = load_snapshots_at(&base);
    let current_fp = current_fingerprint().map(|fp| fp.user_id);

    // 先在主线程提取元数据与凭证（纯内存解析），线程内只做 HTTP 查询与采样落盘
    let metas: Vec<_> = snapshots
        .iter()
        .map(|snap| {
            (
                snap.id.clone(),
                snap.display_name.clone(),
                snap.email.clone(),
                snap.fingerprint.clone(),
            )
        })
        .collect();
    // 指纹与凭证打包进线程：查询成功后写采样需要知道归属账号。
    // 多账号（≥2 个快照）时禁用 config_providers 回退（见 snapshot_credential）：
    // 该路径无法与快照身份精确对应，宁可提示"无凭证"也不查成别的账号。
    let allow_config_fallback = snapshots.len() < 2;
    let jobs: Vec<_> = snapshots
        .iter()
        .map(|snap| {
            (
                snap.fingerprint.clone(),
                snapshot_credential(snap, allow_config_fallback),
            )
        })
        .collect();

    let results: Vec<_> = std::thread::scope(|scope| {
        let handles: Vec<_> = jobs
            .into_iter()
            .map(|(fingerprint, cred)| {
                scope.spawn(move || match cred {
                    // 错误文案刻意不以「未找到 ZCode Coding Plan 凭证」开头：
                    // 该前缀被前端识别为"当前账号未登录"的引导分支，而这里是
                    // "该快照缺凭证"的账号级提示，语义不同，绝不能混用
                    None => (
                        None,
                        Some(
                            "该账号快照无可用 Coding Plan 凭证，请切换到该账号后重新捕获"
                                .to_string(),
                        ),
                    ),
                    Some((_provider_key, token, base_url)) => {
                        match crate::quota::query_quota_with(&token, &base_url) {
                            Ok(q) => {
                                // 写带指纹采样（静默失败，不影响额度查询本身）；
                                // 与 fetch_quota 的 30s 采样经 (ts, account) 防抖去重。
                                // 指纹为空说明快照本身没有身份，同样不写（不可归属
                                // 的采样只会在服务端变成幽灵分组）。
                                if !fingerprint.is_empty() {
                                    let snap = crate::quota::snapshot_of(&q, Some(&fingerprint));
                                    crate::quota_history::append_snapshot(&snap);
                                }
                                (Some(q), None)
                            }
                            // 失败原样透传（query_quota_with 的错误串不含 token）
                            Err(e) => (None, Some(e)),
                        }
                    }
                })
            })
            .collect();
        handles
            .into_iter()
            // 查询线程内无 unwrap/panic 路径，join 失败仅为兜底
            .map(|h| {
                h.join()
                    .unwrap_or((None, Some("额度查询线程异常退出".to_string())))
            })
            .collect()
    });

    // 全部采样落盘后统一计算各账号今日增量（一次历史文件读取）
    let accounts: Vec<String> = metas.iter().map(|m| m.3.clone()).collect();
    let deltas = crate::quota_history::today_deltas(&accounts).unwrap_or_default();

    Ok(metas
        .into_iter()
        .zip(results)
        .map(
            |((id, display_name, email, fingerprint), (quota, error))| AccountQuotaEntry {
                id,
                display_name,
                email,
                fingerprint: fingerprint.clone(),
                is_current: Some(fingerprint.as_str()) == current_fp.as_deref(),
                today_delta: if quota.is_some() {
                    deltas.get(&fingerprint).copied()
                } else {
                    None
                },
                quota,
                error,
            },
        )
        .collect())
}

// ============================================================
// 第五节：ZCode 桌面应用进程控制（macOS / Windows）
// ============================================================

/// 桌面应用可执行名/进程名（macOS）。
/// pub(crate)：agent_theme 的 AgentApp::launch（open -a 启动）复用。
#[cfg(target_os = "macos")]
pub(crate) const ZCODE_APP_NAME: &str = "ZCode";

/// ZCode 桌面应用是否在运行（osascript 查询 LaunchServices，与 open -a
/// 同源）。stdout trim 后等于 "true" 才算运行中；命令执行失败按原语义
/// 返回 false。
/// 历史教训（勿回退为 pgrep）：真实 ZCode 主进程 argv[0]="ZCode"、是
/// launchd 直接子进程，macOS pgrep 的任何匹配形式（-x / 子串 / -f 全量
/// 命令行）都看不到它（pgrep 数据源与 ps comm 不一致的典型案例），用
/// pgrep -x 会永远误判"未运行"——restart_zcode 因此只 open -a 聚焦不
/// 重启、安装流程"② 退出目标应用"被整体跳过。osascript
/// `application "ZCode" is running` 走 LaunchServices，实测对运行中的
/// ZCode 返回 "true"，可靠。
/// 不加 TTL 缓存：全部调用点为低频用户操作（状态查询/安装/卸载/重启）
/// 或退出、启动等待循环（250ms 间隔，循环本身即节流），且等待循环依赖
/// 实时结论——缓存会延迟退出/启动确认，与流程正确性冲突。
/// pub(crate)：agent_theme（动态壁纸注入）复用。
#[cfg(target_os = "macos")]
pub(crate) fn zcode_running() -> bool {
    let script = format!("application \"{}\" is running", ZCODE_APP_NAME);
    std::process::Command::new("osascript")
        .args(["-e", &script])
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim() == "true")
        .unwrap_or(false)
}

/// 退出 ZCode：osascript 优雅退出 → 轮询 5s → pkill 兜底 → 再轮询 3s。
/// osascript 自身失败（如未授予 Automation 权限）静默降级 pkill。
/// 未运行直接返回 Ok。
/// pub(crate)：agent_theme（动态壁纸注入）复用。
#[cfg(target_os = "macos")]
pub(crate) fn quit_zcode() -> Result<(), String> {
    if !zcode_running() {
        return Ok(());
    }
    let _ = std::process::Command::new("osascript")
        .args(["-e", &format!("quit app \"{}\"", ZCODE_APP_NAME)])
        .output();
    for _ in 0..20 {
        if !zcode_running() {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(250));
    }
    // pkill 与 pgrep 同数据源，大概率杀不到 ZCode 主进程（见
    // zcode_running 注释）——保留作最后手段，不强求命中；osascript
    // quit 主路径走 AppleScript 事件，可靠性高，正常到此步前已退出
    let _ = std::process::Command::new("pkill")
        .args(["-x", ZCODE_APP_NAME])
        .output();
    for _ in 0..12 {
        if !zcode_running() {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(250));
    }
    Err("无法退出 ZCode 桌面应用（可尝试手动退出后重试）".into())
}

/// 启动 ZCode 桌面应用（open -a）。返回是否成功。
/// pub(crate)：agent_theme（动态壁纸注入）复用。
#[cfg(target_os = "macos")]
pub(crate) fn launch_zcode() -> bool {
    std::process::Command::new("open")
        .args(["-a", ZCODE_APP_NAME])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

// ---------- Windows 实现 ----------
// 思路与 macOS 相同：优雅退出 → 轮询 → 强杀兜底 → 轮询。
// 本应用是 GUI 进程，直接 Command::new 拉起 tasklist/taskkill 会闪控制台
// 黑窗，统一走 CREATE_NO_WINDOW 的 run_hidden。

/// Windows 下的进程/镜像名（tasklist/taskkill 用，不带 .exe）。
#[cfg(windows)]
const ZCODE_EXE_NAME: &str = "ZCode";

/// exe 路径缓存文件（~/.agenthud/.zcode-exe-path）。ZCode 可装在任意盘符
/// （如 D:\app\ZCode），切换时趁进程存活捕获一次，之后 ZCode 未运行的
/// 切换也能自动重启。
/// pub(crate)：agent_theme 的 Windows 安装目录探测复用同一缓存，避免
/// 两个模块各自维护一套安装位置发现逻辑。
#[cfg(windows)]
pub(crate) fn zcode_exe_cache_path() -> Result<PathBuf, String> {
    Ok(config_dir()?.join(".zcode-exe-path"))
}

/// 将 exe 完整路径写入缓存。写失败静默（缓存是优化不是依赖）。
/// pub(crate)：agent_theme 皮肤页探测时的现场捕获复用（捕获时机从
/// "仅退出 ZCode 时"扩展到"缓存缺失且进程在运行时"）。
#[cfg(windows)]
pub(crate) fn cache_zcode_exe_path(exe: &Path) {
    if let Ok(cache) = zcode_exe_cache_path() {
        if let Some(parent) = cache.parent() {
            let _ = fs::create_dir_all(parent);
        }
        let _ = fs::write(&cache, exe.to_string_lossy().as_bytes());
    }
}

/// 静默执行外部命令（CREATE_NO_WINDOW，GUI 进程下不闪控制台黑窗）。
/// pub(crate)：lib.rs 的 show_notification（Windows toast 走 PowerShell）复用。
#[cfg(windows)]
pub(crate) fn run_hidden(program: &str, args: &[&str]) -> Option<std::process::Output> {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    std::process::Command::new(program)
        .args(args)
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .ok()
}

/// ZCode 桌面应用是否在运行（tasklist 按镜像名过滤；CSV + /NH 避开
/// 本地化表头，未命中时输出 "INFO: ..." 行也不含镜像名，不会误判）。
/// pub(crate)：agent_theme（动态壁纸注入）复用。
#[cfg(windows)]
pub(crate) fn zcode_running() -> bool {
    let filter = format!("IMAGENAME eq {ZCODE_EXE_NAME}.exe");
    match run_hidden("tasklist", &["/FI", &filter, "/FO", "CSV", "/NH"]) {
        Some(out) if out.status.success() => String::from_utf8_lossy(&out.stdout)
            .to_lowercase()
            .contains(&format!("{}.exe", ZCODE_EXE_NAME.to_lowercase())),
        _ => false,
    }
}

/// 捕获运行中 ZCode 进程的 exe 完整路径（PowerShell Get-Process；
/// 强制 UTF-8 输出——管道重定向下 5.1 默认按 OEM 代码页编码，中文安装
/// 路径会变乱码；失败返回 None，不阻塞退出流程）。
/// pub(crate)：agent_theme 的 Windows 安装目录探测复用（皮肤页探测时
/// 现场捕获，见 cache_or_captured_install_candidates）。
#[cfg(windows)]
pub(crate) fn capture_zcode_exe_path() -> Option<PathBuf> {
    let script = format!(
        "[Console]::OutputEncoding=[System.Text.Encoding]::UTF8; \
          (Get-Process -Name {ZCODE_EXE_NAME} -ErrorAction SilentlyContinue \
          | Where-Object {{ $_.Path }} | Select-Object -First 1 -ExpandProperty Path)"
    );
    let out = run_hidden("powershell", &["-NoProfile", "-Command", &script])?;
    let path = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if path.is_empty() {
        None
    } else {
        Some(PathBuf::from(path))
    }
}

/// 读取并校验缓存的 exe 路径：只信任指向 ZCode.exe 且真实存在的内容，
/// 防篡改后借本面板拉起任意程序。无效/缺失返回 None。
/// pub(crate)：agent_theme 的 Windows 安装目录探测复用（取父目录作为
/// 安装根候选），launch_zcode 亦复用，校验逻辑只此一处。
#[cfg(windows)]
pub(crate) fn zcode_exe_cached() -> Option<PathBuf> {
    let cache = zcode_exe_cache_path().ok()?;
    let s = fs::read_to_string(cache).ok()?;
    let s = s.trim();
    let exe_suffix = format!("{}.exe", ZCODE_EXE_NAME).to_lowercase();
    if s.is_empty() || !s.to_lowercase().ends_with(&exe_suffix) {
        return None;
    }
    let exe = PathBuf::from(s);
    exe.is_file().then_some(exe)
}

// ---------- ZCode 安装位置发现（自 agent_theme 下沉至此） ----------
// 下沉理由：ZCode 安装位置发现能力归到 accounts（exe 缓存机制所在地），
// launch_zcode 与 agent_theme 的安装根目录探测（windows_install_root_
// or_default）共用同一候选来源，依赖方向与既有的 agent_theme →
// accounts 一致。

/// Windows 安装根目录候选（按常见度排序）：
/// `%LOCALAPPDATA%\Programs\ZCode`（用户级安装，最常见）→
/// `%ProgramFiles%\ZCode` → `%ProgramFiles(x86)%\ZCode`（机器级安装；
/// 环境变量未设置时再兜底固定路径 C:\Program Files\ZCode 与
/// C:\Program Files (x86)\ZCode，覆盖 Windows 装在非 C 盘以外仍读得到
/// 环境变量的常规场景）。
#[cfg(windows)]
pub(crate) fn windows_install_candidates() -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Some(base) = std::env::var_os("LOCALAPPDATA") {
        out.push(PathBuf::from(base).join("Programs").join("ZCode"));
    }
    if let Some(base) = std::env::var_os("ProgramFiles") {
        out.push(PathBuf::from(base).join("ZCode"));
    }
    if let Some(base) = std::env::var_os("ProgramFiles(x86)") {
        out.push(PathBuf::from(base).join("ZCode"));
    }
    out.push(PathBuf::from(r"C:\Program Files\ZCode"));
    out.push(PathBuf::from(r"C:\Program Files (x86)\ZCode"));
    out
}

/// 从候选目录中选第一个含 `resources\app.asar` 的（Electron 应用安装根
/// 目录的判定特征）。纯函数：candidates 由 windows_install_candidates
/// 构造或测试注入，便于 cfg(windows) 单测构造假目录树。
#[cfg(windows)]
pub(crate) fn find_install_root(candidates: &[PathBuf]) -> Option<PathBuf> {
    candidates
        .iter()
        .find(|c| c.join("resources").join("app.asar").is_file())
        .cloned()
}

/// 判断注册表卸载表项是否属于 ZCode：DisplayName 或 UninstallString 含
/// 关键字（不区分大小写），双字段兜底——DisplayName 由安装器写入，不同
/// 版本/渠道措辞可能变化。纯函数，便于注入测试。
#[cfg(windows)]
pub(crate) fn is_zcode_uninstall_entry(
    display_name: Option<&str>,
    uninstall_string: Option<&str>,
) -> bool {
    const KEYWORD: &str = "zcode";
    display_name
        .map(|s| s.to_lowercase().contains(KEYWORD))
        .unwrap_or(false)
        || uninstall_string
            .map(|s| s.to_lowercase().contains(KEYWORD))
            .unwrap_or(false)
}

/// 从 UninstallString 提取安装目录（卸载 exe 所在目录）。electron-builder
/// 的 NSIS 卸载项实测 InstallLocation 为空、UninstallString 形如
/// `"D:\app\ZCode\Uninstall ZCode.exe" /allusers`（带引号路径 + 参数尾巴）。
/// 带引号时取第一对引号内的路径；无引号时按空格逐段拼接并以文件存在性
/// 消歧（防"Program Files"这类含空格目录被截断）。解析失败返回 None；
/// 提取结果不校验存在性，统一交 find_install_root 按 app.asar 判定。
/// 纯函数，便于注入测试。
#[cfg(windows)]
pub(crate) fn install_dir_from_uninstall_string(s: &str) -> Option<PathBuf> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    if let Some(rest) = s.strip_prefix('"') {
        let end = rest.find('"')?;
        return PathBuf::from(&rest[..end]).parent().map(|p| p.to_path_buf());
    }
    // 无引号：逐段拼接直到拼出一个真实存在的文件（未命中返回 None）
    let parts: Vec<&str> = s.split_whitespace().collect();
    for i in 1..=parts.len() {
        let candidate = parts[..i].join(" ");
        if Path::new(&candidate).is_file() {
            return PathBuf::from(candidate).parent().map(|p| p.to_path_buf());
        }
    }
    None
}

/// 二级候选：注册表卸载表项。NSIS/MSI 安装器无论 per-user 还是
/// per-machine 都会写 Uninstall 键，枚举 HKCU、HKLM、HKLM\WOW6432Node
/// 三处视图（WOW6432Node 覆盖 32 位安装器写入的兼容视图）。安装器未
/// 卸载干净时的残留项会在 find_install_root 的 app.asar 校验处自然
/// 落空；枚举/读值失败静默跳过，不影响后续固定候选。
#[cfg(windows)]
pub(crate) fn registry_install_candidates() -> Vec<PathBuf> {
    use winreg::enums::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_READ};
    use winreg::RegKey;

    let mut out = Vec::new();
    for hive in [HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE] {
        for sub in [
            r"Software\Microsoft\Windows\CurrentVersion\Uninstall",
            r"Software\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall",
        ] {
            let Ok(root) = RegKey::predef(hive).open_subkey_with_flags(sub, KEY_READ) else {
                continue;
            };
            for guid in root.enum_keys().flatten() {
                let Ok(entry) = root.open_subkey_with_flags(&guid, KEY_READ) else {
                    continue;
                };
                let display: Option<String> = entry.get_value("DisplayName").ok();
                let uninstall: Option<String> = entry.get_value("UninstallString").ok();
                if !is_zcode_uninstall_entry(display.as_deref(), uninstall.as_deref()) {
                    continue;
                }
                // InstallLocation 有值且目录真实存在时直接采信（部分安装器会写）
                let install_loc: Option<String> = entry.get_value("InstallLocation").ok();
                if let Some(dir) = install_loc
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .map(PathBuf::from)
                    .filter(|p| p.is_dir())
                {
                    out.push(dir);
                    continue;
                }
                if let Some(dir) = uninstall.as_deref().and_then(install_dir_from_uninstall_string)
                {
                    out.push(dir);
                }
            }
        }
    }
    out
}

/// ZCode.exe 启动候选（去重保序）：缓存 exe → 注册表卸载表项（经
/// roots_with_asar 判定过滤）→ 固定候选表（含 x86），全部映射为
/// <根>\ZCode.exe。与安装预检（agent_theme 的 windows_install_root_
/// or_default）刻意同序——预检按"缓存/现场捕获 → 注册表 → 固定"探测，
/// 多 ZCode 安装并存且缓存缺失时启动若不同序，会"注入 A 安装、拉起
/// B 安装"。缓存命中时跳过注册表枚举（exe 已通过存在性校验，仅固定
/// 候选零开销兜底，与预检"缓存命中零注册表开销"同口径）；缓存缺失
/// 才注册表 + 固定全量探测。
#[cfg(windows)]
pub(crate) fn zcode_exe_candidates() -> Vec<PathBuf> {
    let cached = zcode_exe_cached();
    let roots = if cached.is_some() {
        windows_install_candidates()
    } else {
        let mut roots = roots_with_asar(&registry_install_candidates());
        roots.extend(windows_install_candidates());
        roots
    };
    zcode_exe_candidates_from(cached.as_deref(), &roots)
}

/// 按安装根判定特征（resources\app.asar 存在，与 find_install_root 同
/// 判据）过滤候选根：残留卸载表项可能指向"有 exe 无 asar"的坏目录，
/// 纳入启动候选前先剔除（固定候选维持原语义不过滤，它们本来就是虚拟
/// 兜底位置，由 launch 侧的 exe.is_file() 自然落空）。纯函数，便于
/// 注入假目录树测试。
#[cfg(windows)]
fn roots_with_asar(roots: &[PathBuf]) -> Vec<PathBuf> {
    roots
        .iter()
        .filter(|r| r.join("resources").join("app.asar").is_file())
        .cloned()
        .collect()
}

/// zcode_exe_candidates 的纯函数核心：cached 为完整 exe 路径、roots 为
/// 安装根列表，注入假数据即可单测映射 / 去重 / 保序。
#[cfg(windows)]
fn zcode_exe_candidates_from(cached: Option<&Path>, roots: &[PathBuf]) -> Vec<PathBuf> {
    let exe_name = format!("{ZCODE_EXE_NAME}.exe");
    let mut out: Vec<PathBuf> = Vec::new();
    for exe in cached
        .map(|exe| exe.to_path_buf())
        .into_iter()
        .chain(roots.iter().map(|root| root.join(&exe_name)))
    {
        // 候选总数个位数，线性去重足够（保序：只保留首次出现）
        if !out.contains(&exe) {
            out.push(exe);
        }
    }
    out
}

/// 退出 ZCode：先捕获并缓存 exe 路径（供重启用）→ taskkill 发送 WM_CLOSE
/// 优雅退出 → 轮询 5s → taskkill /F 强杀 → 再轮询 3s。未运行直接返回 Ok。
/// 只匹配 ZCode.exe 镜像，CLI 会话进程（node.exe）不受影响，与 macOS
/// pkill -x 的边界一致。
/// pub(crate)：agent_theme（动态壁纸注入）复用。
#[cfg(windows)]
pub(crate) fn quit_zcode() -> Result<(), String> {
    if !zcode_running() {
        return Ok(());
    }
    // 趁进程还活着记下安装位置；捕获失败不阻塞（重启退回常见路径探测）
    if let Some(exe) = capture_zcode_exe_path().filter(|p| p.is_file()) {
        cache_zcode_exe_path(&exe);
    }
    let image = format!("{ZCODE_EXE_NAME}.exe");
    let _ = run_hidden("taskkill", &["/IM", image.as_str()]);
    for _ in 0..20 {
        if !zcode_running() {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(250));
    }
    let _ = run_hidden("taskkill", &["/F", "/IM", image.as_str()]);
    for _ in 0..12 {
        if !zcode_running() {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(250));
    }
    Err("无法退出 ZCode 桌面应用（可尝试手动退出后重试）".into())
}

/// 启动 ZCode 桌面应用：按 zcode_exe_candidates 的候选顺序（缓存 exe →
/// 注册表卸载表项（经 asar 判定）→ 固定位置（含 x86）；缓存命中时免
/// 注册表枚举）逐个尝试，首个存在且 open::that_detached 启动成功的候选
/// 命中并回写 exe 缓存（此后含账号切换重启在内的所有调用面零探测开销）；
/// open 分离启动，不随本面板退出而终止。
/// pub(crate)：agent_theme（动态壁纸注入）复用。
#[cfg(windows)]
pub(crate) fn launch_zcode() -> bool {
    for exe in zcode_exe_candidates() {
        if exe.is_file() && open::that_detached(&exe).is_ok() {
            // 启动成功回写缓存：写失败静默（缓存是优化不是依赖）
            cache_zcode_exe_path(&exe);
            return true;
        }
    }
    false
}

// Linux：桌面端进程控制暂未实现，切换事务在 quit 环节直接报错引导手动操作。
#[cfg(target_os = "linux")]
fn quit_zcode() -> Result<(), String> {
    Err("当前平台暂不支持自动切换：请手动退出 ZCode 后重试".into())
}

#[cfg(target_os = "linux")]
fn launch_zcode() -> bool {
    false
}

// ============================================================
// 存储层单测（temp 目录，不触碰真实 ~/.agenthud 与 ~/.zcode）
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// 每个测试独立的临时基目录，结束时清理。
    struct TempBase(PathBuf);

    impl TempBase {
        fn new(tag: &str) -> Self {
            let dir = std::env::temp_dir().join(format!(
                "zbar-accounts-test-{tag}-{}-{}",
                std::process::id(),
                now_ms()
            ));
            fs::create_dir_all(&dir).expect("创建临时目录失败");
            TempBase(dir)
        }
    }

    impl Drop for TempBase {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn sample_snapshot(id: &str, name: &str) -> AccountSnapshot {
        AccountSnapshot {
            version: 1,
            id: id.to_string(),
            fingerprint: id.to_string(),
            display_name: name.to_string(),
            email: Some("a@b.c".into()),
            created_at: 1000,
            updated_at: 1000,
            credentials_raw: "{\"k\":\"v\"}".into(),
            config_providers: Map::new(),
            login_provider: None,
            name_locked: false,
        }
    }

    #[test]
    fn save_list_rename_remove_roundtrip() {
        let base = TempBase::new("roundtrip");
        save_snapshot_at(&base.0, &sample_snapshot("user1", "一号")).unwrap();
        let mut later = sample_snapshot("user2", "二号");
        later.created_at = 2000; // 保证排序确定（不同 created_at）
        save_snapshot_at(&base.0, &later).unwrap();

        let list = load_meta_list_at(&base.0);
        assert_eq!(list.len(), 2, "两个快照都应被列出");
        assert_eq!(list[0].id, "user1", "按 created_at 升序");

        // 加载单个
        let snap = load_snapshot_at(&base.0, "user1").expect("应能读回快照");
        assert_eq!(snap.display_name, "一号");
        assert_eq!(snap.credentials_raw, "{\"k\":\"v\"}");

        // 非法 id 直接 None（防路径穿越）
        assert!(load_snapshot_at(&base.0, "../etc").is_none());
        assert!(load_snapshot_at(&base.0, "a/b").is_none());

        // 重命名（复用生产 rename 的底层：load→改→save）
        let base_path = base.0.clone();
        let _guard = accounts_lock()
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let mut snap = load_snapshot_at(&base_path, "user1").unwrap();
        snap.display_name = "新名字".into();
        save_snapshot_at(&base_path, &snap).unwrap();
        assert_eq!(load_meta_list_at(&base_path)[0].display_name, "新名字");

        // 删除
        remove_snapshot_at(&base_path, "user1").unwrap();
        assert_eq!(load_meta_list_at(&base_path).len(), 1);
        // 再删报错
        assert!(remove_snapshot_at(&base_path, "user1").is_err());
    }

    #[test]
    fn corrupted_snapshot_is_skipped() {
        let base = TempBase::new("corrupt");
        save_snapshot_at(&base.0, &sample_snapshot("good", "好的")).unwrap();
        // 写一个损坏的快照文件
        let dir = base.0.join("accounts");
        fs::write(dir.join("broken.account.json"), "not a json").unwrap();
        // 非快照后缀的文件不参与扫描
        fs::write(dir.join("notes.txt"), "hello").unwrap();

        let list = load_meta_list_at(&base.0);
        assert_eq!(list.len(), 1, "损坏快照被跳过，正常快照保留");
        assert_eq!(list[0].id, "good");
    }

    #[test]
    fn permissions_are_hardened() {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let base = TempBase::new("perm");
            save_snapshot_at(&base.0, &sample_snapshot("u1", "n")).unwrap();

            let dir = base.0.join("accounts");
            let mode = fs::metadata(&dir).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o700, "快照目录应为 0700");

            let file = dir.join("u1.account.json");
            let mode = fs::metadata(&file).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600, "快照文件应为 0600");
        }
    }

    #[test]
    fn snapshot_id_filters_characters() {
        let fp = Fingerprint {
            user_id: "abc/DEF-123".into(),
            email: None,
            display_name: None,
        };
        assert_eq!(snapshot_id_of(Some(&fp)), "abcDEF-123");
        // 全非法字符退化为 unknown-
        let fp2 = Fingerprint {
            user_id: "/./".into(),
            email: None,
            display_name: None,
        };
        assert!(snapshot_id_of(Some(&fp2)).starts_with("unknown-"));
        assert!(snapshot_id_of(None).starts_with("unknown-"));
    }

    /// 快照默认名优先级：昵称 > 邮箱 > 账号-{id前8}
    #[test]
    fn default_display_name_priority() {
        let with_nick = Fingerprint {
            user_id: "u1".into(),
            email: Some("a@b.c".into()),
            display_name: Some("小智".into()),
        };
        assert_eq!(
            default_display_name(Some(&with_nick), Some("a@b.c"), "u1"),
            "小智"
        );
        let no_nick = Fingerprint {
            user_id: "u1".into(),
            email: Some("a@b.c".into()),
            display_name: None,
        };
        assert_eq!(default_display_name(Some(&no_nick), Some("a@b.c"), "u1"), "a@b.c");
        assert_eq!(default_display_name(None, None, "abcdefgh1234"), "账号-abcdefgh");
    }

    /// name_locked 行为：重捕获仅当旧快照未锁定时才刷新 display_name
    #[test]
    fn recapture_respects_name_locked() {
        // 未锁定：重捕获用新默认名覆盖（复用生产 capture 的 upsert 分支逻辑）
        let base = TempBase::new("relock-false");
        let mut old = sample_snapshot("user1", "旧名字");
        old.name_locked = false;
        save_snapshot_at(&base.0, &old).unwrap();

        let fp = Fingerprint {
            user_id: "user1".into(),
            email: Some("new@b.c".into()),
            display_name: Some("新昵称".into()),
        };
        let mut refreshed = old.clone();
        refreshed.display_name = default_display_name(Some(&fp), refreshed.email.as_deref(), "user1");
        refreshed.email = fp.email.clone();
        refreshed.updated_at = 2000;
        save_snapshot_at(&base.0, &refreshed).unwrap();
        let got = load_snapshot_at(&base.0, "user1").unwrap();
        assert_eq!(got.display_name, "新昵称", "未锁定时重捕获刷新默认名");
        assert_eq!(got.email.as_deref(), Some("new@b.c"), "email 照旧刷新");
        assert_eq!(got.created_at, 1000, "created_at 保留");

        // 已锁定：重捕获保留手动命名（email/fingerprint 等其余字段照旧刷新）
        let base2 = TempBase::new("relock-true");
        let mut locked = sample_snapshot("user2", "手动名");
        locked.name_locked = true;
        save_snapshot_at(&base2.0, &locked).unwrap();

        let fp2 = Fingerprint {
            user_id: "user2".into(),
            email: Some("new@b.c".into()),
            display_name: Some("新昵称".into()),
        };
        let mut refreshed2 = locked.clone();
        // 生产 capture 的分支：name_locked=true 直接沿用旧 display_name
        refreshed2.display_name = locked.display_name.clone();
        refreshed2.email = fp2.email.clone();
        refreshed2.fingerprint = fp2.user_id.clone();
        refreshed2.updated_at = 2000;
        save_snapshot_at(&base2.0, &refreshed2).unwrap();
        let got2 = load_snapshot_at(&base2.0, "user2").unwrap();
        assert_eq!(got2.display_name, "手动名", "锁定名不被重捕获覆盖");
        assert_eq!(got2.email.as_deref(), Some("new@b.c"), "锁定时 email 仍刷新");
        assert_eq!(got2.fingerprint, "user2");
    }

    /// 老快照（无 name_locked 字段）反序列化为 false，保持原兼容行为
    #[test]
    fn legacy_snapshot_without_name_lock_defaults_false() {
        let base = TempBase::new("legacy");
        let dir = base.0.join("accounts");
        fs::create_dir_all(&dir).unwrap();
        // 手写无 name_locked 字段的快照（模拟升级前的老文件）
        fs::write(
            dir.join("old.account.json"),
            r#"{
                "version": 1,
                "id": "old",
                "fingerprint": "old",
                "display_name": "老账号",
                "email": null,
                "created_at": 1000,
                "updated_at": 1000,
                "credentials_raw": "{}",
                "config_providers": {}
            }"#,
        )
        .unwrap();
        let snaps = load_snapshots_at(&base.0);
        assert_eq!(snaps.len(), 1);
        assert_eq!(snaps[0].name_locked, false, "老快照缺字段默认未锁定");
        assert_eq!(snaps[0].display_name, "老账号");
    }

    /// load_snapshots_at 返回完整快照（含 credentials_raw / config_providers）并按 created_at 升序
    #[test]
    fn load_snapshots_returns_full_content_sorted() {
        let base = TempBase::new("full-snaps");
        save_snapshot_at(&base.0, &sample_snapshot("user1", "一号")).unwrap();
        let mut later = sample_snapshot("user2", "二号");
        later.created_at = 2000;
        later.credentials_raw = "{\"k\":\"v2\"}".into();
        save_snapshot_at(&base.0, &later).unwrap();

        let snaps = load_snapshots_at(&base.0);
        assert_eq!(snaps.len(), 2);
        assert_eq!(snaps[0].id, "user1", "按 created_at 升序");
        assert_eq!(snaps[0].credentials_raw, "{\"k\":\"v\"}");
        assert_eq!(snaps[1].credentials_raw, "{\"k\":\"v2\"}");
    }

    #[test]
    fn coding_plan_providers_filters_keys() {
        let config = r#"{
            "provider": {
                "builtin:bigmodel": {"apiKey": ""},
                "builtin:bigmodel-coding-plan": {"apiKey": "k1"},
                "builtin:zai-coding-plan": {"apiKey": ""},
                "uuid-custom": {"apiKey": "k2"}
            }
        }"#;
        let providers = coding_plan_providers(Some(config));
        assert_eq!(providers.len(), 2, "只保留 key 含 coding-plan 的 provider");
        assert!(providers.contains_key("builtin:bigmodel-coding-plan"));
        assert!(providers.contains_key("builtin:zai-coding-plan"));
        // apiKey 为空的 coding-plan 也保留
        assert_eq!(
            providers.get("builtin:zai-coding-plan").unwrap().get("apiKey"),
            Some(&serde_json::json!(""))
        );
        // 缺失/坏格式 → 空 map
        assert!(coding_plan_providers(None).is_empty());
        assert!(coding_plan_providers(Some("bad json")).is_empty());
    }

    /// 快照凭证选择：login_provider 优先——即使混入的其他账号 key 在固定序里
    /// 更靠前，也必须用捕获时记录的那个；字段缺失或其 apiKey 失效则回退固定序。
    /// 本用例的 credentials_raw 不含账号级套餐凭证键（`{"k":"v"}`），
    /// snapshot_credential 第 1 级落空，验证的正是第 2/3 级（config_providers）
    /// 语义——故按单账号传 allow_config_fallback=true。
    #[test]
    fn snapshot_credential_prefers_login_provider() {
        let mut snap = sample_snapshot("u1", "一号");
        snap.config_providers = serde_json::from_str(
            r#"{
                "builtin:bigmodel-coding-plan": {
                    "options": {"apiKey": "keyA-other-account", "baseURL": ""}
                },
                "builtin:zai-coding-plan": {
                    "options": {"apiKey": "keyB-this-account", "baseURL": "https://api.z.ai/api/anthropic"}
                }
            }"#,
        )
        .unwrap();

        // 无 login_provider（老快照）→ 固定序选 bigmodel
        let got = snapshot_credential(&snap, true).unwrap();
        assert_eq!(got.0, "builtin:bigmodel-coding-plan");
        assert_eq!(got.1, "keyA-other-account");

        // 记录了 zai → 必须用 zai 的凭证（bigmodel 虽在固定序优先也不得命中）
        snap.login_provider = Some("builtin:zai-coding-plan".into());
        let got = snapshot_credential(&snap, true).unwrap();
        assert_eq!(got.0, "builtin:zai-coding-plan");
        assert_eq!(got.1, "keyB-this-account");
        assert_eq!(got.2, "https://api.z.ai/api/anthropic");

        // login_provider 对应 key 的 apiKey 变为空 → 回退固定序
        snap.config_providers
            .get_mut("builtin:zai-coding-plan")
            .unwrap()["options"]["apiKey"] = serde_json::json!("");
        let got = snapshot_credential(&snap, true).unwrap();
        assert_eq!(got.0, "builtin:bigmodel-coding-plan");
    }

    /// 快照的 login_provider → config_providers（含 baseURL），模拟"切换前刷新"
    /// 后的现场：被切走账号的快照里可能存着其他账号的 apiKey。
    fn snapshot_with_providers(id: &str) -> AccountSnapshot {
        let mut snap = sample_snapshot(id, "一号");
        snap.config_providers = serde_json::from_str(
            r#"{
                "builtin:bigmodel-coding-plan": {
                    "options": {
                        "apiKey": "config-key-of-other-account",
                        "baseURL": "https://open.bigmodel.cn/api/anthropic"
                    }
                },
                "builtin:zai-coding-plan": {
                    "options": {
                        "apiKey": "config-key-zai",
                        "baseURL": "https://api.z.ai/api/anthropic"
                    }
                }
            }"#,
        )
        .unwrap();
        snap
    }

    /// 第 1 级（最高优先级）：快照自带 credentials_raw 的账号级套餐凭证键，
    /// identity 按快照指纹精确命中本账号——同一份凭证原文里混有外账号的键
    /// （ZCode 跨账号累积是常态）时，必须取本账号的 key，绝不取 config 的。
    /// 测试值用明文（decrypt_value 对非 enc:v1: 值原样放行），无需真实 secret。
    #[test]
    fn snapshot_credential_prefers_own_account_plan_key() {
        let mut snap = snapshot_with_providers("111");
        snap.login_provider = Some("builtin:bigmodel-coding-plan".into());
        // 键序在 JSON 解析后不可依赖：两个账号各一个键，本账号（111）不在首位
        snap.credentials_raw = serde_json::json!({
            "zcodejwttoken": "jwt",
            "account-provider:coding-plan:account:bigmodel-team-coding-plan:account:222:api-key": "plan-key-222",
            "account-provider:coding-plan:account:bigmodel-individual-coding-plan:account:111:api-key": "plan-key-111"
        })
        .to_string();

        let got = snapshot_credential(&snap, true).unwrap();
        assert_eq!(got.1, "plan-key-111", "必须命中快照指纹（111）的凭证");
        assert!(
            got.0.ends_with(":account:111:api-key"),
            "第 1 级返回 credentials.json 原始键名（诊断用），实得 {}",
            got.0
        );
        assert_eq!(
            got.2, "https://open.bigmodel.cn/api/anthropic",
            "base_url 取自快照自身 config_providers 的 login_provider"
        );
    }

    /// 第 1 级无匹配（老快照的 credentials_raw 没有新形态键）→ 单账号
    /// （allow_config_fallback=true）静默回退第 2/3 级，login_provider 优先
    /// 与固定序语义不变，base_url 随选中 provider 返回
    #[test]
    fn snapshot_credential_falls_back_without_plan_keys() {
        let mut snap = snapshot_with_providers("111");
        snap.login_provider = Some("builtin:zai-coding-plan".into());
        // 老版本 ZCode 的凭证原文：只有身份键，没有账号级套餐凭证键
        snap.credentials_raw = r#"{"zcodejwttoken":"jwt"}"#.into();

        let got = snapshot_credential(&snap, true).unwrap();
        assert_eq!(got.0, "builtin:zai-coding-plan");
        assert_eq!(got.1, "config-key-zai");
        assert_eq!(got.2, "https://api.z.ai/api/anthropic");
    }

    /// 第 1 级候选键存在但密文解不开（跨平台搬运 secret 不同）→ 静默跳过并回退
    /// config_providers，绝不把坏值或错误串抛给调用方
    #[test]
    fn snapshot_credential_skips_undecryptable_plan_key() {
        let mut snap = snapshot_with_providers("111");
        snap.login_provider = Some("builtin:zai-coding-plan".into());
        snap.credentials_raw = serde_json::json!({
            // 段数正确但认证必失败的密文
            "account-provider:coding-plan:account:bigmodel-coding-plan:account:111:api-key": "enc:v1:AAAA.BBBB.CCCC"
        })
        .to_string();

        let got = snapshot_credential(&snap, true).unwrap();
        assert_eq!(got.0, "builtin:zai-coding-plan", "解不开时回退 config_providers");
        assert_eq!(got.1, "config-key-zai");
    }

    /// 第 1 级的 credentials_raw 非法 JSON / 非对象 → 同样静默回退
    /// （老快照损坏或异常形态不得让额度面板整条报错）
    #[test]
    fn snapshot_credential_falls_back_on_bad_credentials_raw() {
        for raw in ["not json", "[1, 2]", ""] {
            let mut snap = snapshot_with_providers("111");
            snap.login_provider = Some("builtin:zai-coding-plan".into());
            snap.credentials_raw = raw.into();
            let got = snapshot_credential(&snap, true)
                .unwrap_or_else(|| panic!("credentials_raw={raw:?} 时应回退 config_providers"));
            assert_eq!(got.1, "config-key-zai", "credentials_raw={raw:?}");
        }
        // 第 1/2/3 级全落空（config_providers 无可用凭证）→ None 契约不变
        let mut empty = sample_snapshot("111", "一号");
        empty.credentials_raw = "not json".into();
        assert!(snapshot_credential(&empty, true).is_none());
    }

    /// 多账号（本机 ≥2 个快照）收紧：第 1 级落空时禁止 config 回退——快照的
    /// config_providers/login_provider 无法与快照身份精确对应（切换前刷新会把
    /// 现场 config 刷进被切走账号的快照），即使里面有可用凭证也一律 None，交由
    /// account_quotas 提示"请切换到该账号后重新捕获"（宁缺毋错，绝不查成别的
    /// 账号）；同一份快照按单账号（true）仍保持原三级回退语义
    #[test]
    fn snapshot_credential_multi_account_skips_config_fallback() {
        let mut snap = snapshot_with_providers("111");
        snap.login_provider = Some("builtin:zai-coding-plan".into());
        // 老快照：凭证原文里没有账号级套餐凭证键，config_providers 里却有可用凭证
        snap.credentials_raw = r#"{"zcodejwttoken":"jwt"}"#.into();

        assert!(
            snapshot_credential(&snap, false).is_none(),
            "多账号时不得回退 config_providers（该凭证可能属于其他账号）"
        );
        // 单账号（老行为）语义不变：回退到 login_provider 指定的 config 凭证
        assert_eq!(snapshot_credential(&snap, true).unwrap().1, "config-key-zai");

        // 凭证原文损坏（非 JSON）同样不回退 config，只报"无凭证"
        snap.credentials_raw = "not json".into();
        assert!(snapshot_credential(&snap, false).is_none());
    }

    /// 多账号收紧不影响第 1 级：快照自带 credentials_raw 里被快照指纹精确命中的
    /// 账号级套餐凭证键照常可用（base_url 仍取自快照自身 config_providers，
    /// allow_config_fallback 开关对命中的第 1 级无影响）
    #[test]
    fn snapshot_credential_multi_account_uses_matching_plan_key() {
        let mut snap = snapshot_with_providers("111");
        snap.login_provider = Some("builtin:bigmodel-coding-plan".into());
        // 同一份凭证原文里混有外账号（222）的键，本账号（111）键不在首位
        snap.credentials_raw = serde_json::json!({
            "zcodejwttoken": "jwt",
            "account-provider:coding-plan:account:bigmodel-team-coding-plan:account:222:api-key": "plan-key-222",
            "account-provider:coding-plan:account:bigmodel-individual-coding-plan:account:111:api-key": "plan-key-111"
        })
        .to_string();

        let got = snapshot_credential(&snap, false).unwrap();
        assert_eq!(got.1, "plan-key-111", "必须命中快照指纹（111）的凭证");
        assert!(
            got.0.ends_with(":account:111:api-key"),
            "第 1 级返回 credentials.json 原始键名（诊断用），实得 {}",
            got.0
        );
        assert_eq!(got.2, "https://open.bigmodel.cn/api/anthropic");
        // 单账号路径同样优先第 1 级，两开关结果一致
        assert_eq!(snapshot_credential(&snap, true).unwrap().1, "plan-key-111");
    }

    /// 构造纯文本 JWT（decrypt_value 对非 enc:v1: 值原样放行，测试无需真实 secret）
    fn plaintext_jwt(user_id: &str) -> String {
        use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64;
        use base64::Engine as _;
        format!(
            "{}.{}.sig",
            B64.encode(br#"{"alg":"HS256"}"#),
            B64.encode(format!(r#"{{"user_id":"{user_id}"}}"#).as_bytes())
        )
    }

    /// 现场两文件（凭证含 user_id，config 含一个 coding-plan provider）
    fn live_for(user_id: &str) -> LiveFiles {
        LiveFiles {
            credentials: Some(format!(
                r#"{{"zcodejwttoken":"{}"}}"#,
                plaintext_jwt(user_id)
            )),
            config: Some(
                r#"{"provider":{"builtin:bigmodel-coding-plan":{"options":{"apiKey":"k1","baseURL":"https://open.bigmodel.cn/api/anthropic"}}}}"#
                    .into(),
            ),
        }
    }

    /// credentials.json 合并写回（merge_credentials_with_live）：
    /// 快照缺的新形态套餐键从现场补齐、同名键取现场（更新鲜）、
    /// 其余键一律以快照为准（身份键绝不能被现场覆盖）
    #[test]
    fn merge_credentials_fills_account_provider_keys() {
        let snapshot = r#"{
            "zcodejwttoken": "snap-jwt",
            "oauth:bigmodel:access_token": "snap-at",
            "account-provider:coding-plan:account:bigmodel-coding-plan:account:111:api-key": "snap-old"
        }"#;
        let live = r#"{
            "zcodejwttoken": "live-jwt",
            "oauth:bigmodel:access_token": "live-at",
            "account-provider:coding-plan:account:bigmodel-coding-plan:account:111:api-key": "live-fresh",
            "account-provider:coding-plan:account:bigmodel-coding-plan:account:222:api-key": "live-other-account"
        }"#;
        let merged: Value =
            serde_json::from_str(&merge_credentials_with_live(snapshot, Some(live))).unwrap();
        let obj = merged.as_object().unwrap();

        // 身份键以快照为准（切换靠它们生效）
        assert_eq!(obj.get("zcodejwttoken").unwrap(), "snap-jwt");
        assert_eq!(obj.get("oauth:bigmodel:access_token").unwrap(), "snap-at");
        // 快照缺的新形态键 → 从现场补齐（跨账号累积是 ZCode 常态，一律搬运）
        assert_eq!(
            obj.get("account-provider:coding-plan:account:bigmodel-coding-plan:account:222:api-key")
                .unwrap(),
            "live-other-account"
        );
        // 同名键 → 取现场值（更新鲜）
        assert_eq!(
            obj.get("account-provider:coding-plan:account:bigmodel-coding-plan:account:111:api-key")
                .unwrap(),
            "live-fresh"
        );
        // 快照原有的键一个都不能丢
        for key in serde_json::from_str::<Value>(snapshot)
            .unwrap()
            .as_object()
            .unwrap()
            .keys()
        {
            assert!(obj.contains_key(key), "快照键 {key} 不得丢失");
        }
    }

    /// 非 account-provider: 前缀的现场键（含现场新增的其他键）不得进入结果
    #[test]
    fn merge_credentials_keeps_snapshot_only_for_other_keys() {
        let snapshot = r#"{"zcodejwttoken":"snap","oauth:active_provider":"snap-p"}"#;
        let live = r#"{
            "zcodejwttoken": "live",
            "oauth:active_provider": "live-p",
            "web-remote-control:external-relay:pass_hash": "h",
            "brand-new-key": "v"
        }"#;
        let merged: Value =
            serde_json::from_str(&merge_credentials_with_live(snapshot, Some(live))).unwrap();
        let obj = merged.as_object().unwrap();
        assert_eq!(obj.len(), 2, "非 account-provider: 前缀键一律以快照为准");
        assert_eq!(obj.get("zcodejwttoken").unwrap(), "snap");
        assert_eq!(obj.get("oauth:active_provider").unwrap(), "snap-p");
    }

    /// 降级保护：现场缺失、现场/快照任一侧不是 JSON 对象 → 原文整串回写
    #[test]
    fn merge_credentials_falls_back_to_raw() {
        let snapshot = r#"{"zcodejwttoken":"snap-jwt"}"#;
        // 现场无文件（None）→ 原文
        assert_eq!(merge_credentials_with_live(snapshot, None), snapshot);
        // 现场不是合法 JSON / 不是对象 → 原文
        assert_eq!(
            merge_credentials_with_live(snapshot, Some("not json")),
            snapshot
        );
        assert_eq!(
            merge_credentials_with_live(snapshot, Some("[1,2]")),
            snapshot
        );
        // 快照自身不是合法 JSON 对象（老文件损坏）→ 原文，不因合并丢内容
        assert_eq!(
            merge_credentials_with_live("not json", Some(r#"{"a":"b"}"#)),
            "not json"
        );
    }

    /// 切换路径的 upsert（update_only=true）：只刷新指纹命中的已存在快照，
    /// 绝不新建条目；created_at / name_locked 语义与捕获一致
    #[test]
    fn upsert_update_only_never_creates_snapshot() {
        let base = TempBase::new("upsert-only");
        let live = live_for("u1");

        // 基目录下无 u1 快照 → Ok(None)，且不落任何文件（静默跳过）
        assert!(
            upsert_snapshot_at(&base.0, &live, true).unwrap().is_none(),
            "指纹无对应快照时应跳过"
        );
        let dir = base.0.join("accounts");
        assert_eq!(
            fs::read_dir(&dir).map(|e| e.count()).unwrap_or(0),
            0,
            "update_only 下不得新建任何快照文件"
        );

        // 已有 u1 快照（未锁定名）→ 只刷新内容，保留 created_at
        let mut old = sample_snapshot("u1", "旧名字");
        old.created_at = 1000;
        old.credentials_raw = r#"{"zcodejwttoken":"old"}"#.into();
        save_snapshot_at(&base.0, &old).unwrap();

        let outcome = upsert_snapshot_at(&base.0, &live, true)
            .unwrap()
            .expect("应命中已存在快照");
        assert!(outcome.updated_existing, "命中已存在快照");
        let got = load_snapshot_at(&base.0, "u1").unwrap();
        assert_eq!(
            got.credentials_raw,
            live.credentials.clone().unwrap(),
            "凭证刷新为现场原文"
        );
        assert_eq!(
            got.login_provider.as_deref(),
            Some("builtin:bigmodel-coding-plan"),
            "记录捕获时选中的 provider"
        );
        assert_eq!(got.created_at, 1000, "created_at 保留");
        assert_eq!(got.display_name, "账号-u1", "未锁定名时按默认优先级刷新");
        assert!(
            !got.config_providers.is_empty(),
            "config_providers 同步刷新"
        );
        assert_eq!(load_meta_list_at(&base.0).len(), 1, "不产生新条目");

        // 已锁定名 → 保留手动命名
        let mut locked = sample_snapshot("u1", "手动名");
        locked.name_locked = true;
        save_snapshot_at(&base.0, &locked).unwrap();
        upsert_snapshot_at(&base.0, &live, true).unwrap();
        assert_eq!(
            load_snapshot_at(&base.0, "u1").unwrap().display_name,
            "手动名",
            "name_locked=true 时不得覆盖 display_name"
        );
    }

    /// 捕获路径（update_only=false）：无对应快照时新建；凭证缺失/非法 JSON
    /// 时 Ok(None)，由调用方按升级前文案报错
    #[test]
    fn upsert_capture_creates_and_reports_missing_credentials() {
        let base = TempBase::new("upsert-create");
        let live = live_for("u2");
        let outcome = upsert_snapshot_at(&base.0, &live, false)
            .unwrap()
            .expect("捕获路径应新建快照");
        assert!(!outcome.updated_existing, "首次捕获是新条目");
        assert_eq!(outcome.account.id, "u2");
        assert_eq!(outcome.account.display_name, "账号-u2");

        // 凭证不存在 / 非法 JSON → Ok(None)（调用方报错）
        assert!(upsert_snapshot_at(
            &base.0,
            &LiveFiles {
                credentials: None,
                config: None
            },
            false
        )
        .unwrap()
        .is_none());
        assert!(upsert_snapshot_at(
            &base.0,
            &LiveFiles {
                credentials: Some("not json".into()),
                config: None
            },
            false
        )
        .unwrap()
        .is_none());

        // 升级前语义：凭证是合法 JSON 但取不到指纹（如数组）→ 仍建 unknown-* 快照
        let before = load_meta_list_at(&base.0).len();
        let odd = upsert_snapshot_at(
            &base.0,
            &LiveFiles {
                credentials: Some("[1,2]".into()),
                config: None
            },
            false,
        )
        .unwrap()
        .expect("可解析的凭证照旧产出快照");
        assert!(
            odd.account.id.starts_with("unknown-"),
            "取不到指纹时退化为 unknown-* 快照"
        );
        assert_eq!(
            load_meta_list_at(&base.0).len(),
            before + 1,
            "降级捕获只新增一个条目"
        );

        // 切换路径（update_only）下同一份凭证不命中任何快照 → 跳过，不留垃圾条目。
        // 必须用独立基目录：unknown-* 的 id 由毫秒时间戳生成，若与上面刚创建的
        // unknown 快照落在同一毫秒，本调用会命中并刷新它（返回 Some）——那属于
        // 生产语义的正常表现，却会让"前提是无快照可命中"的断言变成 flaky。
        let fresh = TempBase::new("upsert-only-odd");
        assert!(upsert_snapshot_at(
            &fresh.0,
            &LiveFiles {
                credentials: Some("[1,2]".into()),
                config: None
            },
            true
        )
        .unwrap()
        .is_none());
        // 不新增条目：目录不存在（无任何快照）即"零落盘"
        assert_eq!(
            fs::read_dir(fresh.0.join("accounts"))
                .map(|e| e.count())
                .unwrap_or(0),
            0,
            "update_only 下不得新建任何快照文件"
        );

        // 错误文案与升级前逐字一致（前端按此文案引导登录）
        assert_eq!(
            capture_without_credentials_error(&LiveFiles {
                credentials: None,
                config: None
            }),
            "未找到 ZCode 登录凭证（ZCode 数据目录下 credentials.json 不存在），请先在 ZCode 客户端登录后再捕获"
        );
        assert!(capture_without_credentials_error(&LiveFiles {
            credentials: Some("not json".into()),
            config: None
        })
        .starts_with("credentials.json 格式异常: "));
    }

    /// 自动捕获路径（skip_unchanged=true）的幂等短路：
    /// - 首轮新建快照；同内容第二轮 Unchanged 跳过，不落盘、updated_at 不变；
    /// - config 的 coding-plan provider 变化（凭证原文未变）时不得短路，照常刷新；
    /// - 手动捕获路径回归（skip_unchanged=false）：同内容仍正常返回成功并重写。
    #[test]
    fn auto_capture_skips_unchanged_and_manual_still_refreshes() {
        let base = TempBase::new("auto-skip");
        let live = live_for("u1");

        // 首轮：新建快照
        let first = match upsert_snapshot_at_opts(&base.0, &live, false, true).unwrap() {
            UpsertOutcome::Upserted(o) => o,
            other => panic!("首轮应新建快照，实得 {other:?}"),
        };
        assert!(!first.updated_existing, "首轮是新条目");
        assert_eq!(first.account.id, "u1");
        let created = load_snapshot_at(&base.0, "u1").unwrap();

        // 等毫秒时间戳推进，保证"是否重新落盘"可由 updated_at 区分
        std::thread::sleep(std::time::Duration::from_millis(3));

        // 同内容第二轮：幂等跳过，updated_at 保持首轮值（零写盘）
        assert!(
            matches!(
                upsert_snapshot_at_opts(&base.0, &live, false, true).unwrap(),
                UpsertOutcome::Unchanged
            ),
            "同内容轮询应短路跳过"
        );
        let after_skip = load_snapshot_at(&base.0, "u1").unwrap();
        assert_eq!(
            after_skip.updated_at, created.updated_at,
            "跳过轮不得刷新 updated_at"
        );
        assert_eq!(after_skip.credentials_raw, created.credentials_raw);

        // config 变化（apiKey 刷新）而凭证原文未变：不得短路，照常 upsert
        let mut live_config_changed = live.clone();
        live_config_changed.config = Some(
            r#"{"provider":{"builtin:bigmodel-coding-plan":{"options":{"apiKey":"k2","baseURL":"https://open.bigmodel.cn/api/anthropic"}}}}"#
                .into(),
        );
        let refreshed = match upsert_snapshot_at_opts(
            &base.0,
            &live_config_changed,
            false,
            true,
        )
        .unwrap()
        {
            UpsertOutcome::Upserted(o) => o,
            other => panic!("config 变化应照常 upsert，实得 {other:?}"),
        };
        assert!(refreshed.updated_existing, "命中已存在快照");
        let after_refresh = load_snapshot_at(&base.0, "u1").unwrap();
        assert!(
            after_refresh.updated_at > created.updated_at,
            "config 变化后应重新落盘"
        );
        assert_eq!(
            after_refresh
                .config_providers
                .get("builtin:bigmodel-coding-plan")
                .and_then(|p| p.get("options"))
                .and_then(|o| o.get("apiKey")),
            Some(&serde_json::json!("k2")),
            "config_providers 跟随现场刷新"
        );

        // 手动捕获路径回归（旧签名 skip_unchanged=false）：凭证/config 与快照
        // 一致仍正常返回成功（capture_account 不走短路）
        std::thread::sleep(std::time::Duration::from_millis(3));
        let manual = upsert_snapshot_at(&base.0, &live_config_changed, false)
            .unwrap()
            .expect("手动捕获同内容仍应返回成功");
        assert!(manual.updated_existing, "命中已存在快照");
        let after_manual = load_snapshot_at(&base.0, "u1").unwrap();
        assert!(
            after_manual.updated_at > after_refresh.updated_at,
            "手动捕获同内容仍应重新落盘刷新 updated_at"
        );
        assert_eq!(after_manual.created_at, created.created_at, "created_at 保留");
    }

    /// 现场换新指纹（用户在 ZCode 客户端换了账号登录）：自动捕获路径新建
    /// 快照、既有快照不受影响——「换账号自动捕获」的核心场景。
    #[test]
    fn auto_capture_new_fingerprint_creates_snapshot() {
        let base = TempBase::new("auto-new");
        // 先捕获 u1（模拟既有账号），现场换成 u2 登录
        let live1 = live_for("u1");
        upsert_snapshot_at(&base.0, &live1, false).unwrap();
        let live2 = live_for("u2");

        let outcome = match upsert_snapshot_at_opts(&base.0, &live2, false, true).unwrap() {
            UpsertOutcome::Upserted(o) => o,
            other => panic!("新指纹应新建快照，实得 {other:?}"),
        };
        assert!(!outcome.updated_existing, "新指纹是新条目");
        assert_eq!(outcome.account.id, "u2");
        assert_eq!(
            outcome.account.display_name, "账号-u2",
            "payload 携带的账号名按默认优先级生成"
        );

        // u1 保留、u2 新建；新快照凭证原文 = 现场 u2 原文
        let metas = load_meta_list_at(&base.0);
        assert_eq!(metas.len(), 2, "既有快照保留，新快照加入");
        let snap2 = load_snapshot_at(&base.0, "u2").unwrap();
        assert_eq!(snap2.credentials_raw, live2.credentials.unwrap());
    }

    /// 非法 JSON（ZCode 正在写文件/半截写入）：自动捕获路径静默跳过
    /// （Nothing，不落盘不报错），现场恢复合法内容后下一轮再捕获。
    #[test]
    fn auto_capture_invalid_json_skips_silently() {
        let base = TempBase::new("auto-badjson");
        let bad = LiveFiles {
            credentials: Some("not json".into()),
            config: None,
        };
        assert!(
            matches!(
                upsert_snapshot_at_opts(&base.0, &bad, false, true).unwrap(),
                UpsertOutcome::Nothing
            ),
            "非法 JSON 应静默跳过"
        );
        // 零落盘：连快照目录都不创建
        assert!(!base.0.join("accounts").exists());
    }

    /// P1 回归锚定：自动捕获路径对「合法 JSON 但取不到指纹」的现场
    /// （登出残留 `{}`、解密失败、JWT 缺 user_id/sub 等）必须整轮跳过。
    /// 机理：unknown-* 的 id 含毫秒时间戳、每轮不同，按 id 查旧快照的
    /// 幂等短路永远命中不了，若照常 upsert，60s 一轮会每轮新建一个
    /// unknown 快照（一天 1440 个）并广播事件。手动捕获同一输入仍产出
    /// unknown 快照（既有语义不变）。
    #[test]
    fn auto_capture_fingerprint_none_rounds_never_accumulate() {
        let no_fp = LiveFiles {
            credentials: Some("{}".into()), // 合法 JSON 登出残留：取不到指纹
            config: None,
        };

        // 全新基目录：连续多轮（模拟 60s 轮询）全部 Nothing、零落盘——
        // 连快照目录都不创建，文件数自然不增长
        let base = TempBase::new("auto-nofp");
        for round in 1..=5 {
            assert!(
                matches!(
                    upsert_snapshot_at_opts(&base.0, &no_fp, false, true).unwrap(),
                    UpsertOutcome::Nothing
                ),
                "第 {round} 轮：无指纹现场应整轮跳过"
            );
        }
        assert!(
            !base.0.join("accounts").exists(),
            "自动路径不得为无指纹现场创建任何快照文件"
        );

        // 已有快照的目录同样不增长：先正常捕获 u1，再让无指纹现场连续
        // 多轮，文件数保持不变
        let base2 = TempBase::new("auto-nofp-mixed");
        upsert_snapshot_at(&base2.0, &live_for("u1"), false).unwrap();
        let before = load_meta_list_at(&base2.0).len();
        for _ in 0..5 {
            assert!(matches!(
                upsert_snapshot_at_opts(&base2.0, &no_fp, false, true).unwrap(),
                UpsertOutcome::Nothing
            ));
        }
        assert_eq!(
            load_meta_list_at(&base2.0).len(),
            before,
            "无指纹轮次不得新增任何快照文件"
        );

        // 手动捕获回归：同一输入仍产出 unknown-* 快照（既有语义锚定）
        let manual = upsert_snapshot_at(&base.0, &no_fp, false)
            .unwrap()
            .expect("手动捕获无指纹凭证照旧产出快照");
        assert!(
            manual.account.id.starts_with("unknown-"),
            "手动路径保留 unknown 降级语义"
        );
        assert_eq!(load_meta_list_at(&base.0).len(), 1);

        // 切换路径（update_only=true）不受防堆积闸影响：同一输入照旧静默
        // 跳过（独立基目录，避免与上面手动创建的 unknown 快照同毫秒相撞）
        let fresh = TempBase::new("auto-nofp-switch");
        assert!(
            upsert_snapshot_at(&fresh.0, &no_fp, true).unwrap().is_none(),
            "切换路径对无指纹快照本就静默跳过"
        );
        assert!(!fresh.0.join("accounts").exists());
    }

    // ---- 应用设置（自动捕获开关）：路径注入版，temp 目录，不碰真实
    //      ~/.agenthud；开关「开」路径的捕获行为由上方 upsert 层既有测试
    //      覆盖（auto_capture_step(true) 只是原样转调 auto_capture_once）----

    /// 每个测试独立的临时文件路径，父目录随用随建、结束时清理。
    struct TempSettings(PathBuf);

    impl TempSettings {
        fn new(tag: &str) -> Self {
            let dir = std::env::temp_dir().join(format!(
                "zbar-appsettings-test-{tag}-{}-{}",
                std::process::id(),
                now_ms()
            ));
            fs::create_dir_all(&dir).expect("创建临时目录失败");
            TempSettings(dir.join("app-settings.json"))
        }
    }

    impl Drop for TempSettings {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(self.0.parent().unwrap());
        }
    }

    /// 默认值：文件缺失 / 损坏 / 非法 JSON 一律回退「自动捕获开」——
    /// 读取失败绝不能把开关静默关掉。
    #[test]
    fn 应用设置_缺失与损坏回退默认开() {
        let p = TempSettings::new("default");
        assert_eq!(load_app_settings_at(&p.0).auto_capture, true, "文件缺失默认开");
        fs::write(&p.0, "not json").unwrap();
        assert_eq!(load_app_settings_at(&p.0).auto_capture, true, "非法 JSON 默认开");
        // 半截字段（手改配置丢字段）：serde default 补开
        fs::write(&p.0, "{}").unwrap();
        assert_eq!(load_app_settings_at(&p.0).auto_capture, true, "缺字段默认开");
    }

    /// 落盘 roundtrip：关 → 读回关；重新开 → 读回开（原子写后内容完整）。
    #[test]
    fn 应用设置_开关落盘roundtrip() {
        let p = TempSettings::new("roundtrip");
        save_app_settings_at(&p.0, &AppSettings { auto_capture: false }).unwrap();
        assert_eq!(
            load_app_settings_at(&p.0).auto_capture,
            false,
            "关闭后读回应为关"
        );
        save_app_settings_at(&p.0, &AppSettings { auto_capture: true }).unwrap();
        assert_eq!(
            load_app_settings_at(&p.0).auto_capture,
            true,
            "重新开启读回应为开"
        );
        // 临时文件不残留（原子写以 rename 收尾）
        assert!(!p.0.with_extension("json.tmp").exists());
    }

    /// 门控短路：开关关闭时 auto_capture_step 在读取任何现场文件之前
    /// 直接返回 None（零读零写）——不依赖也不触碰 ZCode 数据目录，
    /// 因此在任意环境（含无凭证的 CI）都可稳定断言。
    /// 开启路径 = 原样转调 auto_capture_once，其 upsert 行为由上方
    /// auto_capture_* 系列测试覆盖。
    #[test]
    fn 自动捕获_开关关闭时整轮跳过() {
        assert!(
            auto_capture_step(false).is_none(),
            "开关关闭时本轮必须直接跳过"
        );
    }
}

// ============================================================
// Windows 专属：安装位置发现与 exe 启动候选的测试
// （自 agent_theme 下沉；非 Windows 平台不编译）
// ============================================================

#[cfg(all(windows, test))]
mod windows_tests {
    use super::*;

    /// 安装根目录探测纯函数（find_install_root）：
    /// - 候选按顺序命中第一个含 resources\app.asar 的目录；
    /// - 仅目录存在而无 asar 的候选不命中；全部未命中 / 空列表 → None。
    #[test]
    fn 安装根目录探测_选中含asar的候选() {
        let base = std::env::temp_dir().join(format!("zbar-winroot-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        // 候选一：目录与 resources 都在但没有 app.asar（未安装完/残留）
        let empty = base.join("empty-install");
        // 候选二：完整安装（resources\app.asar 存在）
        let real = base.join("real-install");
        fs::create_dir_all(empty.join("resources")).unwrap();
        fs::create_dir_all(real.join("resources")).unwrap();
        fs::write(real.join("resources").join("app.asar"), b"x").unwrap();

        let found =
            find_install_root(&[empty.clone(), real.clone()]).expect("应命中第二个候选");
        assert_eq!(found, real, "应跳过无 asar 的候选");
        assert_eq!(
            find_install_root(std::slice::from_ref(&empty)),
            None,
            "无 asar 不命中"
        );
        assert_eq!(find_install_root(&[]), None, "空候选列表不命中");

        let _ = fs::remove_dir_all(&base);
    }

    /// 卸载串提取安装目录（install_dir_from_uninstall_string）：
    /// 带引号路径 + 参数尾巴是 electron-builder NSIS 的实测形态；无引号
    /// 含空格路径靠文件存在性消歧（临时目录自建文件验证，不依赖系统文件）。
    #[test]
    fn 卸载串提取_带引号路径取引号内父目录() {
        // 本机注册表实测形态：带引号 + /allusers 参数尾巴
        assert_eq!(
            install_dir_from_uninstall_string(r#""D:\app\ZCode\Uninstall ZCode.exe" /allusers"#),
            Some(PathBuf::from(r"D:\app\ZCode"))
        );
        // 带引号无参数
        assert_eq!(
            install_dir_from_uninstall_string(r#""C:\Program Files\ZCode\Uninstall ZCode.exe""#),
            Some(PathBuf::from(r"C:\Program Files\ZCode"))
        );
        // 前后空白容忍
        assert_eq!(
            install_dir_from_uninstall_string(r#"  "D:\ZCode\Uninstall ZCode.exe"  "#),
            Some(PathBuf::from(r"D:\ZCode"))
        );
    }

    #[test]
    fn 卸载串提取_无引号含空格靠存在性消歧() {
        // 无引号 + 目录与文件名都含空格：逐段拼接在拼出完整文件路径时
        // 命中，父目录即安装目录（临时目录自建文件，不依赖系统文件）
        let base = std::env::temp_dir().join(format!("zbar-winsu-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        let install = base.join(r"my zcode");
        fs::create_dir_all(&install).unwrap();
        fs::write(install.join("Uninstall ZCode.exe"), b"x").unwrap();
        let s = install.join("Uninstall ZCode.exe");
        assert_eq!(
            install_dir_from_uninstall_string(&s.to_string_lossy()),
            Some(install),
            "含空格目录应拼出完整路径后命中"
        );
        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn 卸载串提取_畸形输入返回空() {
        assert_eq!(install_dir_from_uninstall_string(""), None);
        assert_eq!(install_dir_from_uninstall_string("   "), None);
        // 只有开引号（畸形串）
        assert_eq!(
            install_dir_from_uninstall_string(r#""D:\app\ZCode\Uninstall ZCode.exe"#),
            None
        );
        // 无引号且拼不出真实存在的文件（存在性消歧全部未命中）
        assert_eq!(
            install_dir_from_uninstall_string(r"Q:\no-such\dir\Uninstall ZCode.exe"),
            None
        );
    }

    /// 卸载表项归属判定（is_zcode_uninstall_entry）：DisplayName 或
    /// UninstallString 任一含关键字即命中，不区分大小写。
    #[test]
    fn 卸载表项判定_双字段关键字兜底() {
        assert!(is_zcode_uninstall_entry(Some("ZCode 3.10.1"), None));
        assert!(is_zcode_uninstall_entry(None, Some(r#""D:\app\ZCode\Uninstall ZCode.exe""#)));
        // 大小写不敏感
        assert!(is_zcode_uninstall_entry(Some("zcode desktop"), None));
        assert!(is_zcode_uninstall_entry(None, Some(r#""D:\APP\zcode\Uninstall ZCode.exe""#)));
        // 双字段皆空皆无关 → 不命中
        assert!(!is_zcode_uninstall_entry(None, None));
        assert!(!is_zcode_uninstall_entry(Some("Some Other App"), Some(r#"C:\Other\un.exe"#)));
    }

    /// 注册表候选的 asar 判定过滤（roots_with_asar，与 find_install_root
    /// 同判据）：残留卸载表项指向"有 exe 无 asar"的坏目录时剔除，
    /// 完整安装根保序保留；空列表 / 全部无效 → 空。
    #[test]
    fn 注册表候选过滤_要求asar存在() {
        let base =
            std::env::temp_dir().join(format!("zbar-asar-filter-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        let good1 = base.join("good1");
        let stale = base.join("stale"); // 有 exe 无 asar 的残留目录
        let good2 = base.join("good2");
        for d in [&good1, &stale, &good2] {
            fs::create_dir_all(d.join("resources")).unwrap();
        }
        fs::write(good1.join("resources").join("app.asar"), b"x").unwrap();
        fs::write(stale.join("ZCode.exe"), b"x").unwrap();
        fs::write(good2.join("resources").join("app.asar"), b"x").unwrap();

        assert_eq!(
            roots_with_asar(&[good1.clone(), stale.clone(), good2.clone()]),
            vec![good1, good2],
            "无 asar 的残留根应被剔除，其余保序保留"
        );
        assert!(roots_with_asar(&[]).is_empty());
        assert!(
            roots_with_asar(&[stale]).is_empty(),
            "仅有 exe 的残留根不应进入启动候选"
        );

        let _ = fs::remove_dir_all(&base);
    }

    /// exe 启动候选纯函数（zcode_exe_candidates_from）：缓存 exe 优先 →
    /// 各安装根映射为 <根>\ZCode.exe；与缓存同源的候选去重且保序
    /// （只保留首次出现）。
    #[test]
    fn exe启动候选_映射去重保序() {
        let cached = PathBuf::from(r"D:\app\ZCode\ZCode.exe");
        let roots = vec![
            PathBuf::from(r"C:\Users\u\AppData\Local\Programs\ZCode"),
            PathBuf::from(r"D:\app\ZCode"), // join 后与缓存 exe 重复
            PathBuf::from(r"C:\Program Files\ZCode"),
        ];
        assert_eq!(
            zcode_exe_candidates_from(Some(&cached), &roots),
            vec![
                PathBuf::from(r"D:\app\ZCode\ZCode.exe"),
                PathBuf::from(r"C:\Users\u\AppData\Local\Programs\ZCode\ZCode.exe"),
                PathBuf::from(r"C:\Program Files\ZCode\ZCode.exe"),
            ],
            "缓存优先、安装根映射为 ZCode.exe、重复只保留首次"
        );
        // 无缓存 → 直接从安装根映射
        assert_eq!(
            zcode_exe_candidates_from(None, &roots[..1]),
            vec![PathBuf::from(
                r"C:\Users\u\AppData\Local\Programs\ZCode\ZCode.exe"
            )]
        );
        // 无缓存且无根 → 空候选
        assert!(zcode_exe_candidates_from(None, &[]).is_empty());
    }

    /// 真实数据源薄封装（zcode_exe_candidates）：固定候选兜底保证非空，
    /// 全部候选都指向 ZCode.exe 且互不重复（注册表/环境变量只读探测，
    /// 枚举失败静默不影响兜底）。
    #[test]
    fn exe启动候选_全部指向zcodeexe且无重复() {
        let got = zcode_exe_candidates();
        assert!(!got.is_empty(), "固定候选兜底应保证至少一条");
        let mut seen = std::collections::HashSet::new();
        for exe in &got {
            assert!(
                exe.ends_with("ZCode.exe"),
                "候选应指向 ZCode.exe：{}",
                exe.display()
            );
            assert!(seen.insert(exe.clone()), "候选不应重复：{}", exe.display());
        }
    }
}

