#!/usr/bin/env python3
"""Convert live telemetry SSE frames into scrubbed leash-replay-v1 JSONL."""

from __future__ import annotations

import argparse
import copy
import json
import pathlib
import sys
from typing import Any, Iterable

FORMAT = "leash-replay-v1"


def set_timestamp(value: dict[str, Any], key: str, timestamp_ms: int) -> None:
    nested = value.get(key)
    if isinstance(nested, dict):
        nested["ts_ms"] = timestamp_ms


def normalize_and_scrub(source: dict[str, Any], timestamp_ms: int) -> dict[str, Any]:
    frame = copy.deepcopy(source)
    telemetry = frame["telemetry"]
    frame["ts_ms"] = timestamp_ms
    telemetry["ts_ms"] = timestamp_ms
    telemetry["robot"] = "pinkie"
    telemetry["profile"] = "replay"
    telemetry["source"] = "replay"
    telemetry["session_id"] = None
    telemetry["workers"] = []
    telemetry["resource"] = None
    camera = telemetry["sensors"]["camera"]
    camera["stream_url"] = None
    camera["snapshot_url"] = None
    raw_frame = telemetry["sensors"]["raw_frame"]
    raw_frame["source"] = "replay"
    raw_frame["last_ms"] = timestamp_ms
    raw_frame["payload"] = None
    for sensor_name in ("range_scan", "imu"):
        sensor = telemetry["sensors"][sensor_name]
        if sensor.get("last_ms") is not None:
            sensor["last_ms"] = timestamp_ms
        if isinstance(sensor.get("sample"), dict):
            sensor["sample"]["ts_ms"] = timestamp_ms

    localization = telemetry["localization"]
    localization["ts_ms"] = timestamp_ms
    if isinstance(localization.get("pose"), dict):
        localization["pose"]["pose"]["ts_ms"] = timestamp_ms
        localization["health"]["last_update_ms"] = timestamp_ms
    provider = telemetry["localization_provider"]
    for field in ("last_update_ms", "last_received_ms"):
        if provider.get(field) is not None:
            provider[field] = timestamp_ms
    for grid_name in ("map", "occupancy_grid", "costmap"):
        grid = telemetry[grid_name]
        grid["ts_ms"] = timestamp_ms
        set_timestamp(grid, "origin", timestamp_ms)
        if isinstance(grid.get("metadata"), dict):
            grid["metadata"]["ts_ms"] = timestamp_ms
            set_timestamp(grid["metadata"], "origin", timestamp_ms)

    frame["health"].update(
        {
            "mode": "replay",
            "replay": True,
            "profile": "replay",
            "uptime_ms": timestamp_ms,
            "physical_actuation_enabled": False,
            "physical_navigation_enabled": False,
            "operator_token": {
                "active": False,
                "owner_id": None,
                "expires_in_ms": None,
                "speed_mode": None,
            },
        }
    )
    frame["command"]["session_id"] = None
    frame["safety"]["physical_actuation_enabled"] = False
    frame["safety"]["physical_navigation_enabled"] = False

    visualization = frame["visualization"]
    visualization["ts_ms"] = timestamp_ms
    visualization["robot"] = "pinkie"
    for field in ("map", "pose", "twist", "path", "occupancy_grid", "costmap"):
        set_timestamp(visualization, field, timestamp_ms)
    visualization["range_scan"] = copy.deepcopy(telemetry["sensors"]["range_scan"])
    visualization["imu"] = copy.deepcopy(telemetry["sensors"]["imu"])
    visualization["localization"] = copy.deepcopy(localization)
    visualization["localization_provider"] = copy.deepcopy(provider)
    visualization["map"] = copy.deepcopy(telemetry["map"])
    visualization["occupancy_grid"] = copy.deepcopy(telemetry["occupancy_grid"])
    visualization["costmap"] = copy.deepcopy(telemetry["costmap"])
    return frame


def parse_sse(lines: Iterable[str]) -> list[dict[str, Any]]:
    frames = []
    for line in lines:
        line = line.strip()
        if line.startswith("data:"):
            value = json.loads(line.removeprefix("data:").strip())
            if value.get("kind") == "telemetry":
                frames.append(value)
    return frames


def replay_events(frames: list[dict[str, Any]]) -> list[dict[str, Any]]:
    if not frames:
        raise ValueError("telemetry stream produced no frames")
    first_timestamp = int(frames[0]["ts_ms"])
    events = []
    for index, source in enumerate(frames):
        timestamp = max(0, int(source["ts_ms"]) - first_timestamp)
        frame = normalize_and_scrub(source, timestamp)
        sequence = index * 4
        for offset, kind, data in (
            (0, "telemetry", frame),
            (1, "sensors", frame["telemetry"]["sensors"]),
            (2, "camera", frame["telemetry"]["sensors"]["camera"]),
            (3, "command", frame["command"]),
        ):
            events.append(
                {
                    "format": FORMAT,
                    "ts_ms": timestamp,
                    "seq": sequence + offset,
                    "kind": kind,
                    "data": data,
                }
            )
    return events


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("sse_input", type=pathlib.Path)
    parser.add_argument("output")
    args = parser.parse_args()
    try:
        output = None if args.output == "-" else pathlib.Path(args.output)
        if output is not None and output.exists():
            raise ValueError("output already exists")
        frames = parse_sse(args.sse_input.read_text().splitlines())
        events = replay_events(frames)
        rendered = "\n".join(json.dumps(event, separators=(",", ":")) for event in events) + "\n"
        if output is None:
            print(rendered, end="")
        else:
            output.write_text(rendered)
            print(json.dumps({"ok": True, "frames": len(frames), "events": len(events)}))
    except (OSError, json.JSONDecodeError, KeyError, TypeError, ValueError) as error:
        print(f"calibration replay error: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
