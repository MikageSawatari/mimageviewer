# 類似索引の増分照合と収束

## 範囲と状態（2026-09-11）

第一段階の実装・独立差分レビュー・関連狭域検証は完了し、`98dc41f4b` に保存した。第二段階の DB 読み取り削減も実装・独立差分レビュー・狭域 11 件の検証を終えた。測定用 r4r1 の release build と合成 DB 4,627,166 行・キー長 80/120 UTF-8 bytes の AB/BA は成功した。新方式の sampled peak Private Bytes は約 921/1,137 MiB。親は旧方式の peak より小さいことと「1 GB くらい」という目安を踏まえ、実アプリ全体の未測定という限界を残して現 inventory 表現の採用を承認した。追加 memory redesign/benchmark は行わず、進捗 UI snapshot・全体 gate・確認 build を進める。機能の再有効化や master 統合、リリース完了ではない。
作業は `codex/similar-index-incremental-reconcile`、基点は `77a6f270e`。
master の休止版 `3f5481c14` は取り込まない。統合・再有効化・リリースは別判断とし、後の統合では `Option<SimilarIndexManager>` と製品 Paused capability を保持する。

設計・進行は親、実装・テストは implement_resume、独立レビューは review_resume。
実装とレビューは別の Sol / xhigh 担当。ソース・テストは実装担当だけが編集し、本書と README は親が編集する。
旧資料の ClaudeCode 担当指定に代えて今回の利用者指定を適用する。detached 経路の変更は予定しない。

## 根因と既存記録

`SimilarIndexNotifier::request_reconcile` が通知の path/kind を持たず revision を進め、worker が pass 終了時の revision 差で全 root の `run_index_job` を再実行する。継続通知があると全件照合を反復する。既存 async-architecture の revision による再照合設計を改める。

引継ぎ値は約 464 万署名、DB 約 2.07 GB、約 5000 件/秒、small read 約 18000 回/秒。今回の再測定ではない。本番 DB を開く・走査することで再現しない。
configure が watcher 登録より先に走る欠落窓、登録失敗のログのみ処理、DB 更新後の検索 snapshot 公開前に Complete を出す境界も修正対象。

## 採用する契約

1. 単一 coordinator が desired config、watch health、job phase、pending work、array ack を型で所有する。これらは直交する状態であり、一つの排他的 phase に情報を落とさない。
2. Full 開始時に lock 下で config/root/watch epoch と event watermark を固定する。成功かつ prune-safe な Full だけが baseline 以前の dirty を吸収する。開始後のイベントは Delta へ残す。失敗・cancel・不完全観測では dirty を失わない。
3. 通常通知は Full を cancel/restart しない。file は親 directory、directory は parent と subtree、削除は parent と removed prefix を再照合する。祖先 scope の包含で重複をまとめ、現 filesystem を正とする。book/loose 分類・ページ順・表現の退役と scope prune を原子的に確定する。
4. watcher は各 root の Ready/Unavailable が確定してから initial Full。Ready は登録直後、metadata 初期走査前に通知する。失敗しても初期照合は可能だが Degraded とし、有限 backoff で登録を再試行する。復旧 gap を覆う Full 後にのみ監視の整合を回復したと扱う。root 削除・shutdown は retry も退役させる。
5. root 変更と purge は一つの Reconfigure 計画で所有する。purge、必要な Full、Delta の優先順とし、最終 keep-root union で重複 root を保護する。古い watcher/job は epoch で拒否する。overflow は gap ごとに coalesce し cancel storm を起こさない。
6. DB の store identity と through change sequence を検索 snapshot の同 store/applied sequence が満たして公開されるまで caught-up/Complete としない。CAS 競合後の新しい snapshot は ack 可能。refresh 失敗・履歴欠落・store 交換は完了扱いにしない。UI/mutex 内で待たない。
7. ZIP/PDF は本単位の staging/Complete 原子性と正規ページ順を維持する。decode/password/corrupt の失敗時は旧 Complete を保護する。未列挙の旧ページを終端 prune で消さない。削除 journal は同一 transaction。
8. password 変更も必要な config epoch に含める。ActivityGate の既存所有・負荷制御を維持し、単なる稼働状態変化で Full を再投入しない。
9. 進捗は発見済み件数と未知の残量を区別する。delta の局所失敗で全 corpus の結果を上書きしない。共有 watcher の変更は metadata/name の動作も維持する。

## 段階と検証

第一段階は上記 scheduler/増分処理/公開境界を一貫して実装し、狭域回帰と独立差分レビューを行う。
Full 中の通知は `[Full, Delta]`、Delta 中の通知は次 Delta、overflow/root 変更だけが必要な repair Full を発生させることを決定的な barrier テストで確認する。
baseline 前後・prune 中通知、watch 登録失敗/復旧、remove/readd/重複 root、旧 epoch、array CAS/store 交換/shutdown、rename、book↔loose、ZIP/PDF、I/O 不完全時の保護を含める。実 IndexerManager→supervisor→notifier 境界の回帰も必要。

第二段階は Full 自体の DB 点 SELECT を compact inventory に置換する。scan 前の有限 read transaction で metadata のみを取得して閉じ、exact String key map と record/seen bitset を使う。全 SearchRow や署名は保持しない。既存 seen clone と prune 全 key 再読込を置換し、追加常駐にしない。snapshot 後の新規行や container→loose 所有変更を誤削除しない。全件 read を常に短時間とは約束せず、取消を確認しロード時間・mutex 占有時間を測る。
約 464 万件で key 長 48/80/120 UTF-8 bytes の inventory の初期設計概算は 646/788/965 MiB、検索 array 等は別。これは実測ではなく、最新の capacity 段階増加を含む推計と実測で更新する。synthetic DB で点 SELECT と compact 方式の同値・時間・SQL 数・peak memory を比較し、容量が過大なら exact collision 検証を持つ arena 方式を評価する。

