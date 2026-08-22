# 内容ハッシュによる編集内容の引き継ぎ 設計ドキュメント

OS 側 (エクスプローラー等) でファイルを移動・コピーすると、mIV の編集内容 (補正 / 消しゴム /
モザイク / ローカル調整 / テキスト注釈 / 出力範囲 / 回転 / ★ / タグ …) が引き継がれない。
本機能は **ファイル内容のハッシュ**を identity として編集内容を再結合し、
利用者に確認したうえで新しい場所へ複製する。

- 状態: **Phase 1 実装済み・実機確認済み (2026-08-22)**。A1 台帳 / A2 検出 / A3a コピー / A3b 復元 UI に加え、実機で見つかった 5 件を修正済み
- **未着手 (次にやること)**:
  1. **A6 複数コピー元の選択 UI** — 正本 [briefs/edit-identity-a6-multi-source-choice.md](briefs/edit-identity-a6-multi-source-choice.md)。実機で 3 候補が全部 `last_edit_at = 0` で同点になり、
     注釈を持つ原本が既定に選ばれなかった。backfill 由来の行は必ず 0 なので §5 の既定順は同点で機能しない
  2. **アプリ内コピー / ページ移動の同じ穴** — `apply_book_page_edit_copies` も comic 行をコピー後に
     共有後処理へ入るが、そこは `comic_docs` を失効しない。復元と同じ削除事故が起こり得る (A4 の調査で判明、未修正)
  3. **注釈サムネイルの再起動後消失** — 復元経由の分は `edit_previews` のファイル所有修正で解消。
     復元を通っていないファイルでも起きるかは未確認。`edit_preview_close:` の計装が理由を名指しする
- 関連: [preset-and-adjustment.md §9](preset-and-adjustment.md) (フォルダ側サイドカー)、
  [src/rename_key_migration.rs](../src/rename_key_migration.rs) (アプリ内リネーム移行)、
  [src/metadata_transfer.rs](../src/metadata_transfer.rs) (明示的なメタ情報書き出し / 取り込み)

---

## 1. 背景 — 現在の移動耐性と、その穴

編集データは全て `%APPDATA%` の中央 DB に **絶対パスをキー**にして入っている
(`adjustment_db::normalize_path`、ZIP / PDF は `zip_entry_key` で `<容器パス>::<エントリ|page_N>`)。
移動耐性のための層は既に 3 つあるが、いずれも OS 側の個別ファイル操作には効かない。

| 層 | 実体 | 効く場面 | 効かない場面 |
| --- | --- | --- | --- |
| フォルダ側サイドカー `mimageviewer.dat` | [src/sidecar.rs](../src/sidecar.rs)、既定 ON | **フォルダごと**の移動 / コピー (相対キー) | 単体ファイルのコピー、フォルダ外への移動 |
| リネーム移行 | [src/rename_key_migration.rs](../src/rename_key_migration.rs) | **アプリ内**リネーム (現行 `STORES` 22 descriptor) | アプリ外のリネーム / 移動 (モジュール冒頭に「将来課題」と明記済み) |
| ポータブルメタ情報 | [src/metadata_transfer.rs](../src/metadata_transfer.rs) | 明示的な書き出し / 取り込み | 自動ではない。照合も **パス + size/mtime** で内容ハッシュではない |

さらに調査で判明した点として、**サイドカーがミラーするのは
`adjust / mask / conceal / local_adjust / export_crop / comic / tags` だけ**で、
**★レーティング・回転・見開き・トリム・本の続きは含まれない**。つまりこれらは
「フォルダごと移動」でも失われる。内容ハッシュ方式はこの穴も同時に塞ぐ。

---

## 2. スコープ

### 対象

- 物理ファイル: **画像 / ZIP / PDF / 変換対象アーカイブ (RAR・7z・LZH・入れ子入り ZIP)**
- 復元する範囲: **編集・表示状態・★・タグを一括** (項目ごとの取捨選択 UI は持たない)
- **移動 (元が消えている) とコピー (元が残っている) の両方**を検出し、既定でどちらも確認する

### 非対象 (v1)

