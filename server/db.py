"""SQLite 数据库操作：自动建库建表 + 增删改查。

关键点（与 Rust 版一致）：
- 服务端数据库是自建自用的，首次启动 usage.db 不存在时，
  sqlite3.connect() 会自动创建文件 + 执行建表 SQL，对用户透明。
- 表结构、字段名与 Rust 版完全一致，保证客户端无需改动。
- model_usage 是 append-only，用 (device_id, local_rowid) 作主键去重。
- 新版支持多数据来源（source 维度）：'zcode'（ZCode 本地库）/ 'codex'
  （Codex CLI 导入库），主键改为 (device_id, source, local_rowid)。
  旧库首次启动自动迁移（见 _migrate_usage_records），老数据无损。
"""

import json
import math
import sqlite3
import threading
import time

from config import DB_PATH, DATA_DIR

# 线程锁：Flask 多线程模式下保护 sqlite 写入
_db_lock = threading.Lock()

# ===== 建表 SQL（与 Rust 版 schema.rs 完全一致）=====

SCHEMA_DEVICES = """
CREATE TABLE IF NOT EXISTS devices (
    device_id   TEXT    PRIMARY KEY,
    device_name TEXT    NOT NULL,
    token_hash  TEXT    NOT NULL,
    created_at  INTEGER NOT NULL
)
"""


def _usage_records_schema(table):
    """usage_records 建表 SQL。

    source 标记数据来源（'zcode' / 'codex'）：同一台设备、同一 local_rowid
    在不同 source 下互不冲突（两套 rowid 序列各自从 1 递增）。
    迁移时用同一份定义建 usage_records_new，保证新旧表结构严格一致。
    session_id / project_key / project_display 为 proto 5 新增的会话与
    项目维度列（全部可空，缺省 NULL）；旧库由 _migrate_usage_records_columns
    用 ALTER TABLE 原地补列，补列后列序与本定义一致（都在末尾）。
    duration_ms / time_to_first_token_ms 为 proto 8 新增的耗时/首字列
    （可空，缺省 NULL）：仅存储与回传，不参与聚合；旧库由
    _migrate_usage_records_speed_columns 同款 ALTER 原地补列。
    """
    return f"""
CREATE TABLE IF NOT EXISTS {table} (
    device_id                   TEXT    NOT NULL,
    source                      TEXT    NOT NULL DEFAULT 'zcode',
    local_rowid                 INTEGER NOT NULL,
    started_at                  INTEGER NOT NULL,
    model_id                    TEXT,
    provider_id                 TEXT,
    input_tokens                INTEGER NOT NULL DEFAULT 0,
    output_tokens               INTEGER NOT NULL DEFAULT 0,
    cache_read_input_tokens     INTEGER NOT NULL DEFAULT 0,
    cache_creation_input_tokens INTEGER NOT NULL DEFAULT 0,
    reasoning_tokens            INTEGER NOT NULL DEFAULT 0,
    computed_total_tokens       INTEGER NOT NULL DEFAULT 0,
    uploaded_at                 INTEGER NOT NULL,
    session_id                  TEXT,
    project_key                 TEXT,
    project_display             TEXT,
    duration_ms                 REAL,
    time_to_first_token_ms      REAL,
    PRIMARY KEY (device_id, source, local_rowid)
)
"""


SCHEMA_USAGE_RECORDS = _usage_records_schema("usage_records")

# 额度快照表：客户端每次查询额度后追加一条，周期解析靠 weekly_reset 跳变。
# 与客户端本地 quota_history.jsonl 字段对齐，多一个 device_id 用于按设备筛选。
# account 为快照归属账号指纹（客户端多账号采样隔离用）：旧客户端不传，
# 统一存空串；主键含 account，同设备多账号同毫秒落盘的多条采样互不撞键
# （旧主键 (device_id, ts) 会把其他账号的采样静默吞掉）。
SCHEMA_QUOTA_SNAPSHOTS = """
CREATE TABLE IF NOT EXISTS quota_snapshots (
    device_id    TEXT    NOT NULL,
    ts           INTEGER NOT NULL,
    account      TEXT    NOT NULL DEFAULT '',
    level        TEXT,
    weekly_pct   INTEGER NOT NULL DEFAULT 0,
    weekly_reset INTEGER,
    hour5_pct    INTEGER NOT NULL DEFAULT 0,
    mcp_pct      INTEGER NOT NULL DEFAULT 0,
    mcp_used     INTEGER,
    mcp_total    INTEGER,
    PRIMARY KEY (device_id, ts, account)
)
"""

# Agent 额度快照按窗口拆行保存，查询时再组装回客户端使用的 snapshot 形状。
# plan_type 为空时统一保存为空字符串；快照身份不含 plan_type，避免套餐标签
# 变化后同一来源/时间/窗口产生重复采样。
# proto 7（全量 provider 额度同步）扩展四列：
# - label：凭证备注名（同一 provider 多凭证的账号维度，进主键——同设备同
#   来源同毫秒的多凭证采样互不撞键；旧数据统一 ''，与旧主键去重一致）；
# - plan_name：套餐展示名（DeepSeek 等无 plan_type 概念的 provider 用）；
# - balance_json：按量计费余额（{amount, currency, granted?, topped_up?}）；
# - message：查询失败原因（status != ok 的错误态快照，供手机端展示）。
# 窗口为空的余额型/错误态快照落一行 window_key='' 的哨兵行（used_pct=0），
# 组装回快照时该行不进 windows 数组、只贡献 label/plan_name/balance/message。
SCHEMA_AGENT_QUOTA_SNAPSHOTS = """
CREATE TABLE IF NOT EXISTS agent_quota_snapshots (
    device_id    TEXT    NOT NULL,
    source       TEXT    NOT NULL,
    ts           INTEGER NOT NULL,
    label        TEXT    NOT NULL DEFAULT '',
    plan_type    TEXT    NOT NULL DEFAULT '',
    plan_name    TEXT,
    window_key   TEXT    NOT NULL,
    used_pct     REAL    NOT NULL DEFAULT 0,
    reset_at     INTEGER,
    balance_json TEXT,
    message      TEXT,
    PRIMARY KEY (device_id, source, ts, label, window_key)
)
"""

INDEX_STARTED = "CREATE INDEX IF NOT EXISTS idx_started ON usage_records(started_at)"
INDEX_DEVICE_STARTED = "CREATE INDEX IF NOT EXISTS idx_device_started ON usage_records(device_id, started_at)"
INDEX_SNAPSHOT_TS = "CREATE INDEX IF NOT EXISTS idx_snapshot_ts ON quota_snapshots(ts)"
INDEX_AGENT_QUOTA_SNAPSHOT_TS = (
    "CREATE INDEX IF NOT EXISTS idx_agent_quota_snapshot_ts "
    "ON agent_quota_snapshots(ts)"
)
INDEX_AGENT_QUOTA_SNAPSHOT_SOURCE_TS = (
    "CREATE INDEX IF NOT EXISTS idx_agent_quota_snapshot_source_ts "
    "ON agent_quota_snapshots(source, ts)"
)

AGENT_QUOTA_WINDOWS = {
    "codex": {"hour5", "weekly"},
    "claude": {"hour5", "weekly"},
    "cursor": {"cursor_auto", "cursor_api"},
    # Kimi Code（客户端 lib.rs 以 hour5 / weekly 两个窗口上传）：此前不在本表里，
    # 服务端会**静默丢弃**它的额度快照，导致多设备额度视图里 kimi 永远没有数据。
    # 窗口键与 codex/claude 同款，直接并进来即可。
    "kimi": {"hour5", "weekly"},
    # ===== proto 7：全量 provider 额度同步（窗口键 = 桌面 provider_quota.rs
    # 各模块归一后的 ProviderQuotaWindow.key，白名单与客户端
    # provider_quota_history.rs 的 PROVIDER_SNAPSHOT_SOURCES 保持一致）。
    # 窗口键集合为空 = 纯余额型 provider（无百分比窗口，快照只带 balance），
    # insert 侧按「windows 为空但 balance/plan_name/message 有值即落库」放行。
    "deepseek": set(),
    "moonshot": set(),
    "minimax": {"interval", "weekly"},
    "opencodego": {"hour5", "weekly", "monthly"},
    "alibaba": {"hour5", "weekly", "monthly"},
    "qoder": {"credits"},
    "longcat": {"quota", "fuel"},
    "mimo": {"credits"},
    "stepfun": {"sub_credits", "topup_credits"},
    "grok": {"monthly"},
    "alibabatoken": {"credits"},
}

SCHEMA_CONFIG = """
CREATE TABLE IF NOT EXISTS config (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
)
"""