master の Cargo 待機は 2026-09-11 完了通知で解除済み。jobs=1、狭域から実行記録を残す。最終共有変更 gate とユーザー向け build は未実施。本番 APPDATA/DB、実行中アプリ、master target は操作しない。実機起動は別途承認された検証枠のみ。

## 第一段階の検証記録（途中）

`target/similar-index-incremental-phase1-review-20260911/logs/` の保存ログと各 exit 記録を親が確認した。

| フィルター | 結果 |
| --- | --- |
| incremental_reconcile（初回） | 14 passed / 0 failed |
| incremental_reconcile（追加回帰後） | 16 passed / 0 failed |
| search_watcher | 7 passed / 0 failed |
| similar_db | 32 passed / 0 failed / 1 ignored |

各 exit code は 0。これは第一段階の途中差分に対する狭域結果であり、未実装の compact inventory や最終リリース差分を保証しない。独立実装レビュー、必要な追加回帰、全体 gate、検証 build は未完了。
追加回帰の証跡は同 logs の `incremental-reconcile-final.log` と `.exit.txt`。その後、通知の FS metadata 分類が scheduler mutex 内にあることを親が指摘し、lock 外への移動を依頼した。この後続差分は上記成功記録の対象外。master の CI check 用に Cargo 枠を返却し、次の Cargo は返却通知後とする。

## 後の master 統合条件

master の `docs/similar-feature-pause-v3.8.0.md` を読み取り照合した。休止版の実行時正本は App の manager Option 単独であり、別の runtime bool を加えない。
今回追加する configure/password 更新/watch bootstrap 入口は、統合時も manager 不在なら実行しない。共有 watcher の metadata/name 処理と類似専用処理を区別し、Paused で similar 専用 supervisor を生成しない effective resolver を維持する。
query/prefill/purge と DB 登録を休止したまま統合できることを確認し、再有効化の承認とコード統合を混同しない。保存済み true、DB、配列ファイルを削除・書換えない既存休止回帰を維持する。
Enabled 側の新規回帰は capability 注入の既存手法へ合わせる。UI の類似表示を復帰させる判断は、性能改善の検証後に別途行う。

## 第一段階の独立差分レビュー

`freeze-current` の patch SHA256 は `7d46db2be5129cbaafdc20d1524f6be795ede9d067bc010b0c4d9c65540e340a`。親が現在の 8 source と保存 hash の一致を確認した。
独立 Sol レビューで以下を検出し、全件修正を承認した。第一段階は未承認であり、下記修正後の差分を再検収する。

| 優先度 | 問題と必要な契約 |
| --- | --- |
| P1 | ancestor scope が child を吸収するとき最新 event sequence を max で引き継ぐ。Full baseline 後の変更を消さない |
| P1 | watch health 変更で別 root の新しい pending Full/gap を古い要求で上書きしない。共通の単調 merge を使う |
| P1 | conditional purge の Skipped と Committed を区別し、未実行の削除義務を新 config epoch へ引き継ぐ |
| P1 | 同 root の supervisor 再生成でも停止から再登録までの gap と repair を所有する |
| P1 | watcher 復旧時に metadata の欠落も修復する。overflow/manual full の類似通知を長い metadata 処理の後まで遅らせない |
| P1 | path 分類 Unknown を parent の非再帰観測だけで済ませない。配下再観測を残し、削除は断定しない |
| P2 | watch Pending 中の purge だけの完了を全体 Complete と表示しない |
| P2 | dirty scope の正規化・包含集約の lock 内計算量と容量を制限する。上限時も通知を失わず、通常通知で全 root Full を反復する構造へ戻さない |
| P2 | watcher 終了の検出を最大 60 秒待ちにせず、bounded lifecycle 通知または適切な deadline で検知する |

array store/sequence ack、Delta 原子的 publish/prune、Full conditional finalize、typed degraded reason にはこのレビューで追加 P1/P2 はなかった。これは上表を含む完成承認ではない。
旧 freeze は保存し、修正は新しい r2 証跡へ記録する。master に Cargo 枠を返したため、修正中の新 Cargo は返却通知まで待つ。

### r2 の途中結果

master の CI check 完了通知後、Cargo 枠が返却され狭域確認を再開した。
`target/similar-index-incremental-phase1-review-r2-20260911/logs/incremental-reconcile.log` は網羅性エラー 2 件によるコンパイル失敗で、テスト実行失敗ではない。
修正後の `incremental-reconcile-r1.log` では 25 passed / 0 failed を親が確認した。関連 module、最終 source 固定、指摘対応の独立再レビューは引き続き未完了。
その後、AwaitingArray 中の config/watch gap 取消が一般失敗として停止し、pending repair を起動しない追加 P1 を実装担当が確認した。Cancelled と Failed を型で区別し、shutdown 以外の取消は義務を復元して次 job を選ぶ修正を採用した。25 件の成功はこの後続修正を含まない。master の sidecar 段階検証へ Cargo 枠を再度返却し、その間は source/回帰修正とレビュー準備を進める。

### 監視終了の検知時間の範囲

P2-3 の health tick は event-loop 待機中の約 1 秒間隔を保証範囲とする。共有 supervisor は metadata initial/full/recovery scan を同期実行する既存構造のため、その間の watcher 終了は scan 完了後に検知して metadata と類似の両方を修復する。全期間 1 秒の検知は約束しない。
overflow/manual 要求の類似 gap は metadata scan 前に記録し、scan 後の停止検知・復旧でも repair 義務を失わないことは必須。全期間の短時間検知が別途必要なら、health 所有を metadata scan から分離する変更が必要になる。親と独立レビュー担当がこの範囲を確認した。

