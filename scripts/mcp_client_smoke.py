"""Ask an AI client (Codex CLI or Claude Code CLI) to use the mIV MCP tools, then summarize.

The MCP server is registered only for this run (command-line config), so the user's
~/.codex/config.toml and Claude settings are not changed. mIV must already be running
with the same data dir (normally the disposable portable copy).

Usage:
  python -I scripts/mcp_client_smoke.py [--client codex|claude] [--exe PATH]
                                        [--data-dir PATH] [--prompt TEXT | --prompt-file FILE]

Output: target/mcp-client-smoke/<time>-<client>.jsonl (raw events) and a summary on stdout:
every tool call with its arguments and outcome, then the final answer.
Exit code: 0 if the client finished and at least one mIV tool call succeeded, else 1.
"""
import argparse
import datetime
import json
import os
import subprocess
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
SERVER = "mivtrial"
DEFAULT_PROMPT = (
    f"{SERVER} の MCP tool だけを使って、mIV のタグ一覧と、"
    "タグ cat が付いた画像のファイル名を答えてください。ファイルやシェルは使わないこと。"
)
APPDATA = os.environ.get("APPDATA", "")
CODEX_JS = os.path.join(APPDATA, "npm", "node_modules", "@openai", "codex", "bin", "codex.js")
CLAUDE_EXE = os.path.join(APPDATA, "npm", "node_modules", "@anthropic-ai", "claude-code", "bin", "claude.exe")


def toml_literal(s):
    if "'" in s:
        sys.exit(f"path contains a quote: {s}")
    return f"'{s}'"


def codex_cmd(exe, data_dir, prompt, model, effort):
    return [
        "node", CODEX_JS, "exec", "--json", "--skip-git-repo-check",
        "--model", model, "-c", f'model_reasoning_effort="{effort}"', "--sandbox", "read-only",
        "-c", f"mcp_servers.{SERVER}.command={toml_literal(exe)}",
        "-c", f"mcp_servers.{SERVER}.args=['--data-dir',{toml_literal(data_dir)}]",
        prompt,
    ]


def claude_cmd(exe, data_dir, prompt, out_dir):
    cfg = os.path.join(out_dir, "claude-mcp-config.json")
    with open(cfg, "w", encoding="utf-8") as f:
        json.dump({"mcpServers": {SERVER: {"command": exe, "args": ["--data-dir", data_dir]}}}, f, indent=2)
    return [
        CLAUDE_EXE, "-p", prompt, "--mcp-config", cfg, "--strict-mcp-config",
        "--output-format", "stream-json", "--verbose",
        "--allowedTools", f"mcp__{SERVER}__*",
    ]


def summarize_codex(events):
    calls, answer, ok = [], None, False
    for ev in events:
        item = ev.get("item") or {}
        if ev.get("type") == "item.completed" and item.get("type") == "mcp_tool_call":
            err = item.get("error")
            status = "error: " + err["message"].splitlines()[-1].strip() if err else item.get("status")
            ok |= not err and item.get("server") == SERVER
            calls.append((item.get("tool"), item.get("arguments"), status))
        if ev.get("type") == "item.completed" and item.get("type") == "agent_message":
            answer = item.get("text")
    return calls, answer, ok


def summarize_claude(events):
    calls, pending, answer, ok = [], {}, None, False
    for ev in events:
        msg = ev.get("message") or {}
        content = msg.get("content")
        for part in content if isinstance(content, list) else []:
            if part.get("type") == "tool_use":
                pending[part.get("id")] = (part.get("name"), part.get("input"))
            elif part.get("type") == "tool_result":
                name, args = pending.pop(part.get("tool_use_id"), ("?", None))
                err = part.get("is_error")
                ok |= not err and str(name).startswith(f"mcp__{SERVER}__")
                text = part.get("content")
                if isinstance(text, list):
                    text = " ".join(c.get("text", "") for c in text if isinstance(c, dict))
                calls.append((name, args, "error: " + str(text)[:200] if err else "completed"))
        if ev.get("type") == "result":
            answer = ev.get("result")
            if ev.get("is_error"):
                ok = False
    return calls, answer, ok


def main():
    sys.stdout.reconfigure(errors="replace")
    ap = argparse.ArgumentParser()
    ap.add_argument("--client", choices=["codex", "claude"], default="codex")
    ap.add_argument("--exe", default=os.path.join(ROOT, "target", "debug", "mimageviewer-mcp.exe"))
    ap.add_argument("--data-dir", default=os.path.join(ROOT, "target", "portable-smoke", "data"))
    ap.add_argument("--prompt")
    ap.add_argument("--prompt-file")
    ap.add_argument("--model", default="gpt-6.1-sol")
    ap.add_argument("--effort", default="low")
    ap.add_argument("--timeout", type=int, default=600)
    a = ap.parse_args()
    prompt = a.prompt or DEFAULT_PROMPT
    if a.prompt_file:
        with open(a.prompt_file, encoding="utf-8-sig") as f:
            prompt = f.read()
    exe, data_dir = os.path.abspath(a.exe), os.path.abspath(a.data_dir)
    for p in (exe, data_dir):
        if not os.path.exists(p):
            sys.exit(f"not found: {p}")
    out_dir = os.path.join(ROOT, "target", "mcp-client-smoke")
    os.makedirs(out_dir, exist_ok=True)
    stamp = datetime.datetime.now().strftime("%Y%m%d-%H%M%S")
    log = os.path.join(out_dir, f"{stamp}-{a.client}.jsonl")
    if a.client == "codex":
        cmd = codex_cmd(exe, data_dir, prompt, a.model, a.effort)
    else:
        cmd = claude_cmd(exe, data_dir, prompt, out_dir)
    with open(log, "w", encoding="utf-8") as f:
        proc = subprocess.run(cmd, stdin=subprocess.DEVNULL, stdout=f, stderr=subprocess.PIPE,
                              cwd=out_dir, timeout=a.timeout)
    events = []
    with open(log, encoding="utf-8", errors="replace") as f:
        for line in f:
            line = line.strip()
            if line.startswith("{"):
                try:
                    events.append(json.loads(line))
                except ValueError:
                    pass
    summarize = summarize_codex if a.client == "codex" else summarize_claude
    calls, answer, ok = summarize(events)
    print(f"client: {a.client}  exit: {proc.returncode}  log: {log}")
    print(f"server: {exe}  data-dir: {data_dir}")
    for ev in events:
        for s in ev.get("mcp_servers") or []:
            print(f"  mcp server {s.get('name')}: {s.get('status')}")
    for i, (tool, args, status) in enumerate(calls, 1):
        print(f"  call {i}: {tool} {json.dumps(args, ensure_ascii=False)} -> {status}")
    if not calls:
        print("  (no tool calls)")
    print("answer:")
    print(answer or "(none)")
    if proc.returncode != 0:
        print("stderr (tail):")
        print("\n".join(proc.stderr.decode("utf-8", "replace").splitlines()[-15:]))
    sys.exit(0 if proc.returncode == 0 and ok else 1)


if __name__ == "__main__":
    main()