ALL_SCHEMA = [
    SCHEMA_DEVICES,
    SCHEMA_USAGE_RECORDS,
    SCHEMA_QUOTA_SNAPSHOTS,
    SCHEMA_AGENT_QUOTA_SNAPSHOTS,
    INDEX_STARTED,
    INDEX_DEVICE_STARTED,
    INDEX_SNAPSHOT_TS,
    INDEX_AGENT_QUOTA_SNAPSHOT_TS,
    INDEX_AGENT_QUOTA_SNAPSHOT_SOURCE_TS,
    SCHEMA_CONFIG,
]


def _migrate_usage_records(conn):
    """usage_records 增加 source 维度的幂等自动迁移。

    检测现有表无 source 列时：建新表 → 旧数据全部按 source='zcode' 搬入 →
    删旧表 → 改名。整个过程单事务完成，老数据无损；已迁移/全新库直接跳过
    （可重复调用，幂等）。
    """
    cols = [r["name"] for r in conn.execute("PRAGMA table_info(usage_records)").fetchall()]
    if not cols or "source" in cols:
        return

    conn.isolation_level = None  # 手动管理事务，保证搬迁原子性
    try:
        conn.execute("BEGIN IMMEDIATE")
        conn.execute(_usage_records_schema("usage_records_new"))
        conn.execute(
            """
            INSERT INTO usage_records_new
                (device_id, source, local_rowid, started_at, model_id, provider_id,
                 input_tokens, output_tokens, cache_read_input_tokens,
                 cache_creation_input_tokens, reasoning_tokens,
                 computed_total_tokens, uploaded_at)
            SELECT device_id, 'zcode', local_rowid, started_at, model_id, provider_id,
                   input_tokens, output_tokens, cache_read_input_tokens,
                   cache_creation_input_tokens, reasoning_tokens,
                   computed_total_tokens, uploaded_at
            FROM usage_records
            """
        )
        conn.execute("DROP TABLE usage_records")
        conn.execute("ALTER TABLE usage_records_new RENAME TO usage_records")
        conn.execute("COMMIT")
        print("[zbar-sync] usage_records 已自动迁移：新增 source 维度，老数据无损保留")
    except Exception:
        conn.execute("ROLLBACK")
        raise
    finally:
        conn.isolation_level = ""


def _migrate_quota_snapshots(conn):
    """quota_snapshots 增加 account 维度的幂等自动迁移。

    检测现有表无 account 列时：建新表（主键扩为 device_id+ts+account）→
    旧数据全部按 account='' 搬入 → 删旧表 → 改名。单事务完成，老数据无损；
    已迁移/全新库直接跳过（可重复调用，幂等）。主键扩展后同设备多账号
    同毫秒的采样互不撞键；旧客户端（不上传 account）统一存 ''，
    与旧主键 (device_id, ts) 的去重行为一致。
    """
    cols = [r["name"] for r in conn.execute("PRAGMA table_info(quota_snapshots)").fetchall()]
    if not cols or "account" in cols:
        return

    conn.isolation_level = None  # 手动管理事务，保证搬迁原子性
    try:
        conn.execute("BEGIN IMMEDIATE")
        conn.execute(SCHEMA_QUOTA_SNAPSHOTS.replace(
            "IF NOT EXISTS quota_snapshots", "IF NOT EXISTS quota_snapshots_new"
        ))
        conn.execute(
            """
            INSERT INTO quota_snapshots_new
                (device_id, ts, account, level, weekly_pct, weekly_reset,
                 hour5_pct, mcp_pct, mcp_used, mcp_total)
            SELECT device_id, ts, '', level, weekly_pct, weekly_reset,
                   hour5_pct, mcp_pct, mcp_used, mcp_total
            FROM quota_snapshots
            """
        )
        conn.execute("DROP TABLE quota_snapshots")
        conn.execute("ALTER TABLE quota_snapshots_new RENAME TO quota_snapshots")
        conn.execute("COMMIT")
        print("[zbar-sync] quota_snapshots 已自动迁移：新增 account 维度，老数据无损保留")
    except Exception:
        conn.execute("ROLLBACK")
        raise
    finally:
        conn.isolation_level = ""


def _migrate_usage_records_columns(conn):
    """usage_records 幂等补列：session_id / project_key / project_display（proto 5）。

    proto 5 客户端上传的明细会带会话与项目维度字段；老库直接用
    ALTER TABLE ADD COLUMN 原地补列（全部可空、不改主键、不搬数据），
    已有行三列为 NULL——与 proto 2/3/4 客户端上传时的落库行为完全一致。
    ALTER 不会删除表上的索引，无需像 source/account 迁移那样重建索引。
    可重复调用，幂等。
    """
    cols = [r["name"] for r in conn.execute("PRAGMA table_info(usage_records)").fetchall()]
    if not cols:
        return
    added = False
    for name in ("session_id", "project_key", "project_display"):
        if name not in cols:
            conn.execute(f"ALTER TABLE usage_records ADD COLUMN {name} TEXT")
            added = True
    if added:
        conn.commit()
        print(
            "[zbar-sync] usage_records 已补列：session_id / project_key / "
            "project_display（proto 5 会话与项目维度，老记录为空）"
        )


def _migrate_agent_quota_snapshots(conn):
    """agent_quota_snapshots 扩展 provider 维度的幂等自动迁移（proto 7）。

    老表无 label 列（主键 device_id+source+ts+window_key）时：建新表（主键
    扩为 device_id+source+ts+label+window_key，并带 plan_name / balance_json /
    message 三列）→ 旧数据全部按 label='' 搬入 → 删旧表 → 改名。单事务完成，
    老数据无损；已迁移/全新库直接跳过（可重复调用，幂等）。旧数据 label=''
    与旧主键的去重行为一致；同设备多凭证的采样此后按 label 区分互不撞键。
    """
    cols = [r["name"] for r in conn.execute("PRAGMA table_info(agent_quota_snapshots)").fetchall()]
    if not cols or "label" in cols:
        return

    conn.isolation_level = None  # 手动管理事务，保证搬迁原子性
    try:
        conn.execute("BEGIN IMMEDIATE")
        conn.execute(SCHEMA_AGENT_QUOTA_SNAPSHOTS.replace(
            "IF NOT EXISTS agent_quota_snapshots", "IF NOT EXISTS agent_quota_snapshots_new"
        ))
        conn.execute(
            """
            INSERT INTO agent_quota_snapshots_new
                (device_id, source, ts, label, plan_type, plan_name,
                 window_key, used_pct, reset_at, balance_json, message)
            SELECT device_id, source, ts, '', plan_type, NULL,
                   window_key, used_pct, reset_at, NULL, NULL
            FROM agent_quota_snapshots
            """
        )
        conn.execute("DROP TABLE agent_quota_snapshots")
        conn.execute("ALTER TABLE agent_quota_snapshots_new RENAME TO agent_quota_snapshots")
        conn.execute("COMMIT")
        print(
            "[zbar-sync] agent_quota_snapshots 已自动迁移：新增 label/plan_name/"
            "balance_json/message 列，老数据无损保留"
        )
    except Exception:
        conn.execute("ROLLBACK")
        raise
    finally:
        conn.isolation_level = ""


def _migrate_usage_records_speed_columns(conn):
    """usage_records 幂等补列：duration_ms / time_to_first_token_ms（proto 8）。

    proto 8 客户端上传的明细会带耗时/首字字段（请求日志页远端行的耗时/
    首字/速度列数据源）；老库直接用 ALTER TABLE ADD COLUMN 原地补列
    （全部可空、不改主键、不搬数据），已有行两列为 NULL——与 proto 2/3/4
    客户端上传时的落库行为完全一致。ALTER 不会删除表上的索引，无需像
    source/account 迁移那样重建索引。可重复调用，幂等。
    """
    cols = [r["name"] for r in conn.execute("PRAGMA table_info(usage_records)").fetchall()]
    if not cols:
        return
    added = False
    if "duration_ms" not in cols:
        conn.execute("ALTER TABLE usage_records ADD COLUMN duration_ms REAL")
        added = True
    if "time_to_first_token_ms" not in cols:
        conn.execute(
            "ALTER TABLE usage_records ADD COLUMN time_to_first_token_ms REAL"
        )
        added = True
    if added:
        conn.commit()
        print(
            "[zbar-sync] usage_records 已补列：duration_ms / "
            "time_to_first_token_ms（proto 8 耗时/首字维度，老记录为空）"
        )


