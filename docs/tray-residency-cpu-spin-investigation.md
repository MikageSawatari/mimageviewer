# トレイ常駐時 CPU spin 調査記録

調査日: 2026-08-03  
対象: v2.10.0 / eframe 0.33.3 / winit 0.30.13 / Windows

## 観測された失敗と不変条件

実環境では、メインウィンドウを `ShowWindow(SW_HIDE)` でトレイへ格納した直後から
`mimageviewer-core.exe` のメインスレッドが 1 論理コアの 97〜98% を使い続けた。一方、
`frame.begin` と通常ログは格納時点で停止していた。この組合せは `App::update` 内のループではなく、
その外側にある winit event loop が空回りしていることを示す。

期待する不変条件は次のとおり。

- paused media / still のトレイ常駐中は、要求が無ければ event loop は sleep する。
- 非表示中に正当な repaint が要求された場合は、OS の redraw 通知に依存せず有限時間で消化する。
- active resident media は既存の bounded `WM_PAINT` bridge で進み、トレイからの復帰・終了と
  外部起動による復帰も従来どおり処理する。

## 原因を特定した証拠

一時計装を eframe 0.33.3 の `WinitAppWrapper::check_redraw_requests` に入れ、期限到来 repaint の
対象ウィンドウについて `Window::is_visible()` と `ControlFlow` 遷移を記録した。サムネイル要求が
残った状態で同じ HWND を非表示にすると、次の順序を確認した。

1. `App::update` 末尾で `requested` が非空となり repaint を要求する。
2. eframe の repaint 時刻が到来する。
3. 対象 HWND は `is_visible() == Some(false)` なのに、eframe 0.33.3 は先に
   `ControlFlow::Poll` を設定して `window.request_redraw()` を呼ぶ。
4. Windows は非表示 HWND に `RedrawRequested` を返さないため、`run_ui_and_paint` は呼ばれない。
5. eframe は repaint map から要求を削除済みで、`WaitUntil` へ戻す次の時刻も無い。
   後続イベントが無ければ `Poll` が残り、winit の main thread だけが空回りする。

一時計装の記録は次の 1 行で、製品コードからは削除済みである。

```text
due repaint: invisible window; ControlFlow::Poll set before request_redraw
```

隔離 portable build の 8 秒試行では、その後に到着した thumbnail worker event が
`ControlFlow::Wait` を設定したため CPU は 1 コア換算 0.8% に戻った。これは失敗が
「最後の due repaint の後に別イベントが来るか」に依存して不安定に見える理由でもある。
実環境の 13 分継続した 97〜98% は、後続イベントが無い側の同じ遷移で説明できる。

