#!/usr/bin/env python3
"""Cuánto de lo que lee Claude Code en cada turno viene de explorar código.

Cada resultado de herramienta se queda en el contexto y se vuelve a leer en cada turno
siguiente (hasta compactar), así que se cuenta por los turnos que vive. Se compara con
toda la entrada de la sesión (input + cache, deduplicado por mensaje).

  scripts/measure-exploration.py                 # el proyecto de la carpeta actual
  scripts/measure-exploration.py ~/.claude/projects/-Users-yo-app …
  scripts/measure-exploration.py --since 2026-10-01   # solo sesiones desde esa fecha
  scripts/measure-exploration.py --self-test

Tokens ≈ caracteres / 4. Solo el hilo principal: los subagentes no cuentan (es justo lo
que se ahorra al delegar).
"""
import collections
import glob
import json
import os
import re
import sys

EXPLORE_TOOLS = {"Read", "Grep", "Glob", "LS"}
# Lo mismo que cuenta el mod (claude-plugin/hooks/register.tsx).
EXPLORE_SHELL = re.compile(
    r"^\s*(?:cd\s+\S+\s*&&\s*)?(?:rg|grep|egrep|find|fd|ls|tree|cat|head|tail|sed\s+-n|wc|awk)\b"
)


def kind(name, args):
    if name in EXPLORE_TOOLS:
        return "explore"
    if name == "Bash" and EXPLORE_SHELL.match(args.get("command") or ""):
        return "explore"
    if name in ("Task", "Agent"):
        return "subagent"
    return "other"


def size(content):
    if isinstance(content, str):
        return len(content) // 4
    if isinstance(content, list):
        return sum(len(c.get("text", "")) for c in content if isinstance(c, dict)) // 4
    return 0


def measure(lines, since=""):
    """(entrada total, tokens-turno por tipo, llamadas por tipo, turnos)."""
    total, weight, calls, turns = 0, collections.Counter(), collections.Counter(), 0
    pending, live, seen = {}, [], set()
    for line in lines:
        try:
            m = json.loads(line)
        except ValueError:
            continue
        if m.get("isSidechain") or (since and (m.get("timestamp") or "") < since):
            continue
        if m.get("type") == "system" and m.get("subtype") == "compact_boundary":
            live = []
            continue
        msg = m.get("message") or {}
        content = msg.get("content")
        if m.get("type") == "assistant":
            mid = msg.get("id")
            if mid and mid not in seen:
                seen.add(mid)
                turns += 1
                u = msg.get("usage") or {}
                total += sum(
                    u.get(k) or 0
                    for k in ("input_tokens", "cache_creation_input_tokens", "cache_read_input_tokens")
                )
                for k, t in live:
                    weight[k] += t
            for b in content if isinstance(content, list) else []:
                if isinstance(b, dict) and b.get("type") == "tool_use":
                    k = kind(b.get("name", ""), b.get("input") or {})
                    pending[b.get("id")] = k
                    calls[k] += 1
        elif m.get("type") == "user" and isinstance(content, list):
            for b in content:
                if isinstance(b, dict) and b.get("type") == "tool_result":
                    live.append((pending.get(b.get("tool_use_id"), "other"), size(b.get("content"))))
    return total, weight, calls, turns


def project_dir():
    root = os.environ.get("CLAUDE_CONFIG_DIR") or os.path.expanduser("~/.claude")
    return os.path.join(root, "projects", re.sub(r"[^A-Za-z0-9]", "-", os.getcwd()))


def report(path, since):
    files = glob.glob(os.path.join(path, "*.jsonl"))
    lines = (line for f in files for line in open(f, errors="ignore"))
    total, weight, calls, turns = measure(lines, since)
    share = 100 * weight["explore"] / max(total, 1)
    print(f"{os.path.basename(path)}: {len(files)} sesiones, {turns:,} turnos")
    print(f"  explorar: {share:.1f} % de toda la entrada ({calls['explore']:,} llamadas)")
    print(f"  subagentes: {calls['subagent']:,} llamadas")


def self_test():
    use = lambda i, name, args: {"type": "assistant", "message": {"id": f"a{i}", "usage": {"input_tokens": 100}, "content": [{"type": "tool_use", "id": f"t{i}", "name": name, "input": args}]}}
    result = lambda i, chars: {"type": "user", "message": {"content": [{"type": "tool_result", "tool_use_id": f"t{i}", "content": "x" * chars}]}}
    lines = [json.dumps(x) for x in [
        use(1, "Bash", {"command": "rg -n main src"}), result(1, 400),  # 100 tokens
        use(2, "Bash", {"command": "cargo build"}), result(2, 40),
        use(3, "Edit", {}),  # el resultado de rg sigue vivo: 2 turnos más
    ]]
    total, weight, calls, turns = measure(lines)
    assert (total, turns) == (300, 3), (total, turns)
    assert weight["explore"] == 200 and calls["explore"] == 1, weight
    print("ok")


if __name__ == "__main__":
    args = sys.argv[1:]
    if "--self-test" in args:
        self_test()
        sys.exit()
    since = ""
    if "--since" in args:
        i = args.index("--since")
        since, args = args[i + 1], args[:i] + args[i + 2 :]
    for path in args or [project_dir()]:
        report(path, since)
