"""Agent 额度快照同步服务端回归测试。"""

import sys
import tempfile
import unittest
from pathlib import Path


SERVER_DIR = Path(__file__).resolve().parent
if str(SERVER_DIR) not in sys.path:
    sys.path.insert(0, str(SERVER_DIR))

import app
import db
from auth import hash_token


class AgentQuotaSyncTest(unittest.TestCase):
    """覆盖 Agent 快照的数据库和 HTTP 同步行为。"""

    def setUp(self):
        self.temp_dir = tempfile.TemporaryDirectory()
        data_dir = Path(self.temp_dir.name)
        db.DATA_DIR = data_dir
        db.DB_PATH = data_dir / "usage.db"
        db.init_db()

        db.insert_device("device-a", "设备 A", hash_token("token-a"), 1)
        db.insert_device("device-b", "设备 B", hash_token("token-b"), 2)
        app.MASTER_TOKEN = "master-token"
        self.client = app.app.test_client()

    def tearDown(self):
        self.temp_dir.cleanup()

    def _headers(self, token="token-a"):
        return {"Authorization": f"Bearer {token}"}

    def _snapshot(self, source, ts, used_pct, key="weekly", plan_type="pro"):
        return {
            "source": source,
            "ts": ts,
            "plan_type": plan_type,
            "windows": [
                {
                    "key": key,
                    "used_pct": used_pct,
                    "reset_at": 2_000_000,
                }
            ],
        }

    def test_init_is_idempotent_and_schema_is_complete(self):
        db.init_db()
        db.init_db()

        conn = db.get_conn()
        try:
            columns = {
                row["name"]: row["type"]
                for row in conn.execute(
                    "PRAGMA table_info(agent_quota_snapshots)"
                ).fetchall()
            }
            self.assertEqual(
                columns,
                {
                    "device_id": "TEXT",
                    "source": "TEXT",
                    "ts": "INTEGER",
                    "label": "TEXT",
                    "plan_type": "TEXT",
                    "plan_name": "TEXT",
                    "window_key": "TEXT",
                    "used_pct": "REAL",
                    "reset_at": "INTEGER",
                    "balance_json": "TEXT",
                    "message": "TEXT",
                },
            )
        finally:
            conn.close()

    def test_sync_accepts_agent_snapshots_and_preserves_zai_fields(self):
        payload = {
            "records": [],
            "last_rowid": 0,
            "snapshots": [
                {
                    "ts": 100,
                    "level": "免费",
                    "weekly_pct": 12,
                    "weekly_reset": 200,
                }
            ],
            "last_snapshot_ts": 100,
            "agent_quota_snapshots": [
                {
                    "source": "codex",
                    "ts": 1_000,
                    "plan_type": "pro",
                    "windows": [
                        {"key": "hour5", "used_pct": 5.5, "reset_at": 1_500},
                        {"key": "weekly", "used_pct": 10, "reset_at": 2_000},
                    ],
                }
            ],
            "last_agent_quota_snapshot_ts": 1_000,
        }

        response = self.client.post(
            "/sync", json=payload, headers=self._headers()
        )
        body = response.get_json()
        self.assertEqual(response.status_code, 200)
        # 当前服务端全能力 = proto 8（明细带耗时/首字；该断言随 proto 演进同步更新）
        self.assertEqual(body["proto"], 8)
        self.assertEqual(body["accepted"], 0)
        self.assertEqual(body["accepted_snapshots"], 1)
        self.assertEqual(body["max_snapshot_ts"], 100)
        self.assertEqual(body["accepted_agent_quota_snapshots"], 2)
        self.assertEqual(body["max_agent_quota_snapshot_ts"], 1_000)

        duplicate = self.client.post(
            "/sync", json=payload, headers=self._headers()
        ).get_json()
        self.assertEqual(duplicate["accepted_snapshots"], 0)
        self.assertEqual(duplicate["accepted_agent_quota_snapshots"], 0)

        db.insert_agent_quota_snapshots(
            "device-a", [self._snapshot("codex", 1_000, 15)], 1_000
        )
        deduped = db.query_agent_quota_snapshots(0, 2_000, ["device-a"])
        weekly = next(
            window["used_pct"]
            for snapshot in deduped
            if snapshot["source"] == "codex"
            for window in snapshot["windows"]
            if window["key"] == "weekly"
        )
        self.assertEqual(weekly, 15.0)

        old_snapshots = self.client.get(
            "/snapshots?from_ms=0&to_ms=200", headers=self._headers()
        ).get_json()["snapshots"]
        self.assertEqual(len(old_snapshots), 1)
        self.assertEqual(old_snapshots[0]["weekly_pct"], 12)
        self.assertNotIn("source", old_snapshots[0])

    def test_sync_without_agent_fields_remains_compatible(self):
        response = self.client.post(
            "/sync",
            json={"records": [], "last_rowid": 0},
            headers=self._headers(),
        )
        body = response.get_json()
        self.assertEqual(response.status_code, 200)
        # 同上：当前服务端全能力 = proto 8
        self.assertEqual(body["proto"], 8)
        self.assertEqual(body["accepted_agent_quota_snapshots"], 0)
        self.assertIsNone(body["max_agent_quota_snapshot_ts"])

    def test_invalid_agent_sources_and_percentages_are_ignored(self):
        response = self.client.post(
            "/sync",
            json={
                "records": [],
                "last_rowid": 0,
                "agent_quota_snapshots": [
                    {
                        "source": "__proto__",
                        "ts": 1_000,
                        "windows": [{"key": "weekly", "used_pct": 20}],
                    },
                    {
                        "source": "codex",
                        "ts": 1_001,
                        "windows": [
                            {"key": "weekly", "used_pct": -1},
                            {"key": "hour5", "used_pct": 101},
                        ],
                    },
                ],
            },
            headers=self._headers(),
        )
        self.assertEqual(response.status_code, 200)
        self.assertEqual(response.get_json()["accepted_agent_quota_snapshots"], 0)
        self.assertEqual(
            db.query_agent_quota_snapshots(0, 2_000, ["device-a"]), []
        )

    def test_agent_snapshot_query_groups_windows_and_filters(self):
        db.insert_agent_quota_snapshots(
            "device-a",
            [
                {
                    "source": "codex",
                    "ts": 1_000,
                    "plan_type": "pro",
                    "windows": [
                        {"key": "hour5", "used_pct": 4.5, "reset_at": 1_100},
                        {"key": "weekly", "used_pct": 8, "reset_at": 2_000},
                    ],
                },
                self._snapshot("claude", 1_200, 9),
            ],
            1_300,
        )
        db.insert_agent_quota_snapshots(
            "device-b", [self._snapshot("codex", 1_000, 12)], 1_300
        )

        response = self.client.get(
            "/agent-quota-snapshots?from_ms=1000&to_ms=1300&source=codex",
            headers=self._headers(),
        )
        snapshots = response.get_json()["snapshots"]
        self.assertEqual(response.status_code, 200)
        self.assertEqual(len(snapshots), 2)
        self.assertEqual(
            {snapshot["device_id"] for snapshot in snapshots},
            {"device-a", "device-b"},
        )
        device_a = next(
            snapshot for snapshot in snapshots if snapshot["device_id"] == "device-a"
        )
        self.assertEqual(device_a["source"], "codex")
        self.assertEqual(device_a["plan_type"], "pro")
        self.assertEqual(
            [window["key"] for window in device_a["windows"]],
            ["hour5", "weekly"],
        )

        only_a = self.client.get(
            "/agent-quota-snapshots?from_ms=0&to_ms=2000&devices=device-a",
            headers=self._headers(),
        ).get_json()["snapshots"]
        self.assertEqual({snapshot["device_id"] for snapshot in only_a}, {"device-a"})

        without_a = self.client.get(
            "/agent-quota-snapshots?from_ms=0&to_ms=2000&exclude_device=device-a",
            headers=self._headers(),
        ).get_json()["snapshots"]
        self.assertEqual(
            {snapshot["device_id"] for snapshot in without_a}, {"device-b"}
        )

    def test_cleanup_and_merge_cover_agent_snapshots(self):
        db.insert_agent_quota_snapshots(
            "device-a", [self._snapshot("codex", 10, 3)], 10
        )
        db.insert_agent_quota_snapshots(
            "device-a", [self._snapshot("codex", 20, 5)], 20
        )
        db.delete_before(20)
        self.assertEqual(
            db.query_agent_quota_snapshots(0, 100, ["device-a"])[0]["ts"], 20
        )

        db.insert_agent_quota_snapshots(
            "device-b", [self._snapshot("codex", 20, 99)], 20
        )
        db.insert_agent_quota_snapshots(
            "device-b", [self._snapshot("codex", 30, 7, key="weekly")], 30
        )
        records_moved, old_snapshots_moved = db.merge_devices(
            "device-b", "device-a"
        )
        self.assertEqual(records_moved, 0)
        self.assertEqual(old_snapshots_moved, 0)

        merged = db.query_agent_quota_snapshots(0, 100, ["device-a"])
        self.assertEqual(
            {(snapshot["ts"], snapshot["device_id"]) for snapshot in merged},
            {(20, "device-a"), (30, "device-a")},
        )
        at_conflict = next(snapshot for snapshot in merged if snapshot["ts"] == 20)
        self.assertEqual(at_conflict["windows"][0]["used_pct"], 99.0)
        self.assertEqual(
            db.query_agent_quota_snapshots(0, 100, ["device-b"]), []
        )

        db.insert_agent_quota_snapshots(
            "device-a", [self._snapshot("cursor", 40, 1)], 40
        )
        db.delete_device_records("device-a")
        self.assertEqual(
            db.query_agent_quota_snapshots(0, 100, ["device-a"]), []
        )

    def test_provider_snapshots_with_balance_and_label_round_trip(self):
        """proto 7：余额型 provider 快照（空 windows + balance）落库并回读。"""
        accepted = db.insert_agent_quota_snapshots(
            "device-a",
            [
                {
                    "source": "deepseek",
                    "ts": 5_000,
                    "label": "主账号",
                    "plan_name": "Pay-as-you-go",
                    "windows": [],
                    "balance": {
                        "amount": 110.5,
                        "currency": "CNY",
                        "granted": 10.0,
                        "topped_up": 100.5,
                    },
                }
            ],
            5_000,
        )
        self.assertEqual(accepted, 1)

        snapshots = db.query_agent_quota_snapshots(0, 9_000, ["device-a"])
        self.assertEqual(len(snapshots), 1)
        snap = snapshots[0]
        self.assertEqual(snap["source"], "deepseek")
        self.assertEqual(snap["label"], "主账号")
        self.assertEqual(snap["plan_name"], "Pay-as-you-go")
        self.assertEqual(snap["windows"], [])
        self.assertEqual(
            snap["balance"],
            {
                "amount": 110.5,
                "currency": "CNY",
                "granted": 10.0,
                "topped_up": 100.5,
            },
        )

    def test_provider_snapshots_multi_label_same_ts_do_not_collide(self):
        """同 source 同 ts 多凭证（label 维度）互不撞键，查询按账号拆分。"""
        db.insert_agent_quota_snapshots(
            "device-a",
            [
                {
                    "source": "moonshot",
                    "ts": 6_000,
                    "label": "账号一",
                    "windows": [],
                    "balance": {"amount": 1.0, "currency": "CNY"},
                },
                {
                    "source": "moonshot",
                    "ts": 6_000,
                    "label": "账号二",
                    "windows": [],
                    "balance": {"amount": 2.0, "currency": "CNY"},
                },
            ],
            6_000,
        )
        snapshots = db.query_agent_quota_snapshots(0, 9_000, ["device-a"])
        self.assertEqual({s["label"] for s in snapshots}, {"账号一", "账号二"})
        amounts = {s["label"]: s["balance"]["amount"] for s in snapshots}
        self.assertEqual(amounts, {"账号一": 1.0, "账号二": 2.0})

    def test_provider_window_snapshot_keeps_windows_and_plan(self):
        """有窗口的 provider 快照（qoder credits）窗口按白名单保留。"""
        db.insert_agent_quota_snapshots(
            "device-a",
            [
                {
                    "source": "qoder",
                    "ts": 7_000,
                    "label": "工作号",
                    "plan_name": "Pro",
                    "windows": [
                        {"key": "credits", "used_pct": 42.5, "reset_at": 7_500},
                        # 白名单外窗口被过滤
                        {"key": "unknown_window", "used_pct": 10.0},
                    ],
                }
            ],
            7_000,
        )
        snapshots = db.query_agent_quota_snapshots(0, 9_000, ["device-a"])
        self.assertEqual(len(snapshots), 1)
        self.assertEqual(len(snapshots[0]["windows"]), 1)
        self.assertEqual(snapshots[0]["windows"][0]["key"], "credits")
        self.assertEqual(snapshots[0]["plan_name"], "Pro")

    def test_error_state_snapshot_without_payload_stays_dropped(self):
        """旧客户端语义：空 windows 且不带 proto 7 载荷的快照照旧整体丢弃。"""
        accepted = db.insert_agent_quota_snapshots(
            "device-a",
            [{"source": "codex", "ts": 8_000, "windows": []}],
            8_000,
        )
        self.assertEqual(accepted, 0)

    def test_error_state_snapshot_with_message_is_stored(self):
        """错误态快照（message 有值）落哨兵行，回读供手机端展示错误原因。"""
        accepted = db.insert_agent_quota_snapshots(
            "device-a",
            [
                {
                    "source": "deepseek",
                    "ts": 8_500,
                    "label": "过期号",
                    "windows": [],
                    "message": "API Key 无效或已过期",
                }
            ],
            8_500,
        )
        self.assertEqual(accepted, 1)
        snapshots = db.query_agent_quota_snapshots(0, 9_000, ["device-a"])
        snap = next(s for s in snapshots if s["ts"] == 8_500)
        self.assertEqual(snap["windows"], [])
        self.assertEqual(snap["message"], "API Key 无效或已过期")

    def test_camel_case_balance_keys_are_accepted(self):
        """桌面端 serde camelCase（toppedUp）与 snake_case 双键兼容落库。

        历史缺陷：桌面 ProviderQuotaBalance 序列化为 toppedUp，而解析端只读
        topped_up——充值金额被静默丢弃。本用例按真实桌面载荷形态（camelCase）
        构造请求，防止测试再用 snake_case 自证掩盖键名漂移。
        """
        db.insert_agent_quota_snapshots(
            "device-a",
            [
                {
                    "source": "deepseek",
                    "ts": 9_100,
                    "label": "camelCase 键",
                    "windows": [],
                    # 桌面端 serde rename_all = "camelCase" 的真实形态
                    "balance": {
                        "amount": 88.0,
                        "currency": "CNY",
                        "granted": 8.0,
                        "toppedUp": 80.0,
                    },
                }
            ],
            9_100,
        )
        snapshots = db.query_agent_quota_snapshots(0, 9_900, ["device-a"])
        self.assertEqual(len(snapshots), 1)
        balance = snapshots[0]["balance"]
        # 回读口径统一为 snake_case
        self.assertEqual(balance["granted"], 8.0)
        self.assertEqual(balance["topped_up"], 80.0)
        self.assertNotIn("toppedUp", balance)

    def test_merge_backfills_proto7_payload_on_conflict(self):
        """设备合并冲突时：窗口行守较高用量；哨兵行的扩展载荷按 COALESCE 回填。

        哨兵行（window_key=''，used_pct=0）沿用 used_pct < ? 门控会永假——
        余额/错误原因在合并时会被静默丢掉，此处锁定回填语义。
        """
        # 目标设备先落一条无载荷的哨兵行（旧版本/部分数据）
        db.insert_agent_quota_snapshots(
            "device-a",
            [
                {
                    "source": "deepseek",
                    "ts": 9_500,
                    "label": "合并账号",
                    "windows": [],
                    "balance": {"amount": 1.0, "currency": "CNY"},
                }
            ],
            9_500,
        )
        # 手工把目标行的载荷清空，模拟「同主键但目标无扩展数据」的冲突形态
        conn = db.get_conn()
        try:
            conn.execute(
                """
                UPDATE agent_quota_snapshots
                SET balance_json = NULL, message = NULL
                WHERE device_id = 'device-a' AND ts = 9500
                """
            )
            conn.commit()
        finally:
            conn.close()

        # 来源设备同主键行带余额与错误原因
        db.insert_agent_quota_snapshots(
            "device-b",
            [
                {
                    "source": "deepseek",
                    "ts": 9_500,
                    "label": "合并账号",
                    "windows": [],
                    "balance": {"amount": 5.5, "currency": "CNY"},
                    "message": "余额暂不可用于 API 调用",
                }
            ],
            9_500,
        )
        db.merge_devices("device-b", "device-a")

        merged = db.query_agent_quota_snapshots(0, 9_900, ["device-a"])
        snap = next(s for s in merged if s["ts"] == 9_500)
        # 扩展载荷从来源行回填（目标为空时不丢）
        self.assertEqual(snap["balance"]["amount"], 5.5)
        self.assertEqual(snap["message"], "余额暂不可用于 API 调用")
        # 来源设备数据已迁走
        self.assertEqual(db.query_agent_quota_snapshots(0, 9_900, ["device-b"]), [])

    def test_legacy_schema_migrates_in_place(self):
        """旧 schema（无 label 列）的库 init_db 后自动迁移，老数据保留。"""
        # 造一个旧结构的库（proto 6 及之前的表定义）
        conn = db.get_conn()
        try:
            conn.execute("DROP TABLE agent_quota_snapshots")
            conn.execute(
                """
                CREATE TABLE agent_quota_snapshots (
                    device_id   TEXT    NOT NULL,
                    source      TEXT    NOT NULL,
                    ts          INTEGER NOT NULL,
                    plan_type   TEXT    NOT NULL DEFAULT '',
                    window_key  TEXT    NOT NULL,
                    used_pct    REAL    NOT NULL DEFAULT 0,
                    reset_at    INTEGER,
                    PRIMARY KEY (device_id, source, ts, window_key)
                )
                """
            )
            conn.execute(
                """
                INSERT INTO agent_quota_snapshots
                    (device_id, source, ts, plan_type, window_key, used_pct, reset_at)
                VALUES ('device-a', 'codex', 100, 'pro', 'weekly', 12.0, 200)
                """
            )
            conn.commit()
        finally:
            conn.close()

        db.init_db()  # 迁移 + 二次 init 幂等
        db.init_db()

        snapshots = db.query_agent_quota_snapshots(0, 1_000, ["device-a"])
        self.assertEqual(len(snapshots), 1)
        self.assertEqual(snapshots[0]["windows"][0]["used_pct"], 12.0)
        self.assertIsNone(snapshots[0]["label"])
        # 迁移后同 ts 不同 label 的新快照可继续落库
        db.insert_agent_quota_snapshots(
            "device-a",
            [
                {
                    "source": "deepseek",
                    "ts": 100,
                    "label": "新账号",
                    "windows": [],
                    "balance": {"amount": 3.0, "currency": "CNY"},
                }
            ],
            100,
        )
        merged = db.query_agent_quota_snapshots(0, 1_000, ["device-a"])
        self.assertEqual({s.get("label") for s in merged}, {None, "新账号"})


if __name__ == "__main__":
    unittest.main()
