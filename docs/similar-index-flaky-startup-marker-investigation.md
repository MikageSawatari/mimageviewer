# 類似索引 startup marker の全件実行時失敗の調査

調査日: 2026-10-19。対象: `flaky-similar-index`、基点 `35703c18433d8ac85ee98abec57c2aab6e76f1fd`。
設計・検収は ClaudeCode、今回は調査のみ。製品の修正・コミット・アプリ起動は行っていない。
ビルド・テストの子プロセスへ `MSBUILDDISABLENODEREUSE=1` を渡した。

## 1. 結論と確度

CPU 負荷下で対象テストの最後の `scanned_once_matches` 失敗を再現し、
`ArrayAckWaitError::Failed("similar index load failed: base publish failed: … (0x80070005)")`
を捕捉した。走査・inventory 読み込みは終わっているが、検索配列の初期ロードが失敗し、
`worker_loop` が ack Failed → `restore_unfinished_plan` → `Degraded(ArrayPublication)` → `stop_worker`
を通るため、`mark_scanned_once` へ到達しない。

根本の所有境界は、**メモリロードの失効が、既に開始した base ファイルへの I/O を失効・直列化しない**点。
`configure` は Loading を Unloaded に戻し、後続 configure は別の loader を開始できる。
旧 loader はメモリ公開直前に epoch を調べるが、それより前に `load_or_rebuild` が
`similar.base` を置換し、item change 履歴も prune する。旧・新 loader のファイル操作は重なる。
本テストでは manager の破棄・同一 data_dir の再オープンも、旧 loader の終了を待たずに行う。

Windows の隔離 API 実験で、先行公開のファイルハンドルを開いたまま後続の
`MoveFileExW(REPLACE_EXISTING | WRITE_THROUGH)` を行うと error 5、ハンドルを閉じると成功することを確認した。
`write_base` は `BufWriter<File>` を保持したまま置換し、関数を抜けてから閉じる。
初期ロードの重なりはこの窓を実際に作れる。

同一 manager の通常の configure 変更でも到達することを、§4 の決定的 witness で確認した。
したがって、テスト fixture だけの問題ではなく製品にも到達経路がある競合と判断する。
構成を一度だけ設定して変更しない起動でも同じ競合が起きる、という証拠は今回得ていない。
当初の「Unloaded で publication worker stopped」という仮説の終端は今回観測していない。
対象テストの再起動側は scheduler state を直接設定して worker を実行し、configure を呼ばないため、
再起動側 Loading の configure による失効を前提とする説明はコードと一致しない。
旧 manager の epoch 破棄と、新 manager の load 失敗を区別する必要がある。
旧来の全件失敗ログは終端理由を含まないため、ClaudeCode が観測した過去 2 件の原因まで
同一と断定しない。今回の負荷再現 2 件については型付きエラーと最終状態を採取済み。
自然再現中の個々の OS ハンドル所有者は採取していない。競合の具体的な順序は、
実 loader と実ファイル API を使う §4.2 で固定して成立を確認した。

## 2. 観測された失敗と不変条件

ClaudeCode の既報:

- 対象: `similar_index::tests::startup_configuration_away_back_then_normal_stop_cannot_reuse_old_marker`。
- nextest の本体 lib 全件約 13,750 件で 3 回中 2 回、基点ソースの `src/similar_index.rs:11083` で失敗。
- 単独 8 回、similar_index 101 件 × 15 周・`-j 48` では再現なし。
- 指定された原本ログ:
  `target/test-lib-full/runs/20261019T080511070Z-9cc32a9e/stderr.log`。
  13,752 件中 13,751 成功・対象 1 件失敗、全体 406.967 秒、対象 0.422 秒。

本テストが要求する不変条件は、構成変更で旧印を失効させ、変更を元へ戻して Full 完了前に終了した場合、
次回起動では旧印を再利用せず実 Full を行い、**DB と検索 snapshot の公開を完了してから新しい印を立てる**こと。
最後の assert 前の `!reused`、`reconcile_job_counts_for_test() == (1, 0)`、
`full_inventory_load_count() == 1` は Full を実行した証拠で、array ack 成功の証拠ではない。
印だけを立てる、assert を緩める変更はこの契約を破る。