def init_db():
    """初始化数据库：创建数据目录 + 自动建库建表（幂等，可重复调用）。

    老库（无 source 列）在此处自动迁移；迁移中 DROP TABLE 会连带删掉
    usage_records 上的两个索引，故迁移后重放一遍索引语句挂回新表。
    quota_snapshots 的 account 迁移同理（DROP 会删掉 ts 索引）。
    proto 5 / proto 8 的补列迁移均用 ALTER TABLE 原地完成，不影响索引。
    """
    DATA_DIR.mkdir(parents=True, exist_ok=True)
    conn = get_conn()
    try:
        conn.execute("PRAGMA journal_mode=WAL")
        conn.execute("PRAGMA busy_timeout=5000")
        for sql in ALL_SCHEMA:
            conn.execute(sql)
        conn.commit()
        _migrate_usage_records(conn)
        _migrate_quota_snapshots(conn)
        _migrate_usage_records_columns(conn)
        _migrate_agent_quota_snapshots(conn)
        _migrate_usage_records_speed_columns(conn)
        if not conn.in_transaction:
            # 迁移中的 DROP TABLE 会连带删掉索引（含 agent 快照表上的两个），
            # 这里重放一遍挂回新表
            for sql in (
                INDEX_STARTED,
                INDEX_DEVICE_STARTED,
                INDEX_SNAPSHOT_TS,
                INDEX_AGENT_QUOTA_SNAPSHOT_TS,
                INDEX_AGENT_QUOTA_SNAPSHOT_SOURCE_TS,
            ):
                conn.execute(sql)
            conn.commit()
    finally:
        conn.close()


def get_conn():
    """获取一个新连接。每次操作开新连接（sqlite 轻量，单用户场景足够）。"""
    conn = sqlite3.connect(str(DB_PATH), timeout=5)
    conn.row_factory = sqlite3.Row
    return conn


def now_ms():
    """当前 UTC 毫秒时间戳。"""
    return int(time.time() * 1000)


# ===== 设备表操作 =====

def insert_device(device_id, device_name, token_hash, created_at):
    """注册新设备，写入 devices 表。"""
    with _db_lock:
        conn = get_conn()
        try:
            conn.execute(
                "INSERT INTO devices (device_id, device_name, token_hash, created_at) VALUES (?, ?, ?, ?)",
                (device_id, device_name, token_hash, created_at),
            )
            conn.commit()
        finally:
            conn.close()


def find_device_by_token_hash(token_hash):
    """按 device_token 哈希查找设备 id（鉴权用）。返回 device_id 或 None。"""
    conn = get_conn()
    try:
        row = conn.execute(
            "SELECT device_id FROM devices WHERE token_hash = ?",
            (token_hash,),
        ).fetchone()
        return row["device_id"] if row else None
    finally:
        conn.close()


def list_devices():
    """列出所有设备（附各设备记录数与最后活跃时刻）。

    last_seen_at = 该设备最后一条明细的 started_at（MAX 走既有
    idx_device_started 索引），语义是「最后一条明细的数据时刻」；只传过
    快照、从无明细的设备为 NULL（旧客户端按缺省处理显示 —）。
    """
    conn = get_conn()
    try:
        rows = conn.execute(
            """
            SELECT d.device_id, d.device_name, d.created_at,
                   (SELECT COUNT(*) FROM usage_records u WHERE u.device_id = d.device_id) AS cnt,
                   (SELECT MAX(started_at) FROM usage_records u WHERE u.device_id = d.device_id) AS last_seen
            FROM devices d
            ORDER BY d.created_at ASC
            """
        ).fetchall()
        return [
            {
                "device_id": r["device_id"],
                "device_name": r["device_name"],
                "created_at": r["created_at"],
                "record_count": r["cnt"],
                "last_seen_at": r["last_seen"],
            }
            for r in rows
        ]
    finally:
        conn.close()


# ===== 同步写入 =====

def insert_usage_records(device_id, records, uploaded_at):
    """批量插入明细记录（主键 (device_id, source, local_rowid) 去重 + 修订覆盖）。

    每条记录的 source 缺省为 'zcode'（旧客户端不传即 zcode，向后兼容）。
    proto 5 起每条记录还可带 session_id / project_key / project_display，
    proto 8 起还可带 duration_ms / time_to_first_token_ms（均可缺省，
    旧客户端不传时落库为 NULL）；主键去重不受影响。修订覆盖分支对
    耗时/首字用 COALESCE 非空优先：新值非空才覆盖，空值不抹掉已存值
    ——修订重传的记录若来自无耗时列的老来源库，不得丢此前已写入的耗时。
    同主键重传且 computed_total_tokens 更大时覆盖旧值：Claude Code 会话流式
    落盘，客户端可能先上传某条消息的中间值、稍后本地修正为终值并补传
    （见客户端 claude 模块的 updated_at 修订机制）。另外允许同 Token 数的
    空 model_id 被后续非空模型名修正；已识别出的模型不会被空值覆盖。
    返回实际写入（插入或覆盖）条数。
    """
    if not records:
        return 0
    with _db_lock:
        conn = get_conn()
        try:
            cur = conn.cursor()
            accepted = 0
            for r in records:
                cur.execute(
                    """
                    INSERT INTO usage_records
                        (device_id, source, local_rowid, started_at, model_id, provider_id,
                         input_tokens, output_tokens, cache_read_input_tokens,
                         cache_creation_input_tokens, reasoning_tokens,
                         computed_total_tokens, uploaded_at,
                         session_id, project_key, project_display,
                         duration_ms, time_to_first_token_ms)
                    VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
                    ON CONFLICT(device_id, source, local_rowid) DO UPDATE SET
                        started_at = CASE
                            WHEN excluded.computed_total_tokens > usage_records.computed_total_tokens
                                THEN excluded.started_at
                            ELSE usage_records.started_at
                        END,
                        -- 空模型不能覆盖已经识别出的模型；反过来允许同 Token
                        -- 数的空模型被后续回填的非空模型修正。
                        model_id = CASE
                            WHEN COALESCE(excluded.model_id, '') != ''
                                THEN excluded.model_id
                            ELSE usage_records.model_id
                        END,
                        provider_id = CASE
                            WHEN excluded.computed_total_tokens > usage_records.computed_total_tokens
                                THEN excluded.provider_id
                            ELSE usage_records.provider_id
                        END,
                        input_tokens = CASE
                            WHEN excluded.computed_total_tokens > usage_records.computed_total_tokens
                                THEN excluded.input_tokens
                            ELSE usage_records.input_tokens
                        END,
                        output_tokens = CASE
                            WHEN excluded.computed_total_tokens > usage_records.computed_total_tokens
                                THEN excluded.output_tokens
                            ELSE usage_records.output_tokens
                        END,
                        cache_read_input_tokens = CASE
                            WHEN excluded.computed_total_tokens > usage_records.computed_total_tokens
                                THEN excluded.cache_read_input_tokens
                            ELSE usage_records.cache_read_input_tokens
                        END,
                        cache_creation_input_tokens = CASE
                            WHEN excluded.computed_total_tokens > usage_records.computed_total_tokens
                                THEN excluded.cache_creation_input_tokens
                            ELSE usage_records.cache_creation_input_tokens
                        END,
                        reasoning_tokens = CASE
                            WHEN excluded.computed_total_tokens > usage_records.computed_total_tokens
                                THEN excluded.reasoning_tokens
                            ELSE usage_records.reasoning_tokens
                        END,
                        computed_total_tokens = CASE
                            WHEN excluded.computed_total_tokens > usage_records.computed_total_tokens
                                THEN excluded.computed_total_tokens
                            ELSE usage_records.computed_total_tokens
                        END,
                        -- proto 5 会话/项目维度：仅在携带非 NULL 值时回填，
                        -- 不用 NULL 抹掉此前已写入的归属信息。
                        session_id = COALESCE(excluded.session_id, usage_records.session_id),
                        project_key = COALESCE(excluded.project_key, usage_records.project_key),
                        project_display = COALESCE(excluded.project_display, usage_records.project_display),
                        -- proto 8 耗时/首字：同款非空优先，修订重传不带耗时
                        -- （老来源库无该列）时保留已存值。
                        duration_ms = COALESCE(excluded.duration_ms, usage_records.duration_ms),
                        time_to_first_token_ms = COALESCE(
                            excluded.time_to_first_token_ms,
                            usage_records.time_to_first_token_ms
                        )
                    WHERE excluded.computed_total_tokens > usage_records.computed_total_tokens
                       OR (
                           COALESCE(usage_records.model_id, '') = ''
                           AND COALESCE(excluded.model_id, '') != ''
                       )
                    """,
                    (
                        device_id,
                        r.get("source", "zcode") or "zcode",
                        r["local_rowid"],
                        r["started_at"],
                        r.get("model_id", ""),
                        r.get("provider_id", ""),
                        r.get("input_tokens", 0),
                        r.get("output_tokens", 0),
                        r.get("cache_read_input_tokens", 0),
                        r.get("cache_creation_input_tokens", 0),
                        r.get("reasoning_tokens", 0),
                        r.get("computed_total_tokens", 0),
                        uploaded_at,
                        r.get("session_id"),
                        r.get("project_key"),
                        r.get("project_display"),
                        r.get("duration_ms"),
                        r.get("time_to_first_token_ms"),
                    ),
                )
                accepted += cur.rowcount
            conn.commit()
            return accepted
        finally:
            conn.close()


