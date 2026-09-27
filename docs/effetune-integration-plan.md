# EffeTune (Mixwright) 組み込み — サンプル版設計

状態: 設計第 3 版 (2026-09-27)。第 1 版 (REVISE、P1×5 / P2×4 / P3×1) と第 2 版 (REVISE、P1×4 / P2×2) への
Sol 設計レビューを反映。
サンプル版 (試験用) の範囲を定める。配布版で決めることは §10。

## 0. 目的と決定済み事項

EffeTune (Frieve-A、MIT) の VST3 版 **EffeTune Mixwright** (v0.11.1、WebView2 UI、AVX2/FMA 必須) を
mIV の音声経路へ組み込み、エフェクト処理とビジュアライザーを使えるようにする。

利用者と合意済みの方針 (2026-09-27):

- 既存の VST3 設定画面・VST3 パネル・チェーンプリセットには **出さない**。
- 位置は **ユーザー VST3 チェーンの後ろに固定** (手動ブースト・安全リミッターの前)。
- **専用の DspBridge (別プロセス)** で動かす。既存チェーンの隠しスロットにはしない。
- ウィンドウは VST3 パネルとは別扱いで、**メイン画面のツールバー**から開く。
- **リモート配信にも適用する** (「リモートだと聴こえ方が違う」を防ぐ)。
- **「一度起動したら終了まで常に経由」**。起動するのは次のどちらかのとき:
  - 起動時: 保存済みの状態が「有効」(§5.3) または「読めない」のとき
  - 実行中: 利用者がツールバーのボタンを押したとき
  起動後は、エフェクトを全部オフにしても終了までは素通しで経由し続ける。
  経路へ出入りするのは「起動」の 1 回と、§3.4 の失敗時の 1 回だけ。
- ON/OFF の正本は **Mixwright の状態 1 か所**。mIV 側に独立した ON/OFF 設定を持たない。

## 1. 既存コードの前提 (コード確認済み、Sol レビューで確認)

- ローカルの `DspBridge` は App のフィールド (`src/app.rs` `dsp_bridge`)。リモート配信は 1 セッション
  につき別の `DspBridge` を作る (`ClocklessAudioProcessing::with_remote_vst3`)。複数共存は既にある。
- `DspBridge::enable()` は host exe を展開するだけで、プロセスは最初の `add_plugin` で起動する。
- プラグインのパスは `.vst3` bundle ディレクトリのままでよい (SDK の `Module::create` が解決)。
- 状態は base64 で運ばれる不透明なバイト列。Mixwright v0.11.1 の `getState` は JSON をそのまま書く
  (上流ソース `src/plugin/plugin_processor.cpp` の `getState` → `StateCodec::encode`。
  mIV の checkout には無いので、fixture は §8 の方法で揃える)。
- **host の `query_state` は bridge の音声スレッド上の「state op fence」で実行され、GUI スレッドへ
  `invoke_sync_for(kPluginStateTimeout)` で最大 5 秒待つ** (`crates/vst3-host/src/main.cpp` の
  audio loop、`plugin_loader.cpp` `PluginLoader::query_state`)。その間 mIV 側の
  `process_audio_blocking` は 100ms で失敗し、連続 3 回で bridge が無効化される
  (`src/video/audio.rs` の `VST3_CONSECUTIVE_FAILURE_DISABLE`)。**再生中に状態を取ると音が止まり得る。**
- **host の `open` (bridge の最初のプラグイン) と `add_plugin` は、初期状態の decode / restore に
  失敗しても既定状態で続行し、成功として返す** (`main.cpp` の open / add_plugin 処理)。
- host の GUI スレッドの非同期タスクは、エディタと WebView のメッセージを配る同じメッセージループで
  実行される (`plugin_loader.cpp`)。bridge のイベントは 1 本の受信口に入り、同期呼び出しは想定外の
  イベントを捨てる (`bridge.rs`)。