### r2 最終狭域検証と追加レビュー

master から枠返却後、r2 固定差分（patch SHA `E334AA65C0FF82F875E1DD25108CBA078A1A2EE6FEBFD804989D95E6646BECDA`）を検証した。
`target/similar-index-incremental-phase1-review-r2-20260911/validation-final.txt` に、増分 29、watcher 7、DB 33（1 ignored）、supervisor 11、manager 7 の成功と core check 成功を記録。親が manifest を確認した。
独立再レビューでは以下 4 境界が未解決であり、r2 は完成承認ではない。r3 で一括修正する。

- Unknown→Subtree の再観測時に NotFound となった場合、RemovedPrefix として配下の旧索引を削除する。実 DB 回帰を追加。
- 既存 Subtree(root) が容量対策の RootRepair を吸収しないよう優先順位を定義し、多数の RemovedPrefix に対する容量上限を確認。
- AwaitingArray 取消の回帰を enum 判定だけでなく、未完 job 復元から queued Full 再選択まで通す。
- active worker 中の watch replacement 取消後は、Pending barrier に応じ AwaitingWatch を確定する。取消状態の表示だけで止めない。

r1/r2 証跡は保持し、r3 の成功証拠に流用しない。未変更モジュールの再検証は必要性に応じて判断する。

### r3 狭域検証

`target/similar-index-incremental-phase1-review-r3-20260911/logs/incremental-reconcile.log` と `incremental-reconcile.exit.txt` を親が確認し、30 passed / 0 failed / exit 0。今回の 4 境界を修正した差分に対する結果。固定差分の独立再レビューはまだ未完了。

その後、独立 r3 差分レビューは blocking P1/P2 なしで承認された。続けて既存 `similar_index::tests` 全体を確認し、66 passed / 1 failed / 5 ignored。
失敗 `index_worker_open_also_starts_memory_load_without_a_panel_query` は、scheduler configure だけで watcher を Pending のままにした旧 fixture が eager worker 開始を期待していた。親も source を照合し、begin_watch→watch_ready を通す test-only 修正を承認した。製品差分の承認は維持し、修正後の exact/module/core/glyph 検証を行う。

最終 UI 検証ではお気に入り索引の進捗・AwaitingWatch・Degraded を実描画 helper で確認する小 snapshot を追加する。既存 `tests/ui_snapshot.rs` は類似結果/状態パネルの coverage はあるが、お気に入り索引進捗は未収録。これは最終 handoff 前の未完了事項として扱い、第一段階の scheduler 修正と再レビューを広げない。

### 第一段階の検証確定

旧 fixture 修正後、exact 1、`similar_index::tests` 67 passed / 0 failed / 5 ignored、core check exit 0、glyph 0、owned-source fmt/diff check 0 を確認した。製品差分は独立 r3 承認後に変更していない。
最終証跡は `target/similar-index-incremental-phase1-review-r3-20260911/fixture-validation/MANIFEST.txt`、SHA256 `7BA2EB9E97DF6F83B19B5C7660D66EBC28C3AEBC0861ED36EAB585EE67AB7207`。
親も module/glyph のログ、manifest hash、現在の source 8 件と最終 hash の一致（不一致 0）を確認した。
関連 watcher/DB/supervisor/manager の結果は r2 記録を参照。後続 r3 は similar_index の取消・scope 修正と回帰、最終差分は旧 fixture のみ。全体 gate は Phase 2 と最終 UI 検証後に実施する。

## Phase 2 実装前の採用条件

DB 調査担当の接続案 `target/similar-index-incremental-phase2-design-20260911.md` を独立レビューし、以下の補正を採用した。第一段階の検証区切り後に実装する。

- Full だけが job-local の Clone 不可 inventory を所有する。Delta は全件 inventory をロードせず既存局所 publish を維持する。
- exact String map、compact item/container metadata、ceil(N/64) word の AtomicU64 bitset を使う。Phase 1 の seen-container 保護により CSR member 列は不要。
- inventory は scan 前の有限 read transaction で得て閉じる。4096 行程度ごとの取消を確認し、ロード時間と DB mutex 占有時間を測る。「常に短時間」とは約束しない。scoped worker が共有参照し、全 join 後に by-value finalizer へ渡す。
- `current_and_published` は current hash かつ loose または snapshot container が Complete。orphan/Failed/Building は false。owner/page/mtime/size と container freshness の判定は旧実装と同値にする。旧実装が Stale とする不正な raw state/page count を、loader 全体失敗へ強化しない。
- container の member_count は report 用であり、新たな freshness 条件を追加しない。ZIP/PDF canonical 列挙は維持する。
- snapshot 後の新 member を bulk `DELETE WHERE container_key` で消さない。初期 item_id と初期 owner を候補とし、live の両方が一致する場合だけ削除と journal を同一 transaction に書く。container 行も snapshot identity/generation と整合し surviving live member がない場合だけ削除する。
- new member、old member→loose、orphan→loose、loose→新 container、同 key container 更新、失敗 container 旧 Complete 保護を小 DB 回帰で確認する。
- u32 index/member_count は checked overflow。summary の capacity hint は破損巨大値を無条件 reserve せず上限と fallible reserve を持つ。finalizer で全 key String clone/Vec を再生成しない。
- `ScanPass` の共有 descriptor と per-work Delta seen owner は分ける。loader の取消は scheduler cancel/stale として扱い、FilesystemIncomplete へ混同しない。commit→watermark→array ack は Phase 1 のまま。
- メモリ計測は map/record/bitset のほか、visited_dirs、work queue、SQL 一時 String、resize peak、検索 array を分けて記録する。まず標準 exact map、arena は測定で必要と判断した場合のみ。
- 小 legacy/new 同値比較後、synthetic 4,627,166 件・K80・25% loose/75% contained・99% current を代表ケースとする。実データ分布とは主張しない。K48/K120 は accounting から推計し、誤差や容量の閾値を越える場合に追加実測する。
- A/B の削除順は一致しなくてよい。最終 rows/container/summary、正規化した `(item_id, op)` の sorted multiset、単調 watermark/ack を比較し、重複 journal を集合化で隠さない。時間比較は最低 AB/BA の二順序とし、一回の実行順や OS cache の差を性能改善と断定しない。
- SQL TEMP batching 案は小 batch では主 index の random probe が残り、全件版では二相処理・temp I/O・取消 lifecycle が増えるため初版では採用しない。大規模測定は master と資源調整してから実行する。

