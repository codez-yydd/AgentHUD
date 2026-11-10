"""zbar-sync：ZBar 多设备额度同步服务（自托管）。

Python + Flask 实现。启动自动建库、生成 master token 并打印到日志。
监听 HTTP 端口，提供同步上传/聚合查询/清理接口。

所有 API 路径、字段名、鉴权逻辑与 Rust 版完全一致，客户端无需改动。
"""

import base64
import hashlib
import json
import logging
import math
import re
import threading
import time

from flask import Flask, Response, g, jsonify, request

import db
from auth import (
    hash_token,
    load_or_create_master_token,
    random_device_id,
    random_hex,
    safe_eq,
)
from config import HOST, PORT

app = Flask(__name__)

# 启动时读取/生成的 master token（全局）
MASTER_TOKEN = None

# 自动清理配置 key
CLEANUP_CONFIG_KEY = "cleanup_config"

# 桌面端发布的价格表（手机端只读）。整表存 config 表，后写覆盖。
PRICING_CONFIG_KEY = "pricing"
# 价格表的「读-改-写」必须串行化：多台设备同时推送不同模型时，若各自
# 先读同一 revision 再写，后写者会覆盖前者的变更（lost update）。
PRICING_LOCK = threading.Lock()
MAX_PRICING_MODELS = 2000
MAX_PRICE_VALUE = 1_000_000.0
MAX_FX_RATE = 1000.0


def _agent_quota_storage_available():
    """判断当前加载的 db.py 是否完整支持 Agent 额度快照。

    宝塔更新文件时可能短暂只替换其中一个模块。兼容模式下继续提供
    原有同步服务，避免新版 app.py 调用旧版 db.py 直接抛 AttributeError。
    """
    return all(
        callable(getattr(db, name, None))
        for name in (
            "insert_agent_quota_snapshots",
            "max_agent_quota_snapshot_ts_of",
            "query_agent_quota_snapshots",
        )
    ) and isinstance(getattr(db, "AGENT_QUOTA_WINDOWS", None), dict)


def _quota_account_available():
    """判断当前加载的 db.py 是否支持额度快照的 account 维度（proto 4）。

    旧版 db.py 的 quota_snapshots 无 account 列（存不进也查不出），快照
    回传时 account 恒缺失，客户端多账号今日增量的多端合并会静默退化为
    仅本机。标记为 proto 4 供客户端探测；兼容模式下继续原有同步服务。
    """
    return callable(getattr(db, "_migrate_quota_snapshots", None))


def _session_project_available():
    """判断当前加载的 db.py 是否支持会话/项目维度（proto 5）。

    宝塔热替换场景下可能出现新 app.py + 旧 db.py：旧版 insert_usage_records
    会忽略 records 里的新字段（不报错、只是不落库）。此处与 proto 2/3/4 的
    探测方式一致，按明细表幂等补列能力标记 proto 5；项目维度数据仅落库
    存储，服务端暂无聚合查询接口（/projects 已随手机端查看功能下线移除）。
    """
    return callable(getattr(db, "_migrate_usage_records_columns", None))


def _usage_records_query_available():
    """判断当前加载的 db.py 是否支持逐条明细查询（GET /usage_records，proto 6）。

    请求日志页「全部设备」口径依赖该接口；与旧版 db.py 配合时接口返回空
    结果（客户端 404/空结果均降级为纯本机），原有同步服务不受影响。
    """
    return callable(getattr(db, "query_usage_records", None))


def _provider_quota_sync_available():
    """判断当前加载的 db.py 是否支持全量 provider 额度同步（proto 7）。

    宝塔热替换窗口（新 app.py + 旧 db.py）下，旧版 db.py 的
    agent_quota_snapshots 无 label/plan_name/balance 列、AGENT_QUOTA_WINDOWS
    无新 provider——新客户端上传的 provider 快照会被静默丢弃。此处同时探测
    迁移函数与新白名单（deepseek 等余额型来源），两项齐备才报 proto 7；
    客户端据此决定是否推进 provider 快照游标，未升级时不推进、升级后补传。
    """
    return callable(getattr(db, "_migrate_agent_quota_snapshots", None)) and (
        "deepseek" in getattr(db, "AGENT_QUOTA_WINDOWS", {})
    )


def _speed_fields_available():
    """判断当前加载的 db.py 是否支持明细的耗时/首字列（proto 8）。

    宝塔热替换场景下可能出现新 app.py + 旧 db.py：旧版 insert_usage_records
    只按显式字段名读取 records，未知 JSON 键天然忽略（多传字段无害、只是
    不落库）。此处与 proto 2/3/4/5/6/7 的探测方式一致，按明细表幂等补列能力
    标记 proto 8；请求日志页远端行的耗时/首字/速度列依赖该能力。
    """
    return callable(getattr(db, "_migrate_usage_records_speed_columns", None))


# ===== 鉴权辅助 =====

def get_master_token():
    """全局 master token（启动时初始化）。"""
    return MASTER_TOKEN


def require_master_token(req_data):
    """校验 body 里的 master_token。失败返回 (error_response, None)。"""
    token = req_data.get("master_token", "")
    if not safe_eq(token, get_master_token()):
        return (jsonify({"error": "master_token 无效"}), 401)
    return None


def require_device_token():
    """校验 Header 里的 device_token，返回 device_id。
    失败返回 (error_response, None)。
    """
    auth_header = request.headers.get("Authorization", "")
    token = None
    if auth_header.startswith("Bearer "):
        token = auth_header[7:].strip()
    if not token:
        return (jsonify({"error": "缺少 Authorization 头"}), 401), None
    token_hash = hash_token(token)
    device_id = db.find_device_by_token_hash(token_hash)
    if not device_id:
        return (jsonify({"error": "device_token 无效"}), 401), None
    return None, device_id


# ===== 同步接口 =====

# /sync 上传明细（records）的来源白名单：source 由持有 device token 的客户端
# 上传，属于任意字符串，不校验会原样落库（usage_records.source）并回流到
# /usage 等展示接口，构成 XSS 注入面，这里统一收紧。
# 新增来源必须与客户端 sync.rs 的 upload_derived_source_incremental 同步加：
# 客户端对白名单外的 source 会收到 400（该来源游标因此不推进，明细不丢，
# 部署新版服务端后自动补传）。
SYNC_SOURCE_WHITELIST = {"zcode", "codex", "claude", "kimi", "cursor", "dsh", "opencode", "minimaxcode"}


def _first_invalid_record_source(records):
    """返回 records 中第一个白名单外的 source 值（用于 400 错误信息），无则 None。

    与 db.insert_usage_records 的缺省语义保持一致：source 缺失或为空按 zcode。
    """
    for item in records:
        if not isinstance(item, dict):
            continue
        source = str(item.get("source") or "zcode")
        if source not in SYNC_SOURCE_WHITELIST:
            return source
    return None


@app.post("/register")
def register():
    """注册新设备。验证 master_token 后生成 device_id + device_token。"""
    data = request.get_json(force=True)
    err = require_master_token(data)
    if err:
        return err

    name = (data.get("device_name") or "").strip()
    if not name:
        return jsonify({"error": "设备名称不能为空"}), 400
    if len(name) > 32:
        return jsonify({"error": "设备名称过长（最多 32 字符）"}), 400

    device_id = data.get("device_id") or random_device_id()
    device_token = random_hex()
    token_hash = hash_token(device_token)
    db.insert_device(device_id, name, token_hash, db.now_ms())
    return jsonify(
        {
            "device_id": device_id,
            "device_token": device_token,
            "device_name": name,
        }
    )