- **mIV Remote**: 提案 UI を出さない。worker も UI も core 側に閉じるので remote-ipc 版に影響しない
- **動画 / 音声のブックマーク**: `video_bookmarks` は id が PK の非一意テーブルで、コピーには
  id 再採番が要る (Phase 2)
- **ZIP エントリ単位の内容一致** (ZIP から展開したばら画像 ↔ ZIP 内エントリ): Phase 2
- **PDF ページ単位の内容一致**: 対応するバイト同一性が無いため原理的に不可 (容器一致で足りる)
- **PDF パスワード**: リネーム移行 (`rename_key_migration::run_at` の step 2) は
  DPAPI 保存済みパスワードを新キーへ移すが、**復元では移さない** (A3a 実装時に確認、2026-08-21)。
  リネームは同じファイルなので移さないと開けなくなるが、復元は**別のパスにある別ファイル**で、
  §2 の復元範囲 (編集・表示状態・★・タグ) にパスワードは入っていない。資格情報を利用者の
  明示操作なしに増やさない側に倒す。コピー先は初回オープン時に 1 回入力してもらう

---

## 3. 識別方式

### 3.1 3 段照合

通常操作でディスク I/O を 1 バイトも発生させないことが設計の要。

| 段 | 内容 | コスト | 発生条件 |
| --- | --- | --- | --- |
| 0 | size 一致 | メモリ照合のみ | 毎フォルダ (数百件でも μs) |
| 1 | 先頭 64KB + size の SHA-256 | 1 シーク + 64KB read | size が一致したときだけ |
| 2 | ファイル全体の SHA-256 | サイズ分の read | 段 1 も一致したときだけ |

- **size はフォルダ走査で既に取得済み** (`App::image_metas: Vec<Option<(mtime, size)>>`、
  [src/app/folder_scan.rs:399](../src/app/folder_scan.rs:399))。段 0 に追加 I/O は無い。
- 巨大ファイルも段 2 は全体を読む (打ち切らない)。段 1 まで一致していれば同一の可能性が高く、
  部分ハッシュで妥協する理由が無いため。I/O 負荷が気になる利用者には設定 OFF を用意する (§7)。
- 段 2 の実測 (開発機、414MB を SHA-256): **cold 寄り 463ms (893MB/s) / warm 348ms (1,189MB/s)**。
  20MB の JPEG で約 20ms、500MB の ZIP で 0.5 秒 (SSD)。HDD では read が律速する。

### 3.2 台帳 `content_identity.db`

```sql
CREATE TABLE edit_origin (
  file_key      TEXT PRIMARY KEY,  -- path_key::normalize_keep_drive した物理ファイル
  size          INTEGER NOT NULL,
  head_hash     TEXT NOT NULL,     -- 先頭 64KB + size の SHA-256
  full_hash     TEXT,              -- 全体 SHA-256 (未計算なら NULL)
  hashed_mtime  INTEGER NOT NULL,  -- 再ハッシュ要否の判定
  kind          TEXT NOT NULL,     -- image / zip / pdf / convertible
  last_edit_at  INTEGER NOT NULL,  -- 候補が複数あるときの既定選択に使う (0 = 未編集)
  has_restorable_content INTEGER NOT NULL
                                    -- 1: 復元可能な状態を持つ、0: 検出 hash cache のみ
);
CREATE INDEX edit_origin_full ON edit_origin(full_hash);

CREATE TABLE restore_declined (
  full_hash  TEXT NOT NULL,
  target_key TEXT NOT NULL,
  PRIMARY KEY (full_hash, target_key)
);
```

