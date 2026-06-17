#!/usr/bin/env python3
import json
import select
import subprocess
import sys
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]


def read_json_line(proc: subprocess.Popen[str], timeout: float = 10.0) -> dict:
    ready, _, _ = select.select([proc.stdout], [], [], timeout)
    if not ready:
        stderr = proc.stderr.read() if proc.stderr else ""
        raise RuntimeError(f"timed out waiting for MCP response\nstderr:\n{stderr}")
    line = proc.stdout.readline()
    if not line:
        stderr = proc.stderr.read() if proc.stderr else ""
        raise RuntimeError(f"MCP process closed stdout\nstderr:\n{stderr}")
    return json.loads(line)


def send(proc: subprocess.Popen[str], message: dict) -> None:
    assert proc.stdin is not None
    proc.stdin.write(json.dumps(message, separators=(",", ":")) + "\n")
    proc.stdin.flush()


def main() -> int:
    proc = subprocess.Popen(
        ["cargo", "run", "--quiet", "--", "serve", "mcp", "--profile", "sim"],
        cwd=ROOT,
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
        bufsize=1,
    )
    try:
        send(
            proc,
            {
                "jsonrpc": "2.0",
                "id": 1,
                "method": "initialize",
                "params": {
                    "protocolVersion": "2025-11-25",
                    "capabilities": {},
                    "clientInfo": {"name": "leash-smoke", "version": "0.1.0"},
                },
            },
        )
        init = read_json_line(proc)
        assert init["id"] == 1, init
        assert "result" in init, init

        send(proc, {"jsonrpc": "2.0", "method": "notifications/initialized", "params": {}})

        send(proc, {"jsonrpc": "2.0", "id": 2, "method": "tools/list", "params": {}})
        tools = read_json_line(proc)
        names = {tool["name"] for tool in tools["result"]["tools"]}
        required = {"health", "capabilities", "observe", "invoke_capability", "stop", "estop", "capture"}
        missing = required - names
        assert not missing, f"missing tools: {sorted(missing)}"

        send(
            proc,
            {
                "jsonrpc": "2.0",
                "id": 3,
                "method": "tools/call",
                "params": {"name": "health", "arguments": {}},
            },
        )
        health = read_json_line(proc)
        assert health["id"] == 3, health
        assert "result" in health, health
        payload = json.loads(health["result"]["content"][0]["text"])
        assert payload["ok"] is True, payload
        assert payload["profile"] == "sim", payload

        print("mcp smoke ok")
        return 0
    finally:
        if proc.stdin:
            proc.stdin.close()
        try:
            proc.wait(timeout=3)
        except subprocess.TimeoutExpired:
            proc.terminate()
            proc.wait(timeout=3)


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except Exception as exc:
        print(f"mcp smoke failed: {exc}", file=sys.stderr)
        raise

