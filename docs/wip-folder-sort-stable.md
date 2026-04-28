# WIP: グリッドのフォルダ系ブロック ソート対応 + 日付ソート安定化

> **Status**: WIP — working tree (master, 未コミット) に変更が残っている。別セッションで再開する。
>
> **作成日**: 2026-04-29 (v0.9.0 TRT 統合 マージ後の整理)

## 背景

mImageViewer のグリッド表示は Explorer / Finder の慣習に倣って 2 段構成:

1. **フォルダ系ブロック** (上段): `GridItem::Folder`, `ZipFile`, `PdfFile`,
   `ConvertibleArchive` など「コンテナ系」アイテム
2. **メディア系ブロック** (下段): 画像 / 動画ファイル

`docs/spec.md §5.3` でこの構造は文書化されている (= WIP で記述追加中)。

ところが、**ツールバーの sort_order を「日付↓」等に切り替えても上段ブロックの並びが
変わらない**バグがあった。具体的には:

- メディア系 (`all_media.sort_by(...)`) は `self.settings.sort_order` に従って並ぶ
- フォルダ系 (`paired.sort_by(...)`) は `name().to_lowercase()` 昇順で固定 (= ハードコード)

つまり「日付降順」を選んでも、上段のフォルダ・ZIP・PDF は常にファイル名昇順だった。

加えて、日付ソートで `mtime_secs` が同一秒のファイル群は `read_dir` の戻り順
(= ファイルシステム依存で不安定) に並ぶ問題もあった。同一秒のファイル群を持つ
ケース (= 一括コピー / 一括 export / git checkout 等) は実用的に頻繁にある。

## 既存の変更 (working tree、未コミット)

### `docs/spec.md` (+5 -2)

§5.3 「グリッド内のアイテム順」の記述を更新:

- 旧: 「フォルダを先頭に、続いて画像・動画ファイルを表示する」
- 新: 「フォルダ・ZIP・PDF・対応アーカイブ（コンテナ系）を先頭ブロック、画像・動画
  ファイル（メディア系）を後続ブロックの 2 段構成で表示する（Explorer / Finder と
  同じ慣習）」「両ブロック内に同じソート順が適用される」

### `src/settings.rs` (+25 -2)

`SortOrder::compare` の `DateAsc` / `DateDesc` で **mtime 同点時にファイル名昇順で
tiebreak** するよう変更:

```rust
Self::DateAsc => mtime_a
    .cmp(&mtime_b)
    .then_with(|| name_a.to_lowercase().cmp(&name_b.to_lowercase())),
Self::DateDesc => mtime_b
    .cmp(&mtime_a)
    .then_with(|| name_a.to_lowercase().cmp(&name_b.to_lowercase())),
```

ユニットテストも追加 (`sort_order_compare_date_tiebreak_by_name`)。

### `src/app.rs` (+13 -3)

フォルダ系ブロックのソートを `name` 固定から `sort_order` 経由に変更:

```rust
let sort = self.settings.sort_order;  // 取得位置を folder ソート前に移動
{
    let mut paired: Vec<_> = folders.into_iter().zip(folder_metas).collect();
    paired.sort_by(|(a, ma), (b, mb)| {
        let an = a.name();
        let bn = b.name();
        let a_mt = ma.map(|(mt, _)| mt).unwrap_or(0);
        let b_mt = mb.map(|(mt, _)| mt).unwrap_or(0);
        sort.compare(&an, a_mt, &bn, b_mt, natural_sort_key)
    });
    ...
}
```

`folder_metas` から `mtime` を取り出して compare に渡すロジック。

### `vendor/bench_baseline.json` (削除)

なぜ削除したか不明 (おそらく WIP 中のテスト調整に伴う一時的な操作)。
**削除は意図的か要確認**:

- 残すべき (= 検索 bench の baseline): リリース前 perf 回帰チェックで使う
  `scripts/check_bench_regression.py` の入力。これが無いと Phase 2 の bench 確認
  ができない
- 不要なら commit でファイル削除を確定