- `reset_plugins_sync` は最大 2 秒待ち、期限切れでも `()` を返して処理を続ける。reset の ack は
  1 本の受信口で、同じ bridge を 2 つの pump が同時に reset すると互いの ID を捨て得る。
- `DspBridge::total_latency_samples` は上限超過時に自分でスロットを自動 bypass する
  (上限超えのスロット、またはチェーン超過時は最大のスロット)。
- **Mixwright v0.11.1 の `getState` の並行安全性 (上流ソースで確認、タグ v0.11.1
  `src/plugin/plugin_processor.cpp`)**: `getState` は `commitPendingControllerWritesIfAudioIdle(true)`
  を呼ぶが、音声処理中 (component active かつ audio idle でない) なら mutex を取る前に return する。
  音声コールバック (`process`) は `processingResourcesMutex_` も `stateMutex_` も取らない
  (同ファイルのコメント「The audio callback never takes this mutex」と `process` 本体で確認)。
  したがって UI スレッドからの `getState` が `process` を止める経路はソース上は無い。
  **実機での負荷試験 (§8) で裏付ける。**
- 音声 pump は `Option<Arc<DspBridge>>` を open 時に受け取り pump へ move する (open 時点の固定値)。
  リミッター条件は `vst_chain_active || pre_limiter_gain > 1.0 || normalize_boost_active`。
  `DspBridge::total_latency_samples` は自分のスロットだけで 2 秒上限を見る。
- GUI の owner は `current_gui_owner_hwnd()` が「fullscreen owner → カーソル下／前面の mIV ウィンドウ
  → main」の順で選ぶ。`set_main_hwnd` だけではメインウィンドウになる保証がない。
- 起動時ロード完了時の `poll_vst3_startup_load` は、ユーザー VST の pending だけを見て
  `resume_deferred_vst3_media_open` を呼ぶ。
- 共有メモリ・イベント名 `miv-vst-shm-{pid}-{µs}` は同一マイクロ秒で衝突し得る。
  `CreateFileMappingW` は既存オブジェクトを開いても成功扱いになっている。

## 2. 構成

```
decode → normalize → [ユーザー VST3 チェーン: dsp_bridge] → [EffeTune: 専用 bridge]
       → 手動ブースト → 安全リミッター → 出力
```

## 3. 所有者と状態遷移

### 3.1 `EffetuneController` (新規、`src/effetune/`)

App が 1 つ持つ。EffeTune の実行状態・経路への公開・状態保存の **唯一の所有者**。
pump・リモート・GUI はここへ報告するだけで、EffeTune の bridge を直接 disable しない。

```rust
enum EffetuneRuntime {
    Unavailable(UnavailableReason), // bundle 無し / AVX2・FMA 無し / VST3 非対応環境
    Idle,                           // 起動していない
    Loading { origin: LoadOrigin, open_gui_when_ready: bool }, // Startup | UserButton
    Running { generation: u64 },    // 経路に公開済み
    Failed(EffetuneFailure),        // この実行中は再起動しない。音声は経路から外れて素通し
}
enum EffetuneFailure {
    LoadFailed(String),
    RestoreFailed(String),      // 保存済み状態を適用できなかった (§5.4)
    ProcessFailed(String),      // pump からの連続失敗報告
    LatencyExceeded { total_secs: f64 },
    HostLost(String),
}
```

- `Running` から `Idle` へは戻らない。`Failed` からも戻らない (次回起動で再試行)。
- すべての失敗は `EffetuneController::fail(EffetuneFailure)` の 1 か所を通る。そこで
  (1) 経路スロットを空にする (§3.2)、(2) bridge を disable する、(3) 理由を保持してツールチップに出す。
- ユーザー VST 側の既存の disable / rebuild / 環境設定の経路は `self.dsp_bridge` だけを対象にしたまま
  変えない。

### 3.2 経路への公開 (途中挿入に対応)

