# A3b: 復元ウィンドウ (編集内容の復元 Phase 1 の最終段)

**正本は [docs/edit-content-identity-plan.md](../edit-content-identity-plan.md)。**
着手前に全文を読むこと。特に §5 (検出フロー)、§6 (UI)、§6.2 (「もう聞かない」の粒度)、
§8.2 (復元後の後始末)、§9 (テスト)。

A1 (台帳と記録) / A2 (検出) / A3a (コピーエンジン) は実装済み。
`src/content_identity.rs`、`src/content_identity/restore.rs`、
`src/app/content_identity_detection.rs`、`src/app/content_identity_restore.rs` を読むこと。

**これで Phase 1 が完成する。**

## 1. A3b で作るもの

- 非モーダル復元ウィンドウ (§6.1) と、A2 が保持している候補との配線
- **選択された候補をまとめて復元する batch 入口** (§3、規模の問題があるので必読)
- `restore_declined` を実際に書く操作 (§6.2)
- 復元後の後始末を A3a の関数経由で適用 (§8.2 の 2 / 3、mirror は 1)
- A3a に残っている `#[cfg_attr(not(test), allow(dead_code))]` の**撤去**
  (A3b が本番の呼び出し元になるので不要になる)
- マニュアル ([htdocs/mimageviewer/manual/](../../htdocs/mimageviewer/manual/)) と
  製品ページの記述

## 2. 作らないもの

- **右クリックメニューの「編集内容の復元を確認…」は Phase 2**。§6.2 が安全網として触れているが
  §10 の Phase 表では Phase 2 に置かれており、そちらを採る。A2 の修正で
  **`[閉じる]` は何も記録せず次に開いたら再提示される**ようになったので、
  この安全網は Phase 1 には要らない。
- **「このフォルダ以下では聞かない」は作らない** (§6.2)。使わない利用者は全体 OFF で足りる。
- 項目ごとの取捨選択 UI (補正だけ / タグだけ) は持たない (§2)。復元範囲は一括。

## 3. ⚠️ 規模: 候補ごとに 21 個の DB を開かない

A3a の `restore_candidate_at` は **1 候補ごとに** `copy_stores_at` を呼ぶ。
`copy_stores_at` は `STORES` を外側、mapping を内側で回すので、**1 回の呼び出しにつき
DB open が 21 回**走る。

§6.1 が想定するのは「フォルダ丸ごとコピーで数百件」である。
そのまま N 候補をループすると **N × 21 回の DB open** になる (500 件なら 10,500 回)。

**`restore.rs` に batch 入口を足すこと。** 選択された全候補の mapping を集めてから
`copy_stores_at` を **1 回**呼ぶ形にすれば、DB open は N によらず 21 回で済む。
`content_identity.db` を開く `mark_restored_origin_at` /
`load_restore_runtime_updates` も同様にまとめる。

**既存の `restore_candidate_at` を消してよい**。テスト専用に残す理由が無い
(batch 入口を 1 候補で呼べば同じ)。

## 4. ウィンドウ (§6.1)

```
編集内容の復元

このフォルダに、以前編集したファイルと内容が同じファイルが 3 件あります。
編集内容 (補正・消しゴム・モザイク・注釈・トリミング・★・タグ) を複製しますか?

  [すべて選ぶ] [すべて解除]

  ☑ IMG_0421.jpg   ← D:\photo\2025\IMG_0421.jpg (移動)
  ☑ chapter03.cbz  ← D:\manga\chapter03.cbz (コピー元は残っています)   [他 2 件の候補 ▼]
  ☐ scan.pdf       ← E:\old\scan.pdf (コピー元は残っています)

  □ 次から確認しない (環境設定で元に戻せます)

                                         [復元する]  [閉じる]
```

守ること:

- **モーダルにしない。** 検出はフォルダを開いた 1〜2 秒後に非同期で確定するので、
  モーダルだと閲覧を中断させる。
- **`common_modal_dialog_open` には登録する** ([app.rs:15439](../../src/app.rs:15439) の
  `modal_dialog_block_reason`)。背面グリッドへのホイール / キー漏れだけ止める。
  **state の有無ではなく「今描かれるか」を返す述語**を登録すること
  (CLAUDE.md「ダイアログ (egui::Window)」)。候補を保持したままフルスクリーン中で
  描いていない間に入力を止めてはならない。
