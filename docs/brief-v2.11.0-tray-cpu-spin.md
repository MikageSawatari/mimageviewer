# ブリーフ: トレイ常駐中にメインスレッドが 1 コアを 100% 占有する不具合

対象バージョン: v2.10.0 (リリース済み) で再現。修正は v2.11.0。
担当: 実装 = Codex Sol / レビュー・検収 = ClaudeCode。

## 1. 観測事実 (2026-08-03 実測)

利用者環境で、mImageViewer をタスクトレイに格納したあと **メインスレッドが 1 コアを
97〜98% 使い続ける**。13 分以上経過しても解消しなかった。

計測値:

```
Path : %APPDATA%\mimageviewer\runtime\2.10.0\mimageviewer-core.exe --perf-log
起動 : 21:47:18

mimageviewer-core (PID 93820) : 9.81 秒 / 10 秒 = 98% (1 コア換算)
dwm                           : 0.00 秒 / 10 秒 = 0%
システム全体                  : 15.7%  プロセッサキュー長 0.3
```

スレッド単位 (8 秒サンプリング、172 スレッド中):

```
  Tid DeltaSec Pct   State
37028     7.73  97 Running     <- プロセス開始時刻と同一 = メインスレッド
(他 171 スレッドはすべて Wait)
```

ウィンドウ列挙の結果、可視のトップレベルウィンドウは 1 個 (無題) のみ。
`IsIconic` は false。つまり最小化ではなく `SW_HIDE` によるトレイ格納状態。

## 2. 決定的なログ

`%APPDATA%\mimageviewer\logs\mimageviewer.log` のメインスレッド (t1) 最終ログ:

```
 35.34s  [heartbeat] app_update fullscreen=Some(0) native_main_backdrop=false ...
 36.02s  tray: closing fullscreen/media session before residency fullscreen=Some(0) fs_video=false native_pending=false
 36.02s  conceal: reset mode
 36.02s  text: reset mode
 36.10s  settings: save ok: favorites=24 rotated=false
 36.10s  tray: window hidden to tray (Win32 SW_HIDE, placement saved); retained_viewports=0 routed_presenters=0
 36.10s    [queue] push +20H +0L  keep=[0..20)  vis=[0..20)  requested=20
        <- メインスレッドのログはここで途絶。以降 13 分間ゼロ。
```

同セッションの `perf_events.jsonl` では `frame.begin` イベントが **t=35.3s で停止**している
(それ以前は 2062 件、最大 118fps で出ていた)。

トレイスレッド (t39) と VST3 bridge (t42) は以降も通常どおり heartbeat を出し続けており、
それらは正常。

## 3. ここから言えること

- `App::update` は走っていない (`frame.begin` が出ない、ログも出ない)
- それでもメインスレッドは `Running` で 1 コア全開
- したがって **winit のイベントループだけが空回りしている**
- ウィンドウが hidden なので present が発生せず、vsync による throttle もかからない
  ため、60fps ではなく CPU 上限まで回っている

## 4. 仮説 (検証すること。決め打ちで修正しないこと)

`src/tray_integration.rs` の `hide_to_tray` (190 行付近) には次の設計意図が書かれている。

> **重要**: `ViewportCommand::Visible(false)` は使わない。どちらの hide でも通常の
> `App::update` は止まるが、Win32 `ShowWindow(hwnd, SW_HIDE)` を直接使えば…

つまり「hide したら update は止まる」前提で組まれている。実際 update は止まっている。
問題は **止まった後にイベントループが sleep に入らない**ことである。

有力な筋:

1. hide の直前・直後に発行された repaint 要求 (即時 repaint) が消化されないまま残る。
   ログの `[queue] push +20H +0L requested=20` が示すとおり、hide 時点で
   **Pending サムネイル 20 件を抱えていた**。`docs/display-pipeline.md` §1.2 のとおり
   Pending が残る間は毎フレーム `ctx.request_repaint()` する仕様なので、hide 直前の
   update がこれを要求している可能性が高い。
2. eframe は window が不可視だと `run_ui_and_paint` をスキップする。スキップされると
   repaint 要求が消化されず、`ControlFlow` が `Poll` (または即時 wake) のまま次の
   ループへ入る。以降、要求を消す主体が存在しないので永久に回る。
3. `hide_to_tray` 末尾の `if keep_detached_viewer_alive { ctx.request_repaint(); }`
   (240-242 行) は今回のケースでは false のはずだが、経路として確認すること。