- controller が `EffetuneAudioSlot` (共有スロット) を持つ。中身は
  `Option<(generation: u64, Arc<DspBridge>)>`。`ArcSwapOption` 相当で、pump は **ブロックごとに 1 回**
  読む (lock を取らない)。既に依存にある型を使えるならそれを使い、無ければ `Mutex` を 1 ブロック 1 回
  取るだけの単純な実装でよい (試験用。pump スレッドは cpal の RT スレッドではない)。
- 公開は、ロード worker が次を **すべて成功させた後** に、generation を 1 増やして行う。
  ロード途中の bridge は公開しない。
  1. ロードと状態の厳格な復元 (§5.4)
  2. 既存と同じ warm-up
  3. **reset を行い、成功を確認する** (まだどの pump も使っていないので競合しない)
  4. 初回の状態取得 (§5.2)。成功した状態を controller のメモリ上の「最新状態」にする
  どれかが失敗したら公開せず `fail()`。
- `AudioDspChain { user: Option<Arc<DspBridge>>, effetune: Arc<EffetuneAudioSlot> }` を
  `VideoPlayer::open*` → `audio::start` → `run_pump` へ渡す (スロットは全 player で共有の 1 つ)。
  動画・音楽・キャッシュ済み player の再利用・動画→音声モードのすべてが同じスロットを見る。
  `RemoteHeadless` の open は従来どおり `None` 相当 (ローカル pump を持たない)。

### 3.3 pump の処理

ユーザーチェーンの処理の **後** に EffeTune の段を置く。

- 各ブロックでスロットを読む。generation の変化で pump は reset しない (公開前に reset 済み、§3.2)。
  pump は generation を PDC の記録に使うだけ。
- シーク時の EffeTune の reset は、結果を返す版 (`Result`) を使う。期限切れ・失敗は controller へ
  `ProcessFailed` として報告し、そのシーク後のブロックには EffeTune 段を適用しない。
  複数の player が同時に同じ bridge を reset する競合は、既存のユーザーチェーンの共有 bridge と
  同じ前提 (同時に音声を出す pump は 1 つ) に従う。この前提をコードで確認し、成り立たなければ
  実装前に報告する。
- EffeTune 段の入力は **ユーザーチェーン通過後のサンプル**。EffeTune 段が失敗したブロックは
  **ユーザーチェーン通過後のサンプル** をそのまま出す (normalize 直後に戻さない)。
- 失敗カウンタは EffeTune 段専用に持つ (閾値はユーザーチェーンと同じ 3 回 / 回復 5 回)。
  閾値に達したら pump は **controller へ `ProcessFailed` を報告するだけ** (mpsc)。bridge の
  disable は controller が行う。報告からスロットが空になるまでの間も、失敗ブロックは上記の
  fallback で流す。
- 出力バッファは EffeTune 段専用に 1 本持って使い回す。
- 終了時の `flush_silence` を EffeTune 段にも行う。
- **遅延 (PDC)**: チャンクごとに「実際に適用した段」の遅延だけを合算して記録する。
  - プラグイン遅延の合計 = ユーザーチェーン (適用時) + EffeTune (適用時)。
  - **2 秒上限はプラグイン遅延の合計に掛ける** (リミッター・time stretch の遅延は上限の外)。
    優先順位は **ユーザーチェーン優先**。ユーザーチェーンは既存どおり自分の中で上限を守る。
  - EffeTune 段を **適用する前に** 「ユーザーチェーンの遅延 + EffeTune の現在の遅延」を確認し、
    上限を超えるなら適用せず controller へ `LatencyExceeded` を報告する。
    値を丸めると音と映像がずれるため、丸めない。遅延が処理の後で変わった場合は、次のブロックの
    適用前の確認で捉える。
  - EffeTune の bridge では `total_latency_samples` の **自動 bypass を使わない** (bridge に
    latency 方針 `AutoBypass` (既存) / `ReportOnly` を持たせ、EffeTune は `ReportOnly`)。
    自動 bypass で「Running なのに素通し」になる別の持ち主を作らないため。
  - 途中挿入で遅延が増えた場合は、既存の per-chunk PDC 公開と decoder の追従に任せる。
