# S2 レビュー: サイドカー確認の再訪時再利用 (v4.0.0 出荷前・追加分)

対象: 作業ツリー未コミット差分 (HEAD = `a5a1fc43b`)
`src/sidecar_import/probe_reuse.rs` (untracked, 267 行) / `src/sidecar_import.rs` /
`src/sidecar.rs` / `src/app/sidecar_restore.rs` / `src/app.rs` の該当 hunk /
`docs/sidecar-confirmation-reuse-plan.md` (untracked)

本レビューはソース読解のみ。アプリの起動・ビルド・テスト実行は行っていない。
実行時の挙動に関する記述はすべて「コードを読んだ限りの推定」であり、計測値は
実装者が `docs/sidecar-confirmation-reuse-plan.md` §5-6 と
`target/sidecar-confirmation-reuse-20260921/measurement.md` に残した記録の引用である
(観測者 = 実装担当。レビュアー自身の観測ではない)。

---

## 0. 要約

| 重要度 | 件数 |
| --- | --- |
| P1 (出荷前必須) | **0** |
| P2 | **4** (S2-1 〜 S2-4) |
| P3 | **8** (S2-5 〜 S2-12) |

**中核の安全性は成立していると読める。** 「同期済み」の判定に使う入力
(pending/failed writer、全 bytes SHA-256 を含む disk identity、要求 family の
marker、最後の source revalidation) は**毎回すべて再取得**され、再利用されるのは
「byte 同一の入力に対する決定的な parse + validate の結果」だけである
(`src/sidecar_import/probe_reuse.rs:51-94`)。復旧データを誤って skip する経路は
見つからなかった。

**同梱判断: v4.0.0 には入れず、v4.0.x へ送ることを推奨する。**

理由 (優先順):

1. **外したときの損失が小さく、実測されている。** 失うのは Ctrl+↑↓ 1 回あたり
   約 150 ms の待ちで、しかも同一親フォルダを行き来する場合に限られる。
   v4.0.0 の主題 (コレクション) とは独立で、この機能が無くても v4.0.0 の
   レビュー指摘は 1 件も残らない。
2. **入れたときのリスクが「正しさ」ではなく「常駐メモリ」に出る** (S2-1)。実測で
   1 フォルダあたり約 148 MiB、上限 256 MiB の parse を**ナビゲーションを跨いで**
   保持する。これは既存の `evict_verified_clean` / `sidecars.clear()` が
   「長時間稼働時のメモリリーク防止」として明示的に捨てている物
   (`src/sidecar.rs:1084-1088`, `src/app.rs:27353-27356`) を再導入する方向であり、
   開発機 (RTX 4090 / 大容量 RAM) 以外での影響が未確認のまま出荷直前に入ることになる。
   設定で無効化できず、解放点も無い。
3. **効いているかを出荷後に確認する手段が無い** (S2-2)。perf イベントが 1 つも
   追加されておらず、hit も miss 理由も `analyze_perf.py` から見えない。
   リリース前 smoke (`perf_smoke.ps1` / `check-idle-health.ps1`) でも、この
   fast path が動いたかどうかは判別できない。
4. 計画 §4 が自ら挙げた回帰のうち **writer 競合系が丸ごと未テスト** (S2-4)。
   この 1 点だけなら追加して同梱でもよいが、1.〜3. と合わせると出荷直前に
   入れる理由が弱い。

v4.0.x へ送る場合、S2-1 の代替案 (S2-12: parse を保持しない「検証証明書」方式)
を検討すると、実測上の節約の大部分 (prepare 約 90 ms / probe 全体 約 125 ms) を
**メモリ保持ゼロ**で得られる可能性がある。こちらの方が構造的に小さい。

---

## 1. 確認して問題が無かった点

数が多いので先に列挙する。いずれもソース読解による確認。

- **証明の作成条件は計画どおり全部そろって検査されている**
  (`src/sidecar_import/probe_reuse.rs:142-173`)。
  `SidecarImportProbe::Current` かつ `source: Some(token)` (= `Disk`) であること、
  `result.source_validation_error.is_none()`、`!sidecar.is_dirty()`、
  要求 family がすべて `AlreadySynchronized` / 非要求が `NotRequested`
  (`family_synchronized`, :175-181)、`sidecar.folder() == folder`、
  予算内 (`estimate_proof_heap`) — 1 つでも欠ければ `None`。
  さらに `clone_clean_import_snapshot` が `dirty || disabled` を二重に拒否する
  (`src/sidecar.rs:688-700`)。
  `SidecarImportProbe::Current` は terminal な `Failed` family も取り得る
  (`terminal_probe`, `src/sidecar_import.rs:689-707`) が、上記の family 条件で
  除外される。計画 §3.1 の「Current だけを証明にしない」は守られている。