測定の限界: synthetic A/B は DB 照合・prune の比較であり、本番の filesystem 列挙、画像 decode、ネットワーク、音声再生と競合する全体性能を直接保証しない。setup と結果照合の時間・メモリを測定本体から区別し、合成分布と測定順序を結果に添える。本番 5000 件/秒という引継ぎ値を比較対象の実測 baseline に置き換えて報告しない。

### Phase 2 第一単位の検証

Full inventory と scan/finalize の接続を 2 source に実装した。小 DB 回帰 8 passed / 0 failed、Delta loader 0 回の実経路 exact 回帰 1 passed / 0 failed、両 exit 0。
証跡は `target/similar-index-incremental-phase2-r1-20260911/MANIFEST.txt`、SHA256 `EBCE39E81806D61F099776B4EF0BDC367D2F6C2F8BEEB9325A73133256A8C7CF`。
親が stdout/exit と source 2 件の snapshot/current hash 一致を確認した。独立実装レビューへ渡して source を固定。大規模 harness はまだ未実装・未実行で、性能改善の検収は未完了。Cargo 枠は master の gate/build へ返却した。

### Phase 2 r1 独立レビュー

以下を採用し、同じ実装担当が 2 source を修正する。r1 の成功結果は後続差分の検証結果として扱わない。

- P1: Full inventory の全件 read が既存 ActivityGate の最初の確認より前に始まり、明示 pause 中でも新規作業を開始する。DB mutex/read transaction 取得前に既存の pause 契約を通す。操作中の 1/1 と開始済み作業の完走は維持し、pause 開始時 loader 0・解除後開始・pause 中取消を回帰で確認する。
- P2: 小 DB の legacy/new 同値比較に orphan、zero/extra members、旧 hash、不正 state/負 page count、stale metadata を含める。finalizer の取消だけでなく loader の定期取消も確認する。

snapshot item-id/owner による削除、container identity/generation と surviving member 保護、scoped worker join 後の inventory 消費、Delta loader 0 の実経路について、このレビューで追加指摘はなかった。大規模性能の承認は含まない。
修正中も master に貸出中の Cargo 枠は使用しない。後続 A/B harness は setup/legacy/inventory/verify の process を分離し、setup と照合の時間・peak memory を測定対象へ混ぜない。

### Phase 2 r2 静的固定

上記 P1/P2 の修正と回帰を追加し、独立差分レビューへ渡した。`target/similar-index-incremental-phase2-r2-20260911/MANIFEST.txt` の SHA256 は `61C63A87C38E6FAAA0B96139D0E7D1113D87F1093C3827CEBC7AB80D83CD0CD3`。親が manifest と現在の 2 source の hash 一致を確認した。
この時点では Cargo 未実行であり、回帰成功・コンパイル成功は未確認。master の gate/build へ貸出中の枠返却後に追加回帰を実行する。

独立 r2 差分レビューは P1/P2 解消・追加 P1/P2 なし。同じ固定差分の compile/test 成功を条件とする静的承認であり、性能承認は含まない。記録は同 artifact の `review-r2-static.md`、SHA256 `D09392BF5401C6479AB5E46126FF4F842D1DF7CB5AD16CB4BEB0EF7F2D1B07AD`。Cargo 枠返却時は `full_reconcile_inventory` filter から再開する。

### Phase 2 r2 狭域検証確定

master の portable 実機確認に合わせ、jobs=1 の `full_reconcile_inventory` filter だけを実行した。初回は追加テストの scoped Sender capture 2 箇所で E0373 となり、テスト未実行。明示参照と Sender の move だけを直した r2r1 で 11 passed / 0 failed / exit 0。旧方式との edge-case A/B、4097 行での loader 途中取消、pause/active/cancel の実経路を含む。
親も r2→r2r1 差分が test-only であること、stdout、validation 記録、現在の source 2 件の hash 一致を照合した。独立静的承認後の製品コード変更はない。
証跡は `target/similar-index-incremental-phase2-r2r1-20260911/validation/VALIDATION.txt`、SHA256 `AB9B232E50D3FEAF764B53A66674B0F6246289F149D9B5F9C44F11092A4B2786`。Cargo/rustc 終了を確認して枠を返却。大規模 harness、4.64M A/B、module 全体、全体 gate、最終 UI snapshot とビルドは引き続き未完了。

### 大規模測定の準備枠

master から harness 実装・測定設計・静的レビューまで承認された。master の portable 実機確認中は Cargo、大規模 DB 生成、benchmark を開始しない。通常 APPDATA と実 DB は使用せず、合成 DB・copy・SQLite temp/sort spill・照合成果物はこの worktree の target 内専用 run に限定する。canonical path と reparse point を検査し、既存成果物の上書きを拒否する。測定 process の TEMP/TMP も run 配下へ向ける。