- 安全リミッター: EffeTune 段を適用したチャンクでは常にリミッターを通す (決定的な規則)。

### 3.4 起動時の読み込みとメディアオープンの遅延

- 起動時、保存ファイル (§5.1) を worker で読み、`Effective` または `Unparseable` なら
  `Loading { origin: Startup }` で worker ロードを始める。`Inert` / ファイル無しなら `Idle`。
- 起動時ロード中は、既存の VST3 と同じくメディアのオープンを遅延させる。
- **遅延の解除は「ユーザー VST の起動時ロード」と「EffeTune の起動時ロード」の両方が決着した後**
  (成功・失敗・worker 切断のいずれも「決着」)。既存の `resume_deferred_vst3_media_open` を呼ぶ
  箇所を「両方決着したか」を見る 1 つの判定に集約し、どちらの完了順でも 1 回だけ解除する。
  解除時に mounted / active / parked の viewer context を辿る既存の振る舞いは変えない。
- ボタン操作による途中起動 (`origin: UserButton`) ではメディアのオープンを遅延させない。
- `vst3_enabled` とは独立。VST3 が無効でも EffeTune は動く。

## 4. ウィンドウ

- ツールバーに `ToolbarSectionId::EffeTune` を追加 (既存 `FolderTree` の toggle に倣う)。既定で表示。
  - ランプ (selectable の active) = `Running` かつ 最新の判定が `Effective`。
  - クリック: `Idle` → `Loading { UserButton, open_gui_when_ready: true }` /
    `Running` → GUI の表示・非表示 / `Loading` → 何もしない / `Unavailable`・`Failed` → 無効表示。
  - ツールチップに状態と理由 (Failed の理由、判定が古い可能性、Unparseable の理由) を出す。
- **GUI の owner はメインウィンドウに固定**。`DspBridge` に GUI owner 方針
  (`GuiOwnerPolicy::FixedMain` / 既存の `Auto`) を持たせ、EffeTune の bridge は `FixedMain`。
  attach と再表示のたびに main HWND が有効か確認し、無効なら表示しない (理由を log)。
  fullscreen owner は設定しない。TOPMOST にもしない。
- 既存のフルスクリーン関連の VST GUI 操作 (owner 付け替え、全 GUI 表示／非表示、TOPMOST、HUD の
  allowlist、フォーカスの受け渡し) は `self.dsp_bridge` のみを対象のまま変えない。
- `pump_gui_signals` を EffeTune の bridge についても毎フレーム呼ぶ。`vst3_enabled` や
  VST マネージャ表示の分岐の外で呼ぶ。× で閉じたら非表示にするだけで経路からは外さない。
- **既知の制約 (サンプル版、意図的に触らない)**:
  - 同じモニターでフルスクリーンにすると、EffeTune のウィンドウは裏に隠れる。
  - フルスクリーンのフォーカス受け渡し (`vst3_gui_visible` を見る処理) は EffeTune の窓を知らない。
    フルスクリーン中に EffeTune 窓を操作した後、フルスクリーンを 1 回目にクリックしたときの扱いが
    ユーザー VST の場合と違い得る。
  - これらは fullscreen / viewport の経路に触れるため、配布版で detached リワークの手続き
    (`docs/detached-rework-plan.md` §2) を踏んで決める。サンプル版では detached 述語・viewport 経路に
    触れない。

## 5. 状態の取得・保存・判定

### 5.1 保存先

- Mixwright の状態: `data_dir/effetune/mixwright-state.json` (base64 を decode した生バイト)。
  settings.db に載せない (状態は IR 等で MB 級になり得るが、`settings_kv` は保存のたびに全行を
  書き直すため)。
