"""定价同步接口（POST/GET /pricing）回归测试。"""

import json
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


class PricingSyncTest(unittest.TestCase):
    """服务端全局唯一价：增量合并、revision、删除与兼容。"""

    def setUp(self):
        self.temp_dir = tempfile.TemporaryDirectory()
        data_dir = Path(self.temp_dir.name)
        db.DATA_DIR = data_dir
        db.DB_PATH = data_dir / "usage.db"
        db.init_db()

        db.insert_device("device-a", "设备 A", hash_token("token-a"), 1)
        db.insert_device("device-b", "设备 B", hash_token("token-b"), 2)
        self.client = app.app.test_client()

    def tearDown(self):
        self.temp_dir.cleanup()

    def _headers(self, token="token-a"):
        return {"Authorization": f"Bearer {token}"}

    def _post(self, payload, token="token-a"):
        return self.client.post("/pricing", json=payload, headers=self._headers(token))

    def _get(self, token="token-a"):
        return self.client.get("/pricing", headers=self._headers(token))

    def _price(self, input_, output, cache_read=0):
        return {"input": input_, "output": output, "cache_read": cache_read}

    def test_cache_write_price_is_optional_and_round_trips(self):
        price = dict(self._price(2, 10, 0.2), cache_write=2.5)
        clean, error = app._validate_model_price("m", price)
        self.assertIsNone(error)
        self.assertEqual(clean, price)
        for value in (-1, True, float("inf")):
            self.assertIsNotNone(app._validate_model_price("m", dict(price, cache_write=value))[1])

    def _bootstrap(self):
        response = self._post({
            "changes": {
                "model-a": self._price(1, 2),
                "model-b": self._price(3, 4),
            },
            "fx_rate": 7.2,
        })
        self.assertEqual(response.status_code, 200, response.get_json())
        return response.get_json()

    def test_empty_state_is_readable_before_first_publish(self):
        response = self._get()
        self.assertEqual(response.status_code, 200)
        body = response.get_json()
        self.assertEqual(body["usd"], {})
        self.assertEqual(body["revision"], 0)
        self.assertEqual(body["hidden"], [])
        self.assertFalse(body["synced"])

    def test_bootstrap_and_read_round_trip(self):
        body = self._bootstrap()
        self.assertEqual(body["revision"], 1)
        self.assertEqual(body["model_count"], 2)
        self.assertEqual(body["applied"], 2)
        self.assertFalse(body["concurrent"])
        self.assertAlmostEqual(body["fx_rate"], 7.2)

        body = self._get().get_json()
        self.assertTrue(body["synced"])
        self.assertEqual(body["revision"], 1)
        self.assertEqual(
            body["usd"]["model-a"],
            {"input": 1.0, "output": 2.0, "cache_read": 0.0},
        )

    def test_different_models_merge_across_devices(self):
        first = self._bootstrap()
        self.assertEqual(first["revision"], 1)

        # 设备 B 基于旧 revision 改另一个模型：增量合并，不覆盖 model-a
        response = self._post({
            "base_revision": 0,
            "changes": {"model-b": self._price(30, 40)},
        }, token="token-b")
        self.assertEqual(response.status_code, 200, response.get_json())
        body = response.get_json()
        self.assertEqual(body["revision"], 2)
        self.assertTrue(body["concurrent"])
        self.assertEqual(body["usd"]["model-a"], {"input": 1.0, "output": 2.0, "cache_read": 0.0})
        self.assertEqual(body["usd"]["model-b"], {"input": 30.0, "output": 40.0, "cache_read": 0.0})

        # 设备 A 下一轮拉到合并后的表
        body = self._get().get_json()
        self.assertEqual(body["revision"], 2)
        self.assertEqual(set(body["usd"]), {"model-a", "model-b"})

    def test_same_model_last_write_wins(self):
        self._bootstrap()
        response = self._post({
            "base_revision": 1,
            "changes": {"model-a": self._price(99, 100)},
        }, token="token-b")
        body = response.get_json()
        self.assertEqual(body["usd"]["model-a"]["input"], 99.0)
        self.assertFalse(body["concurrent"])
        # 另一个模型仍在
        self.assertIn("model-b", body["usd"])

    def test_delete_model_with_null(self):
        self._bootstrap()
        response = self._post({
            "base_revision": 1,
            "changes": {"model-a": None},
        })
        body = response.get_json()
        self.assertEqual(body["revision"], 2)
        self.assertEqual(set(body["usd"]), {"model-b"})

    def test_hidden_and_fx_update_without_model_changes(self):
        self._bootstrap()
        response = self._post({
            "base_revision": 1,
            "changes": {},
            "hidden": ["model-b", "model-b"],
            "fx_rate": 7.3,
        })
        body = response.get_json()
        self.assertEqual(body["revision"], 2)
        self.assertEqual(body["hidden"], ["model-b"])
        self.assertAlmostEqual(body["fx_rate"], 7.3)

    def test_noop_does_not_bump_revision(self):
        self._bootstrap()
        response = self._post({
            "base_revision": 1,
            "changes": {"model-a": self._price(1, 2)},
            "fx_rate": 7.2,
        })
        body = response.get_json()
        self.assertEqual(body["revision"], 1)
        self.assertEqual(body["applied"], 0)
        self.assertFalse(body["concurrent"])

    def test_legacy_full_table_is_replaced(self):
        legacy = {
            "usd": {
                "legacy-a": self._price(5, 6),
                "legacy-b": self._price(7, 8),
            },
            "fx_rate": 7.1,
        }
        response = self._post(legacy)
        self.assertEqual(response.status_code, 200, response.get_json())
        body = response.get_json()
        self.assertEqual(body["revision"], 1)
        self.assertEqual(set(body["usd"]), {"legacy-a", "legacy-b"})
        # 旧表被整体替换，随后增量协议继续工作
        response = self._post({
            "base_revision": 1,
            "changes": {"legacy-c": self._price(9, 10)},
        })
        body = response.get_json()
        self.assertEqual(body["revision"], 2)
        self.assertEqual(set(body["usd"]), {"legacy-a", "legacy-b", "legacy-c"})

    def test_old_stored_format_without_revision_is_readable(self):
        db.set_config(app.PRICING_CONFIG_KEY, json.dumps({
            "usd": {"old": {"input": 1, "output": 2, "cache_read": 0}},
            "fx_rate": 7.2,
            "updated_at": 123,
            "device_id": "device-a",
        }))
        body = self._get().get_json()
        self.assertEqual(body["revision"], 0)
        self.assertEqual(body["updated_by"], "device-a")
        self.assertEqual(body["usd"]["old"]["input"], 1.0)

    def test_delete_all_models_is_allowed(self):
        self._bootstrap()
        response = self._post({
            "base_revision": 1,
            "changes": {"model-a": None, "model-b": None},
        })
        self.assertEqual(response.status_code, 200, response.get_json())
        body = response.get_json()
        self.assertEqual(body["revision"], 2)
        self.assertEqual(body["usd"], {})
        self.assertEqual(body["model_count"], 0)

    def test_requires_device_token(self):
        self.assertEqual(self.client.get("/pricing").status_code, 401)
        self.assertEqual(self.client.post("/pricing", json={"changes": {}}).status_code, 401)
        self.assertEqual(
            self.client.get("/pricing", headers=self._headers("bad-token")).status_code,
            401,
        )

    def test_rejects_invalid_payloads(self):
        self._bootstrap()
        cases = [
            {"changes": {"m": {"input": -1, "output": 1, "cache_read": 0}}},
            {"changes": {"m": {"input": True, "output": 1, "cache_read": 0}}},
            {"changes": {"m": {"input": 1, "output": 1}}},
            {"changes": {"": self._price(1, 1)}},
            {"changes": {"m": self._price(1, 1)}, "fx_rate": 0},
            {"changes": {"m": self._price(1, 1)}, "fx_rate": "7.2"},
            {"changes": {"m": self._price(1, 1)}, "fx_rate": True},
            {"changes": {"m": self._price(1, 1)}, "hidden": "not-a-list"},
            {"changes": {"m": self._price(1, 1)}, "hidden": [1]},
            {"changes": {"m": self._price(1, 1)}, "base_revision": -1},
            {"changes": {"m": self._price(1, 1)}, "base_revision": "1"},
            {"changes": {"m": self._price(1, 1)}, "usd": {"x": self._price(1, 1)}},
            {"usd": {}},
        ]
        for payload in cases:
            with self.subTest(payload=payload):
                response = self._post(payload)
                self.assertEqual(response.status_code, 400, response.get_json())

        # 非法请求不应破坏已发布的数据
        body = self._get().get_json()
        self.assertEqual(body["revision"], 1)
        self.assertEqual(set(body["usd"]), {"model-a", "model-b"})


if __name__ == "__main__":
    unittest.main()
