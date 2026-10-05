# EffeTune (Mixwright) 組み込み — サンプル版設計

状態: 設計第 5 版 (2026-09-27)。第 1 版 (REVISE、P1×5 / P2×4 / P3×1) と第 2 版 (REVISE、P1×4 / P2×2) への
Sol 設計レビューを反映。第 3 版への指摘 (REVISE、P1×1 / P2×3 / P3×1) を第 4 版で、第 4 版への指摘 (REVISE、P1×2 / P3×1) を第 5 版で反映。第 5 版は ACCEPT WITH CHANGES (P3×1、テストの記述) で、その修正を反映済み。
サンプル版 (試験用) の設計記録と、v4.3.0 配布版の決定をまとめる。配布の同梱・署名・通知と
ポータブル版の範囲は §10 に確定。残る対象外事項も同節に記録する。

## 0. 目的と決定済み事項

EffeTune (Frieve-A、MIT) の VST3 版 **EffeTune Mixwright** (v0.12.0、WebView2 UI、AVX2/FMA 必須) を
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
  当初のリミッター条件は `vst_chain_active || pre_limiter_gain > 1.0 || normalize_boost_active`。
  2026-10-04 の follow-up 決定で最終段は常時有効に変更（§13、公開済み v4.3.0 には未収録）。
  `DspBridge::total_latency_samples` は自分のスロットだけで 2 秒上限を見る。
- GUI の owner は `current_gui_owner_hwnd()` が「fullscreen owner → カーソル下／前面の mIV ウィンドウ
  → main」の順で選ぶ。`set_main_hwnd` だけではメインウィンドウになる保証がない。
- 起動時ロード完了時の `poll_vst3_startup_load` は、ユーザー VST の pending だけを見て
  `resume_deferred_vst3_media_open` を呼ぶ。
- 共有メモリ・イベント名 `miv-vst-shm-{pid}-{µs}` は同一マイクロ秒で衝突し得る。
  `CreateFileMappingW` は既存オブジェクトを開いても成功扱いになっている。

## 2. 構成

