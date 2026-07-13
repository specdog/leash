from __future__ import annotations

import copy
import hashlib
import importlib
import importlib.util
import json
import unittest

from profile import canonical_bytes
from support import candidate_profile, verified_zero


class AcceptanceTests(unittest.TestCase):
    def acceptance(self):
        spec = importlib.util.find_spec("acceptance")
        if spec is None:
            self.fail("acceptance module is missing")
        return importlib.import_module("acceptance")

    def profile(self) -> dict:
        profile = candidate_profile()
        profile["measurement"] = {
            "procedure_revision": "issue-166-v1",
            "measured_at": "2026-07-10T00:00:00Z",
            "acceptance_manifest_sha256": None,
        }
        return profile

    def analysis(self, phase: str, run_indexes: tuple[int, ...]) -> dict:
        runs = []
        for run_index in run_indexes:
            runs.append(
                {
                    "capture": f"{phase}-{run_index}.jsonl",
                    "evidence_sha256": hashlib.sha256(
                        f"{phase}-{run_index}".encode()
                    ).hexdigest(),
                    "phase": phase,
                    "run_index": run_index,
                    "generic": {
                        "resource_samples": 2,
                        "stop_events": {
                            "initial": verified_zero(run_index * 2 - 1, run_index * 10),
                            "final": verified_zero(run_index * 2, run_index * 10 + 1),
                        },
                    },
                    "accepted": True,
                }
            )
        return {
            "ok": True,
            "format": "leash-waveshare-ugv-calibration-analysis-v1",
            "profile": "pinkie-v1",
            "phase": phase,
            "series_complete": phase != "square" or run_indexes == (1, 2, 3),
            "runs": runs,
        }

    def map_reload_proof(self, profile: dict) -> dict:
        digest = hashlib.sha256(canonical_bytes(profile)).hexdigest()
        map_metadata = {
            "map_id": "waveshare-ugv-map",
            "map_revision": "lineage-a",
            "grid_revision": "grid-a",
            "frame_id": "map",
        }
        artifacts = [
            {
                "file": f"accepted-room.{suffix}",
                "size_bytes": 100 + index,
                "sha256": hashlib.sha256(suffix.encode()).hexdigest(),
            }
            for index, suffix in enumerate(
                ("posegraph", "data", "yaml", "pgm", "lineage.json")
            )
        ]
        return {
            "ok": True,
            "format": "leash-waveshare-ugv-map-reload-proof-v2",
            "profile": profile["profile"],
            "calibration_sha256": digest,
            "map_name": "accepted-room",
            "lineage": {
                "format": "leash-map-lineage-v1",
                "map_id": "waveshare-ugv-map",
                "map_revision": "lineage-a",
                "frame_id": "map",
            },
            "saved_artifacts": {
                "before": artifacts,
                "after": copy.deepcopy(artifacts),
            },
            "before": {
                "captured_at_ms": 1_700_000_000_000,
                "map": map_metadata,
                "provider": {
                    "state": "tracking",
                    "provider_instance_id": "instance-a",
                    "generation": 1,
                    "last_received_ms": 1_700_000_000_000,
                    "stale_after_ms": 1_000,
                },
                "pose": {
                    "pose": {"ts_ms": 1_700_000_000_000},
                    "covariance": [0.01] * 9,
                },
                "command": {"left_cmd": 0.0, "right_cmd": 0.0},
            },
            "after": {
                "captured_at_ms": 1_700_000_001_000,
                "map": copy.deepcopy(map_metadata),
                "provider": {
                    "state": "tracking",
                    "provider_instance_id": "instance-b",
                    "generation": 2,
                    "last_received_ms": 1_700_000_001_000,
                    "stale_after_ms": 1_000,
                },
                "pose": {
                    "pose": {"ts_ms": 1_700_000_001_000},
                    "covariance": [0.01] * 9,
                },
                "command": {"left_cmd": 0.0, "right_cmd": 0.0},
            },
            "leash": {
                "pid": 4242,
                "unchanged": True,
                "entry_verified_zero": verified_zero(1, 10),
                "exit_verified_zero": verified_zero(2, 11),
            },
            "container": {
                "running": True,
                "oom_killed": False,
                "restart_count": 1,
            },
            "recorder_issues_motion": False,
        }

    def evidence(self):
        profile = self.profile()
        analyses = [
            ("stationary-analysis.json", self.analysis("stationary", (1,))),
            ("straight-analysis.json", self.analysis("straight", (1,))),
            ("turn-analysis.json", self.analysis("turn", (1,))),
            ("square-analysis.json", self.analysis("square", (1, 2, 3))),
        ]
        map_proof = ("map-reload.json", self.map_reload_proof(profile))
        body_artifact = {
            "file": "body-artifact.png",
            "sha256": hashlib.sha256(b"scrubbed body artifact").hexdigest(),
            "reviewed": True,
            "no_persistent_robot_body_artifact": True,
        }
        return profile, analyses, map_proof, body_artifact

    def build(self):
        acceptance = self.acceptance()
        profile, analyses, map_proof, body_artifact = self.evidence()
        manifest = acceptance.build_manifest(
            profile,
            analyses,
            map_proof,
            body_artifact,
            acceptance.default_watchdog_status(),
        )
        return acceptance, profile, manifest

    def validate_map_proof(self, proof: dict, profile: dict) -> dict:
        validator = getattr(self.acceptance(), "validate_map_reload_proof", None)
        self.assertIsNotNone(validator, "map reload proof validator is missing")
        return validator(proof, profile)

    def test_builds_stage_one_manifest_without_claiming_autonomous_readiness(self):
        acceptance, profile, manifest = self.build()

        validated = acceptance.validate_manifest(manifest, profile)

        self.assertTrue(validated["ok"])
        self.assertTrue(validated["readiness"]["calibration_accepted"])
        self.assertTrue(validated["readiness"]["mapping_ready"])
        self.assertFalse(
            validated["readiness"]["physical_autonomous_exploration_ready"]
        )
        self.assertEqual(
            [entry["phase"] for entry in validated["captures"]],
            ["stationary", "straight", "turn", "square", "square", "square"],
        )
        self.assertEqual(
            acceptance.manifest_sha256(validated),
            hashlib.sha256(acceptance.canonical_manifest_bytes(validated)).hexdigest(),
        )

    def test_rejects_missing_square_run(self):
        acceptance, profile, manifest = self.build()
        manifest["captures"] = [
            entry
            for entry in manifest["captures"]
            if not (entry["phase"] == "square" and entry["run_index"] == 3)
        ]

        with self.assertRaisesRegex(ValueError, "square runs 1, 2, and 3"):
            acceptance.validate_manifest(manifest, profile)

    def test_rejects_missing_verified_zero_or_resources(self):
        acceptance, profile, manifest = self.build()
        no_zero = copy.deepcopy(manifest)
        no_zero["captures"][0]["verified_zero"]["final"] = None
        with self.assertRaisesRegex(ValueError, "verified zero"):
            acceptance.validate_manifest(no_zero, profile)

        no_resources = copy.deepcopy(manifest)
        no_resources["captures"][0]["resource_samples"] = 0
        with self.assertRaisesRegex(ValueError, "resource"):
            acceptance.validate_manifest(no_resources, profile)

    def test_rejects_final_zero_that_is_not_newer_than_entry(self):
        acceptance, profile, manifest = self.build()
        stale_zero = copy.deepcopy(manifest)
        stops = stale_zero["captures"][0]["verified_zero"]
        stops["final"]["command_sequence"] = stops["initial"]["command_sequence"]
        stops["final"]["adapter_sample_sequence"] = stops["initial"][
            "adapter_sample_sequence"
        ]

        with self.assertRaisesRegex(ValueError, "newer"):
            acceptance.validate_manifest(stale_zero, profile)

    def test_rejects_wrong_calibration_digest_map_proof_or_body_hash(self):
        acceptance, profile, manifest = self.build()
        wrong_digest = copy.deepcopy(manifest)
        wrong_digest["calibration_sha256"] = "f" * 64
        with self.assertRaisesRegex(ValueError, "calibration digest"):
            acceptance.validate_manifest(wrong_digest, profile)

        wrong_map = copy.deepcopy(manifest)
        wrong_map["map_reload"]["format"] = "legacy"
        with self.assertRaisesRegex(ValueError, "map reload"):
            acceptance.validate_manifest(wrong_map, profile)

        wrong_body = copy.deepcopy(manifest)
        wrong_body["body_artifact"]["sha256"] = "invalid"
        with self.assertRaisesRegex(ValueError, "body artifact"):
            acceptance.validate_manifest(wrong_body, profile)

    def test_validates_exact_map_reload_proof(self):
        profile = self.profile()

        validated = self.validate_map_proof(self.map_reload_proof(profile), profile)

        self.assertTrue(validated["ok"])

    def test_map_proof_rejects_missing_lineage_artifact(self):
        profile = self.profile()
        proof = self.map_reload_proof(profile)
        proof["saved_artifacts"]["before"] = [
            artifact
            for artifact in proof["saved_artifacts"]["before"]
            if not artifact["file"].endswith(".lineage.json")
        ]

        with self.assertRaisesRegex(ValueError, "lineage artifact"):
            self.validate_map_proof(proof, profile)

    def test_map_proof_rejects_changed_posegraph_hash(self):
        profile = self.profile()
        proof = self.map_reload_proof(profile)
        proof["saved_artifacts"]["after"][0]["sha256"] = "f" * 64

        with self.assertRaisesRegex(ValueError, "saved artifact"):
            self.validate_map_proof(proof, profile)

    def test_map_proof_rejects_same_provider_instance(self):
        profile = self.profile()
        proof = self.map_reload_proof(profile)
        proof["after"]["provider"]["provider_instance_id"] = proof["before"][
            "provider"
        ]["provider_instance_id"]

        with self.assertRaisesRegex(ValueError, "provider instance"):
            self.validate_map_proof(proof, profile)

    def test_map_proof_rejects_unchanged_provider_generation(self):
        profile = self.profile()
        proof = self.map_reload_proof(profile)
        proof["after"]["provider"]["generation"] = proof["before"]["provider"][
            "generation"
        ]

        with self.assertRaisesRegex(ValueError, "generation"):
            self.validate_map_proof(proof, profile)

    def test_map_proof_rejects_changed_lineage(self):
        profile = self.profile()
        proof = self.map_reload_proof(profile)
        proof["after"]["map"]["map_revision"] = "lineage-b"

        with self.assertRaisesRegex(ValueError, "lineage"):
            self.validate_map_proof(proof, profile)

    def test_map_proof_rejects_changed_stopped_grid_revision(self):
        profile = self.profile()
        proof = self.map_reload_proof(profile)
        proof["after"]["map"]["grid_revision"] = "grid-b"

        with self.assertRaisesRegex(ValueError, "grid revision"):
            self.validate_map_proof(proof, profile)

    def test_autonomous_readiness_requires_all_watchdogs_at_250_ms(self):
        acceptance = self.acceptance()
        profile, analyses, map_proof, body_artifact = self.evidence()
        watchdogs = {
            name: {"status": "proven", "ttl_ms": 250}
            for name in (
                "process_termination",
                "event_loop_stall",
                "serial_disconnect",
            )
        }

        manifest = acceptance.build_manifest(
            profile, analyses, map_proof, body_artifact, watchdogs
        )

        self.assertTrue(
            manifest["readiness"]["physical_autonomous_exploration_ready"]
        )
        too_slow = copy.deepcopy(manifest)
        too_slow["watchdogs"]["serial_disconnect"]["ttl_ms"] = 251
        with self.assertRaisesRegex(ValueError, "readiness"):
            acceptance.validate_manifest(too_slow, profile)


if __name__ == "__main__":
    unittest.main()
