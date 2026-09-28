# EffeTune (Mixwright) 組み込み — サンプル版設計

状態: 設計第 5 版 (2026-09-27)。第 1 版 (REVISE、P1×5 / P2×4 / P3×1) と第 2 版 (REVISE、P1×4 / P2×2) への
Sol 設計レビューを反映。第 3 版への指摘 (REVISE、P1×1 / P2×3 / P3×1) を第 4 版で、第 4 版への指摘 (REVISE、P1×2 / P3×1) を第 5 版で反映。第 5 版は ACCEPT WITH CHANGES (P3×1、テストの記述) で、その修正を反映済み。
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
  - 実装事実 (2026-09-28): 利用不可・失敗・判定不能のツールチップには短い日本語の理由を出し、
    enum 名、host の詳細エラー、bundle の絶対パスは表示しない。診断詳細は log に残す。
- **GUI の owner はメインウィンドウに固定**。`DspBridge` に GUI owner 方針
  (`GuiOwnerPolicy::FixedMain` / 既存の `Auto`) を持たせ、EffeTune の bridge は `FixedMain`。
  attach と再表示のたびに main HWND が有効か確認し、無効なら表示しない (理由を log)。
  fullscreen owner は設定しない。TOPMOST にもしない。
- 既存のフルスクリーン関連の VST GUI 操作 (owner 付け替え、全 GUI 表示／非表示、TOPMOST、HUD の
  allowlist、フォーカスの受け渡し) は `self.dsp_bridge` のみを対象のまま変えない。
- **EffeTune の窓には、host の container が付けるタイトルバーの電源 (bypass) ボタンを出さない**。
  このボタンはスロットを bypass にするため、controller が `Running` のまま音が素通しになる別の持ち主を
  作ってしまう。ON/OFF の正本は Mixwright の状態 (画面内の全体バイパス) の 1 か所にする。
  ユーザー VST の窓の電源ボタンは従来どおり (実装時に判明、2026-09-27)。
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
- 取得のきっかけ: **ロード直後 (公開前、§3.2)、GUI を非表示にしたとき (× を含む)、終了時、
  リモート配信セッションの受け付け時 (§6)** の 4 つ。
  GUI 表示中の定期取得はしない (第 2 版の 2 秒ごとの取得は取りやめ)。ランプは窓を閉じたときに
  更新される。
- **応答の経路**: `query_state_concurrent` の完了とエラーは要求 ID 付きのイベントで返し、bridge の
  event pump が **既存の `event_rx` とは別の、要求 ID ごとの経路** へ振り分ける (同期呼び出しが
  捨てないように)。bridge の終了・タイムアウト時は未完了の要求をすべて「中断」で完了させる。
  host 側は、要求を受けた時点の loader を保持したまま GUI スレッドで実行し、loader の破棄は
  未完了の要求の完了 (または中断) の後に行う。
- **「中断」の区別**: GUI スレッドのキューで **まだ始まっていない** 取得は取り消して「中断」で完了
  させる。**既に `getState` を実行中** のものは取り消せないので、loader を生かしたまま完了を待つ。
  **呼び出し側の期限と host の健全性の見張りを分ける**:
  - 呼び出し側 (リモートの受け付け、終了時の待ち) は自分の期限が来たら **待つのをやめるだけ**。
    実行中の取得はそのまま完了させ、結果は採用規則 (generation) に従って扱う。host は終了させない。
  - host を終了させるのは **独立した見張り (watchdog)** だけ。1 回の取得の実行時間が固定の上限
    (host の既存 `kPluginStateTimeout` と同じ 5 秒) を超えたら、EffeTune 専用の host を終了させてから
    解放し、controller は `fail(HostLost)`。専用 host なのでユーザー VST には影響しない。
  - 例外: アプリ終了時は、終了の期限が来たら見張りを待たずに専用 host を終了させてよい
    (保存ファイルは前のまま)。
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
- **状態は配信セッションの受け付け時に取り直す**。窓を開いたまま設定を変えた直後に配信を始めても
  ローカルと同じ音にするため。配信 worker が controller の取得キュー (§5.2) へ取得を依頼し、
  **配信開始の共通の期限の中で** 待つ。取得に成功した状態でそのセッションの EffeTune を読み込む。
  取得が失敗・期限切れのときは、そのセッションでは EffeTune 段を外し、「EffeTune の最新の設定を
  取得できなかった」warning を出す (古い状態や既定状態で代わりに動かさない)。
  UI スレッドでは待たない。
