#!/usr/bin/env python3
"""Build and validate Pinkie's typed Stage 1 calibration acceptance manifest."""

from __future__ import annotations

import argparse
import copy
import hashlib
import json
import pathlib
import re
import sys
from typing import Any

from profile import canonical_bytes, validate

FORMAT = "leash-waveshare-ugv-calibration-acceptance-v1"
ANALYSIS_FORMAT = "leash-waveshare-ugv-calibration-analysis-v1"
MAP_PROOF_FORMAT = "leash-waveshare-ugv-map-reload-proof-v2"
SHA256_RE = re.compile(r"^[0-9a-f]{64}$")
REQUIRED_RUNS = (
    ("stationary", 1),
    ("straight", 1),
    ("turn", 1),
    ("square", 1),
    ("square", 2),
    ("square", 3),
)
WATCHDOGS = ("process_termination", "event_loop_stall", "serial_disconnect")


def fail(message: str) -> None:
    raise ValueError(message)


def is_sha256(value: Any) -> bool:
    return isinstance(value, str) and SHA256_RE.fullmatch(value) is not None


def canonical_json_bytes(value: Any) -> bytes:
    return (json.dumps(value, sort_keys=True, separators=(",", ":")) + "\n").encode()


def canonical_manifest_bytes(manifest: dict[str, Any]) -> bytes:
    return canonical_json_bytes(manifest)


def manifest_sha256(manifest: dict[str, Any]) -> str:
    return hashlib.sha256(canonical_manifest_bytes(manifest)).hexdigest()


def default_watchdog_status() -> dict[str, dict[str, Any]]:
    return {
        name: {"status": "not-proven", "ttl_ms": None}
        for name in WATCHDOGS
    }


def basename(value: Any, label: str) -> str:
    if not isinstance(value, str) or not value or pathlib.Path(value).name != value:
        fail(f"{label} must be a basename")
    return value


def verified_zero(value: Any, label: str) -> dict[str, Any]:
    if not isinstance(value, dict):
        fail(f"{label} is missing typed verified zero evidence")
    counters = (
        "command_sequence",
        "write_completed_at_ms",
        "adapter_sample_sequence",
        "confirmation_received_at_ms",
    )
    if any(
        isinstance(value.get(field), bool)
        or not isinstance(value.get(field), int)
        or value[field] < 0
        for field in counters
    ):
        fail(f"{label} contains invalid verified zero counters")
    if value.get("acknowledged") is not True:
        fail(f"{label} does not acknowledge verified zero")
    if value.get("statement") != "zero command confirmed":
        fail(f"{label} has an invalid verified zero statement")
    if not isinstance(value.get("source"), str) or not value["source"].strip():
        fail(f"{label} has no verified zero source")
    if value["confirmation_received_at_ms"] < value["write_completed_at_ms"]:
        fail(f"{label} confirms verified zero before the write completed")
    return copy.deepcopy(value)


def watchdog_readiness(watchdogs: Any) -> bool:
    if not isinstance(watchdogs, dict) or set(watchdogs) != set(WATCHDOGS):
        fail("watchdog readiness must include process termination, event loop stall, and serial disconnect")
    ready = True
    for name in WATCHDOGS:
        proof = watchdogs[name]
        if not isinstance(proof, dict) or proof.get("status") not in {
            "not-proven",
            "proven",
        }:
            fail(f"watchdog {name} must have status not-proven or proven")
        ttl_ms = proof.get("ttl_ms")
        if proof["status"] == "not-proven":
            if ttl_ms is not None:
                fail(f"watchdog {name} cannot record a TTL before it is proven")
            ready = False
        elif (
            isinstance(ttl_ms, bool)
            or not isinstance(ttl_ms, int)
            or ttl_ms < 0
        ):
            fail(f"watchdog {name} must record a non-negative TTL")
        elif ttl_ms > 250:
            ready = False
    return ready


