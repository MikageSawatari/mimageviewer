# backlog §1.93 — 動画補正スロットが egui 経路に無い

対象: [next-release-backlog.md](../next-release-backlog.md) §1.93。§1.92 (別ウィンドウの
<kbd>Z</kbd>) を直したときの同型監査で見つけた穴。**§1.92 と同じ構造の欠陥**なので、
同じ形で閉じる。

## 0. 確定している事実 (source inspection、2026-08-19)

`FsVideo` コンテキストの action は **39 個**。うち egui 経路が処理しているのは:

- `handle_video_input` ([ui_fullscreen.rs:33073](../../src/ui_fullscreen.rs:33073)) が **27 個**
- `VideoPrevFile` / `VideoNextFile` は別位置 ([ui_fullscreen.rs:18345](../../src/ui_fullscreen.rs:18345)、
  音声と共通の `consume_first_action`) で **2 個**

残る **10 個 = `VideoAdjustSlot1..10`** が egui 経路に存在しない。dispatch は
[native_video.rs:7142](../../src/app/native_video.rs:7142) の native key handler にしかない。

`VideoAdjustSlot1..10` = 「動画補正スロット N を読み込む」、既定 chord は
<kbd>Ctrl</kbd>+<kbd>1</kbd>..<kbd>0</kbd> と Numpad 版
([keymap.rs:5489](../../src/keymap.rs:5489) の `ctrl_digit_pair`)。保存側は HUD の
パネル経由 (`VideoAdjustSaveSlot`) だけで、キー割り当ては元から無い。これは意図どおり
(action 名も「読み込む」) なので**保存にキーを足さない**。

§1.92 で「別ウィンドウの動画再生中に他アプリから戻るとキーが egui へ届く」ことを
実測で確定させた。同じ状態で <kbd>Ctrl</kbd>+<kbd>1</kbd> が無反応になっているはず。

## 1. 利用者判断 (2026-08-19、確定)

**音声モードでは動画補正スロットを読み込めないようにする。**

これは現状の native gate
(`!self.video_audio_mode_hides_native_presenter_for(fs_idx)`、
[native_video.rs:7141](../../src/app/native_video.rs:7141)) と一致する。したがって
**native 側の挙動は変えない**。egui 側へ同じ述語を置けば両経路が揃う。

補足 (実装時に確認すること): `handle_video_input` の呼び出し自体が
`is_video_fs && !fs_music_view_active` で gate されている
([ui_fullscreen.rs:17900](../../src/ui_fullscreen.rs:17900)) ので、通常の音声モード
(音楽ビュー) ではこの関数へ入らない。述語は重複するが、**外側の gate が変わっても
規則が崩れないよう明示的に置く**。その意図をコメントに残すこと。

## 2. やること

### 2.1 slot action の一覧を 1 箇所にする (ここが構造的な要点)

現在 `VideoAdjustSlot1..10` の配列は [native_video.rs:7142](../../src/app/native_video.rs:7142)
にベタ書きされている。egui 側にもう 1 つ配列を書くと、**将来スロットを増やしたときに
片方だけ増える**。§1.92 と §1.93 が同型で並んだ理由がまさにこれなので、繰り返さない。

`keymap.rs` に `pub const VIDEO_ADJUST_SLOT_ACTIONS: [KeyAction; 10]` を置き、
native / egui / 既存テスト ([keymap.rs:8966](../../src/keymap.rs:8966) の配列) の
3 箇所から参照する。要素数と順序が slot index に対応することを doc comment に書く。

### 2.2 egui 経路へ mapping を足す

`handle_video_input` の中、§1.92 で足した `VideoToggleAudioMode` ブロックの直後に置く。
ガードは同じ 4 つ + 音声モードの述語:

- `self.fs_context_menu_idx.is_none()`
- `!ctx.wants_keyboard_input()`
- `!self.any_modal_dialog_open_for_fullscreen_keys()`
- `!self.normalize_scan_is_modal_for_current_player(fs_idx)`
- `!self.video_audio_mode_hides_native_presenter_for(fs_idx)`

IME は関数先頭で既に弾かれているので追加しない。消費は `consume_action_no_repeat`
(native 側も `!key.repeat` なので repeat 挙動が揃う)。

**所有権の順序を守る**: 自分の context でないキーは消費しない。上のガードは
**consume より前**に評価すること (§4.2 で踏んだ罠。[keymap-spec.md](../keymap-spec.md) の
契約)。

