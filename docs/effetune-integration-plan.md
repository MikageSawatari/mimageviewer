# EffeTune (Mixwright) 組み込み — サンプル版設計

状態: 設計第 1 版 (2026-09-27)。サンプル版 (試験用) の範囲を定める。配布版の範囲は §9。

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
  - 起動時: 保存済みの状態が「有効」(§4) のとき
  - 実行中: 利用者がツールバーのボタンを押したとき
  起動後は、エフェクトを全部オフにしても終了までは素通しで経由し続ける。
  経路へ出入りするのは「起動」の 1 回だけで、途中で抜いたり戻したりしない。
- ON/OFF の正本は **Mixwright の状態 1 か所**。mIV 側に独立した ON/OFF 設定を持たない。
  止めたいときは Mixwright の全体バイパスを使う。

## 1. 既存コードの前提 (コード確認済み)

- ローカルの `DspBridge` は App のフィールド (`src/app.rs` `dsp_bridge: Arc<DspBridge>`)。
  global static ではない。リモート配信は 1 セッションにつき別の `DspBridge` を 1 つ作る
  (`ClocklessAudioProcessing::with_remote_vst3`、`src/video/clockless_transcode.rs`)。
  **同じプロセス内に DspBridge が複数あってよい構造は既にある。**
- `DspBridge::enable()` は host exe を展開するだけで、プロセスは最初の `add_plugin` で起動する。
- プラグインのパスは `.vst3` bundle ディレクトリのままでよい (SDK が中の DLL を解決する)。
- 状態は base64 文字列 (`query_state_sync_slot` → `Event::PluginState`)。
  Mixwright の `getState` は **JSON をそのまま**書く (`StateCodec::encode`)。
- 音声 pump (`src/video/audio.rs` `run_pump`) は `Option<Arc<DspBridge>>` を引数で受け取り、
  `process_block` → `total_latency_samples` → 失敗時は dry へ fallback、連続 3 回失敗で
  `disable_with_reason`。その後に手動ブースト・安全リミッター
  (`limiter_active = vst_chain_active || pre_limiter_gain > 1.0 || normalize_boost_active`)。
  動画と音楽は同じ pump を通る (`VideoPlayer::open*` の呼び出しは 2 か所)。
- GUI は bridge プロセスが作る `WS_POPUP|WS_EX_TOOLWINDOW` の container に attach する。
  owner は `DspBridge::current_gui_owner_hwnd()` が決め、`fullscreen_owner_hwnd` が未設定なら
  `main_hwnd` 系になる。GUI スレッドは `OleInitialize` 済み (WebView2 の要件を満たす)。
- 共有メモリ名 `miv-vst-shm-{pid}-{µs}` は同一マイクロ秒で衝突し得る (DspBridge が 2 つ同時に
  pipe を開くと顕在化する)。

## 2. 構成

```
decode → normalize → [ユーザー VST3 チェーン: dsp_bridge] → [EffeTune: effetune bridge]
       → 手動ブースト → 安全リミッター → 出力
```

### 2.1 所有者 `EffetuneController` (新規、`src/effetune/`)

App が 1 つ持つ。EffeTune に関する状態の唯一の所有者。

```rust
enum EffetuneRuntime {
    Unavailable(UnavailableReason), // bundle 無し / AVX2・FMA 無し / VST3 非対応環境
    Idle,                           // 起動していない (経路に入っていない)
    Loading { open_gui_when_ready: bool },
    Running,                        // 経路に入っている。終了まで戻らない
    Failed(String),                 // 起動失敗 or 連続失敗で停止。この実行中は再起動しない
}
```

- `bridge: Arc<DspBridge>` を持つ (チェーンは Mixwright 1 スロットのみ)。
- `effective: EffectiveState` (§4) を持つ。ランプ表示に使う。
- `Running` から `Idle` へは戻らない (「一度起動したら常に経由」)。
- `Failed` への遷移: ロード失敗、または pump での連続失敗による `disable_with_reason`。
  音声は dry で流れ続ける。理由はツールチップに出す。

### 2.2 音声経路への受け渡し

`dsp_bridge: Option<Arc<DspBridge>>` を運んでいる引数を、型付きの束に置き換える:

```rust
pub struct AudioDspChain {
    pub user: Option<Arc<DspBridge>>,     // 既存のユーザー VST3 チェーン
    pub effetune: Option<Arc<DspBridge>>, // EffeTune。Running/Loading のときだけ Some
}
```

- `VideoPlayer::open*` → `audio::start` → `run_pump` の引数を `AudioDspChain` にする。
- pump 内では、ユーザーチェーンの処理の直後に、同じ形 (失敗カウンタ・fallback・latency) で
  EffeTune を処理する。**失敗カウンタは bridge ごとに別に持つ。**
