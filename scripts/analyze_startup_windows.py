#!/usr/bin/env python3
"""Print observation-only startup-windows JSONL; no product or desktop interaction."""

import argparse
import json
from pathlib import Path


def read_log(path):
    with Path(path).open(encoding="utf-8") as source:
        return [json.loads(line) for line in source if line.strip()]


def timeline(lines):
    header = next((line for line in lines if line.get("event") == "session"), {})
    yield (
        f"pid={header.get('pid', '?')} ui_thread={header.get('ui_thread_id', '?')} "
        f"STARTUPINFO flags={header.get('startup_dwFlags', 0):#x} "
        f"show={header.get('startup_wShowWindow', '?')}"
    )
    yield "Time is QPC milliseconds from core entry; process t_us and WinEvent OS event time are estimates."
    yield "UI snapshots precede WndProc; WinEvent snapshots describe delivery. CBT cmd is an effective SW_ notification."
    yield "   entry_ms source              hwnd               event                         state / detail"
    records = [line for line in lines if "qpc" in line]
    for row in sorted(records, key=lambda row: row["qpc"]):
        detail = row.get("detail") or {}
        observed = row.get("snapshot") or {}
        delivered = row.get("delivery_snapshot") or {}
        cached = row.get("cached_identity") or {}
        state = []
        if row.get("hwnd") != "0x0":
            state.append(f"generation={row.get('native_generation') if row.get('native_generation') is not None else 'unknown'}")
        if observed:
            state.extend([
                f"valid={int(observed.get('valid', False))}",
                f"vis={int(observed.get('visible', False))}",
                f"WS_VIS={int(observed.get('ws_visible', False))}",
                f"max={int(observed.get('ws_maximize', False))}",
                f"rect={observed.get('rect')}",
                f"dpi={observed.get('dpi')}",
                f"style={observed.get('style', 0):#x}/{observed.get('exstyle', 0):#x}",
                f"owner_pid/tid={observed.get('owner_pid')}/{observed.get('owner_thread_id')}",
            ])
        if "cmd_show" in detail:
            state.append(f"effective_cmd_show={detail['cmd_show']}")
        for key in ("creation_xywh", "xywh", "show_window", "hide_window", "no_activate",
                    "wparam", "lparam", "position", "inner_size", "maximized", "ready"):
            if key in detail:
                state.append(f"{key}={detail[key]}")
        if "flags" in detail:
            state.append(f"SWP={detail['flags']:#x}")
        if "event_us_estimate" in detail:
            state.append(f"OS_event_process_ms~={detail['event_us_estimate'] / 1000:.3f}")
            state.append(f"delivery_age_ms~={detail.get('delivery_age_ms_estimate')}")
        if detail.get("scope_contract", "").startswith("event scope unknown"):
            state.append("event_scope=unknown_at_delivery")
        cloak = delivered.get("cloaked")
        if cloak is not None:
            state.append(f"cloaked_at_delivery={cloak}")
        for label, title in (("creation_title", detail.get("creation_title")),
                             ("title_at_delivery", delivered.get("title")),
                             ("cached_title_last_observed", cached.get("title"))):
            if title is not None:
                state.append(f"{label}={json.dumps(title, ensure_ascii=False)}")
        if "hooks" in detail or "cbt_error" in detail:
            state.append(json.dumps(detail, ensure_ascii=False))
        yield (f"{row.get('entry_us', 0) / 1000:11.3f} {row.get('source', '?'):19} "
               f"{row.get('hwnd', '?'):18} {row.get('event', '?'):29} {' '.join(state)}")
    footer = next((line for line in lines if line.get("event") == "session.end"), {})
    yield f"end: records={footer.get('records', '?')} dropped={footer.get('dropped', '?')} main_visible_commit={footer.get('main_visible_commit_recorded', '?')}"
    yield "Notifications do not intercept all ShowWindow calls or their requested nCmdShow. Foreground events are limited to this process."


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("log", type=Path)
    args = parser.parse_args()
    print("\n".join(timeline(read_log(args.log))))


if __name__ == "__main__":
    main()
