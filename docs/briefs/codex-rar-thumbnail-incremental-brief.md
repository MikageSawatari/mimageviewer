# backlog §2.3 — RAR が多いフォルダで、全件判定が終わるまでサムネイルが出ない

対象: [next-release-backlog.md](../next-release-backlog.md) §2.3 (専用スレ >>246, >>249-250)。
v3.1.2 で対応する、と利用者へ回答済み。**5ch 由来 4 件のうち一番重い。**

症状: ローカルの RAR でも、フォルダを開いた直後は 2〜10 秒ほど形式アイコンのままになる。

## 0. 確定している事実 (実測 + source inspection)

- `nav/archive_cache_peek` は 133 RAR に対して **3,280.7 / 3,913.6 / 3,363.4ms**。
  最後の重い RAR のサムネイル ready まで約 5.91 秒。
- 原因の構造は 3 つ重なっている:
  1. `start_converted_archive_cache_paths_refresh`
     ([app.rs:23854](../../src/app.rs:23854)) が候補を**1 worker で順番に**調べ、RAR ごとに
     `inspect_for_direct_read` を実行する。
  2. 結果は**全候補の確認が終わってから** `ConvertedArchiveCachePathsResult` として
     1 個の `HashMap` で届く ([app.rs:24009](../../src/app.rs:24009))。途中結果を公開しない。
  3. `make_load_request` の `ConvertibleArchive` 分岐
     ([app.rs:67354](../../src/app.rs:67354)) が
     `converted_archive_cache_paths.get(&archive_key)?` で **`None` を返して終わる**ので、
     heavy thumbnail queue に要求すら入らない。
- **map の「entry が無い」が 2 つの意味を兼ねている**のが構造上の芯:
  「まだ調べていない」と「調べた結果、直読みできず変換 cache も無い」。後者は本当に
  サムネイル source が無いので `None` が正しく、前者だけが早すぎる。

## 1. やること

### 1.1 候補ごとの状態を typed にする

`HashMap<String, PathBuf>` をやめ、候補ごとに次を持つ:

| 状態 | 意味 |
| --- | --- |
| `Pending` | この generation でまだ調べていない |
| `Direct(PathBuf)` | 直読み可能。header 解決済みのパス |
| `CachedZip(PathBuf)` | 変換済み ZIP が cache にある |
| `Unavailable` | 調べた結果、直読み不可かつ cache 無し (変換が必要) |

**entry の有無を pending / unavailable の兼用 sentinel にしない。** これを守らないと
今と同じ曖昧さが別の形で戻る。

`make_load_request` は `Direct` / `CachedZip` で要求を作り、`Pending` / `Unavailable` では
今と同じく `None` を返す。**呼び出し側から見た型が 4 状態に分かれていること**が要点で、
「まだ」と「無い」を区別できるようになる。

### 1.2 1 件ごとに公開する

worker は候補 1 件の判定が終わるたびに結果を送り、UI 側は届いた順に同 generation の
map へ反映する。**全件完了を最初のサムネイル表示の条件にしない。**

- 既存の `ConvertedArchiveCachePathsResult` は「全件まとまった 1 通」を前提にしているので、
  逐次通知に合う形へ変える。既存の perf event `nav/archive_cache_peek` は
  **全体の総括として残す** (peeked / hits / ms は今の意味のまま最後に 1 回)。
- 反映のたびに、その key に依存する tile の再要求が走ること。現在
  `poll_converted_archive_cache_paths` が `changed_keys` から依存 index を求めて
  invalidate している経路を、逐次でも同じ意味で働くようにする。
- **pin 依存 (`pin_archive_dependencies`) も逐次で正しく解決すること。** container pin が
  変換対象アーカイブを指す経路 ([app.rs:23895](../../src/app.rs:23895)) を壊さない。

### 1.3 可視範囲を先に判定する

候補の判定順を、spawn 時点の可視範囲 → keep range → そこからの距離順にする。
全件を待たずに、いま見えているものから埋まる。

- **スクロールに追従した動的な優先度変更は今回入れない。** PDF pool の
  `promote_to_high_normal` に当たる仕組みは別案件。報告された症状 (フォルダを開いた
  直後の 2〜10 秒) は spawn 時点の順序で解消する。この判断を backlog に明記すること。
- **同期 header scan を UI thread へ戻さない。** 判定は worker のまま。
- 既存の heavy I/O 予算を無視して候補数ぶん thread を spawn しない。worker を増やすかは
  計測して決める。増やす場合も上限を持たせ、根拠を報告に書く。

