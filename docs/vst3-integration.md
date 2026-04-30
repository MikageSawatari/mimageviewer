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
- **プラグイン GUI は常に最前面** (`WS_EX_TOPMOST`)。動画フルスクリーン中も裏に
  隠れず、動画を見ながら EQ 調整 / LUFS 確認ができる。

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
│   - WS_EX_TOPMOST で常に最前面 (動画フルスクリーンに隠れない)
│   - 各スロットが個別の HWND を持つ。V キーで全スロット一斉トグル
└─ Settings (settings.json):
    - vst3_enabled: bool (default false)
    - vst3_plugins: Vec<Vst3PluginEntry>  ← チェーン定義
    -   .path: String
    -   .bypass: bool
    -   .state: Option<Base64<...>>  (= IComponent::getState、将来用)
    - vst3_gui_visible: bool  (V キートグル状態)

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
| `src/settings.rs` | VST3 設定 4 項目を追加 | 拡張 |
| `src/ui_dialogs/preferences.rs` | 動画タブに VST3 セクション追加 | 拡張 |
| `src/ui_dialogs/vst3_manager.rs` | VST3 プラグイン管理ウィンドウ | **新規** |
| `src/video/audio.rs` | pump thread に DspBridge 経由処理を挿入 | 拡張 |
| `src/app.rs` | DspBridge を Option<Arc<DspBridge>> として保持 + V キーハンドラ | 拡張 |
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
   AudioBuffer に push
                → cpal RT → 出力 (変更なし)
```

設計判断:

- **plugin process は audio-pump thread で実行する**。cpal RT スレッドではない。
  bridge IPC roundtrip (~1-2ms) を AudioBuffer の depth (1.5秒) で吸収する。
- **enable=false なら処理ゼロオーバーヘッド**。frame をそのまま push。
- **bridge unload 中も音声は流れる**。ロード前は plugin pass-through (= 何もしない)。
- **block size は decoder のフレームサイズに依存しない**。bridge 側で variable
  block size を扱える (Phase 0b で実装済)。

bridge IPC のレイテンシ実測 (Phase 0b):
- `set_event` + `wait_event` 1 周: 1-2ms (Windows context switch)
- 1.5 秒 buffer に対して十分小さい。realtime 維持可能。

## 5. プラグイン GUI ホスティング

要件 (ユーザー要望):
- アプリ起動中ずっとプラグイン GUI を表示しておける
- **V キーで全プラグイン GUI 一斉表示/非表示**トグル
- プラグイン管理ウィンドウで個別表示/非表示
- v0.9.0 では 1 個までだが UI は将来の複数対応を見据える

実装:
- bridge プロセス側で `IPlugView::attached(hwnd)` で親ウィンドウに接続
  (Phase 0b で動作確認済)
- ホスト側 (Rust) は `winit` ではなく **CreateWindowExW で独立 HWND** を作成
  (eframe のメインビューポートとは別ウィンドウ)
- `SetParent` はクロスプロセスでも動作する (= bridge が hwnd 値を受け取って
  自プロセス内で `IPlugView::attached(hwnd)` を呼ぶ)
- リサイズ追従 / DPI 対応は Phase 0b で完成済 (`tester/src/plugin_gui.rs` 参照)
- V キーは:
  - メインビューポートの input handler で検知
  - フルスクリーンビューポートの input handler でも検知 (両方対応)
  - 全 plugin GUI HWND に対して `ShowWindow(SW_SHOW/SW_HIDE)`

## 6. 設定永続化

settings.json に以下 4 項目を追加:

```json
{
  "vst3_enabled": false,
  "vst3_plugin_path": "C:/Program Files/Common Files/VST3/Pro-Q 4.vst3",
  "vst3_plugin_state": "<base64 of IComponent::getState() chunk>",
  "vst3_gui_visible": true
}
```

- `vst3_enabled`: 環境設定の動画タブ「VST3 プラグイン処理を有効にする」
- `vst3_plugin_path`: 管理ウィンドウで選択した最後のプラグイン
- `vst3_plugin_state`: プラグイン側の現在状態 (= EQ カーブ等)。
  bridge から `query_state` コマンドで取得し、settings 保存時に更新。
  読み込み時に bridge へ `restore_state` で復元。
- `vst3_gui_visible`: V キー / 管理ウィンドウのトグル状態

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
- プラグイン側の **lookahead / latency は state を維持**したまま継続

実装メモ: VST3 仕様では `IAudioProcessor::setupProcessing()` を再呼び出す前に
`setActive(false)` が必要。state は `getState/setState` で chunk 化して保存・復元
する (= setActive(false) でも内部状態は揺るがない仕様)。

## 8. Phase 別実装順 (v0.9.0 に向けた残作業)

| Phase | 内容 | 規模見積もり |
| --- | --- | --- |
| **A1** | bridge exe を `include_bytes!` で埋め込み + APPDATA 展開 | 0.5 日 |
| **A2** | `src/video/dsp/` モジュールを tester から移植 (build.rs / 単体テスト含む) | 1 日 |
| **A3** | Settings 4 項目 + 環境設定 UI (動画タブに VST3 セクション) | 0.5 日 |
| **A4** | audio-pump thread に bridge process 挿入 | 0.5 日 |
| **A5** | プラグイン GUI ホスト + V キー + 管理ウィンドウ | 1 日 |
| **A6** | state 保存/復元 (`query_state` / `restore_state` 追加) | 0.5 日 |
| **A7** | docs (architecture-overview / async-architecture / ui-responsiveness / README) 更新 | 0.5 日 |
| **A8** | 回帰テスト (perf_smoke / ui_snapshot) + 実機確認 | 0.5 日 |

合計 ~5 日。v0.9.0 に同梱可能。

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
   フルスクリーンウィンドウが foreground を取ると plugin GUI が裏に隠れる
   可能性。`SetWindowPos(HWND_TOPMOST)` で常に手前にするオプションを
   管理ウィンドウに追加する (= デフォルト OFF)
3. **state 保存タイミング**: 設定保存時に bridge に query_state して同期
   取得すると UI スレッドがブロックする。**worker 化** (CLAUDE.md
   ui-responsiveness §4) してから書き戻す
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
- [ ] V キー一斉トグル動作確認
- [ ] settings.json に vst3_plugin_state が保存され、再起動で復元されること
