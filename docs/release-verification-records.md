# リリース前確認の記録

各リリースで実際に取った perf smoke / idle health / bench / 依存確認の結果を、版ごとに
残す。**次に何かが遅くなったときの比較対象**がここにある。手順そのものは
`CLAUDE.md` の「リリース手順チェックリスト」と [release-operations.md](release-operations.md) が正本で、
このファイルは測定値と、そのとき気づいたことだけを持つ。

[next-release-backlog.md](next-release-backlog.md) から分離した (2026-08-16)。バックログは
未着手の作業候補だけを置く場所で、済んだ測定の記録が混ざると残件が読み取りにくくなるため。

新しい版の記録は**このファイルの先頭側 (新しい順)** に足す。

---

## v3.4.0 (2026-09-01)

- **依存**: PDFium を `chromium/8021` → **`chromium/8035` へ更新**。FFmpeg は
  `setup-ffmpeg.sh check` が「新版あり」と言うが**誤検知**で、提示された
  `n7.1.5-12-g1fdbca85aa` は現行 `n7.1.5-16-g9a4bb2c579` より**古い** (12 < 16、
  文字列比較のため)。DLL の ProductVersion は `n7.1.5-16-g9a4bb2c579-20260816` で
  `vendor/ffmpeg/VERSION` と一致しており、LGPL 対応ソースのズレも無し。更新不要。
  VST3 bridge は C++ ソースが 2026-05-16 から変更無しで再ビルド不要 (`-SkipVst3Bridge`)。
  Susie ワーカーは再ビルド済み (ソース変更無し)。
- **非 Windows ビルド**: `check-non-windows-shadow.ps1` で **PASS**。
  master は未 push だったので GitHub Actions は前版のまま = このローカル検査が代替。
- **Rust テスト**: `test-full.ps1` で **7,846 件通過 / 失敗 0**。
- **idle health**: `static-foreground` / `static-background` / `tray-residency` の
  **3 シナリオとも PASS**。全区間で perf event 0 件 (完全 sleep)、CPU 1 コア比は
  0.0062 / 0.0083 / 0.0167。`tray-residency` は `8/0 visible`。
  **`video-pin-background` は未実施** — 動画を代表画像に固定したフォルダを用意できず。
  この版はアイドル高画質化の経路に触れていないため waiver とした。
- **perf smoke**: フレーム 5,112 / 16ms 以上のギャップ 94 件 /
  ギャップ p50 27ms・p95 314ms・max 483ms。
  **100ms を超えた 15 件はすべて説明が付いた**: `request_repaint_after_idle_upgrade`
  11 件 (実測ギャップが `idle_upgrade_delay_ms` 222〜447ms と整合)、`none` 3 件 (就寝)、
  最初の `tail_repaint` より前 1 件 (起動直後の `load_folder_end=145.6ms`)。
  **それ以外の 100ms 超は 0 件。**
  `pre_grid_breakdown.total_ms` は **p50 0.129 / p95 0.183ms** (健全基準 0.3ms 以下)。
  max 37.98ms は `Ctrl+G` で検索バーを初めて開いた 1 フレームの
  `global_search_bar_ms=37.75` のみで、2 番目は同じ場所の 2.30ms。

- **v3.3.1 が「次版から見る」とした「描画中のみの比率」は取れなかった。**
  perf smoke の解析後にアプリを再度動かしたため、同じ `perf_events.jsonl` に別セッションが
  追記され、`analyze_perf.py` が見た 5,112 フレームの区間を後から復元できなくなった
  (先頭 5,112 件で切っても件数が合わない)。
  **次版からは、smoke 直後に perf log を版名付きでコピーしてから解析する。**

### この版の test gate が見つけたもの

- **`FtsWriterDispatcher` の起床通知取りこぼし (2026-04-22 混入、v2.x から出荷済み)**。
  test gate が `ingest_worker::tests::cancel_mid_ingest_flushes_partial_and_returns_cancelled`
  で 47 分ハング。**プロセスが生きているうちに cdb で両スレッドのスタックを採取**して確定
  (テスト側 = `Drop` → `JoinHandle::join`、dispatcher = `Condvar::wait`)。
  `Drop` が停止フラグを Mutex の**外**で立ててから notify していたため、dispatcher が
  「述語を確認したが wait に入る前」の窓に落ちると通知が失われる。フラグを `Queue` の
  フィールドへ移して構造的に解消 (`e935c79b`)。本番では最後の `Arc` が落ちる場所
  = 終了時に踏み得るので、「アプリが終了しきらない」症状として出ていた可能性がある。
