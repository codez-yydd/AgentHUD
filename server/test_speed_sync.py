"""Origin timing roundtrip, equal-token backfill and old-client compatibility."""
import tempfile
import unittest
from pathlib import Path
import app
import db
from auth import hash_token


class SpeedSyncTest(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.old_dir, self.old_path = db.DATA_DIR, db.DB_PATH
        db.DATA_DIR = Path(self.tmp.name)
        db.DB_PATH = db.DATA_DIR / "usage.db"
        db.init_db()
        db.insert_device("origin", "origin", hash_token("test-token"), 1)
        self.client = app.app.test_client()
        self.headers = {"Authorization": "Bearer test-token"}
        self.row = dict(local_rowid=1, started_at=1793520715804, source="zcode",
                        model_id="deepseek-v4.1-flash", output_tokens=334,
                        computed_total_tokens=460334, duration_ms=4782,
                        time_to_first_token_ms=3915)

    def tearDown(self):
        db.DATA_DIR, db.DB_PATH = self.old_dir, self.old_path
        self.tmp.cleanup()

    def upload(self, evidence=None):
        row = dict(self.row)
        if evidence is not None:
            row["speed_evidence"] = evidence
        response = self.client.post("/sync", json={"records": [row]}, headers=self.headers)
        self.assertEqual(response.status_code, 200)
        return response.get_json()

    def read(self):
        response = self.client.get("/usage_records?from_ms=1793520000000", headers=self.headers)
        self.assertEqual(response.status_code, 200)
        return response.get_json()["rows"][0]

    def test_equal_token_history_backfill_and_old_client(self):
        self.upload()
        evidence = dict(version=1, span_ms=4782, quality="request_average")
        result = self.upload(evidence)
        self.assertEqual(result["proto"], 9)
        self.assertEqual(self.read()["speed_evidence"], evidence)
        self.upload()  # An older client cannot erase validated timing.
        self.assertEqual(self.read()["speed_evidence"], evidence)
        self.assertEqual(self.read()["output_tokens"], 334)

    def test_stream_and_explicit_unavailable_roundtrip(self):
        self.upload(dict(version=1, span_ms=867, quality="generation"))
        self.assertEqual(self.read()["speed_evidence"]["span_ms"], 867)
        unavailable = dict(version=1, span_ms=None, quality=None)
        self.upload(unavailable)
        self.assertEqual(self.read()["speed_evidence"], unavailable)

    def test_invalid_timing_is_not_accepted(self):
        for evidence in [dict(version=1, span_ms=-1, quality="generation"),
                         dict(version=1, span_ms=True, quality="generation"),
                         dict(version=1, span_ms=867, quality="unknown"),
                         dict(version=99, span_ms=867, quality="generation")]:
            self.upload(evidence)
            self.assertIsNone(self.read()["speed_evidence"])

    def test_schema_migration_is_idempotent(self):
        self.upload()
        with db.get_conn() as conn:
            conn.execute("ALTER TABLE usage_records DROP COLUMN speed_evidence")
            db._migrate_usage_records_speed_columns(conn)
            db._migrate_usage_records_speed_columns(conn)
        self.assertEqual(self.read()["output_tokens"], 334)

    def test_multi_page_history_enrichment_does_not_duplicate_tokens(self):
        rows = [dict(self.row, local_rowid=i) for i in range(1, 651)]
        response = self.client.post("/sync", json={"records": rows, "last_rowid": 650}, headers=self.headers)
        self.assertEqual(response.status_code, 200)
        evidence = dict(version=1, span_ms=868, quality="generation")
        for page in (rows[:500], rows[500:]):
            response = self.client.post("/sync", json={
                "records": [dict(row, speed_evidence=evidence) for row in page],
                "last_rowid": page[-1]["local_rowid"],
            }, headers=self.headers)
            self.assertEqual(response.status_code, 200)
        with db.get_conn() as conn:
            counts = conn.execute("SELECT COUNT(*), SUM(output_tokens), COUNT(speed_evidence) FROM usage_records").fetchone()
        self.assertEqual(tuple(counts), (650, 650 * 334, 650))


if __name__ == "__main__":
    unittest.main()