@app.post("/sync")
def sync():
    """增量上传明细 + 额度快照。

    body 字段（两类 snapshots 均可选，向后兼容旧客户端）：
    - records: 用量明细数组（每条可带 source='zcode'|'codex'，缺省 zcode；
      客户端保证每批 records 属同一来源，zcode 与 codex 各自独立分批上传）。
      proto 5 起每条还可带 session_id / project_key / project_display，
      proto 8 起还可带 duration_ms / time_to_first_token_ms（均可选）；
      旧客户端不传即落库 NULL，行为不变
    - last_rowid: 本批记录游标（按本批来源的 rowid 序列计数）
    - snapshots: Z.ai 额度快照数组（可选）
    - last_snapshot_ts: Z.ai 快照游标（可选）
    - agent_quota_snapshots: Codex / Claude / Cursor 额度快照数组（可选）
    - last_agent_quota_snapshot_ts: Agent 快照游标（可选）
    """
    err, device_id = require_device_token()
    if err:
        return err

    data = request.get_json(force=True)
    records = data.get("records", [])
    snapshots = data.get("snapshots", [])
    agent_quota_snapshots = data.get("agent_quota_snapshots", [])

    # source 白名单校验：批次中含白名单外来源时返回 400 拒绝整批，任何数据
    # 都不落库；游标（last_rowid 等）由客户端持有，校验失败时响应不携带游标，
    # 客户端游标不推进，修正来源后重新上传即可，不会丢数据或死循环
    # （客户端对 4xx 的处理是降级重试一次后报错）。
    invalid_source = _first_invalid_record_source(records)
    if invalid_source is not None:
        return (
            jsonify({"error": f"records 包含白名单外的 source 值: {invalid_source}"}),
            400,
        )
    # agent_quota_snapshots 的 source 不在此重复校验：db.insert_agent_quota_snapshots
    # 已按 AGENT_QUOTA_WINDOWS 白名单逐条过滤（非法来源静默跳过、不落库），
    # 注入入口已被覆盖，且该"单条忽略"语义由测试
    # test_invalid_agent_sources_and_percentages_are_ignored 锁定。

    now = db.now_ms()

    # 明细（source 逐条读取，缺省 zcode；主键 (device_id, source, local_rowid) 去重）
    accepted = 0
    if records:
        accepted = db.insert_usage_records(device_id, records, now)
    last_rowid = data.get("last_rowid")
    if last_rowid is None:
        # 兼容不传 last_rowid 的旧客户端。当前客户端恒传 last_rowid（sync.rs），
        # 此分支实际不会命中。注意：不能用 max_rowid_of(device_id, source) 作为
        # 回退——设备合并后该值会因合并记录的大偏移 rowid 而膨胀到 20 亿+，旧
        # 客户端会把它当游标写回，导致后续上传被永久跳过（静默丢数据）。返回 0
        # 让旧客户端全量重传（INSERT OR IGNORE 幂等去重，不丢数据），最坏只是
        # 多一次冗余上传。
        max_rowid = 0
    else:
        max_rowid = last_rowid

    # 快照（可选；旧客户端不传 snapshots，跳过）
    accepted_snaps = 0
    max_snapshot_ts = None
    if snapshots:
        accepted_snaps = db.insert_snapshots(device_id, snapshots, now)
        last_ts = data.get("last_snapshot_ts")
        max_snapshot_ts = last_ts if last_ts is not None else db.max_snapshot_ts_of(device_id)

    # Agent 额度快照（可选；新客户端使用，旧客户端不会传此字段）
    accepted_agent_quota_snapshots = 0
    max_agent_quota_snapshot_ts = None
    if agent_quota_snapshots and _agent_quota_storage_available():
        accepted_agent_quota_snapshots = db.insert_agent_quota_snapshots(
            device_id, agent_quota_snapshots, now
        )
        last_agent_ts = data.get("last_agent_quota_snapshot_ts")
        max_agent_quota_snapshot_ts = (
            last_agent_ts
            if last_agent_ts is not None
            else db.max_agent_quota_snapshot_ts_of(device_id)
        )

    return jsonify({
        "accepted": accepted,
        "max_rowid": max_rowid,
        "accepted_snapshots": accepted_snaps,
        "max_snapshot_ts": max_snapshot_ts,
        "accepted_agent_quota_snapshots": accepted_agent_quota_snapshots,
        "max_agent_quota_snapshot_ts": max_agent_quota_snapshot_ts,
        # 服务端协议版本：8 = 明细带耗时/首字（duration_ms /
        # time_to_first_token_ms，旧客户端不传落 NULL）；7 = 支持全量 provider
        # 额度同步（agent_quota_snapshots 扩展 label/plan_name/balance/message
        # 字段 + AGENT_QUOTA_WINDOWS 收录 deepseek/moonshot/minimax/opencodego/
        # alibaba/qoder/longcat/mimo/stepfun/grok/alibabatoken）；6 = 支持逐条
        # 明细查询（GET /usage_records，请求日志页「全部设备」口径）；5 = 明细
        # 支持会话/项目维度（session_id / project_key / project_display）；
        # 4 = 额度快照带 account 维度（多账号采样隔离与多端今日增量合并）；
        # 3 = 支持 Agent 额度快照同步；若当前加载的是旧版 db.py，则降级为
        # 7/6/5/4/3/2，客户端按版本探测能力。
        # proto 2 的多来源 usage_records.source 行为。
        # 客户端据此探测——旧版服务端（无 source 列）会把 codex 记录按
        # (device_id, local_rowid) 撞键静默丢弃，客户端发现 proto < 2 时
        # 不会推进 codex 游标，升级服务端后自动恢复，数据不丢。
        "proto": (
            (9 if hasattr(db, "_speed_evidence") else 8)
            if _speed_fields_available()
            else (
                7
                if _provider_quota_sync_available()
                else (
                    6
                    if _usage_records_query_available()
                    else (
                        5
                        if _session_project_available()
                        else (
                            4
                            if _quota_account_available()
                            else (3 if _agent_quota_storage_available() else 2)
                        )
                    )
                )
            )
        ),
    })


# ===== 定价同步（服务端为全局唯一价，桌面端可编辑，手机端只读） =====

def _empty_pricing_table():
    return {
        "revision": 0,
        "updated_at": 0,
        "updated_by": "",
        "usd": {},
        "hidden": [],
        "fx_rate": 0,
    }


def _normalize_pricing_table(stored):
    """把历史/当前存储格式归一化为统一表结构。

    兼容旧格式：此前版本用 device_id 记录发布者、没有 revision；读取时
    按 revision=0 处理，下一次增量写入会自然升级。
    """
    if not isinstance(stored, dict):
        raise ValueError("pricing config 不是 JSON 对象")
    usd = stored.get("usd")
    usd = usd if isinstance(usd, dict) else {}
    hidden = stored.get("hidden")
    hidden = [h for h in hidden if isinstance(h, str)] if isinstance(hidden, list) else []
    revision = stored.get("revision", 0)
    if isinstance(revision, bool) or not isinstance(revision, int) or revision < 0:
        revision = 0
    updated_at = stored.get("updated_at", 0)
    if isinstance(updated_at, bool) or not isinstance(updated_at, int):
        updated_at = 0
    updated_by = stored.get("updated_by") or stored.get("device_id") or ""
    if not isinstance(updated_by, str):
        updated_by = ""
    fx_rate = stored.get("fx_rate", 0)
    if isinstance(fx_rate, bool) or not isinstance(fx_rate, (int, float)):
        fx_rate = 0
    fx_rate = float(fx_rate)
    if not math.isfinite(fx_rate) or fx_rate < 0:
        fx_rate = 0
    return {
        "revision": revision,
        "updated_at": updated_at,
        "updated_by": updated_by,
        "usd": usd,
        "hidden": hidden,
        "fx_rate": fx_rate,
    }


def _load_pricing_table():
    """读取价格表；无表返回 None，损坏抛 ValueError。"""
    raw = db.get_config(PRICING_CONFIG_KEY)
    if not raw:
        return None
    return _normalize_pricing_table(json.loads(raw))


def _pricing_response(table, *, synced=True, applied=0, concurrent=False):
    table = _normalize_pricing_table(table)
    return jsonify({
        "revision": table["revision"],
        "updated_at": table["updated_at"],
        "updated_by": table["updated_by"],
        "usd": table["usd"],
        "hidden": table["hidden"],
        "fx_rate": table["fx_rate"],
        "model_count": len(table["usd"]),
        "synced": synced,
        "applied": applied,
        "concurrent": concurrent,
    })


def _validate_model_price(model_id, raw_price):
    """校验单个模型价格，返回 (price, error)。"""
    if not isinstance(raw_price, dict):
        return None, f"模型 {model_id} 的价格必须是对象或 null"
    price = {}
    keys = ("input", "output", "cache_read") + (("cache_write",) if "cache_write" in raw_price else ())
    for key in keys:
        value = raw_price.get(key)
        if isinstance(value, bool) or not isinstance(value, (int, float)):
            return None, f"模型 {model_id} 缺少有效的 {key}"
        number = float(value)
        if not math.isfinite(number) or number < 0 or number > MAX_PRICE_VALUE:
            return None, f"模型 {model_id} 的 {key} 超出有效范围"
        price[key] = number
    return price, None


def _validate_price_changes(changes):
    """校验 changes：{model_id: price|null}，null 表示删除。"""
    if not isinstance(changes, dict):
        return None, "changes 必须是对象"
    if len(changes) > MAX_PRICING_MODELS:
        return None, f"单次变更模型数超过上限（{MAX_PRICING_MODELS}）"
    clean = {}
    for raw_id, raw_price in changes.items():
        if not isinstance(raw_id, str):
            return None, "模型 id 必须是字符串"
        model_id = raw_id.strip()
        if not model_id or len(model_id) > 200:
            return None, "模型 id 为空或过长"
        if raw_price is None:
            clean[model_id] = None
            continue
        price, message = _validate_model_price(model_id, raw_price)
        if message:
            return None, message
        clean[model_id] = price
    return clean, None


def _validate_hidden(hidden):
    if not isinstance(hidden, list):
        return None, "hidden 必须是字符串数组"
    if len(hidden) > MAX_PRICING_MODELS:
        return None, f"隐藏模型数超过上限（{MAX_PRICING_MODELS}）"
    clean = []
    seen = set()
    for raw in hidden:
        if not isinstance(raw, str):
            return None, "hidden 只能包含字符串"
        item = raw.strip()
        if not item or len(item) > 200:
            return None, "hidden 中存在空值或过长的模型 id"
        if item not in seen:
            seen.add(item)
            clean.append(item)
    return clean, None


