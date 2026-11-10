"""AgentHUD 资源库（/agent-assets）服务端回归测试。

契约见 doc/agent-assets-contract.md §3/§4/§8/§9。用临时库起 Flask
test_client，覆盖创建→列表→版本→下载往返、幂等三连、乐观并发、路径
安全、限额、版本淘汰、鉴权与旧库升级。仅用标准库 + 现有依赖。
"""

import base64
import hashlib
import json
import sqlite3
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


def canonical_manifest(manifest):
    """§3.3 清单规范 JSON 字节（键排序、紧凑分隔符、UTF-8）。"""
    return json.dumps(
        manifest, ensure_ascii=False, separators=(",", ":"), sort_keys=True
    ).encode("utf-8")


def bundle_sha(entries):
    """§3.3 bundle 指纹：path 字节 + 0x00 + 原始内容，按 path 升序。"""
    digest = hashlib.sha256()
    for path, content in sorted(entries, key=lambda item: item[0].encode("utf-8")):
        digest.update(path.encode("utf-8"))
        digest.update(b"\x00")
        digest.update(content)
    return digest.hexdigest()


def make_bundle(files, **manifest_overrides):
    """构造一个指纹合法的 AssetBundle（files: {path: bytes}）。"""
    entries = sorted(files.items(), key=lambda item: item[0].encode("utf-8"))
    manifest_files = [
        {
            "path": path,
            "size": len(content),
            "sha256": hashlib.sha256(content).hexdigest(),
            "executable": path.endswith((".sh", ".bat", ".cmd", ".exe", ".ps1")),
            "binary": b"\x00" in content[:8192],
        }
        for path, content in entries
    ]
    manifest = {
        "schema_version": 1,
        "tool": "zcode",
        "kind": "skill",
        "native_format": "skill-dir",
        "native_id": "demo-skill",
        "display_name": "演示技能",
        "description": "服务端回归测试用",
        "origin_scope": "user",
        "target_versions": [],
        "entry_path": manifest_files[0]["path"],
        "files": manifest_files,
    }
    manifest.update(manifest_overrides)
    return {
        "manifest": manifest,
        "manifest_sha256": hashlib.sha256(canonical_manifest(manifest)).hexdigest(),
        "bundle_sha256": bundle_sha(entries),
        "contents": [
            {"path": path, "content_base64": base64.b64encode(content).decode("ascii")}
            for path, content in entries
        ],
    }