def capture_entry(
    analysis_file: str,
    analysis_sha256: str,
    run: dict[str, Any],
) -> dict[str, Any]:
    if run.get("accepted") is not True:
        fail(f"analysis {analysis_file} contains an unaccepted run")
    phase = run.get("phase")
    run_index = run.get("run_index")
    if (phase, run_index) not in REQUIRED_RUNS:
        fail(f"analysis {analysis_file} contains an unexpected phase or run index")
    capture_file = basename(run.get("capture"), "capture file")
    capture_sha256 = run.get("evidence_sha256")
    if not is_sha256(capture_sha256):
        fail(f"analysis {analysis_file} has an invalid capture digest")
    generic = run.get("generic")
    if not isinstance(generic, dict):
        fail(f"analysis {analysis_file} is missing generic evidence")
    resource_samples = generic.get("resource_samples")
    if (
        isinstance(resource_samples, bool)
        or not isinstance(resource_samples, int)
        or resource_samples <= 0
    ):
        fail(f"analysis {analysis_file} has no resource samples")
    stops = generic.get("stop_events")
    if not isinstance(stops, dict):
        fail(f"analysis {analysis_file} is missing verified zero evidence")
    return {
        "phase": phase,
        "run_index": run_index,
        "capture_file": capture_file,
        "capture_sha256": capture_sha256,
        "analysis_file": analysis_file,
        "analysis_sha256": analysis_sha256,
        "resource_samples": resource_samples,
        "verified_zero": {
            "initial": verified_zero(stops.get("initial"), "capture start"),
            "final": verified_zero(stops.get("final"), "capture end"),
        },
    }


def saved_artifacts(
    value: Any, map_name: str, label: str
) -> dict[str, dict[str, Any]]:
    if not isinstance(value, list):
        fail(f"map reload proof {label} saved artifacts must be a list")
    expected = {
        f"{map_name}.posegraph",
        f"{map_name}.data",
        f"{map_name}.yaml",
        f"{map_name}.pgm",
        f"{map_name}.lineage.json",
    }
    records: dict[str, dict[str, Any]] = {}
    for artifact in value:
        if not isinstance(artifact, dict):
            fail(f"map reload proof {label} saved artifact is invalid")
        name = basename(artifact.get("file"), "saved artifact file")
        size_bytes = artifact.get("size_bytes")
        if (
            isinstance(size_bytes, bool)
            or not isinstance(size_bytes, int)
            or size_bytes <= 0
            or not is_sha256(artifact.get("sha256"))
        ):
            fail(f"map reload proof {label} saved artifact metadata is invalid")
        if name in records:
            fail(f"map reload proof {label} contains duplicate saved artifacts")
        records[name] = {
            "file": name,
            "size_bytes": size_bytes,
            "sha256": artifact["sha256"],
        }
    lineage_name = f"{map_name}.lineage.json"
    if lineage_name not in records:
        fail("map reload proof is missing the saved lineage artifact")
    if set(records) != expected:
        fail("map reload proof saved artifact set is incomplete")
    return records


def proof_snapshot(value: Any, label: str) -> dict[str, Any]:
    if not isinstance(value, dict):
        fail(f"map reload proof {label} snapshot must be an object")
    captured_at_ms = value.get("captured_at_ms")
    if isinstance(captured_at_ms, bool) or not isinstance(captured_at_ms, int):
        fail(f"map reload proof {label} snapshot timestamp is invalid")
    map_metadata = value.get("map")
    if not isinstance(map_metadata, dict) or any(
        not isinstance(map_metadata.get(field), str)
        or not map_metadata[field].strip()
        for field in ("map_id", "map_revision", "grid_revision", "frame_id")
    ):
        fail(f"map reload proof {label} map identity is invalid")
    provider = value.get("provider")
    if not isinstance(provider, dict) or provider.get("state") != "tracking":
        fail(f"map reload proof {label} provider is not tracking")
    instance_id = provider.get("provider_instance_id")
    generation = provider.get("generation")
    last_received_ms = provider.get("last_received_ms")
    stale_after_ms = provider.get("stale_after_ms")
    if (
        not isinstance(instance_id, str)
        or not instance_id.strip()
        or isinstance(generation, bool)
        or not isinstance(generation, int)
        or generation <= 0
        or isinstance(last_received_ms, bool)
        or not isinstance(last_received_ms, int)
        or isinstance(stale_after_ms, bool)
        or not isinstance(stale_after_ms, int)
        or stale_after_ms <= 0
        or not 0 <= captured_at_ms - last_received_ms <= stale_after_ms
    ):
        fail(f"map reload proof {label} provider freshness is invalid")
    localized_pose = value.get("pose")
    if (
        not isinstance(localized_pose, dict)
        or not isinstance(localized_pose.get("covariance"), list)
        or len(localized_pose["covariance"]) != 9
        or not isinstance(localized_pose.get("pose"), dict)
    ):
        fail(f"map reload proof {label} localization covariance is invalid")
    pose_ts_ms = localized_pose["pose"].get("ts_ms")
    if (
        isinstance(pose_ts_ms, bool)
        or not isinstance(pose_ts_ms, int)
        or not 0 <= captured_at_ms - pose_ts_ms <= stale_after_ms
    ):
        fail(f"map reload proof {label} localized pose is stale")
    command = value.get("command")
    if (
        not isinstance(command, dict)
        or command.get("left_cmd") != 0
        or command.get("right_cmd") != 0
    ):
        fail(f"map reload proof {label} snapshot is not stopped")
    return value