def _validate_fx_rate(value):
    if isinstance(value, bool) or not isinstance(value, (int, float)):
        return None, "fx_rate 必须是数字"
    number = float(value)
    if not math.isfinite(number) or not (0 < number <= MAX_FX_RATE):
        return None, f"fx_rate 超出有效范围（0, {MAX_FX_RATE}]"
    return number, None


@app.post("/pricing")
def publish_pricing():
    """桌面端按模型增量发布价格（服务端全局唯一价）。

    读-改-写全程持有 PRICING_LOCK，保证并发推送不同模型时不会互相覆盖。
    """
    with PRICING_LOCK:
        return _publish_pricing_locked()


def _publish_pricing_locked():
    """publish_pricing 的实现；调用方必须持有 PRICING_LOCK。

    body（新协议）：
    - changes: {model_id: {input, output, cache_read} | null}，null=删除
    - base_revision: 客户端基于的服务端版本（可选，缺省视为当前版本）
    - hidden / fx_rate: 可选，整表隐藏名单与汇率

    兼容旧协议：直接传 usd（整表）时按整表替换处理，revision 正常递增。
    服务端按模型合并：不同设备改不同模型互不覆盖；同一模型以最后到达
    的写入为准，并在响应里返回 concurrent 标记供客户端提示。
    """
    err, device_id = require_device_token()
    if err:
        return err
    data = request.get_json(silent=True)
    if not isinstance(data, dict):
        return jsonify({"error": "请求体必须是 JSON 对象"}), 400

    try:
        current = _load_pricing_table()
    except (TypeError, ValueError):
        logging.exception("pricing config 解析失败")
        return jsonify({"error": "服务端价格配置损坏"}), 500
    if current is None:
        current = _empty_pricing_table()

    base_revision = data.get("base_revision", current["revision"])
    if isinstance(base_revision, bool) or not isinstance(base_revision, int) or base_revision < 0:
        return jsonify({"error": "base_revision 必须是非负整数"}), 400
    concurrent = current["revision"] > 0 and base_revision != current["revision"]

    changes = data.get("changes")
    legacy_usd = data.get("usd")
    if changes is not None and legacy_usd is not None:
        return jsonify({"error": "changes 与 usd 不能同时提交"}), 400

    new_usd = dict(current["usd"])
    applied = 0
    if changes is not None:
        clean_changes, message = _validate_price_changes(changes)
        if message:
            return jsonify({"error": message}), 400
        for model_id, price in clean_changes.items():
            if price is None:
                if model_id in new_usd:
                    del new_usd[model_id]
                    applied += 1
            elif new_usd.get(model_id) != price:
                new_usd[model_id] = price
                applied += 1
    elif legacy_usd is not None:
        clean_changes, message = _validate_price_changes(
            {k: v for k, v in legacy_usd.items()} if isinstance(legacy_usd, dict) else None
        )
        if message:
            return jsonify({"error": message}), 400
        if not clean_changes or any(v is None for v in clean_changes.values()):
            return jsonify({"error": "usd 必须是非空价格对象"}), 400
        if new_usd != clean_changes:
            applied = len(set(new_usd) ^ set(clean_changes)) + sum(
                1 for k, v in clean_changes.items() if new_usd.get(k) != v
            )
            new_usd = clean_changes
    else:
        return jsonify({"error": "缺少 changes（或旧协议 usd）"}), 400

    changed_meta = False
    new_hidden = current["hidden"]
    if "hidden" in data:
        clean_hidden, message = _validate_hidden(data.get("hidden"))
        if message:
            return jsonify({"error": message}), 400
        if clean_hidden != new_hidden:
            new_hidden = clean_hidden
            changed_meta = True

    new_fx = current["fx_rate"]
    if "fx_rate" in data:
        clean_fx, message = _validate_fx_rate(data.get("fx_rate"))
        if message:
            return jsonify({"error": message}), 400
        if clean_fx != new_fx:
            new_fx = clean_fx
            changed_meta = True

    if applied == 0 and not changed_meta:
        return _pricing_response(current, applied=0, concurrent=concurrent)

    table = {
        "revision": current["revision"] + 1,
        "updated_at": db.now_ms(),
        "updated_by": device_id,
        "usd": new_usd,
        "hidden": new_hidden,
        "fx_rate": new_fx,
    }
    db.set_config(PRICING_CONFIG_KEY, json.dumps(table, ensure_ascii=False))
    return _pricing_response(table, applied=applied, concurrent=concurrent)


@app.get("/pricing")
def get_pricing():
    """读取服务端全局价格表（桌面端合并 + 手机端只读共用）。"""
    err, _device_id = require_device_token()
    if err:
        return err
    try:
        table = _load_pricing_table()
    except (TypeError, ValueError):
        logging.exception("pricing config 解析失败")
        return jsonify({"error": "服务端价格配置损坏"}), 500
    if table is None:
        return _pricing_response(_empty_pricing_table(), synced=False)
    return _pricing_response(table)


def _resolve_device_filter(q_args, all_ids):
    """解析 devices / exclude_device 参数，得到最终要查询的设备集合。
    语义：devices 非空 → 仅查这些；否则 exclude_device 非空 → 排除这些；都空 → 全部。
    """
    devices_str = q_args.get("devices", "")
    exclude_str = q_args.get("exclude_device", "")

    if devices_str:
        want = [s.strip() for s in devices_str.split(",") if s.strip()]
        return [d for d in all_ids if d in want], True
    if exclude_str:
        exclude = [s.strip() for s in exclude_str.split(",") if s.strip()]
        return [d for d in all_ids if d not in exclude], True
    # 全部
    return list(all_ids), False


# /usage 防御常量：
# - 缺省窗口 = 近 24 小时。原缺省 from_ms=0 会让 _query_trend 从 1970 年起
#   逐桶循环（hour 桶约 50 万轮查询），是内存/CPU 放大器；
# - 最大时间窗口 400 天（对比页 90 天合法上限再放余量）；
# - 最大趋势桶数 3000：覆盖 90 天 hour 桶 = 2160 的合法最大请求，超出说明
#   窗口与桶宽不匹配，提示改用 day 桶或收窄窗口。
USAGE_DEFAULT_WINDOW_MS = 24 * 3_600_000
USAGE_MAX_WINDOW_MS = 400 * 86_400_000
USAGE_MAX_TREND_BUCKETS = 3000


@app.get("/usage")
def usage():
    """聚合查询：返回指定设备集合在时间范围内的 overall + by_model + trend。

    可选 query 参数 source（'zcode' / 'codex'）：不传 = 全部来源合并；
    by_model 每个分组带 source 字段，供前端区分展示。
    """
    err, _ = require_device_token()
    if err:
        return err

    # 缺省 from_ms = 近 24h（原缺省 0 的内存放大问题见模块常量注释）
    try:
        from_ms = int(
            request.args.get("from_ms", str(db.now_ms() - USAGE_DEFAULT_WINDOW_MS))
        )
        to_ms = int(request.args.get("to_ms", str(db.now_ms())))
    except ValueError:
        # 非数字的 from_ms/to_ms（如 ?from_ms=abc）按 4xx 风格拒绝，而非 500
        return jsonify({"error": "from_ms/to_ms 参数必须为数字"}), 400
    bucket = request.args.get("bucket", "day")
    if bucket not in ("hour", "day"):
        return jsonify({"error": "bucket 必须是 hour 或 day"}), 400
    source = (request.args.get("source") or "").strip()

    # 窗口防御：超 400 天直接拒绝，避免按超长窗口逐桶扫描
    if to_ms - from_ms > USAGE_MAX_WINDOW_MS:
        return jsonify(
            {
                "error": (
                    f"时间窗口超上限 {USAGE_MAX_WINDOW_MS // 86_400_000} 天，"
                    "请收窄 from_ms/to_ms"
                )
            }
        ), 400
    # 桶数防御：窗口与桶宽不匹配时拒绝，避免逐桶循环上万轮
    bucket_width = 3_600_000 if bucket == "hour" else 86_400_000
    if (to_ms - from_ms) // bucket_width > USAGE_MAX_TREND_BUCKETS:
        return jsonify(
            {"error": "趋势桶数超上限 3000，请改用 day 桶或收窄时间窗口"}
        ), 400

    # 取所有 device_id 作为筛选池
    all_devices = db.list_devices()
    all_ids = [d["device_id"] for d in all_devices]
    filter_ids, has_filter_param = _resolve_device_filter(request.args, all_ids)

    # 边界：显式指定了 devices/exclude_device 但过滤后为空，返回空结果而非全部
    if has_filter_param and not filter_ids:
        return jsonify(db.empty_usage_result(from_ms, to_ms))

    result = db.query_usage(from_ms, to_ms, bucket, filter_ids, source=source or None)
    return jsonify(result)


@app.get("/models")
def models():
    """全部设备、全部来源出现过的模型清单（轻量，价格配置用）。

    客户端价格设置页把清单并入本地模型列表，让"其他设备同步上来、
    本机没有"的模型也能直接配价并参与价格更新检查。
    旧客户端不调用本接口，无兼容性问题。
    """
    err, _ = require_device_token()
    if err:
        return err
    return jsonify({"models": db.list_all_models()})


