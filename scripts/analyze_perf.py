#!/usr/bin/env python3
"""
mimageviewer パフォーマンスイベントログ (perf_events.jsonl) の解析ツール。

`mimageviewer.exe --perf-log` で起動すると
`%APPDATA%\\mimageviewer\\logs\\perf_events.jsonl` が作成される。
このスクリプトでそれを読み込み、入力→表示レイテンシやサムネイル優先度違反を分析する。

使い方:
    python scripts/analyze_perf.py <path/to/perf_events.jsonl> <subcommand> [options]

サブコマンド:
    summary             全イベントの件数とカテゴリ別 breakdown を表示
    latency             seq ごとに input → *.ready / *.paint のレイテンシを集計
    priority            可視サムネイルが未 decode のうちに非可視が先に処理された違反を検出
    dump <seq>          指定 seq に紐づく全イベントを時系列で列挙
    timeline [seq]      ガントチャート (matplotlib が必要)。seq 指定可
    thumbs              サムネイル decode 時間の分布 (priority=H/L 別)

依存:
    標準ライブラリのみ必須。timeline は matplotlib、latency 詳細統計は任意で pandas。
"""
from __future__ import annotations

import argparse
import json
import sys
from collections import defaultdict
from pathlib import Path


# -----------------------------------------------------------------------
# ロード
# -----------------------------------------------------------------------

def load_events(path: Path) -> list[dict]:
    """JSON Lines をイベント配列に読み込む。壊れた行はスキップ。"""
    events: list[dict] = []
    with path.open("r", encoding="utf-8") as f:
        for lineno, line in enumerate(f, 1):
            line = line.strip()
            if not line:
                continue
            try:
                events.append(json.loads(line))
            except json.JSONDecodeError as e:
                print(
                    f"warning: line {lineno}: {e}",
                    file=sys.stderr,
                )
    return events


def fmt_key(key: str | None, maxlen: int = 60) -> str:
    if key is None:
        return "-"
    if len(key) > maxlen:
        return "…" + key[-(maxlen - 1) :]
    return key


# -----------------------------------------------------------------------
# summary
# -----------------------------------------------------------------------

def cmd_summary(events: list[dict]) -> None:
    if not events:
        print("(イベント 0 件)")
        return

    t_min = min(e.get("t", 0.0) for e in events)
    t_max = max(e.get("t", 0.0) for e in events)
    print(f"イベント件数: {len(events)}")
    print(f"計測時間    : {t_max - t_min:.2f} 秒")
    print()

    # カテゴリ × 種別
    counts: dict[tuple[str, str], int] = defaultdict(int)
    for e in events:
        counts[(e.get("cat", "?"), e.get("kind", "?"))] += 1

    print(f"{'cat':<8} {'kind':<20} count")
    print("-" * 40)
    for (cat, kind), c in sorted(counts.items()):
        print(f"{cat:<8} {kind:<20} {c}")
    print()

    # seq 数
    seqs = {e.get("seq") for e in events if e.get("seq", 0) > 0}
    print(f"ユニーク input_seq 数: {len(seqs)}")

    # フレーム数
    frame_count = sum(1 for e in events if e.get("cat") == "frame" and e.get("kind") == "begin")
    if frame_count:
        duration = t_max - t_min
        fps = frame_count / duration if duration > 0 else 0.0
        print(f"フレーム数           : {frame_count} (平均 {fps:.1f} fps)")


# -----------------------------------------------------------------------
# latency
# -----------------------------------------------------------------------

