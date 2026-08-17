# backlog §4.2 — 音声モードで Z が効かない (FsZoomMode が他コンテキストのキーを先に消費している)

対象: backlog §4.2 (利用者報告、v2.10.0 から未解決)。症状は「動画→音声モードに入ったあと、
Z で動画表示へ戻れない」。逆方向 (動画→音声) は native presenter 側が処理するので効く。

前セッションで原因を特定済み。この brief は**原因修正**であり、症状パッチではない。

## 0. 原因 (ソースで確定)

`update_fs_zoom_mode_keys` は Z のエッジを**コンテキスト判定より前に消費している**。

```
src/ui_fullscreen.rs:6258   take_key_hold_edges(ctx, KeyAction::FsZoomMode)   ← ここで Z を消費
src/ui_fullscreen.rs:6272   if !self.fs_zoom_mode_context_ok(fs_idx) { ... }   ← 動画/音声の除外はここ
```

呼び出しは `handle_fs_key_input` の [ui_fullscreen.rs:17402](../../src/ui_fullscreen.rs:17402)。
音楽ビューが Z を取るのは同じ関数の [ui_fullscreen.rs:17834](../../src/ui_fullscreen.rs:17834)
(`consume_action_no_repeat(KeyAction::VideoToggleAudioMode)`) で、**400 行あと**。
`take_key_hold_edges` は Win32 経路で egui 側の双子イベントも claim する
([keymap.rs:6807](../../src/keymap.rs:6807)) ので、両方のキューから消える。
結果、音楽ビューが尋ねる時点で Z は残っていない。

`fullscreen_shortcut_event_summary` は `ctx.input` で読むだけなので無罪。

### 0.1 なぜ「判定前に消費」が今の形になっているか

これは意図的な設計で、コメントが 6256-6257 にある。`fs_zoom_mode_context_ok`
([ui_fullscreen.rs:6182](../../src/ui_fullscreen.rs:6182)) は**性質の違う 5 つの除外を
1 つの bool に畳んでいる**:

| 除外理由 | item | Z の正当な持ち主 | 今の挙動 |
| --- | --- | --- | --- |
| 連結表示 (`is_paged` false) | 画像 | **FsZoomMode** | トースト「[全画面ズーム] ページ単位表示でのみ使用できます」 |
| パノラマ / 分析 / 編集 / 比較 | 画像 | **FsZoomMode** | 無言 no-op (状態リセットのみ) |
| 動画 / 音声 | Video / Audio | **VideoToggleAudioMode** | 無言 no-op ← **バグ** |

上 4 行は「持ち主は FsZoomMode で正しいが今は使えない」状態で、先に消費して理由を出すのは
正しい。**最後の 1 行だけが「持ち主が別」**。

keymap 自身がそう宣言している — [keymap.rs:5499](../../src/keymap.rs:5499) に
`Z: 動画→音声モードのトグル。画像の Z (FsZoomMode) とは別コンテキスト (FsVideo) なので競合しない`
とあり、`context()` は `FsImage` ([keymap.rs:4613](../../src/keymap.rs:4613)) と
`FsVideo` ([keymap.rs:4652](../../src/keymap.rs:4652)) に分かれている。
**宣言上は競合しないのに、実行時に FsImage 側が判定前に取ってしまう**のが不具合の正体。
KeyContext は割り当て時の衝突検査と help 表示にしか使われておらず、実行時に
「今どのコンテキストか」を表す述語がコードに存在しない。

## 1. やること

**キーの所有権判定を、消費より前の独立した述語にする。** 5 つの除外を丸ごと前に動かすのは
**しない** (連結表示のトーストが消え、消費されない Z が下流へ漏れる)。

### 1.1 所有権述語を足す

`App` に追加する (置き場所は `fs_zoom_mode_context_ok` の直前が読みやすい):

```rust
/// フルスクリーンの現ページが FsVideo コンテキストか (動画 / 音声 / 動画→音声モード)。
///
/// keymap は同じキーを FsImage と FsVideo の両方へ割り当てても衝突扱いしない
/// (既定の Z = `FsZoomMode` / `VideoToggleAudioMode`、keymap.rs の default_chords 参照)。
/// 実行時に「今どちらのコンテキストか」を表す状態は無いので、キーを消費する側が
/// 自分の所有でないキーを取らないためにこれを見る。
fn fs_video_key_context_active(&self, fs_idx: usize) -> bool {
    matches!(
        self.items.get(fs_idx),
        Some(GridItem::Video(_)) | Some(GridItem::Audio(_))
    )
}
```

動画→音声モードは item が Video なのでこの述語で覆われる (既存コメント 6194 と同じ根拠)。

### 1.2 消費の前に所有権を見る

`update_fs_zoom_mode_keys` の**先頭**に置く。`take_key_hold_edges` より前、
`level_permit` の判定より前 (所有していないキーは focus 喪失時のドレインもしない)。