```
decode → normalize → [ユーザー VST3 チェーン: dsp_bridge] → [任意の EffeTune 前段リミッター]
       → [EffeTune: 専用 bridge]
       → 手動ブースト → 常時有効の最終安全リミッター → 出力
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
    §13 の前段 limiter を通した場合はその実遅延も PDC へ加算する。
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
- 最終安全リミッター: エフェクト・音量・normalize gain に関係なく常時適用する。
  約 5ms の実 lookahead 遅延を常に PDC へ含める（2026-10-04 follow-up、公開済み v4.3.0 には未収録）。

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
  - クリック: `Idle` → `Loading { UserButton, open_gui_when_ready: Some(ShowPermit) }` /
    `Running` → 非表示なら表示・アクティブ化、表示中で手前なら非表示、背面なら手前へ出してアクティブ化 /
    `Loading` → 何もしない / `Unavailable`・`Failed` → 無効表示。ただしR2の
    `Unavailable(BundlePreparationFailed)`だけはクリックでworker再解決→ロードを試す。
  - ツールチップに状態と理由 (Failed の理由、判定が古い可能性、Unparseable の理由) を出す。
  - 実装事実 (2026-09-28): 利用不可・失敗・判定不能のツールチップには短い日本語の理由を出し、
    enum 名、host の詳細エラー、bundle の絶対パスは表示しない。診断詳細は log に残す。
- **GUI は owner のない tool window** (`GuiOwnerPolicy::Unowned`、利用者決定 2026-09-30)。
  `FixedMain` の owned popup は Windows の規則でメインより常に手前になり、メインをクリックしても
  裏へ回せないため変更した。owner HWND は常に 0、タスクバーのボタンは増やさない。ユーザー VST は
  既存の `Auto`。fullscreen owner は設定しない。TOPMOST にもしない。
  main HWND は owner と分けた参照として DPI と最小化の確認にだけ使う。初期位置・保存済み rect は
  従来どおり。host の `set_owner` とドラッグ終了による owner 復元で再所有させない。
- **「手前」の判定は foreground window のルートから owner chain を辿ると、当該 editor に到達するか**
  の 1 つにする。editor 自身とその popup を含め、別のユーザー VST は含めない。
  メインへの primary click は `WM_MOUSEACTIVATE` → `WM_LBUTTONDOWN` の組で、メインが手前になる前の
  foreground を採取する。次のクリックで置き換え、キーボードによるボタン操作は現在の foreground を使う。
  タッチ／ペンも `WM_POINTERACTIVATE` と primary `WM_POINTERDOWN`、legacy `WM_TOUCH` の primary down
  から同じ入力状態を通す。非 client・別ボタン・取消し・非アクティブ化で未完了の activation を破棄する。
  明示的な表示／背面からの復帰だけで foreground を許可し、host GUI thread でアクティブ化する。
- **mIV による一時非表示は host の `GuiVisibility` が単独で所有**する。表示希望と非表示理由の集合
  (`Minimized` / `RemoteSession`) を持ち、理由がすべて解除され、表示希望が残るときだけ非アクティブで戻す。
  最小化や Remote による hide で `user_hidden`、設定保存、state capture、音声の実行状態を変えない。
  §1.312: `effetune_keep_visible_when_minimized`（既定 false）で `Minimized` だけを解除できる。
  起動保存値と Preferences OK を既存 gate に公開し、変更時だけ worker の既存 reconcile を通知する。
  最小化中も即時反映し、表示希望のない窓は開かない。`RemoteSession` は常に優先する。
  tray-only `SW_HIDE` は OS の最小化ではなく、両設定とも既存どおり窓を残す。主窓が iconic なら設定に従う。
  最小化はメインの `WM_SIZE` で共有 atomic の連番を更新し、worker に通知する。
  WndProc は bridge を参照せず、`DspBridge.inner` のロック・列挙・IPC は worker 側だけで行う。
  実際の表示直前にも現在の `IsIconic(main)` を確認する。
  アプリの非アクティブ化では窓を隠したり手前へ戻したりしない。mIV 終了時は既存の bridge teardown で消す。
- **Remote が操作権を持つ間は窓を隠す** (利用者決定 2026-10-01)。音の look-ahead による反映遅延と
  端末より先行するビジュアライザーで PC 上の編集が紛らわしいため。
  正本は `remote_session_blocks_local_control()` (取得中・所有中・drain 中を含む、音声トラック計画 §9B)。
  「音響調整」は無効表示し、ツールチップで理由と復帰を説明する。取得時に未完了の自動 GUI open 意図を取消す。
  初回 attach は非表示で行い、専用 host-control worker が完了時の Remote 状態を確認してから表示する。
  Remote または最小化中に attach が完了した窓は隠したままで、解除時に新しく開かない。
  worker はイベントに載った古い Remote 値を再生せず、現在の `SessionHandle` の phase を読む。
  UI 側の直前通知値は重複通知の抑制だけに使い、最小化から復帰したときに UI frame より前でも
  新しい Remote 取得を検出する。handle の登録・切り離しと同じ境界で参照を更新する。
  未表示の open 要求は `ShowPermit` (native 最小化イベント連番と既存 Remote 取得連番) を利用者の
  open 要求時 (Loading 中の要求を含む) に採取し、attach 前後で照合する。
  attach 中に最小化／Remote の開始と終了が両方済んだ場合も取消し、
  まだ表示していなかった窓を「復帰」として開かない。時間窓や独自 Remote revision は使わない。
  **配送後の取消しも host の GUI thread で確定する** (2026-10-01 review fix1)。
  専用共有 mapping は native 最小化連番と Remote の取得連番・phase、および最小化中の表示設定の read-only projection
  を運ぶ（§1.312 で version 2 / 40 byte、設定 atomic は offset 32）。Remote は `SessionStateMachine` の遷移・参照の登録／切り離しと同じロック内で公開し、
  worker に復帰判定を通知する。worker は source を weak に参照し、通知 sender の循環で残留しない。
  `set_gui_visibility_checked` は request ID と発行時の連番を運び、GUI thread が共有値と最小化を
  表示・アクティブ化の直前に照合する。取消しは未表示の窓の表示希望を作らず、既に表示希望のある
  窓の希望は保存する。判定後に始まる最小化／Remote は通常の一時非表示として扱う。
  `Shown` / `Hidden` / `Cancelled` / `Error` を返し、Rust は pipe 書き込みだけで表示を公開しない。
  ACK と native close は 1 本の FIFO signal を通り、`pump_gui_signals` が Rust の表示情報を更新する。
  5 秒の ACK 待ち・mapping 作成／検証失敗は GUI failure として扱い、未検査の表示へ代替しない。
  背面からの明示的な前面化も同じ検査と GUI task を通る。自動復帰は従来どおり非アクティブ。
  単純化として GUI 操作を既存 worker に直列化し、初回 hidden attach と理由集合を使う。
  モーダル化や editor の破棄・再生成は、再生・ビジュアライザー・設定画面の通常操作を妨げるため採らない。
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

> **第 6〜9 版で置き換え (§12)**: リモートはローカルの bridge を共有する。以下は第 4〜5 版の記録。

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

## 7. bundle の配置 (サンプル版の記録、配布版は §10.2)

- `vendor/effetune-mixwright/EffeTune Mixwright.vst3` (gitignore 済み、v0.12.0、未署名)。
- `scripts/build-dev.ps1` が `target\dev-runtime\effetune\EffeTune Mixwright.vst3` へ
  ディレクトリごとコピーする (変更時のみ)。
- 解決は **EffeTuneモジュール自身** が所有する。配布版はlauncherが検証して渡すgenerationを
  `<exe_dir>/effetune/<hash12>-<generation>/EffeTune Mixwright.vst3` として一度だけ解決し、
  controllerの生存中は固定する。launcher経由でない通常版はcurrent pointerから同じ規則で選ぶ。
  修復不能時の専用envはUnavailableと詳細logへ反映し、別世代へ黙ってfallbackしない。
- build-devのpointer不在時だけ従来の `<exe_dir>/effetune/EffeTune Mixwright.vst3` を使う。
  portableはこの従来経路を維持しbundleを同梱しない。`native_assets` のportable専用公開範囲は広げない。
  `current_exe()`失敗時も `.` へfallbackしない。bundle不在はUnavailable(BundleMissing)。
- `is_x86_feature_detected!("avx2") && is_x86_feature_detected!("fma")` が偽なら
  `Unavailable(CpuUnsupported)`。
- サンプル版では release / portable / launcher への埋め込みを行わず、build.rs の必須チェックにも
  入れなかった。v4.3.0 配布版の launcher 対応は §10.2。

## 8. テスト

単体・状態遷移:
- `EffectiveState`: 空 / 全体バイパス / A と B / section 無効区間 / 壊れた JSON / 未知の版。
  fixture は Mixwright v0.11.1 の上流ソース (`src/bridge/state_codec.cpp` の encode) の出力形から作り、
  出典 (タグと関数) をテストに書く。実機の状態は §5.2 の log で後から集める。
- controller の遷移: Idle→Loading→Running、Running から戻らない、各 `EffetuneFailure` が
  `fail()` の 1 か所を通りスロットが空になる、Unavailableでは準備失敗だけworkerで再試行し、
  拒否された同世代を使わず別の公開世代を固定する（他のUnavailableはクリック不可）、
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
- **(第 6〜9 版で置き換え。bridge 共有で配信受け付け時の状態の取り直しは無くなった。現行のテストは §12.9)**
  リモート: ユーザー VST 無効 + EffeTune Running で経路が作られる、2 段の順序、ローカルと同じ合成規則、
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
- owner / ボタン: 実際の EffeTune bridge は main / fullscreen HWND があっても owner 0、ユーザー VST は
  `Auto` のまま。非表示・表示中かつ手前・表示中かつ背面の 3 分岐、editor の popup owner chain、
  マウス／タッチの activation と次の press、キーボードによる古い snapshot の破棄を単体で固定する。
- 一時非表示: `crates/vst3-host/tests/gui_visibility.cpp` の compile-time テストで、理由を両順序で
  解除して最後だけ復帰する、非表示だった窓は開かない、同じ理由の重複通知は集合として扱う、
  owned の従来動作を固定する。Rust 側では Remote 取得〜drain 完了まで表示要求を拒否し、
  UI 通知前の正本の取得と、Loading / attach 中に完了した最小化・Remote 区間でも open が取消されることを確認する。
- 配送後: Rust の最終検査後〜GUI task 実行前の最小化／Remote と、その区間が既に終了した場合を
  host の permit 判定で検査する。取消し後の復帰で未表示の窓が開かない、取消した raise で前の
  表示希望を消さない、ACK／native close の FIFO 順序、request ID の照合と EOF／shutdown、
  native size の通知経路が bridge のロックを取得しないことを回帰テストで固定する。
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
- (第 6〜9 版で置き換え: 配信受け付け時の状態再取得は撤去、§12.8) リモートの状態再取得は worker が
  ユーザーチェーン準備を終えた後に投入する。reset も共有の絶対期限の残りだけ待つ。pump の失敗報告は音声スロットで世代ごとに 1 回へ集約する。
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
- `DspBridge` の方針フィールド: GUI owner (`Auto` / `FixedMain` / `Unowned`) と latency (`AutoBypass` / `ReportOnly`)。
  既存の bridge は既定値で従来どおり動く。

## 10. 決定済みの配布方針と残る対象外事項

- 最小化中もビジュアライザーを残す設定は当初の配布作業では対象外。公開後の §1.312 で実装、既定は一緒に隠す。§14 を参照。
- **Windows Sandbox では音響調整 (と EPUB 変換) が動かない — 対処しない (2026-10-04 利用者判断)。** Sandbox では
  `HKLM\SOFTWARE\WOW6432Node\Microsoft\EdgeUpdate\ClientState\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}\EBWebView` が
  存在しない旧版フォルダ (152.0.4191.66) を指し、実フォルダはホストと共有の 153 / 154。EdgeUpdate が無効なのでずれが直らない。
  WebView2 の既定の探し方が 0x80070002 で失敗し、EffeTune は PENDING → TIMEOUT、EPUB は「WebView2 Runtime が見つかりません」になる。
  EBWebView だけを直すと成功、pv だけでは失敗 (サブPCの診断ツール wv2diag による実験、記録は サブPC `C:\miv-sandbox\wv2-diag\out\`)。
  同じ症状は [WebView2Feedback #5697](https://github.com/MicrosoftEdge/WebView2Feedback/issues/5697) に未解決で報告されている。
  普通の PC では更新の中断などの例外時だけで、WebView2 を使うアプリ全体が失敗し、ランタイムの修復で直る (Web 調査、一部コミュニティ回答)。
  予備の探し方 (EdgeWebView\Application の最新版を `browserExecutableFolder` / `WEBVIEW2_BROWSER_EXECUTABLE_FOLDER` で明示) は
  Evergreen を想定した使い方ではなく、更新通知が来なくなるため採らない。壊れた環境向けの案内表示も追加しない (利用者判断)。

- v4.3.0 の同梱・署名・ライセンス通知は §10.2 に確定。商標注記の追加要否は別途確認する。
- マニュアル・製品ページ・privacy には、Mixwright の WebView データの保存先が mIV の data_dir の
  外になる点を記載済み。installer/readme.txt も共有データと Remote の残存フォルダを明記する。
- フルスクリーン中の EffeTune ウィンドウの扱い・フォーカス受け渡し (detached リワークの手続きが必要)
- 配信中の起動のリモート反映 (設定変更は第 6〜9 版の bridge 共有で配信中も反映される。配信中の
  起動は次の配信から、§12.4)
- 作者への連絡

### 10.1 ポータブル版 (利用者決定 2026-09-28、UI方針追記 2026-10-01)

- ポータブル版は **ユーザー VST も音響調整 (EffeTune) も無効のまま** (vst3-host.exe を同梱しない現状を維持)。
- **利用者決定 2026-10-01**: portable は EffeTune のツールバーボタンとカスタマイズ候補を表示しない。
  v4.3.0 の「重要な変更点」からも、同ボタン追加の必読告知と EffeTune 新機能の紹介を除く。
  通常版の表示・告知は維持し、Remote 接続・音声トラック選択・長さ表示の告知は両版に残す。
- 除外条件は `portable` build flavor とする。通常版で bundle が見つからない場合は配布／展開の
  不具合として扱い、ボタンと「必要なファイルが見つかりません」の表示を維持する。
  portable の保存済みツールバーレイアウトから EffeTune 項目を削除せず、表示と編集用の投影だけで除外する。
- 理由: Mixwright は mIV の data_dir に関係なく `%APPDATA%` に書く (上流 v0.11.1 で確認:
  プリセット = `%APPDATA%\effetune\` か `%APPDATA%\Frieve\EffeTunePlugin\`、config.json も同所、
  WebView の保存領域 = CHOC `getUserDataFolder()` により `%APPDATA%\<ホスト exe 名>\`)。
  ユーザー VST も APPDATA に書くものが多い。いずれもポータブル版の「APPDATA を使わない」前提と合わない。
- 参考: 過去のポータブル版の誤検知の原因は未署名の vst3-host.exe そのもので、フォルダ走査 (core の
  `src/video/dsp/scanner.rs`) ではなかった。
- Mixwright のパイプライン プリセットは DAW やデスクトップ版 EffeTune と共有される (作者の設計)。
  WebView の保存領域はホスト exe ごとに分かれる。配布版の privacy.html に APPDATA への保存を記載済み。

### 10.2 v4.3.0 の配布同梱 (2026-10-01)

- **単体exe版とインストーラ版に Mixwright v0.12.0 の bundle 全体を同梱する**。インストーラは
  launcher をインストールする。portable は §10.1 の決定どおり bundle と VST host を同梱しない。
- launcher の build.rs が `vendor/effetune-mixwright/` (または `MIMV_EFFETUNE_DIR` で指定した
  staging) の VERSION と bundle を必須検証し、相対パス順に全ファイルを列挙して埋め込む。
  v0.12.0 の bundle は 424 ファイル、32,320,639 bytes (約30.82 MiB)。bundle 外の VERSION を含む
  入力は計425ファイル。VERSION、ファイル一覧、サイズと SHA-256 から bundle の同一性を記録する。
  無ければ取得・配置の復旧手順付きで build を停止する。
- **R1修正 (2026-10-02)**: 承認済み `manifest.sha256` にVERSION＋424ファイルの一覧とSHA-256を
  固定し、vendor原本の欠落・追加・改変を署名前／埋め込み前に拒否する。署名stageも全非PEが一致、
  PEはchecksum／証明書以外が原本と一致し、指定発行元の有効署名があることを要求する。
- 起動時は `runtime/<version>/effetune/<hash12>-<generation>/EffeTune Mixwright.vst3/`
  へ新しい世代を構築する。全ファイルのhashと一覧を検証してから、小さな `effetune/current`
  pointerだけをatomicに更新する。公開処理は公開済みtreeを移動・削除しない。
  旧世代のcleanupは起動後のcore背景workerで行う（下記2026-10-05追補）。修復は別世代の公開でありin-placeの置換ではない。
  正常stampの一覧・サイズ・更新時刻・作成時刻が一致するときは全量再hashもwrite lockも不要。
  metadataを保持したままの内容改変は通常起動での検出対象外。書込不能／publisher busyなどで
  repairできなくてもcoreは起動する。失敗理由と拒否世代を専用envからUnavailable UI／ログへ伝え、古いtreeを
  黙って代用しない。成功時のgenerationもlauncherがenvで指定し、coreがそのpathを一度解決・固定する。
- `build-dist.ps1` が呼ぶ `build-release.ps1 -Sign` は vendor 原本を変更せず target の staging に
  bundle をコピーし、拡張子が
  `.vst3` の plugin PE を含む全 PE を **launcher の埋め込み前に署名**する。
  launcher build 時に `MIMV_EFFETUNE_DIR` を staging に向け、PE dependency gate も同じ
  staging の bundle を明示的な検査入力にする。
- VST hostは `data_dir/vst3/hosts/<host+CRT SHA256>/mimageviewer-vst3-host.exe` へ展開し、
  CRT4本は非検索subdir `vcrt/` に置く。System32の全4本が存在・版数読取可能で各DLLの
  file versionが同梱セット以上なら全4本System32、他は全4本同梱。一度選び依存順でpreloadし、混在させない。
  旧host／旧CRTを触らず、成功のみcacheして抽出失敗の再試行を許す。basenameは維持するので
  WebViewの `%APPDATA%/mimageviewer-vst3-host.exe/` 保存先は変わらない。
- SDK Windows hosting moduleはMIT原文を保持したtracked copyでUTF-8→UTF-16／wide APIを使う。
  IPCのWindows backslash／Unicode escapeもdecodeする。pluginのANSI APIに影響する
  activeCodePage manifestは使わない。stateはopaque IPC bytesで、hostにstate/preset path I/Oはない。
- **R3補正 (2026-10-02、利用者決定)**: System32は4本それぞれが同梱版以上の場合だけ
  選ぶ。1本でも古い／読取不能なら全4本同梱へ統一する。直接起動用vendor hostも必須CRTセットを
  保持し、CMakeが公式正本を `vendor/vst3-host/vcrt/` に毎回配置する。必須条件の緩和はしない。
- **R2簡素化 (2026-10-02、利用者決定)**: CRTはDLLごとの組み合わせを作らず一組で選ぶ。
  publisher競合は最大60秒のOS lock待機で起動を直列化する（try_lock＋sleepループは使わない）。
  timeout／修復失敗時は既存Loading／pending_loadに再解決とロードを統合し、新しい待機stateを追加しない。
  準備失敗だけ音響調整ボタンを再試行可能にし、workerでcurrent pointerを再読する。整合性検査が拒否した
  同じ世代は不可、別の公開済み世代だけを固定する。pointer不在時のdev fallbackはretryには使わない。
  generationはcontent hash先頭12桁＋nonceに短縮するが、stampはfull manifestを比較する。最深file pathの
  UTF-16長が260以上なら公開せず、UIにパスが長すぎる理由を示す。SDK directory checkのNotFound以外のerrorも
  単体DLL pathへfallbackせず報告し、Win32にはnative backslash wide pathを渡す。
  hostはCMakeで現trackedソースhash markerを埋め、署名前／core埋込前／bare cargo releaseのgateで照合する。
  APPDATAから旧hostをコピーするbuild fallbackは削除。旧世代cleanupは下記追補および
  [バックログ§1.316](next-release-backlog.md#1316-effetune-公開済み旧世代の-best-effort-cleanup--2026-10-02)を参照。
- 3種類の通知全文を `third_party/effetune-mixwright/v0.12.0/` に原文のまま追跡し、about の
  EffeTune Mixwright / Steinberg VST3 SDK (MIT) 一覧と折り畳み全文表示に使用する。
  `.gitattributes` の `third_party/effetune-mixwright/** -text` で Windows の `core.autocrlf=true`
  でもバイト列を保持する。Gitの保存内容もLF。vendor が存在するテストでは VERSION と通知全文の完全一致を確認する。portable は EffeTune
  一覧・通知の埋め込みを行わない。bundle 内の元通知も省略せず配布する。
  2026-10-03 の公開前レビュー対応で、JSZip 内の lie / immediate / setImmediate と
  pako の zlib 由来コードの原文を `supplemental/NOTICES.txt` として第4の折り畳み通知に
  埋め込む。これは mIV 独自の補足であり、承認済み bundle と manifest は変更しない。
- EffeTune の共有プリセット／設定、host 名の WebView 保存領域、Remote sibling の保存領域は
  アンインストール後も残す。削除は利用者の判断で手動とし、アンインストーラの挙動は変えない。

#### 起動後の旧世代清掃（§1.331 / §1.316、2026-10-05）

- 利用者決定は「今の版だけ残して削除」。core の起動完了・初回描画後、`runtime-cleanup` worker
  が `<data_dir>/runtime` 直下の SemVer として読める他版を削除する。ダウングレードでも新しい版を
  削除し、再度その版を起動すれば launcher が再展開する。portable は完全な no-op。
- **P1/P2対応の利用者決定（2026-10-05）**: 各coreはexeの属する `runtime/<version>` と、
  固定したEffeTune世代の外側lockをsharedでプロセス寿命中保持する。`--data-dir` が異なる
  coreも同じ実体のlockを使う。成功した世代解決はpathとleaseを持つ単一ownerとして固定し、
  初回はegui構築前、準備失敗後の再解決は既存load worker内で取得する。UI updateにI/Oを追加しない。
  版の使用権はWindowsのcase aliasも認識してcanonical実体で取得する。APPDATAのredirect自体は
  起動を妨げず、清掃の祖先reparse拒否だけを適用する。
- **P1/P3再レビュー対応（2026-10-05）**: cooperative lockは削除tree外の `runtime/.locks/` に
  固定する。版は `<version>.in-use`／`<version>.extract`、publisherは `<version>.effetune`、
  世代は `<version>.gen-<hash12>-<generation>`。`.locks` はSemVerではなく、列挙でも明示skip。
  lockは微小な永久fileとして残し、古いlockも削除・rotateしない。tree内のlockを消すと別fileへ
  排他が分裂するため、削除処理の工夫で対応せず、削除範囲から除外する単純な所有配置を採用する。
  外側lockは `FILE_SHARE_READ | FILE_SHARE_WRITE` のみでopenし、delete sharingを許可しない。
  runtime親のcanonical identityとversion／generation名から算出し、削除済みtreeの存在を要求しない。
  `.locks` directoryと各lock fileもreparse拒否・canonical confinement／regular file検査を行う。
- launcherはruntime親を作成・canonical化し、外側版shared leaseを取得してから版directoryを
  作成・展開する。清掃が先に取得した場合の起動を維持するため、launcher側の版shared取得だけは
  従来publisherと同じOS workerによる最大60秒待機を使う（清掃側は一度だけ非blocking）。
  exe/DLL抽出は外側exclusive extract lockで直列化する。
  正常世代を使う経路でも外側世代shared leaseを取得してから検査し、新規世代も公開前に取得する。
  新しい世代tree内にlease fileは置かない。旧 `.in-use.lock` があるtreeのstamp互換のため、
  この旧管理fileだけは引き続きasset inventoryから除外し、regular fileかは検査する。
- **released publisher互換**: `runtime/<version>/.effetune.lock` は既存launcherが使うため、
  新publisherは外側lock→旧lockの順に両方exclusive取得し、1回分の従来最大60秒budget内で待つ。
  清掃は外側publisherを非blocking取得後、旧fileがあればそのlockも非blocking取得して保持する。
  世代だけの削除なら旧fileは削除範囲外でありdelete sharingを禁止できる。版全体の削除時に限り
  内側旧fileはdelete sharingを許可してtreeと削除する（外側lockは永久保持したまま）。
  旧file名の消失による未対応旧launcherの新規同時起動は、末尾の既存受容raceの範囲。
  新対応launcherは外側版／publisher lockで同一性を維持するため、この旧fileへ依存しない。
- spawn直後にlauncherがleaseを放す隙間を作らない。coreが自身の版leaseと初回の世代pin／Unavailableを
  確定してnamed eventで通知するまで、launcherはshared leaseを保持してOS waitする。
  coreはこの通知に対して待機しない。coreが先に終了した場合もlauncherは終了できる。
  生存中coreのpinをtimeoutで放すことはしない。通常はこの引き渡し直後にlauncherが終了する。
- 現在版の `effetune` はcurrentと当該coreの固定世代を残す。canonical化した世代名を比較し、
  nonceの大文字・小文字違いも同じ実体として扱う。清掃は各候補の外側使用中lockをexclusiveで
  非blocking取得し、版全体なら外側extract／publisher lockも取得する。
  現在版の世代清掃も外側publisherと存在する旧publisherを取得し、**tree削除完了まで保持**する。
  busy/errorなら資産の削除を始めず今回は見送る。初回世代未確定、未知配置、current異常も見送る。
- 各targetはcanonical化してruntime内の直下child関係を確認する。root／祖先／全subtreeの
  symlink・junction・reparse pointは明示的に拒否し、そのtreeを残す。削除失敗はlogのみ。
  部分削除は許容し、次回起動で再度試す。timer、retry loop、UI通知、終了時joinは持たない。
- **P1 #2は受容（利用者決定2026-10-05）**: 事前検査後に親directoryをjunctionへ差し替える
  TOCTOUは対処しない。成立には利用者と同じ権限で動く別processが必要で、そのprocessは同じfileを
  直接削除できるため、新たな権限を与えない。既存のcanonical／reparse事前検査は維持するが、
  親directoryをhandleで固定した競合耐性を保証するものとは説明しない。
- 同梱bundleだけが対象。EffeTune設定／プリセット／IR／測定データ、WebView保存領域は削除しない。
- **旧版互換（ClaudeCode決定2026-10-05。利用者決定「今の版だけ残して削除」の範囲内で方針1を選択）**: workerが起動ごとに一度だけ
  `K32EnumProcesses` と `QueryFullProcessImageNameW`（query limited rights）でprocess画像を列挙し、
  候補tree内で実行中の画像があれば削除しない。世代内にexeが無い旧coreもあるため、現在版内の
  別 `mimageviewer-core.exe` が動く間は全旧世代を保守的に残す（自coreとremote等のhelperは除外）。
  この事前検査はlock対応版にも適用し、既存のshared／exclusive lock保護も維持する。
  missing leaseは事前検査後に清掃側で外側へ新規作成してexclusive取得するので、未使用の既存旧版も
  清掃できる。作成不能／権限不足は通常の削除失敗と同じくlogして次回起動へ回す。
  process画像はcanonical化して比較し、列挙失敗／固定bufferの不足は全清掃を見送る。
  終了済み／query拒否のprocessは画像を取得できないため検査対象外。全画像の一度の列挙だけで、
  module列挙・VM read・待機・再試行は行わず、UI／起動／初期化の経路には置かない。
  API仕様: [EnumProcesses](https://learn.microsoft.com/en-us/windows/win32/api/psapi/nf-psapi-enumprocesses)、
  [QueryFullProcessImageNameW](https://learn.microsoft.com/en-us/windows/win32/api/winbase/nf-winbase-queryfullprocessimagenamew)。
  開発機のheadless test実測は525画像の列挙＋canonical化で約123ms（性能保証ではない）。
- **旧launcher同時開始raceも受容（同決定）**: snapshot後に新規起動する未対応旧launcherは
  lockを取得しないため、清掃との競合を完全には防げない。新旧版を利用者が同時に起動する必要が
  あり、旧launcherは次回起動で再展開する。上記親junction TOCTOUと併記する受容範囲であり、
  lock対応版同士は版lease／抽出／公開lockで保護する。
- 回帰testは製品の `cleanup_effetune` を呼び、注入した削除操作内で実tree削除後も別handleの
  publisher exclusive／世代shared取得が失敗することを確認する。版全体も削除・再作成の途中で
  同じ外側lockへの取得が失敗し、guard解放後だけ成功する。sleepやtest自身による代理guardは無い。

### 10.3 同梱版更新 (2026-10-04、v4.3.0 公開前)

- 利用者決定: 同梱 EffeTune は未リリースのため、v0.11.1 で保存した EffeTune 状態の
  移行・互換処理は追加しない。旧版ソース確認・実機観測の記録は履歴として残す。
- 承認元は Mixwright v0.12.0、release commit `6f4e2ee`、Windows x64 ZIP の SHA-256 は
  `31df641d9da41aa36a5e5dc3e684282bd534e9e26ef9c5e6ff3b8c47c4f95408`。
  vendor の bundle 全424ファイルが ZIP と一致。旧407ファイルから17追加、削除なし。
- 新しい第三者成分は Rhythm Analyzer の fdlibm 5.3 (atan / atan2)。上流 DSP NOTICE に
  Sun Microsystems の原文が追加済み。その他2通知と JSZip は旧版と同一。
  lie / immediate / setImmediate / pako の zlib ヘッダーは今も上流通知に無いため補足を維持。
- 日本語資料は external/effetune commit `03bffda352287f4f438d063d9660aad7864e553a` の
  verbatim snapshot に更新。Analog Meter / Rhythm Analyzer / Tonal Balance EQ と Visualizer
  拡張を収録し、今回表示されたコントローラーマッピングの設定入口も説明書に追加。
  独立プレーヤー、Clean Feed、外部音声設定など VST で使えない説明は override で訂正。
- 非起動 PE gate は plugin の x64 と VC import closure を確認。最長の相対名は104 UTF-16単位。
  40文字のユーザー名、generation最大45文字を使う通常profile例の最深pathは252単位 (<260)。
- 製品・plugin は起動しない。従来の v0.11.1 実機観測を v0.12.0 の実行結果として扱わない。
  通常 launcher test は release core / remote / EPUB worker が未buildのため保留。
  公開前に release lead が最終 full gate と release build、および実機確認を行う。


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

## 12. 第 6〜13 版 (第 9 版 Sol ACCEPT WITH CHANGES。第 10〜13 版は実装時に判明した不足の補い): リモート配信はローカルの bridge を共有する (2026-09-28、利用者合意)

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

### 12.3 処理の持ち主 (第 7 版で具体化。第 6 版への Sol レビュー REVISE を反映)

**前提 (コード確認、Sol レビュー)**: 操作権がリモートへ移ると、ローカルの player は `set_playing(false)` で
一時停止するだけで生きている (`src/app.rs` の取得経路)。出力 callback は消費を止めるが、音声 pump は
起きて処理済みキューを埋め続け、フレーム受け取り時の reset や終了時の無音 flush も行う
(`src/video/audio.rs`)。したがって「一時停止中は処理しない」は成り立たない。

- **P1 (単一の調停者)**: App が 1 つ持つ `DspProcessingCoordinator` が、共有 bridge (ユーザー VST・
  EffeTune) に対する **すべての host 操作 (process / reset / flush)** の許可を出す。許可は世代付きの
  token (`Local { epoch }` / `Remote { session, generation }`) で表す。token を持たない側は host 操作を
  しない。
- **P2 (切り替えは非同期に確認)**: 持ち主の切り替えは「新しい持ち主を予約 → 旧持ち主の実行中の
  host 操作の終了を確認 → 新しい持ち主が handoff reset → 許可」の順。確認は pump / 配信 worker 側で行い、
  **UI スレッドでは待たない**。
- **P2b (ローカル同士の切り替え、第 8 版)**: `Local` の token は **pump のインスタンスに結び付ける**
  (`Local { pump_instance, epoch }`)。ローカルのメディアを差し替えるとき (前の player の drop 後も
  pump の join は別スレッドで続き、しばらく生き残る)、調停者は前の pump の token を取り消し、
  前の pump の実行中の host 操作 (process・reset・flush) の終了を確認してから、新しい pump に token を
  渡す。同じ token を 2 つの pump が使うことは無い。
- **P2c (許可はブロック単位、第 9 版)**: ローカル pump の 1 ブロックは、ユーザーチェーンと EffeTune の
  2 回の host 呼び出しにまたがる。許可を host 呼び出しごとに確認すると、2 回の間に持ち主が入れ替わり、
  リモートの処理と reset が割り込める。そこで pump は **生の音声をキューから取り出す前に、ブロック単位の
  許可 (permit) を調停者から得て、2 段の処理と処理済みキューへの格納が終わるまで保持する**。調停者の
  取り消しは、実行中のブロックの permit が返るまで待つ (待つのは pump / 配信 worker 側で、UI スレッドでは
  ない)。permit を得られない pump は生の音声を取り出さない。配信 worker も同じく、1 チャンクの全段と
  出力の確定までを 1 つの permit で行う。reset・flush も permit の中で行う。
- **P2d (ローカルの札の移動、第 10 版。実装担当が実装前に指摘)**: ローカルの複数 viewer context
  の player は同時に存在でき、別の pump から札を取り直す場合がある。第 9 版は、その手順を定めていなかった。
  - ローカルの札は **最後に再生を始めたローカル pump** が持つ。再生開始 (一時停止からの再開を含む) の
    遷移で、その pump のインスタンスが札を要求する。
  - **札を得るときは、どの経路でも同じ手順 (第 12 版で順序を確定)**: 前の持ち主 (リモートの配信 worker、
    または別のローカル pump) の実行中 permit の終了を待つ → handoff reset (結果を返す) → 札を得る player を
    **出力を止めたまま (一時停止の状態で) 要求時点の位置へ再 seek** し、新しい seek serial の設定と処理済み
    キューの消去を済ませる → **その後で** 札を有効にする → 要求された再生状態にする。札が有効になるまで
    その pump の DSP ブロックの permit は閉じたままなので、古い生の音声で bridge を呼ぶことは無い。
    **(第 13 版) 札を得ようとしている pump は「受け取り中」の状態になり、再 seek を始めてから札が有効に
    なる (または受け渡しが失敗・上限超過で終わる) までの間、素通しブロックも含めて生の音声の取り出しと
    格納をしない**。再 seek の後に届いた新しいフレームが素通しでキューへ入り、再生開始直後に聞こえることを
    防ぐため。
    待ちと確認は pump / worker 側で行い、UI スレッドでは待たない。
  - **札を持たない pump は bridge を呼ばず、DSP の段を飛ばして素通しで処理する** (第 9 版の「止まって
    待つ」を置き換え)。溜まった素通しの音は、その pump が札を得るときの再 seek で必ず捨てられる。
  - ローカルで 2 つ以上の player が同時に音を出す経路があるかを実装時にコードで確認する。ある場合、
    札を持たない方は素通しで鳴る (明記する制約)。無い場合はその旨を記録する。
  - detached の述語・lifecycle は変えない。再生開始の遷移に札の要求を足すだけにとどめる。
- **P3 (第 10 版で置き換え)**: 札を持たないローカル pump は host 操作をしない (P2d の素通し)。
- **P2e (2 種類のブロック、第 11 版)**: pump のブロック処理を 2 つに分ける。
  - **DSP ブロック**: bridge を呼び得るブロック。P2c のとおり、生の音声を取り出す前に permit を得て、
    2 段の処理と処理済みキューへの格納まで保持する。札を持つ pump だけが使う。
  - **素通しブロック**: bridge を一切呼ばないブロック。permit を取らずに生の音声を取り出してよい。
    処理済みキューへの格納の時点で seek serial を確かめ、古い serial の結果は捨てる。**比較先は時計側の
    現在の seek serial** (seek の要求で先に進む) とする。既存の確認はバッファ側の serial
    (`pump_seek_serial`、pump が後で追いつく) と比べるだけなので、その間に取り出した結果を捨てられない
    (第 12 版)。代わりに seek の要求時にバッファ側の serial も進める方式でもよい。
    札を持たない pump はこちらだけを使う。
- **P2f (再開時は音を止めて待つ、第 11 版)**: 札を得ようとする player は、受け渡しが終わるまで **出力と
  時計を、要求した時点の位置で止めておく** (一時停止中なら一時停止のまま、再生開始の要求なら再生状態に
  入れずに待つ)。順序は P2d の手順どおり (待ち → reset → 一時停止のまま再 seek → 札を有効化 → 要求された
  再生状態にする)。再 seek には既存の精密 seek (`seek_with_play_state` を一時停止指定で) を使う。
  これで素通しの音が聞こえることは無い。
  - **reset の失敗** (プラグイン側の失敗) は、その段の失敗経路 (ユーザーチェーンの既存の disable、
    EffeTune の `fail()`) に送り、player は DSP なし (素通し) で要求どおり再開する。
  - **受け渡しの待ちが上限を超えた場合** は、プラグインの失敗ではないので失敗経路に送らない。その要求を
    取り消し (遅れて前の permit が返っても、その要求では reset も札の付与もしない)、「受け渡しの
    タイムアウト」を別の理由として 1 回報告し、player は素通しで再開する。上限は実装時に既存の値に
    合わせて決め、記録する。
  - **(第 13 版) どちらの失敗の経路でも、素通しで再開する前に、要求時点の位置へ一時停止のまま再 seek し
    て処理済みキューを空にする**。一時停止中の出力は処理済みの chunk を保持しているので、そのままでは
    受け渡し前に処理された音が聞こえる。「受け取り中」の状態は、この再 seek の後に解く。
- **P2g (要求の取り違え防止、第 11 版)**: 札の要求に単調増加の番号を付ける。付与の直前に「その pump が
  今も生きていて、再生を要求している最新の要求者か」を確かめ、違えば付与しない。一時停止・メディアの
  差し替え・リモートによる取得・アプリ終了では、待機中の要求を取り消す。
- **P4 (配信の世代)**: 配信セッション ID を最初の worker を起動する **前に** 割り当てる (現在は後で
  割り当てている、`src/video/stream/session.rs`)。各 worker は `(session, generation)` token を
  処理が終わるまで持ち、終わった後に返す。新しい世代の worker は、旧世代の token が返った後にだけ
  handoff reset して処理を始める (既存の generation resource gate の順序と揃える)。
  停止・切断・seek・画質変更・連続した再取得・アプリ終了を同じ遷移で扱う。
- **P5 (第 14 版で置き換え)**: 音声トラック計画 §9B の利用者決定 (2026-09-26) により、Remote
  取得時には全ローカル閲覧ウィンドウを terminal close し、返却時も復元しない。Remote の再生位置を
  PC に書き戻した後、保持された player の古い位置が上書きする競合を防ぐためである。従って旧 P5 の
  保存位置、再取得間の持ち越し、返却時の再 seek は不要。player の破棄後も pump の join が遅れて残るため、
  調停者はその実行中 permit の終了を待ってから Remote の最初の host 操作を許す。ローカル同士の
  受け渡し時の一時停止 seek と seek serial の規則 (P2d〜P2g) は維持する。
- **P6 (終了時)**: アプリ終了時は、配信 worker が token を返す (処理が止まった) ことを期限付きで
  確認してから、既存の VST 状態スナップショットと EffeTune の最終取得を行う。**期限切れのときは、まだ
  配信側が token を持っている bridge の状態を問い合わせず、前回保存した状態を残す** (ログに残す)。
  token が返った bridge だけを問い合わせる。

### 12.4 reset と失敗の扱い (第 7 版)

- handoff reset は両 bridge とも **結果を返す版** を使う。ユーザーチェーン側の既存の `void` の reset
  (失敗をログに残して握りつぶす) を handoff には使わない。
- 「キャンセル・token を失った」と「プラグインの失敗」を型で分ける。前者は失敗として数えない。
- 失敗の閾値はローカルと同じ (連続 3 回、回復 5 回) に揃え、リモート側で閾値に達した失敗は、
  ユーザーチェーンは既存の disable、EffeTune は controller の `fail()` へ **1 回だけ** 送る。
- 配信 worker は EffeTune の bridge を `Arc` で持ちっぱなしにせず、ブロックごとに controller が公開する
  スロットを読む。スロットが空になったら (controller が `fail()` した)、その段を以後適用しない。
- **途中参加はしない (第 8 版)**: ブロックごとのスロット確認は、**配信の受け付けで採用した段** にだけ行う。
  配信中に EffeTune を起動した場合、その配信には加えず、次の配信から適用する (§6 の既知の制約と同じ)。

### 12.5 サンプルレート (第 7 版)

- ユーザーチェーンと EffeTune は別々に、読み込んだ時点の出力デバイスのレートで準備される。後から
  プラグインを読み直すとユーザー bridge のレートが変わり得る。
- **配信の処理レートの決め方 (第 8 版)**: 配信の受け付け時に、**有効な段** から優先順で決める:
  ユーザーチェーン (有効かつ active なスロットがあり、レートが 0 でない) → EffeTune (受け付けで採用され、
  レートが 0 でない) → 素通し時の既定。決めたレートは世代ごとに固定する。無効な段がレートを決めて、
  有効な段が不一致扱いで外れることが無いようにする。
- 配信の受け付け時と、各ブロックの処理の前 (bridge の組み直しの後を含む) に、**段ごとに** bridge の
  レートと配信の処理レートが一致するかを確認する。一致しない段は、その配信では適用せず warning を
  出す (段ごとに固定)。
- 遅延の秒換算は、実際に適用した段のレートで行う。第 5 版の遅延補正はそのまま使う。

### 12.6 起動直後・未準備のとき (第 7 版)

- ユーザー VST の起動時読み込み、または EffeTune のロードが終わっていない間に配信が始まった場合、
  配信の受け付けは **待機状態** に入り、UI の poll で状態を見る (UI スレッドで待たない)。
- 期限 (既存の 15 秒の開始予算のうち、変換の開始と最初の再生リストに要る分を残した範囲) の時点で、
  段ごとに独立に判断する。間に合った段は使い、間に合わなかった段はその配信では適用せず warning を出す。

### 12.7 範囲外: ローカルの別窓が動画を保持していると配信開始が待たされる件

- 利用者の実機ログで、同じ動画をローカルの別窓 (一時停止中) が開いていると、リモートの開始が
  15 秒の予算を使い切って失敗した (`StartPlayerTimeout`、2 回目で成功)。
- 原因 (Sol レビュー): 取得時に他の viewer context の player を一時停止するが保持するため、一時停止中の
  動画デコーダが同時 1 本の枠を占め、リモートの headless player を作れない。
- **bridge の共有とは無関係で、本版では直さない**。直すには detached の lifecycle に触れる
  (凍結ルールの手続きが要る) か、リモートの受け付けをメタデータだけで行う経路が要る。別の不具合として扱う。

第 14 版では音声トラック計画 §9B の取得 barrier が全 viewer を閉じるため、この保持 decoder
による競合は発生しない。上記は第 13 版までの経緯として残す。

### 12.8 撤去するもの

- リモート用のセッション bridge (`ClocklessAudioProcessing::with_remote_vst3` が作る `DspBridge::new()`、
  EffeTune の配信用 bridge)。
- 配信受け付け時の EffeTune 状態の取り直し (第 4〜5 版の §6)、その capture source・受け付け helper と
  それらのテスト、ロード予算の共有期限。
- リモート側の plugin 読み込みに関する warning (ロード失敗・時間切れ)。処理中の失敗・レート不一致・
  未準備の warning は残す。

### 12.9 テスト

- 調停者: token を持たない側が process / reset / flush を呼ばない。Local→Remote→Local と、配信の
  世代の切り替え (旧 worker がまだ動いている状態を含む) で、各持ち主の最初の host 操作の前に handoff
  reset が成功している。
- (第 14 版) 札を持たないローカル pump は host 操作をせず素通しで処理する。別のローカル pump
  から札を得るときに再 seek でそれまでの処理済み音が捨てられる。Remote 取得 barrier は全ローカル
  viewer を閉じる。player の破棄後に旧 pump の permit が残っても、Remote の最初の host 操作は
  その permit が返るまで始まらない。Remote 返却時に viewer は復元・再 seek されない。
- 停止・切断・seek・画質変更・連続した再取得・アプリ終了の各遷移で token が返る。終了時は token が
  返ってからスナップショットを取る。期限切れのときは、まだ持たれている bridge を問い合わせない。
- reset の失敗、キャンセル、プラグインの失敗が区別され、失敗が既存の経路へ 1 回だけ送られる。
  controller が EffeTune のスロットを空にしたら、配信側もその段を適用しなくなる。
- 段ごとのレート一致の確認 (受け付け時と組み直し後)、不一致の段だけが外れる。
- 起動時ロード中に配信を始めた場合の待機状態と、期限での段ごとの判断。
- 第 5 版の遅延補正テストは残す。
- (第 9・11 版) DSP ブロックで、生の音声を取り出した後・ユーザーチェーンの後・EffeTune の後の各時点で
  取り消しを要求しても、そのブロックは permit の中で最後まで処理・格納され、取り消しはその後に完了する。
  札を持たない pump は素通しブロックだけを使い、bridge を呼ばない。素通しブロックの結果は古い seek serial
  なら格納時に捨てられる (決定的なテストで確かめる)。
- (第 11 版) 再開を要求した player は、受け渡しが終わるまで出力も時計も進まず、受け渡し後に
  `seek_with_play_state` で要求時点の位置から再開する (素通しの音が聞こえない)。受け渡しの失敗・上限超過では
  素通しで再開し、失敗が 1 回だけ報告される。
- (第 12 版) 札の有効化は再 seek の serial 設定とキュー消去の後。その間 DSP ブロックの permit は閉じていて、
  古い生の音声で bridge が呼ばれない。
- (第 12 版) 受け渡しの待ちの上限超過はプラグインの失敗として扱われず (EffeTune が `Failed` にならない)、
  遅れて返った permit でもその要求は札を得ない。reset の失敗だけが段の失敗経路に入る。
- (第 12 版) 素通しブロックを取り出した後・格納の前に seek を要求すると、そのブロックは格納時に捨てられる。
- (第 13 版) 再 seek から札の有効化までの間にフレームが届いても、「受け取り中」の pump は取り出さず、
  再生開始後に最初に聞こえるのは DSP を通した音になる。
- (第 13 版) reset の失敗・受け渡しの上限超過の両方で、素通しの再開前に再 seek とキューの消去が行われ、
  最初に聞こえる chunk が受け渡し前に処理された音ではない。
- (第 11 版) 古い要求が新しい要求より後に完了しても札は新しい要求者に行く。一時停止・差し替え・リモートの
  取得・終了で待機中の要求が取り消される。
- (第 8 版) ローカルのメディア差し替えで、前の pump が process・reset・flush の途中にいる状態でも、
  新しい pump は前の pump の host 操作の終了後にだけ token を得る。
- (第 8 版) 終了時に配信側の token が期限までに返らない場合、その bridge を問い合わせず、保存ファイル・
  設定の状態が変わらない。
- (第 8 版) 処理レートの決定: ユーザーチェーンが無効・active なしのとき EffeTune のレートで決まり、
  EffeTune が不一致扱いで外れない。
- (第 8 版) 配信開始後に EffeTune を起動しても、その配信には加わらない。

### 12.9b 実機での受け入れ条件 (第 9 版、利用者が確認)

- ローカル再生からリモート配信へ切り替えた直後の **最初に聞こえる区間から** EQ が効いていること
  (bridge 共有で復元待ちが無くなったことの確認。反映完了を知る手段がホストに無いため、実機でしか確かめられない)。
- 配信中の seek と画質変更の直後も EQ が効いたままであること。
- **handoff reset が読み込み済みの EQ パイプラインを保つこと** (reset の後に素通しに戻らないこと) を記録する。
- Remote を閉じた後、PC で改めて動画を開くと EQ が効き、Remote が書き戻した位置と音声トラックから始まること。

### 12.10 文書

- `docs/vst3-integration.md` §2 の「streaming session 専用 DspBridge」の記述を、共有と調停者の説明に
  置き換え、設計変更の理由を残す。
- `docs/async-architecture.md` の配信の世代の説明、`docs/architecture-overview.md` の host プロセス数、
  `docs/web-remote-video-streaming-plan.md` のセッション共有の VST 段の説明を更新。
- 本書 §6 (第 4〜5 版のリモート 2 段構成)、§8 のリモートのテスト、§9 の実装記録のうちリモートの状態
  再取得、§10 の「配信中の設定変更のリモート反映」は第 6〜9 版で置き換えた (本版で注記済み)。

### 12.11 実装記録 (第 13 版)

- 調停者は `src/video/dsp/coordinator.rs` の `DspProcessingCoordinator`。ローカルの札は
  `pump_instance` と `epoch`、リモートの札は `session` と `generation` を持つ。
  ローカルの受け渡し待ち上限は 2 秒。期限切れの要求は取り消して素通しで再開し、host 故障にはしない。
- ローカル pump は調停者から DSP 許可・素通し・受け取り中を一回の判定で得る。
  素通しブロックは自身の pump の遷移世代を取り出し時に記録し、格納時は予約・取消と同じ lock の下で
  世代を再確認する。自身の世代が変わったブロックだけを捨て、別の同時再生 pump の取得では捨てない。
  札も予約も持たない pump の段が無効になっても世代は進めず、pump 破棄時は札がなくても進める。
  pump 終了後に世代の記録を除去する。
- ユーザー VST と音響調整の両方に適用可能な段がない場合、ローカルの open / resume は
  受け渡し・一時停止・追加 seek を行わない。pump の各ブロックでは armed atomic の読み取りだけで
  素通し経路に入り、調停者の lock を取らない。再生中に段が適用可能になったときは、予約が始まるまで
  観測済み扱いにせず、既存の受け渡し手順で札を要求する。メタデータ待機中に段が消えた場合は要求位置へ
  再 seek して autoplay 意図を復元し、素通しで再開する。受け渡し worker の起動失敗は型付きの終端結果として
  一度記録し、同じ適用状態では繰り返し一時停止・再 seek しない。段の再適用か新しい再生要求で再試行する。
- ローカルの複数 viewer context はそれぞれ生きた `VideoPlayer` と音声出力を保持できる。
  同時に可聴な player が存在し得るため、最後に再生開始した pump が札を取り、他方は素通しで鳴る。
  初回再生の要求がメタデータより先に来た場合、保存済み再生位置の確定まで受け渡しの予約を遅らせる。
  メタデータより先の明示的 seek はその位置を優先し、確定後に共通の受け渡しと一時停止 seek を行う。
  待機中のローカル要求が別のローカル再生開始に抜かれた場合も、要求位置へ再 seek して素通しへ戻る。
  リモート取得または終了による取り消しでは再生を再開しない。
- リモート受け付けのロード待ちは 15 秒の開始予算から後段用 3 秒を残し、最大 10 秒。
  配信で使う段の起動時ロードだけを待ち、期限時はユーザー VST と音響調整を個別に判断する。
  session ID は初回 worker より前に割り当てる。EffeTune は受け付け時の slot 世代だけを採用し、
  各音声ブロックで slot を読み直す。
- 終了時の DSP quiescence 上限は 2 秒。まだリモートが札または実行中 permit を持つ場合は
  共有 bridge の状態取得を飛ばし、既存の保存状態を残す。

### 12.12 第 14 版: master の音声トラック選択との統合

- 音声トラック計画 §9B の明示的な利用者決定 (2026-09-26) を優先する。Remote 取得時は全ローカル
  閲覧ウィンドウを閉じ、返却時に復元しない。旧 P5 専用の取得位置 snapshot、再取得間の持ち越し、
  返却時の再 seek とそのテストを撤去した。Remote の位置書き戻しを保持中の古い player が上書きしない。
- ローカルの音声トラック変更は同じ player の精密 seek と新 seek serial を発行する。札を持つ pump
  だけが permit 内で seek 世代の reset と両段の処理を行う。札を持たない pump は素通し、取得中は
  raw dequeue を停止し、seek 前の素通しブロックは格納時に捨てる。ローカル同士の P2d〜P2g は維持する。
- Remote の選択トラック index と世代ごとの Norm DB lookup は master の経路を採用する。同じ
  `(session, generation)` token を共有 bridge の処理に使い、音声トラック変更で旧世代が退役して
  から新世代が handoff reset する。新しい専用 DSP bridge は作らない。
- ローカルの受け渡し待ちは一時停止によって engine の play intent を消す。待機中に音声トラックを
  切り替える場合は、未完了 handoff の再生要求と要求位置を使って新しい要求へ置き換える。
  これにより切り替えが素通しの一時停止 seek へ変わらず、旧表示フレームの位置へ戻らない。
- §12.9 の旧 P5 の保持 player 再 seek テストは廃止。取得 barrier の全 viewer close テストと、
  player 破棄後も残る pump permit が返るまで Remote の最初の host 操作を待つ調停者テストで確認する。


## 13. 2026-10-04 v4.3.0 リリース前: EffeTune 入力のピーク保護

### 原因・承認済み仕様

利用者の 2 台で、加工なし・音量 100% の AAC 音楽動画でも Level Meter が OVERLOAD を表示。
デコード後 sample peak は +2.04 / +2.10 dBFS。mIV は f32 をそのまま渡し、Mixwright
v0.11.1 の meter は `|sample| > 1.0` を検出する（true peak ではない）。ユーザーの
Pro-L 2 を前段で有効にすると消えるため、EffeTune 入力の sample peak が原因。
利用者承認に従い、音量を全体的に下げる pre-gain は加えず、ユーザー VST3 後・EffeTune 前に
独立 SafetyLimiter（0 dBFS、5 ms、100 ms release、ceiling へ clamp）を追加。
既存最終 limiter は順序・有効化条件・HUD 通知を維持する。前段の低減で HUD は点灯させない。

### 所有境界・単純化・遅延

- `EffetuneInputLimiter` は各 local pump / clockless processor が所有し、scratch にコピーして処理。
  原本は `compose_samples` の fallback 用に保持。成功時だけ前段 + EffeTune latency を採用し、
  failure / Ok(false) / 無効 / admission 不成立 / permit なしでは原本とユーザー側 latency を維持。
- admission は従来の plugin 合計 2 秒という契約を維持。前段 5 ms は最終 limiter 同様に上限外。
  PDC には実際の lookahead frame 数 / sample rate を足す（44.1 kHz の丸めも一致）。
- local seek serial 更新、段の非適用、EffeTune generation 変更、最終 limiter reset に合わせて前段を reset。
  Remote は seek serial 変更と非適用で reset、新 worker / generation は初期 state から開始。
- 簡素化を検討: 再生中の設定反映に新しい atomic / 遅延切り替えを足す代わりに、既存の開始時
  snapshot を採用。`effetune_pre_limiter_enabled` は player 作成 / Remote 配信受付時に取り、
  Remote の seek・画質変更世代では維持。画面を閉じて開き直す／配信終了後の再開で反映する。
  pause/resume・既存 player の再利用では変えない。新再生の limiter は必ず空で始まる。
- 独立設計レビュー（GPT-6.1 Sol / xhigh）: ACCEPT、blocking finding なし。

### 設定・UI・経路確認

既定 ON、serde `default_true`。settings_db は残り全 field を settings_kv へ保存するため
schema / allow-list 変更不要。runtime 所有の GUI rect と異なり preferences-owned field とし、
`overwrite_non_preferences_from` に加えない。動画ページに音響調整節を追加し portable の描画と検索を除外。
ラベル案: 「EffeTune に渡す前に 0dB を超える音を抑える」。説明は全体音量を下げないこと、
可視化時の OVERLOAD、OFF と最終出力保護、閉じて開き直す／配信再開の反映時点。

確認した producer / consumer:
- `App::local_audio_dsp_chain` → 動画 builder / 音楽 builder → `audio.rs` pump（動画の音声表示モードも同じ）。
- Remote 動画 / 音楽 → `remote_clockless_audio_processing` → shared bridge adapter → `ClocklessAudioProcessor`。
- RemoteHeadless metadata player は音声 DSP chain を持たず、配信は clockless worker のみが処理。
- `SharedEffeTuneProcessor` の process は上記 clockless consumer 経由のみ。
- 終了時の `flush_silence` はゼロ入力で、メディア音声の迂回経路ではない。

### 検証記録

本 worktree の未コミット差分で実施（製品 exe は起動していない）:

| コマンド / 対象 | 結果 | ログ |
| --- | --- | --- |
| `cargo check -p mimageviewer --bin mimageviewer-core` | exit 0 | `target/prelimiter-check.log` |
| 同上 `--features portable` | exit 0 | `target/prelimiter-check-portable.log` |
| `cargo test -p mimageviewer --lib effetune_pre_limiter` | 7 passed / exit 0。設定 UI→checkbox→OK→保存→再読込→local consumer / Remote admission、上限、OFF、遅延、reset | `target/prelimiter-unit.log` |
| 同上 `video::audio::tests` | 52 passed / exit 0 | `target/prelimiter-audio.log` |
| 同上 `video::clockless_transcode::tests` | 33 passed、実ホスト profile 用 1 ignored / exit 0。前段 ON の AAC fallback 連続性も確認 | `target/prelimiter-clockless.log` |
| 同上 `effetune::composition::tests` | 3 passed / exit 0 | `target/prelimiter-compose.log` |
| 同上 `settings_db::tests` | 120 passed / exit 0 | `target/prelimiter-settings-db.log` |
| `cargo test -p mimageviewer --test ui_snapshot` | 60 passed / exit 0 | `target/prelimiter-ui-snapshot.log` |
| `python scripts/check_ui_glyphs.py` | exit 0、危険 glyph 0 | `target/prelimiter-glyphs.log` |
| `cargo fmt` / `cargo fmt --check` / `git diff --check` | 実施、check は exit 0 | — |

追加した画像は `preferences_effetune_input_limit_dark.png` のみ。既存画像は変更せず、
新規画像の日本語・行の収まりを目視確認した。AAC fixture は前段＋最終＋plugin 遅延を
先頭 block 内に収めるため 512→1024 frames とし、既存の部分 trim 検証の意味を維持。
独立完了レビューも ACCEPT（文書の encoding 指摘は修正して再確認済み）。

`test-full.ps1` は vendor の `mimageviewer-vst3-host.exe` を起動する handler test を含むため、
利用者の `mimageviewer*.exe` 起動禁止に従い未実施。全体 gate と実機の Level Meter / A/V sync / seek / OFF / Remote 確認は
リリース担当の検証枠に残す。

確認用 build は `build-dev.ps1 -PreserveRuntime -WaitForOtherBuildsMinutes 0` で成功（exit 0、
normal feature set / dev-runtime、core 9m36s、Remote / EPUB worker も成功、VCRT PE check は runtime=4 / pe=3）。
初回は turbojpeg-sys の並列 MSBuild が失敗したため、当該依存を単一ジョブでビルド後、
今回の invocation のみ `CARGO_BUILD_JOBS=1` / `MSBUILDDISABLENODEREUSE=1` を指定した。
設定ファイル・build script は変更せず、コマンド終了時に env を元へ戻した。
`target/dev-runtime/mimageviewer-core.exe` を利用者確認用に用意し、起動はしていない。
この normal build は既定で実利用中の `%APPDATA%/mimageviewer` を使う。

### Codex P2 対応: 通常 EOS の保持音声排出（2026-10-04、公開済み v4.3.0 には未収録）

47c28db2c は利用者判断により v4.3.0 へ入れず、末尾欠落を許容して公開した。以下は次版向けの実装・検証記録。

独立レビュー指摘: `b55403c5f` では前段 limiter の lookahead が通常 EOS に出力されず、
設定 ON で 44.1kHz の 221 / 48kHz の 240 frames が追加で失われていた。
local pump は EOS 後も seek を待つため、終了時の `flush_silence` の結果を使う修正では届かない。

- `AudioDspTail` に最後の適用済み段・遅延・boost・連続 PTS を trim/reconcile 前に保存する。
  通常 EOS のみ、ユーザー VST3 後へ無音を入れ、前段 → 同じ EffeTune → 最終 limiter を通す。
  前段 lookahead + EffeTune の報告遅延を排出後、最終 limiter へ直接無音を入れてその 5ms も排出する。
  同じ仕組みで安全に出せるため、最終 limiter の既存の末尾欠落も修正する（前段 OFF / EffeTune 不在でも適用）。
- PDC は最後の実ブロックと同じ値を保持し、audible PTS は連続。normalize / stretch /
  ユーザー VST3 を再適用しない。前段だけの低減は HUD に反映せず、最終段の従来の判定を使う。
  任意長の EffeTune 残響やユーザー VST3 / stretch の tail、local resampler の既存の微小欠落は対象外。
- local は decoded EOS の後に rx / deferred / raw を出し切り、既存 refill の最大 10ms ブロックで
  processed cap・所有 permit・seek 確認・pre-target trim・tap・queue commit を維持する。
  Remote は decoder / resampler drain 後、AAC finish 前に checkpoint / permit / mux 経由で排出する。
  cancel / stop / seek / Remote source-limit は排出しない。段の置換・無効化・失敗時は前段保持音声を捨て、
  既に最終 limiter が保持する出力だけを排出する。
- 設計の簡素化: channel 全体を新 enum に変えず、時計上の短い mutex 内の typed `AudioEos`
  （Decoding / Decoded / Draining / Complete）を decoder・pump・tick の単一完了所有者にする。
  seek serial 更新と EOS reset/publication を同じ lock で直列化し、IPC や buffer lock を保持しない。
  audio packet EOF も demux serial 付きにして、優先 Flush 後の古い EOF を拒否する。
  native / 非 native tick は tail commit 完了まで EOF/loop に進まない。
  長い plugin delay を一括 IPC に渡さず、通常の小ブロックへ分ける。
- 独立設計レビュー（GPT-6.1 Sol / xhigh）: amended design ACCEPT。

完了時の独立コードレビューも ACCEPT。空の新 seek 世代で EOS になった場合は、pump の stale clear で
buffer owner を現 serial へ進め、両 limiter / tail を捨てて完了できるようにした。
排出の IPC 直前にも seek / cancel / permit を確認し、失敗時は通常の StageHealth 閾値を維持する。

| follow-up 検証 | 結果 | ログ |
| --- | --- | --- |
| `cargo test -p mimageviewer --lib video::audio::tests` | 56 passed。非ゼロ末尾、ON/OFF、44.1/48kHz、短い入力の trim、cancel、世代、host admission | `target/eos-audio.log` |
| 同上 `video::clockless_transcode::tests` | 36 passed / 実ホスト用 1 ignored。長い plugin delay の分割、reconcile が全削除した末尾、failure 閾値 | `target/eos-clockless.log` |
| 同上 `video::clock::tests` / `video::decoder::demux_serial_tests` | 14 / 5 passed。世代付き EOS と完了 state | `target/eos-clock.log` / `target/eos-demux.log` |
| 同上 `eos_tick` / `eos_empty_audio_seek` | 各 1 passed。native/headless の in-flight gate、空 seek の実 pump → tick | `target/eos-tick.log` / `target/eos-empty-seek.log` |
| 同上 `video::tests::native_tick_` / `reaches_eof_with_real_pump` | 4 / 2 passed。既存 deadline と実 decoder の動画・audio-only EOF | `target/eos-native-tick.log` / `target/eos-real-pump-eof.log` |
| `cargo check -p mimageviewer --bin mimageviewer-core` / 同上 `--features portable` | 両方 exit 0 | `target/eos-check.log` / `target/eos-check-portable.log` |
| `cargo fmt` / `cargo fmt --check` / `git diff --check` | 実施、check は exit 0 | — |
| `python scripts/check_ui_glyphs.py` | exit 0、危険 glyph 0。UI・snapshot は変更なし | `target/eos-glyphs.log` |

製品 exe は未起動。任意長の effect tail と local resampler の既存の微小欠落は引き続き対象外。

follow-up の確認用 build も `build-dev.ps1 -PreserveRuntime -WaitForOtherBuildsMinutes 0` で成功
（exit 0、normal feature set、core 10m21s、Remote / EPUB worker 成功、PE runtime=4 / pe=3）。
今回の process 内だけ `CARGO_BUILD_JOBS=1` / `MSBUILDDISABLENODEREUSE=1` を設定し、終了時に戻した。
ログは `target/eos-build-dev.log`（stdout の build/staging 記録）。
`target/dev-runtime/mimageviewer-core.exe` を更新したが起動せず、確認は利用者へ引き渡す。
最終の `cargo fmt` / `cargo fmt --check` / `git diff --check` も exit 0。
コミットせず、follow-up 専用メッセージを `target/prelimiter-msg.txt` に UTF-8 / BOM なしで上書きした。


### Codex P2 対応: 完了 wake・Remote 終端容量・最終 limiter 常時適用（2026-10-04）

この follow-up は公開済み v4.3.0 には未収録。47c28db2c に master（Mixwright v0.12.0）を
統合した 78567823e に対する独立レビューの P2 2件と、同日の利用者決定に対応する。

- local の Complete publication は現 seek serial の Draining → Complete が成功したときだけ
  `EngineEventSender::wake_ui()` で ROOT を即時起床させる。tail IPC 中に tick の deadline が
  None でも完了を観測し、既存の 48ms quiet timer を開始する。stale / 重複完了では起床しない。
- Remote の finishing は引き続き capacity 待ちを解除する。終端待機を増やす単純化は、
  未公開 fragment を端末が release できない循環を再導入するため採らない。既存の working /
  codec 終端用 2 slot に `ceil((最大 plugin 2秒 + 両 limiter 約10ms) / fragment 2秒)` の
  2 slot を予約する。ring 保持数は live capacity + 4。metadata と A/V / audio-only mux を
  同じ helper で設定する。作成中 fragment がほぼ満杯でも、最大 tail と AAC finish が最古の
  未読 fragment を追い出さない。通常時の先読み admission / release 規則は維持する。
- 最終 limiter は local 動画・音楽（audio-only / 動画の音声表示を含む）と Remote 動画・音楽で
  常時実行。`limiter_required` / `limiter_active` / normalize の最大 boost 追跡 / inactive reset を撤去。
  normalize の 4秒 ramp が gain 1.0 をまたいでも遅延線を保ち、約 5ms の実 frame 数による
  PDC を常に加える。通常 EOS で最終段を必ず排出する。前段は EffeTune 成功時かつ設定 ON のみ、
  failure / fallback では前段の音・遅延を除外し、最終段の 5ms は残す。plugin admission の 2秒には
  両 limiter を引き続き含めない。HUD は最終段の 1dB 以上の低減だけを通知する。
- 設定 snapshot と checkbox は変更なし。即時反映の検討は backlog §1.324 に残す。
- 回帰は ROOT wake と tick の返す quiet deadline、stale / 重複完了、実 DSP → AAC / mux / finish
  の full manual-control ring、エフェクトなしの非ゼロ末尾、normalize gain の unity 越えを確認する。

独立完了レビュー（GPT-6.1 Sol / xhigh）は ACCEPT。対象は 78567823e に対する本 round の未コミット差分。

| 本 round の検証 | 結果 | ログ |
| --- | --- | --- |
| `cargo test -p mimageviewer --lib video::audio::` | 58 passed | `target/limiter-round-audio.log` |
| 同上 `video::clockless_transcode::` | 38 passed / 実ホスト用 1 ignored | `target/limiter-round-clockless.log` |
| 同上 `effetune::composition::` / `video::decoder::` / `video::clock::` | 3 / 80（性能計測 1 ignored）/ 14 passed | `target/limiter-round-test-1.log`〜`-3.log` |
| 同上 `eos_tick` / `eos_empty_audio_seek` / `video::tests::native_tick_` / `reaches_eof_with_real_pump` | 1 / 1 / 4 / 2 passed | `target/limiter-round-test-4.log`〜`-7.log` |
| 同上 `video::stream::segmenter::` | 7 passed。関連 lib tests 計 208 passed / 2 ignored | `target/limiter-round-segmenter.log` |
| `cargo check -p mimageviewer --bin mimageviewer-core` / 同上 `--features portable` | 両方 exit 0 | `target/limiter-round-check.log` / `target/limiter-round-check-portable.log` |
| `cargo fmt` / `cargo fmt --check` / `git diff --check` | 実施、check は exit 0 | — |
| `python scripts/check_ui_glyphs.py` | exit 0、危険 glyph 0。UI / snapshot は変更なし | `target/limiter-round-glyphs.log` |

自動検証の初回失敗は新しい常時遅延の期待値と normalize の浮動小数点許容差を修正して解消。
製品 exe は未起動。全体 gate は実 VST host 起動を含むため従来の禁止に従い未実施。
実機での A/V sync、ループ・連続再生、末尾に音がある素材、最大遅延の Remote は次版の検証枠に残す。

本 round の確認用 build は `build-dev.ps1 -PreserveRuntime -WaitForOtherBuildsMinutes 0` で成功
（exit 0、normal feature set、core 6m41s、Remote / EPUB worker 成功、PE runtime=4 / pe=3）。
この invocation のみ `CARGO_BUILD_JOBS=1` / `MSBUILDDISABLENODEREUSE=1` とし、終了時に復元した。
製品 exe は起動せず、`target/dev-runtime/mimageviewer-core.exe` を利用者へ引き渡す。
既定は実利用中の `%APPDATA%/mimageviewer`。起動前に installed / tray-resident mIV を閉じる。
ビルド結果の要約を `target/limiter-round-build-dev.log` に記録し、this round 専用の commit message は
`target/prelimiter-msg.txt` に UTF-8 / BOM なしで上書きした。コミットはしていない。

### Codex P3 対応: 0 秒開始の初期遅延補正（2026-10-04）

6bef20dd5 に対する独立再レビューで、`audible_pts_after_latency` が負の PTS を 0 に clamp し、
初回・0 秒への seek / loop で最終 limiter の先頭無音を trim できない P3 を確認。
20ms の 2 chunk は最初が PTS 20ms まで伸び、次が 15ms に始まり、EOS までの出力が
入力より約 5ms 多くなる。常時 limiter により dry 再生にも影響する。この follow-up も
公開済み v4.3.0 には未収録。

- source timeline への遅延換算は保ち、`max(0.0)` のみを除く。負の audible PTS は
  `ProcessedChunk` / `AudioDspTail` の未 trim metadata に保持し、既存 pre-target trim で除去する。
  Remote は既に負の PTS を保持し、AAC 入口で trim するため変更しない。
- 新しい補正フラグや開始時専用分岐は追加しない。同じ通常処理 / tail / seek reset を使う。
- 回帰は 44.1 / 48kHz、native / headless の実 pump と callback を通す。先読みより短い入力、
  先読みと同じ長さ、1 frame 長い入力、短い分割入力、20ms の 2 chunk をそれぞれ
  初回 → 実 EOF tick の loop-to-zero → user seek-to-zero で再生する。
  全 PCM の一致、入力 frames == EOS までの出力 frames、PTS の連続と終端時刻、PDC を検証する。

独立 follow-up レビュー（GPT-6.1 Sol / xhigh）は ACCEPT。対象は 6bef20dd5 に対する未コミット差分。

| P3 follow-up 検証 | 結果 | ログ |
| --- | --- | --- |
| `cargo test -p mimageviewer --lib zero_start_pump` | 1 passed。上記 60 組み合わせを実 pump / callback で確認 | `target/p3-zero-start.log` |
| 同上 `video::audio::` | 59 passed（新しい回帰を含む） | `target/p3-audio.log` |
| 同上 `video::tests::native_tick_` / `eos_tick` / `eos_empty_audio_seek` / `reaches_eof_with_real_pump` | 4 / 1 / 1 / 2 passed | `target/p3-test-1.log`〜`-4.log` |
| 同上 `video::clock::` | 14 passed。上記関連テスト計 81 passed、重複する狭い回帰を除く | `target/p3-test-5.log` |
| `cargo check -p mimageviewer --bin mimageviewer-core` / 同上 `--features portable` | 両方 exit 0 | `target/p3-check.log` / `target/p3-check-portable.log` |
| `cargo fmt` / `cargo fmt --check` / `git diff --check` | 実施、check は exit 0 | — |

Remote / composition / decoder の処理と UI は変更なし。前 round の有効な検証結果を再利用し、
UI snapshot と glyph check の再実行は不要。製品 exe は未起動。実機の A/V sync / seek / loop と
任意長の残響・ユーザー VST3 tail についての前 round の残事項は維持する。

P3 の確認用 build も `build-dev.ps1 -PreserveRuntime -WaitForOtherBuildsMinutes 0` で成功
（exit 0、normal features、core 3m06s、Remote / EPUB worker 成功、PE runtime=4 / pe=3）。
この invocation 内だけ `CARGO_BUILD_JOBS=1` / `MSBUILDDISABLENODEREUSE=1` とし、終了時に復元。
build / staging の information stream は `target/p3-build-dev.log` に記録した。
`target/dev-runtime/mimageviewer-core.exe` を更新したが起動していない。通常 APPDATA を使うため、
利用者の確認前に installed / tray-resident mIV を閉じる。実機確認の残事項は前 round と同じ。
コミットせず、`target/prelimiter-msg.txt` はこの P3 follow-up のみの内容で UTF-8 / BOM なしに上書き済み。


### 実機フィードバック: 周回先頭への resampler 末尾混入（2026-10-04）

5a569fb6f の dev-runtime を利用者が確認。2秒 / 48kHz stereo の可逆素材で、
末尾4ms・10msのバーストは loop ON のとき2クリック、OFFなら1クリック。
中央4msの対照素材は1回。通常 profile の Pro-L 2 + EffeTune では末尾4ms素材に
open 直後のノイズもあった。これらは利用者の聴感結果であり、PCM測定ではない。
この調査・修正も公開済み v4.3.0 には未収録。codec だけの Flush と SWR の EOS 未排出は
v4.3.0 のソースにも存在する。利用者は installed v4.3.0 と聴感比較していないため、
クリック全体を今回の EOS 変更による退行と断定しない。

- 従来の P3 回帰は AudioFrame を注入し、Complete 後にまとめて callback で消費した。
  decoder / swresample の履歴や、callback 間の underrun 無音を検査していなかった。
  新しい回帰は実 WAV / MKV decoder → pump → callback → UI の EOF Full-loop tick →
  demux seek / Flush を通し、全 callback 出力と実消費 PCM を別々に保持する。
  48kHz 出力では各素材3周の単一バーストを確認。44.1kHz 出力では末尾4ms WAV の
  次周先頭に peak 約0.69995 を再現し、単体 decoder と実 EOF-loop の両方が修正前に失敗した。
- 原因は audio worker の Flush が codec だけを初期化し、SWR の delay / filter 履歴を
  次 serial へ持ち越したこと。単なる seek reset では保持音声を捨てるため、通常 EOS では
  codec → SWR → 共通 PCM sender → decoded EOS → 既存 DSP tail の順に排出する。
  seek / stop / cancel では履歴を捨て、codec・SWR・sample timeline を同じ所有境界で初期化する。
- SWR の出力は入力 frame の PTS へ毎回貼り直さず、一つの timeline owner が入力端と
  実出力 sample 数による出力端を管理する。stream time-base の丸めは連続扱いにし、
  実際の forward discontinuity は旧 SWR を排出・初期化してから新 PTS へ移る。
  missing / backwards timestamp の既存の単調補完を保つ。新しい待ち時間・transport flag は設けない。
- 排出中の pause は保持音声を捨てない。共通 sender が現在の PCM を stack 上に保持し、
  bounded channel の空きを待つ。pause 中に enqueue できても callback の Playing gate は
  音を出さず、次 packet の取得前に decoder が park する。seek / cancel は既存 serial /
  cancel fence で pending PCM を破棄し、queue accounting を戻す。専用 EOS mode や queue は追加しない。
- 成功した SWR 変換が0出力でも入力端を進め、後続入力と EOS で回復するケースを検査する。
  ただし48kHz→44.1kHzで全体1入力 frameだけの極短媒体は、標準SWR NULL flush自体が
  0出力となることを別テストで確認した。これは今回の2秒素材の欠落／混入とは別の上流制約で、
  padding / crop による新しい回復処理は今回の範囲に含めない。
- user VST / EffeTune の reset 呼出しは open の DSP handoff と、新 serial の最初の有効 frame に
  既に存在し、v4.3.0 と条件は同じ。user VST は今回の EOS drain に入力されない。
  従って通常 profile の open ノイズを「reset 呼び忘れ」や今回の drain だけで説明する根拠はない。
  setProcessing の切替と報告 latency 分の無音処理は plugin 内部状態の完全消去を保証しない。
  強い再初期化は visualizer 履歴、GUI thread、latency 再確認、owner handoff に影響するため、
  reset ACK と段ごとの最初の非ゼロ出力を実機で照合してから判断する。
- 診断は worker の通常ログと `audio_epoch` perf events に reset、drain frames、sample rate、
  decoded / user VST 後 / EffeTune 後 / 出力準備の最初の非ゼロ PTS を記録する。
  callback の `first_device_output` は全出力 PCM（underrun 無音を含む）のうち最初の非ゼロ PTS。
  RT callback は既存 buffer lock 内で atomics だけを更新し、ログ出力は pump が行う。
  VST reset は request ID / ACK / latency frames を記録する。マイク測定や OS / device 内の
  buffering を測るものではない。新 profile と通常 profile の open・loop の比較に使う。

forward discontinuity の保証は decoder / SWR の区間境界まで。既存の下流 DSP は同一 serial の
PTS gap を区間別に管理せず、保持した旧 PCM を次 chunk の時刻へ割り当てる制約がある。
v4.3.0 にも存在する別の pipeline 設計事項であり、この loop / seek-to-zero 修正では変更しない。

独立 follow-up レビュー（GPT-6.1 Sol / xhigh）は ACCEPT。実機ノイズ全体の解決という判定ではなく、
再現した decoder / SWR の混入、pause 所有境界と診断の差分に対する判定。

| 実機フィードバック round の検証 | 結果 | ログ |
| --- | --- | --- |
| 修正前の実 decoder seek / UI EOF Full-loop 回帰 | 44.1kHz の次周先頭に旧末尾バーストを検出し失敗（意図した再現） | `target/audio-tail-seek-before.log` / `target/audio-tail-loop-before.log` |
| `cargo test -p mimageviewer --lib audio_tail_seek_fixture_tests` | 6 passed。実素材、timeline、forward gap、cancel、pause / resume / seek、0出力後の回復 | `target/audio-tail-test-0.log` |
| 同上 `video::audio::` / `video::decoder::` / `video::clockless_transcode::` | 60 / 86 / 38 passed、decoder / clockless 各1 ignored | `target/audio-tail-test-1.log`〜`-3.log` |
| 同上 `effetune::composition::` / `video::clock::` | 3 / 14 passed | `target/audio-tail-test-4.log` / `-5.log` |
| 同上 `real_decoder_eof_full_loop` | 1 passed。WAV / MKV × 44.1 / 48kHz × tail4ms / tail10ms / mid4ms × 3周、計36周。全 callback 出力と実消費 PCM を検査 | `target/audio-tail-test-6.log` |
| 同上 `video::tests::native_tick_` / `eos_tick` / `eos_empty_audio_seek` / `reaches_eof_with_real_pump` | 4 / 1 / 1 / 2 passed | `target/audio-tail-test-7.log`〜`-10.log` |
| 関連テスト計（重複する狭い6件を除く） | 210 passed / 2 ignored | 上記 |
| `cargo check -p mimageviewer --bin mimageviewer-core` / 同上 `--features portable` | 両方 exit 0 | `target/audio-tail-check.log` / `target/audio-tail-check-portable.log` |
| `cargo fmt` / `cargo fmt --check` / `git diff --check` / glyph check | 成功。UI文字列変更なし、dangerous glyph 0 | `target/audio-tail-fmt.log` / `target/audio-tail-glyphs.log` |

製品 exe は起動せず、元の実機素材は読み取りだけで、通常 profile の設定・データを操作していない。
検査用の実コピーを `testdata/audio-tail/` に収録した。実機確認手順と perf log の比較方法は
`target/audio-tail-hardware-verification.md` に記録。通常 profile の open ノイズと利用者の出力 rate、
修正後の末尾／先頭の聴感は利用者の確認待ち。

確認用 build は `build-dev.ps1 -PreserveRuntime -WaitForOtherBuildsMinutes 0` を実行したが、
dev-runtime core（PID 112920）と Remote（PID 117288）が使用中のため exit 1 で更新を拒否した。
実行中アプリを停止せず、旧確認用 exe は保持。利用者の通常終了後に同じ script を再実行する。
この invocation だけ `CARGO_BUILD_JOBS=1` / `MSBUILDDISABLENODEREUSE=1` を設定し、終了時に復元した。

## 14. 最小化中の窓表示設定 (§1.312、2026-10-04)

- 利用者が実装を承認。`Settings.effetune_keep_visible_when_minimized` を追加し、既定 false。
  released settings を維持する加算的変更で、旧 JSON field / `settings_kv` key の欠落は serde default false。
  `settings_db` は既存の全 field serialization / deserialization 経路で保存し、schema version は変更しない。
- 環境設定「動画・音声 → 動画」の音響調整グループで入力ピーク保護の次にチェックボックスを配置。
  portable build flavor は描画・検索索引とも cfg で除外する。保存済み設定自体は保持する。
- 表示設定は起動時と Preferences OK 時に既存 `GuiGate` へ atomic で公開する。
  mapping の version 2 / 40 byte は C++ と Rust で検査し、末尾 offset 32 がこの bool の u64 projection。
  値が変わったときだけ既存 host-control worker の `ReconcileVisibility` を通知する。
  GUI thread が現在値を読み `GuiVisibility::reconcile_main` で `Minimized` 理由だけを設定する。
  `RemoteSession` と requested-visible の所有者は変更しない。main HWND 消失時は ON でも抑止する。
- 単純化: 次回最小化までの保留値や追加の表示状態を作る案より、既存 reason-set の即時再評価を採用。
  最小化中の OK も即時反映し、accepted な表示希望だけを非アクティブ復帰する。
  未表示の初回 open / attach は従来どおり最小化イベントで取消し、ON にしただけで新しく開かない。
  エディター再生成・モーダル化は、窓を残す目的と通常操作を妨げるため不要。
- owner HWND 0 / tool window / 非 TOPMOST、既存の復帰 `SW_SHOWNA` を維持。owner・z-order・fullscreen・
  detached・DSP・state capture・`user_hidden` の経路は変更しない。
  tray-only `SW_HIDE` は `IsIconic` ではなく、現行コードも EffeTune を抑止しないため両設定でこの挙動を維持。
  新しい tray reason や tray visibility の共有状態を追加しない。格納前から iconic なら最小化設定に従う。
- 独立設計レビュー: GPT-6.1 Sol / xhigh が §4 と現行コードを確認し、上記所有境界・単純化に同意。
  実装担当が自動検証を所有し、同じ Cargo target への重複実行を避ける。

### 検証記録

対象: `next-effetune-minimized` / base `d3cd4152a` の未コミット差分。実アプリは起動せず、利用者確認待ち。
独立 completion source review は GPT-6.1 Sol / xhigh が ACCEPT（重大な指摘なし）。

- `cmake --build crates/vst3-host/build --config Release`: exit 0。既存と追加の GUI 純状態 compile-time 回帰を含む。
  `scripts/vst3-host-identity.ps1 -ValidateRepo .` も exit 0、source identity
  `b981fb61b8ad8ba64a80eb8e16ad75f98754d2baa3465aef97f3120c9167207e`。
  新 host は 835,584 bytes、SHA256 `582f47aa1f061080a61dc7c3bbfaaac65489ba44c9f49fa2ee2a9105deac1f49`。
- `cargo check -p mimageviewer --bin mimageviewer-core`: exit 0。
- 同 check `--features portable`: exit 0（`target/minvis/check-portable.log`）。
- `cargo test -p mimageviewer --lib effetune`: exit 0、57 passed（`target/minvis/effetune-tests.log`）。
  gate の policy 配送・連番維持、旧 blob / DB key 欠落、DB round trip、起動保存値、Preferences edit → OK → gate、既存 host handler を含む。
- `cargo test -p mimageviewer --lib effetune_minimized --features portable`: exit 0、2 passed
  （`target/minvis/portable-tests.log`）。保存設定の互換性と検索候補の除外を確認。
- `UPDATE_SNAPSHOTS=1 cargo test -p mimageviewer --test ui_snapshot preferences_effetune_input_limit_dark`:
  exit 0、1 passed（`target/minvis/snapshot-update.log`）。更新 PNG を目視確認し、初期値 OFF・入力保護の次の配置・日本語ラベルの欠けなしを確認。
- `python scripts/check_ui_glyphs.py`: exit 0、危険な glyph 0。`cargo fmt --all --check` と `git diff --check` も clean。
- `scripts/test-full.ps1 -SuppressCrashDialogs`: exit 101、launcher build の前提となる
  `target/release/mimageviewer-{core,remote,epub-pdf}.exe` がこの worktree にないため、テスト実行前に停止。
  product test failure ではなく不足前提。ログ `target/minvis/test-full.log`。full gate は未完了。
- `scripts/build-dev.ps1 -PreserveRuntime`: exit 1、同じ worktree の dev-runtime core（PID 10696）が使用中のため更新を拒否。
  実行中 core / Remote を停止せず、既存の確認 exe は更新されていない（`target/minvis/build-dev.log`）。
  利用者の通常終了後に同じ script を再実行する。今回の指示に従い build-dev を選択し、C++ bridge 自体は先に再 build 済み。

- `cargo test -p mimageviewer --lib`: exit 101、10,247 passed / 1 failed / 51 ignored、644.76 秒
  （`target/minvis/core-lib-tests.log`）。唯一の失敗は未変更の
  `fs_page_load_scheduler::tests::slow_read_and_decode_stay_within_process_budget` の最終 `peak == 3`（実際 2）。
  scheduler が Running を公開してから worker が active/peak を更新する前に gate を解放できるテスト同期の隙間を、
  実装者・独立 reviewer が確認した。permit 上限違反ではなく、機能差分の ACCEPT は維持。full-lib 全体は green と扱わない。
  同じテストを `-- --exact` で孤立再実行すると exit 0 / 1 passed（`target/minvis/scheduler-isolated.log`）。
  別の test-only follow-up は gate 解放前に `stats == 3 && active == 3` を待つ。期待値は緩和せず、今回の差分には入れない。
- `cargo test -p mimageviewer --test ui_snapshot`: 並列実行は 17 件成功後に `0xc0000005 / STATUS_ACCESS_VIOLATION` で異常終了
  （`target/minvis/snapshot-compare.log`）。原因は未特定で、snapshot 比較失敗とは区別して記録する。
  同じ比較を `-- --test-threads=1` で直列実行し、exit 0 / 60 passed（25.62 秒、`target/minvis/snapshot-compare-serial.log`）。
  更新した EffeTune snapshot も更新モードなしで一致を確認した。

実アプリ起動・commit は行っていない。引き継ぎは `target/minvis-msg.txt`（英語 subject、UTF-8 BOM なし）。
確認用 core を更新するには利用者がこの worktree の dev-runtime core / Remote を通常終了してから
`scripts/build-dev.ps1 -PreserveRuntime` を再実行する。旧 exe は今回の機能確認には使わない。
利用者確認は、OFF の最小化・復帰、ON の最小化中の窓操作・復帰、
Remote と最小化の両解除順、ユーザーが閉じた窓の非復帰、tray-only 格納、再起動後の設定保持。