## 3. 負荷による自然再現

証跡ディレクトリ: `target/similar-startup-investigation/`。
負荷再現時のソース差分は `src/similar_index.rs` の診断 36 行のみ。
`MIV_SIMILAR_STARTUP_TRACE` がある場合に限る `cfg(test)` の stderr 観測で、遅延・barrier・
挙動変更は入れていない。診断は複数 mutex を順番に読むもので、全状態の原子的 snapshot ではない。
返却された `ArrayAckWaitError` と最終 progress は別途記録している。

### 3.1 対象単独

```powershell
$env:MSBUILDDISABLENODEREUSE = '1'
$env:MIV_SIMILAR_STARTUP_TRACE = '1'
cargo nextest run -p mimageviewer --lib `
  -E 'test(=similar_index::tests::startup_configuration_away_back_then_normal_stop_cannot_reuse_old_marker)' `
  --stress-count 100 -j 48 --no-fail-fast
```

`target/similar-startup-stress-initial.log`: nextest 集計 100/100 周成功、テスト部分 25.936 秒。
PowerShell のリダイレクトを使った外側の runner は exit 1 を返したため、終了コード 0 の gate 証拠には使わない。
`-j 48` は次節の複数テストに対する並列度で、対象 1 件だけの stress 周回を 48 並列にする指定ではない。

### 3.2 CPU 負荷 + モジュール stress

この PC の `Environment.ProcessorCount` は 24。
標準ライブラリだけの Python helper が 24 個の算術ループ process を並走させ、
nextest を `-j 48` で実行する。負荷は最大 180 秒、テスト子プロセスは最大 900 秒。
終了時は helper 自身が作った負荷 process だけを停止・join する。

```powershell
python target/similar-startup-investigation/stress-load.py
# 子コマンド:
# cargo nextest run -p mimageviewer --lib -E 'test(/^similar_index::/)' --stress-count 10 -j 48 --no-fail-fast
```

`cpu-load-module.log`: 101 件 × 10 周 = 1,010 実行、1,008 成功・対象 2 失敗。
失敗は 3 周目 0.916 秒と 8 周目 0.758 秒。nextest 60.915 秒、helper wall 約 70 秒、
nextest exit 100（helper は Windows の終了コードとして 100 を返し、tool runner 表示は 1）。
他の 100 テストは全周成功。自動 retry は使用していない。

抜粋（3 周目、`off_purge=true` 側の再起動）:

```text
load captured_epoch=1 current_epoch=2 discarded=true result=Ok(Some(2))
load captured_epoch=0 current_epoch=0 discarded=false result=Err("similar index load failed: base publish failed: アクセスが拒否されました。 (0x80070005)")
event=before_array_request memory=Failed(...) epoch=1 array=[running=false requested=false required=None error=None]
event=array_ack=Failed("similar index load failed: base publish failed: … (0x80070005)")
  memory=Failed(...) epoch=1 array=[running=true requested=true required=Some(StoreWatermark { … through_change_seq: 2 }) error=None]
event=restarted_worker_returned progress=Degraded { … reason: ArrayPublication("similar index load failed: base publish failed: … (0x80070005)") }
assertion failed: db.scanned_once_matches(&fingerprint).unwrap()
```

8 周目は `off_purge=false` 側、removed=1。同じ `base publish failed / 0x80070005`、
memory Failed / epoch=1、ArrayPublication で停止。Cancelled / stale / worker-stopped ではなかった。
旧 loader の結果が epoch 不一致で捨てられる時点で、DB/base の副作用は既に終わっている。

## 4. 順序を固定した検証

### 4.1 Windows のハンドル寿命

```powershell
python target/similar-startup-investigation/publication-handle-probe.py
```

`publication-handle-probe.log`:

```text
writer_held share=READ|WRITE|DELETE replace=0 error=5
writer_closed replace=1 error=0
reader_held share=READ|WRITE|DELETE replace=0 error=5
reader_closed replace=1 error=0
```

専用 `target/.../handle-probe/` だけを使用。失敗する順序と閉じた場合の対照条件を直接比較した。
単なる CPU 飽和や実際の ACL 不備がなくても、このファイルハンドルの重なりで error 5 になる。
これは製品コードを使った loader の witness と区別する。