> **`last_edit_at` は「編集」でしか進まない** (A1 実装時に確定、2026-08-21)。mIV の編集は
> 非破壊なのでファイルのバイト列は変わらず、2 回目以降の編集は必ず「再ハッシュ不要」経路を
> 通る。つまりこの列を維持するのはその経路だけであり、読書位置の記録 (ページ送りごとに発火)
> を同じ経路に流すと**読んだだけで `last_edit_at` が進む**。それでは §5 の「別々に編集された
> 候補のうち新しい方を既定にする」が成り立たず、読んだだけのコピーが既定になって、
> 実際の編集を持つ側から復元されない。そのため記録は `Edit` / `ViewingState` の 2 種類を持つ:
> **行の作成は両方が行い、`last_edit_at` を進めるのは `Edit` だけ**。`ViewingState` だけで
> 作られた行は `last_edit_at = 0` (= 未編集) になり、A3 の既定選択では最後に回る。
>
> **`has_restorable_content` は `last_edit_at` と独立した事実である** (A2 修正時に確定、
> 2026-08-21)。A1 の `Edit` / `ViewingState` はどちらも実際の復元対象状態から発火するため
> `1` を設定する。本の続きは `ViewingState` で `last_edit_at = 0` の行を作るが、復元範囲内
> なので有効な復元元である。一方、A2 が照合済みファイルの hash を cache するだけの行は
> `0` で作り、既存の `1` は変更しない。起動時の size index は `1` の行だけを載せる。
> これにより hash cache 行が次回検出を抑止せず、A1 が後から同じ行を記録すれば再 hash なしで
> `1` へ昇格する。
>
> **A4 schema migration (2026-08-22)**: 未リリース store でも A1 build を実データで動かした
> 開発環境には旧 schema が残るため、schema 作成と upgrade は 1 関数へ集約し、
> `PRAGMA user_version` で管理する。unversioned A1 table に
> `has_restorable_content` が無い場合は
> `ALTER TABLE ... ADD COLUMN ... INTEGER NOT NULL DEFAULT 1` で上げる。A1 時代の行は検出 cache
> ではなく全て実際の記録なので default `1` が正しい。期待 schema へ上げられない場合は空 index
> として扱わず、台帳 state を `Unusable(detail)` にして検出 / backfill を開始せず利用者へ通知する。
>
> **`file_key` は path キーなので、[rename_key_migration.rs](../src/rename_key_migration.rs) の
> `STORES` 表へ必ず追加すること。** この表はリネーム移行と削除 worker の hard purge が
> 共有する正本で、追加を忘れるとアプリ内リネーム後に台帳だけ旧パスを指す。
>
> **メモリ索引は台帳の authoritative snapshot とし、削除 / key rewrite 後に増分推測しない**
> (2026-08-22)。`STORES` descriptor の transaction 成功境界で
> `content_identity.db / edit_origin.file_key` の mutation effect を立て、リネーム、hard purge、
> purge journal 再試行、孤児メタデータ整理の完了 report から App の同じ入口へ返す。App は旧索引を
> 段 0 から即時 gate して検出 / backfill を cancel し、`GlobalIoSemaphore` Low priority の
> cancellable worker で台帳全件を再読込する。再読込中に commit した recorder / detection /
> backfill / restore promotion の行は global update channel または完了 report で Loading queue に
> 積み、全件 snapshot へ merge してから Ready に戻す。段 0 は引き続きメモリだけを参照し、
> SQLite を開かない。

### 3.3 記録のタイミング

- **編集の確定点** (`save_mask_with_sidecar` / `save_conceal_with_sidecar` / `set_page_params` 等)
  でそのページの物理ファイルを worker に投げ、ハッシュを 1 回だけ記録する。
  そのファイルは今まさに表示しており OS キャッシュに載っているので実質無コスト。
- **既存編集の遡り (backfill) は一括スキャンしない**。「編集を持つページを含むフォルダを開いた
  ときに、台帳に無いものだけ裏でハッシュする」で自然に埋まる。開発機の実データで
  **編集を持つ物理ファイルは 921 件** (編集キー 1642 件 / jpg 466・png 395・pdf 30・zip 12) なので、
  通常利用の数セッションで完了する。
- backfill は通常の物理フォルダを開いた時だけ、その一覧の presence 集合から物理ファイルを
  容器単位で dedup して `ViewingState` として記録する。検出と同じ `GlobalIoSemaphore` Low priority、
  folder 切替 cancel、chunk 間 cancel を使い、UI thread ではファイルも SQLite も読まない。
- `(file_key, size, hashed_mtime)` が一致する限り再計算しない。

---

## 4. 変換アーカイブ (RAR / 7z / LZH / 入れ子入り ZIP)

**再変換もキャッシュ ZIP の共有も不要**。理由は、変換キャッシュ ZIP の置き場が
**元ファイルパスだけの純関数**だから ([src/archive_cache.rs:52](../src/archive_cache.rs:52)):

