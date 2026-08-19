# backlog §2.8 — RAR の直読み判定を、安い順・可視範囲だけに絞る

対象: [next-release-backlog.md](../next-release-backlog.md) §2.8。v3.1.2 で **A と B の両方**を
入れる (利用者判断 2026-08-19)。§2.3 の続きで、同じ typed state の上に載る。

**A を先に完成させ、B はその上に積む。** A だけでも「2 回目以降は走査ゼロ」になるので、
段階が分かれていることが後の切り分けに効く。可能なら A と B で作業を分けて報告すること。

## 0. 前提 (確認済み。ここは調べ直さなくてよい)

- **判定表 `converted_archive_cache_paths` はグリッドのためだけにある。** 本を開く経路は
  `spawn_archive_scan` ([archive_convert.rs:170](../../src/ui_dialogs/archive_convert.rs:170)) が
  `RarInspectionOrigin::ExplicitOpen` でその 1 冊分を自前で走らせ、判定表を参照しない。
  **事前判定を減らしても開く動作は変わらない。**
- 全走査 (`inspect_for_direct_read_run`、[rar_loader.rs](../../src/rar_loader.rs)) が必要なのは:
  - `decision`: `is_solid` と `has_encrypted_headers()` は**ヘッダで即**。
    entry ごとの `is_encrypted()` と nested archive の**不在証明**だけが全走査を要求する。
  - `summary`: 変換ダイアログとリモートの開く導線だけが使う。グリッドは使わない。
  - `resolved_path`: `open_listing_from_volume_with_key` のヘッダ操作だけで済み、
    128 件の `VOLUME_RESOLUTION_CACHE` を持つ。**全走査は不要。**
- thumbnail の cache key は `convertible_archive_cache_base_key(path, format, use_full_path_keys)`
  で **RAR 自身のパスと形式**から作られる。`req.mtime` / `req.file_size` も `..base` 経由で
  **RAR 自身のもの**。したがって **cache 参照に解決後のパスは要らない**。

## A. 安い順に並べ替える + キャッシュ限定要求

### A1. peek を全走査より前に出す

現在の worker ([app.rs](../../src/app.rs) の `start_converted_archive_cache_paths_refresh`) は
RAR に対して必ず `inspect_for_direct_read` を回してから `db.peek` を引く。peek に要るのは
`resolved_path` だけなので、**変換済み ZIP がある RAR でも毎回全走査を払っている**。

順序を変える:

1. 分割 RAR の volume 解決 (ヘッダ操作、既存 cache 付き)
2. `db.peek` — ヒットしたら `CachedZip` で確定。**全走査しない**
3. miss のときだけ `inspect_for_direct_read` を回して `Direct` / `Unavailable` を決める

**挙動が 1 つ変わる**: 直読み可能かつ変換済み ZIP も持つ RAR は、今まで `Direct` だったが
`CachedZip` になる。どちらも (mtime, size) で検証された正当な読み取り元なので採用してよいが、
**意図した変更として backlog に明記し、テストで固定すること**。黙って変えない。

### A2. `Pending` の間も cache-only 要求を組む

`make_load_request` の `ConvertibleArchive` 分岐
([app.rs:67692](../../src/app.rs:67692)) は `load_path()?` で `Pending` を弾いている。
その結果、**キャッシュに絵が揃っていてもタイルは形式アイコンのまま**になる
(P キーでピンした RAR が典型)。

- `Pending` でも要求を組み、**cache-only** であることを要求自体に持たせる。
  `LoadRequest` に真偽値を増やすのではなく、**読み取り元の状態を型で表す**こと
  (`skip_cache: bool` の隣にもう 1 つ bool を並べない)。
- worker は cache-only 要求が **miss したらアーカイブを開かずに終える**。タイルは
  `Pending` のまま残り、判定が解決した時点で通常要求として組み直される
  (§2.3 の `evict_thumbnail_for_reload` 経路がそのまま使える)。
- `Unavailable` は従来どおり要求しない。
- ピンの適用 (`apply_folder_thumb_pin`) は cache-only 要求にも効くこと。ピンが cache key を
  上書きするので、**ピンした絵がキャッシュにあれば即出る**のが A2 の主目的。

## B. 候補を可視範囲 + 先読み範囲に絞る

### B1. 候補集合

「可視範囲 + keep range + **それらの項目のピン依存先**」にする。ピン依存先は可視範囲外の
アーカイブを指すことがあるので、可視 `ConvertibleArchive` だけに絞ってはいけない
(現在 `pin_roots` は全項目から集めている。ここも範囲に合わせる)。

