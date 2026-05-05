# VST3 プラグイン統合設計 (v0.9.0)

## 1. ゴール (v0.9.0 スコープ)

動画音声に **VST3 プラグインのチェーン** (複数プラグインを直列接続) を挿入して、
加工後の音声をスピーカーに出力する。LUFS 測定 + EQ + コンプ等の組み合わせを想定。

設計判断 (= [vst-bitwig-... プラン](file:///C:/Users/mikag/.claude/plans/vst-bitwig-vst-vst-lufs-eq-vst-scalable-flame.md) からの抜粋):

- **C++ bridge プロセス** (= `mimageviewer-vst3-host.exe`) で VST3 SDK を扱う。
  Rust 本体とは stdin/stdout (制御) + shared memory + named event (音声)。
- **Phase 0b は完成済み**。`crates/vst3-host/` (C++) と
  `crates/vst3-host-tester/` (Rust 検証用 GUI) が動作確認済み。
- **v0.9.0 はチェーン (複数プラグイン) 対応**。各プラグインは別 bridge 子プロセスで
  動かし、`audio-pump` がチェーン順に IPC を回す。プラグインクラッシュは隣のプラグインに
  波及しない (= 個別 bridge プロセス分離)。
- **チェーン長の実用上限**: 各 IPC roundtrip ~1-2ms × N。1024-sample frame
  (= 21ms) を realtime で処理する余裕を考慮すると **5 個程度まで**が安全圏。
- **デフォルト OFF**。利用者は少数想定。環境設定で ON にしたときに初回スキャン。
- **プラグインインスタンス永続化**。アプリ起動中ずっと bridge 群を握り、動画切替で
  再ロードしない (= EQ カーブや LUFS の積算が動画切替で消えない)。
- **プラグイン GUI はフルスクリーン再生中だけ最前面**。動画フルスクリーンが
  focus を持つ間は VST GUI host と bridge-owned plugin surface の両方を TOPMOST
  にし、Alt+Tab 等で他アプリへ切り替えたら NOTOPMOST に戻す。再度フルスクリーン
  に focus が戻ったら動画の上へ復帰する。

## Fullscreen focus handoff (2026-05)

VST editor windows are native windows owned by the bridge process. When a plugin
editor has focus, fullscreen keyboard shortcuts must stay inactive so the plugin
can receive keys. If the user clicks the fullscreen video/image area, mIV
reclaims focus for that native fullscreen window with `SetForegroundWindow` /
`SetActiveWindow` / `SetFocus` and also sends `ViewportCommand::Focus`. While
the VST GUI workspace is visible, fullscreen background clicks are treated as
focus-restoration clicks and suppressed until the primary button is released.
Playback control in that mode should use keyboard commands such as `Enter`,
which avoids accidentally toggling play/pause when the user's intent is to move
focus away from a plugin editor. If a key event is delivered to the main
viewport instead of the fullscreen viewport during this handoff, mIV forwards
the fullscreen shortcut keys through the same fullscreen key handler before the
main-grid shortcut gate can discard them. The VST3 manager panel itself is not
treated as a modal dialog for fullscreen media shortcuts, so video keys still
work while the panel is open.
The handoff check stores the previous frame's Win32 foreground HWND and compares
that value against the clicked fullscreen HWND rather than only trusting
`viewport().focused`, because cross-process owner windows can keep egui's focus
  flag true and a click can activate fullscreen before the click handler observes
  the foreground window.
The native Win32 focus call is only made when the previous/current foreground
state shows that fullscreen actually needs to reclaim focus, and the call is
rate-limited. Plain fullscreen clicks where `GetForegroundWindow()` already
matches the fullscreen HWND must not call `SetForegroundWindow`/`SetFocus`,
because repeated no-op focus calls can briefly stall the Windows input queue and
show up as simultaneous audio/video hitches.

## Editor chrome resize rules (2026-05)

The bridge-owned VST editor surface uses a custom dark title area above the
plugin child HWND. The title area includes a left-side power button that toggles
the slot bypass state through the same Rust `DspBridge::set_bypass` path used by
the VST3 playback panel, so the setting is persisted in `settings.json` and the
normal PDC auto-bypass guard still applies. The right side shows the current
plugin-reported latency as a separate `ms` readout next to the close button; the
readout is repainted when the bridge observes `kLatencyChanged`. The right-side
close button hides only that plugin editor and marks the slot `user_hidden=true`.

When the user resizes the outer editor frame, the bridge
first asks `IPlugView::checkSizeConstraint` and snaps the outer frame back to
the plugin-approved client size before calling `IPlugView::onSize`. Plugin
initiated `IPlugFrame::resizeView` calls are accepted unless a native
WM_ENTERSIZEMOVE resize session is actively in progress; a stale WM_SIZE
timestamp alone is not enough to suppress them, because many editors implement
their own resize handle inside the plugin view. Resize-path redraws only
invalidate the affected HWNDs and let the normal message pump repaint them; they
do not use synchronous `RDW_UPDATENOW`, which can make native resize drags wait
for plugin relayout/paint work on every mouse step.

## 2. 全体構成

```
mimageviewer-core.exe (Rust)
├─ DspBridge (singleton, src/video/dsp/)
│   ├─ Vec<PluginSlot>          ← チェーン (順番が音声適用順)
│   │   ├─ Slot[0]: bridge プロセス + プラグイン名 + GUI HWND + bypass フラグ
│   │   ├─ Slot[1]: ...
│   │   └─ ...
│   ├─ active_slot_count (atomic): bypass=false の Loaded 個数
│   └─ scratch_a / scratch_b: 多段チェーンの ping-pong 用 (alloc 再利用)
├─ src/video/audio.rs: audio-pump thread が DspBridge::process_block を呼び、
│   全アクティブスロットを順番に IPC で通す。enable=false / 全 bypass / 0 個なら no-op
├─ src/video/dsp/gui.rs: プラグイン GUI ホスト (Win32 子ウィンドウ)
│   - フルスクリーン中は WS_EX_TOPMOST で動画の手前に維持
│   - 各スロットが個別の HWND を持つ。再生中パネルから全体 / 個別表示を切り替える
└─ Settings (settings.json):
    - vst3_enabled: bool (default false)
    - vst3_plugins: Vec<Vst3PluginEntry>  ← チェーン定義
    -   .path: String
    -   .bypass: bool
    -   .state: Option<Base64<...>>  (= IComponent::getState)
    -   .user_hidden: bool
    -   .window_rect: Option<...>
    - vst3_gui_visible: bool  (再生中パネルの全体表示状態)

各 PluginSlot は独立した bridge 子プロセス:
vendor/vst3-host/mimageviewer-vst3-host.exe (C++ bridge)
├─ 1 プロセス = 1 プラグイン (= プラグインクラッシュの隔離)
├─ stdin/stdout: 制御 (length-prefixed JSON)
├─ Shared memory: 2 本の SPSC ring (in/out, f32 stereo) — slot 別に独立
└─ Named events: sig_in / sig_out で同期

include_bytes! でメイン exe に埋め込み、初回 enable 時に
%APPDATA%\mimageviewer\vst3\mimageviewer-vst3-host.exe へ展開
(PDFium / Susie ワーカー / FFmpeg DLL と同パターン)
```

## 3. ディレクトリ / モジュールマップ

| パス | 役割 | 状態 |
| --- | --- | --- |
| `crates/vst3-host/` (C++) | VST3 ホスト bridge (Phase 0b 済) | 既存 |
| `crates/vst3-host-tester/` (Rust) | 単独検証用 GUI (Phase 0b 済) | 既存 (リリース exe には含めない) |
| `vendor/vst3sdk/` | Steinberg VST3 SDK (MIT, gitignore) | 既存 |
| `vendor/vst3-host/mimageviewer-vst3-host.exe` | bridge ビルド成果物 | 既存 |
| `src/video/dsp/mod.rs` | DspBridge 公開 API + module root | **新規** |
| `src/video/dsp/bridge.rs` | bridge 子プロセス管理 + IPC | **新規** (testerからポート) |
| `src/video/dsp/shm.rs` | shared memory + SPSC ring (Windows) | **新規** (testerからポート) |
| `src/video/dsp/scanner.rs` | VST3 plugin scan (`%COMMONPROGRAMFILES%\VST3\` 等) | **新規** (testerからポート) |
| `src/video/dsp/gui.rs` | プラグイン GUI 用の Win32 親ウィンドウ管理 | **新規** (testerからポート) |
| `src/video/dsp/extract.rs` | bridge exe の APPDATA 展開 (PDFium pattern) | **新規** |
| `src/settings.rs` | VST3 設定 (`vst3_enabled`, `vst3_plugins`, `vst3_gui_visible`, `vst3_chain_slots`) | 拡張 |
| `src/ui_dialogs/preferences.rs` | 環境設定→VST3 プラグインページ | 拡張 |
| `src/ui_dialogs/vst3_manager.rs` | 動画再生中の VST3 プレイバックパネル | **新規** |
| `src/video/audio.rs` | pump thread に DspBridge 経由処理を挿入 | 拡張 |
| `src/app.rs` | DspBridge を保持し、起動時ロード / 終了時 snapshot を行う | 拡張 |
| `build.rs` | `vendor/vst3-host/mimageviewer-vst3-host.exe` 存在チェック (PDFium と同様) | 拡張 |

## 4. 音声経路への結線

現状 (master の v0.8 系):
```
decoder → audio_rx → audio-pump thread → AudioBuffer (Mutex) → cpal RT → 出力
```

VST3 enable 時:
```
decoder → audio_rx → audio-pump thread:
   if vst3_enabled && bridge.is_loaded():
       bridge.process_block(frame.samples) → frame.samples
       safety_limiter(frame.samples)        → frame.samples
   AudioBuffer に push
                → cpal RT → 出力 (変更なし)
```

設計判断:

- **plugin process は audio-pump thread で実行する**。cpal RT スレッドではない。
  bridge IPC roundtrip (~1-2ms) を AudioBuffer の processed depth (100ms) で吸収する。
- **VST3 チェーン後段には安全 limiter を入れる**。ユーザーが limiter プラグインを
  末尾に挿していない場合の保険で、5ms lookahead / -1dBFS ceiling / 100ms release の
  固定 sample-peak limiter とする。true-peak limiter ではないため inter-sample peak は
  完全保証しないが、視聴時の hard clip 保険として扱う。VST3 が無効、または active
  plugin が無い場合は完全に bypass する。
- **enable=false なら処理ゼロオーバーヘッド**。frame をそのまま push。
- **bridge unload 中も音声は流れる**。ロード前は plugin pass-through (= 何もしない)。
- **block size は decoder のフレームサイズに依存しない**。bridge 側で variable
  block size を扱える (Phase 0b で実装済)。

bridge IPC のレイテンシ実測 (Phase 0b):
- `set_event` + `wait_event` 1 周: 1-2ms (Windows context switch)
- 100ms buffer に対して十分小さい。realtime 維持可能。

## 5. プラグイン GUI ホスティング

要件:
- アプリ起動中ずっとプラグイン GUI を表示しておける
- 動画再生中のホバーバーから VST3 プレイバックパネルを開き、チェーン全体の
  ON/OFF、個別 GUI 表示、bypass を操作できる
- プラグインの追加・削除・並べ替えは環境設定→VST3 プラグインページで行う
- 閉じた plugin GUI は `user_hidden` として保存し、次回の全体表示で勝手に復活させない

実装:
- bridge プロセス側で `IPlugView::attached(hwnd)` で親ウィンドウに接続
  (Phase 0b で動作確認済)
- ホスト側 (Rust) は `winit` ではなく **CreateWindowExW で独立 HWND** を作成
  (eframe のメインビューポートとは別ウィンドウ)
- `SetParent` はクロスプロセスでも動作する (= bridge が hwnd 値を受け取って
  自プロセス内で `IPlugView::attached(hwnd)` を呼ぶ)
- リサイズ追従 / DPI 対応は Phase 0b で完成済 (`tester/src/plugin_gui.rs` 参照)
- 全体表示は snapshot した z-order を尊重し、`DeferWindowPos` でまとめて表示する。
  個別に閉じた GUI は `user_hidden=true` になり、全体表示から除外する。

## 6. 設定永続化

settings.json に以下を保存する:

```json
{
  "vst3_enabled": false,
  "vst3_plugins": [
    {
      "path": "C:/Program Files/Common Files/VST3/Pro-Q 4.vst3",
      "bypass": false,
      "state": "<base64 of IComponent::getState() chunk>",
      "user_hidden": false,
      "window_rect": null
    }
  ],
  "vst3_gui_visible": false,
  "vst3_video_compact": false,
  "vst3_chain_slots": {
    "slots": [
      {
        "name": "Slot 1",
        "plugins": [ /* same schema as vst3_plugins */ ],
        "gui_visible": true,
        "video_compact": false
      }
    ]
  }
}
```

- `vst3_enabled`: 環境設定→VST3 プラグイン「VST3 プラグイン処理を有効にする」
- `vst3_plugins`: チェーン定義。配列順に音声を通す
- `state`: プラグイン側の現在状態 (= EQ カーブ等)。
  bridge から `query_state` コマンドで取得し、settings 保存時に更新。
  読み込み時に bridge へ `restore_state` で復元。
- `user_hidden`: ユーザーが個別に閉じた GUI を、全体表示で再表示しないためのフラグ。
  再生中 VST3 パネルでは個別 GUI の表示状態を Bitwig 風の小さなウィンドウ枠アイコンで示し、
  表示中はオレンジ、非表示は灰色で描画する。
- `window_rect`: plugin GUI の位置とサイズ
- `vst3_gui_visible`: 再生中パネルからの全体表示状態
- `vst3_video_compact`: 再生中パネルの動画フル / 右上 1/4 表示状態
- `vst3_chain_slots`: 再生中パネルから保存・読込する 10 個のチェーンスロット。
  各 slot は `vst3_plugins` と同じ plugin entry 配列を持つため、plugin bypass、
  `user_hidden`、state chunk、GUI 位置/サイズ、動画表示モードをまとめて復元できる。
  2026-05-03 follow-up: Save snapshots the current per-plugin GUI visibility
  into `user_hidden` when global VST GUI visibility is on. Load restores global
  GUI visibility only after all plugins have been added, and the manager panel
  uses `vst3_plugins` as disabled placeholder rows while the bridge rebuilds so
  the plugin list count stays stable.
  A later 2026-05-03 follow-up also feeds saved `gui_size` into the initial
  editor attach path as the host outer size. The bridge skips the immediate
  `getSize()` resize-back in that case, so plugins that do not persist editor
  size internally still reopen at the saved size.

bridge プロトコル拡張 (= Phase 0b に追加):

```
親 → bridge:
  {"cmd":"query_state"}
  {"cmd":"restore_state","state":"<base64>"}

bridge → 親:
  {"event":"state","state":"<base64>"}
```

## 7. 動画切替時の挙動

ユーザー要望: 動画再生のたびにプラグインを再初期化しない。

設計:
- bridge プロセスは **アプリ起動から終了まで生存**
- プラグインは settings の plugin_path に対して **1 度だけロード**
- 動画切替時:
  - decoder 再初期化 (既存処理) → 新 sample_rate / channels が決まる
  - **sample_rate が変わったら** bridge に `setup_processing` 再呼び出し
    (= プラグインの state は維持、IO config だけ再構成)
  - sample_rate が同じなら何もしない
  - `audio-pump` は新しい動画の最初の有効 audio frame でも `reset_plugins_sync`
    を実行する。bridge / plugin instance は動画間で永続するため、前動画の
    VST delay-line や shared-memory out ring の残りを次動画の冒頭へ漏らさない。
- プラグイン側の **lookahead / latency は state を維持**したまま継続

実装メモ: VST3 仕様では `IAudioProcessor::setupProcessing()` を再呼び出す前に
`setActive(false)` が必要。state は `getState/setState` で chunk 化して保存・復元
する (= setActive(false) でも内部状態は揺るがない仕様)。

## 8. 実装状況と残作業

| 項目 | 状態 |
| --- | --- |
| bridge exe 埋め込み + APPDATA 展開 | 完了 |
| 複数プラグインチェーン | 完了 |
| 環境設定→VST3 プラグインページ | 完了 |
| audio-pump からの `DspBridge::process_block` | 完了 |
| plugin GUI ホスト / z-order / user_hidden | 完了 |
| plugin state 保存 / 復元 | 完了 |
| PDC 最小補正 / safety limiter | 完了 |
| peak / gain reduction / OVER 表示 | 未実装 |
| SSL Meter Pro 等の右クリックメニュー即閉じ対策 | 未実装 |
| Buffering 中の raw_pending back-pressure 改善 | 未実装 |
| PDC 実機検証 (mIV Test Latency 等) | 検証待ち |

## 9. ライセンス対応

VST3 SDK 3.8.0 (MIT、2025-10-20 以降) を採用しているため、**追加の法務作業なし**。

- bridge プロセスソース (`crates/vst3-host/`): MIT (mIV と同じ)
- bridge ビルド成果物: MIT
- Steinberg の MIT 著作権表示を環境設定→ヘルプの「ソフトウェア情報」と
  `installer/readme.txt` に追記する (FFmpeg LGPL 通知と同じ場所)
- **VST トレードマーク (ロゴ) は使わない**。「VST3 プラグインをサポート」テキスト表記のみ。

## 10. 既知のリスク / 未確定事項

1. **商用プラグイン互換性**: Phase 0b で Pro-Q 4 の動作確認は済。LUFS 系
   (Youlean LM2 等) も同じ pattern で動くはずだが未検証。リリース前に
   1-2 個追加検証する
2. **プラグイン GUI を別ウィンドウで開いたまま動画フルスクリーンに入る挙動**:
   フルスクリーン中は topmost で手前に維持する。通常表示中は通常ウィンドウとして扱う。
3. **state 保存タイミング**: 設定保存時に bridge に query_state して同期
   取得すると UI スレッドがブロックするため、終了時 / VST3 OFF / chain rebuild 前に
   worker で snapshot する。
4. **DPI 異モニター跨ぎ**: Phase 0b で Per-Monitor v2 対応済だが、
   実機で 4K + FHD 跨ぎリサイズを再確認する

## 11. 配布物への影響

- `mimageviewer.exe` (launcher) のサイズ: 既存 ~365MB に bridge exe (~640KB) 追加 → ~366MB
- `mimageviewer-core.exe`: 既存に bridge exe を `include_bytes!` で内包
- 初回 VST3 enable 時 (= デフォルトでは展開されない) に
  `%APPDATA%\mimageviewer\vst3\mimageviewer-vst3-host.exe` を展開
- **bridge exe を埋め込む位置はメイン exe (= core)**。launcher は変更不要。

## 12. リリース前チェックリスト追加項目

CLAUDE.md の「リリース手順チェックリスト」に追記:

- [ ] `bash scripts/setup-vst3-sdk.sh` 完了済 (vendor/vst3sdk/)
- [ ] `cmake --build crates/vst3-host/build --config Release` 完了済
      (vendor/vst3-host/mimageviewer-vst3-host.exe が更新されている)
- [ ] Pro-Q 4 等の商用プラグインで音声経路を実機確認
- [ ] 動画再生中の VST3 パネルから全体表示 / 個別 GUI / bypass が操作できること
- [ ] settings.json に `vst3_plugins[].state` が保存され、再起動で復元されること
- [ ] safety limiter 有効時に過大出力が -1dBFS ceiling 以下に抑えられること
# 2026-05 chain bridge note

The original VST3 implementation used one bridge process per plugin. The
current direction is one bridge process per VST3 chain, so mIV remains isolated
from plugin crashes while all plugin editors and audio processing share one
bridge process. See [vst3-chain-bridge-redesign.md](vst3-chain-bridge-redesign.md)
for the migration plan and protocol shape.

Follow-up: the chain bridge keeps one audio/control process, but plugin editors
should no longer share one bridge GUI thread. Drag diagnostics after the
Bitwig-style owner/z-order refactor still show 250-300ms editor message gaps,
so the next GUI architecture is one STA editor/message-pump thread per plugin
slot inside the same bridge process. The detailed design lives in
[vst3-chain-bridge-redesign.md](vst3-chain-bridge-redesign.md).

## 2026-05 startup load policy

VST3 startup chain loading runs on a dedicated `vst3-startup-load` worker, not
inside the blocking startup-init path. This keeps image browsing responsive when
the user starts mIV only to view images or thumbnails.

If the user opens a video while the startup VST3 chain is still loading, the
fullscreen video open is deferred. The fullscreen viewport stays black and shows
the current VST3 startup progress text in the center. When the worker completes,
the deferred video open resumes automatically.

The worker still performs the same bridge enable and sequential `add_plugin`
calls as the previous startup path. Loading is intentionally sequential for now
because the Rust-side startup protocol sends one command at a time; the bridge's
per-slot GUI/load threads make future batch or parallel loading possible without
changing the fullscreen waiting behavior.

## 2026-05 fixed-size editor restore policy

Some fixed-size VST3 editors (`IPlugView::canResize() == false`) still call
`IPlugFrame::resizeView` during `attached()` or the first `onSize()` to request
their natural editor size. When mIV is restoring a user-saved outer editor shell
size, that plugin request must not resize the host-owned shell back to the
natural size. During the initial restored attach for fixed-size plugins, the
bridge forwards `onSize()` to the plugin but temporarily suppresses the host
HWND `SetWindowPos` side effect from `resizeView`. After the restored attach
settles, host resizing is re-enabled so explicit plugin/user resize paths keep
their existing behavior.

## 2026-05 native fullscreen HUD and scan progress

The native fullscreen video HUD only shows its `VST` top-bar button when
`settings.vst3_enabled` is true. When VST3 processing is disabled, fullscreen
playback does not advertise the VST3 playback panel because the button cannot
open a useful plugin GUI workspace.

The native DComp fullscreen path does not render the legacy egui fullscreen
viewport, so the playback VST3 panel is mirrored into the native overlay as a
self-drawn panel. The UI thread sends a small snapshot of bridge slot state,
chain-slot labels, latency/bypass flags, and compact-video mode to the presenter;
button clicks come back as native overlay events and are handled by the same
`DspBridge` actions used by the legacy playback panel. While native fullscreen
is active, GUI close/bypass signals are still pumped on the UI thread so plugin
window state stays synchronized with settings.

Compact video mode is applied by the native presenter itself: the DComp video
visual is transformed into the upper-right quarter while the black background
remains full-screen. Presenter teardown is signaled from the UI thread but the
thread join runs on a helper thread, so rapid video switching cannot freeze
`App::update` if Win32 window creation/destruction stalls.

The Preferences VST3 scanner reports probe progress over the scan worker
channel. The UI drains progress messages and shows the current `(done/total)`
count, plus the plugin currently being probed, while the bridge subprocess probe
continues off the UI thread.