```
cache_zip = <cache_root>/<h[..2]>/<h>/<basename>.zip   where h = sha256(path_key::normalize(src))
```

mtime も size も内容も混ざっていないので、**コピー先の将来のキャッシュ ZIP パスを、変換前に
計算できる**。編集キーを先に付け替えておけば、後日そのコピーを開いて変換したときに
キャッシュはちょうど予測したパスに生成され、編集内容が既にそこにある状態になる。
`archive_cache.db` は一切触らない (= 既存の 1 行 = 1 ファイル所有という前提を壊さない)。

またキー基底は 2 種類ある ([src/metadata_transfer.rs:4028](../src/metadata_transfer.rs:4028) の
`PortableVirtualKeyBase`)。直接閲覧した場合は Source 基底、変換して開いた場合は
ConvertedCache 基底になるため、**アーカイブ 1 ファイルの復元では 4 面をまとめてコピーする**:

| # | 旧キー | 新キー | 主な中身 |
| --- | --- | --- | --- |
| 1 | `<old>` | `<new>` | ★ / タグ / 代表サムネピン / 本の続き / 見開き |
| 2 | `<old>::` prefix | `<new>::` prefix | 直接閲覧した ZIP のページ編集 |
| 3 | `cache_zip(old)` | `cache_zip(new)` | 変換キャッシュ側のコンテナ状態 |
| 4 | `cache_zip(old)::` prefix | `cache_zip(new)::` prefix | 変換キャッシュ側のページ編集 |

metadata_transfer は 2 と 4 が同時に存在するとエラーにする (import は 1 基底しか選べないため) が、
本機能は**両方コピーすればよいだけ**なのでエラーにしない。

> **注意**: `cache_zip_path_for` は `path_key::normalize` (= **ドライブ文字を落とす**) を使うため、
> `C:\a\x.rar` と `D:\a\x.rar` は同じキャッシュ ZIP を指す。別ドライブの同一相対パスへコピーした
> 場合、3 / 4 のキーは元から一致していてコピーが no-op になる。**正しい挙動だがテストで明示する。**

同じ理由で `book_resume` / `spread` / `view_trim_books` も drive-stripped キー
(`StoreKeyNormalization::DriveStripped`) なので、ストアごとの正規化は `STORES` 表の
`normalization` に従うこと。自前で正規化を選ばない。

---

## 5. 検出フロー

1. **フォルダ読み込み完了時**、`image_metas` の size を、起動時にメモリへ載せた
   `has_restorable_content = 1` の台帳行の size 集合と突き合わせる (I/O ゼロ)。
   台帳は数百〜数千行なので数十 KB。
   起動時ロードは既存の `adjusted_page_keys` 等 ([src/app.rs:12855](../src/app.rs:12855)) に相乗りする。
2. ヒットした項目だけ worker へ。**`GlobalIoSemaphore` の Low 優先度**
   ([src/io_semaphore.rs](../src/io_semaphore.rs))、フォルダ切替でキャンセル。UI スレッドからは
   一切 I/O しない (CLAUDE.md「UI スレッドでの同期 I/O は即 worker 化する」)。
3. 段 1 → 段 2 で確定。結果は `has_restorable_content = 0` の台帳行としてキャッシュする。
   再訪時に対象を stat した `(size, hashed_mtime)` が一致すれば、その行の
   `head_hash` / `full_hash` を再利用してファイルを読まない。
4. 次のものは無言で捨てる:
   - 復元先に既に編集がある (サイドカーと同じ「既存が authoritative」)
   - `restore_declined` に載っている
   - ハッシュ不一致
5. 残りを mpsc で UI へ返し、復元ウィンドウを出す。

- **フルスクリーン中は提示しない**。検出結果は保持し、一覧に戻ったときに出す。
- 候補が複数ある (同一内容のファイルが複数箇所で別々に編集されている) 場合は、
  `last_edit_at` 降順 → `file_key` 昇順で並べる。`last_edit_at` がすべて 0 の backfill 行も
  順序を安定させ、既定の先頭候補以外を利用者が選べるようにする。編集データ量は
  既定選択にも並べ替えにも使わない。

---

## 6. UI

### 6.1 復元ウィンドウ (非モーダル `egui::Window`)

