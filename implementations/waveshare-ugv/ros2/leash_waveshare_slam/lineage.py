"""Stable map-lineage and provider-instance helpers."""

from __future__ import annotations

import hashlib
import json
import uuid
from pathlib import Path
from typing import Any


MAP_LINEAGE_FORMAT = "leash-map-lineage-v1"
_LINEAGE_KEYS = {"format", "map_id", "map_revision", "frame_id"}


def grid_revision(map_sample: dict[str, Any]) -> str:
    canonical = {
        "frame_id": map_sample["frame_id"],
        "width": int(map_sample["width"]),
        "height": int(map_sample["height"]),
        "resolution_m": float(map_sample["resolution_m"]),
        "origin": map_sample["origin"],
        "cells": [int(value) for value in map_sample["cells"]],
    }
    encoded = json.dumps(canonical, sort_keys=True, separators=(",", ":")).encode()
    return hashlib.sha256(encoded).hexdigest()


def load_active_lineage(path: Path) -> dict[str, str]:
    try:
        value = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as error:
        raise ValueError(f"invalid active map lineage: {error}") from error
    if not isinstance(value, dict) or set(value) != _LINEAGE_KEYS:
        raise ValueError("invalid active map lineage fields")
    if value.get("format") != MAP_LINEAGE_FORMAT:
        raise ValueError("invalid active map lineage format")
    if any(not isinstance(value.get(key), str) or not value[key].strip() for key in _LINEAGE_KEYS):
        raise ValueError("invalid active map lineage values")
    return {key: value[key] for key in ("format", "map_id", "map_revision", "frame_id")}


def create_provider_instance_id() -> str:
    return uuid.uuid4().hex