- 最新状態が無い・ロード失敗・復元失敗のときも、そのセッションでは EffeTune 段を外し、
  既存の `ClocklessVstStatus` の warning で理由を区別して出す。
- **期限は 1 本**: 配信開始のロード予算 (既存: 残り時間から encoder 分を引き、上限 10 秒) を
  **絶対時刻の期限 1 つ**として、**ユーザーチェーンの読み込み → 状態の取り直し → EffeTune の読み込み**
  の順に共有する。既存のユーザーチェーンを先に読み込むので、EffeTune 側の待ちがユーザーチェーンの
  予算を奪わない。各段が自分の予算で期限を作り直さない。取り直しや EffeTune の段が期限切れでも、
  読み込めたユーザーチェーンは使う。
- 期限切れで待つのをやめても、ローカルの EffeTune は止めない (§5.2 の見張りとは別)。
- **段ごとの結果と失敗を分ける**: 既存の `ClocklessVstChain` の失敗フラグ・status は 1 つで、
  失敗時はユーザーチェーン前のサンプルに戻る。2 段にするため、段ごとに結果・失敗フラグを持ち、
  EffeTune 段の失敗ではユーザーチェーン後のサンプルに戻す。status は段ごとの状態を集約して
  「ユーザー VST は有効、EffeTune は失敗」のように表せる形にする。ユーザーチェーン段の既存の
  挙動 (失敗時の戻り先・warning) は変えない。
- リモートでも `strict_state` を使う。
- 実機で確認した配信時の追加条件 (2026-09-28): Mixwright は読み込み時の遅延 0 から、
  復元した効果の遅延 (例: 44.1 kHz で 128 samples) を再生開始後に通知する。このとき
  clockless audio の `audible_pts_secs` が後退し、AAC encoder の連続性チェックにより配信が止まる。
  `ClocklessAudioProcessor` は generation/seek ごとに直前の適用済み合計 latency
  (plugin + safety limiter、`audible_pts_secs` から引いた全量) を保持する。
  遅延増加分は出力先頭からサンプル単位で除去し、チャンクより長ければ残りを次へ持ち越す。
  遅延減少分は同数の無音を先頭へ挿入する。各チャンクの PTS/長さをサンプル数に合わせて更新し、
  `pdc_latency_secs_at_process` は実際に処理した PDC のまま残す。元の source 進捗と video PTS は
  変更せず、AAC encoder の連続性チェックも維持する。ユーザー VST の遅延変更と、
  EffeTune 単独段が失敗・上限超過で外れて limiter も停止する場合に同じ規則を適用する。
- `remote_clockless_audio_processing` の「`!vst3_enabled || plugins.is_empty()` なら VST なし」の
  早期 return を、EffeTune を含めた条件に直す。ユーザー VST が無効でも EffeTune だけで経路を作る。
- **既知の制約 (サンプル版)**:
  - 配信中に EffeTune を起動しても、その配信セッションは EffeTune なしのまま。次のセッションから適用。
  - 配信中に EffeTune の設定を変えても、そのセッションには反映しない (次のセッションの受け付け時に
    取り直す)。

## 7. bundle の配置 (サンプル版)

- `vendor/effetune-mixwright/EffeTune Mixwright.vst3` (gitignore 済み、v0.11.1、未署名)。
- `scripts/build-dev.ps1` が `target\dev-runtime\effetune\EffeTune Mixwright.vst3` へ
  ディレクトリごとコピーする (変更時のみ)。