```
編集内容の復元

このフォルダに、以前編集したファイルと内容が同じファイルが 3 件あります。
編集内容 (補正・消しゴム・モザイク・注釈・トリミング・★・タグ) を複製しますか?

1 件、複数のコピー元があります。コピー元を選択してください。

  [すべて選ぶ] [すべて解除]

  ☑ IMG_0421.jpg   ← D:\photo\2025\IMG_0421.jpg (移動)
  ☑ chapter03.cbz  ← コピー元を選択 (3 件) [D:\manga\chapter03.cbz (コピー元は残っています) ▼]
  ☐ scan.pdf       ← E:\old\scan.pdf (コピー元は残っています)

  □ 次から確認しない (環境設定で元に戻せます)

                                         [復元する]  [閉じる]
```

- **モーダルにしない**。検出はフォルダを開いた 1〜2 秒後に非同期で確定するので、
  モーダルだと閲覧を中断させる。`common_modal_dialog_open` には登録し、背面グリッドへの
  ホイール / キー漏れだけ止める (CLAUDE.md「ダイアログ (egui::Window)」)。
- **1 件ずつ聞かない**。フォルダ丸ごとコピーで数百件になるため、フォルダ単位の一括提示。
- **[すべて選ぶ] / [すべて解除]** を必ず置く。
- 移動 (元が消えている) とコピー (元が残っている) を行に明示する。既定チェックは両方 ON。
- 複数のコピー元を持つ行が 1 つでもあれば、ウィンドウ上部に該当行数と選択を促す注意を
  表示する。単一候補だけなら注意を出さず、従来の行表示も変えない。
- 複数候補の行は候補数を明示し、選択欄自体に現在のコピー元パスと移動 / コピーの別を
  表示する。候補を切り替えると、その行の実際の復元元も同じ選択へ切り替わる。
- 復元範囲は一括 (項目別チェックボックスは持たない)。

### 6.2 「もう聞かない」の粒度 (決定)

| 操作 | 効果 |
| --- | --- |
| 行のチェックを外して `[復元する]` | 外した行を `restore_declined` に**恒久記録** |
| `[閉じる]` | **記録しない** (次にそのフォルダを開いたら再提示) |
| `□ 次から確認しない` | **設定を OFF** (全体停止、§7) |

- **「このフォルダ以下では聞かない」は作らない**。使わない利用者は全体 OFF で足りる、という判断。
- [閉じる] と × は何も記録せず、同じフォルダを次に開いたときに再提示する。右クリックからの
  手動再確認は Phase 2 とし、Phase 1 には置かない。

---

## 7. 設定

環境設定 → フォルダ:

```
☑ コピー・移動したファイルの編集内容を復元するか確認する
   (OFF にすると照合のためのファイル読み取りを一切行いません)
```

- `Settings::edit_restore_prompt_enabled: bool` = **既定 true**
- **OFF = 検出スキャンと folder-open backfill を完全停止** (段 0 の size 照合や presence 選別も
  開始しない → folder open 起因の matching / backfill I/O はゼロ)
- **記録側 (編集保存時のハッシュ台帳への記録) は OFF でも継続する**。記録は編集した瞬間の
  1 ファイル 1 回だけで実質無コストであり、ここも止めると後で ON に戻したときに過去の編集を
  一切拾えなくなるため。設定文言も「確認する / しない」であって「記録しない」ではない。
- **A4 での衝突解決**: backfill も記録ではあるが、編集確定に伴う記録と違って folder open を
  起点に新しい file read を発生させる。このため OFF では backfill を止める。設定の補足文
  「OFF にすると照合のためのファイル読み取りを一切行いません」から利用者が期待する挙動を優先し、
  OFF 中も継続する「記録側」は、その場で利用者が確定した編集 / 表示状態の A1 記録だけを指す。

---

## 8. 実装構造

### 8.1 コピー実処理は `STORES` 駆動の `copy_store` を新設する

既存のコピー実装は 2 つあるが、どちらもそのままでは使えない / 使うべきでない。

- `App::copy_book_page_edit_key` ([src/app.rs:30612](../src/app.rs:30612)) — 型付き 8 ストア横断コピー。
  ただし **App のハンドル経由 = UI スレッド前提**で worker から呼べない。