実装前の比較モデル照合で、legacy は実際の load_item/container_freshness/item_keys_for_container と seen/finalize、inventory は実 loader/observe/member_count/finalizer を使う方針を確認した。snapshot 後の new row/owner 変更は保護を強化した専用回帰を正本とし、旧 prune を誤った期待値にしないよう timed A/B から除く。
代表条件は 4,627,166 rows・K80・25% loose/75% contained・256-page group・16 workers。1% の old-hash/stale/missing/new probe 配分と分母・実数を記録する。setup、legacy、inventory、verify は別 process とし、最低 AB/BA の両順序で測る。
実行枠取得後は小規模 end-to-end を先行し、必要なら 10 万行 pilot で所要時間とメモリの見積を更新する。現時点で大規模所要時間は未測定。C ドライブ空きは準備時点で約 766 GiB、削除作業は行わない。

その後 master の実機確認が環境要因で終了し、CPU/IO 枠が解放された。独立検収と測定条件・見込み時間/メモリ・指標の一括報告を先行し、各 process は有限 1 周と時間上限・取消を持たせる。大規模実行の終了期限は 2026-09-11 09:00 JST。性能結果だけでは master merge や類似機能の再有効化を行わない。

### 測定の資源・実行条件（静的検収前の計画値）

| 項目 | 条件 |
| --- | --- |
| 件数 | まず 4,096 行で 4 mode の一巡、その後 100,000 行 pilot、最後に 4,627,166 行。各件数は別 run directory |
| 実行順 | setup → ab-legacy → ab-inventory → ba-inventory → ba-legacy → verify。各 mode は別 process、同時実行しない |
| CPU | 比較処理は 16 worker と計測 sampler。コンパイルは jobs=1。CPU/I/O の占有は実行枠内だけ |
| メモリ | K80 inventory 自体は従来設計で約 0.8 GiB の概算。SQLite、runtime、allocator、legacy seen の peak は未測定であり、pilot から更新する。検索 array 約 212 MiB は今回の DB-only 比較に常駐させず別項目とする |
| ディスク | baseline と 4 trial の DB、WAL、SQLite temp、結果 JSON/log を専用 target/run 内へ保持。実データの 2.07 GB は合成 DB の実測サイズではない。pilot の bytes/row から必要容量を更新する |
| 時間 | 所要時間は未測定。初期上限案は compile 15 分、小規模各 mode 2 分、pilot 各 mode 5 分、本測定各 mode 25 分。開始時の残時間と pilot 結果により縮小・延期を判断する |
| 終了 | 全体打切りは 08:50 JST、所有子 process 回収と記録を 09:00 JST までに完了。timeout/取消/例外は当該測定 process だけ回収し、run を incomplete として後続停止。既存部分 run は再利用しない |

比較指標は load/lookup/finalize/total wall time、logical SQL call 数、process 初期/終了と sampled WS/private peak・差分、inventory map/record/bitset accounting と growth、DB/WAL/temp 開始/peak/終了 bytes。内部一時 allocation と sampler 間の瞬間 peak は完全観測とは主張しない。process lifetime peak と測定区間の sampled peak は区別する。
最終 rows/container、summary/report、sorted `(item_id, op)` multiset と watermark を照合し、結果照合の時間・メモリは測定本体から分ける。DB-only harness は scheduler/array ack の end-to-end 測定ではなく、既存回帰を参照する。実行前に独立レビューとこの条件の master への一括報告を必須とする。

### Harness r1 静的レビュー

source 3 件と manifest の hash 一致を親が確認し、測定条件を master へ報告した。master は独立検収後の 4,096→100,000 を承認。本規模は pilot の時間・容量・総 peak から再判断し、異常増大/同値不一致では進まない。
独立レビューは比較 DB/probe/fingerprint、測定本体と照合の sampler 分離、logical SQL の実測/モデル表記を確認したが、外部 runner に 2 件の P1 があり未承認。

- setup 時の immutable run identity（実行 exe SHA/rows）を create-new で固定し、全 step で照合する。先行 step の完了と実行順を検査し、途中 rebuild・順序違い・incomplete run の継続を拒否する。
- wrapper の write/Start-Process より前に、drive root から target/run/temp/exe までの全 path component の reparse を検査する。child の Rust 側検査だけでは wrapper の先行書込みを保護できない。

同じ実装担当が runner だけを修正し、再検収も対応差分に限定する。Cargo/合成 DB 生成/測定は引き続き未実行。

### Harness r2 静的承認と実行開始

runner 限定修正後、P1×2 解消・追加 P1/P2 なしで静的承認。記録は `target/similar-index-incremental-phase2-benchmark-harness-r2-20260911/review-static-approval.md`、SHA256 `EB34D9CF377D7BD8EF7840D3CBC5FA5DE71BAB75CC2657F0F7156D6FCABF4818`。製品/測定 source 3 件は r1 と同じ。
親は条件を master へ報告済みで、jobs=1 compile→negative preflight→4,096 行の 6 mode を実装担当へ開始指示した。compile/実行の成功は、この開始指示だけでは未確認。4,096 の結果を確認してから 100,000 pilot へ進む。

4,096 行は compile exit 0、negative preflight 2/2、6 mode と全同値判定が成功した。証跡は同 r2 artifact の `validation-4096/VALIDATION.txt`。親もログと verify JSON を確認。ただし最初の exe は unoptimized test profile だったため、これは harness 機能検証に限定し性能評価へ流用しない。
性能比較は source 不変で release lib-test exe を jobs=1 で構築し、fresh 4,096 一巡→100,000 pilot を実行する。exe 変更後の同値確認は必要だが、変更していない runner の negative preflight は繰り返さない。compile 上限は release 用に 30 分へ変更、08:50 絶対打切りは維持。debug/release の値を同じ A/B へ混ぜない。