### 4.2 同一 manager の再構成 witness

調査専用 ignored test:
`similar_index::startup_investigation::reconfigure_overlapping_loaders_fail_publication`。
`src/similar_startup_investigation.rs` と、base path が一致する場合だけ有効な
`cfg(test)` probe を使用する。通常 suite の意味を変えないため opt-in とした。

固定する順序:

1. 新規の使い捨て DB・base 未作成で、通常の `manager.configure` により旧 loader を開始。
2. 旧 loader の temp 書き込み・sync 後、base の置換直前で channel 待機。
3. 同じ manager に共通除外の変更を通常の `configure` から渡す。
   Loading → Unloaded、epoch 失効、新 loader 開始を実コードで通す。
4. 新 loader も base 未作成を観測し、temp を準備した時点で旧 loader の置換を許可。
5. 旧 loader は置換成功後も既存の writer handle を保持した場所で待機。
6. 新 loader の置換は実 API の error 5 で失敗。旧 loader を解放し、旧結果の epoch 破棄と
   新結果の MemoryState::Failed 公開をそれぞれ待つ。
7. fixture 内で watch を Ready にし、実 `worker_loop` による Full / array ack / Degraded /
   印未作成を検査する。watcher 自体や UI は起動しない。

channel の 10 秒は調査 probe のデッドロック検出上限であり、製品の timeout 変更ではない。
現在の不具合を assert する調査 witness なので成功は「修正済み」を意味しない。
最終の回帰テストにする際は新契約に合わせて期待結果を反転・再設計する。

```powershell
$env:MSBUILDDISABLENODEREUSE = '1'
$env:MIV_SIMILAR_STARTUP_TRACE = '1'
cargo test -p mimageviewer --lib `
  startup_investigation::reconfigure_overlapping_loaders_fail_publication -- --ignored --nocapture