**ただしこれは仮説である。** 実際にどこでループしているかを、計装またはデバッガで
特定してから修正すること。仮説に合う修正を先に書かないこと。

## 5. 制約 (CLAUDE.md「バグ修正の一般原則」に従う)

- **症状パッチを入れない**。「hidden のときは `std::thread::sleep` を挟む」「無条件に
  `ControlFlow::Wait` へ落とす」といった、根本原因に対応しない回避策は不可。
  トレイ常駐中も、トレイスレッドからの復帰要求・resident media の WM_PAINT bridge・
  外部からの起動要求は従来どおり動く必要がある。
- 状態の producer / consumer を列挙すること。repaint 要求を出す側 (Pending サムネイル、
  detached viewer、resident media、indexer 等) と、それを消化する側 (update / paint) の
  対応関係を整理し、**hidden 中に要求だけが積まれて消化されない構造そのもの**を直す。
- 同型の入口も確認すること。トレイ格納は `maybe_intercept_close` 経由だけでなく、
  トレイメニュー、最小化、外部要求など複数の入口があり得る。open / restore / quit の
  各ライフサイクルで同じ状態が正しく処理されるか確認する。
- **detached viewer に触れる場合は要注意**。`docs/detached-rework-plan.md` の凍結ルールが
  有効。`keep_detached_viewer_alive` 経路を変更する必要が出た場合は、実装前に
  プラン §2 を読み、症状パッチではなく構造的修正であることを ClaudeCode と合意してから
  進め、触れた範囲を同プランに記録すること。

## 6. 期待する成果物

1. **原因の特定と記録** — どこでループしていたかを、証拠 (計装ログ・イベント列など) と
   ともに報告する。`docs/` に調査結果を残す (ファイル名は任せる。既存の
   `docs/idle-health-check.md` へ追補する形でもよい)。
2. **根本修正** — 上記の制約を満たすこと。
3. **回帰テスト**:
   - 純ロジックとして切り出せる部分は unit test を追加する
     (例: 「hidden 状態では repaint 要求を積まない / 積んだ要求が hidden 遷移時に
     解決される」ことを状態遷移として検証する)
   - `scripts/check-idle-health.ps1` に **トレイ格納シナリオを追加**する。既存の
     `static-foreground` / `static-background` / `video-pin-background` と同じ枠組みで、
     「サムネイル読み込み中にトレイへ格納 → 一定時間後に CPU 使用率が閾値以下」を
     判定できるようにする。`docs/idle-health-check.md` も更新する。
     このシナリオが無かったことが、本件がリリースまで残った直接の理由である。
4. **ドキュメント更新** — `docs/ui-responsiveness.md` に、hidden / トレイ常駐中の
   repaint 契約を明記する。

## 7. 検証手順 (利用者に依頼する実機確認の想定)

1. mIV を起動し、サムネイルが多いフォルダを開く
2. サムネイル読み込みが完了しきる前にウィンドウを閉じてトレイへ格納する
3. 1 分放置し、タスクマネージャーで `mimageviewer-core.exe` の CPU が
   ほぼ 0% であることを確認する
4. トレイアイコンから復帰し、サムネイルが正常に表示されること、
   ウィンドウ位置・サイズが復元されることを確認する
5. 動画・音楽を再生中にトレイ格納した場合は、従来どおり再生が継続すること
   (resident media 経路の退行がないこと)

## 8. 参照

- `src/tray_integration.rs` — `hide_to_tray` / `sync_after_restore` / `maybe_intercept_close`
- `src/tray.rs` — トレイスレッド (50ms ループ、resident media WM_PAINT bridge)
- `docs/display-pipeline.md` §1.2 — Pending サムネイルと毎フレーム repaint の仕様
- `docs/ui-responsiveness.md` — UI スレッド応答性の方針と計装
- `docs/idle-health-check.md` — アイドル健全性チェックの仕様と閾値
- `CLAUDE.md`「バグ修正の一般原則」「並行処理: try_lock + sleep は使わない」

## 9. 作業手順

1. ブランチ `fix/tray-residency-cpu-spin` を切る
2. 調査 → 原因特定 → 修正 → テスト追加
3. `cargo fmt` (引数なし・ワークスペース全体) を必ず通す
4. `cargo test -p mimageviewer --lib` と、関連する統合テストを通す
5. 完了したら ClaudeCode へ「原因・修正内容・テスト・触れた範囲」を報告する
   (実機確認は利用者が行うので、検証用ビルドの作成は ClaudeCode 側で行う)