- ウィンドウ位置・サイズ: settings に `effetune_gui_pos: Option<(i32,i32)>` /
  `effetune_gui_size: Option<(u32,u32)>`。`overwrite_non_preferences_from` でもコピーする。
- 未リリース機能なのでマイグレーション不要。

### 5.2 取得 (音声を止めない)

- host に **新コマンド `query_state_concurrent`** を追加する。GUI スレッドへ **非同期に** 投げて
  `getState` を呼び、結果を `plugin_state` イベント (要求 ID 付き) で返す。音声スレッドは待たない。
  VST3 の規約では `IComponent::getState` は UI スレッドから呼ばれ、処理と並行し得る (DAW の再生中
  保存と同じ)。**このコマンドは EffeTune の bridge だけが使う。** ユーザー VST の既存 `query_state`
  (音声スレッドの fence) は変えない。
- 取得のきっかけ: **ロード直後 (公開前、§3.2)、GUI を非表示にしたとき (× を含む)、終了時** の 3 つ。
  GUI 表示中の定期取得はしない (第 2 版の 2 秒ごとの取得は取りやめ)。ランプは窓を閉じたときに
  更新される。
- **応答の経路**: `query_state_concurrent` の完了とエラーは要求 ID 付きのイベントで返し、bridge の
  event pump が **既存の `event_rx` とは別の、要求 ID ごとの経路** へ振り分ける (同期呼び出しが
  捨てないように)。bridge の終了・タイムアウト時は未完了の要求をすべて「中断」で完了させる。
  host 側は、要求を受けた時点の loader を保持したまま GUI スレッドで実行し、loader の破棄は
  未完了の要求の完了 (または中断) の後に行う。
- **取得は controller の 1 本の直列キューで行う**。各要求に generation を振り、
  新しい generation の結果だけを採用する。同時に in-flight は 1 本。
- 取得・decode・判定・書き込みのどれかが失敗したら、**前のファイルと前の判定を保つ**
  (失敗を `Inert` とみなさない)。失敗は log に残す。
- 書き込みは同じディレクトリの一意な一時ファイルへ書いてから置き換える
  (既存の `data_dir` の置換パターンに倣う)。UI スレッドでは書かない。
- 終了時の順序: 新しいポーリングを止める → 最終取得を 1 回要求 → 期限付き (既存の終了時 VST
  スナップショットと同程度) で書き込み完了を待つ → 期限切れなら前のファイルのまま終了 (log)。
  既存の `on_exit_inner` の VST スナップショットとは独立に行い、互いを待たせない。
- 試験のため、取得した状態の判定結果・サイズ・取得にかかった時間を毎回 log に出す
  (実機の fixture 収集と負荷試験を兼ねる)。

### 5.3 `EffectiveState`

```rust
enum EffectiveState {
    Effective,           // 音が変わり得る設定がある
    Inert,               // 空、または全体バイパス
    Unparseable(String), // 形式が読めない・未知の版
}
```

- 対象は `formatVersion == 1` の形 (Mixwright v0.11.1 の `StateCodec`)。他の版は `Unparseable`。
- `masterBypass == true` → `Inert`。
- `currentPipeline` ("A"/"B") のパイプラインを見る。section プラグインの `enabled` が false の区間は
  無効。区間外または有効区間で `enabled == true` のプラグインが 1 つでもあれば `Effective`。
  (上流 `aggregatePipelineLatency` と同じ有効判定)
- それ以外は `Inert`。
- ランプは最後に取得できた判定を表す。窓を開いている間の変更は、窓を閉じるまで反映しない (ツールチップに明記)。
- `Unparseable` は起動条件では起動する側に倒す。ランプは点けない。ファイルは書き換えない
  (取得に成功した新しい状態でだけ置き換える)。

### 5.4 状態の厳格な復元

