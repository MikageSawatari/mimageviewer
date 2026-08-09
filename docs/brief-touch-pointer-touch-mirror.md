# ブリーフ: egui-winit の `pointer_touch_id` を正確にミラーする (交互ピンチの原因)

対象: v2.13.0。実装 = Codex Sol / レビュー・検収 = ClaudeCode。
正本: [docs/touch-support-plan.md](touch-support-plan.md) §5.2。

前提 (完了・コミット済み): Step 0 `bb9574b2` / Step 1 `66cb2910` / Step 2 `6be3afd3` /
Step 3 `42aa037a` / Step 3c `cae9ffd6` / ピンチ昇格 `21ef4643` / 1 フレーム 1 回 `04f71c12` /
相関診断 `b22e7d00`。

---

## 1. 症状と再現条件

実機報告 (2026-08-07):

> ピンチが 2 回に 1 回、交互にしか効かない。
> **中央をタップして HUD を出した後**に同じようにやると再現する。

## 2. 原因 (実機ログで確定済み。推測ではない)

### 2.1 egui-winit 0.33.3 は「押していない接点」に release を出す

`egui-winit-0.33.3/src/lib.rs` の `on_touch`:

```rust
if self.pointer_touch_id.is_none() || self.pointer_touch_id.unwrap_or_default() == touch.id {
    match touch.phase {
        Started   => { self.pointer_touch_id = Some(touch.id); on_cursor_moved(); Pressed; }
        Moved     => { on_cursor_moved(); }
        Ended     => { self.pointer_touch_id = None; Released; PointerGone; }
        Cancelled => { self.pointer_touch_id = None; PointerGone; }   // release は出ない
    }
}
```

**gate は「pointer 接点が未設定」または「自分が pointer 接点」。**

2 本指ピンチで **pointer 接点だった指が先に離れる**と `pointer_touch_id = None` に戻る。
その後、**残っていた 2 本目の指が離れたとき `is_none()` が真になるので release が発行される** —
その接点は一度も `Pressed` を受けていないのに。

同様に、**2 本目の指が Start したときは gate が偽なので合成 pointer は一切出ない**。

### 2.2 こちらの相関層がそれを想定していない

`src/touch_correlation.rs` の `process_event` は、対応する pending が無い primary を
「マウス入力」とみなして fail-closed する:

```rust
Event::PointerButton { button: PointerButton::Primary, .. } => {
    frame.ambiguous = true;
    self.cancel_stream(geometry, now_ms);
}
```

→ §2.1 の release がここに落ちる。

**実機ログの裏付け** (`MIV_TOUCH_DEBUG` 有効、776 correlation 行):

| 観測 | 件数 |
| --- | ---: |
| `ambiguity=[unmatched_primary(...)]` で **すべて `pressed=false`**、かつ `pending=None->None`、`pointer_touch=absent->absent` | 13 |
| `ambiguity=[pending_mismatch(pending=StartMoved, event=Touch(...Move...))]` | 2 |
| `owner=Cancelled->Cancelled` (cancel 後、指を離すまで無反応) | 65 |

`pending_mismatch` も同根で、**2 本目の Start に対して `StartMoved` を期待して待ってしまっている**。

### 2.3 なぜ「交互」になり、なぜ HUD を出すと再現しやすいか

`ambiguous` は **フレーム単位で全コマンドを捨てる**。クロームを表示するとフレームが重くなり、
イベントが 1 フレームにまとめて入るようになるため、
**前のジェスチャ末尾の余計な release と、次のジェスチャの開始が同じフレームに同居**する。
結果、次のピンチが開始時点で死ぬ → 効く / 効かないが交互に出る。

## 3. 直すこと — ミラーを正確にする

**`unmatched_primary` の fail-closed 自体は正しいので残す。** 直すのは
**こちらが持つ `pointer_touch` ミラーが egui-winit の gate を正確に再現していない**点。

`src/touch_correlation.rs` のミラーを、§2.1 の疑似コードと**1 対 1 で対応する形**にすること:

- gate = 「ミラーが `None`」**または**「その接点がミラーと一致」
- gate が真のとき、phase ごとに期待する合成 pointer 列:
  - `Start` → ミラーを `Some(id)` に。`PointerMoved` → `PointerButton(pressed)` を期待
  - `Move` → `PointerMoved` を期待
  - `End` → ミラーを `None` に。`PointerButton(released)` → `PointerGone` を期待
  - `Cancel` → ミラーを `None` に。**`PointerGone` のみ期待 (release は来ない)**
- gate が偽のとき (= 別の接点が pointer 接点)、**その接点に合成 pointer 列は一切来ない**。
  pending を立てないこと

**「release が来たら黙って捨てる」ような対症的な緩和をしないこと。** それをすると
本物のマウス release まで捨てて plan §5.15 の保証が壊れる。
**gate を正しく再現すれば、その release は「期待された列の一部」になる**ので、
fail-closed に触れる必要がない。

## 4. 依存更新の番人を強化する (必須)

既存の `egui_winit_0_33_3_signature_is_exact_and_ordered` は**単一接点の場合しか固定していない**。
今回の穴はそこから漏れた。**複数接点の契約も同じテストで固定すること**:

- 2 本目の `Start` は合成 pointer を伴わない
- **pointer 接点が先に離れた後、残った接点の `End` は release + `PointerGone` を伴う**
  (今回の本命。これが無いと同じ退行を繰り返す)
- `Cancel` は `PointerGone` のみで release を伴わない
- doc comment に `egui-winit-0.33.3/src/lib.rs` の `on_touch` を参照し、
  **gate の条件式そのもの**を引用しておくこと

## 5. テスト

§4 に加えて:

- **2 本指ピンチで pointer 接点が先に離れる列**が `ambiguous` にならず、
  Zoom/Pan が正常に出ること (今回の回帰テスト本体)
- 2 本目が先に離れる順序でも正常なこと
- 前のジェスチャの末尾と次のジェスチャの開始が**同じフレームに同居**しても、
  次のジェスチャが `Cancelled` にならないこと (「交互」の直接的な回帰テスト)
- **本物のマウス release は従来どおり `ambiguous` になること** (§3 の緩和をしていない確認)
- 既存のタッチ関連テストがすべて通ること

## 6. 完了条件

- `cargo fmt` (引数なし) を通すこと
- `cargo test -p mimageviewer --lib` が**全件**通ること (現在 4885 件)
- `cargo check -p mimageviewer --bin mimageviewer-core` が通ること
- **plan §5.2 に、この gate の正確な内容を追記すること。**
  「先頭 1 接点のみ pointer をエミュレート」という現在の記述は不正確で、
  今回の穴の原因になった。**接点が離れた後に gate が再び開く**ことを明記する

## 7. 制約

- **アプリを起動しないこと。** 検証ビルドは ClaudeCode が用意する
- ブランチ操作・コミットは不要。master の作業ツリーで作業する
- **診断ログ (`b22e7d00`) は残すこと。** まだ実機確認が続く
- **範囲を広げないこと。** しきい値変更や新ジェスチャは不可
- detached-rework 凍結ルールは有効

完了したら、変更内容・**ミラーをどう gate と対応させたか**・テスト結果・
plan の更新箇所を報告すること。