- **`rotation_db` / `book_resume_db` のテスト分離不備** (`af60acff`)。
  data_dir の test override を自分で張らず、同時に走る別テストのガードへ相乗りしていた。
  ガードが先に落ちると temp dir ごと消えて `CannotOpen`。他 worktree で cargo が 4 本
  走っている負荷下で初めて出た。**他 5 ファイルはガードを張っており、この 2 件だけ。**

---

## v3.3.1 (2026-08-30)

- **依存**: PDFium `chromium/8021` は最新で更新なし。**FFmpeg を
  `n7.1.5-12-g1fdbca85aa` → `n7.1.5-16-g9a4bb2c579` へ更新した** (v3.3.0 では見送った差分)。
  4 コミットのうち 2 件が H.264 のフレームスレッド同期の修正
  (`h264_direct` の colocated field pair 待ち合わせ、`pthread_frame` の side data 同期) で、
  デコーダの正しさに関わるため取った。DLL 名は不変 (`avcodec-61` 等) なので
  `ffmpeg_loader.rs` / `build.rs` の変更なし。更新前の DLL ProductVersion は
  `vendor/ffmpeg/VERSION` と一致していて腐っていなかった。対応ソースは commit 指定で取得し、
  sha256 をツリー外バックアップと照合済み。VST3 bridge / Susie は C++ / Rust ソースに
  変更が無いので再ビルド不要。
- **非 Windows ビルド**: `check-non-windows-shadow.ps1` で **PASS** (エラー 0)。
- **Rust テスト**: lib 6,742 + 統合 18 本 + UI スナップショット 43、すべて緑。
- **idle health**: 4 シナリオとも **PASS**。全区間で perf event 0 件 (完全 sleep)。
  CPU 1 コア比は 0.0104〜0.0344。`video-pin-background` は
  `idle_upgrade_ineligible=1` / `matched=31` でセットアップ成立を確認。
  `tray-residency` は `8/0 visible` (全ウィンドウ非表示)。
- **perf smoke**: 2 回取った。**どちらも 100ms 超のギャップに説明のつかないものは 0 件。**

  | | フレーム | 区間 | 16ms 未満 (全体) | 16ms 未満 (描画中のみ) | 描画中 p95 / max |
  | --- | --- | --- | --- | --- | --- |
  | 1 回目 (動画サムネの多いフォルダ) | 3,236 | 46.0s | 89.6% | 75.6% | 23 / 47ms |
  | 2 回目 (キャッシュ済みフォルダ) | 834 | 11.2s | 91.4% | **92.4%** | 17 / **25ms** |

  - 100ms 超の内訳は両方とも `none` (就寝) / `request_repaint_after_idle_upgrade`
    (予定どおりの遅延 wake。実測ギャップが `idle_upgrade_delay_ms` と整合) /
    最初の `tail_repaint` より前、のみ。
  - `pre_grid_breakdown.total_ms` は **p95 0.2〜0.3ms**。1 回目の max 42ms は
    Ctrl+G で検索バーを開いた 1 フレームの `global_search_bar_ms` のみで、他は 0.5ms 以下。
  - 1 回目が重いのはサムネイル処理と重なるフレーム (402 枚で 69.7%) に集中しており、
    その回は 46 秒で **6,961 件**の thumb イベントが出ていた (2 回目は 2,070 件)。

- **この版で分かったこと: 「16ms 未満が 97% 以上」という目安は、セッションの長さと
  内容が揃っていないと比較に使えない。**
  この比率は**正しい就寝をヒッチとして数える**ので、短い・暇なセッションほど悪化する。
  実際 2 回目 (11 秒) は健全そのものなのに 91.4% で、v3.3.0 の 97.3% (4,708 フレーム) と
  並べると退行に見えた。並べてよいのは **`request_repaint` が直前にある「実際に描いている
  フレーム」だけに絞った比率**で、こちらは 2 回目 92.4% / 最大 25ms と明確に健全。
  次版からは全体比率ではなくこちらを見る。

## v3.3.0 (2026-08-29)

- **依存**: PDFium `chromium/8021` は最新で更新なし。FFmpeg は現行
  `n7.1.5-12-g1fdbca85aa` に対し `n7.1.5-16-g9a4bb2c579` が出ていたが**更新しない判断**
  (同じ 7.1.5 系で 4 コミット差、DLL 名も不変。更新すると LGPL 対応ソースの再配置と
  製品ページ改訂、動画再生の再確認が付く)。DLL の ProductVersion は `vendor/ffmpeg/VERSION`
  と一致していて腐っていない。VST3 bridge は C++ 最終変更 2026-05-16 に対し exe が
  08-23 ビルドなので再ビルド不要 (`-SkipVst3Bridge`)。