- host の **`open` (最初のプラグイン) と `add_plugin` の両方** に `strict_state: bool` を追加する。
  true のとき、初期状態の base64 decode または `setState` に失敗したら **ロード失敗として返す**
  (既定状態で続行しない)。EffeTune の bridge は常に true。ユーザー VST は従来どおり false。
  失敗は `Running` の公開前に決まる。
- 失敗したら controller は `Failed(RestoreFailed)`。**保存ファイルは書き換えない**
  (既定状態で上書きして利用者の設定を失わない)。
- host のプロトコル版 (`PROTOCOL_VERSION`、Rust `src/video/dsp/bridge.rs` と C++ `protocol.h`) を
  1 上げる。`vendor/vst3-host/mimageviewer-vst3-host.exe` を再ビルドする。

## 6. リモート配信

- 配信セッションを作る時点で EffeTune が `Running` なら、そのセッションに適用する。
- **リモートもローカルと同じ 2 段構成にする**: セッション用のユーザーチェーン bridge とは別に、
  セッション用の EffeTune bridge を 1 つ作り、既存の `ClocklessVstProcessor` を 2 段つなぐ。
  段の合成規則 (適用前の遅延確認、ユーザーチェーン優先の上限、失敗時はユーザーチェーン後の
  サンプルに戻る、`ReportOnly`) は **ローカルと同じ純関数** を使う。
  - 第 2 版の「同じ host に足す」案は取りやめ。同じ host では自動 bypass がユーザー側のプラグインを
    外し得て、ローカルと優先順位が食い違うため。
  - 代償: 配信中の host プロセスは最大 4 (ローカル 2 + リモート 2)。Mixwright が落ちてもリモートの
    ユーザーチェーンは巻き込まれない。
- 状態は controller のメモリ上の **最新状態** を使う (§3.2 の手順 4 により、`Running` なら必ずある)。
  `prepare_once` の中でローカルへ問い合わせない。EffeTune のロードは既存のロード予算の中で行う。
  最新状態が無い・ロード失敗・復元失敗のときは、そのセッションでは EffeTune 段を外し、
  既存の `ClocklessVstStatus` の warning で「EffeTune の状態が無い」「EffeTune のロードに失敗」を
  区別して出す。既定状態で代わりに動かさない。
- リモートでも `strict_state` を使う。
- `remote_clockless_audio_processing` の「`!vst3_enabled || plugins.is_empty()` なら VST なし」の
  早期 return を、EffeTune を含めた条件に直す。ユーザー VST が無効でも EffeTune だけで経路を作る。
- **既知の制約 (サンプル版)**:
  - 配信中に EffeTune を起動しても、その配信セッションは EffeTune なしのまま。次のセッションから適用。
  - 配信中に EffeTune の設定を変えても、次のセッションまで反映しない (最新状態は窓を閉じたときに
    更新される)。

## 7. bundle の配置 (サンプル版)

- `vendor/effetune-mixwright/EffeTune Mixwright.vst3` (gitignore 済み、v0.11.1、未署名)。
- `scripts/build-dev.ps1` が `target\dev-runtime\effetune\EffeTune Mixwright.vst3` へ
  ディレクトリごとコピーする (変更時のみ)。
- 実行時は `native_assets::bundled_root().join("effetune").join("EffeTune Mixwright.vst3")`。
  無ければ `Unavailable(BundleMissing)`。
- `is_x86_feature_detected!("avx2") && is_x86_feature_detected!("fma")` が偽なら
  `Unavailable(CpuUnsupported)`。
- release / portable / launcher への埋め込みはしない。build.rs の必須チェックにも入れない。

## 8. テスト

単体・状態遷移:
- `EffectiveState`: 空 / 全体バイパス / A と B / section 無効区間 / 壊れた JSON / 未知の版。
  fixture は Mixwright v0.11.1 の上流ソース (`src/bridge/state_codec.cpp` の encode) の出力形から作り、
  出典 (タグと関数) をテストに書く。実機の状態は §5.2 の log で後から集める。