### B2. 範囲が動いたら追加投入する

今の worker は folder install 時に候補リストを固定して 1 回起動するだけ。keep range と
同じように追従させる。

- **スクロール中は投入しない。** 既存の `decide_prefetch_allowed`
  (スクロール入力から 100ms 未満は Block / 可視に Pending が残る間は Block / 3 秒で backstop)
  と同じ扱いに乗せる。新しい時間窓を作らない。
- 既に解決済みの候補を再投入しない。
- 範囲外へ出た候補の in-flight 判定は、**そのまま完了させてよい** (1 冊分なので)。
  結果は同 generation なら反映する。

### B3. ロールアップ絞り込みは従来どおり全体

`has_rollup_edit_filter()` ([settings.rs:1557](../../src/settings.rs:1557)) が true のときは、
範囲外コンテナを分類できないと絞り込み結果が不完全になり、**スクロールのたびに一覧の中身が
変わって見える**。この場合は folder 全体を候補にする (現在の挙動)。

他の消費者 (色フィルタ / スマートフォルダ / サブ展開 / 右クリックメニュー) は 1 項目ごとの
解決しか要求しないことを確認済み。**新たに folder 全体を要求する消費者を作らないこと。**

## 2. やらないこと

- **§2.7 (header の二重走査) をここで直さない。** 走る回数が減るだけで、二重走査は残る。
  別項のまま残す。
- 判定そのものを開く瞬間まで完全に遅らせる案 (可視サムネイルも判定なしで出す) は今回入れない。
  「サムネイルは出るが Direct か分からない」状態の扱いに設計判断が要る。
- `DECISION_CACHE_CAPACITY` を増やさない。
- 時間窓で競合を吸収しない (憲法 §2 規則 5)。B2 の抑制は既存の prefetch 判定の再利用であり、
  新しい窓を作るものではない。
- 開く経路 (`spawn_archive_scan`) に手を入れない。

## 3. テスト

### A

1. 変換済み ZIP を持つ RAR が **`inspect_for_direct_read` を呼ばずに** `CachedZip` へ解決する
   (呼び出し回数を test double か既存の `rar/inspection_begin` 計装で固定する)。
2. 直読み可能かつ変換済み ZIP も持つ RAR が `CachedZip` になる (A1 の意図した変更)。
3. cache 済みの `Pending` 候補が cache-only 要求で**サムネイルを出す**。
4. cache-only 要求が **miss したらアーカイブを開かない**。
5. ピンした RAR が `Pending` のうちにピンした絵を出す。
6. `Unavailable` は要求を作らない (回帰)。

### B

7. 範囲外の候補が `Pending` のまま残り、判定が走らない。
8. スクロールで範囲に入った候補が投入される。
9. スクロール中は投入されない (既存の抑制に乗っていること)。
10. `has_rollup_edit_filter()` が true のときは全候補が投入される。
11. §2.3 で入れた 4 テストと既存の archive / pin / smart folder テストが無修正で通る。
    **赤くなったら報告して止まる。**

## 4. 実測

`C:\tmp\miv-rar-thumbnail-test-100` (30,000 entry の RAR × 30 複製、133 本) で
**isolated profile** (`--data-dir` を使った使い捨て) に `--perf-log` を取り、次を数字で示す:

- **初回 (cold)**: 走査された候補数と、可視 12 件の `thumb/ready` までの時間
- **2 回目 (warm、変換 cache あり)**: `rar/inspection_begin` が何件出るか (期待は 0 に近い)

§2.3 の実測 (可視 12 件 0.801〜0.860 秒 / 候補 #130 が 4.021 秒) が比較対象。

## 5. ドキュメント

- [docs/virtual-folders.md](../virtual-folders.md): 判定の順序と候補範囲。
- [docs/async-architecture.md](../async-architecture.md): 範囲追従とキャンセル規約。
- [next-release-backlog.md](../next-release-backlog.md) §2.8 に結果を追記して閉じる。
  A1 の挙動変更 (`Direct` → `CachedZip`) を明記する。§2.7 は開いたまま残す。

## 6. 機械チェック

```
cargo fmt --all && cargo fmt --all -- --check
cargo check -p mimageviewer --bin mimageviewer-core
.\scripts\test-full.ps1
.\scripts\build-dev.ps1
```

commit / stage はしない。ブランチは `master`。
報告は **A と B を分けて**書き、変更ファイル一覧、追加テスト、§4 の実測値を含める。