発火時の処理は native 側と同じ 2 つ:

```rust
self.load_video_adjust_slot(slot_idx);
self.request_native_video_hud_repaint(ctx);
```

`load_video_adjust_slot` ([native_video.rs:741](../../src/app/native_video.rs:741)) は
settings 更新 + `sync_native_video_grade` + save + toast まで持っているので、
**呼び出し側で追加の同期処理を書かない**。空スロットのときのトーストもこの中にある。

### 2.3 診断の 1 行

§1.92 で入れた native 側の key 診断と揃うよう、egui 経路の発火にも
**どの経路で・どの slot が読み込まれたか**が分かる記録を残す。既存の
`VideoAudioEnterSource` と同じ発想で、経路を typed に区別できる形にする。
高頻度イベントを新設しない (発火時の 1 行だけ)。

## 3. やらないこと

- **native 側の gate を変えない。** VST GUI 表示中の音声モード
  (`video_audio_vst_active_for` が true) では `video_audio_mode_hides_native_presenter_for`
  が false になり、slot の読み込みが通る。これは現状の挙動で、**今回は揃えるのが目的**
  なので egui 側も同じにする。ここを締めるかどうかは別途利用者判断が要る (報告に書くこと)。
- 保存 (`VideoAdjustSaveSlot`) にキー割り当てを足さない。
- `KeyContext` の実行時モデルを一般化しない。
- 時間窓で競合を吸収しない (憲法 §2 規則 5)。
- 他の 27 action の処理順を動かさない。

## 4. テスト

`src/app/tests.rs` に `handle_video_input` を直接呼ぶ既存テスト
([tests.rs:50634](../../src/app/tests.rs:50634) 付近) があるので、同じ形で足す。

1. **parity**: item = Video、fullscreen で <kbd>Ctrl</kbd>+<kbd>1</kbd> を egui へ入れて
   `handle_video_input` を通すと、slot 1 が読み込まれる (`settings.video_adjustments` が
   slot の内容になる)。**修正前に落ちることを確認して報告する** (ソース読解だけで終えない)。
2. **音声モードでは読み込まない**: `video_audio_mode == Some(idx)` の状態で同じキーを
   入れても `video_adjustments` が変わらないこと。
3. **空スロット**: 未保存の slot を指すキーで `video_adjustments` が変わらないこと
   (既存の空スロット分岐が効いていることの確認)。
4. **所有権**: 上のガードが立っているとき (例: modal が開いている)、キーが**消費されずに
   残る**こと。§4.2 と同じ回帰。
5. **一覧の単一化**: `VIDEO_ADJUST_SLOT_ACTIONS` の要素数と、`KeyAction` 側の
   `VideoAdjustSlot*` の総数が一致することを test で固定する (片方だけ増えたら落ちる)。
6. mutation: 1, 2, 4 のガード / mapping を削除または反転して**実際に落ちることを確認**し、
   結果を報告に含める (「落ちるはず」ではなく実行結果)。

## 5. 凍結ルール

detached viewer の発火面に触れるので
[detached-rework-plan.md](../detached-rework-plan.md) §2 (憲法) の対象。着手前に §2 を読む。
完了時に §11 へ、触れた範囲と「これは症状パッチではなく構造的修正である」根拠を追記する
(§1.92 の追記が直前の前例)。

## 6. ドキュメント

- [keymap-spec.md](../keymap-spec.md): 動画コンテキストのキーが native / egui の
  両経路へ届き得ること、両方に mapping が要ることを §1.92 の記述に続けて明記する。
- [video-architecture.md](../video-architecture.md): §1.92 で書いた
  `toggle_video_audio_mode` の節に、slot も同じ扱いであることを足す。
- [next-release-backlog.md](../next-release-backlog.md) §1.93 に結果を追記して閉じる
  (エントリ末尾に追記。冒頭の記述を消さない)。

## 7. 機械チェック

```
cargo fmt --all && cargo fmt --all -- --check
cargo check -p mimageviewer --bin mimageviewer-core
.\scripts\test-full.ps1
.\scripts\build-dev.ps1
```

commit / stage はしない。ブランチは `master`。
報告には 4.1 の修正前 fail、mutation 結果、変更ファイル一覧、§3 の VST サブケースの
確認結果を含める。