def validate_map_reload_proof(
    proof: Any, profile: dict[str, Any]
) -> dict[str, Any]:
    validate(profile, require_values=True)
    calibration_sha256 = hashlib.sha256(canonical_bytes(profile)).hexdigest()
    if (
        not isinstance(proof, dict)
        or proof.get("format") != MAP_PROOF_FORMAT
        or proof.get("ok") is not True
        or proof.get("profile") != profile["profile"]
        or proof.get("calibration_sha256") != calibration_sha256
    ):
        fail("map reload proof identity or calibration digest is invalid")
    map_name = proof.get("map_name")
    if (
        not isinstance(map_name, str)
        or not re.fullmatch(r"[A-Za-z0-9][A-Za-z0-9._-]{0,63}", map_name)
    ):
        fail("map reload proof map name is invalid")

    artifact_sets = proof.get("saved_artifacts")
    if not isinstance(artifact_sets, dict):
        fail("map reload proof saved artifacts are missing")
    before_artifacts = saved_artifacts(
        artifact_sets.get("before"), map_name, "before"
    )
    after_artifacts = saved_artifacts(artifact_sets.get("after"), map_name, "after")
    if before_artifacts != after_artifacts:
        fail("map reload proof saved artifact size or hash changed across reload")

    lineage = proof.get("lineage")
    if (
        not isinstance(lineage, dict)
        or set(lineage) != {"format", "map_id", "map_revision", "frame_id"}
        or lineage.get("format") != "leash-map-lineage-v1"
        or any(
            not isinstance(lineage.get(field), str) or not lineage[field].strip()
            for field in ("map_id", "map_revision", "frame_id")
        )
    ):
        fail("map reload proof lineage is invalid")

    before = proof_snapshot(proof.get("before"), "before")
    after = proof_snapshot(proof.get("after"), "after")
    lineage_identity = {
        field: lineage[field] for field in ("map_id", "map_revision", "frame_id")
    }
    before_identity = {
        field: before["map"][field]
        for field in ("map_id", "map_revision", "frame_id")
    }
    after_identity = {
        field: after["map"][field]
        for field in ("map_id", "map_revision", "frame_id")
    }
    if before_identity != lineage_identity or after_identity != lineage_identity:
        fail("map reload proof lineage changed across save and reload")
    if before["map"]["grid_revision"] != after["map"]["grid_revision"]:
        fail("map reload proof stopped grid revision changed across reload")
    if (
        before["provider"]["provider_instance_id"]
        == after["provider"]["provider_instance_id"]
    ):
        fail("map reload proof provider instance did not change")
    if after["provider"]["generation"] <= before["provider"]["generation"]:
        fail("map reload proof provider generation did not advance")

    leash = proof.get("leash")
    if (
        not isinstance(leash, dict)
        or isinstance(leash.get("pid"), bool)
        or not isinstance(leash.get("pid"), int)
        or leash["pid"] <= 0
        or leash.get("unchanged") is not True
    ):
        fail("map reload proof does not preserve the Leash process")
    entry_zero = verified_zero(leash.get("entry_verified_zero"), "map reload entry")
    exit_zero = verified_zero(leash.get("exit_verified_zero"), "map reload exit")
    if (
        exit_zero["command_sequence"] <= entry_zero["command_sequence"]
        or exit_zero["adapter_sample_sequence"]
        <= entry_zero["adapter_sample_sequence"]
    ):
        fail("map reload exit verified zero evidence must be newer than entry")

    container = proof.get("container")
    if (
        not isinstance(container, dict)
        or container.get("running") is not True
        or container.get("oom_killed") is not False
    ):
        fail("map reload proof container is unhealthy")
    if proof.get("recorder_issues_motion") is not False:
        fail("map reload proof recorder must not issue motion")
    return proof