def cmd_latency(events: list[dict]) -> None:
    """seq ごとに input.* から *.ready / *.paint / thumb.ready までのレイテンシを算出。"""
    # seq → input.t
    input_t: dict[int, tuple[float, str]] = {}
    # seq → list of (cat.kind, t)
    downstream: dict[int, list[tuple[str, float]]] = defaultdict(list)

    for e in events:
        seq = e.get("seq", 0)
        if not seq:
            continue
        cat = e.get("cat", "")
        kind = e.get("kind", "")
        t = e.get("t", 0.0)
        if cat == "input":
            # 最初の input イベントだけ採用
            input_t.setdefault(seq, (t, kind))
        else:
            if kind in ("ready", "paint", "job_ready", "decode_end"):
                downstream[seq].append((f"{cat}.{kind}", t))

    print(f"{'seq':>6} {'input_kind':<16} {'fs.ready (ms)':>14} {'fs.paint (ms)':>14} {'thumb.first (ms)':>18} {'ai.job_ready (ms)':>18}")
    print("-" * 105)

    fs_ready = []
    fs_paint = []
    thumb_first = []
    ai_ready = []

    for seq in sorted(input_t.keys()):
        t0, kind = input_t[seq]
        downs = downstream.get(seq, [])
        fs_r = next((t for ck, t in downs if ck == "fs.ready"), None)
        fs_p = next((t for ck, t in downs if ck == "fs.paint"), None)
        thumbs = [t for ck, t in downs if ck == "thumb.ready"]
        thumb_t = min(thumbs) if thumbs else None
        ai_r = next((t for ck, t in downs if ck == "ai.job_ready"), None)

        def d(x):
            return f"{(x - t0) * 1000:>14.1f}" if x is not None else f"{'-':>14}"

        def d18(x):
            return f"{(x - t0) * 1000:>18.1f}" if x is not None else f"{'-':>18}"

        print(f"{seq:>6} {kind:<16} {d(fs_r)} {d(fs_p)} {d18(thumb_t)} {d18(ai_r)}")

        if fs_r is not None: fs_ready.append((fs_r - t0) * 1000)
        if fs_p is not None: fs_paint.append((fs_p - t0) * 1000)
        if thumb_t is not None: thumb_first.append((thumb_t - t0) * 1000)
        if ai_r is not None: ai_ready.append((ai_r - t0) * 1000)

    def stats(name: str, xs: list[float]) -> None:
        if not xs:
            print(f"  {name:<18} n=0")
            return
        xs_sorted = sorted(xs)
        n = len(xs)
        p50 = xs_sorted[n // 2]
        p95 = xs_sorted[min(n - 1, int(n * 0.95))]
        p99 = xs_sorted[min(n - 1, int(n * 0.99))]
        print(
            f"  {name:<18} n={n:<4} min={min(xs):>6.1f} p50={p50:>6.1f} "
            f"p95={p95:>7.1f} p99={p99:>7.1f} max={max(xs):>7.1f} ms"
        )

    print()
    print("レイテンシ統計:")
    stats("fs.ready", fs_ready)
    stats("fs.paint", fs_paint)
    stats("thumb.first_ready", thumb_first)
    stats("ai.job_ready", ai_ready)


# -----------------------------------------------------------------------
# priority
# -----------------------------------------------------------------------

def cmd_priority(events: list[dict]) -> None:
    """優先度違反: 可視 priority=True な thumb が未 decode のうちに、
    非 priority な thumb が先にデコード完了した件数を検出する。

    手順:
      1. thumb.enqueue イベントから idx → priority の最新状態を追跡
      2. thumb.decode_begin を時系列に並べ、同じ seq 範囲内で
         priority=False が priority=True より先に begin したケースを数える
    """
    enqueue_priority: dict[int, bool] = {}  # idx → 最新の priority
    violations: list[dict] = []

    # idx → 最後の enqueue 時刻と priority
    last_enqueue: dict[int, tuple[float, bool]] = {}
    # 現在 priority=True なキューに積まれていて未 decode_begin な idx のセット
    pending_hi: set[int] = set()

    for e in events:
        if e.get("cat") != "thumb":
            continue
        kind = e.get("kind")
        idx = e.get("idx")
        t = e.get("t", 0.0)

        if kind == "enqueue":
            pri = bool(e.get("priority", False))
            enqueue_priority[idx] = pri
            last_enqueue[idx] = (t, pri)
            if pri:
                pending_hi.add(idx)
            else:
                pending_hi.discard(idx)

        elif kind == "decode_begin":
            cur_pri = enqueue_priority.get(idx, False)
            if not cur_pri and pending_hi:
                # 高優先度が残っているのに低優先度が先に入った → 違反
                violations.append({
                    "t": t,
                    "lo_idx": idx,
                    "pending_hi": sorted(pending_hi),
                })
            pending_hi.discard(idx)  # これ以降はこの idx は処理中

        elif kind in ("decode_end", "skip"):
            pending_hi.discard(idx)

    print(f"検出された優先度違反: {len(violations)} 件")
    print()
    for v in violations[:30]:
        print(
            f"  t={v['t']:>8.3f}s  lo_idx={v['lo_idx']:>4}  "
            f"pending_hi={v['pending_hi'][:8]}{'...' if len(v['pending_hi']) > 8 else ''}"
        )
    if len(violations) > 30:
        print(f"  ... 他 {len(violations) - 30} 件")


# -----------------------------------------------------------------------
# dump <seq>
# -----------------------------------------------------------------------

def cmd_dump(events: list[dict], seq: int, include_frames: bool) -> None:
    hit = [e for e in events if e.get("seq", 0) == seq]
    if not hit:
        print(f"(seq={seq} に紐づくイベントなし)")
        return
    # 入力→描画の可読性のため frame.begin は既定で除外 (--with-frames で表示)
    filtered = [
        e for e in hit
        if include_frames or e.get("cat") != "frame"
    ]
    t0 = hit[0].get("t", 0.0)
    suppressed = len(hit) - len(filtered)
    print(f"seq={seq} イベント {len(hit)} 件  (frame.begin {suppressed} 件を非表示)")
    for e in filtered:
        dt = (e.get("t", 0.0) - t0) * 1000
        extras = {
            k: v
            for k, v in e.items()
            if k not in {"t", "tid", "cat", "kind", "key", "seq"}
        }
        extras_str = " ".join(f"{k}={v}" for k, v in extras.items())
        print(
            f"  +{dt:>7.1f}ms  [t{e.get('tid', '?'):>2}] "
            f"{e.get('cat', '?'):<6}.{e.get('kind', '?'):<14} "
            f"{fmt_key(e.get('key'), 50):<52} {extras_str}"
        )


# -----------------------------------------------------------------------
# thumbs
# -----------------------------------------------------------------------

def cmd_thumbs(events: list[dict]) -> None:
    """thumb.decode_end の時間分布を priority=H/L、from_cache=True/False 別に表示。"""
    # idx → 最後の priority (enqueue から取る)
    idx_priority: dict[int, bool] = {}
    buckets: dict[tuple[str, bool], list[float]] = defaultdict(list)

    for e in events:
        if e.get("cat") != "thumb":
            continue
        kind = e.get("kind")
        if kind == "enqueue":
            idx_priority[e.get("idx")] = bool(e.get("priority", False))
        elif kind == "decode_end":
            idx = e.get("idx")
            pri = idx_priority.get(idx, False)
            from_cache = bool(e.get("from_cache", False))
            ms = e.get("ms", 0.0)
            key = ("H" if pri else "L", from_cache)
            buckets[key].append(ms)

    def stats(label: str, xs: list[float]) -> None:
        if not xs:
            print(f"  {label:<28} n=0")
            return
        xs_sorted = sorted(xs)
        n = len(xs)
        p50 = xs_sorted[n // 2]
        p95 = xs_sorted[min(n - 1, int(n * 0.95))]
        print(
            f"  {label:<28} n={n:<5} "
            f"min={min(xs):>6.1f} p50={p50:>6.1f} p95={p95:>7.1f} "
            f"max={max(xs):>7.1f} ms"
        )

    print("サムネイル decode 時間分布 (priority / キャッシュ別):")
    for pri in ("H", "L"):
        for cached in (True, False):
            label = f"priority={pri}  from_cache={cached}"
            stats(label, buckets.get((pri, cached), []))


# -----------------------------------------------------------------------
# timeline
# -----------------------------------------------------------------------

def cmd_timeline(events: list[dict], only_seq: int | None) -> None:
    try:
        import matplotlib.pyplot as plt
    except ImportError:
        print("matplotlib が未インストールです: pip install matplotlib", file=sys.stderr)
        sys.exit(1)

    # スレッド × 時刻で色分けして散布 + カテゴリ別に色
    # (ガントではなく散布 — スパン構造を正式に作ってないので)
    if only_seq is not None:
        events = [e for e in events if e.get("seq", 0) == only_seq]
        if not events:
            print(f"(seq={only_seq} のイベントなし)", file=sys.stderr)
            sys.exit(1)

    cats = sorted({e.get("cat", "?") for e in events})
    cat_color = {c: f"C{i}" for i, c in enumerate(cats)}

    fig, ax = plt.subplots(figsize=(14, 6))
    tids = sorted({e.get("tid", 0) for e in events})
    tid_y = {t: i for i, t in enumerate(tids)}

    for e in events:
        t = e.get("t", 0.0)
        y = tid_y[e.get("tid", 0)]
        cat = e.get("cat", "?")
        ax.plot(t, y, ".", color=cat_color[cat], markersize=4)

    ax.set_yticks(list(tid_y.values()))
    ax.set_yticklabels([f"t{t}" for t in tids])
    ax.set_xlabel("time (s)")
    ax.set_title(
        f"perf_events timeline"
        + (f" (seq={only_seq})" if only_seq is not None else "")
    )
    legend_handles = [
        plt.Line2D([0], [0], marker="o", linestyle="", color=cat_color[c], label=c)
        for c in cats
    ]
    ax.legend(handles=legend_handles, loc="upper right")
    ax.grid(True, axis="x", alpha=0.3)
    plt.tight_layout()
    plt.show()


# -----------------------------------------------------------------------
# main
# -----------------------------------------------------------------------

def main() -> None:
    parser = argparse.ArgumentParser(
        description="mimageviewer perf_events.jsonl analyzer"
    )
    parser.add_argument("jsonl", type=Path, help="perf_events.jsonl のパス")
    subs = parser.add_subparsers(dest="cmd", required=True)
    subs.add_parser("summary")
    subs.add_parser("latency")
    subs.add_parser("priority")
    subs.add_parser("thumbs")
    p_dump = subs.add_parser("dump")
    p_dump.add_argument("seq", type=int)
    p_dump.add_argument("--with-frames", action="store_true", help="frame.begin も表示する")
    p_tl = subs.add_parser("timeline")
    p_tl.add_argument("seq", type=int, nargs="?", default=None)

    args = parser.parse_args()

    if not args.jsonl.is_file():
        print(f"ファイルが見つかりません: {args.jsonl}", file=sys.stderr)
        sys.exit(1)

    events = load_events(args.jsonl)

    if args.cmd == "summary":
        cmd_summary(events)
    elif args.cmd == "latency":
        cmd_latency(events)
    elif args.cmd == "priority":
        cmd_priority(events)
    elif args.cmd == "thumbs":
        cmd_thumbs(events)
    elif args.cmd == "dump":
        cmd_dump(events, args.seq, args.with_frames)
    elif args.cmd == "timeline":
        cmd_timeline(events, args.seq)


if __name__ == "__main__":
    main()