### 1.4 同じ RAR を二度 header scan しない

完了条件に「同一 RAR の header 判定回数を計装または test double で固定し、一覧判定直後の
サムネイル生成で同じ全 entry scan を繰り返さない」がある。

- `rar_loader` の `DECISION_CACHE_CAPACITY` は 32
  ([rar_loader.rs:15](../../src/rar_loader.rs:15))。full inspection を保持するので
  **単に増やす修正にはしない** (メモリが増える)。
- 1.1 の typed 状態が generation 内の正本になるので、**サムネイル要求と open は
  そこから解決済みパスを受け取る**形にする。判定を持っているのに再実行する経路が
  残っていないか、`inspect_for_direct_read` の呼び出し元を全部数えてから決める
  ([metadata_ops.rs:1149](../../src/app/metadata_ops.rs:1149),
  [smart_folder.rs:1554](../../src/app/smart_folder.rs:1554),
  [archive_convert.rs:178](../../src/ui_dialogs/archive_convert.rs:178),
  [archive_job.rs:1789](../../src/remote_ipc/archive_job.rs:1789) と本経路)。
- **範囲が 1.1-1.3 より大きくなると判断したら、そこで止めて報告する。**
  1.1-1.3 だけでも症状は解消するので、無理に押し込まない。

### 1.5 世代とキャンセル

フォルダ切替 / 再読み込みで cancel し、`items_generation` を照合して**旧フォルダの
途中結果を新しい一覧へ反映しない**。逐次公開にすると取り違えの窓が増えるので、
**1 件ごとに generation を確認する**こと。

## 2. やらないこと

- `DECISION_CACHE_CAPACITY` を増やすだけの修正にしない。
- スクロール追従の動的優先度を入れない (§1.3)。
- **§2.6 (ダブルクリックが時々無反応) をこの修正で解決扱いにしない。** サムネイル遅延が
  原因という仮説は、stress folder で open が成立したことで既に否定されている。
- 時間窓・遅延・retry で症状を隠さない (憲法 §2 規則 5)。
- 変換 (実際に ZIP を作る) 処理には手を出さない。ここは判定と cache 参照だけ。

## 3. テスト

1. typed 状態の遷移: `Pending` → `Direct` / `CachedZip` / `Unavailable`。
   `Unavailable` では要求を作らず、`Pending` とは型で区別できること。
2. 逐次公開: 候補 3 件のうち 1 件目が解決した時点で、その tile の要求が作れること
   (残り 2 件が `Pending` のままでも)。
3. 世代: 旧 generation の逐次結果が新しい一覧へ反映されないこと。
4. pin 依存: container pin が指す変換対象アーカイブが逐次経路でも解決されること。
5. 判定回数: test double か計装で、同一 RAR の header 判定が一覧判定 + サムネイル生成で
   **1 回**であることを固定する (1.4 を縮小した場合は、実際に達成した回数を固定して
   報告に差分を書く)。
6. 既存の archive cache / pin / smart folder のテストが無修正で通ること。
   **赤くなったら報告して止まる。**

## 4. 実測で確認すること

実装後、`C:\tmp\miv-rar-thumbnail-test-100` (30,000 entry の RAR を 30 個複製、
代表画像が末尾) で `--perf-log` を取り、**可視 RAR のサムネイルが全 133 件の判定完了を
待たずに順次表示される**ことを数字で示す。手順と数字を報告に書く
(利用者が実機確認するときの比較対象になる)。

## 5. ドキュメント

- [docs/virtual-folders.md](../virtual-folders.md): 変換対象アーカイブの判定と
  cache 参照の流れが変わるので更新する。
- [docs/async-architecture.md](../async-architecture.md): 逐次公開とキャンセル規約。
- [next-release-backlog.md](../next-release-backlog.md) §2.3 に結果を追記して閉じる
  (エントリ末尾に追記。冒頭の記述を消さない)。§1.3 で見送った動的優先度も明記する。

## 6. 機械チェック

```
cargo fmt --all && cargo fmt --all -- --check
cargo check -p mimageviewer --bin mimageviewer-core
.\scripts\test-full.ps1
.\scripts\build-dev.ps1
```

commit / stage はしない。ブランチは `master`。
報告には変更ファイル一覧、追加テスト、§4 の実測値、1.4 の到達範囲を含める。