winit 0.30.13 の Windows 実装も確認した。`Window::request_redraw()` は
`RedrawWindow(..., RDW_INTERNALPAINT)` を使うが、非表示 HWND では対応する
`RedrawRequested` が配送されない。さらに、この事象と修正方針は eframe upstream の
[Issue #7776](https://github.com/emilk/egui/issues/7776) と、2026-03-24 に merge された
[PR #7905](https://github.com/emilk/egui/pull/7905) と一致する。したがって brief の仮説を
アプリ側の状態から推測したのではなく、実際の event-loop 遷移と依存実装の両方で原因を確定した。

## producer / consumer とライフサイクル

| 区分 | producer / 入口 | consumer / 結果 |
| --- | --- | --- |
| thumbnail | Pending、upload backlog、worker completion が `request_repaint` | 可視時は OS `RedrawRequested`、非表示時は eframe が直接 `run_ui_and_paint` |
| async UI work | AI、検索、スマートフォルダ、編集 preview 等の worker 完了と期限付き poll | 同上。非表示中の要求だけを最短 100 ms に制限 |
| viewport lifecycle | fullscreen cleanup、detached viewer の既存 repaint 要求 | viewport command を含め同じ scheduler が消化。detached の predicate / state は変更しない |
| tray restore / quit | tray thread の `ShowWindow` / `WM_CLOSE` と外部復帰検知 | 復帰後の通常 event、または非表示 window の direct UI pass で処理 |
| active resident media | tray thread の 50 ms bounded `WM_PAINT` bridge | winit `RedrawRequested` → `App::update`。既存経路を維持 |

旧実装の欠陥は producer が多いことではなく、非表示時だけ consumer が OS 通知とともに消失するのに
scheduler が `Poll` を選んだことだった。したがって個々の producer を hidden flag で抑止しない。

## 根本修正

プロジェクトが使う eframe 0.33.3 を `vendor/eframe` として固定し、upstream PR #7905 の
scheduler 修正を同版へ backport した。

- 期限到来時、visible window だけに `ControlFlow::Poll` + `request_redraw` を使う。
- invisible / minimized window は `run_ui_and_paint` を直接呼び、OS が返さない redraw event を
  consumer にしない。これにより復帰等の viewport command も処理できる。
- hidden window の `RequestRepaint` は最短 100 ms 後へ制限し、短周期要求が event 側から
  throttle を迂回できないようにする。
- direct pass 後は「アプリが既に要求した repaint」にだけ `max(要求済み時刻, now + 100 ms)` を
  適用する。即時要求は後ろへ送る一方、5 秒後など既に先の予定は早めない。要求が無い window
  へ heartbeat は新規作成しないため、still / paused は完全 sleep へ収束する。

`App` や tray code に hidden 専用 guard、追加 state、sleep、repaint 全消去は追加していない。
backend への hidden sleep も追加していない。これは CPU を薄める症状パッチではなく、要求を
消化する所有境界を OS callback から eframe scheduler へ移す修正である。

## hidden 中の 2 つの駆動経路

hidden 中の `App::update` には目的の異なる 2 つの入口がある。

1. vendored eframe scheduler は、thumbnail / worker / viewport command などアプリ要求済みの
   generic repaint を direct `run_ui_and_paint` で消化する。各要求は 100 ms 以上後へ下限設定され、
   要求が無いときは起床しない。
2. tray thread の 50 ms `WM_PAINT` bridge は、active resident media の時間進行だけを保証する。
   `player_needs_resident_media_updates` が live play intent / 未処理 continuous EOF を、
   `tray_resident_media_updates_needed` が EOF candidate resolver / typed handoff まで projection する。

両入口は同じ winit main thread 上で直列に `App::update` を実行するため、player や navigation state を
並行更新しない。bridge は atomic pending claim が true の間は次を post せず、すべての
`App::update` 入口がその claim を ack する。scheduler と bridge の期限が近い場合に UI pass が
逐次しても、bridge wake が無制限に積まれたり並行実行されたりはしない。

再生中は `VideoPlayer::tick` の 16 ms repaint 要求が scheduler 側では 100 ms 下限になるが、bridge の
projection は EOF 検出、candidate resolver、source-swap / native-open / tile-swap handoff の完了まで
維持される。したがって動画・音楽の EOF 検出と次トラック遷移は 50 ms pump の粒度で継続し、
generic scheduler の 100 ms 待ちへ退行しない。paused / handled terminal EOF / still は projection
対象外なので bridge も停止する。

## 回帰検査

- eframe unit test: production の `throttle_existing_repaint` に対して、即時要求を 100 ms 後へ
  遅らせること、先の予定を早めないこと、要求の無い hidden window に heartbeat を挿入しない
  ことを検査する。
- `scripts/check-idle-health.ps1 -Scenario tray-residency`: サムネイル読込中に手動でトレイへ
  格納し、測定開始・終了の両方で「対象 PID が top-level HWND を所有し、その可視数が 0」を確認後、
  既存と同じ 15 秒 CPU / update / repaint / work / log gate を適用する。
- tray / App の駆動ロジックと detached viewer の predicate / viewport state は変更せず、App / tray 側コメントだけを 2 経路の契約へ合わせた。

## 自動検証結果

2026-08-03 に次を確認した。

- `cargo fmt`: PASS
- `cargo fmt --manifest-path vendor/eframe/Cargo.toml -- --check`: PASS
- vendored eframe hidden scheduler unit test: 3 passed
- `cargo check -p mimageviewer --bin mimageviewer-core`: PASS
- `cargo test -p mimageviewer --lib`: 4689 passed / 0 failed / 21 ignored
- `scripts/test_analyze_perf.py`: 16 passed
- `check-idle-health.ps1`: PowerShell parser、埋め込み C# compile / invoke、scenario / report field の
  static contract が PASS

`tray-residency` の CPU 実測と restore / active media の実機確認は release verification binary と
利用者環境を使うため、brief §7 / §9 に従ってレビュー・検収側で行う。

---

## 検証状況 (2026-08-03, ClaudeCode 検収)

**確認済み**

- vendored eframe のスケジューラ単体テスト 3 本: 通過
- `cargo test -p mimageviewer --lib`: 4689 passed / 0 failed / 21 ignored
- `cargo fmt --check` (リポジトリルート): 通過
- コードレビュー: throttle の向き (`max`)、`UserEvent::RequestRepaint` 経路の追加ガード
- **退行が無いこと**: portable-smoke の隔離環境で、可視 / 最小化 / 非表示 (SW_HIDE) の
  いずれも 0.4〜1.6% (1 コア換算)。heartbeat 間隔が 24〜33 秒へ伸び、深い就寝に入る

**未確認 — 元の症状が消えたことの実証**

隔離環境で「hidden 移行時に repaint 要求が残っている」条件を作れなかった。試行:

| 試行 | 結果 |
| --- | --- |
| PNG 252 枚のフォルダ | 1.3 秒で読み込み完了 → 就寝。最小化時点で pending なし |
| PDF 40 ファイル (各 6 ページ) | 5 秒未満で完了 → 就寝。同上 |
| アニメーション GIF をフルスクリーン再生 | `PostMessage(WM_KEYDOWN, VK_RETURN)` が winit/egui の入力経路に届かず、フルスクリーンが開かなかった (ログの `fullscreen=None` で確認) |

利用者側でも「サムネイル一覧は通常一瞬で終わるので、読み込み中にトレイ格納する操作自体が
難しい」ため再現に至らず、2026-08-03 に**再現確認を見送る判断**をした。

外部からの `SW_HIDE` は `hide_to_tray` (GPU リソース解放・設定保存・heartbeat 抑制を含む)
を通らないため、仮に成功していても実経路の完全な代替にはならない点にも注意。

**残った検証手段**: `scripts/check-idle-health.ps1 -Scenario tray-residency`。
設計上この検証のために作られており、リリースゲートでもある。再現条件を作れる状況が
できたら実行すること。