- **flush / strict writer fence 成功後にだけ probe が走る。**
  worker は `batch.run_on_worker(SIDECAR_WRITER_IDLE_TIMEOUT)` が `Ok` の場合だけ
  `probe_for_restore` を呼ぶ (`src/app/sidecar_restore.rs:906-916`)。順序は従来と同じ。
- **再利用時に pending / failed writer を disk より先に調べている。**
  `try_current` の `revalidate_import_source`
  (`src/sidecar_import/probe_reuse.rs:65`) は
  `revalidate_import_source_from` の `Disk` 枝で
  `pending_import_snapshot` → `is_failed_for_import` → `token.revalidate(folder)`
  の順に進む (`src/sidecar.rs:367-375`)。`probe_reuse.rs:63-64` のコメントの主張は
  実装と一致していた。
- **token 比較は metadata だけに落ちていない。** `SidecarDiskToken::revalidate`
  は metadata(前) → 全 bytes read → metadata(後) → SHA-256 を毎回行い、
  `before != after || after != expected || len 不一致 || digest 不一致` のいずれでも
  失敗する (`src/sidecar.rs:213-239`)。同サイズ・同 mtime の外部書き換えは検出される
  (回帰テストあり: `src/sidecar_import.rs:2815-2849`)。
- **marker は毎回読み直している**し、`AlreadySynchronized` を返すのは
  `marker == self.source.sync_marker()` が成立した場合だけ。行が無い (`None`) 場合も
  fallback する (`src/sidecar_import/probe_reuse.rs:68-80`)。
- **revalidate は marker 読み取りの前後で 2 回**行われ、strict `probe` の
  linearization point (marker 読み → revalidate) を保ったうえで、さらに前段の
  revalidate が加わっている。**strict probe より緩い箇所は無い。**
- **cancel を 3 点 (入口 / marker 後 / 2 回目 revalidate 後) で確認**
  (`probe_reuse.rs:58,81,86`)。cancel 時は証明を使わず strict `probe` へ落ち、
  `Cancelled` になる (テストあり: `src/sidecar_import.rs:2898-2912`)。
- **fallback に silent な「同期済み」分岐が無い。** `try_current` が `None` を
  返した場合は必ず従来の `probe` を実行し、typed 結果 (`ImportRequired` /
  `MarkerClearRequired` / `SourceChanged` / `Failed` / `Cancelled`) をそのまま返す
  (`probe_reuse.rs:112-134`)。marker 読み取り error も `.ok()?` で握り潰すのではなく、
  strict 側で同じ error が `SidecarProbeFamilyOutcome::Failed` として再導出される
  (テストが明示的に固定している: `src/sidecar_import.rs:2874-2890`)。
- **再利用結果の parse 同一性の根拠が成立している。** `prepare`
  (`src/sidecar_import.rs:249-384`) の依存は `sidecar.items` と `folder` だけで、
  `validate_and_reconstruct_key` / `prepare_edit_row` / `prepare_mask` /
  `tags_db::prepare_sidecar_tag_item` (`src/tags_db.rs:1087-1109`) はいずれも
  純関数。DB もファイルシステムも参照しない。したがって byte 同一 → prepare 結果同一。
  唯一の非決定性 (`load_for_import` の `legacy_decode_count` 差分による
  `mark_dirty`、`src/sidecar.rs:598-600`) は `is_dirty` gate で安全側に除外される。
- **中央 DB 側の変更と証明の失効は結び付いている。** 判定の実体は
  「marker 行の値 == 現 disk bytes の `sync_marker()`」であり、これは strict probe の
  判定式 (`probe_prepared_family`, `src/sidecar_import.rs:~832-850`) と同一。
  - mIV 内の補正・タグ編集 → owner が dirty → 次回 `Checking` の strict flush が
    サイドカーを書く → bytes 変化 → token 不一致 → miss。
  - メタデータ転送は `sidecar_sync` / `tag_sidecar_sync` を直接更新する
    (`src/metadata_transfer.rs:4329-4342`) → marker 不一致 → miss。
  - rename migration / marker clear も同 2 表を触る → miss または `None` → miss。
  - **「サイドカー bytes が同じなのに中央 DB だけが変わり、しかも marker が
    据え置かれる」ケースでは fast path も strict probe も等しく
    `AlreadySynchronized` を返す。**これは既存設計の性質であり、本差分が
    新たに導入した穴ではない (例: adjustment.db をバックアップから巻き戻した場合)。
    退行ではないので指摘にはしないが、判断材料として記録する。