- 実行時は **EffeTune モジュール自身の解決関数** で `<実行中 exe のディレクトリ>\effetune\EffeTune Mixwright.vst3`
  を探す。`native_assets` は使わない (ポータブル版の DLL 解決専用に `#[cfg(feature = "portable")]` で
  閉じられており、公開範囲を広げない。実装時に判明、2026-09-27)。通常版・ポータブル版で同じ規則。
  `current_exe()` が失敗したら `.` に逃がさず `Unavailable(BundleMissing(理由))`。
  bundle が無ければ `Unavailable(BundleMissing)`。
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
- pump の合成規則 (純関数に切り出す): 段ごとの適用、途中挿入の generation 変化の記録 (reset しない)、
  reset、EffeTune 失敗時はユーザーチェーン後のサンプルに戻る、失敗カウンタの独立、PDC の合算と
  上限、上限超過時に EffeTune 段だけ外れる、リミッター条件。
- 状態取得キュー: 古い generation の結果が新しい結果を上書きしない、失敗時に前のファイルが残る、
  一時ファイル名が衝突しない、終了時の期限。
- 厳格な復元: `open` と `add_plugin` の両方で、失敗がロード失敗になり、保存ファイルが変わらない。
- 非同期の状態取得: 要求 ID の振り分け、状態取得と GUI の問い合わせと終了の交錯で結果が捨てられない、
  host の終了または 5 秒の見張りの超過でだけ未完了の要求が中断として完了する。リモートの受け付けが
  期限切れになっても取得は完了まで続き、ローカルの EffeTune は `Running` のまま。
- 公開前の reset 失敗で公開されない。公開は reset 成功の後、pump は generation 変化で reset せず
  最初のブロックから処理する。シーク時の reset 失敗が `ProcessFailed` になる。
- 遅延: 適用前の確認で上限超過なら EffeTune 段を適用しない、`ReportOnly` では自動 bypass しない。
- リモート: ユーザー VST 無効 + EffeTune Running で経路が作られる、2 段の順序、ローカルと同じ合成規則、
  状態が無い場合に EffeTune 段が外れて warning が出る、配信開始後の起動はそのセッションに入らない。
  **窓を開いたまま設定を変えて配信を始めた場合に、取り直した状態が使われる**。取り直しの失敗・
  期限切れで EffeTune 段が外れて warning が出る。段ごとの失敗 (ユーザー段のみ・EffeTune 段のみ)
  と集約 status。共通の期限が取り直し・EffeTune の読み込みの前・途中で尽きた場合にユーザーチェーンは
  残る。**短いリモート予算で取り直しが期限切れになっても、ローカルは `Running` のまま音も途切れない**
  (host は見張りの上限までは終了しない)。
- 状態取得の中断: 未開始の取得は中断で完了、実行中の取得は loader を生かして待つ、呼び出し側の期限
  切れでは host を終了しない、見張りの上限超過でだけ専用 host が終了し `HostLost` になる、終了処理との交錯。
- settings: 新フィールドが `overwrite_non_preferences_from` で保持される。
- GUI signal: `vst3_enabled=false` でも EffeTune の `GuiUserHidden` が処理される。
- 共有メモリ: 同時に 2 本 `open_audio_pipe` しても名前が衝突しない。既存オブジェクトを開いた場合
  (`ERROR_ALREADY_EXISTS`) は失敗として扱う。

実 host の `getState` を GUI スレッド内で任意の位置に停止させる決定的な試験は保留する。
Mixwright v0.11.1 の取得は短時間で完了し、リポジトリには制御可能な試験用 VST3 plugin がない。
Rust の取得キューでは未開始／実行中の終了交錯を fake executor で試験し、host 側の loader 寿命と
5 秒 watchdog の実負荷時の振る舞いは下記の利用者負荷試験で確認する。

実行時の確認は利用者の実機で行う (GUI が開くか、音が処理されるか、ビジュアライザー、空パイプラインの
遅延と透過性、初回の既定パイプライン)。

**負荷試験 (利用者の実機、必須)**: 再生中に EffeTune の窓でエフェクトの追加・削除・パラメータ操作を
続け、窓の開閉を繰り返す。音が途切れないこと、EffeTune が `Failed` にならないこと、log の
状態取得時間と pump の失敗回数を確認する。問題が出たら取得方式か取得のきっかけを見直す。

## 9. 付随修正

### 実装事実 (2026-09-27)

