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
    nav                 Ctrl+↑↓ ナビの区間別 wall time (DFS / apply / load_folder /
                        start_loading_items / close_fullscreen) を集計
    hitches [--ms N]    フレーム間隔 N ms 超のヒッチを検出し、直前の nav.* 区間を
                        表示 (デフォルト 33ms = 30fps 閾値)
    idle-health         静止区間の update 頻度、repaint 理由の継続、同一 work の
                        反復を検査し、閾値超過時に終了コード 1 を返す
    startup             起動時間のフェーズ別 breakdown (data_dir / models /
                        susie_worker / settings / icon / fonts / theme / app_default /
                        creator_enter/exit / first_frame) を表示
    av_drift [--plot]   動画再生中の音声・映像同期 (A/V drift) と audio underrun /
                        audio_pts_jump / Norm 操作 を時系列で集計する。
                        --plot で matplotlib グラフを開く

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
                event = json.loads(line)
                event["_line"] = lineno
                events.append(event)
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
# nav
# -----------------------------------------------------------------------

def cmd_nav(events: list[dict]) -> None:
    """Ctrl+↑↓ ナビの区間別 wall time 統計。

    対象イベント (src/app.rs で emit):
      nav.dfs_begin / dfs_end             — DFS スレッド (UI ブロックせず)
      nav.apply_begin / apply_end         — DFS 結果の UI 適用区間
      nav.load_folder_begin / _end        — load_folder 全体 (UI ブロック)
      nav.lf_{scan,sort,dup_filter,auto_index}  — load_folder 内の小区間
      nav.sli_begin / sli_end             — start_loading_items 全体
      nav.sli_{sidecar_flush,prewarm_rating,sidecar_import,adjustment_db,
               mask_db,catalog_open,catalog_load_all,catalog_delete_missing,
               spawn_workers,settings_save}  — 内訳
      nav.close_fullscreen_begin/_end     — close_fullscreen 区間
    """
    buckets: dict[str, list[float]] = defaultdict(list)

    for e in events:
        if e.get("cat") != "nav":
            continue
        kind = e.get("kind", "")
        ms = e.get("ms")
        if ms is None:
            continue
        # "dfs_end" -> "dfs" のように集約
        if kind.endswith("_end"):
            label = kind[: -len("_end")]
        else:
            label = kind
        buckets[label].append(float(ms))

    def stats(label: str, xs: list[float]) -> None:
        if not xs:
            return
        xs_sorted = sorted(xs)
        n = len(xs)
        p50 = xs_sorted[n // 2]
        p95 = xs_sorted[min(n - 1, int(n * 0.95))]
        p99 = xs_sorted[min(n - 1, int(n * 0.99))]
        print(
            f"  {label:<32} n={n:<4} min={min(xs):>6.1f} p50={p50:>6.1f} "
            f"p95={p95:>7.1f} p99={p99:>7.1f} max={max(xs):>7.1f} ms"
        )

    # 親区間 → 子区間の階層で表示する
    groups: list[tuple[str, list[str]]] = [
        ("DFS (別スレッド)", ["dfs"]),
        ("apply_folder_nav_result (UI)", ["apply"]),
        ("load_folder 全体 (UI)", ["load_folder"]),
        ("  load_folder 内訳", ["lf_scan", "lf_sort", "lf_dup_filter", "lf_auto_index"]),
        ("close_fullscreen (UI)", ["close_fullscreen"]),
        ("start_loading_items 全体 (UI)", ["sli"]),
        ("  sli 内訳", [
            "sli_sidecar_flush", "sli_prewarm_rating", "sli_sidecar_import",
            "sli_adjustment_db", "sli_mask_db",
            "sli_catalog_open", "sli_catalog_load_all", "sli_catalog_delete_missing",
            "sli_spawn_workers", "sli_settings_save",
        ]),
    ]

    print("Ctrl+↑↓ ナビ区間の wall time:")
    for title, labels in groups:
        print(f"\n[{title}]")
        found = False
        for lab in labels:
            if buckets.get(lab):
                stats(lab, buckets[lab])
                found = True
        if not found:
            print("  (イベントなし)")

    # DFS 由来の input_seq ごとの End-to-End レイテンシ
    # input.grid_ctrl_nav / fs_ctrl_nav から nav.apply_end までの経過
    print("\n[End-to-End: input → apply_end (同一 seq)]")
    input_t: dict[int, tuple[float, str]] = {}
    apply_end_t: dict[int, float] = {}
    for e in events:
        seq = e.get("seq", 0)
        if not seq:
            continue
        cat = e.get("cat", "")
        kind = e.get("kind", "")
        if cat == "input" and kind in ("grid_ctrl_nav", "fs_ctrl_nav"):
            input_t.setdefault(seq, (e.get("t", 0.0), kind))
        elif cat == "nav" and kind == "apply_end":
            # 同一 seq で apply が複数回走る (連鎖 DFS) 場合は最後を採用
            apply_end_t[seq] = e.get("t", 0.0)

    e2e: list[float] = []
    for seq, (t0, _) in input_t.items():
        if seq in apply_end_t:
            e2e.append((apply_end_t[seq] - t0) * 1000.0)
    if e2e:
        stats("input → apply_end (ms)", e2e)
    else:
        print("  (相関できる seq が見つからなかった — input.grid_ctrl_nav / fs_ctrl_nav と nav.apply_end の対が必要)")


# -----------------------------------------------------------------------
# startup — 起動時間のフェーズ別 breakdown
# -----------------------------------------------------------------------

def cmd_startup(events: list[dict]) -> None:
    """main() 入口から first_frame までの各フェーズ経過時間を表示する。

    emit 側 (main.rs / app.rs) から打たれるイベント:
      - data_dir_init / models_extract / susie_worker_extract /
        settings_load / load_icon / before_run_native / creator_enter /
        setup_fonts / apply_theme / app_default / creator_exit / first_frame

    `ms` はそのフェーズ単体の所要時間、`total_ms` は main() 入口からの累計。
    """
    startup_events = [e for e in events if e.get("cat") == "startup"]
    if not startup_events:
        print("(startup イベントなし — --perf-log 有効で再測定してください)")
        return

    # 出力順は emit 順を尊重する (t 昇順 = main() の実行順)
    startup_events.sort(key=lambda e: e.get("t", 0.0))

    print("起動時間フェーズ別 breakdown:")
    print(f"{'step':<26} {'phase ms':>10} {'total ms':>10}")
    print("-" * 50)
    prev_total = 0.0
    for e in startup_events:
        step = e.get("kind", "?")
        ms = e.get("ms")
        total = e.get("total_ms")
        # total_ms だけで ms がないマーカーは、差分を計算して表示する
        if ms is None and total is not None:
            ms_str = f"{total - prev_total:>10.1f}"
        elif ms is not None:
            ms_str = f"{float(ms):>10.1f}"
        else:
            ms_str = f"{'-':>10}"
        total_str = f"{float(total):>10.1f}" if total is not None else f"{'-':>10}"
        print(f"{step:<26} {ms_str} {total_str}")
        if total is not None:
            prev_total = float(total)

    # 最後の first_frame の total_ms を起動時間として明示
    final = next(
        (e for e in reversed(startup_events) if e.get("kind") == "first_frame"),
        None,
    )
    if final and final.get("total_ms") is not None:
        print()
        print(f"=> 起動から初回フレームまで: {float(final['total_ms']):.0f} ms")


# -----------------------------------------------------------------------
# idle-health — 静止状態での高速 repaint / work 再投入ループ検出
# -----------------------------------------------------------------------

IDLE_HEALTH_WORK_KINDS = {
    "enqueue",
    "idle_upgrade_enqueue",
    "idle_upgrade_ineligible",
}

# A 500 ms split misses a 1.5 Hz repaint loop even though it wakes the application forever.
# Static release scenarios have no legitimate periodic work, so keep the same-reason run
# connected across gaps up to one second.
IDLE_HEALTH_REASON_STREAK_GAP_SECS = 1.0


def analyze_idle_health(
    events: list[dict],
    start_t: float,
    end_t: float,
    *,
    target_update_rate: float = 2.0,
    max_update_rate: float = 10.0,
    max_reason_streak_secs: float = 2.0,
    max_same_work: int = 3,
    max_input_events: int = 0,
    expected_pid: int | None = None,
    allow_sleeping_window: bool = False,
    evidence_start_t: float | None = None,
    require_idle_upgrade_ineligible: bool = False,
) -> dict:
    """静止測定区間を解析し、JSON 化可能な report を返す。

    App が正しく sleep すると区間内イベントが 0 件になる。通常は窓ずれと区別するため
    FAIL にし、外部 sampler が同一 process/session を検証した場合だけ
    ``allow_sleeping_window`` で明示的に許可する。
    """
    duration = end_t - start_t
    if duration <= 0.0:
        raise ValueError("end_t は start_t より大きくしてください")

    selected = [
        e
        for e in events
        if start_t <= float(e.get("t", 0.0)) <= end_t
    ]
    selected.sort(key=lambda e: float(e.get("t", 0.0)))

    frame_events = [
        e
        for e in selected
        if e.get("cat") == "frame" and e.get("kind") == "begin"
    ]
    tail_events = [
        e
        for e in selected
        if e.get("cat") == "ui" and e.get("kind") == "tail_repaint"
    ]
    input_events = [e for e in selected if e.get("cat") == "input"]

    action_counts: dict[str, int] = defaultdict(int)
    reason_counts: dict[str, int] = defaultdict(int)
    cause_counts: dict[str, int] = defaultdict(int)
    for event in tail_events:
        action_counts[str(event.get("action", "?"))] += 1
        for reason in event.get("reasons", []) or []:
            reason_counts[str(reason)] += 1
        for cause in event.get("prev_frame_causes", []) or []:
            cause_counts[str(cause)] += 1

    # 同じ理由が短い frame gap で連続する時間を測る。1 秒を超えて次の frame が来た
    # 場合は、継続 repaint ではなく sleep 後の別 run とみなす。
    active_reasons: dict[str, tuple[float, float]] = {}
    max_reason_streaks: dict[str, float] = defaultdict(float)
    for event in tail_events:
        t = float(event.get("t", 0.0))
        present = {str(reason) for reason in (event.get("reasons", []) or [])}
        for reason in list(active_reasons):
            run_start, last_t = active_reasons[reason]
            if reason not in present or t - last_t > IDLE_HEALTH_REASON_STREAK_GAP_SECS:
                max_reason_streaks[reason] = max(
                    max_reason_streaks[reason],
                    last_t - run_start,
                )
                del active_reasons[reason]
        for reason in present:
            if reason in active_reasons:
                run_start, _ = active_reasons[reason]
                active_reasons[reason] = (run_start, t)
            else:
                active_reasons[reason] = (t, t)
    for reason, (run_start, last_t) in active_reasons.items():
        max_reason_streaks[reason] = max(
            max_reason_streaks[reason],
            last_t - run_start,
        )

    work_counts: dict[tuple[str, str, str, str], int] = defaultdict(int)
    for event in selected:
        if event.get("cat") != "thumb" or event.get("kind") not in IDLE_HEALTH_WORK_KINDS:
            continue
        kind = str(event.get("kind"))
        idx = str(event.get("idx", "-"))
        key = str(event.get("key") or f"idx:{idx}")
        generation = str(event.get("items_gen", event.get("seq", 0)))
        work_counts[(kind, key, idx, generation)] += 1

    repeated_work = [
        {
            "kind": identity[0],
            "key": identity[1],
            "idx": identity[2],
            "generation": identity[3],
            "count": count,
        }
        for identity, count in sorted(
            work_counts.items(),
            key=lambda pair: (-pair[1], pair[0]),
        )
        if count > 1
    ]
    max_work_count = max(work_counts.values(), default=0)
    update_rate = len(frame_events) / duration

    failures: list[str] = []
    warnings: list[str] = []
    session_events = [
        e
        for e in events
        if e.get("cat") == "session" and e.get("kind") == "start"
    ]
    matching_session_events = [
        e
        for e in session_events
        if expected_pid is not None and int(e.get("pid", -1)) == expected_pid
    ]
    if expected_pid is not None and not matching_session_events:
        observed_pids = sorted(
            {int(e.get("pid", -1)) for e in session_events if "pid" in e}
        )
        failures.append(
            f"perf log の session PID が測定対象 PID {expected_pid} と一致しません "
            f"(observed={observed_pids})"
        )
    if not selected:
        if not allow_sleeping_window:
            failures.append(
                "測定区間に perf event が無く、窓ずれと sleep を区別できません"
            )
        elif expected_pid is None:
            failures.append(
                "空の測定区間を許可するには同一 session の expected PID が必要です"
            )
        else:
            warnings.append(
                "測定区間は完全に sleep しており perf event は 0 件です "
                "(同一 session PID は確認済み)"
            )
    if require_idle_upgrade_ineligible:
        evidence_start = start_t if evidence_start_t is None else evidence_start_t
        evidence = [
            e
            for e in events
            if evidence_start <= float(e.get("t", 0.0)) <= end_t
            and e.get("cat") == "thumb"
            and e.get("kind") == "idle_upgrade_ineligible"
        ]
        if not evidence:
            failures.append(
                "動画ピン由来の thumb.idle_upgrade_ineligible が準備・測定区間に無く、"
                "シナリオ成立を確認できません"
            )
    if not any(e.get("cat") == "frame" and e.get("kind") == "begin" for e in events):
        failures.append("perf log に frame.begin が無く、測定が有効か確認できません")
    if not any(e.get("cat") == "ui" and e.get("kind") == "tail_repaint" for e in events):
        failures.append("perf log に ui.tail_repaint が無く、対応ビルドか確認できません")
    if len(input_events) > max_input_events:
        failures.append(
            f"測定区間に input event が {len(input_events)} 件あります "
            f"(上限 {max_input_events})"
        )
    if update_rate > max_update_rate:
        failures.append(
            f"静止中の update rate が {update_rate:.2f}/s です "
            f"(上限 {max_update_rate:.2f}/s)"
        )
    elif update_rate > target_update_rate:
        warnings.append(
            f"静止中の update rate が目標 {target_update_rate:.2f}/s を超えています: "
            f"{update_rate:.2f}/s"
        )
    for reason, streak in sorted(max_reason_streaks.items()):
        if streak > max_reason_streak_secs:
            failures.append(
                f"repaint reason `{reason}` が {streak:.2f}s 継続しました "
                f"(上限 {max_reason_streak_secs:.2f}s)"
            )
    if max_work_count > max_same_work:
        failures.append(
            f"同一 thumbnail work が最大 {max_work_count} 回発生しました "
            f"(上限 {max_same_work} 回)"
        )

    top_causes = [
        {"cause": cause, "count": count}
        for cause, count in sorted(
            cause_counts.items(),
            key=lambda pair: (-pair[1], pair[0]),
        )[:10]
    ]
    return {
        "status": "fail" if failures else "pass",
        "window": {
            "start_t": start_t,
            "end_t": end_t,
            "duration_secs": duration,
        },
        "metrics": {
            "events": len(selected),
            "frames": len(frame_events),
            "update_rate_per_sec": update_rate,
            "tail_repaint_events": len(tail_events),
            "input_events": len(input_events),
            "max_same_work": max_work_count,
            "matching_session_events": len(matching_session_events),
        },
        "thresholds": {
            "target_update_rate_per_sec": target_update_rate,
            "max_update_rate_per_sec": max_update_rate,
            "max_reason_streak_secs": max_reason_streak_secs,
            "max_same_work": max_same_work,
            "max_input_events": max_input_events,
            "expected_pid": expected_pid,
            "allow_sleeping_window": allow_sleeping_window,
            "evidence_start_t": evidence_start_t,
            "require_idle_upgrade_ineligible": require_idle_upgrade_ineligible,
        },
        "action_counts": dict(sorted(action_counts.items())),
        "reason_counts": dict(sorted(reason_counts.items())),
        "max_reason_streaks_secs": dict(sorted(max_reason_streaks.items())),
        "repeated_work": repeated_work[:20],
        "top_repaint_causes": top_causes,
        "warnings": warnings,
        "failures": failures,
    }


def cmd_idle_health(
    events: list[dict],
    start_t: float | None,
    end_t: float | None,
    window_secs: float,
    target_update_rate: float,
    max_update_rate: float,
    max_reason_streak_secs: float,
    max_same_work: int,
    max_input_events: int,
    json_out: Path | None,
    expected_pid: int | None = None,
    allow_sleeping_window: bool = False,
    evidence_start_t: float | None = None,
    require_idle_upgrade_ineligible: bool = False,
) -> int:
    if not events:
        print("ERROR: perf event が 0 件です", file=sys.stderr)
        return 2
    if end_t is None:
        end_t = max(float(e.get("t", 0.0)) for e in events)
    if start_t is None:
        start_t = max(0.0, end_t - window_secs)
    try:
        report = analyze_idle_health(
            events,
            start_t,
            end_t,
            target_update_rate=target_update_rate,
            max_update_rate=max_update_rate,
            max_reason_streak_secs=max_reason_streak_secs,
            max_same_work=max_same_work,
            max_input_events=max_input_events,
            expected_pid=expected_pid,
            allow_sleeping_window=allow_sleeping_window,
            evidence_start_t=evidence_start_t,
            require_idle_upgrade_ineligible=require_idle_upgrade_ineligible,
        )
    except ValueError as error:
        print(f"ERROR: {error}", file=sys.stderr)
        return 2

    metrics = report["metrics"]
    window = report["window"]
    print("=== idle health ===")
    print(
        f"測定区間: t={window['start_t']:.3f}..{window['end_t']:.3f} "
        f"({window['duration_secs']:.2f}s)"
    )
    print(
        f"events={metrics['events']} frames={metrics['frames']} "
        f"update_rate={metrics['update_rate_per_sec']:.2f}/s "
        f"tail_repaint={metrics['tail_repaint_events']} "
        f"input={metrics['input_events']} max_same_work={metrics['max_same_work']}"
    )
    if report["action_counts"]:
        print("tail actions:")
        for action, count in report["action_counts"].items():
            print(f"  {action}: {count}")
    if report["reason_counts"]:
        print("repaint reasons (count / max streak):")
        for reason, count in report["reason_counts"].items():
            streak = report["max_reason_streaks_secs"].get(reason, 0.0)
            print(f"  {reason}: {count} / {streak:.2f}s")
    if report["repeated_work"]:
        print("repeated thumbnail work (top 20):")
        for work in report["repeated_work"]:
            print(
                f"  {work['kind']} count={work['count']} idx={work['idx']} "
                f"gen={work['generation']} key={fmt_key(work['key'])}"
            )
    for warning in report["warnings"]:
        print(f"WARNING: {warning}")
    for failure in report["failures"]:
        print(f"FAIL: {failure}")
    print(f"判定: {str(report['status']).upper()}")

    if json_out is not None:
        json_out.parent.mkdir(parents=True, exist_ok=True)
        json_out.write_text(
            json.dumps(report, ensure_ascii=False, indent=2) + "\n",
            encoding="utf-8",
        )
        print(f"JSON report: {json_out}")
    return 1 if report["failures"] else 0


# -----------------------------------------------------------------------
# hitches — フレーム間隔の分布と nav 区間との重なり
# -----------------------------------------------------------------------

def cmd_hitches(events: list[dict], threshold_ms: float) -> None:
    """frame.begin の間隔が threshold_ms を超えたヒッチを検出し、
    その直前 500ms に発生した nav.* 区間を表示する。"""
    frame_ts: list[float] = []
    for e in events:
        if e.get("cat") == "frame" and e.get("kind") == "begin":
            frame_ts.append(e.get("t", 0.0))

    if len(frame_ts) < 2:
        print("(frame.begin が 2 件未満)")
        return

    gaps: list[tuple[float, float]] = []  # (t_end, gap_ms)
    for i in range(1, len(frame_ts)):
        gap = (frame_ts[i] - frame_ts[i - 1]) * 1000.0
        if gap >= threshold_ms:
            gaps.append((frame_ts[i], gap))

    print(f"フレーム数: {len(frame_ts)}  間隔 >= {threshold_ms}ms のヒッチ: {len(gaps)} 件")
    if not gaps:
        return

    gaps_sorted = sorted(g for _, g in gaps)
    n = len(gaps_sorted)
    p50 = gaps_sorted[n // 2]
    p95 = gaps_sorted[min(n - 1, int(n * 0.95))]
    print(
        f"ヒッチ間隔: min={min(gaps_sorted):.1f} p50={p50:.1f} "
        f"p95={p95:.1f} max={max(gaps_sorted):.1f} ms"
    )

    # nav 区間 (end イベント) を直前 500ms 以内に含むものを列挙
    nav_ends = [
        e for e in events
        if e.get("cat") == "nav" and e.get("kind", "").endswith("_end")
    ]

    print("\n最も大きいヒッチ 10 件:")
    for t_end, gap in sorted(gaps, key=lambda x: -x[1])[:10]:
        t_start = t_end - gap / 1000.0
        # 直前 500ms ウィンドウ
        window_start = t_start - 0.5
        nearby = [
            e for e in nav_ends
            if window_start <= e.get("t", 0.0) <= t_end
        ]
        # ms 上位 3 件だけ出す
        nearby.sort(key=lambda e: -float(e.get("ms", 0.0)))
        tags = [
            f"{e.get('kind')}={float(e.get('ms', 0.0)):.1f}ms"
            for e in nearby[:3]
        ]
        tags_str = ", ".join(tags) if tags else "(nav イベントなし)"
        print(
            f"  t={t_end:>8.3f}s  gap={gap:>6.1f}ms  "
            f"直前 nav: {tags_str}"
        )


# -----------------------------------------------------------------------
# spike-context -- native presenter spike の前後イベントを見る
# -----------------------------------------------------------------------

def _percentile(values: list[float], pct: float) -> float:
    """単純な百分位 (リスト未ソート可、pct=50 なら中央値)。"""
    if not values:
        return 0.0
    s = sorted(values)
    if len(s) == 1:
        return s[0]
    rank = (pct / 100.0) * (len(s) - 1)
    lo = int(rank)
    hi = min(lo + 1, len(s) - 1)
    frac = rank - lo
    return s[lo] * (1.0 - frac) + s[hi] * frac


def cmd_av_drift(events: list[dict], plot: bool) -> None:
    """A/V sync drift 解析。

    対象イベント:
      video.av_drift                  — drift サンプル (1Hz + edge)
      video.norm_apply_begin/_end     — Norm 操作の前後 snapshot
      audio_out.snapshot              — pump 1Hz: underrun_active / silence_ms_last_sec
      audio_out.underrun_begin/_end   — silence 区間の begin/end edge
      audio_out.audio_pts_jump        — 大ジャンプ (>5ms or cap 乖離)
      audio_out.buffer_clear          — clear_audio_output_buffer (Norm/seek/etc)

    判定基準は **時系列対応**を主とする:
      norm_apply_begin → audio_out.buffer_clear → underrun_begin → underrun_end →
      audio_out.audio_pts_jump がこの順で並ぶか確認する。
    """
    # drift sample: (t, drift_ms, av_offset_ms_or_None, audio_lead_ms, big_edge)
    drifts: list[tuple[float, float, float | None, float, bool]] = []
    snapshots: list[tuple[float, bool, float, float]] = []  # (t, underrun, silence_ms, processed)
    underrun_begins: list[float] = []
    underrun_ends: list[float] = []
    pts_jumps: list[tuple[float, float, float]] = []  # (t, requested_delta_ms, applied_delta_ms)
    buffer_clears: list[tuple[float, float, float, float]] = []  # (t, processed, raw, tx_queued)
    norm_begins: list[tuple[float, str, float]] = []  # (t, reason, gain_db)
    norm_ends: list[tuple[float, float]] = []  # (t, now)

    # ログ schema 判定 (Codex P3 ① 反映): `av_offset_ms` キーが av_drift event に
    # 一度も現れなければ legacy log (新メトリクス導入前のビルドで取得)。
    # 新ログでは audio inactive 区間の av_offset_ms は明示的に `null` として出るので、
    # キー存在の有無で legacy / 新を区別できる。
    has_av_offset_field = False
    has_audio_lead_field = False

    for e in events:
        t = e.get("t", 0.0)
        cat = e.get("cat", "")
        kind = e.get("kind", "")
        if cat == "video" and kind == "av_drift":
            if "av_offset_ms" in e:
                has_av_offset_field = True
            if "audio_lead_ms" in e:
                has_audio_lead_field = True
            offset_raw = e.get("av_offset_ms")
            offset = (
                float(offset_raw)
                if isinstance(offset_raw, (int, float))
                else None
            )
            drifts.append(
                (
                    t,
                    float(e.get("drift_ms", 0.0)),
                    offset,
                    float(e.get("audio_lead_ms", 0.0)),
                    bool(e.get("big_edge", False)),
                )
            )
        elif cat == "video" and kind == "norm_apply_begin":
            norm_begins.append(
                (t, str(e.get("reason", "?")), float(e.get("gain_db", 0.0)))
            )
        elif cat == "video" and kind == "norm_apply_end":
            norm_ends.append((t, float(e.get("now", 0.0))))
        elif cat == "audio_out" and kind == "snapshot":
            snapshots.append(
                (
                    t,
                    bool(e.get("underrun_active", False)),
                    float(e.get("silence_ms_last_sec", 0.0)),
                    float(e.get("processed_secs", 0.0)),
                )
            )
        elif cat == "audio_out" and kind == "underrun_begin":
            underrun_begins.append(t)
        elif cat == "audio_out" and kind == "underrun_end":
            underrun_ends.append(t)
        elif cat == "audio_out" and kind == "audio_pts_jump":
            pts_jumps.append(
                (
                    t,
                    float(e.get("requested_delta_ms", 0.0)),
                    float(e.get("applied_delta_ms", 0.0)),
                )
            )
        elif cat == "audio_out" and kind == "buffer_clear":
            buffer_clears.append(
                (
                    t,
                    float(e.get("processed_secs_before", 0.0)),
                    float(e.get("raw_pending_secs_before", 0.0)),
                    float(e.get("audio_tx_queued_before", 0.0)),
                )
            )

    # ── テキスト統計 ──
    legacy_log = (not has_av_offset_field) and (not has_audio_lead_field) and bool(drifts)
    if drifts:
        drift_values = [d[1] for d in drifts]
        offset_values = [d[2] for d in drifts if d[2] is not None]
        lead_values = [d[3] for d in drifts]

        print("=== A/V 同期 統計 ===")
        print(f"  サンプル数: {len(drifts)}")
        print(f"  期間:       t={drifts[0][0]:.2f}s 〜 t={drifts[-1][0]:.2f}s")
        if legacy_log:
            print(
                "  schema:     LEGACY (av_offset_ms / audio_lead_ms 未記録のビルドで取得)"
            )
            print(
                "              -> A/V offset / audio lead は表示できない。代わりに"
                " audio_pts_jump の `requested_delta_ms` を見ること"
            )
        print()

        # PRIMARY: av_offset_ms = video_pts − audio_audible_pts (= 体感の音映像差)
        # 注: legacy log では offset_values が空、audio_lead_ms が全て 0 になる。
        if legacy_log:
            print("  >>A/V offset / audio lead: 旧 schema のため未記録")
        elif offset_values:
            abs_off = [abs(v) for v in offset_values]
            print("  >>A/V offset (= ユーザー体感の音映像差、video_pts − audio_audible_pts)")
            print(
                f"    範囲:  min={min(offset_values):+.1f}ms  max={max(offset_values):+.1f}ms"
                f"  mean={sum(offset_values)/len(offset_values):+.1f}ms"
            )
            print(
                f"    |off|: p50={_percentile(abs_off, 50):.1f}"
                f"  p95={_percentile(abs_off, 95):.1f}"
                f"  p99={_percentile(abs_off, 99):.1f}"
                f"  max={max(abs_off):.1f}ms"
            )
            # 1 秒超のズレが続いている期間を検出 (= バグ濃厚)
            long_desync = [(t, off) for t, _, off, _, _ in drifts if off is not None and abs(off) > 1000.0]
            if long_desync:
                print(
                    f"    ! |offset| > 1000ms が {len(long_desync)} サンプル"
                    f" (= 体感で明確にズレているはず)"
                )
                for t, off in long_desync[:5]:
                    print(f"      t={t:>8.2f}s  offset={off:+8.1f}ms")
                if len(long_desync) > 5:
                    print(f"      ... (+{len(long_desync) - 5} 件省略)")
        else:
            print("  >>A/V offset: (audio inactive 区間のみ - 動画 only か音声起動失敗)")
        print()

        # SECONDARY: audio_lead_ms = audio_audible_pts − master_clock (= clock 乖離)
        if not legacy_log:
            abs_lead = [abs(v) for v in lead_values]
            print("  >>audio lead (= audio が master clock から先行している量、post-apply)")
            print(
                f"    範囲:  min={min(lead_values):+.1f}ms  max={max(lead_values):+.1f}ms"
                f"  mean={sum(lead_values)/len(lead_values):+.1f}ms"
            )
            print(
                f"    |lead|: p50={_percentile(abs_lead, 50):.1f}"
                f"  p95={_percentile(abs_lead, 95):.1f}"
                f"  p99={_percentile(abs_lead, 99):.1f}"
                f"  max={max(abs_lead):.1f}ms"
            )
            big_lead = [(t, l) for t, _, _, l, _ in drifts if abs(l) > 100.0]
            if big_lead:
                print(
                    f"    ! |lead| > 100ms が {len(big_lead)} サンプル"
                    f" (= clock が audio に追従できていない可能性)"
                )
            print()

        # TERTIARY: video pacing health (av_drift_ms = video_pts − master_clock)
        abs_drift = [abs(v) for v in drift_values]
        print("  >>video pacing (= video_pts − master_clock、video が clock に追従しているか)")
        print(
            f"    |drift|: p50={_percentile(abs_drift, 50):.2f}"
            f"  p95={_percentile(abs_drift, 95):.2f}"
            f"  p99={_percentile(abs_drift, 99):.2f}"
            f"  max={max(abs_drift):.2f}ms"
        )
        big_edges = [d for d in drifts if d[4]]
        if big_edges:
            print(f"    big_edge: {len(big_edges)} 件 (= 体感 |offset|>30ms にしきい値跨ぎ)")
            for t, drift, off, lead, _ in big_edges[:10]:
                off_str = f"{off:+.1f}" if off is not None else "n/a"
                print(
                    f"      t={t:>8.2f}s  offset={off_str}ms"
                    f"  lead={lead:+.1f}ms  drift={drift:+.2f}ms"
                )
        print()
    else:
        print("(video.av_drift イベントなし — perf-log 取得時に動画再生していなかった可能性)")
        print()

    # ── underrun 区間 ──
    if underrun_begins or underrun_ends:
        print("=== underrun 区間 (audio_out.underrun_begin / underrun_end) ===")
        # begin/end をペアリング (begin の後に最初に来る end を相方とする)
        ends_iter = iter(sorted(underrun_ends))
        next_end: float | None = next(ends_iter, None)
        rows: list[tuple[float, float, float]] = []
        for b in sorted(underrun_begins):
            while next_end is not None and next_end < b:
                next_end = next(ends_iter, None)
            if next_end is None:
                rows.append((b, float("nan"), float("nan")))
            else:
                duration_ms = (next_end - b) * 1000.0
                rows.append((b, next_end, duration_ms))
                next_end = next(ends_iter, None)
        for begin, end, dur in rows[:20]:
            if isinstance(end, float) and end != end:  # NaN check
                print(f"  begin={begin:>8.2f}s  end=(none)")
            else:
                print(f"  begin={begin:>8.2f}s  end={end:>8.2f}s  duration={dur:>6.1f}ms")
        if len(rows) > 20:
            print(f"  ... (+{len(rows) - 20} 件省略)")
        print()
    else:
        print("(audio_out.underrun_begin/end イベントなし — silence 区間検出なし)")
        print()

    # ── audio_pts_jump 一覧 ──
    if pts_jumps:
        print("=== audio_pts_jump (requested_delta vs applied_delta、cap 検出含む) ===")
        for t, req, applied in pts_jumps[:20]:
            cap_diverge = abs(req - applied) > 1.0
            mark = "  [CAP]" if cap_diverge else ""
            print(
                f"  t={t:>8.2f}s  requested={req:+8.2f}ms  applied={applied:+8.2f}ms{mark}"
            )
        if len(pts_jumps) > 20:
            print(f"  ... (+{len(pts_jumps) - 20} 件省略)")
        print()
    else:
        print("(audio_out.audio_pts_jump イベントなし)")
        print()

    # ── buffer_clear 一覧 ──
    if buffer_clears:
        print("=== audio_out.buffer_clear ===")
        for t, processed, raw, tx in buffer_clears[:20]:
            print(
                f"  t={t:>8.2f}s  processed={processed:>5.3f}s  raw_pending={raw:>5.3f}s"
                f"  audio_tx_queued={tx:>5.3f}s"
            )
        if len(buffer_clears) > 20:
            print(f"  ... (+{len(buffer_clears) - 20} 件省略)")
        print()

    # ── Norm 操作一覧 ──
    if norm_begins or norm_ends:
        print("=== Norm 操作 (video.norm_apply_begin / norm_apply_end) ===")
        for t, reason, gain in norm_begins[:30]:
            print(f"  begin t={t:>8.2f}s  reason={reason:>12}  gain_db={gain:+5.2f}")
        if len(norm_begins) > 30:
            print(f"  ... (+{len(norm_begins) - 30} 件省略)")
        print()

    # ── matplotlib プロット (オプション) ──
    if not plot:
        print("(--plot を付けると drift / underrun / norm を時系列グラフで表示)")
        return

    try:
        import matplotlib.pyplot as plt
    except ImportError:
        print("matplotlib が未インストールです: pip install matplotlib", file=sys.stderr)
        return

    if not drifts:
        print("(プロット対象なし)")
        return

    fig, ax = plt.subplots(figsize=(14, 6))
    times = [d[0] for d in drifts]
    drift_ms = [d[1] for d in drifts]
    offset_ms = [d[2] if d[2] is not None else float("nan") for d in drifts]
    lead_ms = [d[3] for d in drifts]
    ax.plot(
        times,
        offset_ms,
        color="tab:cyan",
        linewidth=1.4,
        label="A/V offset = video − audio (ms)",
    )
    ax.plot(
        times,
        lead_ms,
        color="tab:orange",
        linewidth=0.9,
        alpha=0.7,
        label="audio lead = audio − master_clock (ms)",
    )
    ax.plot(
        times,
        drift_ms,
        color="tab:gray",
        linewidth=0.6,
        alpha=0.5,
        label="video pacing = video − master_clock (ms)",
    )
    ax.axhline(0.0, color="gray", linestyle=":", linewidth=0.5)
    ax.set_xlabel("time (s)")
    ax.set_ylabel("ms (signed)")
    ax.set_title("A/V sync over time (offset = perceived A/V mismatch)")

    # underrun 区間を橙色背景で
    ends_sorted = sorted(underrun_ends)
    ends_iter = iter(ends_sorted)
    next_end = next(ends_iter, None)
    for b in sorted(underrun_begins):
        while next_end is not None and next_end < b:
            next_end = next(ends_iter, None)
        end = next_end if next_end is not None else times[-1]
        ax.axvspan(b, end, alpha=0.2, color="orange", label="_underrun")
        if next_end is not None:
            next_end = next(ends_iter, None)

    # norm_apply_begin を縦線で
    for t, reason, _ in norm_begins:
        ax.axvline(t, color="tab:green", linestyle="--", linewidth=0.7, alpha=0.7)
        ax.text(t, ax.get_ylim()[1] * 0.95, f"norm:{reason}", rotation=90, fontsize=7,
                verticalalignment="top")

    # audio_pts_jump をマーカーで
    if pts_jumps:
        ax.scatter(
            [j[0] for j in pts_jumps],
            [0.0] * len(pts_jumps),
            marker="x",
            color="tab:red",
            s=40,
            label="pts_jump",
        )

    ax.grid(True, alpha=0.3)
    ax.legend(loc="upper left")
    plt.tight_layout()
    plt.show()


def _compact_event(e: dict) -> str:
    cat = e.get("cat", "?")
    kind = e.get("kind", "?")
    t = float(e.get("t", 0.0) or 0.0)
    line = int(e.get("_line", 0) or 0)
    parts = [f"L{line}", f"t={t:.3f}", f"{cat}/{kind}"]
    for key in (
        "pts",
        "path",
        "queue_len",
        "copy_ms",
        "keyed_mutex_ms",
        "keyed_mutex_acquire_ms",
        "keyed_mutex_cast_ms",
        "fence_wait_ms",
        "open_shared_ms",
        "copy_call_ms",
        "total_ms",
        "present_waitable_ms",
        "present_call_ms",
        "present_ms",
        "sync_interval",
        "late_ms",
        "gap_ms",
        "pool_len",
        "shared_handle",
        "dropped_full",
        "scale_ms",
        "decode_wait_ms",
        "send_wait_ms",
    ):
        if key in e:
            value = e.get(key)
            if isinstance(value, float):
                parts.append(f"{key}={value:.3f}")
            else:
                parts.append(f"{key}={value}")
    return "  ".join(parts)


def cmd_spike_context(
    events: list[dict],
    metric: str,
    threshold_ms: float,
    window_ms: float,
    limit: int,
    all_events: bool,
) -> None:
    spikes = [
        e
        for e in events
        if e.get("cat") == "native_presenter"
        and e.get("kind") == "fullscreen_present"
        and isinstance(e.get(metric), (int, float))
        and float(e.get(metric, 0.0)) >= threshold_ms
    ]
    spikes.sort(key=lambda e: float(e.get(metric, 0.0)), reverse=True)
    if limit > 0:
        spikes = spikes[:limit]

    print(
        f"native_presenter/fullscreen_present spikes: metric={metric} "
        f">= {threshold_ms:.1f}ms, window=+/-{window_ms:.0f}ms, count={len(spikes)}"
    )
    if not spikes:
        return

    window_s = window_ms / 1000.0
    relevant_cats = {"native_presenter", "video", "demux", "ui"}
    relevant_ui_kinds = {"frame_gap", "slow_frame_breakdown"}
    for i, spike in enumerate(spikes, 1):
        t0 = float(spike.get("t", 0.0) or 0.0)
        lo = t0 - window_s
        hi = t0 + window_s
        context = []
        for e in events:
            t = float(e.get("t", 0.0) or 0.0)
            if not (lo <= t <= hi):
                continue
            if not all_events:
                cat = e.get("cat")
                kind = e.get("kind")
                if cat not in relevant_cats:
                    continue
                if cat == "ui" and kind not in relevant_ui_kinds:
                    continue
            context.append(e)

        print()
        print(f"#{i} spike")
        print(_compact_event(spike))
        print(f"context events: {len(context)}")
        for e in context:
            marker = ">>" if e is spike else "  "
            print(f"{marker} {_compact_event(e)}")


# -----------------------------------------------------------------------
# main
# -----------------------------------------------------------------------

def cmd_scroll(events: list[dict]) -> None:
    """scroll_settle / visible_thumb_first_ready / visible_thumb_all_ready を join して
    可視サムネ表示の latency を集計する。

    出力:
      seq, t_rel, visible_first_idx, target_count, already_loaded,
      first_ready_ms, all_ready_ms, pool snapshot 抜粋
    """
    if not events:
        print("(イベント 0 件)")
        return
    t0 = min(e.get("t", 0.0) for e in events)

    # settle / first_ready / all_ready を seq でインデックス化
    settles: dict[int, dict] = {}
    firsts: dict[int, dict] = {}
    alls: dict[int, dict] = {}
    promotes: list[dict] = []
    suppresses: list[dict] = []
    for e in events:
        if e.get("cat") != "ui" and e.get("cat") != "pdf":
            continue
        k = e.get("kind")
        if k == "scroll_settle":
            seq = e.get("settle_seq", 0)
            settles[seq] = e
        elif k == "visible_thumb_first_ready":
            seq = e.get("settle_seq", 0)
            firsts[seq] = e
        elif k == "visible_thumb_all_ready":
            seq = e.get("settle_seq", 0)
            alls[seq] = e
        elif k == "pool_promote_visible":
            promotes.append(e)
        elif k == "prefetch_suppressed":
            suppresses.append(e)

    if not settles:
        print("(scroll_settle イベントなし — --perf-log で取得し直してください)")
        return

    print(f"=== Scroll settle latency ({len(settles)} settle events) ===")
    print(
        f"{'seq':>4}  {'t_rel':>8}  {'vis_first':>9}  "
        f"{'target':>6}  {'preload':>7}  {'first_ms':>8}  "
        f"{'all_ms':>8}  pool"
    )
    print("-" * 100)
    for seq in sorted(settles.keys()):
        s = settles[seq]
        rel = s.get("t", 0.0) - t0
        first = firsts.get(seq)
        all_ev = alls.get(seq)
        first_ms = f"{first.get('latency_ms', 0):.0f}" if first else "-"
        all_ms = f"{all_ev.get('latency_ms', 0):.0f}" if all_ev else "-"
        pool_str = (
            f"hn={s.get('pool_high_normal', '?')} "
            f"n={s.get('pool_normal', '?')} "
            f"if={s.get('pool_in_flight', '?')}"
        )
        print(
            f"  {seq:>3}  {rel:6.2f}s  {s.get('visible_first_idx', '?'):>9}  "
            f"{s.get('visible_target_count', '?'):>6}  "
            f"{s.get('already_loaded', '?'):>7}  {first_ms:>8}  {all_ms:>8}  {pool_str}"
        )

    # promote イベントの集計
    if promotes:
        print()
        print(f"=== pool_promote_visible: {len(promotes)} events ===")
        total_promoted = sum(p.get("promoted", 0) for p in promotes)
        total_already = sum(p.get("already_high", 0) for p in promotes)
        total_not_found = sum(p.get("not_found", 0) for p in promotes)
        print(f"  合計 promoted={total_promoted}  already_high={total_already}  not_found={total_not_found}")

    # prefetch_suppressed イベントの集計
    if suppresses:
        print()
        print(f"=== prefetch_suppressed: {len(suppresses)} events ===")
        starts = [s for s in suppresses if s.get("transition") == "start"]
        ends = [s for s in suppresses if s.get("transition") == "end"]
        conts = [s for s in suppresses if s.get("transition") == "continue"]
        total_supp_reg = sum(s.get("suppressed_regular", 0) for s in suppresses)
        total_supp_heavy = sum(s.get("suppressed_heavy", 0) for s in suppresses)
        total_pruned_reg = sum(s.get("pruned_regular", 0) for s in suppresses)
        total_pruned_heavy = sum(s.get("pruned_heavy", 0) for s in suppresses)
        backstop_hits = sum(1 for s in suppresses if s.get("backstop_hit"))
        print(
            f"  start={len(starts)} continue={len(conts)} end={len(ends)} backstop_hit={backstop_hits}"
        )
        print(
            f"  suppressed (new enqueue): regular={total_supp_reg} heavy={total_supp_heavy}"
        )
        print(
            f"  pruned (existing queue):  regular={total_pruned_reg} heavy={total_pruned_heavy}"
        )
        # 各 settle 時点の suppression 状態を関連付け
        # Codex P3-3: end だけでなく start/continue も拾い、settle 時点で suppression が
        # active かどうか (= 直前イベントが start/continue で end が来てない) を表示。
        print()
        print(
            f"{'settle':>6}  {'t_rel':>8}  {'sup@settle':>12}  {'last_event':>10}  "
            f"{'visible_pending':>15}  {'allow_reason':>30}"
        )
        print("-" * 100)
        # suppresses は時系列順に並んでいる前提 (perf logger は append-only)
        for seq in sorted(settles.keys()):
            s = settles[seq]
            s_t = s.get("t", 0.0)
            # settle 直前 (= 過去 2 秒以内) の prefetch_suppressed イベントを全種類拾う
            prior = [
                ev for ev in suppresses if 0.0 < (s_t - ev.get("t", 0.0)) < 2.0
            ]
            if not prior:
                continue
            last = prior[-1]
            last_trans = last.get("transition", "?") or "?"
            # active at settle: 直前イベントが start/continue なら active、end なら inactive
            sup_active = last_trans in ("start", "continue")
            sup_label = "ACTIVE" if sup_active else "inactive"
            vis_pending = last.get("visible_pending", "?")
            ar = last.get("allow_reason") or "-"
            rel = s_t - t0
            print(
                f"  {seq:>4}  {rel:6.2f}s  {sup_label:>12}  {last_trans:>10}  "
                f"{vis_pending:>15}  {ar:>30}"
            )

    # latency 分布
    first_latencies = [firsts[k].get("latency_ms", 0) for k in firsts]
    all_latencies = [alls[k].get("latency_ms", 0) for k in alls]
    if first_latencies:
        first_latencies.sort()
        n = len(first_latencies)
        print()
        print(
            f"first_ready latency: n={n} min={first_latencies[0]:.0f}ms "
            f"p50={first_latencies[n // 2]:.0f}ms "
            f"p95={first_latencies[int(n * 0.95) - (0 if n == 1 else 1)]:.0f}ms "
            f"max={first_latencies[-1]:.0f}ms"
        )
    if all_latencies:
        all_latencies.sort()
        n = len(all_latencies)
        print(
            f"all_ready   latency: n={n} min={all_latencies[0]:.0f}ms "
            f"p50={all_latencies[n // 2]:.0f}ms "
            f"p95={all_latencies[int(n * 0.95) - (0 if n == 1 else 1)]:.0f}ms "
            f"max={all_latencies[-1]:.0f}ms"
        )


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
    subs.add_parser("nav")
    subs.add_parser("startup")
    p_hit = subs.add_parser("hitches")
    p_hit.add_argument("--ms", type=float, default=33.0, help="ヒッチ閾値 (ms、既定 33.0)")
    p_idle = subs.add_parser("idle-health")
    p_idle.add_argument("--start-t", type=float, default=None, help="測定開始 t (秒)")
    p_idle.add_argument("--end-t", type=float, default=None, help="測定終了 t (秒)")
    p_idle.add_argument(
        "--window-secs",
        type=float,
        default=15.0,
        help="start-t 省略時に end-t から遡る秒数 (既定 15)",
    )
    p_idle.add_argument("--target-update-rate", type=float, default=2.0)
    p_idle.add_argument("--max-update-rate", type=float, default=10.0)
    p_idle.add_argument("--max-reason-streak-secs", type=float, default=2.0)
    p_idle.add_argument("--max-same-work", type=int, default=3)
    p_idle.add_argument("--max-input-events", type=int, default=0)
    p_idle.add_argument("--expected-pid", type=int, default=None)
    p_idle.add_argument("--allow-sleeping-window", action="store_true")
    p_idle.add_argument("--evidence-start-t", type=float, default=None)
    p_idle.add_argument(
        "--require-idle-upgrade-ineligible",
        action="store_true",
        help="準備開始から測定終了までに動画ピン完成キャッシュ除外の証拠を要求",
    )
    p_idle.add_argument("--json-out", type=Path, default=None)
    p_avd = subs.add_parser("av_drift")
    p_avd.add_argument(
        "--plot",
        action="store_true",
        help="matplotlib で時系列プロット (drift + underrun 帯 + norm 縦線 + pts_jump マーカー)",
    )
    p_spike = subs.add_parser("spike-context")
    p_spike.add_argument("--metric", default="keyed_mutex_acquire_ms")
    p_spike.add_argument("--ms", type=float, default=16.0, help="spike 閾値 (ms、既定 16.0)")
    p_spike.add_argument("--window-ms", type=float, default=100.0, help="前後 window (ms、既定 100)")
    p_spike.add_argument("--limit", type=int, default=10, help="表示する spike 数 (0 なら全件)")
    p_spike.add_argument("--all-events", action="store_true", help="audio 等を含む全イベントを表示")
    p_dump = subs.add_parser("dump")
    p_dump.add_argument("seq", type=int)
    p_dump.add_argument("--with-frames", action="store_true", help="frame.begin も表示する")
    p_tl = subs.add_parser("timeline")
    p_tl.add_argument("seq", type=int, nargs="?", default=None)
    subs.add_parser("scroll")

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
    elif args.cmd == "nav":
        cmd_nav(events)
    elif args.cmd == "startup":
        cmd_startup(events)
    elif args.cmd == "hitches":
        cmd_hitches(events, args.ms)
    elif args.cmd == "idle-health":
        sys.exit(
            cmd_idle_health(
                events,
                args.start_t,
                args.end_t,
                args.window_secs,
                args.target_update_rate,
                args.max_update_rate,
                args.max_reason_streak_secs,
                args.max_same_work,
                args.max_input_events,
                args.json_out,
                args.expected_pid,
                args.allow_sleeping_window,
                args.evidence_start_t,
                args.require_idle_upgrade_ineligible,
            )
        )
    elif args.cmd == "av_drift":
        cmd_av_drift(events, args.plot)
    elif args.cmd == "spike-context":
        cmd_spike_context(events, args.metric, args.ms, args.window_ms, args.limit, args.all_events)
    elif args.cmd == "dump":
        cmd_dump(events, args.seq, args.with_frames)
    elif args.cmd == "timeline":
        cmd_timeline(events, args.seq)
    elif args.cmd == "scroll":
        cmd_scroll(events)


if __name__ == "__main__":
    main()