- **UI スレッドで filesystem / SQLite / SHA-256 / JSON / BTreeMap deep clone を
  していない。** read・hash・marker 読み・`detach_outer_items_for_writable_owner`・
  `estimate_proof_heap` はすべて `sidecar-restore-recheck` worker 上
  (`src/app/sidecar_restore.rs:904-925`)。UI 側は最大 4 件の
  `normalize_path` 比較と Arc の refcount 操作だけ。
- **巨大 Arc の最終 drop を UI スレッドでやらない配慮がある。** 不採用・退役した
  証明は `retire_sidecar_probe_proofs` → 既存の `retire_smart_folder_payloads`
  drop worker へ回している (`src/app/sidecar_restore.rs:503-512`)。
  空 iterator では worker を spawn しない (`src/app/smart_folder.rs:5791-5794` の
  early return) ので、`None` を渡す多数の呼び出しでスレッドは増えない。
  worker spawn 失敗時に `prior` が UI スレッドで drop されるが、cache 側が
  もう 1 本 strong ref を持つので実体解放は起きない。
- **Arc 共有に起因する panic / aliasing の危険は無い。**
  `SidecarFile::items` と `LocalAdjustmentLayers` (= `Arc<Vec<..>>`) に対する
  `Arc::get_mut` / `try_unwrap` / `into_inner` は src 全体で使われていない
  (唯一の `strong_count` 参照は `#[cfg(test)]` helper、`src/sidecar.rs:709`)。
  `items_mut` は `Arc::make_mut` なので copy-on-write で安全。
  さらに `detach_outer_items_for_writable_owner` (`src/sidecar.rs:704-706`) が
  worker 上で外側 map を単独所有化するため、UI スレッド側の初回編集で
  `make_mut` の clone が発生する退行も避けられている。
- **予算オーバーフロー・panic の懸念は現状の定数では成立しない。**
  `estimate_proof_heap` は全加算を `checked_add` / `checked_mul` で行い、
  飽和時は証明を拒否する (`probe_reuse.rs:186-267`)。
  `publish` の `pop_front().expect(...)` (`src/app/sidecar_restore.rs:62-64`) は
  `MAX_PROOF_BYTES (192 MiB) < SIDECAR_REUSE_TOTAL_BYTES (256 MiB)` に依存するが、
  現定数では満たされている (ただし S2-7 参照)。
  `estimated_bytes` の加減算も publish / remove_folder で 1:1 対応しており、
  `get()` は会計を触らない。underflow は読む限り起きない。
- **証明の破棄点が広く取られている。** install warning (dirty write-disabled owner)、
  `ImportRequired`、`MarkerClearRequired`、`SourceChanged`、`Cancelled`、`Failed`、
  `probe_result_has_failure`、flush 解決失敗、Checking worker error のすべてで
  `clear_sidecar_probe_reuse` が呼ばれる
  (`src/app/sidecar_restore.rs:1219,1248,1270,1287,1293,1304,1312,1319,1334,1340`)。
- **publish は「同一 request の terminal Current + owner install 成功 + 非 cancel +
  `ContinuationOwner::Live`」でのみ行われる** (`src/app/sidecar_restore.rs:1266-1285`)。
  `App.sidecar_restore` は App-global に 1 つなので request 相関は構造的に保たれ、
  別 context への completion 転送は発生しない。
- **`SidecarRestoreState` の phase を新しい bool / sentinel で分割していない。**
  証明は `App.sidecar_restore` とは別の `App.sidecar_probe_reuse`
  (`src/app.rs:14160`) が所有し、`Phase` enum は無変更。計画 §3.1 の所有境界は守られている。
- **fast path が strict probe より必ず安いことはコード上で確認できる。**
  strict = read+hash (load_for_import) + JSON decode + `prepare` (全 entry の
  key 検証・mask decode/検証・local-adjust の JSON **再シリアライズ**) + marker×2 +
  revalidate (read+hash)。
  reuse = revalidate (read+hash) + marker×2 + revalidate (read+hash) + 外側 map clone。
  **reuse は strict に含まれない重い処理を 1 つも追加していない**
  (read+hash が 1 回増えるだけ)。しかも `prepare` が作る `edit_rows` / `tag_items` は
  `Current` の場合ひとつも使われずに捨てられ、`ImportRequired` の場合も
  commit worker が `load_for_import` + `prepare` をやり直す
  (`src/app/sidecar_restore.rs:958-970`)。計画 §3.2 の性能上の停止条件
  (「Missing や SQLite marker open が支配的なら短縮は乏しい」) は、
  実装者の隔離ベンチ記録 (prepare 中央値 90.178 ms / marker 2 本 1.318 ms /
  revalidate 0.300 ms) では満たされていない = 続行してよい側、と読める。