def max_rowid_of(device_id, source="zcode"):
    """查询某设备某来源（source）已上传的最大 local_rowid。无数据返回 0。"""
    conn = get_conn()
    try:
        row = conn.execute(
            "SELECT COALESCE(MAX(local_rowid), 0) FROM usage_records "
            "WHERE device_id = ? AND source = ?",
            (device_id, source),
        ).fetchone()
        return row[0] if row else 0
    finally:
        conn.close()


def list_all_models():
    """全部设备、全部来源出现过的模型清单（distinct，价格配置用）。

    客户端价格设置页据此展示"其他设备同步上来、本机没有"的模型，
    并让价格检查能覆盖这些模型（如另一台设备在用的新模型）。
    返回 [{source, provider_id, model_id}, ...]，按 model_id 排序。
    """
    conn = get_conn()
    try:
        rows = conn.execute(
            "SELECT DISTINCT source, provider_id, model_id FROM usage_records "
            "ORDER BY model_id, source"
        ).fetchall()
        return [
            {
                "source": r[0] or "zcode",
                "provider_id": r[1] or "",
                "model_id": r[2] or "",
            }
            for r in rows
            if r[2]
        ]
    finally:
        conn.close()


# ===== 聚合查询（/usage 用）=====

def _build_device_filter(device_ids):
    """构造 device_id 过滤子句和参数。
    返回 (sql_fragment, params)。device_ids 为空时不过滤（查全部）。
    """
    if not device_ids:
        return "", []
    placeholders = ",".join("?" * len(device_ids))
    return f"AND device_id IN ({placeholders})", list(device_ids)


def _build_source_filter(source):
    """构造 source 过滤子句和参数。
    source 为空（None/""）时不过滤（全部来源合并），非空按来源精确匹配。
    """
    if not source:
        return "", []
    return "AND source = ?", [source]


def _query_overall_and_models(conn, from_ms, to_ms, device_ids, source=None):
    """查询整体汇总 + 模型分组。source 非空时只统计该来源。
    by_model 每个分组带 source 字段，供前端区分 ZCode / Codex。
    """
    dev_frag, dev_params = _build_device_filter(device_ids)
    src_frag, src_params = _build_source_filter(source)

    # 整体汇总
    overall_row = conn.execute(
        f"""
        SELECT COUNT(*),
               COALESCE(SUM(input_tokens),0),
               COALESCE(SUM(output_tokens),0),
               COALESCE(SUM(cache_read_input_tokens),0),
               COALESCE(SUM(cache_creation_input_tokens),0),
               COALESCE(SUM(reasoning_tokens),0),
               COALESCE(SUM(computed_total_tokens),0)
        FROM usage_records
        WHERE started_at >= ? AND started_at < ? {dev_frag} {src_frag}
        """,
        [from_ms, to_ms] + dev_params + src_params,
    ).fetchone()

    overall = {
        "requests": overall_row[0],
        "input_tokens": overall_row[1],
        "output_tokens": overall_row[2],
        "cache_read_tokens": overall_row[3],
        "cache_write_tokens": overall_row[4],
        "reasoning_tokens": overall_row[5],
        "total_tokens": overall_row[6],
    }

    # 模型分组（按 source + provider + model 分组，不同来源同名模型分开返回）
    model_rows = conn.execute(
        f"""
        SELECT source, model_id, provider_id, COUNT(*),
               COALESCE(SUM(input_tokens),0),
               COALESCE(SUM(output_tokens),0),
               COALESCE(SUM(cache_read_input_tokens),0),
               COALESCE(SUM(cache_creation_input_tokens),0),
               COALESCE(SUM(reasoning_tokens),0),
               COALESCE(SUM(computed_total_tokens),0) AS total_tokens
        FROM usage_records
        WHERE started_at >= ? AND started_at < ? {dev_frag} {src_frag}
        GROUP BY source, provider_id, model_id
        ORDER BY total_tokens DESC
        """,
        [from_ms, to_ms] + dev_params + src_params,
    ).fetchall()

    by_model = [
        {
            "source": r[0] or "zcode",
            "model_id": r[1] or "",
            "provider_id": r[2] or "",
            "requests": r[3],
            "input_tokens": r[4],
            "output_tokens": r[5],
            "cache_read_tokens": r[6],
            "cache_write_tokens": r[7],
            "reasoning_tokens": r[8],
            "total_tokens": r[9],
        }
        for r in model_rows
    ]

    return overall, by_model


