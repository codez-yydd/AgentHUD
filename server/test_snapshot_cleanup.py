"""无归属额度快照清理回归测试。

覆盖：
- db.delete_unattributed_snapshots 只删 account 为空的 Z.ai 快照，
  带指纹的快照与明细用量不受影响；
- /cleanup 的 unattributed action（清理无账号指纹的历史快照）鉴权与返回。
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


class SnapshotCleanupTest(unittest.TestCase):
    """无归属快照清理：只动 quota_snapshots 里的空 account 行。"""

    def setUp(self):
        self.temp_dir = tempfile.TemporaryDirectory()
        data_dir = Path(self.temp_dir.name)
        db.DATA_DIR = data_dir
        db.DB_PATH = data_dir / "usage.db"
        db.init_db()

        db.insert_device("device-a", "设备 A", hash_token("token-a"), 1)
        app.MASTER_TOKEN = "master-token"
        self.client = app.app.test_client()

    def tearDown(self):
        self.temp_dir.cleanup()

    def _seed(self):
        """3 条快照：1 条带指纹、2 条无归属（缺失 / 空串）。"""
        db.insert_snapshots(
            "device-a",
            [
                {
                    "ts": 1000,
                    "account": "16361781344907127",
                    "level": "max",
                    "weekly_pct": 19,
                },
                {"ts": 2000, "level": "max", "weekly_pct": 19},
                {"ts": 3000, "account": "", "level": "pro", "weekly_pct": 100},
            ],
            4000,
        )
        db.insert_usage_records(
            "device-a",
            [
                {
                    "local_rowid": 1,
                    "started_at": 1000,
                    "model_id": "glm-4.6",
                    "input_tokens": 10,
                    "output_tokens": 5,
                    "cache_read_input_tokens": 0,
                    "computed_total_tokens": 15,
                }
            ],
            4000,
        )

    def test_delete_only_unattributed_snapshots(self):
        self._seed()

        deleted = db.delete_unattributed_snapshots()

        self.assertEqual(deleted, 2, "缺失与空串两种无归属形态都应清掉")
        rows = db.query_snapshots(0, 10**13, ["device-a"])
        self.assertEqual(len(rows), 1, "带指纹快照必须保留")
        self.assertEqual(rows[0]["account"], "16361781344907127")
        # 明细用量与快照清理无关，不能被顺手删掉
        usage = db.query_usage(0, 10**13, "hour", ["device-a"])
        self.assertEqual(usage["overall"]["total_tokens"], 15)

    def test_delete_is_idempotent(self):
        self._seed()

        self.assertEqual(db.delete_unattributed_snapshots(), 2)
        self.assertEqual(db.delete_unattributed_snapshots(), 0, "重复清理应为 0")

    def test_cleanup_endpoint_unattributed(self):
        self._seed()

        resp = self.client.post(
            "/cleanup",
            json={"master_token": "master-token", "action": "unattributed"},
        )

        self.assertEqual(resp.status_code, 200)
        self.assertEqual(resp.get_json()["records_deleted"], 2)
        self.assertEqual(len(db.query_snapshots(0, 10**13, ["device-a"])), 1)

    def test_cleanup_endpoint_requires_master_token(self):
        self._seed()

        resp = self.client.post(
            "/cleanup", json={"master_token": "wrong", "action": "unattributed"}
        )

        self.assertEqual(resp.status_code, 401)
        self.assertEqual(len(db.query_snapshots(0, 10**13, ["device-a"])), 3)


if __name__ == "__main__":
    unittest.main()