```rust
if self.fs_video_key_context_active(fs_idx) {
    // 同じキーは FsVideo の action (既定 Z = VideoToggleAudioMode) の所有物なので
    // エッジを消費しない。消費すると音楽ビューの Z が届かず、音声モードから動画へ
    // 戻れなくなる (backlog §4.2)。
    //
    // 状態だけは畳む: Z ホールド中に画像→動画へ移ったとき、ズームがラッチしたまま
    // 残るのを防ぐ (今の除外分岐が fs_zoom_reset していたのと同じ理由)。ラッチは
    // OS 直読み (非消費) で現在の押下レベルに合わせ、動画から画像へ戻った瞬間に
    // 押しっぱなしの Z が rising edge として照準を始めないようにする。
    self.fs_zoom_reset();
    if let Some(permit) = level_permit {
        self.fs_zoom_z_was_down =
            self.keymap
                .key_held_action(ctx, permit, KeyAction::FsZoomMode);
    }
    return;
}
```

`key_held_action` は OS 直読み (`key_held_chord_via_os`) で**消費しない**ことを確認済み。

### 1.3 既存の分岐とコメントを現状に合わせる

- `fs_zoom_mode_context_ok` の Video / Audio 節は**残す**。Ring / マウスからの
  `toggle_fs_zoom_mode_action` ([ui_fullscreen.rs:6203](../../src/ui_fullscreen.rs:6203))
  がこの述語を使っており、そちらはキーを消費しないので所有権の話が無い。
  コメント (6191-6194) に「キー経路は `fs_video_key_context_active` で先に返るため、
  ここへ来るのは Ring / マウス経路」を追記する。
- 6256-6257 のコメントを更新する。「コンテキスト外でも先に edge を消費し」は
  **同一コンテキスト内で使えない場合** の話であることを明示する。
- 前セッションで足した調査用ログを撤去する: 音声モードの `Z:down` を
  `handle_fs_key_input` 呼び出し前に無制限で出す probe
  ([ui_fullscreen.rs:13470](../../src/ui_fullscreen.rs:13470) 付近、`stage=before_handle_fs_key_input`)。
  役目を終えた。既存の `exit key diagnostic` (`log_video_audio_exit_key_outcome`) は**残す**。

## 2. テスト (すべて mutation を通すこと)

### 2.1 修正前に落ちることを確認する (原因の実証)

T1 を**先に書き、修正前に落ちること**を確認して報告する。ソース読解だけの結論に留めない。

- **T1 所有権**: item = Video (および Audio) で `update_fs_zoom_mode_keys` を通したあと、
  Z の押下エッジが**まだ残っている**こと。`take_key_hold_edges` / `consume_action_no_repeat`
  をあとから呼んで press が取れることで確認する。
- **T2 状態を畳む**: Z を押した状態で item = Video のとき `fs_zoom_active` / `fs_zoom_aiming`
  が false になり、`fs_zoom_z_was_down` が押下レベルを反映すること。
- **T3 連結表示は変わらない**: item = 画像 + 連結表示で Z 押下 → エッジは**消費され**、
  `FsNavNoOpReason::ContinuousReadingUnavailable(Zoom)` が出ること。
  (「述語を丸ごと前に動かす」実装をしたらここが落ちる。)
- **T4 Ring 経路は変わらない**: item = Video で `toggle_fs_zoom_mode_action` を呼ぶと
  ズームは engage せず、今と同じフィードバックが出ること。
  (`fs_zoom_mode_context_ok` から Video / Audio 節を消したらここが落ちる。)

各テストについて、対応するガードを削除 / 反転した状態で**実際に落ちることを確認**し、
結果を報告に含める (「落ちるはず」ではなく実行結果)。

## 3. やらないこと

- 時間窓 (debounce / grace / settle ms) で races を吸収しない (憲法 §2 規則 5)。
- `take_key_hold_edges` の中身を変えない。所有権は呼び出し側の責務。
- KeyContext の実行時モデルを一般化して他の call site へ広げない。今回壊れているのは
  この 1 ペアだけ (`take_key_hold_edges` の call site は全リポジトリで 1 箇所、
  他の KeyHold は `modifier_held_action` で非消費)。
- 音楽ビュー側 ([ui_fullscreen.rs:17825](../../src/ui_fullscreen.rs:17825)) のガードを緩めない。

## 4. ドキュメント

- [docs/keymap-spec.md](../keymap-spec.md): 「同一キーを複数の KeyContext へ割り当ててよい」
  という既存の契約に対して、**キーを消費する側が自分の context を先に確かめる責務がある**
  ことを追記する。実行時の context 述語が無いため、この確認は各 call site の責務になる。
- [docs/next-release-backlog.md](../next-release-backlog.md) §4.2: 原因と修正を記録して閉じる
  (**エントリ末尾に追記**。冒頭の古い記述を残したまま完了扱いにしない)。

## 5. 機械チェック

```
cargo fmt --all && cargo fmt --all -- --check
cargo check -p mimageviewer --bin mimageviewer-core
.\scripts\test-full.ps1
.\scripts\build-dev.ps1
```

commit はしない。stage もしない。ブランチは `master`。
報告には T1 の修正前 fail、T1-T4 の mutation 結果、変更ファイル一覧を含める。