- v0.11.1 の実 bundle を host で開き `getState` した JSON は `pipelineA: []`、未初期化の
  `pipelineB: null` だった。`EffectiveState` は codec のこの配列形を読み、選択中の未初期化 B は
  空と判定する。実 host の strict `open` / `add_plugin` テストで取得形も検査する。
- controller の失敗処理は音声スロットを先に空にし、host の disable / drop を専用 worker に渡す。
  GUI attach エラーは専用 bridge から型付きで controller に渡し、同じ `fail()` を通す。
- GUI の位置・サイズは非表示時に `self.settings` のメモリ値だけ更新し、既存の full-save 経路で
  `settings.db` に保存する。rect 専用 writer / 部分 upsert は置かない。ツールバーからの GUI 非表示は
  controller の host worker に送り、送信結果を後続 frame で適用する。host の × では既に非表示になった
  surface を記録し、UI thread から二重の hide コマンドを送らない。
- 取得 worker は host 応答を期限で「host 異常」と判定しない。リモート受付・終了の期限は
  呼び出し側だけに掛け、host の終了・watchdog は別の結果として扱う。終了の公開 gate は
  `Open` → `Committing` / `Expired` の遷移だけを mutex で決め、rename は mutex 外で行う。
  期限前に認めた rename が進行中なら終了側は待たず、その結果を許容する。stdout EOF は
  `HostExited` として別経路で渡し、取得 worker が子プロセス終了を待って exit code を読む。
  Running 中の予期しない host 終了も専用 monitor worker が exit code を読んで controller に報告する。
  watchdog の終了コードは `0xEFFEC001` とし、stderr の受信順によらず Rust 側が識別する。
- リモートの状態再取得は worker がユーザーチェーン準備を終えた後に投入する。reset も共有の
  絶対期限の残りだけ待つ。pump の失敗報告は音声スロットで世代ごとに 1 回へ集約する。
- bundle 解決は `src/effetune/mod.rs::resolve_bundle_from_exe` に置いた。通常版と portable 版で
  `current_exe` の親から同じ相対パスを使い、`native_assets` には触れていない。
- GUI タイトルバーの電源ボタンは bridge ごとの `show_editor_bypass_button` で切り替える。
  既定値は表示、EffeTune 専用 bridge だけ非表示。
- EffeTune の GUI signal pump は UI frame で行うが、focus、resize、resize session に伴う
  host コマンドは専用 `effetune-host-control` worker に送る。host monitor と GUI attach worker は
  結果が届いた時に egui の repaint を要求し、アイドル中の失敗も controller の `fail()` に渡す。
  host の `GuiUserHidden` / `GuiBypassToggle` も event pump でキューへ入れた直後に同じ
  notifier で repaint を要求する。notifier は EffeTune 専用 bridge の生成時だけ渡す。
- ローカル音声の引き渡し型は `AudioDspChain { user, effetune }`。`EffetuneAudioSlot` は
  `Mutex` をブロックごとに 1 回読んで世代と bridge を取得する。
- 終了時の EffeTune 最終取得は `ExitCaptureFence` を先に作り、既存 VST3 スナップショットと
  並行して進め、その後に開始時点からの 2 秒期限の残りだけ待つ。
- 共有 bridge の「同時に可聴な pump は 1 本」の前提は、メディア open 前に parked media と
  `fs_cache` の他の Video/Audio player を閉じる経路、および `AudioOutput::drop` が停止を通知して
  cpal stream を pause/drop する経路で確認した。旧 pump の終了待ちは別スレッドに逃がすため
  teardown 中に処理スレッドが重なり得るが、旧 stream は可聴ではない。

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

### 10.1 ポータブル版 (利用者決定 2026-09-28)