def _align_bucket_start_utc(ms, bucket):
    """把毫秒时间戳对齐到桶起点（UTC 整除，与 Rust 版一致）。
    label 返回桶起始 ms 字符串，前端按本地时区格式化。
    """
    width = 3_600_000 if bucket == "hour" else 86_400_000
    return (ms // width) * width


def _query_trend(conn, from_ms, to_ms, bucket, device_ids, source=None):
    """查询分桶趋势（单条聚合 SQL，空桶补齐）。

    原实现按桶循环：每个桶单独发一条 GROUP BY 查询，90 天 hour 请求 =
    2160 次桶循环即 2160 次 SQL 往返，单请求耗时数十秒，曾把 waitress
    线程池拖垮。现改为一条按桶分组的聚合 SQL（2160 次 → 1 次）。

    与旧逐桶循环完全等价的保障：
    - 桶对齐：SQL 中 (started_at/width)*width 为 SQLite 整数除法取整再
      回乘，与 _align_bucket_start_utc 的 (ms // width) * width 一致
      （epoch 0 即 UTC 零点/整日边界，数据中 started_at 均为正毫秒，
      截断除法与 Python // 无差异）；
    - 查询范围：旧循环桶序列并集为 [对齐起点, 对齐起点 + N*width)，
      N = ceil((to_ms - 对齐起点)/width)，与逐桶 WHERE [start, start+width)
      的并集相同——from_ms/to_ms 不落在桶边界时，旧实现同样会统计所在
      桶的整桶数据，新实现传同一对齐范围保持一致；
    - 空桶补齐：SQL 结果按桶起点建 dict，再按旧版同一桶序列（同
      _align_bucket_start_utc 起点、同 width 步进）遍历填桶，缺失的桶
      补 by_model=[]、total_tokens=0、requests=0，trend 数组长度与
      label 序列和旧版完全一致；
    - 桶内 by_model 顺序：usage_records 无 (source, provider_id,
      model_id) 覆盖索引，GROUP BY 走临时 B-tree 按分组键升序输出，
      桶内子序列按 (source, provider_id, model_id) 升序，与旧单桶
      GROUP BY 的顺序一致。

    label 返回桶起始 ms 字符串，前端按本地时区格式化 + 按 ms 合并。
    source 非空时只统计该来源。
    """
    width = 3_600_000 if bucket == "hour" else 86_400_000
    start = _align_bucket_start_utc(from_ms, bucket)
    dev_frag, dev_params = _build_device_filter(device_ids)
    src_frag, src_params = _build_source_filter(source)

    # 与旧 while start < to_ms 循环相同的桶序列：to_ms 不超过对齐起点时
    # 循环不执行，直接返回空数组
    if to_ms <= start:
        return []

    # 旧循环最后一次迭代 start + (N-1)*width < to_ms，桶序列并集的右边界
    bucket_count = (to_ms - start + width - 1) // width
    range_end = start + bucket_count * width

    rows = conn.execute(
        f"""
        SELECT (started_at/{width})*{width} AS bucket_ms, source, model_id, provider_id, COUNT(*),
               COALESCE(SUM(input_tokens),0),
               COALESCE(SUM(output_tokens),0),
               COALESCE(SUM(cache_read_input_tokens),0),
               COALESCE(SUM(computed_total_tokens),0)
        FROM usage_records
        WHERE started_at >= ? AND started_at < ? {dev_frag} {src_frag}
        GROUP BY bucket_ms, source, provider_id, model_id
        ORDER BY bucket_ms
        """,
        [start, range_end] + dev_params + src_params,
    ).fetchall()

    # SQL 结果按桶起点归类（SQLite 整数除法结果即桶起点，与循环步进值相等）
    by_bucket = {}
    for r in rows:
        by_bucket.setdefault(r[0], []).append(
            {
                "source": r[1] or "zcode",
                "model_id": r[2] or "",
                "provider_id": r[3] or "",
                "requests": r[4],
                "input_tokens": r[5],
                "output_tokens": r[6],
                "cache_read_tokens": r[7],
                "total_tokens": r[8],
            }
        )

    out = []
    while start < to_ms:
        by_model = by_bucket.get(start, [])
        total_tokens = sum(m["total_tokens"] for m in by_model)
        requests = sum(m["requests"] for m in by_model)
        out.append(
            {
                "label": str(start),  # 桶起始 ms，前端格式化
                "by_model": by_model,
                "total_tokens": total_tokens,
                "requests": requests,
            }
        )
        start += width
    return out


def query_usage(from_ms, to_ms, bucket, device_ids, source=None):
    """/usage 完整查询：返回 overall + by_model + trend。
    device_ids 为空 = 查全部；非空 = 仅这些设备。
    source 为空 = 全部来源；非空 = 仅该来源。
    """
    conn = get_conn()
    try:
        overall, by_model = _query_overall_and_models(conn, from_ms, to_ms, device_ids, source)
        trend = _query_trend(conn, from_ms, to_ms, bucket, device_ids, source)
        return {
            "from_ms": from_ms,
            "to_ms": to_ms,
            "overall": overall,
            "by_model": by_model,
            "trend": trend,
        }
    finally:
        conn.close()


def empty_usage_result(from_ms, to_ms):
    """构造全空的 usage 结果（显式过滤后无设备的边界情况）。"""
    return {
        "from_ms": from_ms,
        "to_ms": to_ms,
        "overall": {
            "requests": 0,
            "input_tokens": 0,
            "output_tokens": 0,
            "cache_read_tokens": 0,
            "cache_write_tokens": 0,
            "reasoning_tokens": 0,
            "total_tokens": 0,
        },
        "by_model": [],
        "trend": [],
    }


# ===== 额度快照（quota_snapshots）=====

def insert_snapshots(device_id, snaps, uploaded_at):
    """批量插入额度快照（INSERT OR IGNORE 去重，按 device_id+ts+account 主键）。
    返回实际写入条数。单条字段缺失时用默认值兜底；account 缺失（旧客户端）
    统一存空串。
    """
    if not snaps:
        return 0
    with _db_lock:
        conn = get_conn()
        try:
            cur = conn.cursor()
            accepted = 0
            for s in snaps:
                cur.execute(
                    """
                    INSERT OR IGNORE INTO quota_snapshots
                        (device_id, ts, account, level, weekly_pct, weekly_reset,
                         hour5_pct, mcp_pct, mcp_used, mcp_total)
                    VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
                    """,
                    (
                        device_id,
                        s.get("ts", 0),
                        s.get("account") or "",
                        s.get("level", ""),
                        s.get("weekly_pct", 0),
                        s.get("weekly_reset"),
                        s.get("hour5_pct", 0),
                        s.get("mcp_pct", 0),
                        s.get("mcp_used"),
                        s.get("mcp_total"),
                    ),
                )
                accepted += cur.rowcount
            conn.commit()
            return accepted
        finally:
            conn.close()


def max_snapshot_ts_of(device_id):
    """查询某设备已上传快照的最大 ts（游标用）。无数据返回 0。"""
    conn = get_conn()
    try:
        row = conn.execute(
            "SELECT COALESCE(MAX(ts), 0) FROM quota_snapshots WHERE device_id = ?",
            (device_id,),
        ).fetchone()
        return row[0] if row else 0
    finally:
        conn.close()


# /snapshots 与 /agent-quota-snapshots 的行数上限：快照约 30s 采样，90 天
# 2 设备约 25 万行，上限取其约 2 倍余量，定位是防设备数/天数失控增长后
# 单请求无界膨胀（而非卡当前数据量）；全局并发内存上界由 waitress 固定
# 线程池兜底（16 线程 × 单请求峰值有限）。超限不静默截断（截断会让
# 客户端周期解析算错），由 app.py 端点层先 COUNT 再查询，超上限直接
# 400 提示收窄时间窗口。
SNAPSHOT_QUERY_MAX_ROWS = 500_000


def count_snapshots(from_ms, to_ms, device_ids):
    """统计时间范围内的快照行数（/snapshots 端点超限前置检查用）。"""
    dev_frag, dev_params = _build_device_filter(device_ids)
    conn = get_conn()
    try:
        row = conn.execute(
            f"SELECT COUNT(*) FROM quota_snapshots WHERE ts >= ? AND ts < ? {dev_frag}",
            [from_ms, to_ms] + dev_params,
        ).fetchone()
        return row[0]
    finally:
        conn.close()


def query_snapshots(from_ms, to_ms, device_ids):
    """查询时间范围内的快照（带 device_id，按 ts 升序）。
    device_ids 为空 = 全部设备；非空 = 仅这些设备。
    account 为快照归属账号指纹；空串（旧客户端数据/旧客户端上传）返回 None，
    与客户端 RemoteSnapshot.account 的 Option 语义对齐。
    """
    dev_frag, dev_params = _build_device_filter(device_ids)
    conn = get_conn()
    try:
        rows = conn.execute(
            f"""
            SELECT device_id, ts, account, level, weekly_pct, weekly_reset,
                   hour5_pct, mcp_pct, mcp_used, mcp_total
            FROM quota_snapshots
            WHERE ts >= ? AND ts < ? {dev_frag}
            ORDER BY ts ASC
            """,
            [from_ms, to_ms] + dev_params,
        ).fetchall()
        return [
            {
                "device_id": r["device_id"],
                "ts": r["ts"],
                "account": r["account"] or None,
                "level": r["level"] or "",
                "weekly_pct": r["weekly_pct"],
                "weekly_reset": r["weekly_reset"],
                "hour5_pct": r["hour5_pct"],
                "mcp_pct": r["mcp_pct"],
                "mcp_used": r["mcp_used"],
                "mcp_total": r["mcp_total"],
            }
            for r in rows
        ]
    finally:
        conn.close()


def _parse_snapshot_balance(balance):
    """解析快照携带的按量余额（{amount, currency, granted?, topped_up?}）。

    非法输入（非字典 / amount 缺失或非有限数）返回 None——视为未携带，
    与旧客户端不上传该字段的行为一致。合法时序列化为 JSON 字符串落库。
    """
    if not isinstance(balance, dict):
        return None
    try:
        amount = float(balance.get("amount"))
    except (TypeError, ValueError):
        return None
    if not math.isfinite(amount):
        return None
    out = {"amount": amount}
    currency = balance.get("currency")
    if isinstance(currency, str) and currency.strip():
        out["currency"] = currency.strip()[:16]
    # 双键兼容：桌面端 ProviderQuotaBalance 是 serde camelCase（序列化为
    # toppedUp），其余调用方（测试/未来客户端）可能用 snake_case；两个键
    # 都接受，取第一个可解析的有限数。granted 单词无形态差异，无需双写。
    for out_key, in_keys in (("granted", ("granted",)),
                             ("topped_up", ("topped_up", "toppedUp"))):
        for key in in_keys:
            try:
                value = balance.get(key)
                if value is None:
                    continue
                value = float(value)
            except (TypeError, ValueError):
                continue
            if math.isfinite(value):
                out[out_key] = value
                break
    return json.dumps(out, ensure_ascii=False)


def insert_agent_quota_snapshots(device_id, snapshots, uploaded_at):
    """批量插入 Agent 额度快照，并按窗口行幂等去重。

    客户端上传的是一个 snapshot 对象数组，每个对象包含多个 windows；
    服务端将其展平存储。uploaded_at 为兼容现有同步写入接口保留，当前表
    不需要记录上传时间。

    proto 7 扩展：每个 snapshot 还可带 label（凭证备注名，同来源多凭证的
    账号维度）、plan_name（套餐名）、balance（余额对象）、message（错误态
    原因），均可缺省。windows 过滤后为空但上述任一字段有值时，落一行
    window_key='' 的哨兵行（余额型 / 错误态快照）；旧客户端的快照不带
    这些字段，空窗口照旧整体丢弃，行为与 proto 3-6 完全一致。
    """
    if not snapshots:
        return 0

    rows = []
    for snapshot in snapshots:
        if not isinstance(snapshot, dict):
            continue
        source = str(snapshot.get("source") or "").strip()
        if source not in AGENT_QUOTA_WINDOWS:
            continue
        try:
            ts = int(snapshot.get("ts"))
        except (TypeError, ValueError):
            continue
        label = str(snapshot.get("label") or "").strip()[:64]
        plan_type = str(snapshot.get("plan_type") or "").strip()[:64]
        plan_name = str(snapshot.get("plan_name") or "").strip()[:64]
        message = str(snapshot.get("message") or "").strip()[:128]
        balance_json = _parse_snapshot_balance(snapshot.get("balance"))
        windows = snapshot.get("windows") or []
        if not isinstance(windows, (list, tuple)):
            continue

        window_rows = 0
        for window in windows[:16]:
            if not isinstance(window, dict):
                continue
            window_key = str(window.get("key") or "").strip()
            if window_key not in AGENT_QUOTA_WINDOWS[source]:
                continue
            try:
                used_pct = float(window.get("used_pct", 0))
            except (TypeError, ValueError):
                continue
            if not math.isfinite(used_pct) or not 0 <= used_pct <= 100:
                continue

            reset_at = window.get("reset_at")
            if reset_at is not None:
                try:
                    reset_at = int(reset_at)
                except (TypeError, ValueError):
                    reset_at = None
            rows.append(
                (
                    device_id,
                    source,
                    ts,
                    label,
                    plan_type,
                    plan_name or None,
                    window_key,
                    used_pct,
                    reset_at,
                    balance_json,
                    message or None,
                )
            )
            window_rows += 1

        # 哨兵行：无窗口的余额型 / 错误态快照（有任一 proto 7 扩展载荷才落库，
        # 旧客户端的空窗口快照不会进入该分支——行为不变）
        if window_rows == 0 and (plan_name or balance_json or message):
            rows.append(
                (
                    device_id,
                    source,
                    ts,
                    label,
                    plan_type,
                    plan_name or None,
                    "",  # window_key 哨兵：组装回快照时不进 windows
                    0.0,
                    None,
                    balance_json,
                    message or None,
                )
            )

    if not rows:
        return 0

    with _db_lock:
        conn = get_conn()
        try:
            cur = conn.cursor()
            accepted = 0
            for row in rows:
                # 使用 INSERT OR IGNORE + UPDATE，兼容旧版 SQLite（部分宝塔
                # Python 运行时自带的 SQLite 低于 3.24，不支持 UPSERT 的
                # ON CONFLICT ... DO UPDATE 语法）。
                cur.execute(
                    """
                    INSERT OR IGNORE INTO agent_quota_snapshots
                        (device_id, source, ts, label, plan_type, plan_name,
                         window_key, used_pct, reset_at, balance_json, message)
                    VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
                    """,
                    row,
                )
                if cur.rowcount:
                    accepted += 1
                    continue

                updated = cur.execute(
                    """
                    UPDATE agent_quota_snapshots
                    SET plan_type = ?, used_pct = ?, reset_at = ?,
                        plan_name = COALESCE(?, plan_name),
                        balance_json = COALESCE(?, balance_json),
                        message = COALESCE(?, message)
                    WHERE device_id = ? AND source = ? AND ts = ?
                      AND label = ? AND window_key = ? AND used_pct < ?
                    """,
                    (
                        row[4],
                        row[7],
                        row[8],
                        row[5],
                        row[9],
                        row[10],
                        row[0],
                        row[1],
                        row[2],
                        row[3],
                        row[6],
                        row[7],
                    ),
                )
                accepted += updated.rowcount
            conn.commit()
            return accepted
        finally:
            conn.close()


def max_agent_quota_snapshot_ts_of(device_id):
    """查询某设备已上传 Agent 快照的最大时间戳；无数据返回 0。"""
    conn = get_conn()
    try:
        row = conn.execute(
            "SELECT COALESCE(MAX(ts), 0) FROM agent_quota_snapshots "
            "WHERE device_id = ?",
            (device_id,),
        ).fetchone()
        return row[0] if row else 0
    finally:
        conn.close()


def _decode_snapshot_balance(balance_json):
    """把 balance_json 列还原为客户端可解析的余额对象（损坏/非法返回 None）。"""
    if not balance_json:
        return None
    try:
        value = json.loads(balance_json)
    except (TypeError, ValueError, json.JSONDecodeError):
        return None
    if not isinstance(value, dict) or "amount" not in value:
        return None
    return value


def _group_agent_quota_rows(rows):
    """把展平的 agent_quota_snapshots 行组装回客户端 snapshot 形状。

    按 (device_id, source, ts, plan_type, label) 分组，windows 逐行追加；
    /agent-quota-snapshots 使用本组装逻辑，保证返回口径稳定。
    proto 7 扩展字段（label / plan_name / balance / message）随组透传：
    label 空串归一为 None（旧数据语义 = 无账号维度）；window_key='' 的
    哨兵行不进 windows 数组，只贡献扩展载荷。
    """
    grouped = {}
    for row in rows:
        key = (row["device_id"], row["source"], row["ts"], row["plan_type"], row["label"])
        snapshot = grouped.get(key)
        if snapshot is None:
            snapshot = {
                "device_id": row["device_id"],
                "source": row["source"],
                "ts": row["ts"],
                "plan_type": row["plan_type"] or None,
                "label": row["label"] or None,
                "plan_name": row["plan_name"] or None,
                "balance": _decode_snapshot_balance(row["balance_json"]),
                "message": row["message"] or None,
                "windows": [],
            }
            grouped[key] = snapshot
        if row["window_key"]:
            snapshot["windows"].append(
                {
                    "key": row["window_key"],
                    "used_pct": row["used_pct"],
                    "reset_at": row["reset_at"],
                }
            )
    return list(grouped.values())


def count_agent_quota_snapshots(from_ms, to_ms, device_ids, source=None):
    """统计时间范围内的 Agent 额度快照原始行数（端点超限前置检查用）。

    以落库原始行为准——这正是 fetchall 的内存口径；分组组装后的
    snapshot 对象数只会更少，用它判断不会漏放行大结果集。
    """
    dev_frag, dev_params = _build_device_filter(device_ids)
    src_frag, src_params = _build_source_filter(source)
    conn = get_conn()
    try:
        row = conn.execute(
            f"""
            SELECT COUNT(*) FROM agent_quota_snapshots
            WHERE ts >= ? AND ts < ? {dev_frag} {src_frag}
            """,
            [from_ms, to_ms] + dev_params + src_params,
        ).fetchone()
        return row[0]
    finally:
        conn.close()


def query_agent_quota_snapshots(from_ms, to_ms, device_ids, source=None):
    """查询 Agent 额度快照，并组装为客户端可直接解析的 snapshot 形状。

    返回的每个对象包含 device_id、source、ts、plan_type、windows 及 proto 7
    扩展字段（label / plan_name / balance / message，旧数据为 None——旧
    客户端忽略未知字段，天然兼容）；device_ids 为空表示查询全部设备，
    source 为空表示查询全部来源。
    """
    dev_frag, dev_params = _build_device_filter(device_ids)
    src_frag, src_params = _build_source_filter(source)
    conn = get_conn()
    try:
        rows = conn.execute(
            f"""
            SELECT device_id, source, ts, label, plan_type, plan_name,
                   window_key, used_pct, reset_at, balance_json, message
            FROM agent_quota_snapshots
            WHERE ts >= ? AND ts < ? {dev_frag} {src_frag}
            ORDER BY ts ASC, device_id ASC, source ASC,
                     plan_type ASC, label ASC, window_key ASC
            """,
            [from_ms, to_ms] + dev_params + src_params,
        ).fetchall()
    finally:
        conn.close()

    return _group_agent_quota_rows(rows)


# /period_detail 单次请求允许的最大周期数：快照周期解析以周为单位，90 天
# 约 13 个周期，200 留足余量；超出视为异常/恶意请求，防止把一次请求拆成
# 无限多轮聚合查询。
PERIOD_DETAIL_MAX_PERIODS = 200


def query_period_detail(periods, device_ids, source=None):
    """按一组周期 [start, end) 返回各周期内的用量明细（按 (小时桶, 模型) 聚合）。

    供客户端用本地 peak 配置折算消耗（服务端无 peak 配置）。每条含
    started_at/model_id/各 token SUM 字段，与本地 db::query_period_consumed
    口径一致；started_at 为小时桶起点（UTC 对齐，桶内明细共用同一时间戳）。
    source 为空 = 全部来源；非空 = 仅该来源。

    聚合数学等价性：客户端折算公式对 token 数线性——V2 是
    Σ(total_tokens × 倍率)，V3 是 Σ(单价 × token)/1e4 × 倍率，单价与倍率
    都是常系数，先 SUM 后乘与逐行乘后加结果一致；倍率取小时桶起点的
    started_at，peak 段边界落在整点时严格等价，非整点边界桶统一取桶起点
    倍率，误差仅限边界桶（客户端时段配置以小时为主，可忽略）。行数从
    全表逐行（90 天可达 28 万+ 行、单请求几百 MB 内存峰值）降到
    模型数 × 24 × 周期数，内存峰值回落到 KB 级。

    行协议：前六个字段与旧逐行响应同形（started_at/model_id/四个 token
    SUM），新增第七字段 requests = 该聚合行代表的原始请求次数（COUNT）。
    旧客户端按行计数把一行当 1 次请求，新客户端按 requests 累加，新旧
    协议各自口径下都正确。
    periods 数量超过 PERIOD_DETAIL_MAX_PERIODS 时抛 ValueError（端点转 400）。
    """
    if len(periods) > PERIOD_DETAIL_MAX_PERIODS:
        raise ValueError(
            f"周期数 {len(periods)} 超上限 {PERIOD_DETAIL_MAX_PERIODS}"
        )

    dev_frag, dev_params = _build_device_filter(device_ids)
    src_frag, src_params = _build_source_filter(source)
    conn = get_conn()
    try:
        out = []
        for start, end in periods:
            # SQLite 整数除法：started_at / 3600000 即 UTC 小时桶号
            rows = conn.execute(
                f"""
                SELECT (started_at / 3600000) * 3600000 AS hour_ms, model_id,
                       COUNT(*),
                       COALESCE(SUM(input_tokens), 0),
                       COALESCE(SUM(output_tokens), 0),
                       COALESCE(SUM(cache_read_input_tokens), 0),
                       COALESCE(SUM(computed_total_tokens), 0)
                FROM usage_records
                WHERE started_at >= ? AND started_at < ? {dev_frag} {src_frag}
                GROUP BY hour_ms, model_id
                ORDER BY hour_ms, model_id
                """,
                [start, end] + dev_params + src_params,
            ).fetchall()
            out.append(
                {
                    "reset_at": start,
                    "end_at": end,
                    "rows": [
                        {
                            "started_at": r[0],
                            "model_id": r[1] or "",
                            "input_tokens": r[3],
                            "output_tokens": r[4],
                            "cache_read_tokens": r[5],
                            "total_tokens": r[6],
                            "requests": r[2],
                        }
                        for r in rows
                    ],
                }
            )
        return out
    finally:
        conn.close()


# ===== 逐条明细查询（/usage_records 用）=====

# /usage_records 的行数上限：与客户端本地单源深页保护（MAX_FETCH_ROWS）同量级，
# 防止一次请求把大表读进内存。
QUERY_RECORDS_MAX_LIMIT = 5000


def query_usage_records(
    from_ms, to_ms, device_ids, source=None, model_id=None, limit=500, offset=0
):
    """逐条明细查询：返回同 WHERE 下的 total + rows + by_model。

    - rows 按 started_at DESC 返回（同毫秒 device_id / source / local_rowid
      稳定序），带 device_id 逐条全字段；客户端「全部设备」口径恒带
      exclude_device=<本机>，两集合不相交，合并即免去重；
    - by_model 按 (source, provider_id, model_id) 分组（与 /usage 的 by_model
      同款命名），供客户端把远端汇总按真实 source 归并进对应来源路折算成本
      （缓存命中率分源口径依赖 source 维度，因此必须带 source 分组）；
    - device_ids 为空 = 查全部；source / model_id 非空时精确过滤；
    - limit 夹在 [0, QUERY_RECORDS_MAX_LIMIT]，offset >= 0。
    """
    dev_frag, dev_params = _build_device_filter(device_ids)
    src_frag, src_params = _build_source_filter(source)
    if model_id:
        model_frag, model_params = "AND model_id = ?", [model_id]
    else:
        model_frag, model_params = "", []
    where = f"started_at >= ? AND started_at < ? {dev_frag} {src_frag} {model_frag}"
    where_params = [from_ms, to_ms] + dev_params + src_params + model_params

    try:
        limit = max(0, min(int(limit), QUERY_RECORDS_MAX_LIMIT))
        offset = max(0, int(offset))
    except (TypeError, ValueError):
        limit, offset = 500, 0

    conn = get_conn()
    try:
        total = conn.execute(
            f"SELECT COUNT(*) FROM usage_records WHERE {where}", where_params
        ).fetchone()[0]

        rows = conn.execute(
            f"""
            SELECT device_id, source, local_rowid, started_at, model_id, provider_id,
                   input_tokens, output_tokens, cache_read_input_tokens,
                   cache_creation_input_tokens, reasoning_tokens,
                   computed_total_tokens, session_id, project_key, project_display,
                   duration_ms, time_to_first_token_ms
            FROM usage_records
            WHERE {where}
            ORDER BY started_at DESC, device_id ASC, source ASC, local_rowid ASC
            LIMIT ? OFFSET ?
            """,
            where_params + [limit, offset],
        ).fetchall()

        by_model_rows = conn.execute(
            f"""
            SELECT source, model_id, provider_id, COUNT(*),
                   COALESCE(SUM(input_tokens),0),
                   COALESCE(SUM(output_tokens),0),
                   COALESCE(SUM(cache_read_input_tokens),0),
                   COALESCE(SUM(cache_creation_input_tokens),0),
                   COALESCE(SUM(reasoning_tokens),0),
                   COALESCE(SUM(computed_total_tokens),0) AS total_tokens
            FROM usage_records
            WHERE {where}
            GROUP BY source, provider_id, model_id
            ORDER BY total_tokens DESC
            """,
            where_params,
        ).fetchall()
    finally:
        conn.close()

    return {
        "total": total,
        "rows": [
            {
                "device_id": r["device_id"],
                "source": r["source"] or "zcode",
                "local_rowid": r["local_rowid"],
                "started_at": r["started_at"],
                "model_id": r["model_id"] or "",
                "provider_id": r["provider_id"] or "",
                "input_tokens": r["input_tokens"],
                "output_tokens": r["output_tokens"],
                "cache_read_input_tokens": r["cache_read_input_tokens"],
                "cache_creation_input_tokens": r["cache_creation_input_tokens"],
                "reasoning_tokens": r["reasoning_tokens"],
                "computed_total_tokens": r["computed_total_tokens"],
                "session_id": r["session_id"],
                "project_key": r["project_key"],
                "project_display": r["project_display"],
                # proto 8 耗时/首字：服务端旧数据该列为 NULL，正常返回，
                # 客户端按缺省 None 处理（前端显示「—」）。
                "duration_ms": r["duration_ms"],
                "time_to_first_token_ms": r["time_to_first_token_ms"],
            }
            for r in rows
        ],
        "by_model": [
            {
                "source": r[0] or "zcode",
                "model_id": r[1] or "",
                "provider_id": r[2] or "",
                "requests": r[3],
                "input_tokens": r[4],
                "output_tokens": r[5],
                "cache_read_tokens": r[6],
                "cache_write_tokens": r[7],
                "reasoning_tokens": r[8],
                "total_tokens": r[9],
            }
            for r in by_model_rows
        ],
    }


# ===== 配置表（自动清理用）=====

def get_config(key):
    conn = get_conn()
    try:
        row = conn.execute("SELECT value FROM config WHERE key = ?", (key,)).fetchone()
        return row["value"] if row else None
    finally:
        conn.close()


def set_config(key, value):
    with _db_lock:
        conn = get_conn()
        try:
            conn.execute(
                "INSERT INTO config (key, value) VALUES (?, ?) "
                "ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                (key, value),
            )
            conn.commit()
        finally:
            conn.close()


# ===== 统计 + 清理 =====

def total_records():
    """服务端总记录数。"""
    conn = get_conn()
    try:
        return conn.execute("SELECT COUNT(*) FROM usage_records").fetchone()[0]
    finally:
        conn.close()


def delete_before(cutoff_ms):
    """按时间清理明细、Z.ai 快照和 Agent 快照，返回删除的明细条数。"""
    with _db_lock:
        conn = get_conn()
        try:
            n = conn.execute(
                "DELETE FROM usage_records WHERE started_at < ?", (cutoff_ms,)
            ).rowcount
            conn.execute(
                "DELETE FROM quota_snapshots WHERE ts < ?", (cutoff_ms,)
            )
            conn.execute(
                "DELETE FROM agent_quota_snapshots WHERE ts < ?", (cutoff_ms,)
            )
            conn.commit()
            return n
        finally:
            conn.close()


def delete_device_records(device_id):
    """按设备清理：删除指定设备的明细、Z.ai 快照和 Agent 快照。"""
    with _db_lock:
        conn = get_conn()
        try:
            cur = conn.execute(
                "DELETE FROM usage_records WHERE device_id = ?", (device_id,)
            )
            conn.execute(
                "DELETE FROM quota_snapshots WHERE device_id = ?", (device_id,)
            )
            conn.execute(
                "DELETE FROM agent_quota_snapshots WHERE device_id = ?", (device_id,)
            )
            conn.commit()
            return cur.rowcount
        finally:
            conn.close()


def delete_unattributed_snapshots():
    """删除无账号指纹的 Z.ai 额度快照，返回删除条数。

    account 为空串有两种来源：旧客户端不传该字段（多账号区分上线前的历史
    采样），以及切号瞬间客户端取不到指纹的那一条。两类都无法归属到具体
    账号、不参与任何账号统计，只会在对比页多出一条"旧快照"系列，统一清掉。
    只影响 quota_snapshots，不触碰明细用量。
    """
    with _db_lock:
        conn = get_conn()
        try:
            n = conn.execute(
                "DELETE FROM quota_snapshots WHERE account IS NULL OR account = ''"
            ).rowcount
            conn.commit()
            return n
        finally:
            conn.close()


def delete_all_usage():
    """全部清空三类用量数据，保留设备注册；返回删除的明细条数。"""
    with _db_lock:
        conn = get_conn()
        try:
            cur = conn.execute("DELETE FROM usage_records")
            conn.execute("DELETE FROM quota_snapshots")
            conn.execute("DELETE FROM agent_quota_snapshots")
            conn.commit()
            return cur.rowcount
        finally:
            conn.close()


def reset_all():
    """reset：清空三类用量数据并删除设备，返回 (usage_deleted, devices_deleted)。"""
    with _db_lock:
        conn = get_conn()
        try:
            u = conn.execute("DELETE FROM usage_records").rowcount
            conn.execute("DELETE FROM quota_snapshots")
            conn.execute("DELETE FROM agent_quota_snapshots")
            d = conn.execute("DELETE FROM devices").rowcount
            conn.commit()
            return u, d
        finally:
            conn.close()


def revoke_device(device_id):
    """撤销设备：同时删除设备、明细、Z.ai 快照和 Agent 快照。"""
    with _db_lock:
        conn = get_conn()
        try:
            u = conn.execute(
                "DELETE FROM usage_records WHERE device_id = ?", (device_id,)
            ).rowcount
            conn.execute(
                "DELETE FROM quota_snapshots WHERE device_id = ?", (device_id,)
            )
            conn.execute(
                "DELETE FROM agent_quota_snapshots WHERE device_id = ?", (device_id,)
            )
            d = conn.execute(
                "DELETE FROM devices WHERE device_id = ?", (device_id,)
            ).rowcount
            conn.commit()
            return d, u
        finally:
            conn.close()


def merge_devices(source_id, target_id):
    """把 source 设备的全部数据合并到 target，然后删除 source。

    用量明细的主键是 (device_id, source, local_rowid)，而 local_rowid 来自
    各机本地的库 rowid（zcode 与 codex 两个来源各自从 1 开始递增）。直接改
    device_id 会撞主键；但若简单地接在 target 现有最大值之后（base+1..base+N）
    也不行——target 客户端的增量上传游标仍是 base，它接下来会用 rowid =
    base+1, base+2, ... 上传自己的真实记录，而服务端用 INSERT OR IGNORE
    去重，这些真实记录会被合并进来的历史记录占用而**静默丢弃、且因游标推进
    而永久丢失**。

    因此这里把合并记录放到一个 target 客户端在可预见未来都不可能触达的远端
    区段（当前最大值 + MERGE_ROWID_OFFSET）。重编号遍历 (source, local_rowid)
    组合按序分配编号——所有来源混在同一个编号序列里即可保证唯一，无需按来源
    分段。sqlite rowid 上限是 2^63，偏移量取 20 亿（即便每秒一条用量也要
    ~63 年才会长到），个人监控工具绝无可能撞上。额度快照主键是
    (device_id, ts)，先丢弃来源中与 target 同 ts 的条目再迁移。Agent 额度
    快照按完整主键 INSERT OR IGNORE 合并，冲突时保留 target 已有值。整个
    操作在一个事务内完成。

    返回 (records_moved, snapshots_moved)。source/target 不存在时抛 ValueError。
    """
    # 远超任何真实客户端 rowid 上限，避免与 target 后续真实上传撞主键
    MERGE_ROWID_OFFSET = 2_000_000_000

    if source_id == target_id:
        raise ValueError("来源设备与目标设备不能相同")

    with _db_lock:
        conn = get_conn()
        try:
            exists = conn.execute(
                "SELECT COUNT(*) AS c FROM devices WHERE device_id IN (?, ?)",
                (source_id, target_id),
            ).fetchone()["c"]
            if exists < 2:
                raise ValueError("来源或目标设备不存在")

            # 1) 用量明细：重编号到 target 客户端不可达的远端区段，再迁移。
            #    遍历 (source, local_rowid) 组合，混合编号保证跨来源唯一
            max_row = conn.execute(
                "SELECT COALESCE(MAX(local_rowid), 0) AS m "
                "FROM usage_records WHERE device_id = ?",
                (target_id,),
            ).fetchone()["m"]
            start = max_row + MERGE_ROWID_OFFSET
            src_rows = conn.execute(
                "SELECT source, local_rowid FROM usage_records WHERE device_id = ? "
                "ORDER BY local_rowid, source",
                (source_id,),
            ).fetchall()
            for i, r in enumerate(src_rows, start=1):
                conn.execute(
                    "UPDATE usage_records SET device_id = ?, local_rowid = ? "
                    "WHERE device_id = ? AND source = ? AND local_rowid = ?",
                    (target_id, start + i, source_id, r["source"], r["local_rowid"]),
                )
            records_moved = len(src_rows)

            # 2) 额度快照：丢弃与 target 同 ts 的来源条目，再迁移
            conn.execute(
                "DELETE FROM quota_snapshots WHERE device_id = ? "
                "AND ts IN (SELECT ts FROM quota_snapshots WHERE device_id = ?)",
                (source_id, target_id),
            )
            snap = conn.execute(
                "UPDATE quota_snapshots SET device_id = ? WHERE device_id = ?",
                (target_id, source_id),
            )
            snapshots_moved = snap.rowcount

            # 3) Agent 额度快照：按完整窗口主键（含 proto 7 的 label 维度）
            # 迁移，冲突时保留较高用量。不使用 INSERT ... SELECT ... ON CONFLICT，
            # 兼容旧版 SQLite。
            agent_rows = conn.execute(
                """
                SELECT source, ts, label, plan_type, plan_name, window_key,
                       used_pct, reset_at, balance_json, message
                FROM agent_quota_snapshots
                WHERE device_id = ?
                """,
                (source_id,),
            ).fetchall()
            for row in agent_rows:
                values = (
                    target_id,
                    row["source"],
                    row["ts"],
                    row["label"],
                    row["plan_type"],
                    row["plan_name"],
                    row["window_key"],
                    row["used_pct"],
                    row["reset_at"],
                    row["balance_json"],
                    row["message"],
                )
                inserted = conn.execute(
                    """
                    INSERT OR IGNORE INTO agent_quota_snapshots
                        (device_id, source, ts, label, plan_type, plan_name,
                         window_key, used_pct, reset_at, balance_json, message)
                    VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
                    """,
                    values,
                ).rowcount
                if not inserted:
                    # 冲突时分两步走，各自守自己的语义：
                    # a) 窗口值修订：较高用量者胜（used_pct < ? 门控——
                    #    哨兵行双方 used_pct 都是 0，此分支对哨兵行天然不生效，
                    #    窗口行行为与旧版完全一致）；
                    conn.execute(
                        """
                        UPDATE agent_quota_snapshots
                        SET plan_type = ?, used_pct = ?, reset_at = ?
                        WHERE device_id = ? AND source = ? AND ts = ?
                          AND label = ? AND window_key = ? AND used_pct < ?
                        """,
                        (
                            values[4],
                            values[7],
                            values[8],
                            values[0],
                            values[1],
                            values[2],
                            values[3],
                            values[6],
                            values[7],
                        ),
                    )
                    # b) proto 7 扩展载荷回填：仅来源行携带且目标行为空时覆盖
                    #    （COALESCE 判空，不带 used_pct 门控——余额型 / 错误态
                    #    哨兵行的 used_pct 恒为 0，若沿用 used_pct < ? 条件将
                    #    永假，余额与错误原因在设备合并时会被静默丢掉）。
                    if values[5] or values[9] or values[10]:
                        conn.execute(
                            """
                            UPDATE agent_quota_snapshots
                            SET plan_name = COALESCE(plan_name, ?),
                                balance_json = COALESCE(balance_json, ?),
                                message = COALESCE(message, ?)
                            WHERE device_id = ? AND source = ? AND ts = ?
                              AND label = ? AND window_key = ?
                            """,
                            (
                                values[5],
                                values[9],
                                values[10],
                                values[0],
                                values[1],
                                values[2],
                                values[3],
                                values[6],
                            ),
                        )
            conn.execute(
                "DELETE FROM agent_quota_snapshots WHERE device_id = ?",
                (source_id,),
            )

            # 4) 删除来源设备记录
            conn.execute(
                "DELETE FROM devices WHERE device_id = ?", (source_id,)
            )

            conn.commit()
            return records_moved, snapshots_moved
        finally:
            conn.close()


def rename_device(device_id, new_name):
    """修改设备显示名。返回更新行数（0 表示设备不存在）。"""
    with _db_lock:
        conn = get_conn()
        try:
            n = conn.execute(
                "UPDATE devices SET device_name = ? WHERE device_id = ?",
                (new_name, device_id),
            ).rowcount
            conn.commit()
            return n
        finally:
            conn.close()