release compile は 05:03:24 JST 開始後、約 26 分でも rustc 最適化中で link 未開始。master は進行中 compile を重複させないため、安全に監視を維持できる条件で開始から 60 分までの延長を承認した。担当が PID/StartTime を照合した同一所有 process の watchdog を設定し、06:03:24 JST を上限として継続した。監視結果は `validation-release-4096/compile-watchdog.txt` に保存する。時間切れは製品不具合と扱わず、同じ条件の再試行を繰り返さない。必要時の既存 dev-runtime profile による同条件 A/B は master 承認済みだが、release 実時間と区別し fresh 小規模から確認する。

### Release pilot と本規模の開始判断

release compile は 31 分 57 秒・exit 0 で完了し、watchdog も期限前正常終了を記録。最初の direct 起動 2 run は DLL 検索環境不足の `0xC0000135` でテスト開始前に終了し、incomplete として性能値から除外した。同一 exe に既存 vendor DLL と target/release の PATH を設定し、fresh 4,096→100,000 の各 6 mode は全照合に成功した。
証跡は harness r2 の `PILOT-SUMMARY.txt` と `PILOT-FILES-SHA256.txt`、個別 JSON は `validation-release-4096/run-release-4096-r2` と `validation-pilot-100000/run-100000`。exe SHA256 は `27CBD0A007381F022EB7725F1987D88D8B2C1E1C4A5F5BBF8DB862128F1262F2`、PATH SHA256 は `1408041D128D9E18C9358D665E6F3F482DACE8F9E5AE3F473690AE5836E68EBD`。
100,000 の旧方式 total は 1.24–1.50 秒、新方式 0.070–0.071 秒。総 sampled peak private は旧方式 180–196 MiB、新方式 62 MiB。parent が setup/verify JSON と集約記録を確認した。これは K80/16 workers の DB-only 予備値であり、アプリ全体の改善倍率ではない。
本規模は map capacity の段階増加を考慮し、新方式の構造 subtotal 約 866 MiB、process 等を含む保守推計約 1.7 GiB、旧方式保守推計約 10 GiB。05:44 JST の GlobalMemoryStatusEx は総 RAM 127.27 GiB/空き 80.97 GiB。DB は 1 本約 1.87 GiB、5 本約 9.35 GiB と一時 WAL 等の見込み。時間の線形参考値は setup 13 秒、旧方式各 57–70 秒、新方式各 3.3 秒、verify 14 秒で、保証値ではない。
同値成功と空き資源・残時間を根拠に、master へ更新条件を一括報告して 4,627,166 行の fresh K80 AB/BA を開始指示した。各 mode 25 分/08:50 絶対打切りを維持。K48/K120 の追加実測条件に達した場合は別途判断し、完了扱いへ省略しない。

### 本規模起動前の runner 診断

本規模最初の setup も `0xC0000135`、stdout/stderr 空でテスト開始前に停止した。DB 生成なし、incomplete run は保持して後続停止。固定 PATH の child 照合と小さな `--list` の切分けで、この環境の Start-Process における window 制御と標準 handle redirect の組合せを失敗境界として確認した。
同一環境で direct 起動、redirect のみ、Hidden のみは exit 0、Hidden+redirect と NoNewWindow+redirect は失敗。ProcessStartInfo の UseShellExecute=false/CreateNoWindow=true/redirect pipes は exit 0。初回の DLL 検索環境だけで解決したとの判断は、この診断結果により訂正する。
runner r2 を保持し、r3 で起動部だけを上記 ProcessStartInfo と非同期 stdout/stderr drain へ変更する。identity/order/timeout/取消/所有 PID 回収は維持。source/exe 不変で独立 runner 差分レビュー→fresh 4,096 一巡→本規模を行う。100,000 pilot の source/exe は変わらないため再測定しない。
master の追加条件により、測定許可を製品のメモリ採用承認とは扱わない。実測後に peak と live inventory の保持量、アプリ常駐分、harness 特有の保持を区別して設計へ戻す。

### メモリ帰属と測定補正

独立限定レビューは harness r2 の `memory-attribution-review.md`、SHA256 `2297AE723763E9BB9366469F23689F26C77311C8B29178EE7E19090156428381`。LogicalCalls/ProbeSummary/Accounting は O(1)、fingerprint と journal の照合は trial sampler 停止後。legacy finalizer の全 key Vec は実製品の費用だが、harness の worker 集合の merge は旧製品 aggregate.snapshot の全 String clone＋元 aggregate の併存を再現しない。よって値は synthetic reconcile-process peak であり、旧製品の全 peak と同一視しない。
final_sample は両方式とも strategy の drop/consume 後で、live inventory の steady は未観測だった。採用判断に必要なため、両方式の probe 後/finalizer 前の O(1) live memory sample を追加する。採用済み K48/K120 の追加実測条件へ対応できるよう key bytes を 48/80/120 指定にし、run identity に固定する。これを runner r3 の起動修正と合わせ r4 一括静的検収へ渡す。製品コードは変更しない。
新 exe で fresh 小規模と pilot の同値を確認し、残時間と資源を再評価して本規模へ進む。旧 exe の結果・incomplete 証跡は保持し、新条件の A/B と混ぜない。

r4 静的レビューの P2 は live delta の基準名（process start ではなく copy/open 後の sampler start）と、dispose 後の evidence 保存失敗時の catch guard だった。数行だけ直した r4r1 を独立承認。記録は `target/similar-index-incremental-phase2-benchmark-harness-r4r1-20260911/review-static-approval.md`、SHA256 `C5B179EFEC3340DD873096684674DF24C01B639B45C914304574764DD3004E0D`。
親は同 release profile/jobs=1、最初から 60 分/08:50 の早い上限と所有 process watchdog を付けた compile、新 exe での key identity negative preflight→fresh K80/4,096→100,000 を開始指示した。harness SHA256 は `090D0D19D6F37B5FF92E7473D7207124FFCE99C870F0E1421AB0D63E4F9EEF3A`、製品 source は不変。master は今回 r4 の独立検収と有限実行を区切りとし、時間不足時の K48/K120 未測定を許容した。K80 の同値/live/peak を優先し、追加設計拡張で compile を反復しない。

