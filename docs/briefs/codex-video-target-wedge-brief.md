# backlog §1.96 — 動画へのページ移動が navigation sequence を閉じられず、以後の移動が止まる

対象: [next-release-backlog.md](../next-release-backlog.md) §1.96。利用者メール (pattier) +
開発側で再現済み。**v3.1.2 に入れる。**

**エントリに書かれた仮説 (入力 permit / focus) は棄却された。** 下の §0 を読むこと。
原因は特定済みで、**リポジトリ内に赤いテストが既に存在する**。

## 0. 原因 (2026-08-19 確定)

### 0.1 元の仮説が外れた理由

エントリは `input_permits.discrete` の fail-closed return
([ui_fullscreen.rs:17515](../../src/ui_fullscreen.rs:17515)) を疑っていた。

利用者の追加報告 (2026-08-19): **キーが効かなくなった状態でも、Space のチェック、Esc、
ルーペ ON/OFF は動く。効かないのはカーソル 4 方向だけ。**

`FsLoupeLockToggle` の消費は [ui_fullscreen.rs:18582](../../src/ui_fullscreen.rs:18582)、
つまり **`handle_fs_key_input` の中、gate より後ろ**にある (関数は 17472 から始まり、
18620 まで他の `fn` は無い)。gate が発火していればルーペも死ぬ。**死んでいない以上、
permit は届いており、gate は無罪。**

### 0.2 実際の機構

`fs_navigation_sequence_blocks_new_target()` が true のまま戻らない。

- 静止画から次の項目へ移るとき、[ui_fullscreen.rs:23609](../../src/ui_fullscreen.rs:23609) が
  `begin_fs_page_navigation_sequence` を呼び、**移動先の page 群を `Display` target とする
  sequence** を作る。移動先が動画でも作る。
- sequence を retire できるのは `observe_fs_navigation_sequence_presented` だけで、その唯一の
  production 呼び出し元は page renderer の `emit_fs_page_turn_ready_for_display_unit`。
  **動画は native presenter が描くのでここへ来ない。**
- 結果、sequence は `Awaiting` / `Presenting` のまま残る。`blocks_new_target()` は
  `RenditionFailed` 以外すべて true ([app.rs:6927](../../src/app.rs:6927)) なので、以後の
  ページ移動が全部拒否される。
- **症状の全部が説明できる**: 動画の上ではカーソルが効く (media 分岐は
  `blocks_new_target()` を見ない)、動画から画像へ戻った後だけ効かなくなる、Esc /
  Space / ルーペは navigation ではないので無事、Esc でフルスクリーンを抜けると復旧する。

### 0.3 既に赤いテストがある

[ui_fullscreen.rs:36618](../../src/ui_fullscreen.rs:36618):

```rust
#[ignore = "red until a natively presented item stops being described as a page display unit"]
fn page_navigation_never_describes_a_natively_presented_item_as_a_page_unit()
```

前セッションがこの欠陥を特定し、テストを書いて `#[ignore]` で置いていた。
**契約も既に書かれている**: `Display` target の各 page は page renderer が描くもの
(= `GridItem::has_page_data`) でなければならない。

兄弟ケース (Ctrl+↑↓ でフォルダ移動して動画に着地) は既に修正済みで、
`folder_navigation_onto_a_video_does_not_wedge_the_next_navigation` が通っている。
**残っているのは同一フォルダ内のページ移動 (↑↓ / ←→) だけ。**

## 1. やること

**page renderer が描かない項目を `Display` target に含めない。**

- `begin_fs_page_navigation_sequence` ([ui_fullscreen.rs:5612](../../src/ui_fullscreen.rs:5612))
  で、target の page 群が全て `has_page_data` を満たすかを確認する。満たさないなら
  **sequence を作らずに移動を成立させる** (呼び出し側は
  `if !moved || self.begin_fs_page_navigation_sequence(...) { self.land_still_page_navigation_target(...) }`
  なので、`true` を返せばそのまま着地する)。
- これは妥当な設計判断でもある: sequence は「前の表示単位を保持して切り替えのちらつきを防ぐ」
  ためのもので、**移動先を native presenter が描くならその約束は成立し得ない**。
- **`blocks_new_target()` の判定を緩めない。** 「詰まったら解除する」形の救済を足すのではなく、
  **閉じられない sequence を作らない**のが構造的な直し方。
- 混在 unit (見開きの片側が動画、等) が理論上あり得るなら、テストが全 page を走査している
  ので同じ条件でカバーされる。

## 2. やらないこと

- 入力 permit / focus / native presenter の focus 処理に手を入れない (§0.1 で無罪)。
- 時間窓・timeout で sequence を強制解除しない (憲法 §2 規則 5)。**これは典型的な症状パッチ。**
- media 分岐 (`start_manual_media_navigation`) の挙動を変えない。
- Ctrl+↑↓ の `FolderItems` sequence 経路を変えない (既に修正済み)。

## 3. テスト

1. **`page_navigation_never_describes_a_natively_presented_item_as_a_page_unit` の
   `#[ignore]` を外して通す。** これが本命。
2. 画像 → 動画 → 画像と移動した後、`blocks_new_target()` が false で、次の移動が受理される
   (報告そのものの再現をハンドラ level で固定する)。
3. 画像 → 画像の通常移動では従来どおり sequence が作られる (回帰)。§1.88 / §1.89 / §1.91 で
   固定した atomic display-unit の契約を弱めていないこと。
4. `folder_navigation_onto_a_video_does_not_wedge_the_next_navigation` が引き続き通る。
5. mutation: 1.1 の条件を外すと 1 と 2 が落ちることを確認して報告する。

## 4. 実機確認 (利用者が行う。手順を報告に書くこと)

再現フォルダを用意してある: **`C:\tmp\miv-mixed-video-nav`** (15 件、画像と動画を交互に配置。
単独動画 2 か所、連続動画 1 か所)。フルスクリーンで <kbd>↓</kbd> を送り続け、動画を
2 つ通過した後もカーソルが効くことを確認する。

## 5. 凍結ルール

native video の発火面に触れるので
[detached-rework-plan.md](../detached-rework-plan.md) §2 (憲法) の対象。着手前に §2 を読み、
完了時に §11 へ「症状パッチではなく構造的修正である」根拠とともに追記する。

## 6. ドキュメント

- [display-pipeline.md](../display-pipeline.md) §2.5.4 (atomic display-unit 契約) に、
  **`Display` target は page renderer が描く項目だけで構成される**ことを明記する。
- [next-release-backlog.md](../next-release-backlog.md) §1.96 に結果を追記して閉じる。
  **棄却された仮説と、棄却の根拠 (ルーペが gate の内側で動いた) も残すこと。**
  エントリ冒頭の記述は消さない。

## 7. 機械チェック

```
cargo fmt --all && cargo fmt --all -- --check
cargo check -p mimageviewer --bin mimageviewer-core
.\scripts\test-full.ps1
.\scripts\build-dev.ps1
```

commit / stage はしない。ブランチは `master`。
報告には変更ファイル一覧、`#[ignore]` を外したテストの実行結果、mutation 結果を含める。