class AgentAssetsApiTest(unittest.TestCase):
    """6 个 /agent-assets 端点的契约行为。"""

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

    # ===== 辅助 =====

    def _headers(self, token="token-a"):
        return {"Authorization": f"Bearer {token}"}

    def _set_limits(self, **overrides):
        db.set_config(app.AGENT_ASSET_LIMITS_CONFIG_KEY, json.dumps(overrides))

    def _create(self, bundle, request_id="req-1", token="token-a"):
        return self.client.post(
            "/agent-assets",
            json={"client_request_id": request_id, "bundle": bundle},
            headers=self._headers(token),
        )

    def _update(self, asset_id, bundle, expected_version, token="token-a"):
        return self.client.put(
            f"/agent-assets/{asset_id}",
            json={"expected_version": expected_version, "bundle": bundle},
            headers=self._headers(token),
        )

    def _list(self, **query):
        return self.client.get(
            "/agent-assets", query_string=query, headers=self._headers()
        ).get_json()

    def _revisions(self, asset_id):
        return self.client.get(
            f"/agent-assets/{asset_id}/revisions", headers=self._headers()
        ).get_json()["items"]

    def _download(self, asset_id, version):
        return self.client.get(
            f"/agent-assets/{asset_id}/revisions/{version}/bundle",
            headers=self._headers(),
        )

    def _assert_error(self, response, status, code):
        self.assertEqual(response.status_code, status, response.get_json())
        body = response.get_json()
        self.assertEqual(body["error"]["code"], code, body)
        self.assertIsInstance(body["error"]["message"], str)
        return body["error"]

    # ===== 往返 =====

    def test_create_list_revisions_download_round_trip(self):
        content = "# Demo\r\n\r\n原始字节必须逐字节保留。\n".encode("utf-8")
        bundle = make_bundle({"SKILL.md": content, "scripts/run.sh": b"#!/bin/sh\n"})
        created = self._create(bundle)
        self.assertEqual(created.status_code, 200, created.get_json())
        result = created.get_json()
        self.assertFalse(result["replayed"])
        self.assertFalse(result["deduplicated"])

        asset = result["asset"]
        self.assertTrue(asset["asset_id"].startswith("a_"))
        self.assertEqual(asset["tool"], "zcode")
        self.assertEqual(asset["kind"], "skill")
        self.assertEqual(asset["native_id"], "demo-skill")
        self.assertEqual(asset["origin_scope"], "user")
        self.assertEqual(asset["latest_version"], 1)
        self.assertEqual(asset["latest_bundle_sha256"], bundle["bundle_sha256"])
        self.assertEqual(asset["latest_manifest_sha256"], bundle["manifest_sha256"])
        self.assertEqual(asset["total_size"], len(content) + len(b"#!/bin/sh\n"))
        self.assertEqual(asset["entry_count"], 2)
        self.assertGreater(asset["created_at"], 0)
        self.assertEqual(asset["created_at"], asset["updated_at"])
        self.assertEqual(asset["updated_by_device"], "device-a")

        revision = result["revision"]
        self.assertEqual(revision["asset_id"], asset["asset_id"])
        self.assertEqual(revision["version"], 1)
        self.assertEqual(revision["bundle_sha256"], bundle["bundle_sha256"])
        self.assertEqual(revision["manifest_sha256"], bundle["manifest_sha256"])
        self.assertEqual(revision["created_by_device"], "device-a")

        listing = self._list()
        self.assertEqual(listing["total"], 1)
        self.assertEqual(listing["page"], 1)
        self.assertEqual(listing["page_size"], 20)
        self.assertEqual(listing["items"], [asset])

        revisions = self._revisions(asset["asset_id"])
        self.assertEqual(revisions, [revision])

        downloaded = self._download(asset["asset_id"], 1)
        self.assertEqual(downloaded.status_code, 200)
        self.assertEqual(downloaded.mimetype, "application/json")
        payload = downloaded.get_json()
        self.assertEqual(payload, bundle)
        # 下载后三层指纹复验必须逐位一致（§3.3 往返不变量）
        self.assertEqual(
            hashlib.sha256(canonical_manifest(payload["manifest"])).hexdigest(),
            bundle["manifest_sha256"],
        )
        entries = [
            (item["path"], base64.b64decode(item["content_base64"]))
            for item in payload["contents"]
        ]
        self.assertEqual(bundle_sha(entries), bundle["bundle_sha256"])
        for item in payload["manifest"]["files"]:
            self.assertEqual(
                hashlib.sha256(dict(entries)[item["path"]]).hexdigest(),
                item["sha256"],
            )

    # ===== 幂等与去重（§8.6）=====

    def test_idempotent_replay_returns_first_result_without_writes(self):
        bundle = make_bundle({"SKILL.md": b"# v1\n"})
        first = self._create(bundle, request_id="req-replay").get_json()

        # 同一 request_id 即使内容不同，也返回首次结果且不产生新写入
        replay = self._create(
            make_bundle({"SKILL.md": b"# v2\n"}), request_id="req-replay"
        )
        self.assertEqual(replay.status_code, 200, replay.get_json())
        body = replay.get_json()
        self.assertTrue(body["replayed"])
        self.assertFalse(body["deduplicated"])
        self.assertEqual(body["asset"], first["asset"])
        self.assertEqual(body["revision"], first["revision"])
        self.assertEqual(self._list()["total"], 1)
        self.assertEqual(len(self._revisions(first["asset"]["asset_id"])), 1)

    def test_identical_content_is_deduplicated(self):
        bundle = make_bundle({"SKILL.md": b"# same\n"})
        first = self._create(bundle, request_id="req-dedup-1").get_json()

        second = self._create(bundle, request_id="req-dedup-2").get_json()
        self.assertFalse(second["replayed"])
        self.assertTrue(second["deduplicated"])
        self.assertEqual(second["asset"]["asset_id"], first["asset"]["asset_id"])
        self.assertEqual(second["asset"]["latest_version"], 1)
        self.assertEqual(second["revision"]["version"], 1)
        self.assertEqual(self._list()["total"], 1)

    def test_same_identity_different_content_coexists(self):
        first = self._create(
            make_bundle({"SKILL.md": b"# a\n"}), request_id="req-coexist-1"
        ).get_json()
        second = self._create(
            make_bundle({"SKILL.md": b"# b\n"}), request_id="req-coexist-2"
        ).get_json()
        self.assertFalse(second["deduplicated"])
        self.assertNotEqual(
            second["asset"]["asset_id"], first["asset"]["asset_id"]
        )
        self.assertEqual(self._list()["total"], 2)
        listing = self._list(tool="zcode", kind="skill", scope="user")
        self.assertEqual(listing["total"], 2)

    def test_dedup_compares_only_latest_revision(self):
        old = make_bundle({"SKILL.md": b"# old\n"})
        created = self._create(old, request_id="req-old").get_json()
        asset_id = created["asset"]["asset_id"]
        self.assertEqual(
            self._update(asset_id, make_bundle({"SKILL.md": b"# new\n"}), 1).status_code,
            200,
        )
        # 旧内容重新 POST：latest 已不同，不能命中去重，应新建资源
        body = self._create(old, request_id="req-old-again").get_json()
        self.assertFalse(body["deduplicated"])
        self.assertNotEqual(body["asset"]["asset_id"], asset_id)
        self.assertEqual(self._list()["total"], 2)

    def test_put_identical_content_is_deduplicated(self):
        bundle = make_bundle({"SKILL.md": b"# same\n"})
        created = self._create(bundle, request_id="req-put-dedup").get_json()
        asset_id = created["asset"]["asset_id"]

        # PUT 与 latest 完全相同的双指纹：不新增版本
        response = self._update(asset_id, bundle, 1)
        self.assertEqual(response.status_code, 200, response.get_json())
        body = response.get_json()
        self.assertFalse(body["replayed"])
        self.assertTrue(body["deduplicated"])
        self.assertEqual(body["asset"]["latest_version"], 1)
        self.assertEqual(body["revision"]["version"], 1)
        self.assertEqual(self._list()["items"][0]["latest_version"], 1)
        self.assertEqual(len(self._revisions(asset_id)), 1)

        # 内容变化后正常追加；再次 PUT 相同内容命中新 latest 的去重
        changed = make_bundle({"SKILL.md": b"# changed\n"})
        updated = self._update(asset_id, changed, 1)
        self.assertEqual(updated.status_code, 200, updated.get_json())
        self.assertFalse(updated.get_json()["deduplicated"])
        again = self._update(asset_id, changed, 2)
        self.assertEqual(again.status_code, 200, again.get_json())
        self.assertTrue(again.get_json()["deduplicated"])
        self.assertEqual(again.get_json()["revision"]["version"], 2)
        self.assertEqual([r["version"] for r in self._revisions(asset_id)], [2, 1])

    # ===== 乐观并发（§8.4）=====

    def test_put_version_conflict_and_success(self):
        created = self._create(
            make_bundle({"SKILL.md": b"# v1\n"}), request_id="req-put"
        ).get_json()
        asset_id = created["asset"]["asset_id"]

        stale = self._update(asset_id, make_bundle({"SKILL.md": b"# v2\n"}), 99)
        error = self._assert_error(stale, 409, "VERSION_CONFLICT")
        self.assertEqual(error["details"]["current_version"], 1)
        self.assertEqual(self._revisions(asset_id)[0]["version"], 1)

        updated = self._update(asset_id, make_bundle({"SKILL.md": b"# v2\n"}), 1)
        self.assertEqual(updated.status_code, 200, updated.get_json())
        body = updated.get_json()
        self.assertFalse(body["replayed"])
        self.assertFalse(body["deduplicated"])
        self.assertEqual(body["asset"]["latest_version"], 2)
        self.assertEqual(body["asset"]["latest_bundle_sha256"], body["revision"]["bundle_sha256"])
        self.assertEqual(body["revision"]["version"], 2)
        self.assertEqual(body["revision"]["created_by_device"], "device-a")

        revisions = self._revisions(asset_id)
        self.assertEqual([item["version"] for item in revisions], [2, 1])
        self.assertEqual(self._download(asset_id, 1).status_code, 200)
        self.assertEqual(self._download(asset_id, 2).status_code, 200)

    def test_put_rejects_identity_change(self):
        created = self._create(
            make_bundle({"SKILL.md": b"# v1\n"}), request_id="req-identity"
        ).get_json()
        asset_id = created["asset"]["asset_id"]
        changed = make_bundle({"SKILL.md": b"# v2\n"}, native_id="other-skill")
        self._assert_error(self._update(asset_id, changed, 1), 400, "VALIDATION_FAILED")
        # 失败不应产生新版本
        self.assertEqual(self._revisions(asset_id)[0]["version"], 1)
        self.assertEqual(self._list()["items"][0]["latest_version"], 1)

    # ===== 结构校验与指纹复验 =====

    def test_tampered_bundles_are_rejected(self):
        base = make_bundle(
            {"SKILL.md": b"# good\n", "scripts/run.sh": b"#!/bin/sh\n"}
        )
        tampered = []

        broken = json.loads(json.dumps(base))
        broken["contents"][0]["content_base64"] = base64.b64encode(b"# evil\n").decode()
        tampered.append(broken)  # sha256 不匹配

        broken = json.loads(json.dumps(base))
        broken["manifest"]["files"][0]["size"] += 1
        broken["manifest_sha256"] = hashlib.sha256(
            canonical_manifest(broken["manifest"])
        ).hexdigest()
        tampered.append(broken)  # 实际大小与清单不一致

        broken = json.loads(json.dumps(base))
        broken["manifest"]["display_name"] = "被篡改"
        tampered.append(broken)  # manifest_sha256 未更新

        broken = json.loads(json.dumps(base))
        broken["contents"][0]["content_base64"] = base64.b64encode(b"# evil\n").decode()
        broken["manifest"]["files"][0]["sha256"] = hashlib.sha256(b"# evil\n").hexdigest()
        broken["manifest"]["files"][0]["size"] = len(b"# evil\n")
        broken["manifest_sha256"] = hashlib.sha256(
            canonical_manifest(broken["manifest"])
        ).hexdigest()
        tampered.append(broken)  # bundle_sha256 未更新

        for index, bundle in enumerate(tampered):
            with self.subTest(case=index):
                self._assert_error(
                    self._create(bundle, request_id=f"req-tamper-{index}"),
                    400,
                    "VALIDATION_FAILED",
                )
        self.assertEqual(self._list()["total"], 0)

    def test_contents_must_match_manifest_order(self):
        bundle = make_bundle({"SKILL.md": b"# ok\n", "a.txt": b"a"})

        reordered = json.loads(json.dumps(bundle))
        reordered["contents"].reverse()
        self._assert_error(
            self._create(reordered, request_id="req-order-1"),
            400,
            "VALIDATION_FAILED",
        )

        unsorted = json.loads(json.dumps(bundle))
        unsorted["manifest"]["files"].reverse()
        unsorted["manifest_sha256"] = hashlib.sha256(
            canonical_manifest(unsorted["manifest"])
        ).hexdigest()
        self._assert_error(
            self._create(unsorted, request_id="req-order-2"),
            400,
            "VALIDATION_FAILED",
        )

    def test_invalid_request_id_and_schema_version(self):
        bundle = make_bundle({"SKILL.md": b"# ok\n"})
        for request_id in ("", "   ", None, 123, "x" * 129):
            with self.subTest(request_id=request_id):
                response = self.client.post(
                    "/agent-assets",
                    json={"client_request_id": request_id, "bundle": bundle},
                    headers=self._headers(),
                )
                self._assert_error(response, 400, "VALIDATION_FAILED")
        for payload in ({}, {"client_request_id": "r"}, {"client_request_id": "r", "bundle": []}):
            with self.subTest(payload=payload):
                self._assert_error(
                    self.client.post(
                        "/agent-assets", json=payload, headers=self._headers()
                    ),
                    400,
                    "VALIDATION_FAILED",
                )
        self._assert_error(
            self.client.post(
                "/agent-assets",
                data="{not json",
                content_type="application/json",
                headers=self._headers(),
            ),
            400,
            "VALIDATION_FAILED",
        )
        for version in (2, "1", True, 1.0, "1.0", None):
            with self.subTest(schema_version=version):
                broken = make_bundle({"SKILL.md": b"# ok\n"}, schema_version=version)
                self._assert_error(
                    self._create(broken, request_id=f"schema-{version}"),
                    400,
                    "VALIDATION_FAILED",
                )

    def test_manifest_file_fields_are_all_required(self):
        # ManifestFile 五字段全部必填且类型正确：缺省/None 不得按 false 兜底，
        # 否则 Rust 客户端（executable/binary 无默认值）无法消费服务端已接受的包。
        for field in ("path", "size", "sha256", "executable", "binary"):
            with self.subTest(field=field):
                bundle = make_bundle({"SKILL.md": b"# ok\n"})
                del bundle["manifest"]["files"][0][field]
                bundle["manifest_sha256"] = hashlib.sha256(
                    canonical_manifest(bundle["manifest"])
                ).hexdigest()
                self._assert_error(
                    self._create(bundle, request_id=f"missing-{field}"),
                    400,
                    "VALIDATION_FAILED",
                )
            with self.subTest(field=field, value=None):
                bundle = make_bundle({"SKILL.md": b"# ok\n"})
                bundle["manifest"]["files"][0][field] = None
                bundle["manifest_sha256"] = hashlib.sha256(
                    canonical_manifest(bundle["manifest"])
                ).hexdigest()
                self._assert_error(
                    self._create(bundle, request_id=f"null-{field}"),
                    400,
                    "VALIDATION_FAILED",
                )
        for flag in ("executable", "binary"):
            bundle = make_bundle({"SKILL.md": b"# ok\n"})
            bundle["manifest"]["files"][0][flag] = 1
            bundle["manifest_sha256"] = hashlib.sha256(
                canonical_manifest(bundle["manifest"])
            ).hexdigest()
            self._assert_error(
                self._create(bundle, request_id=f"flag-type-{flag}"),
                400,
                "VALIDATION_FAILED",
            )

    def test_unknown_manifest_fields_are_accepted_and_round_trip(self):
        # 服务端允许未知字段（契约 §3.2），存原信封、原样返回；指纹按收到的
        # 原始 manifest 重算，客户端解析侧也须按原文复验。
        bundle = make_bundle({"SKILL.md": b"# ok\n"})
        bundle["manifest"]["future_field"] = {
            "nested": [1, 2, 3],
            "text": "第三方客户端扩展",
        }
        bundle["manifest"]["unknown_nullable"] = None
        bundle["manifest_sha256"] = hashlib.sha256(
            canonical_manifest(bundle["manifest"])
        ).hexdigest()
        created = self._create(bundle, request_id="req-unknown-fields")
        self.assertEqual(created.status_code, 200, created.get_json())
        asset_id = created.get_json()["asset"]["asset_id"]
        downloaded = self._download(asset_id, 1)
        self.assertEqual(downloaded.status_code, 200)
        self.assertEqual(downloaded.get_json(), bundle)

    def test_invalid_base64_is_rejected(self):
        for bad in ("%%%", "YQ", "YQ===", "Y Q=="):
            with self.subTest(encoded=bad):
                bundle = make_bundle({"SKILL.md": b"# ok\n"})
                bundle["contents"][0]["content_base64"] = bad
                self._assert_error(
                    self._create(bundle, request_id=f"b64-{bad}"),
                    400,
                    "VALIDATION_FAILED",
                )

    def test_surrogate_request_body_returns_json_400(self):
        # \ud800 孤立代理项：json.loads 能解析，但落库/编码会 500；必须在
        # 解析处折成 §4.2 的 VALIDATION_FAILED 错误体（不能吐 HTML）。
        bundle = make_bundle({"SKILL.md": b"# ok\n"})
        body = json.dumps(
            {"client_request_id": "req-surrogate", "bundle": bundle},
            ensure_ascii=True,
        ).replace('"zcode"', '"\\ud800"')
        response = self.client.post(
            "/agent-assets",
            data=body,
            content_type="application/json",
            headers=self._headers(),
        )
        self._assert_error(response, 400, "VALIDATION_FAILED")

    def test_unknown_native_format_is_tolerated(self):
        bundle = make_bundle(
            {"SKILL.md": b"# ok\n"}, native_format="future-format-x"
        )
        self.assertEqual(self._create(bundle).status_code, 200)

    # ===== 路径安全（§9）=====

    def test_unsafe_paths_are_rejected(self):
        cases = [
            "../escape.md",
            "/absolute.md",
            "a\\b.md",
            "CON",
            "CON.txt",
            "COM1.txt",
            "LPT9",
            "skills/NAME~1",
            "skills/longna~2.txt",
            # §9.4 基名定义：任一点分隔片含 `~数字` 一律拒绝（与 Rust 一致）
            "a.b~1.md",
            "name~1.tar.gz",
            "~1",
            "a//b.md",
            "a/./b.md",
            "a:b.md",
            "a/\x01bad.md",
            "trailing./x.md",
            "trailing /x.md",
            "bad<name>.md",
        ]
        for index, bad_path in enumerate(cases):
            with self.subTest(path=bad_path):
                bundle = make_bundle({"SKILL.md": b"# ok\n", bad_path: b"x"})
                error = self._assert_error(
                    self._create(bundle, request_id=f"req-path-{index}"),
                    400,
                    "PATH_UNSAFE",
                )
                self.assertEqual(error["details"]["path"], bad_path)

        # 同目录大小写折叠碰撞
        collision = make_bundle(
            {"SKILL.md": b"# ok\n", "README.md": b"a", "readme.md": b"b"}
        )
        error = self._assert_error(
            self._create(collision, request_id="req-case"), 400, "PATH_UNSAFE"
        )
        self.assertIn(error["details"]["path"], ("README.md", "readme.md"))
        self.assertEqual(self._list()["total"], 0)

    # ===== 限额（§8.7）=====

    def test_capabilities_returns_default_limits(self):
        response = self.client.get(
            "/agent-assets/capabilities", headers=self._headers()
        )
        self.assertEqual(response.status_code, 200)
        body = response.get_json()
        self.assertEqual(body["api_version"], 1)
        self.assertEqual(body["limits"], app.AGENT_ASSET_LIMIT_DEFAULTS)

    def test_entry_count_limit(self):
        self._set_limits(max_entries_per_bundle=3)
        files = {
            "SKILL.md": b"# ok\n",
            "a.txt": b"a",
            "b.txt": b"b",
            "c.txt": b"c",
        }
        error = self._assert_error(
            self._create(make_bundle(files)), 400, "TOO_MANY_ENTRIES"
        )
        self.assertEqual(error["details"]["limit"], 3)

    def test_file_size_limit(self):
        self._set_limits(max_file_bytes=32)
        limits = self.client.get(
            "/agent-assets/capabilities", headers=self._headers()
        ).get_json()["limits"]
        self.assertEqual(limits["max_file_bytes"], 32)
        error = self._assert_error(
            self._create(make_bundle({"SKILL.md": b"x" * 64})),
            413,
            "PAYLOAD_TOO_LARGE",
        )
        self.assertEqual(error["details"]["limit"], 32)

    def test_bundle_size_limit(self):
        self._set_limits(max_file_bytes=60, max_bundle_bytes=64)
        bundle = make_bundle({"SKILL.md": b"a" * 40, "b.txt": b"b" * 40})
        self._assert_error(self._create(bundle), 413, "PAYLOAD_TOO_LARGE")

    def test_request_body_limit(self):
        self._set_limits(max_request_bytes=400)
        bundle = make_bundle({"SKILL.md": b"#" + b"x" * 500})
        self._assert_error(self._create(bundle), 413, "PAYLOAD_TOO_LARGE")

    def test_library_quota(self):
        self._set_limits(library_quota_bytes=100)
        first = self._create(
            make_bundle({"SKILL.md": b"a" * 60}), request_id="req-quota-1"
        )
        self.assertEqual(first.status_code, 200, first.get_json())
        error = self._assert_error(
            self._create(
                make_bundle({"SKILL.md": b"b" * 60}), request_id="req-quota-2"
            ),
            507,
            "QUOTA_EXCEEDED",
        )
        self.assertEqual(error["details"]["quota_bytes"], 100)
        self.assertEqual(error["details"]["used_bytes"], 60)
        # 相同内容去重不新增字节，不受配额限制
        dedup = self._create(
            make_bundle({"SKILL.md": b"a" * 60}), request_id="req-quota-3"
        )
        self.assertEqual(dedup.status_code, 200, dedup.get_json())
        self.assertTrue(dedup.get_json()["deduplicated"])

    def test_update_quota_accounts_for_evicted_revision(self):
        self._set_limits(library_quota_bytes=120, max_revisions_per_asset=1)
        created = self._create(
            make_bundle({"SKILL.md": b"a" * 60}), request_id="req-net-quota"
        ).get_json()
        # 新版本 60 字节、淘汰旧版本释放 60 字节 → 净增 0，仍在上限内
        response = self._update(
            created["asset"]["asset_id"], make_bundle({"SKILL.md": b"b" * 60}), 1
        )
        self.assertEqual(response.status_code, 200, response.get_json())

    # ===== 版本淘汰（§8.5）=====

    def test_revision_eviction_keeps_newest(self):
        self._set_limits(max_revisions_per_asset=2)
        created = self._create(
            make_bundle({"SKILL.md": b"# v1\n"}), request_id="req-evict"
        ).get_json()
        asset_id = created["asset"]["asset_id"]
        for version in (2, 3):
            response = self._update(
                asset_id,
                make_bundle({"SKILL.md": f"# v{version}\n".encode()}),
                version - 1,
            )
            self.assertEqual(response.status_code, 200, response.get_json())

        revisions = self._revisions(asset_id)
        self.assertEqual([item["version"] for item in revisions], [3, 2])
        self._assert_error(self._download(asset_id, 1), 404, "NOT_FOUND")
        self.assertEqual(self._download(asset_id, 2).status_code, 200)
        self.assertEqual(self._download(asset_id, 3).status_code, 200)
        # 容量统计只算保留版本
        self.assertEqual(db.agent_asset_library_bytes(), len(b"# v2\n") + len(b"# v3\n"))

    # ===== 404 与参数错误 =====

    def test_not_found_and_invalid_revision(self):
        self._assert_error(
            self.client.get(
                "/agent-assets/a_missing/revisions", headers=self._headers()
            ),
            404,
            "NOT_FOUND",
        )
        self._assert_error(
            self.client.get(
                "/agent-assets/a_missing/revisions/1/bundle", headers=self._headers()
            ),
            404,
            "NOT_FOUND",
        )
        created = self._create(
            make_bundle({"SKILL.md": b"# ok\n"}), request_id="req-notfound"
        ).get_json()
        asset_id = created["asset"]["asset_id"]
        self._assert_error(self._download(asset_id, 9), 404, "NOT_FOUND")
        self._assert_error(self._download(asset_id, "abc"), 400, "VALIDATION_FAILED")
        self._assert_error(
            self._update("a_missing", make_bundle({"SKILL.md": b"# ok\n"}), 1),
            404,
            "NOT_FOUND",
        )
        self._assert_error(
            self._update(asset_id, make_bundle({"SKILL.md": b"# ok\n"}), 0),
            400,
            "VALIDATION_FAILED",
        )
        for query in ({"page": 0}, {"page_size": 0}, {"page_size": 101}, {"page": "x"}):
            with self.subTest(query=query):
                response = self.client.get(
                    "/agent-assets", query_string=query, headers=self._headers()
                )
                self._assert_error(response, 400, "VALIDATION_FAILED")

    # ===== 列表筛选与分页（§8.2）=====

    def test_list_filters_and_pagination(self):
        assets = [
            ("zcode", "skill", "user", "pdf-report", "PDF 报告"),
            ("zcode", "agent", "user", "code-reviewer", "代码审查"),
            ("codex", "skill", "user", "pdf-report", "PDF 报告"),
            ("claude", "skill", "project", "pdf-report", "PDF 报告"),
            ("codex", "agent", "user", "writer", "Writer"),
        ]
        for index, (tool, kind, scope, native_id, display_name) in enumerate(assets):
            bundle = make_bundle(
                {"SKILL.md": f"# {native_id}-{index}\n".encode()},
                tool=tool,
                kind=kind,
                origin_scope=scope,
                native_id=native_id,
                display_name=display_name,
                native_format=(
                    "markdown-frontmatter" if kind == "agent" else "skill-dir"
                ),
            )
            response = self._create(bundle, request_id=f"req-list-{index}")
            self.assertEqual(response.status_code, 200, response.get_json())

        self.assertEqual(self._list()["total"], 5)
        self.assertEqual(self._list(tool="zcode")["total"], 2)
        self.assertEqual(self._list(kind="agent")["total"], 2)
        self.assertEqual(self._list(scope="project")["total"], 1)
        self.assertEqual(self._list(q="PDF")["total"], 3)
        self.assertEqual(self._list(q="pdf-report")["total"], 3)
        self.assertEqual(self._list(q="WRITER")["total"], 1)
        # q 中的 % / _ 按字面量匹配，不能当通配符
        self.assertEqual(self._list(q="%")["total"], 0)
        self.assertEqual(self._list(q="_")["total"], 0)
        # 多条件组合
        self.assertEqual(self._list(tool="codex", kind="agent")["total"], 1)

        page1 = self._list(page=1, page_size=2)
        page2 = self._list(page=2, page_size=2)
        page3 = self._list(page=3, page_size=2)
        self.assertEqual(page1["total"], 5)
        self.assertEqual(len(page1["items"]), 2)
        self.assertEqual(len(page2["items"]), 2)
        self.assertEqual(len(page3["items"]), 1)
        ids = {
            item["asset_id"]
            for page in (page1, page2, page3)
            for item in page["items"]
        }
        self.assertEqual(len(ids), 5)

    # ===== 鉴权（§4.1）=====

    def test_requires_device_token(self):
        self._assert_error(self.client.get("/agent-assets"), 401, "UNAUTHORIZED")
        self._assert_error(
            self.client.get("/agent-assets/capabilities"), 401, "UNAUTHORIZED"
        )
        self._assert_error(
            self.client.post("/agent-assets", json={}), 401, "UNAUTHORIZED"
        )
        self._assert_error(
            self.client.put("/agent-assets/a_x", json={}), 401, "UNAUTHORIZED"
        )
        self._assert_error(
            self.client.get("/agent-assets/a_x/revisions"), 401, "UNAUTHORIZED"
        )
        self._assert_error(
            self.client.get("/agent-assets/a_x/revisions/1/bundle"),
            401,
            "UNAUTHORIZED",
        )
        self._assert_error(
            self.client.get("/agent-assets", headers=self._headers("bad-token")),
            401,
            "UNAUTHORIZED",
        )

    def test_revoked_device_is_unauthenticated(self):
        # 现有撤销机制直接删除设备行，旧 token 只能表现为无效凭证（401）；
        # 契约中的 FORBIDDEN/403 需要设备墓碑，当前设备模型不具备。
        db.revoke_device("device-b")
        self._assert_error(
            self.client.get("/agent-assets", headers=self._headers("token-b")),
            401,
            "UNAUTHORIZED",
        )
        self.assertEqual(
            self.client.get("/agent-assets", headers=self._headers()).status_code, 200
        )


