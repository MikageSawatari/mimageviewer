# ブリーフ: 右クリックメニュー一本化 A — 項目定義の tree 化と単一定義化

正本: [docs/context-menu-unification-plan.md](../context-menu-unification-plan.md)。
**着手前に §1 / §1.1 / §1.2 と、棚卸し §5 の表を読むこと。** 判定案はそこにある。

作業場所: この worktree (`C:\home\mimageviewer-extlaunch`, ブランチ `external-tool-launch`)。

## 0. この phase の範囲

**やること**: 右クリックメニューの項目を**1 つの定義**から作り、ネイティブと egui の両方が
その定義を描くようにする。あわせて棚卸しの「落とす / 移す」を適用する。

**やらないこと** (次の phase B):

- 仮想項目 (`ZipImage` / `PdfPage` / `Stack`) でネイティブメニューを出すこと
- OS 由来の項目をサブメニューへ畳むこと、その設定
- サブメニューを**使う**のはこの phase では「アプリケーションで開く…」だけ

つまり **B が今フォールバックで出している内容と、A が出している内容を、1 つの定義に統合する**
のがこの phase のゴール。仮想項目は従来どおり egui で描かれるが、**描く元は共通定義になる**。

## 1. 項目定義

`src/context_menu_model.rs` (新規) に、UI にもWin32 にも依存しない純データを置く。

```rust
pub enum MenuNode {
    Item { command: MenuCommand, label: String, enabled: bool, disabled_reason: Option<String> },
    Submenu { label: String, children: Vec<MenuNode> },
    Separator,
}
```

- `MenuCommand` は現在の `NativeMivCommand` を拡張したもの。棚卸しで「移す」判定の項目
  (ページ名をコピー / 代表画像のパスをコピー / この本のフォルダに移動 / 履歴から削除 /
  選択解除 / open-with 群) を variant として足す。
- 定義を作る関数は**純関数**にする。入力は「対象 (`GridItem` と surface と checked と view flags と
  必要な App 状態のスナップショット)」、出力は `Vec<MenuNode>`。**`App` を直接触らせない。**
  現在 `self.` から読んでいる状態は、必要なものだけを入力構造体へ写して渡す。
- 空の `Submenu` は作らない。連続 / 先頭 / 末尾の `Separator` は定義側で畳む。

## 2. 2 つの描画

- **ネイティブ**: `native_context_menu.rs` が `Vec<MenuNode>` を受け取って HMENU を作る。
  `Submenu` は `CreatePopupMenu` + `AppendMenuW(MF_POPUP)`。
- **egui**: フォールバック描画も同じ `Vec<MenuNode>` を辿る。`Submenu` は既存の
  `CollapsingHeader` でよい。**独自に項目を足さない。**

### ⚠ Shell 項目の挿入位置

現在 `shell_menu_insert_position` は「項目数 + 内部セパレータ数 + 1」で計算している
([native_context_menu.rs:106](../../src/native_context_menu.rs:106))。tree になると
**入れ子の子項目は最上位の位置を消費しない**。`Submenu` は 1 位置、その子は 0 位置である。
ここを間違えると Shell コマンドの同定が壊れる。**位置計算は最上位ノードだけを数える純関数にし、
入れ子を含むケースのテストを書くこと。**

### ⚠ コマンド ID

`MF_POPUP` の親自身は ID を持たない。ID は**葉ノードだけ**に、tree を辿った順で振る。
選択された ID から葉を引き当てる逆写像もテストする。

### ⚠ HMENU の寿命

`AppendMenuW(MF_POPUP)` で親へ付けた子 HMENU は、親の `DestroyMenu` で再帰的に破棄される。
子を個別に破棄しないこと (二重解放になる)。

## 3. 棚卸しの適用

正本 §1 と §5 の判定に従う。**表の判定案と食い違う実装をしないこと。**

**落とす**

- 「最近使ったアプリ」(最大 3 件)。`Settings::record_recent_open_with` の呼び出しも止める。
  **`recent_open_with_apps` はリリース済みなので、テーブル・フィールド・移行は残す**
  (読み書きを止めるだけ)。フィールドに「未使用。次の版で削除を検討」とコメントを付ける。