- **`anchor()` を使わない。`default_pos()` を使う** (CLAUDE.md)。定番は
  `ctx.content_rect().min + egui::vec2(60.0, 40.0)`。
- **`.open(&mut open)` で × を付け**、閉じられたら `[閉じる]` と同じ扱いにする。
- 一覧の `ScrollArea` は **`.auto_shrink([false, ...])`**。数百件を想定するので
  高さも明示的に確保する。
- **[すべて選ぶ] / [すべて解除] を必ず置く。**
- **移動 (元が消えている) とコピー (元が残っている) を行に明示する。** 既定はどちらも ON。
- 候補が複数ある行は `last_edit_at` の新しいものを既定にし、他を選べるようにする (§5)。
  `last_edit_at = 0` (未編集) は最後に回る。
- **フルスクリーン中は提示しない** (§5)。候補は保持し、一覧に戻ったときに出す。
- 文言に**実装語を出さない** (CLAUDE.md「マニュアル・製品ページの記述方針」)。
  ハッシュ・SQLite・worker・台帳といった語は UI に出さない。
- **glyph lint を通す** (`python scripts/check_ui_glyphs.py`)。

## 5. 「もう聞かない」の粒度 (§6.2)

| 操作 | 効果 |
| --- | --- |
| 行のチェックを外して `[復元する]` | 外した行を `restore_declined` に**恒久記録** |
| `[閉じる]` (× 含む) | **記録しない** (次にそのフォルダを開いたら再提示) |
| `□ 次から確認しない` | **設定を OFF** (全体停止、§7) |

**この 3 つを取り違えないこと。** 特に `[閉じる]` が何も記録しないことは、A2 の修正で
成立させた性質そのものなので、テストで固定する。

## 6. 復元の実行

- **UI スレッドで DB を開かない。** copy は worker。
- 完了したら A3a が用意した関数で後始末を適用する:
  - `apply_content_restore_sidecar_mirrors` (§8.2-1)
  - `apply_content_restore_presence` (§8.2-2)
  - `finish_content_identity_restore` (§8.2-3)
  - `edit_origin` の昇格 (§8.2-4) は A3a のエンジン内で済んでいる
- **サイドカーの同期 flush を足さない。** 既存の flush 規律に任せる。
- 復元した件数と失敗をログに残す。失敗しても他の候補の復元を止めない。
- **フォルダ切替 / 設定 OFF で候補をクリアする** (A2 の cancel 経路と同じ扱い)。

## 7. 制約

- **時間窓・sleep・retry で吸収しない。**
- **新しい modal フラグを 60 個目として足さない。** 既存の述語表へ 1 行足すだけにする。
- 既存の A1 / A2 / A3a の**挙動を変えない**。変える必要が出たら止めて報告する。

## 8. テスト

- `[閉じる]` / × が `restore_declined` に**何も書かない**こと。
- チェックを外して `[復元する]` で、**外した行だけ**が `restore_declined` に入ること。
- `□ 次から確認しない` で設定が OFF になり、検出が止まること。
- [すべて選ぶ] / [すべて解除] が全行に効くこと。
- **batch 復元で DB open がストア数に比例し、候補数に比例しないこと**
  (open 回数を数える。§3 の理由)。
- 復元後に presence 集合とサイドカー mirror が更新されること。
- フルスクリーン中は描かれず、`common_modal_dialog_open` も立たないこと。
  一覧に戻ると描かれること。
- 復元後、同じフォルダを開き直しても**その行がもう提案されない**こと
  (A3a の `has_restorable_content` 昇格の帰結)。
- UI スナップショット 1 枚 ([docs/ui-snapshot-policy.md](../ui-snapshot-policy.md))。

## 9. 完了条件

- `cargo fmt` 済み / `cargo test -p mimageviewer --lib` が緑
- `cargo test --test ui_snapshot` が緑 (新規スナップショットは目視確認する)
- `cargo check -p mimageviewer --bin mimageviewer-core` が通る
- `python scripts/check_ui_glyphs.py` が 0 件
- マニュアル・製品ページ・`docs/spec.md` の更新
- **報告に、batch 復元での DB open 回数の実測 (候補 1 件 / 100 件) を書く**