- **Rust 全体テスト**: `test-full.ps1` で **7,604 passed / 0 failed** (exit 0)。
- **idle health**: 4 シナリオ (`static-foreground` / `static-background` /
  `video-pin-background` / `tray-residency`) とも **PASS** (利用者実施)。個々の数値は
  この記録には残していない。
- **perf smoke**: 4,708 フレーム中、間隔 16ms 以上が 126 件 = **16ms 未満が 97.3%**
  (v3.1.0 の 97.74% / v2.9.1 の 97.7% と同等)。100ms 超は 24 件:
  - `action: none` (入力待ちで就寝) **16 件** — 正常
  - `request_repaint_after_idle_upgrade` **5 件** — 正常。実測ギャップが
    `idle_upgrade_delay_ms` とほぼ一致 (418.9ms / 433.8ms など)
  - `request_repaint` **2 件** — 原因特定済み。t=1.18s の 178ms は
    プロセス最初の保存に伴う世代ローテ (`sli_settings_save` 129ms)、t=2.39s の 488ms は
    PDF サムネイル生成中 (1 ページの `render_ms` 408ms)。perf_smoke 自身が
    「PDF cold open 等が紐付いていれば許容」としている類型
  - 最初の `tail_repaint` より前 1 件
  - **説明のつかない UI スレッド同期 I/O は無し。**
- **この版で新たに分かったこと (次版対象、§1.0b / §1.0c)**:
  - `Settings::save()` が**フォルダ移動 1 回ごとに 43〜61ms**。38 回の移動で計 1.8 秒。
    原因は `settings.db` が 36.7 MB で、その 98% が VST3 状態 BLOB (`vst3_plugins` 7 行
    32.04 MB + `vst3_chain_slots` 2 行 4.16 MB、他すべてで 0.2 MB)。**行自体は
    ハッシュ比較で書き直していない**が、そこへ至る clone 2 回・JSON 値構築・ハッシュに
    43ms を払っている。**v3.2.0 に同じコードがあり退行ではない。**
  - 合成データでの `save_full` 実測は既定設定 1.3ms / +500 resume positions 2.2ms /
    +1MB VST3 状態 17.4ms、`rotate_backups` 10 世代 6.9ms、`Settings::clone` 34µs。
    **実機の 43ms を 30 倍過小評価していた。** 次に同種を測るときは実機の settings.db で測る。
  - `settings.db*` が **124 ファイル・2,980 MB** (`preupgrade` 36 個 1,053 MB /
    隔離 22 個 775 MB / その他 55 個 748 MB / `bak1..10` 367 MB / main 37 MB)。
- **補正マスクの圧縮 (R-07 判断材料)**: release ビルド実測で `encode_alpha` は
  0.8MP 2.4ms / 3MP 8.1ms / 12MP 33ms / **24MP 70.6ms**。v3.2.0 の形式 (素の JSON 数値配列)
  は同条件で 11.9 / 49.4 / 195 / **410ms** なので、**v3.3.0 の codec 変更が 5.8 倍速く・
  600 分の 1 の大きさにした後の値**。マスクは画像原寸 (`local_adjust_image_dims`)。
  レイヤー文書の 1 複製は 3MP 3.5ms / 12MP 14.3ms / **24MP 27.7ms (96 MB)**。
- **Remote の寸法 probe (R-23 判断材料)**: `page_dims_without_catalog` はローカル SSD で
  初回 454µs/件・キャッシュ後 23µs/件。1,000 件で 0.45 秒、10,000 件で 4.5 秒。
  open のコストが支配的なので HDD / NAS では 1 件 1〜10ms に伸びる。
- **配布物の回帰チェック**: `dumpbin /dependents` で launcher / core / remote / portable の
  4 本とも `VCRUNTIME140.dll` / `MSVCP140.dll` **なし**。`signtool verify /pa` で単体exe /
  setup.exe / portable の 3 本とも VALID + RFC3161 タイムスタンプ、チェーンは
  `Certum Trusted Network CA → CA 2 → Certum Code Signing 2021 CA → Open Source Developer Taku Sano`。
- **検索 bench**: 全文索引に触れていないため未実施。

## v3.1.0 (2026-08-16)

- **依存**: PDFium `chromium/7988` / FFmpeg `n7.1.5-12-g1fdbca85aa` とも更新なし。DLL の
  ProductVersion が `vendor/ffmpeg/VERSION` と一致、LGPL 対応ソースの差し替え不要。