- 総 latency = ユーザーチェーン + EffeTune。既存の `current_pdc_latency_secs` に合算し、
  上限 `MAX_PDC_LATENCY_SECS` は合計に掛ける。
- 安全リミッター: `effetune_in_path` (EffeTune の bridge が enabled かつスロットが active) を
  条件に加える。EffeTune が経路にいる間は常にリミッターを通す (決定的な規則にする)。
- シーク時の `reset_plugins_sync` を EffeTune にも行う。
- 再生途中で EffeTune が起動した場合 (ボタン操作): 次の再生オープンを待たず、既に開いている
  pump にも反映する必要がある。**方式は Sol レビューで決める** (候補: pump が
  `Arc<ArcSwapOption<DspBridge>>` のような共有スロットを毎ブロック読む / 次の open から有効)。
  サンプル版では「共有スロットを毎ブロック読む」を第一案とする。
  latency が途中で増えるのは既存の PDC 追従 (decoder が毎回 `vst3_pdc_latency_secs` を読む) に任せる。

### 2.3 起動時の読み込み

- 起動時、保存済み状態を worker で読み、`EffectiveState::Effective` または `Unparseable` なら
  `Loading` に入って worker で `enable()` → `add_plugin(bundle, ...)`。
  サンプルレートと block size は既存 `run_vst3_startup_load` と同じ値・同じ決め方を使う。
- 起動時の読み込み中は、既存の VST3 と同じくメディアのオープンを遅延させる
  (`vst3_startup_load_pending()` の判定に EffeTune の起動時ロードを含める)。
  **ボタン操作による途中起動では遅延させない** (再生中の音はそのまま流れ、準備ができたら経路に入る)。
- `vst3_enabled` とは独立。VST3 が無効でも EffeTune は動く。

## 3. ウィンドウ

- ツールバーに新しいセクション `ToolbarSectionId::EffeTune` を追加する。既存の toggle 例
  (`FolderTree`、`egui::Button::selectable(active, ...)`) に倣う。
  - ランプ (selectable の active) = `Running` かつ `effective == Effective`。
  - クリック:
    - `Idle` → `Loading { open_gui_when_ready: true }` (起動して、準備ができたら GUI を表示)
    - `Running` → GUI の表示／非表示を切り替え
    - `Loading` → 何もしない (ツールチップに「準備中」)
    - `Unavailable` / `Failed` → 無効表示。理由をツールチップに出す
  - 既定で表示する。ツールバーの表示設定メニューにも項目を足す。
- GUI の owner は **メインウィンドウ**。EffeTune の bridge には `set_main_hwnd` だけを行い、
  `fullscreen_owner_hwnd` は設定しない。TOPMOST にもしない。
- フルスクリーン終了時に既存 VST GUI を隠す処理 (`set_existing_guis_owner_to_main` 等) は
  `dsp_bridge` だけが対象で、EffeTune には作用しない (作用させない)。
- 利用者が × で閉じた (`GuiUserHidden`) ときは非表示にするだけで、経路からは外さない。
  `pump_gui_signals` を EffeTune の bridge についても毎フレーム呼ぶ。
- **既知の制約 (サンプル版)**: 同じモニターでフルスクリーンにすると EffeTune のウィンドウは
  裏に隠れる。別モニターに置けば見える。フルスクリーン中の扱いは試験後に決める。

## 4. 状態の保存と「有効」の判定

### 4.1 保存先

- Mixwright の状態 JSON: `data_dir/effetune/mixwright-state.json` (base64 を decode した生 JSON)。
  tmp へ書いて rename で置き換える。**UI スレッドでは書かない**。
  settings.db に載せない理由: 状態は IR 等で MB 級になり得るが、`settings_kv` は保存のたびに
  全行を書き直すため。
- ウィンドウ位置・サイズ: settings に `effetune_gui_pos: Option<(i32,i32)>`、
  `effetune_gui_size: Option<(u32,u32)>` を追加し、`overwrite_non_preferences_from` でも
  コピーする (環境設定の OK で巻き戻らないように)。
- 未リリース機能なのでマイグレーションは不要。

### 4.2 状態を取り込むタイミング

- GUI を非表示にしたとき (× を含む)
- GUI 表示中は 2 秒ごと (worker で `query_state_sync_slot`、同時に 1 本まで)
- 終了時 (既存の `snapshot_all_plugin_states_with_deadline` と同じ期限の考え方)

取り込んだら `effective` を更新し、ファイルへ保存する。

### 4.3 `EffectiveState`