def build_manifest(
    profile: dict[str, Any],
    analyses: list[tuple[str, dict[str, Any]]],
    map_proof: tuple[str, dict[str, Any]],
    body_artifact: dict[str, Any],
    watchdogs: dict[str, dict[str, Any]],
) -> dict[str, Any]:
    validate(profile, require_values=True)
    calibration_sha256 = hashlib.sha256(canonical_bytes(profile)).hexdigest()
    captures: list[dict[str, Any]] = []
    for analysis_name, analysis in analyses:
        analysis_file = basename(analysis_name, "analysis file")
        if not isinstance(analysis, dict) or analysis.get("format") != ANALYSIS_FORMAT:
            fail(f"analysis {analysis_file} has an unsupported format")
        if analysis.get("ok") is not True or analysis.get("profile") != profile["profile"]:
            fail(f"analysis {analysis_file} is not accepted for this profile")
        analysis_sha256 = hashlib.sha256(canonical_json_bytes(analysis)).hexdigest()
        runs = analysis.get("runs")
        if not isinstance(runs, list) or not runs:
            fail(f"analysis {analysis_file} contains no runs")
        captures.extend(
            capture_entry(analysis_file, analysis_sha256, run) for run in runs
        )

    order = {key: index for index, key in enumerate(REQUIRED_RUNS)}
    captures.sort(key=lambda entry: order[(entry["phase"], entry["run_index"])])

    map_name, map_document = map_proof
    map_file = basename(map_name, "map reload proof file")
    validated_map = validate_map_reload_proof(map_document, profile)
    map_reload = {
        "file": map_file,
        "sha256": hashlib.sha256(canonical_json_bytes(validated_map)).hexdigest(),
        "format": validated_map["format"],
        "profile": validated_map["profile"],
        "calibration_sha256": validated_map["calibration_sha256"],
        "map_id": validated_map["lineage"]["map_id"],
        "map_revision": validated_map["lineage"]["map_revision"],
        "grid_revision": validated_map["before"]["map"]["grid_revision"],
        "provider_generation_before": validated_map["before"]["provider"]["generation"],
        "provider_generation_after": validated_map["after"]["provider"]["generation"],
        "accepted": True,
    }

    manifest = {
        "ok": True,
        "format": FORMAT,
        "profile": profile["profile"],
        "calibration_sha256": calibration_sha256,
        "captures": captures,
        "map_reload": map_reload,
        "body_artifact": copy.deepcopy(body_artifact),
        "watchdogs": copy.deepcopy(watchdogs),
        "readiness": {
            "calibration_accepted": True,
            "mapping_ready": True,
            "physical_autonomous_exploration_ready": watchdog_readiness(watchdogs),
        },
    }
    return validate_manifest(manifest, profile)