- ポータブル版は **ユーザー VST も音響調整 (EffeTune) も無効のまま** (vst3-host.exe を同梱しない現状を維持)。
- 理由: Mixwright は mIV の data_dir に関係なく `%APPDATA%` に書く (上流 v0.11.1 で確認:
  プリセット = `%APPDATA%\effetune\` か `%APPDATA%\Frieve\EffeTunePlugin\`、config.json も同所、
  WebView の保存領域 = CHOC `getUserDataFolder()` により `%APPDATA%\<ホスト exe 名>\`)。
  ユーザー VST も APPDATA に書くものが多い。いずれもポータブル版の「APPDATA を使わない」前提と合わない。
- 参考: 過去のポータブル版の誤検知の原因は未署名の vst3-host.exe そのもので、フォルダ走査 (core の
  `src/video/dsp/scanner.rs`) ではなかった。
- Mixwright のパイプライン プリセットは DAW やデスクトップ版 EffeTune と共有される (作者の設計)。
  WebView の保存領域はホスト exe ごとに分かれる。配布版の privacy.html に APPDATA への保存を追記する。

## 11. 試験版の引き渡し時点の記録 (2026-09-28)

- 独立コードレビュー (実装者と別の Sol、5 回) の最終判定: SHIP-FOR-USER-TEST。
- 未解決 P3: `Bridge::open_audio_pipe()` を同じ `Bridge` で開き直すと、前の共有メモリとイベントの
  ハンドルを解放しない (このブランチ以前からの既存問題。追加した strict-open テストが踏む)。
- `cargo test -p mimageviewer --lib` は通常実行で 3 回成功 (最終 9288 passed / 0 failed / 48 ignored)。
  実装者の 1 回だけ `STATUS_ACCESS_VIOLATION` で異常終了し、テスト名・ダンプが無いため原因は未特定。
  cdb で包んだ 2 回では再発しなかった (ただしデバッガ下は約 17 倍遅く、時間依存のテストが 40〜46 件
  失敗する。失敗は EffeTune と無関係なモジュールにも広く分布)。
- 実機でまだ誰も確認していないこと: Mixwright の GUI 表示、音声処理、ビジュアライザー、空パイプラインの
  遅延と透過性、初回の既定パイプライン、再生中の編集・開閉の負荷試験 (§8)。

## 12. 第 6 版: リモート配信はローカルの bridge を共有する (2026-09-28、利用者合意)

### 12.1 背景 (実機の観測とコード)

- 利用者が実機で観測: リモート配信の最初の約 5 秒は EQ が効かず、途中から効いた。
- 原因 (上流ソース `src/plugin/plugin_processor.cpp` v0.11.1 で確認): Mixwright の `setState` は
  反映待ちの印を立てて内蔵 WebView に再読込を指示するだけで、音声処理への反映は WebView の
  `rebuildPipeline` 要求 (~1-2 秒後) で行われる。反映完了をホストへ知らせる手段は無い
  (遅延が変わるときの `kLatencyChanged` だけ)。リモートはセッションごとに新しい bridge で状態を
  復元し、実時間より速く先読みするため、反映待ちの間に数秒分が EQ なしで処理される。
- ユーザー VST も、リモートではセッションごとに `settings.vst3_plugins` (保存済み状態) から読み直して
  いる (`src/remote_ipc/ui.rs` `remote_clockless_audio_processing`)。ローカルの GUI で変えた直後の
  設定は保存されるまでリモートに反映されない (本ブランチ以前からの挙動)。
- 旧設計 (`docs/vst3-integration.md` §2、「ローカル再生の plugin state と高速 feed の timeline を
  混在させないため、streaming session 専用 DspBridge を持つ」) の懸念は、リモートとローカルが同時に
  音を出さないこと (`docs/web-remote-plan.md` §2.2: 操作権の移動でローカルのメディアを一時停止) と、
  受け渡し時の reset で満たせる。

### 12.2 決定

リモート配信は、配信用の bridge を作らず、**ローカルの bridge (ユーザー VST チェーン `App::dsp_bridge`、
EffeTune の controller が持つ bridge) をそのまま使う**。

- 読み込みと状態の復元が配信開始時に起きないので、反映待ちが無い。
- ローカルの窓で変えた設定が配信中もそのまま反映される (EffeTune・ユーザー VST とも)。
- EffeTune の窓のビジュアライザーに配信中の音が流れる。ただし先読みのため、端末で聞こえる音より
  先に進む (明記する制約)。
- 配信中の host プロセスはローカルの 2 つだけになる。

### 12.3 不変条件

- **I1 (処理の持ち主は 1 つ)**: 共有 bridge の `process_block` を呼ぶのは、常にローカルの音声 pump か
  リモート配信の処理のどちらか一方。持ち主は型付きの lease (`Local` / `Remote { session, generation }`)
  で表し、単一の所有者が切り替える。候補: 既存の remote session owner の遷移 (取得・解放・奪取) に
  合わせて切り替える。持ち主でない側は `process_block` を呼ばない。
- **I2 (受け渡しで reset)**: 持ち主が変わるとき (Local→Remote、Remote→Local、リモートの generation
  切替・seek) は、新しい持ち主の最初のブロックの前に共有 bridge を reset し、成否を確認する
  (`Result` を返す reset)。失敗はその bridge の既存の失敗経路へ (ユーザーチェーン = 既存の disable、
  EffeTune = controller の `fail()`)。
- **I3 (ローカルの一時停止中の扱い)**: 操作権がリモートにある間、ローカルの pump が持ち主でない状態で
  音声を処理して、処理済みキューに「素通しの音」を貯めないこと。候補: 持ち主でない間は pump が
  処理を進めず待つ (キャンセル・終了では待ちを解く)。ローカルへ戻った最初のブロックは I2 の reset 後。
  **ここは既存コードの一時停止の実態 (pump が停止中もどこまで処理するか) を確認して決める。**
- **I4 (失敗の持ち主は変えない)**: リモート側で起きた失敗も、共有 bridge の既存の持ち主が処理する。
  ユーザーチェーンの連続失敗はローカルでも無効化される (旧設計では配信用 bridge だけが止まった)。
  EffeTune は controller の `fail()` を通す。
- **I5 (遅延)**: 共有しても遅延は配信中に変わり得る (窓での操作)。第 5 版の「適用した遅延の差分だけを
  補正する」処理はそのまま使う。
- **I6 (サンプルレート)**: 共有 bridge は起動時の出力デバイスのレートで準備されている。リモートの
  音声処理のレートは、共有 bridge が準備されたレートに合わせる (配信時に `default_output_sample_rate()`
  を読み直さない)。レートが食い違う場合の扱いを明記する (候補: その段を適用せず warning)。

### 12.4 起動直後・未準備のとき

- ユーザー VST の起動時読み込み、または EffeTune のロードが終わっていない間に配信が始まった場合は、
  配信開始の期限の中で決着を待ち、間に合わなければその段なしで始めて warning を出す
  (既存の「起動時ロード中はメディアのオープンを遅らせる」ゲートと同じ決着判定を使う)。
- ユーザー VST が無効、EffeTune が Running でない場合は、その段は無い (従来どおり)。

### 12.5 撤去するもの

- リモート用のセッション bridge (`ClocklessAudioProcessing::with_remote_vst3` が作る `DspBridge::new()`、
  EffeTune の配信用 bridge)。
- 配信受け付け時の EffeTune 状態の取り直し (§6 の第 4〜5 版の仕組み) と、ロード予算の共有期限。
- リモート側の plugin 読み込みに関する warning の一部 (ロード失敗・時間切れ)。処理中の失敗の warning は残す。

### 12.6 テスト

- lease: 持ち主でない側が `process_block` を呼ばない。Local→Remote→Local で各持ち主の最初のブロックの
  前に reset。reset 失敗が既存の失敗経路に入る。
- ローカルの一時停止中に操作権がリモートへ移っても、処理済みキューに素通しの音が貯まらない。
- 配信の generation 切替・seek で reset。
- 起動時ロード中に配信を始めた場合の待ちと、間に合わない場合の warning。
- 共有 bridge のレートとリモートの処理レートの一致、食い違い時の扱い。
- リモート側の連続失敗がユーザーチェーン・EffeTune の既存の失敗経路に入る。
- 第 5 版の遅延補正テストは残す。

### 12.7 文書

- `docs/vst3-integration.md` §2 の「streaming session 専用 DspBridge」の記述を更新し、設計変更の理由を残す。
- 本書 §6 (第 4〜5 版のリモート 2 段構成) は第 6 版で置き換えた旨を記す。