def _snapshot_limit_response(count):
    """快照行数超限响应：超上限返回 400（不静默截断——截断会让客户端
    周期解析算错），未超限返回 None。"""
    if count is None or count <= db.SNAPSHOT_QUERY_MAX_ROWS:
        return None
    return (
        jsonify(
            {
                "error": (
                    f"快照行数 {count} 超上限 {db.SNAPSHOT_QUERY_MAX_ROWS}，"
                    "请收窄时间窗口"
                )
            }
        ),
        400,
    )


@app.get("/snapshots")
def snapshots():
    """额度快照查询：返回指定设备集合在时间范围内的快照（带 device_id）。

    复用 /usage 的 devices/exclude_device 过滤语义。
    用于对比页/报告页的跨设备周额度周期解析。
    """
    err, _ = require_device_token()
    if err:
        return err

    from_ms = int(request.args.get("from_ms", "0"))
    to_ms = int(request.args.get("to_ms", str(db.now_ms())))

    all_devices = db.list_devices()
    all_ids = [d["device_id"] for d in all_devices]
    filter_ids, has_filter_param = _resolve_device_filter(request.args, all_ids)

    if has_filter_param and not filter_ids:
        return jsonify({"snapshots": []})

    # 行数上限前置检查：先 COUNT 再查（旧版 db.py 无 count_snapshots 时跳过，
    # 保持宝塔热替换窗口兼容）
    if callable(getattr(db, "count_snapshots", None)):
        err = _snapshot_limit_response(db.count_snapshots(from_ms, to_ms, filter_ids))
        if err:
            return err

    snaps = db.query_snapshots(from_ms, to_ms, filter_ids)
    return jsonify({"snapshots": snaps})


@app.get("/agent-quota-snapshots")
def agent_quota_snapshots():
    """查询 Codex / Claude / Cursor 额度快照（带 device_id）。

    返回的 snapshots 保持客户端 AgentQuotaSnapshot 形状，并在顶层增加
    device_id；支持来源、设备集合和时间范围筛选。
    """
    err, _ = require_device_token()
    if err:
        return err

    # 与旧版 db.py 配合时返回空结果，不能因为新增查询接口让原有服务报 500。
    if not _agent_quota_storage_available():
        return jsonify({"snapshots": []})

    from_ms = int(request.args.get("from_ms", "0"))
    to_ms = int(request.args.get("to_ms", str(db.now_ms())))
    source = (request.args.get("source") or "").strip()

    if source and source not in db.AGENT_QUOTA_WINDOWS:
        return jsonify({"snapshots": []})

    all_devices = db.list_devices()
    all_ids = [d["device_id"] for d in all_devices]
    filter_ids, has_filter_param = _resolve_device_filter(request.args, all_ids)

    if has_filter_param and not filter_ids:
        return jsonify({"snapshots": []})

    # 行数上限前置检查：先 COUNT 原始行再查（与 /snapshots 同款；旧版 db.py
    # 无 count_agent_quota_snapshots 时跳过，保持宝塔热替换窗口兼容）
    if callable(getattr(db, "count_agent_quota_snapshots", None)):
        err = _snapshot_limit_response(
            db.count_agent_quota_snapshots(
                from_ms, to_ms, filter_ids, source=source or None
            )
        )
        if err:
            return err

    snapshots = db.query_agent_quota_snapshots(
        from_ms, to_ms, filter_ids, source=source or None
    )
    return jsonify({"snapshots": snapshots})


@app.get("/usage_records")
def usage_records():
    """逐条明细查询：返回指定设备集合在时间范围内的 total + rows + by_model。

    请求日志页「全部设备」口径用。客户端本机记录本地直读，远端查询恒带
    exclude_device=<本机 device_id>（与 /usage 的 exclude_device 语义一致），
    两集合不相交，客户端直接拼接即可，无需去重。
    query 参数：
    - from_ms / to_ms：半开区间 [from_ms, to_ms)，毫秒时间戳；
    - source：来源过滤（可选，如 'zcode' / 'codex'），不传 = 全部来源；
    - model_id：模型 id 精确过滤（可选）；
    - devices / exclude_device：设备集合过滤（复用 _resolve_device_filter）；
    - limit / offset：行数分页，limit 夹紧上限 5000。
    rows 按 started_at DESC 返回（同毫秒 device_id/source/local_rowid 稳定序）；
    by_model 按 (source, provider_id, model_id) 分组，供客户端把远端汇总按
    真实 source 归并进对应来源路做成本折算与分源缓存命中率。
    与旧版 db.py 配合（宝塔热替换窗口）时返回空结果而不是 500，客户端据此
    降级为纯本机口径。
    """
    err, _ = require_device_token()
    if err:
        return err

    if not _usage_records_query_available():
        return jsonify({"total": 0, "rows": [], "by_model": []})

    from_ms = int(request.args.get("from_ms", "0"))
    to_ms = int(request.args.get("to_ms", str(db.now_ms())))
    source = (request.args.get("source") or "").strip()
    model_id = (request.args.get("model_id") or "").strip()
    limit = request.args.get("limit", "500")
    offset = request.args.get("offset", "0")

    all_devices = db.list_devices()
    all_ids = [d["device_id"] for d in all_devices]
    filter_ids, has_filter_param = _resolve_device_filter(request.args, all_ids)

    # 边界：显式指定了 devices/exclude_device 但过滤后为空，返回空结果而非全部
    if has_filter_param and not filter_ids:
        return jsonify({"total": 0, "rows": [], "by_model": []})

    return jsonify(
        db.query_usage_records(
            from_ms,
            to_ms,
            filter_ids,
            source=source or None,
            model_id=model_id or None,
            limit=limit,
            offset=offset,
        )
    )


@app.post("/period_detail")
def period_detail():
    """按一组周期 [start, end) 返回远端逐条用量明细。

    供客户端用本地 peak 配置折算消耗（服务端无 peak 配置）。
    body: {periods: [[start,end],...], devices?, exclude_device?, source?}
    source 可选（'zcode' / 'codex'），不传 = 全部来源。
    """
    err, _ = require_device_token()
    if err:
        return err

    data = request.get_json(force=True)
    periods = data.get("periods", [])
    if not periods:
        return jsonify({"buckets": []})

    all_devices = db.list_devices()
    all_ids = [d["device_id"] for d in all_devices]
    filter_ids, has_filter_param = _resolve_device_filter(data, all_ids)
    if has_filter_param and not filter_ids:
        empty = [{"reset_at": s, "end_at": e, "rows": []} for s, e in periods]
        return jsonify({"buckets": empty})

    source = (data.get("source") or "").strip()
    try:
        buckets = db.query_period_detail(periods, filter_ids, source=source or None)
    except (ValueError, TypeError) as e:
        # 周期数超上限（PERIOD_DETAIL_MAX_PERIODS）等参数错误；畸形 periods
        # 载荷（如 [1] 无法解包成 [start, end)）抛 TypeError，同样转 400
        return jsonify({"error": str(e)}), 400
    return jsonify({"buckets": buckets})


@app.get("/devices")
def devices():
    """列出所有设备（附各设备记录数）。"""
    err, _ = require_device_token()
    if err:
        return err
    return jsonify(db.list_devices())


@app.post("/device/revoke")
def revoke():
    """撤销设备（删设备记录 + 明细）。master token 鉴权。"""
    data = request.get_json(force=True)
    err = require_master_token(data)
    if err:
        return err
    device_id = data.get("device_id", "")
    if not device_id:
        return jsonify({"error": "device_id 缺失"}), 400
    devs, recs = db.revoke_device(device_id)
    return jsonify({"devices_deleted": devs, "records_deleted": recs})


@app.post("/device/merge")
def merge():
    """合并设备：把来源设备数据并入目标设备，再删除来源。master token 鉴权。"""
    data = request.get_json(force=True)
    err = require_master_token(data)
    if err:
        return err
    source = (data.get("source_device_id") or "").strip()
    target = (data.get("target_device_id") or "").strip()
    if not source or not target:
        return jsonify({"error": "source_device_id / target_device_id 缺失"}), 400
    try:
        recs, snaps = db.merge_devices(source, target)
    except ValueError as e:
        return jsonify({"error": str(e)}), 400
    return jsonify({"records_moved": recs, "snapshots_moved": snaps})


@app.post("/device/rename")
def rename():
    """修改设备显示名。master token 鉴权。"""
    data = request.get_json(force=True)
    err = require_master_token(data)
    if err:
        return err
    device_id = (data.get("device_id") or "").strip()
    name = (data.get("device_name") or "").strip()
    if not device_id:
        return jsonify({"error": "device_id 缺失"}), 400
    if not name:
        return jsonify({"error": "设备名称不能为空"}), 400
    if len(name) > 32:
        return jsonify({"error": "设备名称过长（最多 32 字符）"}), 400
    updated = db.rename_device(device_id, name)
    return jsonify({"updated": updated})


# ===== 清理接口 =====