- 「旧XMPタグを取り込む」「旧XMPタグを取り込んでファイルから削除」。**右クリックだけでなく、
  上部「タグ」メニューの同等項目も落とす** (正本 §1 の 6、棚卸し §5.5 の (b))。
  **自動 seed (`tag_legacy_seed_worker`) は止めない。** `tags_db::miv_legacy_tags` と
  `xmp_writer` の共有ヘルパーは `xmp:Rating` 等でも使うので**消さない**。
  明示 worker (`tag_legacy_xmp_worker`) を消すかは、呼び出し元が無くなった時点で判断し、
  **消す場合は報告に明記**すること。
- 複数選択時に B が出している disabled の「パスをコピー」(A の実動作へ統合するため不要)。

**移す** — 棚卸しで「移す」と判定された項目を定義へ入れる。

**文言**

- 「削除 (ゴミ箱)」→ **「ゴミ箱へ移動 (タグ・評価も整理)」**。複数選択版は末尾に `[N件]` を維持。
- 三点リーダーを `...` と `…` のどちらかへ統一する。**どちらに寄せたかを報告すること**
  (既存の多数派に合わせてよい)。
- 「新しいフォルダ」は変更不要。

**揃える** (正本 §1.2)

- 仮想単一ページにも回転を出す。
- 仮想混在の複数選択で「選択項目のパスをコピー」は実項目だけを対象にし、
  **件数表示も実対象数に合わせる** (現在は全 checked 数を出しつつ実際は実項目だけ、というズレがある)。
- 「名前の変更...」「貼り付け」は共通定義に入るので、フォールバック側にも出るようになる。

## 4. 壊さないこと

- **`GridItem` の match は exhaustive を維持する。** 棚卸し表は 9 種だけを挙げているが、
  B は `Audio` / `ZipDir` / `SearchContainer` も扱っている。**列挙漏れで消さないこと。**
- 閲覧履歴 grid がネイティブを bypass している現在の挙動は、この phase では変えない。
- fullscreen の外部ツール起動がフルスクリーンを閉じる既存挙動を変えない。

## 5. テスト

**定義を作る純関数に対して、組み合わせごとの項目一覧を固定するテストを書く。**

- 対象種別 (`Image` / `Video` / `Audio` / `Folder` / `ZipFile` / `PdfFile` / `ConvertibleArchive` /
  `ZipImage` / `PdfPage` / `Stack` / `ZipDir` / `SearchContainer`) × surface (grid / fullscreen) ×
  `has_checked` の主要な組み合わせで、**出る項目のラベル列**を assert する。
- view flags (検索 / タグ / レーティング / 閲覧履歴) による除外。
- Shell 項目の挿入位置 (入れ子あり / なし / 空)。
- 葉への ID 割り当てと逆写像。
- 空 submenu を作らないこと、セパレータが畳まれること。
- 落とした項目がどの組み合わせでも出ないこと。

## 6. 守ること

- コミット前に `cargo fmt` (引数なし)。テストは `cargo test -p mimageviewer --lib`。
- UI 文言を変えたら `python scripts/check_ui_glyphs.py`。
- スナップショットテストが落ちたら [ui-snapshot-policy.md](../ui-snapshot-policy.md) に従い、
  **勝手に `UPDATE_SNAPSHOTS=1` で上書きせず差分の理由を報告**すること。
- UI スレッドで同期 I/O を足さない。定義を作る純関数の中でファイルを触らない。
- **差分が大きくなる。** 機械的な移送と意味のある変更を混ぜず、報告で分けて説明すること。
- 正本と食い違ったら実装を止めて報告すること。

## 7. 完了報告に含めること

- 変更ファイル一覧、追加した型と純関数、テスト結果の件数
- 三点リーダーをどちらに寄せたか
- `tag_legacy_xmp_worker` を消したかどうかと理由
- 棚卸し表と実装が食い違った項目 (あれば)
- 実機で確認すべき操作