判断: **削除は誤りの可能性が高い。`git restore vendor/bench_baseline.json` で復元
推奨**。

### `tmp-test/` (untracked)

中身を確認していない。テスト用に手動作成したフォルダか、誰かのスクリプトの出力か。
コミット前に整理する (`.gitignore` に追加 or 削除)。

## 残タスク

### 必須

- [ ] **`vendor/bench_baseline.json` の扱いを判断**: 復元 or 削除コミット
- [ ] **`tmp-test/` を整理**: 削除 or `.gitignore` 追加
- [ ] **既存テストへの追加カバレッジ**:
  - フォルダ系ブロックも sort_order 反映されることを assert する統合テスト
  - 上段→下段の境界が崩れないことの確認 (フォルダが下段に紛れ込んでないか)
- [ ] **動作確認**:
  - 日付↓ にしてフォルダ・ZIP・PDF が新しい順に並ぶ
  - 同一秒の mtime を持つファイル群が同じ順序で表示される (FS 依存しない)
  - お気に入り編集ダイアログ等、SortOrder::compare を使う他箇所に副作用がないか
- [ ] **コミット**: 上記がすべて OK なら 1〜2 commit で master に push

### 推奨

- [ ] **マニュアル更新** (`htdocs/mimageviewer/manual/grid.html`):
  「ソート順は両ブロック内に適用される」旨を追記。CLAUDE.md「マニュアル・製品
  ページの記述方針」に従い、内部用語 (= GridItem 名等) は出さない
- [ ] **`docs/architecture-overview.md` の display-pipeline 部** に「2 段構成 +
  ブロック内ソート」を反映
- [ ] **回帰テスト** に既存の挙動を固定するスナップショットを追加
  (`tests/ui_snapshot.rs` のグリッド画面、ソート順切替後のレイアウト)

## 注意点 (再開時に把握しておくこと)

### 関連箇所

- `src/app.rs` のグリッド項目構築は `load_folder_inner` あたり (line 2890 周辺)
- `SortOrder::compare` は他に検索結果ソートでも使われる (`src/search_index_*.rs`)
  ので、tiebreak 追加の副作用を確認
- `natural_sort_key` は `src/app.rs` 内で定義、`SortOrder::compare` に渡す
  closure (= 引数 `K: Ord` で抽象化)

### 既存挙動との互換性

- 日付ソートで mtime 同点ケースは旧版で `read_dir` 順 = 不安定だった。本変更で
  名前順固定になるため、ユーザーから見た並びが「変わる」可能性がある (= 機能改善)
- ソート切替を頻繁に使うユーザーは上段の並びが連動するようになって挙動が
  変わるが、これは元々の意図 (バグ修正) なので問題ない

### マージ後 (TRT 統合 v0.9.0) との関係

- src/app.rs の line 2890 周辺は TRT 統合では触っていない (= サムネイル / グリッド
  ロード関連、TRT は AI 推論まわり)。merge 時にコンフリクトなしで stash pop
  が通った
- v0.9.0 リリースに **本 WIP は含まれない** (TRT 統合のみ)。フォルダソート
  修正は v0.9.1 か v0.10.0 など後続リリースで取り込む想定

## 参考: WIP の発端 (推測)

- ユーザー (mikage) が「日付ソートにしてもフォルダの並びが変わらない」のに
  気付いた
- `SortOrder::compare` を見て tiebreak 不在も発見
- フォルダソートを sort_order 化 + tiebreak 追加に着手
- TRT 統合作業 (claude/dazzling-mcclintock-d0be64) の合流前に作業中断、stash 経由
  で working tree に保持

## 別セッションでの再開コマンド

```powershell
# 状態確認
cd C:\home\mimageviewer
git status

# WIP 変更を見る
git diff docs/spec.md src/settings.rs src/app.rs

# bench_baseline.json を復元する場合
git restore vendor/bench_baseline.json

# tmp-test/ を削除する場合
Remove-Item -Recurse -Force tmp-test

# 残タスクのテスト
cargo test --release sort_order

# 動作確認 (リリースビルド)
cargo build --release
.\target\release\mimageviewer.exe
```