def _load_cleanup_config():
    """读取自动清理配置（解析失败用默认值）。"""
    raw = db.get_config(CLEANUP_CONFIG_KEY)
    if raw:
        try:
            cfg = json.loads(raw)
            return {
                "auto_enabled": bool(cfg.get("auto_enabled", False)),
                "auto_days": int(cfg.get("auto_days", 0)),
            }
        except (json.JSONDecodeError, ValueError):
            pass
    return {"auto_enabled": False, "auto_days": 0}


def _save_cleanup_config(cfg):
    db.set_config(CLEANUP_CONFIG_KEY, json.dumps(cfg))


def _purge_unattributed_snapshots():
    """删除无账号指纹的 Z.ai 额度快照，返回删除条数。

    account 为空串 = 旧客户端不传该字段，或切号瞬间客户端取不到指纹的采样。
    两类都无法归属、不参与统计，只会在对比页多出一条"旧快照"系列。清理幂等；
    旧版 db.py 没有该函数时静默跳过（宝塔热替换窗口兼容）。
    """
    fn = getattr(db, "delete_unattributed_snapshots", None)
    if not callable(fn):
        return 0
    n = fn()
    if n > 0:
        print(f"[zbar-sync] 清理无归属额度快照：删除 {n} 条")
    return n


def _spawn_auto_cleanup():
    """启动自动清理后台线程：每 24 小时检查一次。"""

    def worker():
        while True:
            # 无归属快照不属于保留期策略：每次启动/每日都清一遍（幂等）。
            # 新客户端已不再产生这类采样，这里只清历史存量与旧客户端上传。
            _purge_unattributed_snapshots()
            cfg = _load_cleanup_config()
            if cfg["auto_enabled"] and cfg["auto_days"] > 0:
                cutoff = db.now_ms() - cfg["auto_days"] * 86_400_000
                n = db.delete_before(cutoff)
                if n > 0:
                    print(
                        f"[zbar-sync] 自动清理：删除 {n} 条 "
                        f"{cfg['auto_days']} 天前的数据"
                    )
            time.sleep(24 * 3600)

    threading.Thread(target=worker, daemon=True).start()


@app.post("/cleanup")
def cleanup():
    """执行清理（master token 鉴权）。"""
    data = request.get_json(force=True)
    err = require_master_token(data)
    if err:
        return err

    action = data.get("action", "")
    if action == "device":
        device_id = data.get("device_id", "")
        if not device_id:
            return jsonify({"error": "device_id 缺失"}), 400
        n = db.delete_device_records(device_id)
        return jsonify({"action": "device", "records_deleted": n, "devices_deleted": None})

    if action == "before":
        days = data.get("days", 0)
        if not days or days <= 0:
            return jsonify({"error": "days 必须大于 0"}), 400
        cutoff = db.now_ms() - int(days) * 86_400_000
        n = db.delete_before(cutoff)
        return jsonify({"action": "before", "records_deleted": n, "devices_deleted": None})

    if action == "all":
        n = db.delete_all_usage()
        return jsonify({"action": "all", "records_deleted": n, "devices_deleted": None})

    if action == "unattributed":
        n = _purge_unattributed_snapshots()
        return jsonify(
            {"action": "unattributed", "records_deleted": n, "devices_deleted": None}
        )

    if action == "reset":
        u, d = db.reset_all()
        return jsonify({"action": "reset", "records_deleted": u, "devices_deleted": d})

    return (
        jsonify(
            {"error": f"未知 action: {action}（应为 device/before/all/unattributed/reset）"}
        ),
        400,
    )


@app.get("/cleanup/status")
def cleanup_status():
    """查询清理状态（device_token 鉴权）。"""
    err, _ = require_device_token()
    if err:
        return err
    return jsonify(
        {
            "total_records": db.total_records(),
            "devices": db.list_devices(),
            "auto_config": _load_cleanup_config(),
        }
    )


@app.post("/cleanup/config")
def cleanup_config():
    """设置自动清理配置（master token 鉴权）。"""
    data = request.get_json(force=True)
    err = require_master_token(data)
    if err:
        return err
    cfg = {
        "auto_enabled": bool(data.get("auto_enabled", False)),
        "auto_days": int(data.get("auto_days", 0)),
    }
    _save_cleanup_config(cfg)
    return jsonify(cfg)


# ===== 健康检查 =====

@app.get("/health")
def health():
    return "ok"


# ===== 资源库（AgentHUD 子代理与 Skill 管理，契约 §8）=====
#
# 契约见 doc/agent-assets-contract.md：服务端只做存储、结构校验（schema、
# 条目与内容一一对应、逐文件 size/sha256、双指纹复验）、路径安全与容量
# 限制，绝不执行/渲染任何包内容。6 个端点全部走设备鉴权；非 2xx 一律用
# §4.2 统一错误体。

AGENT_ASSET_API_VERSION = 1

# §8.7 默认限额；可由 config 表 key=agent_asset_limits 的同名正整数项覆盖，
# 并通过 GET /agent-assets/capabilities 如实返回（读取即时生效）。
AGENT_ASSET_LIMIT_DEFAULTS = {
    "max_request_bytes": 26_214_400,
    "max_file_bytes": 8_388_608,
    "max_bundle_bytes": 16_777_216,
    "max_entries_per_bundle": 200,
    "max_revisions_per_asset": 20,
    "library_quota_bytes": 268_435_456,
}
AGENT_ASSET_LIMITS_CONFIG_KEY = "agent_asset_limits"

AGENT_ASSET_HEX_RE = re.compile(r"^[0-9a-f]{64}$")
AGENT_ASSET_TOOL_RE = re.compile(r"^[a-z0-9][a-z0-9_-]{0,63}$")
AGENT_ASSET_SHORT_NAME_RE = re.compile(r"^.*~[0-9]+$", re.IGNORECASE)
AGENT_ASSET_RESERVED_CHARS = frozenset('<>"|?*')
AGENT_ASSET_RESERVED_NAMES = frozenset(
    ["con", "prn", "aux", "nul"]
    + [f"com{i}" for i in range(1, 10)]
    + [f"lpt{i}" for i in range(1, 10)]
)


def _asset_error(code, status, message, details=None):
    """契约 §4.2 统一错误体：{"error": {code, message, details?}}。"""
    error = {"code": code, "message": message}
    if details is not None:
        error["details"] = details
    return jsonify({"error": error}), status


def _asset_validation_error(message, details=None):
    return _asset_error("VALIDATION_FAILED", 400, message, details)


def _asset_path_error(path, reason):
    """§9 路径违规：details.path 指明违规条目。"""
    return _asset_error("PATH_UNSAFE", 400, f"包内路径不安全：{reason}", {"path": path})


def _agent_asset_storage_available():
    """当前加载的 db.py 是否支持资源库（宝塔热替换窗口兼容）。"""
    return all(
        callable(getattr(db, name, None))
        for name in (
            "insert_agent_asset",
            "update_agent_asset",
            "query_agent_assets",
            "query_agent_asset_revisions",
            "get_agent_asset_bundle",
            "get_agent_asset",
        )
    )


def _require_agent_asset_storage():
    """db.py 缺资源库能力时返回 500 错误响应，否则 None。"""
    if _agent_asset_storage_available():
        return None
    return _asset_error(
        "INTERNAL", 500, "服务端资源库未初始化（db.py 版本过旧，请同步升级）"
    )


def _agent_asset_limits():
    """读取资源库限额：默认值 + config 表覆盖（非法值忽略）。"""
    limits = dict(AGENT_ASSET_LIMIT_DEFAULTS)
    try:
        raw = db.get_config(AGENT_ASSET_LIMITS_CONFIG_KEY)
        overrides = json.loads(raw) if raw else None
    except (TypeError, ValueError):
        overrides = None
    if isinstance(overrides, dict):
        for key in limits:
            value = overrides.get(key)
            if isinstance(value, bool) or not isinstance(value, int) or value <= 0:
                continue
            limits[key] = value
    # 至少保留 1 个版本、至少 1 条文件，否则资源不可用
    limits["max_entries_per_bundle"] = max(1, limits["max_entries_per_bundle"])
    limits["max_revisions_per_asset"] = max(1, limits["max_revisions_per_asset"])
    return limits


def _require_agent_asset_device():
    """资源库端点鉴权：返回 (错误响应, None) 或 (None, device_id)。

    与 require_device_token 共用同一套设备模型。契约里的 FORBIDDEN（设备
    已撤销）在当前模型下不可区分：撤销设备会直接删除 devices 行，旧 token
    表现为无效凭证并返回 401 UNAUTHORIZED（见契约 §4.1 备注）。
    """
    err, device_id = require_device_token()
    if err:
        return _asset_error("UNAUTHORIZED", 401, "设备凭证缺失或无效"), None
    return None, device_id


