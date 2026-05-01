# Codex レビュー依頼: VST3 プラグイン内部状態の永続化 (`getState/setState chunk`)

## 概要

`docs/vst3-todo.md` の P1 タスク「プラグイン内部状態の永続化 (= EQ カーブ等の保存)」を実装。
プラグインの `IComponent::getState` / `setState` chunk を base64 で IPC して `settings.json`
に保存し、再起動・チェーン再構築・VST3 OFF/ON 切替を跨いで EQ カーブ等が復元される。

## レビュー基準コミット

```bash
git diff e4683ce..HEAD
```

(= `e4683ce fix(vst3): Codex P2/P3 反映 (path-based lookup + preferences merge)` 以降の差分)

このセッションで以下 2 コミット:

- `26c7c30 feat(vst3): プラグイン内部状態の永続化 (getState/setState chunk)` (本実装)
- `feb69c7 refactor(vst3): simplify レビュー反映 (helper / RAII / 並列化)` (= simplify 後)

## 背景

`Vst3PluginEntry::state: Option<String>` フィールドは settings.rs に存在していたが
**bridge protocol 未実装** のため使われていなかった (= 起動毎に全プラグインデフォルト値)。
ユーザー要望 (2026-04): 「EQ カーブ等を再起動後も維持してほしい」。

## 実装概要

### 1. Bridge protocol 追加 (`src/video/dsp/bridge.rs` / `crates/vst3-host/include/protocol.h`)

| 方向 | 種別 | 名前 | フィールド |
|---|---|---|---|
| Rust → C++ | `Cmd` | `QueryState` | (無し) |
| Rust → C++ | `Cmd` | `RestoreState` | `state: String` (= base64) |
| C++ → Rust | `Event` | `PluginState` | `state: String` (= base64) |

`MAX_CONTROL_MSG_SIZE` を 64 KB → 4 MB に拡張 (= ML 系 / preset 内蔵 plugin の chunk が
数百 KB に達する想定。base64 化で +33% overhead を含めても余裕)。Rust 側 `read_event_blocking`
と C++ 側 `protocol.h` 双方にコメント相互参照を入れて同期維持を明記。

### 2. C++ 側 (`crates/vst3-host/src/plugin_loader.cpp`)

```cpp
bool PluginLoader::query_state(std::vector<uint8_t>& out_bytes);
bool PluginLoader::restore_state(const std::vector<uint8_t>& bytes);
```

`MemoryStream` 経由で `IComponent::getState` / `setState`、`IEditController::setComponentState`
で UI 同期。`restore_state` は **RAII ProcessingPauseGuard** で `setProcessing(false) → setState
→ setProcessing(true)` を必ず対称的に走らせる (= setState 失敗時の re-enable 漏れ防止)。

### 3. Base64 helpers (`crates/vst3-host/src/main.cpp`)

外部依存を増やさないため最小実装 (RFC 4648、~50 行)。VSTGUI に
`Base64Codec` があるが現在 bridge は VSTGUI4 とリンクしていないので採用しない。

### 4. Rust 側 (`src/video/dsp/`)

- `Bridge::query_state_sync(timeout)`: `Cmd::QueryState` 送信 → `Event::PluginState` 待機
  (deadline loop、unexpected event は drop してログ)
- `DspBridge::snapshot_all_plugin_states() -> Vec<(String, String)>`: 全 Loaded slot を
  **並列に** query。bridge ごとに thread spawn で `query_state_sync` 同時実行
  (= worst case 10 sec → ~1 sec)
- `DspBridge::add_plugin(..., initial_state: Option<&str>)`: Loaded event 直後・pre-warm 前に
  `Cmd::RestoreState` を fire-and-forget 送信 (= warm-up silence 処理時には既に正しい係数
  で動作 → 初回処理クリック軽減)

### 5. App 側 (`src/ui_dialogs/vst3_actions.rs`)

- `find_vst3_entry_mut(path) -> Option<&mut Vst3PluginEntry>` 共通 helper を追加
  (= 6 箇所の duplicate を集約、Codex P2 path-based lookup 流儀に統一)
- `snapshot_vst3_states_into_settings() -> usize`: bridge から snapshot を取得して
  settings に path 一致で merge、戻り値は更新数 (= 0 なら save 不要判断)
- 保存トリガ:
  - `App::on_exit` (= アプリ終了直前)
  - VST3 OFF へのトグル直前 (preferences ダイアログ OK)
  - `kick_off_vst3_chain_rebuild` 内 (= bridge teardown より前)

### 6. 触ったファイル

```
crates/vst3-host/include/plugin_loader.h    + query_state / restore_state 宣言
crates/vst3-host/include/protocol.h         MAX_CONTROL_MSG_SIZE 64KB → 4MB + 同期コメント
crates/vst3-host/src/plugin_loader.cpp      + query_state / restore_state 実装 (RAII guard)
crates/vst3-host/src/main.cpp               + base64 encode/decode + cmd handler 2 件
src/video/dsp/bridge.rs                     + Cmd / Event variant + query_state_sync
src/video/dsp/mod.rs                        + add_plugin に initial_state、+ snapshot_all_plugin_states (並列)
src/ui_dialogs/vst3_actions.rs              + find_vst3_entry_mut / snapshot helper / kick rebuild 改修
src/ui_dialogs/vst3_manager.rs              find_vst3_entry_mut 適用 (重複削減)
src/ui_dialogs/preferences.rs               VST3 OFF 直前の snapshot/save 追加
src/app.rs                                  on_exit に snapshot/save / add_plugin 呼出更新
docs/vst3-todo.md                           完了マーク
```