- `docs/README.md` の索引に計画が追加されている (`docs/README.md:46`)。

---

## 2. 指摘

### S2-1 [P2] 常時 ~148 MiB の parse 保持が、既存の「ナビゲーション跨ぎで捨てる」方針と衝突する

**根拠**
- `src/app/sidecar_restore.rs:12-13` — `SIDECAR_REUSE_MAX_FOLDERS = 4`,
  `SIDECAR_REUSE_TOTAL_BYTES = 256 * 1024 * 1024`
- `src/sidecar_import/probe_reuse.rs:20` — `MAX_PROOF_BYTES = 192 MiB`
- `src/sidecar.rs:1084-1088` — `evict_verified_clean` 時の `sidecars.remove(&folder)`
- `src/app.rs:27344-27356` — 「メモリ上の表現は破棄して再読み込みに任せる
  (長時間稼働時のメモリリーク防止)」というコメント付きの `self.sidecars.clear()`
- 実装者の計測記録 (`target/sidecar-confirmation-reuse-20260921/measurement.md`):
  実データ `D:\home\scan\comic\mimageviewer.dat` (666 KB) の推定保持量 **146.8 MiB**。
  内訳は 9 枚の local-adjust raster alpha の f32 で約 148,307,996 bytes。

**失敗シナリオ (推定)**

既存の復旧機構は、フォルダを離れるときに clean owner を `sidecars` から
明示的に evict する。本差分の証明はその evict の**後も**同じ内側 Arc
(`LocalAdjustmentLayers`) を保持し続けるため、これまで解放されていた約 148 MB の
f32 alpha が常駐に変わる。4 フォルダ / 合計 256 MiB まで積める。

- 解放点が無い。時間経過での失効も、`sidecars.clear()` との連動も、
  メモリ逼迫時の破棄も無い。LRU から押し出されるか terminal 事象が起きるまで残る。
- 設定で無効化できない。`sidecar_backup_enabled` / `tag_sidecar_backup_enabled` を
  両方 OFF にしても、`families` が両方 `false` になるだけで
  `family_synchronized(false, NotRequested)` が真になり、**証明は作られ続ける**
  (`probe_reuse.rs:158-159,175-181`)。
- 4K 画像 + AI アップスケール + GPU テクスチャを同時に抱える構成では、
  +256 MiB は無視できる量ではない。開発機以外での影響は未確認。

なお計画 §3.1 はこの 256 MiB が「process 全体の絶対上限ではない」と自ら明記して
いるが、**既存コードが意図して捨てている物を復活させる**という衝突までは
書かれていない。

**修正方向**
1. (推奨) S2-12 の証明書方式に切り替え、parse を保持しない。
2. それが無理なら、`flush_all_sidecars` + `sidecars.clear()` の経路
   (`src/app.rs:27353-27356`) とトレイ常駐化・最小化時に
   `sidecar_probe_reuse` も明示的に破棄する。
3. `families` が両方 `false` のときは証明を作らない (現状は作ってしまう)。
4. 少なくとも計画に「実データ 1 フォルダで約 148 MB が常駐に変わる」と数値で
   書き、ユーザーの合意を取る。CLAUDE.md の「Deterministic over Adaptive」方針
   から実行時の空きメモリで挙動を変えるのは避けるべきなので、
   **固定の小さい上限 + 明示的な解放点**という形にする。

---

### S2-2 [P2] perf 計装が無く、fast path が効いたかを出荷後に判定できない

**根拠**
- `src/app/sidecar_restore.rs:1252-1258` — hit 時に `crate::logger::log` を 1 行
  出すだけ。差分全体に `crate::perf::event` の追加が **1 つも無い**
  (`git diff HEAD -- src/app/sidecar_restore.rs src/sidecar_import.rs src/sidecar.rs`
  を `perf::` で grep して 0 件)。
- miss は**まったく記録されない**。`try_current` が `None` を返す理由
  (token 不一致 / marker 不一致 / marker read error / pending writer /
  failed writer / cancel / folder・family・data_dir 不一致) はどこにも出ない。
- 計画 §3.1 は「`SidecarImportProbe` の既存 outcome／elapsed を維持し、
  **fast-hit を別の計測値として記録する**」と書いており、未達。

**失敗シナリオ (推定)**

