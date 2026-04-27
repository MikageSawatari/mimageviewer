#!/usr/bin/env bash
# 起動・ナビゲーション・検索 の代表シナリオで perf-log を取得し、
# `analyze_perf.py hitches` で UI フリーズ (16ms 以上のフレーム間隔) を検出する。
#
# CI が無いので手動運用 (CLAUDE.md リリース手順 Phase 2 参照)。
#
# 使い方:
#   bash scripts/perf_smoke.sh
#
# このスクリプトは mImageViewer をフォアグラウンドで起動するだけ。実際のシナリオ操作
# (Ctrl+↓ 連打 / Ctrl+G で検索 / Ctrl+W で閉じる) はユーザーが手動で行う。終了後に
# %APPDATA%\mimageviewer\logs\perf_events.jsonl を analyze_perf.py に流して結果を出す。

set -euo pipefail

EXE="target/release/mimageviewer.exe"
LOG_DIR_WIN="${APPDATA:-$USERPROFILE/AppData/Roaming}/mimageviewer/logs"
PERF_LOG="$LOG_DIR_WIN/perf_events.jsonl"
THRESHOLD_MS=${PERF_HITCH_MS:-16}

if [[ ! -x "$EXE" ]]; then
    echo "ERROR: $EXE が見つかりません。先に cargo build --release を走らせてください。" >&2
    exit 2
fi

echo "=== perf smoke ==="
echo "1. mImageViewer を --perf-log 付きで起動します。"
echo "2. 以下のシナリオを **手動で** こなしてください:"
echo "   a) 任意のフォルダを開く (Ctrl+Shift+O 等)"
echo "   b) Ctrl+↓ を 5 回押下 (フォルダ間移動)"
echo "   c) Ctrl+G で検索バーを開き、何か入力 → Enter"
echo "   d) ウィンドウを閉じる ([×] で OK、トレイ常駐になっても問題なし)"
echo "3. プロセス終了後、perf_events.jsonl を解析します。"
echo
echo "perf-log: $PERF_LOG"
echo

# 既存の perf log を退避 (最新の 1 シナリオだけ見たい)
if [[ -f "$PERF_LOG" ]]; then
    mv "$PERF_LOG" "$PERF_LOG.prev"
    echo "(既存ログを $PERF_LOG.prev に退避)"
fi

"$EXE" --perf-log
RC=$?
echo
echo "mImageViewer exit code: $RC"

if [[ ! -f "$PERF_LOG" ]]; then
    echo "ERROR: perf-log が生成されていません ($PERF_LOG)。" >&2
    exit 3
fi

echo
echo "=== analyze_perf.py hitches (>= ${THRESHOLD_MS}ms) ==="
python scripts/analyze_perf.py "$PERF_LOG" hitches --ms "$THRESHOLD_MS"

echo
echo "目視確認の観点:"
echo "  - 'ヒッチ: 0 件' なら OK。"
echo "  - 数件出ても直前 nav に妥当な遷移 (PDF cold open 等、~700ms) が紐付いていれば許容。"
echo "  - nav イベントなしのヒッチは UI スレッド同期 I/O 退行の疑い。"
echo "  - p95 > 50ms かつ件数が二桁なら回帰。docs/ui-responsiveness.md §4 に沿って原因切り分け。"