def validate_manifest(
    manifest: Any, profile: dict[str, Any]
) -> dict[str, Any]:
    validate(profile, require_values=True)
    if not isinstance(manifest, dict) or manifest.get("format") != FORMAT:
        fail(f"acceptance manifest format must be {FORMAT}")
    if manifest.get("profile") != profile["profile"]:
        fail("acceptance manifest profile does not match calibration profile")
    calibration_sha256 = hashlib.sha256(canonical_bytes(profile)).hexdigest()
    if manifest.get("calibration_sha256") != calibration_sha256:
        fail("acceptance manifest calibration digest does not match the profile")

    captures = manifest.get("captures")
    if not isinstance(captures, list):
        fail("acceptance manifest captures must be a list")
    observed_runs: list[tuple[str, int]] = []
    for entry in captures:
        if not isinstance(entry, dict):
            fail("acceptance manifest capture entries must be objects")
        phase = entry.get("phase")
        run_index = entry.get("run_index")
        observed_runs.append((phase, run_index))
        basename(entry.get("capture_file"), "capture file")
        basename(entry.get("analysis_file"), "analysis file")
        if not is_sha256(entry.get("capture_sha256")) or not is_sha256(
            entry.get("analysis_sha256")
        ):
            fail("acceptance manifest capture hashes must be lowercase SHA-256 values")
        resource_samples = entry.get("resource_samples")
        if (
            isinstance(resource_samples, bool)
            or not isinstance(resource_samples, int)
            or resource_samples <= 0
        ):
            fail("acceptance manifest capture has no resource samples")
        stops = entry.get("verified_zero")
        if not isinstance(stops, dict):
            fail("acceptance manifest capture is missing verified zero evidence")
        initial_zero = verified_zero(stops.get("initial"), "capture start")
        final_zero = verified_zero(stops.get("final"), "capture end")
        if (
            final_zero["command_sequence"] <= initial_zero["command_sequence"]
            or final_zero["adapter_sample_sequence"]
            <= initial_zero["adapter_sample_sequence"]
        ):
            fail("capture end verified zero evidence must be newer than capture start")
    if tuple(observed_runs) != REQUIRED_RUNS:
        fail("acceptance manifest requires stationary, straight, turn, and square runs 1, 2, and 3")

    map_reload = manifest.get("map_reload")
    if (
        not isinstance(map_reload, dict)
        or map_reload.get("format") != MAP_PROOF_FORMAT
        or map_reload.get("accepted") is not True
        or map_reload.get("profile") != profile["profile"]
        or map_reload.get("calibration_sha256") != calibration_sha256
        or not is_sha256(map_reload.get("sha256"))
    ):
        fail("acceptance manifest map reload proof is invalid")
    basename(map_reload.get("file"), "map reload proof file")

    body_artifact = manifest.get("body_artifact")
    if (
        not isinstance(body_artifact, dict)
        or not is_sha256(body_artifact.get("sha256"))
        or body_artifact.get("reviewed") is not True
        or body_artifact.get("no_persistent_robot_body_artifact") is not True
    ):
        fail("acceptance manifest body artifact is invalid or unreviewed")
    basename(body_artifact.get("file"), "body artifact file")

    autonomous_ready = watchdog_readiness(manifest.get("watchdogs"))
    expected_readiness = {
        "calibration_accepted": True,
        "mapping_ready": True,
        "physical_autonomous_exploration_ready": autonomous_ready,
    }
    if manifest.get("readiness") != expected_readiness:
        fail("acceptance manifest readiness does not match its evidence")
    if manifest.get("ok") is not True:
        fail("acceptance manifest must mark accepted Stage 1 evidence as ok")

    recorded_digest = profile["measurement"].get("acceptance_manifest_sha256")
    if recorded_digest is not None and recorded_digest != manifest_sha256(manifest):
        fail("accepted profile does not reference this acceptance manifest")
    return manifest


def load_json(path: pathlib.Path) -> dict[str, Any]:
    value = json.loads(path.read_text())
    if not isinstance(value, dict):
        fail(f"{path} must contain a JSON object")
    return value


def main() -> int:
    parser = argparse.ArgumentParser()
    subparsers = parser.add_subparsers(dest="command", required=True)

    build = subparsers.add_parser("build")
    build.add_argument("--profile", required=True, type=pathlib.Path)
    build.add_argument("--map-proof", required=True, type=pathlib.Path)
    build.add_argument("--body-artifact", required=True, type=pathlib.Path)
    build.add_argument("--body-artifact-reviewed", action="store_true")
    build.add_argument("--watchdogs", type=pathlib.Path)
    build.add_argument("--output", required=True, type=pathlib.Path)
    build.add_argument("analyses", nargs="+", type=pathlib.Path)

    validate_command = subparsers.add_parser("validate")
    validate_command.add_argument("--profile", required=True, type=pathlib.Path)
    validate_command.add_argument("manifest", type=pathlib.Path)

    digest = subparsers.add_parser("digest")
    digest.add_argument("manifest", type=pathlib.Path)
    args = parser.parse_args()

    try:
        if args.command == "digest":
            print(manifest_sha256(load_json(args.manifest)))
            return 0

        profile = load_json(args.profile)
        if args.command == "validate":
            manifest = validate_manifest(load_json(args.manifest), profile)
            print(json.dumps({"ok": True, "sha256": manifest_sha256(manifest)}))
            return 0

        body_path = args.body_artifact
        body_artifact = {
            "file": body_path.name,
            "sha256": hashlib.sha256(body_path.read_bytes()).hexdigest(),
            "reviewed": args.body_artifact_reviewed,
            "no_persistent_robot_body_artifact": args.body_artifact_reviewed,
        }
        watchdogs = (
            load_json(args.watchdogs)
            if args.watchdogs is not None
            else default_watchdog_status()
        )
        manifest = build_manifest(
            profile,
            [(path.name, load_json(path)) for path in args.analyses],
            (args.map_proof.name, load_json(args.map_proof)),
            body_artifact,
            watchdogs,
        )
        args.output.write_text(json.dumps(manifest, indent=2, sort_keys=True) + "\n")
        print(json.dumps({"ok": True, "sha256": manifest_sha256(manifest)}))
        return 0
    except (OSError, json.JSONDecodeError, KeyError, TypeError, ValueError) as error:
        print(f"calibration acceptance error: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
