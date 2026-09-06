# ブリーフ: 別バージョン発見 Step 5 — 更新が全件再構築を伴わない検索配列

対象: 検索配列の再設計。実装 = Codex Sol / レビュー・検収 = ClaudeCode / 実機確認 = 利用者。

正本: [docs/duplicate-detection-plan.md](duplicate-detection-plan.md) **§21**。
着手前に §21 全体を読むこと。§21.2 に**却下した案とその理由**がある。

作業ツリー: **`C:\home\mimageviewer-dupe`** (branch `duplicate-detection`)。
**コミットしないこと。** ファイルを書き、`cargo fmt` とテストを通したら止まる。

---

## 0. 前提 — 現行実装に引きずられない

**利用者の指示 (2026-09-07)**: 現状の `similar.db` と検索配列は**仮実装**とみなしてよい。
**再索引は許容する。** 移行コードは不要。**最適な形を選ぶこと。**

したがって、スキーマ変更・ファイル形式変更・`hash_version` 引き上げを遠慮しない。
既存の `row_id` ベースのサイドカーは**捨ててよい**。

**解く問題**: 索引の内容が変わるたびに検索配列を全件作り直すため、
ダウンロードフォルダのように頻繁に増える場所で**変更のたびに待たされる**。

**利用者の優先順位 (明示)**: 「正確だが 10 秒待つ」より「多少古くても即座に出る」。

---

## 1. 設計 (§21.3 の具体化)

### 1.1 SQLite が正本。配列は派生

```sql
item(
  item_id       INTEGER PRIMARY KEY AUTOINCREMENT,  -- 永続。再利用しない
  item_key      TEXT NOT NULL UNIQUE,               -- 正規化パス
  revision      INTEGER NOT NULL,                   -- 内容が変わるたびに増やす
  kind, container_key, page_index,
  mtime, file_size, hash_version, pdq256, quality,
  width, height, format
)

item_change(
  seq       INTEGER PRIMARY KEY AUTOINCREMENT,      -- 適用順
  item_id   INTEGER NOT NULL,
  op        INTEGER NOT NULL,                       -- Add / Update / Delete
  revision  INTEGER,
  pdq256    BLOB,                                   -- Add/Update のみ
  quality   INTEGER                                 -- Add/Update のみ
)
```

- **`AUTOINCREMENT` を使う理由**: 通常の rowid は削除後に再利用され得る。
  また現行の `item_key TEXT PRIMARY KEY` は**暗黙 rowid が `VACUUM` で変わり得る** (§21.5)。
  永続 ID を明示的に持ち、**コミット済み ID を再利用しない**。
- **削除して再作成された項目には新しい `item_id` を与える** (revision を上げるのではなく)。
- **`item_change` は本体更新と同じトランザクションで書く** (§21.6)。
  コミット後に channel へ通知するだけでは、**通知前のクラッシュで変更を失う**。
  **通知は起床用、履歴は復旧用。**
- **Delete も必ず履歴に残す。** 省くと古い項目が配列に残り続ける。
- コンテナのページ群は **Complete 公開単位**で履歴に入れる。
  キャンセル / 失敗した staging を流さないこと。

### 1.2 base ファイル (不変)

`similar.base` — ヘッダ + 固定長レコードの配列。

```
record: item_id u64 | pdq256 [u8;32] | quality u8 | revision u32 | padding  = 48 bytes
header: magic, format version, hash_version, PROXY_VERSION, store_id,
        record_count, applied_seq, body checksum
```

- **一度書いたら書き換えない。** 更新は delta 側で表現する
- `applied_seq` = この base が反映済みの `item_change.seq`
- 読み込みは**逐次読み**。**mmap は使わない** (§21.2 の理由。後から独立して比較できる)
- スタンプ不一致・短い・壊れているときは**採用せず、SQLite から作り直す**。
  削除しても無害であること

### 1.3 delta はファイルにしない

**delta 専用ファイルを作らないこと。** 起動時は base を読んだあと、
`item_change` から `seq > base.applied_seq` を読んで**在メモリで delta を作る**。

理由: 追記ファイルは公開長・破損・部分書き込みの管理が増えるが、
**同じ情報が SQLite に既にある**。正本を 2 か所に置かない。

### 1.4 snapshot

```rust
struct SearchSnapshot {
    base: Arc<BaseArray>,        // 不変
    delta: Arc<DeltaSet>,        // 不変。item_id -> Some(sig) | None(削除)
    superseded: Arc<BitSet>,     // base のうち delta が上書きした位置
    applied_seq: i64,
}
```

- **どれも公開後は不変。** 新しい内容は**新しい snapshot を作って差し替える**
- **in-place で書き換えない。** これが §21.2 の競合と UB を構造的に消す唯一の条件
- 検索は開始時に `Arc` を掴み、**その後の更新を待たない**
- `superseded` は**ビット集合**にする。base の各レコードで delta を hash 参照すると
  4.6M 回の参照になる。ビットなら 4.6M bit = 約 578 KB で O(1)

### 1.5 更新 worker と統合 worker

- **更新 worker**: `item_change` の未適用分を読み、新しい `delta` / `superseded` を作って
  次の snapshot を公開する。**処理量は変更件数に比例**する。全件コピーも全件ソートもしない