## 設計判断ポイント (= 確認したい点)

### A. `Cmd::RestoreState` を fire-and-forget にしている

`add_plugin` は Loaded event 受信後、pre-warm の前に restore を送信するが ack を待たない。
理由は:
- pre-warm の `push_audio` / `pull_audio` は **shared memory + sig** (= JSON IPC とは別経路)、
  bridge の control thread が `Cmd::RestoreState` を消費した後に audio_loop が pre-warm 入力を
  処理するため、順序関係は保たれる
- 失敗時は bridge 側で `Event::Error` が発行されて event_rx に流れる
  → ログには出るが起動は中断しない

ただし pre-warm の最中に restore_state がまだ control thread でキューに入ったままだと
warm-up 処理は古い係数 (= デフォルト) で走る。RAII guard 内で `setProcessing(false)` するので
warm-up の `process()` は失敗 (= `kResultNotInitialized` 系) して push が空回りするだけだが、
ユーザー体感的に問題ないか?

代替案: ack を待つ (`restore_state_sync` 化) → pre-warm 前に必ず適用済みを保証。
コストは IPC 1 往復 (~ms) だが、ack を返す cmd プロトコル拡張が必要。

### B. `setProcessing(false) → setState → setProcessing(true)` の妥当性

VST3 仕様上 `setState` は active 状態でも許可されているが、内部状態の途中書換による
1 ブロック分のクリックを避けるため一時停止する。RAII guard で必ず再有効化される。

懸念:
- `setProcessing(false)` 中に audio thread が `process()` を呼ぶと bridge audio thread 側で
  どう扱われるか? (= bridge `audio_pipe` から read した audio block を processor に渡すが、
  setProcessing(false) 中の `processor->process()` は no-op で 0 を返す)
- 問題が起きるとすればこの period 中の audio が silence でスキップされる程度。
  load 直後の pre-warm 段階なのでユーザー体感ゼロのはず

### C. `snapshot_all_plugin_states` の並列化

Codex efficiency review の助言で sequential → thread spawn 並列化を実装。
- 各 bridge は別 stdin/stdout + 別 shm なので独立に IPC 可能
- thread spawn コストは ~50us、10 plugins で ~500us、worst case 1秒の wait と比べて誤差
- ただし `JoinHandle<(String, Result<String, String>)>` の closure capture が `Arc<Bridge>` を
  move する必要がある (= clone 済み)。問題ないか?

### D. `MAX_CONTROL_MSG_SIZE` 4 MB の妥当性

base64 オーバーヘッド +33% 込みで 4 MB ≈ 3 MB の生 state まで対応。Pro-Q 4 / Insight 2 等の
EQ / アナライザ系は数 KB 規模。preset 内蔵 plugin で数百 KB、ML 系 (Acustica 等) で 1-2 MB
の例あり。それを超える場合 bridge stdin/stdout JSON では非効率なので、shared memory 経由
のチャンク転送が必要だが、現状 4 MB で実用上困らない判断。

### E. on_exit の blocking 時間

`snapshot_all_plugin_states` を on_exit で呼ぶ。並列化済みなので worst case 1 秒。
eframe 0.33 の on_exit に shutdown deadline の documented limit は見当たらない。
1 秒程度なら OS 側 (=Windows) の WM_CLOSE → WM_QUIT 処理時間内に収まる。

### F. 保存トリガが 3 箇所 (on_exit / VST OFF / chain rebuild) で網羅性

- 通常終了: on_exit
- VST3 機能を停止: preferences で OFF トグル
- チェーン編集: preferences で path/順序変更 → kick_off_vst3_chain_rebuild

足りていないシナリオはあるか? 例えば:
- アプリがクラッシュ: 保存タイミング無し → 当然 default に戻る (= 諦め)
- preferences ダイアログ Cancel: snapshot は走らない (= 設定変更を破棄したい意図と整合、OK)
- VST3 OFF → ON: 再 enable 時に既存 settings の state で restore される (= OK)
- 単発 GUI × ボタン: state は変わらない (= GUI 表示状態だけ user_hidden で別途永続化済み)

## 検証

```bash
cargo check --bin mimageviewer-core   # OK (commit feb69c7)
cmake --build crates/vst3-host/build --config Release  # OK
```

実機検証 (= 手作業):

1. mIV 起動 → プラグインロード → Pro-Q 4 で EQ カーブを編集 → mIV 終了
2. settings.json を覗いて `vst3_plugins[].state` が空でない base64 であることを確認
3. mIV 再起動 → Pro-Q 4 GUI を開く → 編集した EQ カーブが復元されている
4. VST3 OFF → ON のトグルでも維持されること
5. preferences でチェーン順序を入れ替えて OK → 各 entry の state が新しい順序で復元

## 期待するレビューフォーカス

1. **[P1] バグ**: 上記設計判断 A-F のいずれかに見落としや race があるか?
2. **[P2] レイヤ違反**: bridge protocol / DspBridge / App 間の責務分離が破れていないか?
3. **[P2] 起動速度**: `add_plugin` 内で `Cmd::RestoreState` を pre-warm の前に送るが、
   ML 系 plugin で setState が ~50ms かかるケースがあった場合の挙動 (= warm-up 中に
   並列で適用されるが、warm-up 後の最初の実 audio までに完了する保証は?)
4. **[P3] 改善案**: VSTGUI Base64Codec を採用しないことの妥当性確認、または
   実装上の他の改善余地

返答は P1/P2/P3 サマリ形式で。