def _check_asset_path(path):
    """契约 §9 规则 1–6：返回 None 表示安全，否则返回人读违规原因。

    大小写折叠碰撞是包级检查（同包内两条路径），不在此函数内。
    """
    if not isinstance(path, str) or not path:
        return "路径为空"
    try:
        encoded = path.encode("utf-8")
    except UnicodeEncodeError:
        return "路径不是合法 UTF-8"
    if len(encoded) > 512:
        return "路径超过 512 字节"
    if "\\" in path:
        return "路径只能使用 / 分隔"
    if path.startswith("/"):
        return "路径不能是绝对路径"
    for segment in path.split("/"):
        if not segment:
            return "路径包含空段"
        if segment in (".", ".."):
            return "路径包含 . 或 .. 段"
        if ":" in segment:
            return "路径包含冒号（盘符或 NTFS 数据流）"
        if any(ord(ch) < 0x20 or ord(ch) == 0x7F for ch in segment):
            return "路径包含控制字符"
        if segment.endswith(".") or segment.endswith(" "):
            return "路径段不能以点或空格结尾"
        if any(ch in AGENT_ASSET_RESERVED_CHARS for ch in segment):
            return "路径包含 Windows 保留字符"
        # 去掉首个点之前的部分做保留名比较：CON 与 CON.txt 同拒
        if segment.split(".", 1)[0].lower() in AGENT_ASSET_RESERVED_NAMES:
            return "路径段是 Windows 保留设备名"
        # 8.3 短名形态：把路径段按 `.` 切分，任一切分片匹配 `^.*~[0-9]+$`
        # （仅 ASCII 数字、大小写不敏感）即拒绝。与 Rust safety.rs 逐条一致：
        # `NAME~1`、`name~1.md`、`a.b~1.md`、`name~1.tar.gz`、`~1` 同拒。
        for part in segment.split("."):
            if AGENT_ASSET_SHORT_NAME_RE.fullmatch(part):
                return "路径段是 8.3 短名形态"
    return None


def _validate_asset_manifest(manifest, limits):
    """校验 §3.2 清单结构与 §9 路径安全。

    返回 (asset_meta, error)；成功时 error 为 None。路径条目按
    manifest.files 原始顺序校验，违规即返回 PATH_UNSAFE + details.path。
    """
    schema_version = manifest.get("schema_version")
    # 必须是 JSON 整数：1.0 / "1" / true 一律拒绝（Rust 反序列化 u32 不接受
    # 浮点；允许它们会让服务端接收 Rust 客户端无法消费的清单）。
    if type(schema_version) is not int or schema_version != 1:
        return None, _asset_validation_error(
            "不支持的 manifest.schema_version（必须是整数 1）"
        )

    tool = manifest.get("tool")
    if not isinstance(tool, str) or AGENT_ASSET_TOOL_RE.fullmatch(tool) is None:
        return None, _asset_validation_error(
            "manifest.tool 必须是 §2.1 规定的小写工具 ID"
        )
    kind = manifest.get("kind")
    if kind not in ("agent", "skill"):
        return None, _asset_validation_error("manifest.kind 必须是 agent 或 skill")
    origin_scope = manifest.get("origin_scope")
    if origin_scope not in ("user", "project"):
        return None, _asset_validation_error(
            "manifest.origin_scope 必须是 user 或 project"
        )
    native_format = manifest.get("native_format")
    if (
        not isinstance(native_format, str)
        or not native_format.strip()
        or len(native_format) > 64
    ):
        return None, _asset_validation_error(
            "manifest.native_format 必须是非空字符串（≤64 字符）"
        )
    native_id = manifest.get("native_id")
    if (
        not isinstance(native_id, str)
        or not native_id.strip()
        or len(native_id) > 255
    ):
        return None, _asset_validation_error(
            "manifest.native_id 必须是非空字符串（≤255 字符）"
        )
    display_name = manifest.get("display_name")
    if (
        not isinstance(display_name, str)
        or not display_name.strip()
        or len(display_name) > 255
    ):
        return None, _asset_validation_error(
            "manifest.display_name 必须是非空字符串（≤255 字符）"
        )
    description = manifest.get("description")
    if description is not None and not isinstance(description, str):
        return None, _asset_validation_error(
            "manifest.description 必须是字符串或 null"
        )
    target_versions = manifest.get("target_versions")
    if target_versions is not None and (
        not isinstance(target_versions, list)
        or any(not isinstance(item, str) for item in target_versions)
    ):
        return None, _asset_validation_error(
            "manifest.target_versions 必须是字符串数组"
        )

    files = manifest.get("files")
    if not isinstance(files, list) or not files:
        return None, _asset_validation_error("manifest.files 必须是非空数组")
    if len(files) > limits["max_entries_per_bundle"]:
        return None, _asset_error(
            "TOO_MANY_ENTRIES",
            400,
            f"文件条目数 {len(files)} 超过上限 "
            f"{limits['max_entries_per_bundle']}",
            {
                "entries": len(files),
                "limit": limits["max_entries_per_bundle"],
            },
        )

    paths = []
    total_size = 0
    for index, item in enumerate(files):
        if not isinstance(item, dict):
            return None, _asset_validation_error(
                f"manifest.files[{index}] 必须是对象"
            )
        path = item.get("path")
        if not isinstance(path, str) or not path:
            return None, _asset_validation_error(
                f"manifest.files[{index}].path 必须是非空字符串"
            )
        problem = _check_asset_path(path)
        if problem:
            return None, _asset_path_error(path, problem)
        size = item.get("size")
        if isinstance(size, bool) or not isinstance(size, int) or size < 0:
            return None, _asset_validation_error(
                f"manifest.files[{index}].size 必须是非负整数",
                {"path": path},
            )
        if size > limits["max_file_bytes"]:
            return None, _asset_error(
                "PAYLOAD_TOO_LARGE",
                413,
                f"单文件大小 {size} 超过上限 {limits['max_file_bytes']} 字节",
                {"path": path, "size": size, "limit": limits["max_file_bytes"]},
            )
        sha256 = item.get("sha256")
        if not isinstance(sha256, str) or AGENT_ASSET_HEX_RE.fullmatch(sha256) is None:
            return None, _asset_validation_error(
                f"manifest.files[{index}].sha256 必须是 64 位小写 hex",
                {"path": path},
            )
        for flag in ("executable", "binary"):
            # 五字段全部必填：缺省不能按 false 补默认，否则 Rust 结构体解析
            # （executable/binary 无 serde default）会拒绝服务端已接受的清单。
            if flag not in item:
                return None, _asset_validation_error(
                    f"manifest.files[{index}].{flag} 必填且必须是布尔值",
                    {"path": path},
                )
            if not isinstance(item[flag], bool):
                return None, _asset_validation_error(
                    f"manifest.files[{index}].{flag} 必须是布尔值",
                    {"path": path},
                )
        paths.append(path)
        total_size += size

    if total_size > limits["max_bundle_bytes"]:
        return None, _asset_error(
            "PAYLOAD_TOO_LARGE",
            413,
            f"单包总内容 {total_size} 超过上限 "
            f"{limits['max_bundle_bytes']} 字节",
            {"size": total_size, "limit": limits["max_bundle_bytes"]},
        )

    if len(set(paths)) != len(paths):
        return None, _asset_validation_error("manifest.files 存在重复路径")
    ordered = sorted(paths, key=lambda p: p.encode("utf-8"))
    if paths != ordered:
        return None, _asset_validation_error(
            "manifest.files 必须按 path 的 UTF-8 字节序升序排列"
        )
    folded = set()
    for path in paths:
        key = path.lower()
        if key in folded:
            return None, _asset_path_error(
                path, "与包内另一条路径仅大小写不同（大小写不敏感文件系统冲突）"
            )
        folded.add(key)

    entry_path = manifest.get("entry_path")
    if not isinstance(entry_path, str) or not entry_path:
        return None, _asset_validation_error(
            "manifest.entry_path 必须是非空字符串"
        )
    problem = _check_asset_path(entry_path)
    if problem:
        return None, _asset_path_error(entry_path, problem)
    if entry_path not in paths:
        return None, _asset_validation_error(
            "manifest.entry_path 必须出现在 manifest.files 中",
            {"entry_path": entry_path},
        )

    return {
        "tool": tool,
        "kind": kind,
        "native_id": native_id,
        "origin_scope": origin_scope,
        "native_format": native_format,
        "display_name": display_name,
        "description": description,
    }, None