```

`controlled-reconfigure.log`: exit 0、1 passed、テスト 0.13 秒（compile 2 分 50 秒）。
同一 scheduler において、旧 captured_epoch=1、新 captured_epoch=2 を確認。
新 load は error 5 で Failed を公開し epoch=3、旧 load は成功しても current_epoch=3 のため破棄。
その後の Full は inventory=1 / Full=1、ack Failed → Degraded、印なしを検査して成功した。
最終整形後の反復結果は §8 の台帳を参照。

## 5. コード上の所有・ライフサイクル照合

以下の行番号は変更前 `35703c184` のソースを基準とする。

| 境界 | producer / consumer と確認結果 |
| --- | --- |
| 作成 | constructor は MemoryState::Unloaded / memory_epoch=0。DB I/O はしない |
| ロード入口 | notifier configure（有効 root あり）、worker の eager load、item query fallback。start_memory_load は Unloaded のときだけ Loading にする。book dispatch gate は Loading 中の実行を待つ consumer で、新 loader を開始する入口ではない |
| 構成変更 | scheduler configure:2718 → retain_loaded_snapshot_during_run:3594。epoch を進め、Ready 以外を Unloaded にする。root / exclusion / password revision 変更で到達する |
| メモリ採用 | start_memory_load:948。load_or_rebuild のファイル置換・履歴 prune 後、:1000 付近で epoch を照合する。失効はメモリ公開だけに作用する |
| 配列更新 | array_update_loop:3712。Ready snapshot がある時だけ journal 適用・必要なら DB から base 再構築。array_update.running はこの worker 内の直列化だけで、初期 loader とは共有しない |
| compaction | start_compaction:3856 付近。別の compaction_running が同種同士だけを排他。write_compacted_base → 同じ write_base を呼び、再構築/初期 load と base の I/O owner を共有しない |
| ack | wait_for_array_ack:3662。Failed はすぐ Failed(error)、Missing は source missing、Loading は待機。今回 MemoryState::Failed の分岐で確定 |
| 成功終端 | worker_loop:3460 付近。ack 成功と current 確認の後、ScannedFull の場合に印を書く。今回ここへ未到達 |
| 取消・古い構成 | cancel / config epoch → settle_interrupted_plan。今回の再起動失敗の終端ではない。旧 loader の I/O 自体には cancel が渡っていない |
| close / shutdown | shutdown:3940 は scheduler shutdown / phase cancel / notify / book query shutdown。初期 load・array update・compaction の join は行わない。Drop はこれを呼ぶだけ |
| 再オープン fixture | 対象テストは旧 index worker を join するが、別の memory loader の終了は待たず同じ data_dir に manager を再作成する。実プロセス終了/再起動では旧 process の worker は残らないが、同一 manager の再構成による重複は別に到達できる |

`load_or_rebuild` / `rebuild_from_sqlite` / `write_compacted_base` はいずれも
`similar_search_array.rs::write_base` に集まる。原子的な 1 回の rename と、一連の I/O の所有・直列化は別物。
今回、compaction や array rebuild の組み合わせを実動再現したとは扱わない。

ack のメモリ確認と array_update 確認も別 lock である。
Ready(old) を読んだ後で updater が正常公開・停止すると、古いメモリ観測と停止状態を組み合わせて
worker-stopped を返す候補がある。ただしこれは現時点の source 上の別候補であり、今回の 2 失敗の原因ではない。
実装修正の設計時は ack の状態所有も点検対象に含める。

## 6. 製品での見え方と既存ログ

実アプリを起動した観測ではなく、通常 configure の経路と headless witness に基づく範囲を示す。
初回/キャッシュ再構築のメモリロードが終わる前に、お気に入りの類似索引対象・共通除外・
PDF password revision が変更され、再ロードが重なる状況が入口になる。
負荷は順序の窓を広げる条件で、CPU 負荷そのものやディスク障害を原因とは扱わない。

- DB の Full が終わっても、類似索引は Degraded(ArrayPublication) となり、完了印を立てず停止する。
- お気に入り編集画面は `⚠ 更新未完了`、詳細は
  `更新未完了: 検索用一覧への反映を確認できませんでした: similar index load failed: base publish failed: …`
  を表示する（`ui_dialogs/favorites_editor.rs::similar_index_progress_presentation`）。
- メモリが Failed のため類似 item query は Failed。book query は別エンジンの DB snapshot 経路を持つため、
  item query の失敗から本の検索も停止すると結論しない。通常の画像表示・他索引まで止まるとは確認していない。
- Full の再要求だけでは start_memory_load が Failed を読み直さないため、次の worker も同じ ack error に到達し得る。
  構成変更で Unloaded にする、または次回プロセス起動で作り直すこととは区別する。
  今回、回復操作の製品実装は足していない。

通常 logger が初期化済みなら、`similar reconcile terminal: … terminal=array_publication_failed`
と run/job の相関情報から「配列公開で未完了」は分かる。
しかし `load_or_rebuild` の失敗文字列は MemoryState::Failed と UI に渡すだけで、この枝では直接 log していない。
`array_update.last_error` にも入らず、`similar array update failed` が出る経路とは異なる。
既存通常ログだけでは error 5・どの loader のどの epoch かを確定できない。
旧 epoch の load 成功ログがあっても、それはメモリ採用前なので現状態の成功証拠にならない。
unit test は logger 未初期化なら通常ログ自体が捨てられるため、今回は cfg(test) stderr で観測した。

## 7. 修正方針の候補（未採択・未実装）

### 案 A: 初期ロードを構成変更から切り離し、store の一回の初期化にする

メモリ snapshot は全 store の検索配列であり、root/exclusion の構成に依存するロードではない。
同一 store で進行中の初期ロードを構成変更で捨てて再開する組み合わせをなくし、
後続要求はその初期化完了へ合流する。構成による検索結果の失効と、store/snapshot のロード所有を
混同しないようにする。`memory_epoch` の単純な削除・Loading だけの guard 追加では済ませない。
現在 epoch は item query の stale 判定にも使うため、query scope の失効を保つ設計が必要。

費用は中程度。fixture の擬似再起動では旧 loader を test-only の完了同期で収束させてから
同じ data_dir を再オープンする。shutdown を UI thread の blocking join にする変更はしない。
base 再構築と compaction には別の同型 I/O があるため、共通 base の操作所有も併せて定義する。

### 案 B: store ごとの検索配列 owner にロード・再構築・compaction・公開完了を集約する

既存 background worker の境界を整理し、単一 owner が base の読み取り/置換と snapshot 採用を管理する。
init / refresh / compaction の別々の bool と epoch による重複を減らし、同じ store のファイル操作を
worker 側で順序化する。ack もこの owner の公開済み store/seq と終端から判断する。
scope ごとの scan / watch / ActivityGate の契約は維持し、UI に I/O や待機を移さない。

費用は案 A より大きいが、初期ロードだけの排他追加では残る sibling の競合と、
別 lock の状態観測を一つの所有境界で点検できる。単なる global mutex の追加や
すべての非同期 interleaving の個別救済ではなく、そもそも同時実行の組み合わせを減らす方向。
設計・段階分けと独立設計レビューは ClaudeCode 側で決定する。

どちらも通常の閲覧・索引更新を機能制限しない。初期ロードのためにアプリ全体を modal にする案は、
background 索引の既存用途と UI responsiveness を変えるため、この調査では採用しない。
sleep / retry / timeout 延長 / assert 緩和 / 印だけの先行書き込みは候補に含めない。
まれな実 I/O 障害は既定どおりログ＋通知で割り切り、今回の通常操作の競合と区別する。
fixture の完了同期だけを変える案は製品で到達する競合を解消しないため、単独の根治案にはしない。

## 8. 一時コード、検証台帳、引き継ぎ

コミット候補はこの調査文書と `docs/README.md` の索引追加。以下は調査用のまま残す別扱いの差分:

- `src/similar_index.rs`: cfg(test) の stderr、load 採用/破棄 probe、ack error の型付き観測、
  対象テストの worker 終了後診断、調査 test module 宣言。
- `src/similar_search_array.rs`: exact base path に限定した cfg(test) の置換前後 probe。
  元の置換式を局所 result に置いた以外、製品ビルドの動作は変えていない。
- `src/similar_startup_investigation.rs`: opt-in の決定的 witness。製品修正・恒久 regression としては未承認。
- `target/similar-startup-investigation/`: helper・ログ。ignored でコミット対象外。

最終ソースで `python target/similar-startup-investigation/verify.py` を実行（全段階 exit 0）。
base 公開への調査 probe の影響確認として、既存 similar_search_array module も別途実行した。

| 検証 | 結果 | 証跡（同 target ディレクトリ） |
| --- | --- | --- |
| `cargo fmt --check` | exit 0、14.535 秒 | `fmt-final.log` |
| `cargo nextest run -p mimageviewer --lib -E 'test(/^similar_index::/)' --no-fail-fast` | 101 passed、テスト 1.906 秒、compile 込み 63.532 秒。調査 witness は ignored | `module-final.log` |
| `cargo nextest run -p mimageviewer --lib -E 'test(/^similar_search_array::/)' --no-fail-fast` | 13 passed、テスト 0.256 秒、exit 0 | `search-array-final.log` |
| witness の exact filter、`--run-ignored only --stress-count 10 --no-fail-fast --success-output immediate` | 10/10 周で競合を観測、テスト 2.462 秒、全体 6.219 秒、exit 0 | `controlled-final.log` |
| `git diff --check` | exit 0 | `diff-check.log` |

最終 3 source の SHA-256:

```text
95FEE47533B37AFD46DCDF013923BD13835E569C1A9CA30EDE3483736C32A8A8  src/similar_index.rs
DDE1F549F2E765D1481885143779228B008ED81612BC6C1ED82A43C806C13599  src/similar_search_array.rs
93C737AC77072F485DA40BA028D5E82DA85E64996DFD4DF9E64BFDE6CEF05A8B  src/similar_startup_investigation.rs
C2A54ABBA9C3D437A3FAF1DDC8F43C94455B991EF0D2F5F4F36BCAEE9709E908  cpu-load-module.log
C62BEBFAE8AF2BCF883BEB2CB618A4E56002CEAC8659B0E97D29B410CB333D30  controlled-reconfigure.log
```

アプリの実行時挙動を修正していないので dev verification binary は作成しない。
全件・workspace/vendor gate も修正完了の証拠としては実行していない。今回の全件失敗の調査に必要な
負荷 stress と headless witness を優先した。