### r4r1 小規模・pilot の確定

release compile は 32 分 29 秒/exit 0、新 exe SHA256 `10C0AB3458695F3F50F6F68D57B0F1D067EDE2EB787A746833BC9DC63375E7D2`。所有 compile process は期限前終了。key identity mismatch の起動前拒否と、fresh K80/4,096・100,000 の各 6 mode は成功した。新しい非表示起動方式で loader error は発生していない。
親も r4r1 artifact `validation-k80-100000/run/verify.json` の AB/BA・反復・DB fingerprint 全 True と各 trial JSON を確認。100,000 の旧 total は 1.221–1.228 秒、新 total は 0.069–0.071 秒。probe 後の live Private Bytes は旧 178–185 MiB、新 61–63 MiB、sampled peak は旧 186–188 MiB、新約 62 MiB。これは source 不変の前 pilot と同じ規模の傾向であり、予測外の増大・同値不一致はない。
更新条件を master へ報告して fresh K80/4,627,166 の開始を指示。各 25 分/08:50 打切りを維持し、K48/K120 は K80 の結果と残時間を確認してから判断する。Private Bytes と resident WS、live/peak/post-finalize residual を分けて記録する。

### 本規模 K80/K120 の結果と今回の区切り

同一 release exe・source・runner・PATH、16 workers で fresh 合成 DB 4,627,166 行を測定。K80 と、追加条件を満たした K120 は、それぞれ setup→AB→BA→verify の全 6 mode が成功し、最終行・container・summary・journal multiset・watermark の照合が一致した。K120 は先に fresh 4,096 行でも一巡成功。証跡は r4r1 artifact の `validation-k80-full-4627166/run/` と `validation-k120-full-4627166/run/`。root も verify と各 trial JSON を確認した。

| キー長・方式 | total 秒（AB / BA） | live Private MiB（AB / BA） | sampled peak Private MiB（AB / BA） |
| --- | --- | --- | --- |
| 80 bytes・旧 | 67.557 / 69.112 | 900.64 / 771.87 | 1,235.91 / 1,295.95 |
| 80 bytes・新 | 4.607 / 4.478 | 918.22 / 918.18 | 921.37 / 921.35 |
| 120 bytes・旧 | 71.789 / 71.893 | 984.30 / 984.52 | 1,618.60 / 1,618.88 |
| 120 bytes・新 | 5.210 / 5.189 | 1,134.32 / 1,134.29 | 1,137.33 / 1,137.36 |

total は copy/open/setup/verify/fingerprint を除く synthetic reconcile core の時間。実ファイル走査・decode・scheduler/array 公開は含まず、製品全体や音声途切れの改善倍率とは扱わない。K は合成 ASCII key の UTF-8 bytes であり、本番の平均 path 長を主張しない。OS cache は強制制御していない。

live は probe workers join 後・finalizer 前の process sample。新方式 live WS は K80 約 891 MiB、K120 約 1,105 MiB。finalizer 直後の Private は約 919/1,135 MiB だが、mimalloc の保持を含み恒久的なアプリ常駐量ではない。sampled peak は 50 ms 間隔で瞬間 peak を完全観測しない。旧方式 sampled peak は K80 約 1,236–1,296 MiB、K120 約 1,619 MiBだが、旧製品の aggregate.snapshot clone と元 aggregate の併存を再現しないため旧製品全体の peak の代用にはしない。

item map capacity は 7,340,032、record capacity は 4,627,166、初期 reserve 後の item map/record growth は 0/0。container map/record growth は 4/5。key payload は K80 371,257,840 bytes、K120 556,886,760 bytes。map bucket の内部サイズは容量からのモデル値で、実 allocation の直接観測ではない。logical call は観測 API counter と deterministic finalizer model を分けており、SQLite VM steps や OS random read の実測ではない。

K80 が 900 MiB、K120 が 1 GiB の追加検討目安を超えたが、これは hard cap や採用不可の判定ではない。利用者の「1 GB くらいなら許容」を踏まえて親が判断する。約 1,137 MiB は新方式の試験 process 全体の sampled peak であり、旧方式に上乗せする追加量ではない。同じ K120 旧方式の約 1,619 MiB より約 481 MiB 小さい一方、finalizer 前の live は旧方式より約 150 MiB 大きい。検索 array（既存記録約 212 MiB）や queues/UI/decode の共通常駐分は試験外で、単純加算も概算に留まる。旧製品 clone 非再現の限界も含め、現方式を採用するか exact key の arena 化などを別段階で検討するかが次の判断となる。今回は追加実装・compile を開始しない。

K48 は推計のみで実測していない。K80 の accounting 差が小さく、短いキーの推計が追加測定目安を下回るため、今回の有限枠では省略した。snapshot 後の追加行・所有変更・container 再構築の安全性は旧 unsafe prune との速度比較から除外し、専用の既存回帰を根拠とする。全体 gate、進捗 UI snapshot、確認 build、実機確認、休止版との統合判断は引き続き未完了。master merge・再有効化・アプリ起動・本番 DB 操作は行っていない。