- **統合 worker**: delta が一定量を超えたら、base + delta を畳んだ**新しい不変 base** を
  temp へ書き、fsync してから atomic rename で公開。その後 `applied_seq` までの履歴を削除する
  - **新 base の公開が確定するまで、それに必要な履歴を消さない**
  - **統合中に DB が更新されたことを理由に、完成した base を捨てない** (その分は delta に残る)

---

## 2. 安全性の契約 (§21.4。ここが仕様の核心)

> **配列は候補を提案するだけ。** 返す各項目の identity、検索対象としての有効性、署名、
> 距離、順位、表示情報は、**SQLite の 1 つの読み取り snapshot に対して**成立させる。
> **更新遅延による取りこぼしは許すが、誤答は許さない。**

実装上の帰結:

1. **検索元は `item_key` で SQLite から直接引く。**
   古い配列の索引から探すと、**新規・変更された画像が「索引に無い」と誤報告**される。
   検索元自身が未索引である状態は、それとして別に表示する
2. 候補ごとに、同じ DB 読み取り snapshot で次を確認する:
   - 項目が存在し、検索対象で、コンテナが**公開済み Complete 世代**である
   - `hash_version` / `PROXY_VERSION` が適合する
   - **DB の署名が候補の署名と一致する。不一致なら捨てる**
   - 表示情報 (パス、寸法、形式、サイズ) も**その snapshot から取る**
3. **件数制限は検証の後に適用する。** 古い候補が上位枠を埋めて全滅し得る (§21.4)
4. 検証は**専用接続の読み取りトランザクション**で行い、検索元と候補詳細を同じ snapshot に揃える。
   **その中でファイルのデコードなど遅い処理をしない**

**救えないもの (許容する)**: 変更されたばかりで古い署名が遠く、候補にすら上がらない画像。
**救うもの**: 古い署名だけが近かった項目 — DB 照合で捨てられる。

---

## 3. 起動と復旧

- **互換性を確認できた古い base を先に公開**し、履歴の追随は非同期で進める。
  **内容世代が古いことだけを理由に base を捨てない** (現行実装の欠陥)
- 履歴に欠落があれば**追随完了を偽らない**。裏で新しい base を作る。
  その間も**使える旧 snapshot は維持する**
- base が無い / 壊れているときは SQLite から作る。その間パネルは「準備中」を返す
- **store_id を base に刻む。** バックアップ復元で別 DB に同じ世代番号が付く場合があるため

---

## 4. 範囲外

- mmap (§21.2。後から独立して比較する)
- 削除機能、全件スイープ、「どちらを残すか」、幾何的ズレ対応 — いずれも確定済みで対象外
- 本単位の関係表示・ページ帯・横断一覧 (Step 4。この Step とは独立)

## 5. 併せて判断すること

`similar.db` が **`rename_key_migration::STORES` に登録されていない** (§21.7)。
path-keyed ストアをファイル移動時に追随させる既存の仕組みから外れているため、
**移動のたびに再ハッシュが走る**。タグ・評価と同じ扱いにすべきか判断し、
**理由とともに報告すること** (この Step で入れる / 入れない、どちらでもよい)。

## 6. テスト

- **不変性**: 公開後の base / delta / superseded を書き換える経路が無いこと
  (型で表せるなら型で。`Arc<[T]>` など)
- **履歴の原子性**: item 更新と `item_change` が同じトランザクションであること。
  片方だけコミットされる経路が無いこと
- **削除の反映**: Delete 履歴を適用した snapshot で、その項目が候補に出ないこと
- **ID の非再利用**: 削除 → 同じ `item_key` で再作成 したとき、**新しい `item_id`** になること
- **stale 候補の棄却**: 配列の署名と DB の署名が食い違う候補が**結果に出ない**こと
- **検索元が新規**: 配列に無い画像を検索元にしても、SQLite から引けて検索できること
  (「索引に無い」と誤報告しないこと)
- **件数制限**: 検証前に打ち切らないこと (古い候補で埋めたケースを作って確認)
- **base の互換性**: store_id / hash_version / PROXY_VERSION / record_count / checksum の
  各不一致で**採用されず**、SQLite へ落ちること。**それぞれ別の理由を返すこと**
- **統合中の更新**: 統合中に来た変更が失われず、完成した base も捨てられないこと
- **履歴欠落**: 欠落を検出したとき、追随完了を偽らず旧 snapshot を維持すること

## 7. 計測して報告すること

- 起動から検索可能になるまで (base あり / base 無し)
- **1 件追加したときに、次の検索が可能になるまでの時間** ← この Step の目的
- 統合の所要時間と発生頻度
- 常駐メモリ

**「1 件追加したときの待ち」が現状の全件再構築から何桁減ったか**が合否になる。

## 8. 判断に迷ったとき

- **公開後のデータを書き換えない。** 迷ったら新しい snapshot を作る
- **正本を 2 か所に置かない。** SQLite にある情報をファイル形式で二重管理しない
- **silent fallback を作らない。** base を採用しない理由は型で区別する
- 仕様上どうしても決まらない点は、**実装で埋めずに質問として残す**