- **idle health**: `static-foreground` / `static-background` とも **PASS**。どちらも測定 15 秒の
  区間で perf event 0 件 = 完全 sleep。CPU one-core ratio は 0.0052 / 0.0156、perf ログ増加 0 バイト、
  通常ログ増加 312 / 186 バイト。
  - `video-pin-background` は**未実施**。`-TargetKey` に渡せる「動画を代表画像に固定した
    フォルダ」を用意しなかったため。この版はアイドル高画質化の経路に触れていないので waiver。
    §5.4 (evidence 窓が狭い) が片付けば準備の負担も下がる。
- **perf smoke**: 6060 フレーム中、間隔 16ms 以上が 137 件 = **16ms 未満が 97.74%**
  (v2.9.1 の 97.7% と同等)。100ms 超は 28 件で、内訳は `action: none` (入力待ちで就寝) 5、
  `request_repaint_after_idle_upgrade` (予定どおりの遅延起床) 5、`request_repaint_after_ai_upscale` 4、
  `request_repaint` 13、起動直後 1。
  - `request_repaint` の 13 件は全て t=15〜21 秒のフルスクリーン中。`fullscreen_viewport_ms`
    98〜133ms / `background_polls_ms` 26〜68ms に対し、**描画クロージャ本体
    (`fs_viewport_breakdown` の `central_ms` / `closure_ms`) は 1ms 前後**。時間は viewport の
    present とバックグラウンド結果の取り込み (テクスチャアップロード) にあり、AI アップスケール中の
    設計どおりのコスト。UI スレッド同期 I/O の追加ではない。
- **検索 bench**: 全文索引に触れていないため未実施。

## v2.13.0 (2026-08-11)

- **依存**: PDFium `chromium/7988` / FFmpeg `n7.1.5-12-g1fdbca85aa` とも最新で更新なし。
  DLL の ProductVersion `n7.1.5-12-g1fdbca85aa-20260803` が `vendor/ffmpeg/VERSION` と一致。
  LGPL 対応ソースの差し替え不要。VST3 bridge は C++ 最終変更 2026-08-01 / exe ビルド
  2026-08-06 なので再ビルド不要 (`-SkipVst3Bridge`)。Susie ワーカーは再ビルドした。
  - ⚠ `setup-pdfium.sh check` は「新しいバージョンあり」と誤検知する。`vendor/pdfium/VERSION`
    が無い環境では現行版を「未インストール」と読むため。**DLL の FileVersion で判定する**
    (`153.0.7988.0` = `chromium/7988`)。FFmpeg の VERSION 腐りと同じ型。
- **CI**: master success (run 31402289417)。ubuntu の `cargo check` を含む。
  今サイクルの 114 commit が初めて CI を通った。タッチ対応は `#[cfg(windows)]` の塊なので
  ここが最大の未検証点だったが、漏れは無かった。
- **署名 / 依存 DLL**: 配布 4 種すべて `Certum Trusted Network CA` + RFC3161 タイムスタンプ。
  `dumpbin /dependents` で launcher / core / portable / susie32 とも
  `VCRUNTIME140.dll` / `MSVCP140.dll` なし。
- **idle health**: `static-foreground` / `static-background` とも PASS。
  測定窓は perf event 0 件・perf log 増加 0 バイト (完全 sleep)、CPU one-core ratio
  0.0219 / 0.0115。
  - **1 回目の `static-foreground` は FAIL (35 fps) だったが退行ではない**。起動から約 2 分の
    時点で、索引作成が進行中かつお気に入り編集ダイアログを開いたまま測っていた。
    同ダイアログは進捗を流すため 100ms ごとに repaint を要求する既存仕様
    ([favorites_editor.rs](../src/ui_dialogs/favorites_editor.rs) の live 更新)。
    ダイアログを閉じて測り直すと 0 フレーム。**「過去 3 回は 0 フレーム」という比較だけで
    退行と断定してはいけない**。アプリが busy でなかったことを先に確認する。
  - `video-pin-background` は **§5.4 の既知のゲート欠陥で不成立**。証拠窓が測定区間そのもの
    (t=352.4-367.4) になり、対象キーの thumbnail work は t=337.5 で 15 秒前に終わっていた
    (`matched=0`)。加えて操作側の誤りもあり、ピン留めフォルダの**中**を表示していたため
    マッチしたのは個々の動画ファイル (`from_cache=false` = アイドル高画質化の候補外) で、
    フォルダタイル (`dir::<path>`) ではなかった。**waiver とする**根拠は、
    v2.13.0 が `idle_upgrade` を含む行を 1 行も変更していないこと
    (`git log -S "idle_upgrade" v2.12.0..HEAD -- src/` が 0 件)、および測定区間自体は
    完全 sleep (CPU 0.0052) だったこと。セッションログは
    `target/idle-health/v2.13.0-idle-health-session.jsonl` に退避済み。
