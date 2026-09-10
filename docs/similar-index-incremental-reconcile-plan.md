# 類似索引の増分照合と収束

## 範囲と状態（2026-09-11）

第一段階の実装・独立差分レビュー・関連狭域検証は完了。第二段階の DB 読み取り削減は設計合意済みで未実装。全体 gate・UI snapshot 追加・確認 build は未完了であり、機能の再有効化やリリース完了ではない。
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

第二段階は Full 自体の DB 点 SELECT を compact inventory に置換する。短い read transaction で metadata のみを取得して閉じ、exact String key map と record/seen bitset を使う。全 SearchRow や署名は保持しない。既存 seen clone と prune 全 key 再読込を置換し、追加常駐にしない。snapshot 後の新規行や container→loose 所有変更を誤削除しない。
約 464 万件で key 長 48/80/120 bytes の inventory は概算 646/788/965 MiB、検索 array 等は別。概算であり実測ではない。synthetic DB で点 SELECT と compact 方式の同値・時間・SQL 数・peak memory を比較し、容量が過大なら exact collision 検証を持つ arena 方式を評価する。

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