- `rename_key_migration::migrate_store` — **DB ファイルを直接開く worker 実装**で、
  `STORES` 表 (A1 の `edit_origin` 追加後は 22 descriptor) を網羅。

復元は worker で走るので、**`rename_key_migration` (または同じ `STORES` を共有する兄弟モジュール)
に `copy_store` / `copy_exact` / `copy_prefix` を足すのが構造的に正しい**。同じ表を使うので、
将来ストアが増えてもリネーム・削除 purge・復元が同時に追随する。

```sql
-- unique テーブル: 列は PRAGMA table_info で取得し、キー列だけ差し替える
INSERT OR IGNORE INTO t (c1, c2, ...) SELECT ?new, c2, ... FROM t WHERE c1 = ?old;
```

- `INSERT OR IGNORE` が「復元先に既存があれば既存優先」を自然に満たす。
- prefix コピーは `move_prefix` と同じく **LIKE ではなく `substr` 等値**で列挙する
  (path 中の `%` / `_` を誤爆させない)。長さ引数は SQLite では文字数なので `chars().count()`。
- **`rating.db` の `source_path` 列は導出値**。`migrate_store`
  ([src/rename_key_migration.rs:750](../src/rename_key_migration.rs:750)) と同じ再計算 UPDATE が
  copy 側にも要る。
- **`edit_preview_cache.db` の `cached_path` と `annotation_layers_json[].path` は
  row が所有する WebP の絶対パス**。generic copy の後、その mapping で実際に insert できた row
  だけを per-store fixup へ渡す。下地 + 全注釈 layer が現行の source-key directory に揃っている
  ことを先に確認し、同じ content-hash filename を destination-key directory へコピーしてから 2 列を
  書き換える。source file が 1 つでも欠ける / layout が不正なら destination row を削除し、dangling row
  を増やさない。destination conflict は generic `INSERT OR IGNORE` の結果に含まれないため触らない。
  `copy_exact` 自体は store を知らないまま維持する。
- `reading_history.path` は destination raw path なので、既存の non-generic descriptor fixup が exact
  copy 後に書き換える。`edit_origin` の destination metadata は後段の `mark_restored_origin` が target
  file を stat して昇格時に確定する。
- **非一意テーブル (`video_bookmarks`)** は id 再採番が要るので v1 の対象外 (Phase 2)。
- busy_timeout を付けて本体側の接続と共存する (rusqlite の既定は 5 秒。
  `journal_mode` 変更だけでは効かない点に注意)。

### 8.2 復元後の後始末 (抜かすと台無しになる)

1. **コピー先フォルダのサイドカー `mimageviewer.dat` にもミラーする**
   (`App::with_sidecar_coords_mut`)。ここを抜かすと、そのフォルダを次に丸ごと移動したときに
   また失われる。
2. **メモリ presence 集合の更新** (`adjusted_page_keys` / `mask_page_keys` / `conceal_page_keys` /
   `local_adjust_page_keys` / `comic_page_keys` / `rotation_page_keys`)。グリッドのバッジと
   スマートフォルダ集計が参照する。
3. **復元先キーに materialize 済みの read-once cache を対象限定で失効する**。
   `comic_docs` の空 `Vec` (DB の no-row を表す sentinel)、`rotation_cache` の `None`、
   復元前の edit preview を保持する thumbnail だけを worker report の destination page key で
   未読 / 再要求へ戻す。特に `comic_docs` を全 clear せず、実際に復元された comic key だけを
   `remove` する。
4. 通常の物理フォルダ一覧がまだ current なら、rename 完了と同じ `current_folder` prefix から
   `rehydrate_page_edit_state_for_current_items` を呼び、復元先の idx-keyed page edit state
   (補正 / ローカル調整 / crop / 表示トリム / 消しゴム / 隠蔽 / 注釈 presence) をその場で再構築する。
   完了待ちの間に検索・snapshot 等の合成 view へ移った場合は `is_physical_folder_listing()` が
   false になるため `clear_page_edit_state` のみとし、overlay を出さない既存契約を維持する。
   続けて `rating_cache.clear()` / `invalidate_rating_counts_cache()` / `clear_tags_cache()` を行う。
