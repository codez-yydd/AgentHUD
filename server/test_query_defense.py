"""查询端内存防御回归测试。

覆盖三块防御与聚合口径：
- /period_detail 按 (小时桶, 模型) 聚合（SUM + requests 计数）与周期数上限；
- /usage 缺省 from_ms、时间窗口上限、趋势桶数上限；
- /snapshots 与 /agent-quota-snapshots 的行数上限（先 COUNT 再查，超限 400）。
"""

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


class QueryDefenseTest(unittest.TestCase):
    """覆盖大窗口/大表查询的防御行为与聚合口径。"""

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

    def _record(self, rowid, started_at, model_id, input_tokens, output_tokens,
                cache_read, total):
        return {
            "local_rowid": rowid,
            "started_at": started_at,
            "model_id": model_id,
            "input_tokens": input_tokens,
            "output_tokens": output_tokens,
            "cache_read_input_tokens": cache_read,
            "computed_total_tokens": total,
        }

    # ===== /period_detail 聚合 =====

    def test_period_detail_aggregates_by_hour_and_model(self):
        # 整点起点（可被 3_600_000 整除），桶内偏移任意毫秒
        h0 = 472_222 * 3_600_000
        db.insert_usage_records(
            "device-a",
            [
                # 同小时同模型两条 → 聚成一行，requests=2，SUM 相加
                self._record(1, h0 + 1_000, "glm-4", 100, 200, 300, 600),
                self._record(2, h0 + 2_000, "glm-4", 1, 2, 3, 6),
                # 同小时另一模型 → 独立聚合行
                self._record(3, h0 + 5_000, "glm-4.5", 10, 20, 30, 60),
                # 下一小时的桶中间 → started_at 对齐到桶起点
                self._record(4, h0 + 3_600_000 + 7_000, "glm-4", 7, 8, 9, 24),
            ],
            1,
        )
        # 其他设备的记录：device 过滤后不应出现
        db.insert_usage_records(
            "device-b", [self._record(1, h0 + 3_000, "glm-4", 5, 5, 5, 15)], 1
        )
        # 历史脏数据：token 列全 NULL + 空 model_id → SUM 兜底 0，requests 照计
        conn = db.get_conn()
        try:
            conn.execute(
                """
                INSERT INTO usage_records
                    (device_id, source, local_rowid, started_at, model_id,
                     uploaded_at)
                VALUES ('device-a', 'zcode', 99, ?, NULL, 0)
                """,
                (h0 + 8_000,),
            )
            conn.commit()
        finally:
            conn.close()

        buckets = db.query_period_detail([[h0, h0 + 2 * 3_600_000]], ["device-a"])
        self.assertEqual(len(buckets), 1)
        rows = buckets[0]["rows"]
        # 形状兼容：旧六字段 + 新增 requests
        self.assertEqual(
            {key for row in rows for key in row},
            {
                "started_at", "model_id", "input_tokens", "output_tokens",
                "cache_read_tokens", "total_tokens", "requests",
            },
        )
        # 按 (hour_ms, model_id) 排序：空模型名最前
        self.assertEqual(
            [(row["started_at"], row["model_id"]) for row in rows],
            [
                (h0, ""),
                (h0, "glm-4"),
                (h0, "glm-4.5"),
                (h0 + 3_600_000, "glm-4"),
            ],
        )
        by_key = {(row["started_at"], row["model_id"]): row for row in rows}
        null_row = by_key[(h0, "")]
        self.assertEqual(
            (null_row["input_tokens"], null_row["output_tokens"],
             null_row["cache_read_tokens"], null_row["total_tokens"]),
            (0, 0, 0, 0),
        )
        self.assertEqual(null_row["requests"], 1)
        glm4 = by_key[(h0, "glm-4")]
        self.assertEqual(
            (glm4["input_tokens"], glm4["output_tokens"],
             glm4["cache_read_tokens"], glm4["total_tokens"], glm4["requests"]),
            (101, 202, 303, 606, 2),
        )
        glm45 = by_key[(h0, "glm-4.5")]
        self.assertEqual(
            (glm45["total_tokens"], glm45["requests"]), (60, 1)
        )
        next_hour = by_key[(h0 + 3_600_000, "glm-4")]
        self.assertEqual(
            (next_hour["total_tokens"], next_hour["requests"]), (24, 1)
        )

        # 端点同口径：HTTP 走一遍确认 JSON 形状
        resp = self.client.post(
            "/period_detail",
            json={"periods": [[h0, h0 + 2 * 3_600_000]], "devices": "device-a"},
            headers=self._headers(),
        )
        self.assertEqual(resp.status_code, 200)
        rows = resp.get_json()["buckets"][0]["rows"]
        self.assertEqual(len(rows), 4)
        self.assertEqual(
            next(r for r in rows if r["model_id"] == "glm-4" and r["requests"] == 2)[
                "input_tokens"
            ],
            101,
        )

    def test_period_detail_rejects_over_limit_periods(self):
        periods = [[i * 1000, i * 1000 + 1] for i in range(db.PERIOD_DETAIL_MAX_PERIODS + 1)]
        with self.assertRaises(ValueError):
            db.query_period_detail(periods, [])

        resp = self.client.post(
            "/period_detail", json={"periods": periods}, headers=self._headers()
        )
        self.assertEqual(resp.status_code, 400)
        self.assertIn("周期数", resp.get_json()["error"])

        # 边界：恰好等于上限时放行（空库返回空 rows）
        edge = periods[: db.PERIOD_DETAIL_MAX_PERIODS]
        ok = db.query_period_detail(edge, [])
        self.assertEqual(len(ok), db.PERIOD_DETAIL_MAX_PERIODS)
        self.assertTrue(all(bucket["rows"] == [] for bucket in ok))

    def test_period_detail_rejects_malformed_periods(self):
        # 畸形 periods 载荷（元素无法解包成 [start, end)）抛 TypeError，转 400
        resp = self.client.post(
            "/period_detail", json={"periods": [1]}, headers=self._headers()
        )
        self.assertEqual(resp.status_code, 400)

    # ===== /usage 参数防御 =====

    def test_usage_defaults_from_ms_to_last_24h(self):
        before = db.now_ms()
        resp = self.client.get("/usage", headers=self._headers())
        after = db.now_ms()
        self.assertEqual(resp.status_code, 200)
        # 缺省 from_ms = 请求时刻 - 24h（原缺省 0 会从 1970 年逐桶循环）
        from_ms = resp.get_json()["from_ms"]
        self.assertGreaterEqual(from_ms, before - 24 * 3_600_000)
        self.assertLessEqual(from_ms, after - 24 * 3_600_000)

    def test_usage_rejects_non_numeric_range_params(self):
        # 非数字的 from_ms / to_ms 返回 400，而非 int() 抛错变 500
        resp = self.client.get("/usage?from_ms=abc", headers=self._headers())
        self.assertEqual(resp.status_code, 400)
        self.assertIn("参数必须为数字", resp.get_json()["error"])

        resp = self.client.get(
            "/usage?from_ms=0&to_ms=xyz", headers=self._headers()
        )
        self.assertEqual(resp.status_code, 400)
        self.assertIn("参数必须为数字", resp.get_json()["error"])

    def test_usage_rejects_window_over_limit(self):
        now = db.now_ms()
        resp = self.client.get(
            f"/usage?from_ms=0&to_ms={now}", headers=self._headers()
        )
        self.assertEqual(resp.status_code, 400)
        self.assertIn("时间窗口", resp.get_json()["error"])

    def test_usage_rejects_too_many_trend_buckets(self):
        now = db.now_ms()
        # 130 天 hour 桶 = 3120 > 3000（窗口未超 400 天）
        from_ms = now - 130 * 86_400_000
        resp = self.client.get(
            f"/usage?from_ms={from_ms}&to_ms={now}&bucket=hour",
            headers=self._headers(),
        )
        self.assertEqual(resp.status_code, 400)
        self.assertIn("桶数", resp.get_json()["error"])

        # 同窗口 day 桶（130 桶）与 90 天 hour 桶（2160）仍是合法请求
        self.assertEqual(
            self.client.get(
                f"/usage?from_ms={from_ms}&to_ms={now}&bucket=day",
                headers=self._headers(),
            ).status_code,
            200,
        )
        legal_hour = now - 90 * 86_400_000
        self.assertEqual(
            self.client.get(
                f"/usage?from_ms={legal_hour}&to_ms={now}&bucket=hour",
                headers=self._headers(),
            ).status_code,
            200,
        )

    # ===== 快照行数上限 =====

    def test_snapshot_limit_covers_current_data_scale(self):
        # 上限定位是防单请求无界失控，不得卡当前数据量：90 天 2 设备约
        # 25 万行，默认上限须留约 2 倍余量（曾因 200_000 低于存量 24.8 万
        # 行导致部署即 400，此处锁定下限）。
        self.assertGreaterEqual(db.SNAPSHOT_QUERY_MAX_ROWS, 500_000)

    def test_snapshots_reject_rows_over_limit(self):
        db.insert_snapshots(
            "device-a",
            [{"ts": i * 100, "level": "免费", "weekly_pct": i} for i in range(4)],
            0,
        )
        self.assertEqual(db.count_snapshots(0, 1_000, ["device-a"]), 4)

        original = db.SNAPSHOT_QUERY_MAX_ROWS
        db.SNAPSHOT_QUERY_MAX_ROWS = 3
        try:
            resp = self.client.get(
                "/snapshots?from_ms=0&to_ms=1000", headers=self._headers()
            )
            self.assertEqual(resp.status_code, 400)
            self.assertIn("快照行数", resp.get_json()["error"])
            self.assertIn("收窄时间窗口", resp.get_json()["error"])
        finally:
            db.SNAPSHOT_QUERY_MAX_ROWS = original

        # 未超限时照常返回全量
        ok = self.client.get(
            "/snapshots?from_ms=0&to_ms=1000", headers=self._headers()
        )
        self.assertEqual(ok.status_code, 200)
        self.assertEqual(len(ok.get_json()["snapshots"]), 4)

    def test_agent_quota_snapshots_reject_rows_over_limit(self):
        snapshot = {
            "source": "codex",
            "ts": 1_000,
            "plan_type": "pro",
            "windows": [
                {"key": "hour5", "used_pct": 4.5, "reset_at": 1_100},
                {"key": "weekly", "used_pct": 8, "reset_at": 2_000},
            ],
        }
        db.insert_agent_quota_snapshots(
            "device-a", [snapshot, {**snapshot, "ts": 2_000}], 2_000
        )
        # 原始行 = 2 快照 × 2 窗口 = 4；分组后仅 2 个 snapshot 对象
        self.assertEqual(db.count_agent_quota_snapshots(0, 3_000, ["device-a"]), 4)

        original = db.SNAPSHOT_QUERY_MAX_ROWS
        db.SNAPSHOT_QUERY_MAX_ROWS = 3
        try:
            resp = self.client.get(
                "/agent-quota-snapshots?from_ms=0&to_ms=3000",
                headers=self._headers(),
            )
            self.assertEqual(resp.status_code, 400)
            self.assertIn("快照行数", resp.get_json()["error"])
        finally:
            db.SNAPSHOT_QUERY_MAX_ROWS = original

        ok = self.client.get(
            "/agent-quota-snapshots?from_ms=0&to_ms=3000", headers=self._headers()
        )
        self.assertEqual(ok.status_code, 200)
        self.assertEqual(len(ok.get_json()["snapshots"]), 2)


if __name__ == "__main__":
    unittest.main()