既存の `sli_sidecar_import` perf イベント (`src/app.rs` 付近) は所要 ms しか
持たないので、値が下がらなかったときに「fast path が hit していない」のか
「hit したが別の段が支配的」なのかを切り分けられない。とくに
S2-3 の綴り不一致や、`families` 変化、LRU 押し出し (S2-6) で恒常的に miss する
構成になっていても、ログ上は「以前と同じ遅さ」に見えるだけで気付けない。
CLAUDE.md の「Instrument Silent Paths First」(推測で 2 件外し、観測を先に入れた
1 件は一発で当たった、という経緯) と `docs/ui-responsiveness.md` の
「追加した同期処理の区間には perf::event を必ず差し込む」に反する。

**修正方向**

`sidecar_restore` の既存 `sli_*` 系と同じ family へ、
`reused: bool` と miss 理由の typed enum (`TokenMismatch` / `MarkerMismatch` /
`MarkerReadFailed` / `PendingWriter` / `WriterFailed` / `Cancelled` /
`NoPriorProof` / `BudgetRejected` / `KeyMismatch`) を持つ perf イベントを
1 本追加する。`try_current` は `Option` ではなく理由付きの typed 結果を返し、
worker → UI の `CheckingWorkerResult` にその理由を載せる
(`reused: bool` を置く場所は既にある)。
`scripts/analyze_perf.py` 側の集計は必須ではないが、`nav` family に載せておけば
`perf_smoke.ps1` の出力から拾える。

---

### S2-3 [P2] 再利用した `SidecarFile` が旧 path 綴りを持ち、`App.sidecars` の key が分岐し得る