- **perf smoke**: frame 4476、16ms 超のギャップ 132 件 (97.05% が 16ms 未満)。
  **件数ではなく直前の `ui.tail_repaint.action` で判定**した結果:
  - 100ms 超は 18 件。うち 11 件が `none` (repaint 未要求 = 入力待ちで就寝)、
    6 件が `request_repaint_after_idle_upgrade` で `idle_upgrade_delay_ms` がギャップ長と
    一致 (予定どおりの起床)
  - **`request_repaint` が立っていた 100ms 超は 1 件のみ**: 起動直後 t=1.109s の 198ms
    (`reasons=['requested_nonempty']` = サムネイル要求中)。今サイクルの新経路とは無関係
  - 100ms 超の件数は v2.12.0 の 53 件・v2.9.1 の 24 件より少ない
- **検索 bench**: 全文索引に触れていないため未実施。

## v2.12.0 (2026-08-06)

§5.5 の「記録が残らない」を繰り返さないための実測記録。

- **依存**: PDFium `chromium/7988` / FFmpeg `n7.1.5-12-g1fdbca85aa` とも最新。DLL の
  ProductVersion `n7.1.5-12-g1fdbca85aa-20260803` が `vendor/ffmpeg/VERSION` と一致。
  PDFium MAJOR 152 → 153 の実機確認 (通常 / パスワード付き PDF、動画 / 音声) 済み。
- **CI**: master / main とも success (run 31021319941 / 31021332945)。ubuntu の
  `cargo check` を含む。今サイクルの 32 commit が初めて CI を通った。
- **idle health**: `static-foreground` / `static-background` とも PASS。CPU one-core ratio
  0.0073 / 0.0083、測定窓は perf event 0 件 (完全 sleep)。`video-pin-background` は
  条件を満たすフォルダを用意しなかったため未実施。アイドル高画質化経路は今サイクルで
  触っていないので waiver とする。
- **perf smoke**: frame 7968、16ms 超のギャップ 1376 件。**件数ではなく直前の
  `ui.tail_repaint.action` で判定**した結果:
  - 16-100ms 帯 1323 件のうち **1259 件が `none`** (repaint 未要求 = 入力待ちで就寝)。
    p50 85ms は Ctrl+↓ 連打時のキー間隔そのもの
  - 100ms 超 53 件のうち 42 件が `none`、7 件が `request_repaint_after_idle_upgrade`
    (予定どおりの起床)。最大の 33.7s / 30.3s / 20.1s / 19.6s は idle health の測定窓
  - **`request_repaint` が立っていた 100ms 超は 3 件のみ**: 起動直後 356ms、
    PDF を含むフォルダ読み込み 585ms + 106ms (`load_folder_end=55ms` +
    thumb decode + `pdf/pool_promote_visible`)。今サイクルの新経路とは無関係
  - ⚠ 同一フォルダでの v2.11.0 基準値は未取得なので「悪化していない」とは言えない。
    次サイクルで PDF フォルダの folder-load ギャップを基準値化するか判断する
- **検索 bench**: 全文索引に触れていないため未実施 (ファイル名フィルターは一覧の絞り込み)。

## v2.9.1 (2026-07-30) — perf smoke を最終 tree で取れていない

- 出典: v2.9.1 出荷後の他セッションレビュー (2026-08-01、ClaudeCode)。本セッションで確認済み。
- 何が起きたか: perf smoke の記録は 2026-07-31 の tree のものしか無く (`17231956` は
  CLAUDE.md の判定基準を書き足した commit で、計測そのものではない)、**8/1 に入った構造変更の
  後に取り直した記録が見当たらない**。対象の変更は入力所有権 (`56db11bc` / `596bb65d`)、
  `FsPageLoadState` (`9590b661`)、health watchdog (`2c11e4cd`)。
- ずれている扱い: idle-health 側は §5.4 の waiver を「未実測部分を明示する」形に書き直したのに、
  perf smoke 側は同じ扱いになっていない。CLAUDE.md Phase 2 の 9.6 は「UI 周り / I/O 経路に
  変更を入れたリリースで実施」なので `FsPageLoadState` はまさに対象。
- 対応 (どちらか):
  1. v2.9.1 の tree で perf smoke を取り直す。**PDF フルスクリーンでの計測を含める**
     (§1.39-2 の毎フレーム経路がそこにしか出ない)。
  2. 取らない判断を §5.4 と同じ形で明文化する (何が未実測かを書く)。
- 規模 / 優先度: Small / P2 (プロセス)。次版のリリース前確認までに片付ける。

---