同じ固定 release exe で `full_reconcile_inventory` 回帰を 1 回実行し、11 passed / 0 failed / 1 ignored（手動測定 harness）/ exit 0。ログは r4r1 の `validation-release/full-reconcile-inventory-tests.stdout.log`。`FINAL-PROCESS-STATUS.txt` は所有 measurement process と cargo/rustc/link が 0、source 不変、本番データ・アプリ未操作を記録する。今回の CPU/IO 枠は親へ返却済み。集約は `FINAL-BENCHMARK-SUMMARY.txt`（SHA256 `910CD25BB4031B2CB4B4DB4ED7B5D818F02861D65DF009F7F318525876D743B7`）。

Sol / xhigh の独立限定検収も完了し、P1/P2・blocking 所見なし。`performance-validation-review.md` の SHA256 は `4685D649D3FABEB9784367E22563A1530A687E74D770E279729B93025FE14BE8`。K80 run-local 集約の倍率丸めだけ 14.67→14.66 へ訂正し、測定 JSON/DB/source/exe/runner は不変。最終 evidence manifest は `FINAL-EVIDENCE-SHA256-R1.txt`（SHA256 `C2C20338D04F0116479F832ECA35208A711E2367073AEDFA151B4CD6F0EE338B`）。

### 次の小変更：進捗表示の検証境界

親は現 inventory 表現を採用方向で受け入れた。K120 の約 1.11 GiB は旧方式への追加量ではなく、旧試験 process peak 約 1.58 GiB より小さい。probe 後 live と直後 residual は旧方式より大きく、App 全体 peak は未測定という制約は維持する。追加 memory redesign/benchmark は不要とし、検証と確認 build まで進める。焦点 commit は当該 branch のみに保存し、master merge/再有効化は引渡し後の別判断とする。

所有契約は Full job 内に限定する。`run_index_job` がローカル変数として inventory を所有し、`thread::scope` 内の workers は `ScanPass::Full(&inventory)` で借用する。scope の join 完了後だけ by-value finalizer へ移し、取消・error・prune-unsafe return では通常の Rust drop により解放する。`ScanJobOutcome` は report/prune-safe だけを返し、coordinator・progress・Delta は inventory を保持しない。allocator による Private Bytes の残留と live owner の残留を混同しない。既存の独立レビューと有効な狭域回帰を根拠とし、記録のためだけに同じ測定を再実行しない。

独立担当もこの所有契約を再実行なしで限定確認した。根拠は Clone 不可の `FullReconcileInventory`、値所有の `finalize_full_reconcile_inventory_if`、別経路の `run_delta_index_job`。有効な回帰は `full_reconcile_inventory_obeys_pause_before_loading_and_cancel`、`full_reconcile_inventory_loader_polls_cancellation_during_row_scan`、`full_reconcile_inventory_cancelled_finalize_rolls_back_everything`、`full_reconcile_inventory_matches_legacy_freshness_and_prune` と `...edge_case_matrix`。Delta の load 0 は `incremental_reconcile_missing_subtree_reobservation_prunes_nested_prefix` が確認する。destructor 回数を直接数える専用テストではなく、drop は Rust の所有構造、テストは取消 rollback・no-load・publish 結果をそれぞれ根拠とする。

進捗の製品実装自体は完了済み。`IndexProgress` の Running/AwaitingWatch/AwaitingArray/Degraded 等を `src/ui_dialogs/favorites_editor.rs` が直接描画し、Scanning の確認済み・発見済み・総数未確定、監視準備待ち、検索用一覧への反映中、typed reason を区別する。未完なのはこの描画の snapshot coverage であり、scheduler の是正を最初から実装する必要はない。

次の担当範囲は同ファイルの inline match を小さな実描画 helper へ切り出し、dialog と `tests/ui_snapshot.rs` の小 scene が同じ helper を呼ぶ境界だけとする。対応 snapshot asset も更新する。新 state・ETA・DB API・scheduler API は追加せず、`src/similar_index.rs` と `src/app.rs` は変更不要。docs の証跡追記は親が担当する。

受入条件は、Scanning の未知総数を割合や残数にしないこと、AwaitingArray を Complete/監視中と表示しないこと、AwaitingWatch/Degraded を区別すること。全 `IndexProgress` variant と `FilesystemObservationIncomplete` / `ArrayPublication` / `WatchUnavailable` の reason mapping は純粋テストで確認し、snapshot はテスト側に文言を再実装しない。小さな headless snapshot と helper テストで確認する。Cancelled 詳細行非表示など既存の別仕様を変更せず、live UI 操作はこの小変更に含めない。これは次の設計整理であり、今回の有限測定後に実装・compile を開始していない。

### 進捗表示の検証実装

Phase 2 を `aea120cf6` に保存してから開始。`favorites_editor` は private module のため、snapshot policy に従い同 module の lib unit test から実描画 helper を呼ぶ方式とした。外部 integration test 向けの公開 API は追加しない。既存 inline/activity の presentation mapper と activity row helper を共用し、表示文言・色・順序・truncate/hover は維持した。scheduler/DB/enum に変更はない。

全 variant/stage・3 Degraded reason・disabled を確認する純粋テスト 2 件、snapshot 生成 1 件と通常比較 1 件が成功。glyph 0、fmt/diff check 0。root は `tests/snapshots/favorites_similar_index_progress_dark.png` を目視し、8 行の無効/Scanning/Pruning/待機 2 種/未完了 3 理由が読め、欠け・tofu・意図外の折返しがないことを確認した。証跡は `target/similar-index-incremental-ui-20260911/freeze-r2/` と同親 directory の各 log/exit 記録。全体 gate と normal 確認 build は独立最終検収後に実施する。

独立 Sol / xhigh の最終差分検収も P1/P2 なしで承認。`freeze-r2/review-approval.md` SHA256 は `F8AF863C36ABCB3A3BFCC37AB99F1A99746C0AD059D8711C47D45C0085194DCE`。既存表示契約・pure mapper・実描画経路・検証ログと目視結果を照合済み。
