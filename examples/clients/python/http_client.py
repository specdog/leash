#!/usr/bin/env python3
import json
import os
import urllib.request


BASE_URL = os.environ.get("LEASH_URL", "http://127.0.0.1:8000").rstrip("/")


def request_json(method, path):
    request = urllib.request.Request(f"{BASE_URL}{path}", method=method)
    if method == "POST":
        request.data = b""
    with urllib.request.urlopen(request, timeout=5) as response:
        return json.loads(response.read().decode("utf-8"))


def main():
    health = request_json("GET", "/health")
    telemetry = request_json("GET", "/telemetry")
    stop = request_json("POST", "/stop")

    if health.get("ok") is not True:
        raise SystemExit("health did not report ok=true")
    if not telemetry.get("robot") or telemetry.get("profile") != "sim":
        raise SystemExit("telemetry did not look like a sim frame")
    if stop.get("ok") is not True:
        raise SystemExit("stop did not report ok=true")

    print(json.dumps({
        "ok": True,
        "runtime": "python",
        "profile": health["profile"],
        "robot": telemetry["robot"],
        "stop": stop,
    }, sort_keys=True))


if __name__ == "__main__":
    main()