5. 復元先にも `edit_origin` 行を作る (以後そのファイルが新たなコピー元になり得る)。

**A3a / A3b 実装メモ (2026-08-21)**:

- `restore_candidates_at(data_dir, selected, declined)` は App / egui に依存しない batch 入口。
  全選択候補の物理 path exact / `::` prefix と、変換アーカイブの予測 cache path exact /
  `::` prefix mapping を先に集約し、`copy_stores_at` を 1 回だけ呼ぶ。
- copy は同じ `STORES` を走査し、`unique: true` の 21 descriptor を対象にする。
  現行 22 descriptor のうち `unique: false` の `video_bookmarks` だけは id 再採番が必要なため
  v1 対象外。正規化は各 descriptor の `normalization` から決める。候補 1 件でも 100 件でも
  copy 対象 DB は 21 回だけ開き、`content_identity.db` の昇格 / 拒否記録と runtime state
  読み出しも候補単位では開き直さない。
- worker report は DB copy 後の destination 状態から sidecar mirror、6 種類の presence 差分、
  sidecar-backed edit key の和集合を作る。A3b の短命 worker 完了後、既存 App owner へそれを適用し、
  `finish_book_page_edit_mapping` と同じ cache invalidation を呼ぶ。sidecar の同期 flush は
  増やさない。
- target の `edit_origin` は `has_restorable_content = 1` へ昇格し、A2 の次回 index で復元元に
  なる。チェックを外して [復元する] を押した行だけを `restore_declined` へ記録する。
- 非モーダル `egui::Window` の可視性は
  `fullscreen_idx.is_none() && restore_pending.is_none() && prompt.is_some()` で一元化し、
  描画と背面入力 block の両方が同じ述語を使う。フルスクリーン中または先行 batch 完了待ちは
  候補を保持したまま false。
- [閉じる] / × は候補を閉じるだけで、`restore_declined` も復元 request も作らない。

---

## 9. テスト方針

`rename_key_migration` の既存テスト群 (`migrates_exact_file_keys_across_stores` など) が
ほぼそのまま雛形になる。最低限:

- 画像 / ZIP / PDF の exact + prefix コピーが全ストアに渡ること
- **変換アーカイブ**: `cache_zip_path_for_data_dir` で予測した新パス配下へ 4 面がコピーされること
- **変換アーカイブ・別ドライブ同一相対パス**: 3 / 4 が no-op になること (ドライブ文字除去の帰結)
- 復元先に既存編集があるとき上書きしないこと (`INSERT OR IGNORE`)
- `restore_declined` に載った組み合わせを再提示しないこと
- 段 0 で size 不一致なら**ファイルを一切開かない**こと (I/O 呼び出し回数を数える)
- 設定 OFF で検出 worker が起動しないこと
- `rating.source_path` が新キー基準に再計算されること
- edit preview の下地 + 注釈 layer が destination key directory に複製され、row の両 path 列が
  destination files を指すこと。source / destination のどちらを invalidate しても他方の files が残ること
- source WebP が 1 つでも欠ける edit preview は destination row を残さないこと

---

## 10. フェーズ

| Phase | 内容 |
| --- | --- |
| 1 | 台帳 DB + 記録 worker + 3 段照合 + 検出 worker + 非モーダル復元ウィンドウ + 設定 1 個 + `STORES` 駆動 `copy_store`。**画像 / ZIP / PDF / 変換アーカイブ (4 面キー) を最初から対象**、移動・コピー両方 |
| 2 | ZIP エントリ単位の内容ハッシュ (ZIP ↔ ばら画像の相互復元)、動画 / 音声のブックマーク (id 再採番)、右クリックからの手動再確認 |

### 10.1 Phase 1 の分割 (実装を出す単位)

Phase 1 は 1 回の差分にすると大きすぎてレビューが効かないので、**3 段に割って順に出す**。
各段はそれ自体で完結し、テストが緑になり、次の段が無くても壊れない。