def _parse_asset_bundle(payload, limits):
    """校验 §3 信封、逐文件 size/sha256、§3.3 双指纹与 §9 路径安全。

    返回 (context, error)：context 含 asset_meta / revision_meta /
    bundle_blob（原信封 JSON 字节），error 为 None；失败时 context 为 None。
    """
    bundle = payload.get("bundle")
    if not isinstance(bundle, dict):
        return None, _asset_validation_error("请求体缺少 bundle 对象")
    manifest = bundle.get("manifest")
    if not isinstance(manifest, dict):
        return None, _asset_validation_error("bundle.manifest 必须是对象")

    asset_meta, err = _validate_asset_manifest(manifest, limits)
    if err:
        return None, err

    files = manifest["files"]
    contents = bundle.get("contents")
    if not isinstance(contents, list) or len(contents) != len(files):
        return None, _asset_validation_error(
            "bundle.contents 必须与 manifest.files 一一对应（数量一致）"
        )

    decoded = []
    for index, (item, meta) in enumerate(zip(contents, files)):
        if not isinstance(item, dict):
            return None, _asset_validation_error(
                f"bundle.contents[{index}] 必须是对象"
            )
        path = meta["path"]
        if item.get("path") != path:
            return None, _asset_validation_error(
                f"bundle.contents[{index}].path 与 manifest.files 不同序/不一致",
                {"expected": path, "actual": item.get("path")},
            )
        encoded = item.get("content_base64")
        if not isinstance(encoded, str):
            return None, _asset_validation_error(
                f"bundle.contents[{index}].content_base64 必须是字符串",
                {"path": path},
            )
        try:
            raw = base64.b64decode(encoded, validate=True)
        except (TypeError, ValueError):
            return None, _asset_validation_error(
                f"bundle.contents[{index}].content_base64 不是合法 base64",
                {"path": path},
            )
        size = meta["size"]
        if len(raw) != size:
            return None, _asset_validation_error(
                "文件实际大小与清单 size 不一致",
                {"path": path, "expected": size, "actual": len(raw)},
            )
        if hashlib.sha256(raw).hexdigest() != meta["sha256"]:
            return None, _asset_validation_error(
                "文件 sha256 复验失败", {"path": path}
            )
        decoded.append((path, raw))

    manifest_sha256 = bundle.get("manifest_sha256")
    bundle_sha256 = bundle.get("bundle_sha256")
    if (
        not isinstance(manifest_sha256, str)
        or AGENT_ASSET_HEX_RE.fullmatch(manifest_sha256) is None
    ):
        return None, _asset_validation_error(
            "bundle.manifest_sha256 必须是 64 位小写 hex"
        )
    if (
        not isinstance(bundle_sha256, str)
        or AGENT_ASSET_HEX_RE.fullmatch(bundle_sha256) is None
    ):
        return None, _asset_validation_error(
            "bundle.bundle_sha256 必须是 64 位小写 hex"
        )

    # §3.3：清单规范 JSON（键排序、紧凑分隔符、UTF-8）的 SHA-256。
    # 对**收到的原始 manifest** 计算（含未知字段、保留缺省形态），客户端解析
    # 侧按同一原文复验；打包侧的自有清单按结构体序列化，两者逐位一致。
    try:
        canonical = json.dumps(
            manifest, ensure_ascii=False, separators=(",", ":"), sort_keys=True
        ).encode("utf-8")
    except (TypeError, ValueError):
        return None, _asset_validation_error("manifest 包含无法编码的 Unicode 字符")
    actual_manifest_sha256 = hashlib.sha256(canonical).hexdigest()
    if actual_manifest_sha256 != manifest_sha256:
        return None, _asset_validation_error(
            "manifest_sha256 复验失败",
            {"expected": manifest_sha256, "actual": actual_manifest_sha256},
        )

    # §3.3：按 path 升序逐条喂入 path 字节 + 0x00 + 原始内容字节
    digest = hashlib.sha256()
    for path, raw in decoded:
        digest.update(path.encode("utf-8"))
        digest.update(b"\x00")
        digest.update(raw)
    actual_bundle_sha256 = digest.hexdigest()
    if actual_bundle_sha256 != bundle_sha256:
        return None, _asset_validation_error(
            "bundle_sha256 复验失败",
            {"expected": bundle_sha256, "actual": actual_bundle_sha256},
        )

    return {
        "asset_meta": asset_meta,
        "revision_meta": {
            "manifest_sha256": manifest_sha256,
            "bundle_sha256": bundle_sha256,
            "total_size": sum(len(raw) for _path, raw in decoded),
            "entry_count": len(files),
        },
        # 原信封按收到的键序整体落库，下载时原样返回
        "bundle_blob": json.dumps(
            bundle, ensure_ascii=False, separators=(",", ":")
        ).encode("utf-8"),    }, None


def _read_asset_json_body(limits):
    """读取并解析请求体；超过 max_request_bytes 先拒收（413）。

    content_length 存在时先做前置判断避免读取超大 body；chunked 等无长度
    场景读取后再校验实际字节数。解析失败一律 VALIDATION_FAILED。
    """
    max_bytes = limits["max_request_bytes"]
    content_length = request.content_length
    if content_length is not None and content_length > max_bytes:
        return None, _asset_error(
            "PAYLOAD_TOO_LARGE",
            413,
            f"请求体 {content_length} 超过上限 {max_bytes} 字节",
            {"size": content_length, "limit": max_bytes},
        )
    raw = request.get_data(cache=True)
    if len(raw) > max_bytes:
        return None, _asset_error(
            "PAYLOAD_TOO_LARGE",
            413,
            f"请求体 {len(raw)} 超过上限 {max_bytes} 字节",
            {"size": len(raw), "limit": max_bytes},
        )
    try:
        text = raw.decode("utf-8")
        payload = json.loads(text)
        # json.loads 接受 `\ud800` 这类孤立代理项转义，后续 ensure_ascii=False
        # 落库/复验时会抛 UnicodeEncodeError（Flask 层变成 HTML 500）。这里
        # 先做一次 UTF-8 编码探测，统一折成 §4.2 的 VALIDATION_FAILED。
        json.dumps(payload, ensure_ascii=False).encode("utf-8")
    except (UnicodeDecodeError, ValueError):
        return None, _asset_validation_error(
            "请求体必须是合法 UTF-8 JSON（字符串不得包含非法 Unicode）"
        )
    if not isinstance(payload, dict):
        return None, _asset_validation_error("请求体必须是 JSON 对象")
    return payload, None


@app.get("/agent-assets/capabilities")
def agent_asset_capabilities():
    """契约 §8.1：能力与限额探测端点（旧服务端无此路由）。"""
    err, _device_id = _require_agent_asset_device()
    if err:
        return err
    err = _require_agent_asset_storage()
    if err:
        return err
    return jsonify(
        {"api_version": AGENT_ASSET_API_VERSION, "limits": _agent_asset_limits()}
    )


@app.get("/agent-assets")
def list_agent_assets():
    """契约 §8.2：资源列表（仅元数据）+ 精确筛选 + q 子串 + 分页。"""
    err, _device_id = _require_agent_asset_device()
    if err:
        return err
    err = _require_agent_asset_storage()
    if err:
        return err

    tool = (request.args.get("tool") or "").strip() or None
    kind = (request.args.get("kind") or "").strip() or None
    scope = (request.args.get("scope") or "").strip() or None
    q = (request.args.get("q") or "").strip() or None
    try:
        page = int(request.args.get("page", "1"))
        page_size = int(request.args.get("page_size", "20"))
    except (TypeError, ValueError):
        return _asset_validation_error("page / page_size 必须是整数")
    if page < 1:
        return _asset_validation_error("page 必须 ≥ 1")
    if page_size < 1 or page_size > 100:
        return _asset_validation_error("page_size 必须在 1–100 之间")
    return jsonify(
        db.query_agent_assets(
            tool=tool, kind=kind, scope=scope, q=q, page=page, page_size=page_size
        )
    )


@app.post("/agent-assets")
def create_agent_asset():
    """契约 §8.3/§8.6：创建资源（单事务内校验、幂等、去重、写入版本 1）。"""
    err, device_id = _require_agent_asset_device()
    if err:
        return err
    err = _require_agent_asset_storage()
    if err:
        return err

    limits = _agent_asset_limits()
    payload, err = _read_asset_json_body(limits)
    if err:
        return err
    client_request_id = payload.get("client_request_id")
    if (
        not isinstance(client_request_id, str)
        or not client_request_id.strip()
        or len(client_request_id) > 128
    ):
        return _asset_validation_error(
            "client_request_id 必须是非空字符串（≤128 字符）"
        )
    client_request_id = client_request_id.strip()

    # 先查幂等缓存：断网重试直接返回首次结果，不再做内容校验/写入
    cached = db.get_agent_asset_idempotent_result(device_id, client_request_id)
    if cached is not None:
        return jsonify(cached)

    context, err = _parse_asset_bundle(payload, limits)
    if err:
        return err
    try:
        result = db.insert_agent_asset(
            device_id,
            context["asset_meta"],
            context["revision_meta"],
            context["bundle_blob"],
            client_request_id,
            db.now_ms(),
            limits["library_quota_bytes"],
        )
    except db.AgentAssetQuotaExceeded as exc:
        return _asset_error(
            "QUOTA_EXCEEDED",
            507,
            f"资源库总容量超限（已用 {exc.used_bytes} / 上限 "
            f"{exc.quota_bytes} 字节）",
            {
                "used_bytes": exc.used_bytes,
                "quota_bytes": exc.quota_bytes,
                "needed_bytes": exc.needed_bytes,
            },
        )
    return jsonify(result)