- controller の遷移: Idle→Loading→Running、Running から戻らない、各 `EffetuneFailure` が
  `fail()` の 1 か所を通りスロットが空になる、Unavailable ではクリックで何も起きない、
  起動時条件 (Effective / Unparseable で起動、Inert・ファイルなしで起動しない)。
- 起動時ゲート: 完了順 2 通り、ユーザー VST 無効、どちらかが失敗・worker 切断、遅延中の動画／音声の
  オープンが 1 回だけ再開される。
- pump の合成規則 (純関数に切り出す): 段ごとの適用、途中挿入の generation 変化で最初のブロック前に
  reset、EffeTune 失敗時はユーザーチェーン後のサンプルに戻る、失敗カウンタの独立、PDC の合算と
  上限、上限超過時に EffeTune 段だけ外れる、リミッター条件。
- 状態取得キュー: 古い generation の結果が新しい結果を上書きしない、失敗時に前のファイルが残る、
  一時ファイル名が衝突しない、終了時の期限。
- 厳格な復元: `open` と `add_plugin` の両方で、失敗がロード失敗になり、保存ファイルが変わらない。
- 非同期の状態取得: 要求 ID の振り分け、状態取得と GUI の問い合わせと終了の交錯で結果が捨てられない、
  終了・タイムアウトで未完了の要求が中断として完了する。
- 公開前の reset 失敗で公開されない。シーク時の reset 失敗が `ProcessFailed` になる。
- 遅延: 適用前の確認で上限超過なら EffeTune 段を適用しない、`ReportOnly` では自動 bypass しない。
- リモート: ユーザー VST 無効 + EffeTune Running で経路が作られる、2 段の順序、ローカルと同じ合成規則、
  状態が無い場合に EffeTune 段が外れて warning が出る、配信開始後の起動はそのセッションに入らない。
- settings: 新フィールドが `overwrite_non_preferences_from` で保持される。
- GUI signal: `vst3_enabled=false` でも EffeTune の `GuiUserHidden` が処理される。
- 共有メモリ: 同時に 2 本 `open_audio_pipe` しても名前が衝突しない。既存オブジェクトを開いた場合
  (`ERROR_ALREADY_EXISTS`) は失敗として扱う。

実行時の確認は利用者の実機で行う (GUI が開くか、音が処理されるか、ビジュアライザー、空パイプラインの
遅延と透過性、初回の既定パイプライン)。

**負荷試験 (利用者の実機、必須)**: 再生中に EffeTune の窓でエフェクトの追加・削除・パラメータ操作を
続け、窓の開閉を繰り返す。音が途切れないこと、EffeTune が `Failed` にならないこと、log の
状態取得時間と pump の失敗回数を確認する。問題が出たら取得方式か取得のきっかけを見直す。

## 9. 付随修正

- 共有メモリ・イベント名に process 内の atomic 連番を足す。`CreateFileMappingW` /
  `CreateEventW` で `ERROR_ALREADY_EXISTS` を失敗として扱う (bridge.rs)。
- host の `query_state_concurrent` と `strict_state` (§5.2、§5.4)。
- `DspBridge` の方針フィールド: GUI owner (`Auto` / `FixedMain`) と latency (`AutoBypass` / `ReportOnly`)。
  既存の bridge は既定値で従来どおり動く。

## 10. サンプル版の範囲外 (配布版で決める)

- release / portable / インストーラへの同梱方法、Mixwright の署名、THIRD-PARTY-NOTICES の転載、商標注記
- マニュアル・製品ページ・privacy (Mixwright の WebView データの保存先が mIV の data_dir の外になる点)
- フルスクリーン中の EffeTune ウィンドウの扱い・フォーカス受け渡し (detached リワークの手続きが必要)
- 配信中の起動・設定変更のリモート反映
- 作者への連絡
