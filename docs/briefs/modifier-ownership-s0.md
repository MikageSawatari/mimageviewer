# 修飾キー所有権 S0 — 型を固定する

**正本は [modifier-ownership-design.md](modifier-ownership-design.md)。**
同文書は 7 版まで改訂され、**後の版が前の版を上書きしている**。着手前に次の順で読むこと:

1. **§0 却下・修正の履歴** — 同じ轍を踏まないための記録。**死んだ仮定が 8 件**挙げてある
2. **§11.6 S0 が型で固定するもの** — 今回の範囲そのもの
3. **第 7 版 (§12.1〜§12.7)** — 最新の確定事項
4. §1〜§3 (現状と構造)。ただし **§9 / §10 / §11 / §12 が上書きしている箇所は後者が正**

## 1. 今回の範囲 — S0 のみ

**S0 は型を定義する層で、挙動を変えない。** 配線は L1 で行う (別作業)。

§11.6 の一覧をそのまま実装する:

- **キュー権限の単一 typed state**: `Stable` / `Acquiring` / 内部 attach / `ExternallyAttached`。
  **並列した bool にしない**
- **`DeliveryModifiers` と `CurrentModifiers` を不透明かつ相互変換不可**にする
- 左右別ビットごとの `Known` / `Unknown` と `PossibleAltGr`
- `ChordMatch::{Match, NoMatch, Indeterminate(reason)}`
- キー / ホイールパケット / button-down / button-up の **delivery 刻印付き envelope**
- **ジェスチャ型**: `start: DeliveryModifiers` + `current: CurrentModifiers`
- **detach 失敗を split topology として表現できない** attach transaction outcome
- **owner 専用コンストラクタ**。production に `From<egui::Modifiers>` や生 bool の
  コンストラクタを置かない

§12.5 の不変条件も型とテストで固定する:

- **probe → reseed → transition を所有スレッド上で直列化する。**非同期・再入的な受け渡しにしない
- `PM_NOREMOVE == false` は「以後メッセージが来ない」という約束ではなく**線形化点**である。
  後から届いたメッセージは、commit 前なら owner が `Acquiring` なので fail-closed、
  commit 後なら新 epoch に属する。**どちらでも正しい**ことが型から読み取れるようにする

## 2. 置き場所と作法

- 新規モジュール (`src/modifier_ownership.rs` など) に置く。
- 既存の [keyboard_input.rs](../../src/keyboard_input.rs) が owner / permit / snapshot の
  型パターンを持っているので、**命名と作法をそこに合わせる** (`KeyboardOwner`,
  `ShortcutPermit`, `KeyboardOwnershipSnapshot` 等)。
- **S0 の時点では呼び出し元が無いので `dead_code` 警告が出る。** モジュール先頭に
  スコープを限った `#![allow(dead_code)]` と、**「L1 が配線するまでの暫定。L1 着地時に外す」**
  というコメントを置くこと。**個々の項目に散らして付けない。**

## 3. 触らないもの

- [key_input.rs:243](../../src/key_input.rs:243) の `KeyEdge` の public scalar 修飾キーフィールド
- [native_window.rs:57](../../src/video/native_window.rs:57) の native key パケット
- [keymap.rs:1216](../../src/keymap.rs:1216) の chord 照合
- [keymap.rs:7493](../../src/keymap.rs:7493) / [7566](../../src/keymap.rs:7566) /
  [7708](../../src/keymap.rs:7708) の command fallback

これらは **L1 の着地時に消えるか private な互換 projection になる** (§11.6 / §12.7)。
**S0 では一切変更しない。** 挙動を変えないことが S0 の定義である。

## 4. テスト

型そのものが不変条件を担うので、**テストは「できないこと」を固定する**ものが中心になる:

- `DeliveryModifiers` から `CurrentModifiers` へ (およびその逆へ) 変換できないこと
- `ChordMatch::Indeterminate` が理由を必ず持つこと
- attach transaction outcome が **split topology を表現できない**こと
- 左右別ビットの `Known` / `Unknown` / `PossibleAltGr` の組み合わせが期待どおり畳まれること
- `Acquiring` 中の chord 照合が `Indeterminate` になる (fail-closed) こと

**コンストラクタの禁止は型では表せない**ので、**ソースを走査する unit test** を書く。
本リポジトリには前例がある: raw TextEdit を理由付き allowlist で禁止する test
([CLAUDE.md](../../CLAUDE.md) の「IME 対応」節、[ime_focus.rs](../../src/ime_focus.rs))。
同じ形で、**owner モジュール外に `From<egui::Modifiers>` や生 bool のコンストラクタが
現れたら落ちる**テストを置く。

## 5. やらないこと

- **配線 (L1)**。producer / consumer の移行はしない
- **ポインタの帰属 (L2)**。§11.4 の contract は L2 用の確定事項として設計文書に残す
- `about_to_wait` での `PeekMessageW` probe の実装 (§12.1)。**L1 の一部**
- 既存の挙動を 1 ミリも変えないこと。**S0 のコミットで実機の挙動が変わったら、それは S0 ではない**

## 6. 報告してほしいこと

- **§11.6 の 8 項目それぞれを、どの型でどう表したか**
- **表せなかった項目があれば、実装せずにその理由**
- `dead_code` allow をどこに置いたか (L1 で外す場所が 1 箇所であること)