```rust
enum EffectiveState {
    Effective,            // 音が変わり得る設定がある
    Inert,                // 空、または全体バイパス
    Unparseable(String),  // 形式が読めない
}
```

判定規則 (Mixwright の `aggregatePipelineLatency` と同じ有効判定に揃える):
- `masterBypass == true` → `Inert`
- `currentPipeline` ("A"/"B") のパイプラインを見る。section プラグインの `enabled` が false の
  区間は無効。区間外または有効区間で `enabled == true` のプラグインが 1 つでもあれば `Effective`。
- どれにも当たらなければ `Inert`。
- JSON として読めない、または必要なキーが無い → `Unparseable(理由)`。

`Unparseable` は起動条件では **起動する側**に倒す (利用者が設定した音を黙って失わない)。
黙った fallback にしないため、理由を log に残し、ツールチップにも出す。
ランプは `Unparseable` では点けない。

## 5. リモート配信

- EffeTune が `Running` のとき、リモート配信セッションにも同じ位置で適用する。
- **リモート側は既存のセッション用 bridge 1 つに、ユーザーチェーンの後ろへ Mixwright を足す**
  (プロセスを増やさない)。`remote_clockless_audio_processing` (`src/remote_ipc/ui.rs`) の
  「`!vst3_enabled || plugins.is_empty()` なら VST なし」の早期 return を、EffeTune を含めた
  条件に直す。
- 状態: セッション開始時点のローカルの Mixwright の状態を使う。取得は **配信 worker の
  `prepare_once` の中**で、ローカルの EffeTune bridge へ期限付きで問い合わせる
  (UI スレッドで待たない)。取れなければ保存済みの状態を使い、その旨を既存の warning 経路
  (`ClocklessVstStatus`) で出す。
- **既知の制約 (サンプル版)**: 配信中に EffeTune の設定を変えても、リモート側には次の配信
  セッションまで反映しない。

## 6. bundle の配置 (サンプル版)

- `vendor/effetune-mixwright/EffeTune Mixwright.vst3` (gitignore 済み、v0.11.1)。
- `scripts/build-dev.ps1` が `target\dev-runtime\effetune\EffeTune Mixwright.vst3` へ
  ディレクトリごとコピーする (変更時のみ)。
- 実行時は `native_assets::bundled_root().join("effetune").join("EffeTune Mixwright.vst3")`
  を探す。無ければ `Unavailable(BundleMissing)`。
- `is_x86_feature_detected!("avx2") && is_x86_feature_detected!("fma")` が偽なら
  `Unavailable(CpuUnsupported)`。
- release / portable / launcher への埋め込みはサンプル版では行わない (§9)。
  build.rs の必須チェックにも入れない (無くてもビルドは通り、ボタンが無効になるだけ)。

## 7. 付随修正

- 共有メモリ・イベント名に process 内の atomic 連番を足し、同一マイクロ秒の衝突を無くす
  (bridge.rs)。2 つの DspBridge が同時に pipe を開く今回の構成で顕在化するため。

## 8. テスト

- `EffectiveState` の判定: 空 / 全体バイパス / A と B の切り替え / section 無効区間 /
  壊れた JSON / 未知の形式。fixture は Mixwright の `StateCodec` の出力形に合わせる。
- `EffetuneController` の状態遷移: Idle→Loading→Running、Running から Idle へ戻らない、
  Loading 失敗→Failed、Unavailable ではクリックで何も起きない、起動時条件
  (Effective / Unparseable で起動、Inert・状態なしで起動しない)。
- pump の合成規則 (純関数に切り出す): 総 latency の合算と上限、リミッター発動条件、
  EffeTune の失敗カウンタがユーザーチェーンと独立であること。
- リモート: plugin 列の組み立て (ユーザー VST 無効 + EffeTune Running でも VST 経路が作られる、
  Mixwright が末尾に来る)。
- settings: 新フィールドが `overwrite_non_preferences_from` で保持される。
- 共有メモリ名の衝突しないこと (連番)。

実行時の確認 (GUI が開くか、音が処理されるか、ビジュアライザーが動くか、空パイプラインでの
遅延と音の透過性、初回の既定パイプライン) は、利用者の実機確認で行う。

## 9. サンプル版の範囲外 (配布版で決める)

- release / portable / インストーラへの同梱方法 (zip 埋め込みと APPDATA 展開、または loose 配置)
- Mixwright の署名 (配布物は未署名)、THIRD-PARTY-NOTICES の転載、商標注記
- マニュアル・製品ページ・privacy (Mixwright の WebView データの保存先が mIV の data_dir の外に
  なる点を含む)
- フルスクリーン中の EffeTune ウィンドウの扱い
- 配信中の状態変更のリモート反映
- 作者への連絡
