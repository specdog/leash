#!/usr/bin/env python3
"""Verify a complete private Pinkie physical-navigation evidence set."""

from __future__ import annotations

import argparse
import json
import pathlib
import sys
from typing import Any

FORMAT = "leash-waveshare-ugv-navigation-proof-v1"


def load(path: pathlib.Path) -> dict[str, Any]:
    value = json.loads(path.read_text())
    if value.get("format") != FORMAT or value.get("ok") is not True:
        raise ValueError(f"{path} is not a successful {FORMAT} proof")
    safety = value.get("safety", {})
    required = (
        "operator_confirmed",
        "clear_floor",
        "spotter_present",
        "estop_reachable",
        "final_motor_stop",
    )
    if not all(safety.get(field) is True for field in required):
        raise ValueError(f"{path} is missing supervised safety or final-stop evidence")
    if safety.get("low_speed_cap") != 0.22:
        raise ValueError(f"{path} did not prove the low speed cap")
    artifacts = value.get("artifacts", {})
    if artifacts.get("replay_validated") is not True:
        raise ValueError(f"{path} replay was not validated")
    for field in ("capture_sha256", "replay_sha256"):
        digest = artifacts.get(field, "")
        if len(digest) != 64 or any(character not in "0123456789abcdef" for character in digest):
            raise ValueError(f"{path} has an invalid {field}")
    if value.get("samples", 0) < 1 or value.get("resource_samples", 0) < 1:
        raise ValueError(f"{path} has no telemetry/resource samples")
    return value


def verify(values: list[dict[str, Any]]) -> dict[str, Any]:
    half_meter = [value for value in values if value.get("phase") == "half-meter"]
    map_goals = [value for value in values if value.get("phase") == "map-goal"]
    patrols = [value for value in values if value.get("phase") == "patrol"]
    if len(half_meter) != 1:
        raise ValueError("evidence requires exactly one half-meter goal")
    if half_meter[0].get("final_error_m", 1.0) > 0.15:
        raise ValueError("half-meter goal final error exceeds 0.15 m")
    if len(map_goals) < 3:
        raise ValueError("evidence requires at least three consecutive map goals")
    run_ids = [value.get("run_id") for value in map_goals]
    if len(set(run_ids)) != len(run_ids):
        raise ValueError("map goal run ids must be unique")
    if any(value.get("final_error_m") is None for value in map_goals):
        raise ValueError("map goals require final pose error evidence")
    if len(patrols) != 1:
        raise ValueError("evidence requires exactly one bounded patrol")
    patrol = patrols[0]
    if not patrol.get("target", {}).get("zone_id"):
        raise ValueError("patrol proof has no zone id")
    if patrol.get("terminal", {}).get("status") != "completed":
        raise ValueError("patrol did not complete its terminal waypoint")
    ordered = sorted(values, key=lambda value: value.get("started_at", ""))
    return {
        "ok": True,
        "format": "leash-waveshare-ugv-navigation-acceptance-v1",
        "runs": len(values),
        "run_ids": [value.get("run_id") for value in ordered],
        "half_meter": 1,
        "map_goals": len(map_goals),
        "bounded_patrols": 1,
    }


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("summaries", nargs="+", type=pathlib.Path)
    args = parser.parse_args()
    try:
        print(json.dumps(verify([load(path) for path in args.summaries]), separators=(",", ":")))
    except (OSError, json.JSONDecodeError, TypeError, ValueError) as error:
        print(f"physical navigation evidence error: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