@app.put("/agent-assets/<asset_id>")
def update_agent_asset(asset_id):
    """契约 §8.4：expected_version 乐观并发 + 追加新版本 + 最旧版本淘汰。"""
    err, device_id = _require_agent_asset_device()
    if err:
        return err
    err = _require_agent_asset_storage()
    if err:
        return err

    limits = _agent_asset_limits()
    payload, err = _read_asset_json_body(limits)
    if err:
        return err
    expected_version = payload.get("expected_version")
    if (
        isinstance(expected_version, bool)
        or not isinstance(expected_version, int)
        or expected_version < 1
    ):
        return _asset_validation_error("expected_version 必须是正整数")

    context, err = _parse_asset_bundle(payload, limits)
    if err:
        return err

    current = db.get_agent_asset(asset_id)
    if current is None:
        return _asset_error(
            "NOT_FOUND", 404, "资源不存在", {"asset_id": asset_id}
        )
    if expected_version != current["latest_version"]:
        return _asset_error(
            "VERSION_CONFLICT",
            409,
            f"预期版本 {expected_version}，服务端当前为 "
            f"{current['latest_version']}",
            {"current_version": current["latest_version"]},
        )
    # 身份字段不可变：PUT 只追加版本，不允许借更新改写资源身份
    for field, label in (
        ("tool", "工具"),
        ("kind", "资源类型"),
        ("native_id", "原生标识"),
        ("origin_scope", "作用域"),
    ):
        if context["asset_meta"][field] != current[field]:
            return _asset_validation_error(
                f"更新不能修改资源的{label}（{field}）",
                {
                    "field": field,
                    "current": current[field],
                    "request": context["asset_meta"][field],
                },
            )

    # 相同内容去重（契约 §8.6）：本次信封与该资源当前 latest 的双指纹完全
    # 一致时不新增版本（避免无意义的版本膨胀挤掉历史）。
    if (
        context["revision_meta"]["bundle_sha256"]
        == current["latest_bundle_sha256"]
        and context["revision_meta"]["manifest_sha256"]
        == current["latest_manifest_sha256"]
    ):
        revisions = db.query_agent_asset_revisions(asset_id) or []
        revision = next(
            (r for r in revisions if r["version"] == current["latest_version"]),
            None,
        )
        if revision is not None:
            return jsonify(
                {
                    "asset": current,
                    "revision": revision,
                    "replayed": False,
                    "deduplicated": True,
                }
            )

    try:
        result = db.update_agent_asset(
            asset_id,
            device_id,
            expected_version,
            context["asset_meta"],
            context["revision_meta"],
            context["bundle_blob"],
            db.now_ms(),
            limits["max_revisions_per_asset"],
            limits["library_quota_bytes"],
        )
    except db.AgentAssetNotFound:
        return _asset_error(
            "NOT_FOUND", 404, "资源不存在", {"asset_id": asset_id}
        )
    except db.AgentAssetVersionConflict as exc:
        return _asset_error(
            "VERSION_CONFLICT",
            409,
            f"预期版本 {expected_version}，服务端当前为 "
            f"{exc.current_version}",
            {"current_version": exc.current_version},
        )
    except db.AgentAssetQuotaExceeded as exc:
        return _asset_error(
            "QUOTA_EXCEEDED",
            507,
            f"资源库总容量超限（已用 {exc.used_bytes} / 上限 "
            f"{exc.quota_bytes} 字节）",
            {
                "used_bytes": exc.used_bytes,
                "quota_bytes": exc.quota_bytes,
                "needed_bytes": exc.needed_bytes,
            },
        )
    return jsonify(result)


@app.get("/agent-assets/<asset_id>/revisions")
def list_agent_asset_revisions(asset_id):
    """契约 §8.5：版本列表（version 降序）。"""
    err, _device_id = _require_agent_asset_device()
    if err:
        return err
    err = _require_agent_asset_storage()
    if err:
        return err
    items = db.query_agent_asset_revisions(asset_id)
    if items is None:
        return _asset_error(
            "NOT_FOUND", 404, "资源不存在", {"asset_id": asset_id}
        )
    return jsonify({"items": items})


@app.get("/agent-assets/<asset_id>/revisions/<rev>/bundle")
def get_agent_asset_bundle(asset_id, rev):
    """契约 §8.5：返回指定版本完整 AssetBundle 原信封（含内容）。"""
    err, _device_id = _require_agent_asset_device()
    if err:
        return err
    err = _require_agent_asset_storage()
    if err:
        return err
    try:
        version = int(rev)
    except (TypeError, ValueError):
        return _asset_validation_error("版本号必须是整数")
    if version < 1:
        return _asset_validation_error("版本号必须 ≥ 1")
    blob = db.get_agent_asset_bundle(asset_id, version)
    if blob is None:
        return _asset_error(
            "NOT_FOUND",
            404,
            "资源或版本不存在",
            {"asset_id": asset_id, "version": version},
        )
    return Response(blob, mimetype="application/json")


# ===== 启动入口 =====

def _setup_waitress_logging():
    """配置 waitress 日志，并返回包了访问日志中间件的 WSGI app。

    waitress 3.x 实测（3.0.2 源码 utilities.py）只有两个内置 logger：
    - "waitress"：错误日志（socket 异常、任务异常等）
    - "waitress.queue"：线程池队列告警（propagate 到父 logger，配好
      "waitress" 即可一并输出）
    它没有内置访问日志（不存在 waitress.access logger，serve() 也无
    logger=/ident= 参数），官方推荐用 WSGI 中间件补。这里给 "waitress"
    挂 WARNING 级 StreamHandler 输出错误与队列告警，另用一个轻量中间件
    在每请求完成后向 "waitress.access" logger 记一条访问行，格式复刻
    Werkzeug dev server：`IP - - [21/Oct/2026 10:00:00] "GET /x HTTP/1.1" 200 123`。
    """
    error_log = logging.getLogger("waitress")
    error_handler = logging.StreamHandler()
    error_handler.setFormatter(
        logging.Formatter("%(asctime)s [waitress] %(levelname)s %(message)s")
    )
    error_log.setLevel(logging.WARNING)
    error_log.addHandler(error_handler)

    access_log = logging.getLogger("waitress.access")
    access_handler = logging.StreamHandler()
    access_handler.setFormatter(logging.Formatter("%(message)s"))
    access_log.setLevel(logging.INFO)
    # 关闭向父 logger "waitress" 的传播：避免访问行再被 "waitress" 的
    # handler 输出一次造成重复
    access_log.propagate = False
    access_log.addHandler(access_handler)

    def access_log_middleware(environ, start_response):
        """访问日志中间件：响应发送完毕后记录一行（含中途断连的已发送量）。"""
        status_code = "500"
        size = "-"

        def _start_response(status, response_headers, exc_info=None):
            nonlocal status_code, size
            status_code = status.split(" ", 1)[0]
            for name, value in response_headers:
                if name.lower() == "content-length":
                    size = str(value)
                    break
            return start_response(status, response_headers, exc_info)

        app_iter = app.wsgi_app(environ, _start_response)
        sent = 0
        try:
            for chunk in app_iter:
                sent += len(chunk)
                yield chunk
        finally:
            if hasattr(app_iter, "close"):
                app_iter.close()
            if size == "-" and sent:
                # 流式响应无 Content-Length，用实际发送字节数
                size = str(sent)
            path = environ.get("PATH_INFO", "")
            query = environ.get("QUERY_STRING", "")
            if query:
                path = f"{path}?{query}"
            access_log.info(
                '%s - - [%s] "%s %s %s" %s %s',
                environ.get("REMOTE_ADDR", "-"),
                time.strftime("%d/%b/%Y %H:%M:%S"),
                environ.get("REQUEST_METHOD", "-"),
                path,
                environ.get("SERVER_PROTOCOL", "HTTP/1.1"),
                status_code,
                size,
            )

    return access_log_middleware


def main():
    global MASTER_TOKEN

    # 1. 初始化数据库（自动建库建表）
    db.init_db()

    # 2. master token：读取或生成
    MASTER_TOKEN = load_or_create_master_token()

    # 3. 打印启动信息
    print("[zbar-sync] 初始化完成")
    print(f"[zbar-sync] MASTER_TOKEN: {MASTER_TOKEN}")
    print("[zbar-sync]   ↑ 复制此 token 到客户端「同步设置」注册设备")

    print(f"[zbar-sync] 监听端口: {PORT}")

    # 4. 启动自动清理后台线程
    _spawn_auto_cleanup()

    # 5. 启动 HTTP 服务：优先 waitress（生产级 WSGI 服务器：固定线程池 +
    #    channel_timeout 掐慢连接；Werkzeug dev server 无超时、无并发上限，
    #    慢请求线程会无限堆积，大窗口查询并发时曾把服务器内存打爆）。
    #    未安装 waitress 时回退 app.run，仅限本地调试。
    try:
        from waitress import serve
    except ImportError:
        print(
            "[zbar-sync] 未安装 waitress，回退 Flask dev server"
            "（仅限调试；生产环境请 pip3 install waitress）"
        )
        app.run(host=HOST, port=PORT, threaded=True)
        return
    # waitress 无内置访问日志（详见 _setup_waitress_logging），
    # 这里包一层访问日志中间件后启动
    serve(
        _setup_waitress_logging(),
        host=HOST,
        port=PORT,
        threads=16,
        channel_timeout=60,
    )


if __name__ == "__main__":
    main()