**根拠**
- `src/sidecar_import/probe_reuse.rs:45-49` — `matches()` は
  `normalize_path(folder)` (= 小文字化 + `\`→`/`、`src/adjustment_db.rs`) の
  比較。**raw path は比較していない。**
- `src/sidecar_import/probe_reuse.rs:90` — 返す `SidecarFile` は
  `self.sidecar.clone_clean_import_snapshot()`。`clone_clean_import_snapshot`
  (`src/sidecar.rs:688-700`) は `folder: self.folder.clone()` で
  **証明作成時の raw path** をそのまま持ち回る。
- `src/app/sidecar_restore.rs` の `install_sidecar_restore_cache_owner` は
  `sidecar.folder().to_path_buf()` を `App.sidecars` の key にする。
  `App.sidecars` は `HashMap<PathBuf, SidecarFile>` (`src/app.rs:15448`) で
  **完全一致 key**。
- 加えて `try_current` の `revalidate_import_source(&self.sidecar, ...)` は
  `sidecar.folder()` を使う (`src/sidecar.rs:364`) ので、検証するファイル path も
  要求された path ではなく保存済み path になる。

**失敗シナリオ (推定)**

同じディレクトリに別の綴りで到達した場合
(アドレスバーへの手入力、大文字小文字の違うお気に入り、`\\?\` 前置、
末尾区切りの有無、`subst` / junction 経由など)、`matches()` は正規化後が
一致するので hit する。その結果:

- 復元された owner は **旧綴り key** で `App.sidecars` に入る。
- 表示・編集側が現在の綴りで `App.sidecars` を引くと miss し、別の owner が
  作られる。同一ファイルに対して owner が 2 つ存在し得る。
- 2 owner が別々に dirty になって flush すると、後勝ちで相手の編集を消す。
  `XMP_WRITE_LOCK` 相当の直列化はサイドカー側には無い。

strict probe 経路では `SidecarFile::new(folder.to_path_buf())` に
**要求された綴り**が入るので、この分岐は本差分で初めて生じる。
発生頻度は低いと推定するが、影響はユーザー編集の消失なので重要度を P2 とした。
CLAUDE.md の memory「One Owner Per Spelling」(1 つの意味を 2 か所に書かない /
1 つの述語を 2 つの問いに使わない) にそのまま当たる。

**修正方向**

`try_current` が返す `SidecarFile` の `folder` を**要求された `folder` に付け替える**
(`clone_clean_import_snapshot(folder)` のように引数で受ける)。
そのうえで revalidate も要求 path に対して行う。
あるいは `matches()` に raw path の完全一致も要求し、綴りが違えば miss させる
(marker 用の `folder_key` は正規化のままでよい)。
どちらでも、正規化 key と raw path のどちらが owner の identity なのかを
1 か所で決める形にする。

---

### S2-4 [P2] 計画 §4 が挙げた回帰のうち writer 競合系が丸ごと未テスト

**根拠**

追加された焦点テストが固定しているのは以下 (良い範囲を押さえている):
- `src/sidecar_import.rs:2731-2860` — 変更なし再訪の hit、marker 変更 →
  `ImportRequired`、同サイズ・同 mtime の bytes 変更 → `ImportRequired`、
  削除 → `MarkerClearRequired`、外側 items Arc の単独所有。
- `src/sidecar_import.rs:2862-2913` — family 変更 (ALL → edits のみ) で miss、
  `tags.db` 削除で miss かつ typed `Failed` 維持、cancel で miss。
- `src/app/sidecar_restore.rs:2397-2481` — owner install 成功後にだけ publish。
- `src/app/sidecar_restore.rs:2483-2567` — 5 フォルダの LRU 押し出しと予算会計。

**欠けている回帰** (計画 §4 の箇条書きと対照):

1. **pending writer** — 証明保持中に未完了の write がある状態で miss すること
   (`revalidate_import_source_from` の `pending_import_snapshot` 枝、
   `src/sidecar.rs:368-370`)。計画 §4 が明示的に挙げているのに無い。
2. **failed writer** — `is_failed_for_import` が真のときに miss し、
   `WriterFailed` → terminal `Current`(Failed) になり、証明が破棄されること
   (`src/sidecar.rs:371-373`, `src/sidecar_import.rs:614-620`)。
3. **dirty write-disabled cache owner** — `install_sidecar_restore_cache_owner` が
   warning を返す経路で証明が publish されず、かつ既存の証明が消えること
   (`src/app/sidecar_restore.rs:1268-1270`)。
4. **semantic-invalid / unsupported sidecar** — `source_validation_error` 有りで
   証明が作られないこと (`probe_reuse.rs:156`)。
5. **`is_dirty` (旧 mask の legacy decode)** で証明が作られないこと
   (`src/sidecar.rs:598-600`, `probe_reuse.rs:157`)。
6. **予算超過** — `MAX_PROOF_SOURCE_BYTES` (8 MiB) 超過と `MAX_PROOF_BYTES`
   (192 MiB) 超過で `proof_candidate` が `None` になること
   (`probe_reuse.rs:187-189,262-266`)。この 2 定数に対する直接のテストが無い。
7. **`data_dir` 違い / folder 違いで同一 bytes** — `matches()` の folder・data_dir
   要素が効いていること (token 側の `folder_key` guard と合わせて)。
8. **`Missing` → `Current`(source: None) で `proof_candidate.is_none()`**。
   既存テストは `!missing.reused` しか見ていない
   (`src/sidecar_import.rs:2852-2860`)。
9. **退役 Arc が UI スレッドで最終 drop されないこと** (現状は構造で担保して
   いるだけで、テストが無い)。
10. **別窓 / detached** での証明共有 (`ContinuationOwner::Live` 判定を含む)。

**修正方向**

少なくとも 1.〜5. は `probe_reuse` の worker 内契約テストとして足す。
1.・2. は `revalidate_import_source_from` が `&WriterState` を取る形なので、
`sidecar.rs` 側の既存テスト用 `WriterState` fixture を再利用できる可能性が高い
(`src/sidecar.rs:2460-2515` 付近に既存の PendingWriter テストがある)。
6. は定数を跨がない小さな単体テストで足りる。

---

### S2-5 [P3] miss のとき全 bytes read + SHA-256 が二重になる

`try_current` は `revalidate_import_source` → `token.revalidate` で
**必ず全 bytes を read して hash** してから不一致を判定する
(`src/sidecar.rs:218-237`)。その後 strict `probe` が `load_for_import` で
もう一度 read + hash する。計画 §3.1 は「fallback で二重読取になり得るのは
稀な変更時だけ」と書くが、**編集直後の再訪は必ず miss する** (flush で bytes が
変わるため) ので、稀とは言い切れない。8 MiB 上限の sidecar なら miss 1 回あたり
read+hash が 1 回分丸ごと増える。

修正方向: `try_current` の入口で `std::fs::metadata` の len / mtime を
token と突き合わせ、違えば read せずに `None` を返す。
一致した場合だけ既存の `revalidate_import_source` へ進む
(hash は従来どおり必ず取るので安全性は落ちない)。

### S2-6 [P3] `SIDECAR_REUSE_MAX_FOLDERS = 4` は実データでは事実上 1 で、A→B→A に効かない

実装者の計測では 1 フォルダの推定保持が 146.8 MiB
(`measurement.md`)。合計上限が 256 MiB なので、**この種のフォルダは 2 件目を
publish した時点で 1 件目が押し出される** (`src/app/sidecar_restore.rs:55-65`)。
計画 §3.1 が挙げた「A→B→A の実フォルダ再訪にも対応できるよう小数の
folder-keyed LRU」という目的は、実データでは達成されない。
報告された症状 (同一親フォルダ内の PDF 間移動) には効くので機能としては成立するが、
LRU 深さ 4 は実質的に飾りである。計画にその旨を明記するか、
証明の粒度を見直す (S2-12) のどちらかにする。

### S2-7 [P3] 2 つの上限定数の間の不変条件がコード上で保証されていない

`publish` の `pop_front().expect("proof itself fits the budget")`
(`src/app/sidecar_restore.rs:62-64`) は
`MAX_PROOF_BYTES (192 MiB) < SIDECAR_REUSE_TOTAL_BYTES (256 MiB)` に依存する。
2 つの定数は**別ファイル**にあり (`probe_reuse.rs:20` と
`sidecar_restore.rs:13`)、片方だけ動かすと空 deque に対する `pop_front` で
panic する。`const { assert!(...) }` か debug_assert、あるいは
`MAX_PROOF_BYTES` を `SIDECAR_REUSE_TOTAL_BYTES` から導出する形にする。

### S2-8 [P3] publish しない分岐でも外側 map の deep clone を払っている

`probe_for_restore` は `proof_candidate.is_some()` の時点で
`detach_writable_owner` を呼ぶ (`probe_reuse.rs:126-128`)。
その後 App 側が publish しないと決めた場合
(install warning / 非 Live / cancel / backlog 非空、
`src/app/sidecar_restore.rs:1286-1291`)、この clone は無駄になる。
425 entry の BTreeMap + mask の base64 String + tags Vec のコピーなので
無視できる量ではない (`SidecarEntry` のうち Arc で共有されるのは
`local_adjust_layers` だけ、`src/sidecar.rs:` の `SidecarEntry` 定義)。
worker 側で判断できない以上は現状やむを得ないが、
publish 条件の一部 (`smart_folder_retired_payloads.is_empty()` は
`allow_new_proof` として既に worker へ渡っている) を揃えれば減らせる。

### S2-9 [P3] warm hit 中の cancel が、まだ有効な証明を捨てる

reuse が成功した場合も `proof_candidate` には同じ Arc が入って返る
(`probe_reuse.rs:117`)。App 側で cancel / 非 Live と判定されると
`else` 枝の `clear_sidecar_probe_reuse` が走り
(`src/app/sidecar_restore.rs:1287`)、**token も marker も一致していた証明**が
キャッシュから消える。次回は cold に戻る。
自己修復するので実害は性能だけだが、Ctrl+↑↓ を連打して途中がキャンセルされる
という、まさに本機能の対象シナリオで起きやすい。
reuse (= `worker.reused == true`) の場合は破棄ではなく保持でよいはず。

### S2-10 [P3] `SidecarDiskToken` が `pub` へ昇格している

`src/sidecar.rs:187` — `pub(crate) struct` → `pub struct`。
`SidecarImportProbe` が `pub enum` で、その `Current` に
`source: Option<SidecarDiskToken>` を追加した (`src/sidecar_import.rs:70-74`)
ための private-in-public 回避と読める。
フィールドは private、メソッドは `pub(crate)` のままなので不透明型であり、
`crates/` 配下から `mimageviewer::sidecar::` を参照している箇所も無い
(grep 0 件) ため実害は無い。ただしデータ保護の中核型なので、
`source` を `pub(crate)` な別 accessor に逃がすか、
`SidecarImportProbe` 自体の公開範囲を見直す方が意図が明確になる。

### S2-11 [P3] 設計ドキュメントの同時更新が索引 1 行だけ

`docs/README.md:46` に計画の索引行が入っただけで、
`docs/async-architecture.md` / `docs/ui-responsiveness.md` /
`docs/preset-and-adjustment.md` §9 / `docs/sidecar-import-async-plan.md` §5.x は
未更新 (`git diff --stat HEAD -- docs/` で確認)。
本差分は App-global な共有キャッシュを新設し、既存の drop worker 経路へ
新しい payload 種別を流し、キャッシュ失効規約を追加している。
CLAUDE.md「コード修正時のドキュメント同時更新」の
「ワーカーを増やした、共有アトミック/チャネルを追加した、キャンセル規約を
変えたとき」に該当する。
最低限、`docs/async-architecture.md` の一覧表に
`SidecarProbeReuseCache` の所有・失効・退役経路を 1 行足す。

### S2-12 [P3 / 提案] parse を保持しない「検証証明書」方式の検討 (v4.0.x 向け)

実装者の計測 (`measurement.md`) の内訳は
**strict load 26.015 ms / prepare 93.500 ms / marker 2 本 1.289 ms /
revalidate 0.295 ms / probe 全体 124.897 ms**。
つまり**支配項は `prepare` (約 93 ms) であって JSON decode (約 26 ms) ではない**。

そして `prepare` が作る `edit_rows` / `tag_items` は、`Current` の場合は
一度も使われずに捨てられ、`ImportRequired` の場合も commit worker が
`load_for_import` + `prepare` をやり直す (`src/app/sidecar_restore.rs:958-970`)。
`probe` 内の `prepare` が果たしている役割は実質 2 つだけ:
(a) `source_validation_error` の検出と `disable_writes_for_session` の適用、
(b) family outcome の `Failed` 分類 (`probe_prepared_family`)。

したがって、**parse された `SidecarFile` を丸ごと保持する代わりに、
「この token の bytes は以前 prepare を通し、validation error が無かった」という
数百バイトの証明書だけを保持する**設計が成り立ち得る。hit 時は
`load_for_import` (26 ms) は実行し、`prepare` (93 ms) だけを飛ばす。

- 節約は 93/125 ≒ 75%。現行方式 (125 → 1.9 ms) より小さいが、
  報告された約 150 ms の待ちに対しては同程度の体感差になる可能性がある。
- **メモリ保持がゼロになる** (S2-1 が消える)。証明書が小さいので LRU を
  4 件ではなく数十件持てる (S2-6 も消える)。
- 外側 map の detach / Arc 退役 / 予算会計 / `estimate_proof_heap` の
  267 行がまるごと不要になる (S2-7 / S2-8 / S2-9 も消える)。
- 安全性の根拠は現行と同じ「byte 同一 → prepare は決定的」のまま。

注意: 「marker を先に読んで一致したら `prepare` を飛ばす」という**証明書なしの
単純な並べ替えは等価ではない**。初回訪問時に不正キーを含むサイドカーが
同期済みだった場合、`disable_writes_for_session` が一度も走らなくなるため。
証明書方式はその点を「以前 prepare を通した」という事実で担保する。

この案が成立するかは実測しないと分からない (`load_for_import` 26 ms が
体感で許容されるか)。v4.0.0 に入れる話ではなく、v4.0.x で
現行実装と並べて測るべき候補として記録する。

---

## 3. 同梱判断のまとめ

| | 同梱する | v4.0.x へ送る |
| --- | --- | --- |
| 得る物 | Ctrl+↑↓ の待ち約 150 ms → 推定十数 ms (同一フォルダ再訪時のみ) | — |
| 失う物 | — | 上記の待ち。v4.0.0 の他の指摘には影響しない |
| 残るリスク | 常駐 +最大 256 MiB (実データ 1 件で約 148 MB)、設定で切れない、解放点なし (S2-1)。出荷後に効果を測る手段なし (S2-2)。owner key 分岐 (S2-3)。writer 競合系の回帰未テスト (S2-4) | なし |

**推奨: v4.0.x へ送る。** そのうえで S2-12 の証明書方式を現行実装と並べて
実測し、どちらを採るか決める。

もし同梱を選ぶ場合の最低条件は次の 3 つ:
1. S2-2 の perf 計装 (hit / miss 理由) を入れる — 出荷後に判断するための唯一の手段。
2. S2-3 を直す — ユーザー編集の消失につながり得る唯一の指摘。
3. S2-4 の 1.〜5. (writer 競合 / 検証失敗 / dirty owner) を回帰テストで固定する。

S2-1 は同梱する場合も残る。その場合は「実データ 1 フォルダで約 148 MB が
ナビゲーション跨ぎの常駐に変わる」という数値をユーザーへ提示し、
明示の合意を取ったうえで出荷する。

---

## 4. 本レビューで確認していないこと

- 実行時の挙動 (アプリを起動していない)。約 150 ms が実際に短縮されるか、
  UI modal の wall time がどう変わるかは**未確認**。
  計画 §6 自身も「実 UI modal の wall time と体感はユーザーの検証で確認する」と
  書いており、この差分の効果はまだ誰も実機で見ていない。
- `scripts/test-full.ps1` / `build-dev.ps1` の再実行 (計画 §6 に実装者の PASS 記録あり。
  レビュアーは再実行していない)。
- 同じ作業ツリーに混在しているキー操作追加 (`src/keymap.rs` /
  `src/app/saved_group_actions.rs` / `src/ring_shortcut.rs` /
  `src/ui_dialogs/collections.rs` ほか) は対象外として読んでいない。
  `src/app.rs` / `src/app/tests.rs` / `src/app/smart_folder.rs` / `src/books.rs` の
  hunk も、`sidecar_probe_reuse` フィールド追加 (app.rs:14160, 16928) 以外は
  そちらの作業分と判断した。