class AgentAssetsOldDbUpgradeTest(unittest.TestCase):
    """旧库（只有原有表）经 init_db 自动补建资源库三表，老数据无损。"""

    def setUp(self):
        self.temp_dir = tempfile.TemporaryDirectory()
        self.data_dir = Path(self.temp_dir.name)
        db_path = self.data_dir / "usage.db"

        # 模拟旧版库：只有 devices / config，无资源库三表
        conn = sqlite3.connect(str(db_path))
        try:
            conn.execute(db.SCHEMA_DEVICES)
            conn.execute(db.SCHEMA_CONFIG)
            conn.execute(
                "INSERT INTO devices (device_id, device_name, token_hash, created_at) "
                "VALUES (?, ?, ?, ?)",
                ("old-device", "旧设备", hash_token("old-token"), 123),
            )
            conn.execute(
                "INSERT INTO config (key, value) VALUES ('pricing', ?)",
                ('{"revision": 7}',),
            )
            conn.commit()
        finally:
            conn.close()

        self._saved_paths = (db.DATA_DIR, db.DB_PATH)
        db.DATA_DIR = self.data_dir
        db.DB_PATH = db_path
        db.init_db()
        self.client = app.app.test_client()

    def tearDown(self):
        db.DATA_DIR, db.DB_PATH = self._saved_paths
        self.temp_dir.cleanup()

    def test_init_db_adds_asset_tables_without_touching_old_data(self):
        conn = db.get_conn()
        try:
            tables = {
                row["name"]
                for row in conn.execute(
                    "SELECT name FROM sqlite_master WHERE type = 'table'"
                ).fetchall()
            }
            self.assertTrue(
                {"agent_assets", "agent_asset_revisions", "agent_asset_idempotency"}
                <= tables
            )
            device = conn.execute(
                "SELECT device_name FROM devices WHERE device_id = 'old-device'"
            ).fetchone()
            self.assertEqual(device["device_name"], "旧设备")
            pricing = conn.execute(
                "SELECT value FROM config WHERE key = 'pricing'"
            ).fetchone()
            self.assertEqual(pricing["value"], '{"revision": 7}')
        finally:
            conn.close()

        headers = {"Authorization": "Bearer old-token"}
        listing = self.client.get("/agent-assets", headers=headers)
        self.assertEqual(listing.status_code, 200)
        self.assertEqual(listing.get_json()["total"], 0)

        created = self.client.post(
            "/agent-assets",
            json={
                "client_request_id": "upgrade-1",
                "bundle": make_bundle({"SKILL.md": b"# upgrade\n"}),
            },
            headers=headers,
        )
        self.assertEqual(created.status_code, 200, created.get_json())
        self.assertEqual(created.get_json()["asset"]["latest_version"], 1)


if __name__ == "__main__":
    unittest.main()