| 段 | 中身 | 触る場所 | 完了の判定 |
| --- | --- | --- | --- |
| **A1 台帳と記録** | `content_identity.db` (§3.2)、3 段照合のハッシュ計算 (§3.1)、編集確定点からの記録 worker (§3.3)、`STORES` への `file_key` 追加 | 新規モジュール + 編集保存経路のフック | 編集すると台帳に 1 行増える。`(file_key, size, hashed_mtime)` が同じなら再計算しない。**UI は何も変わらない** |
| **A2 検出** | 起動時の台帳ロード、フォルダ読み込み完了時の段 0 照合、`GlobalIoSemaphore` Low の検出 worker、設定 1 個 (§7) | フォルダ走査完了フック + 新規 worker + 環境設定 | 候補が mpsc で UI へ届く (ログで確認)。**まだウィンドウは出さない**。設定 OFF で worker が起動しない |
| **A3a コピーエンジン** | `STORES` 駆動の `copy_store` / `copy_exact` / `copy_prefix` (§8.1)、変換アーカイブの 4 面キー (§4)、`restore_declined` 書き込み API、復元後の後始末境界 (§8.2) | `rename_key_migration` の copy sibling + `content_identity/restore.rs` | App / egui 非依存の入口をテストだけから駆動し、21 unique descriptor・4 面・後始末 report を検証する。**UI は何も変わらない** |
| **A3b 復元 UI** | **実装済み**。非モーダル復元ウィンドウ (§6)、A2 候補との配線、batch 復元 / `restore_declined`、A3a report の App owner への適用 | `ui_dialogs/content_restore.rs` + A2 poll 配線 | §9 の UI / lifecycle、1 / 100 候補の DB open 計測、UI snapshot を含むテストが通る |

**A2 実装時の確定事項 (2026-08-21)**:

- 物理フォルダ一覧は、最上位 surface が `Folder`、通常フォルダ走査が現在の
  `current_folder` に公開した `normal_folder_omitted_entries` marker が一致し、かつ
  ZIP / PDF 内一覧でも detached physical context でもないことを組み合わせて判定する。
  smart folder / 検索 / snapshot / subfolder expansion などの合成 surface は `Folder` でなく、
  ZIP / PDF は grid の container 状態で除外される。
- 検出先の hash cache は A1 と同じ hash 関数・台帳へ、
  `ContentIdentityTrigger::ViewingState` かつ `has_restorable_content = 0` の cache 観測として
  記録する。検出は編集でも復元可能状態の記録でもないため `last_edit_at` を進めず、
  新規行は `last_edit_at = 0` になる。既存の `has_restorable_content = 1` は維持する。

**A1 を最初に出す理由**: 台帳は使われ始めるまで空なので、**早く入れるほど実データが溜まる**。
A2 / A3 を実装している間に開発機の台帳が埋まり、A3 の実機確認が現実的なデータでできる。

**段をまたいで守ること**:

- **A1 の時点で `STORES` へ `file_key` を登録する** (§3.2 の注記)。ここを A3 まで遅らせると、
  その間のアプリ内リネームで台帳だけ旧パスを指す行が残る。
- **A2 の設定は「確認する / しない」であって「記録しない」ではない** (§7)。A1 の記録は
  設定 OFF でも動き続ける。A2 でこの分岐を実装するとき、A1 側を巻き込んで止めない。
- **UI スレッドから 1 バイトも読まない**。A2 の段 0 照合はメモリ上の size 集合との突き合わせだけ。
  段 1 / 段 2 は必ず worker (CLAUDE.md「UI スレッドでの同期 I/O は即 worker 化する」)。



---

## 11. 検討して採らなかった案

- **ファイル自体に ID を埋める (XMP 等)**: 開く側のコストはゼロだが、非破壊原則に反し、
  PDF ページ / ZIP 内画像 / 読み取り専用メディアで成立しない。
- **ファイルごとのサイドカー (`foo.jpg.miv`)**: 単体コピーでは一緒に運ばれず、今回の穴を塞げない。
- **エクスプローラー操作の監視**: 事後にしか分からず、監視コストも高い。
- **変換キャッシュ ZIP を複数の元パスで共有する**: `lookup` / `delete_entry` /
  `delete_missing_originals` / `clear_all` / `prune_to_size_limit_locked` / `total_size` の
  すべてが「1 行 = 1 ファイル所有」を前提にしており、参照カウント導入が必要になる。
  §4 のとおりキー付け替えだけで足りるので不要。
