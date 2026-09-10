# v3.7.0 後の開発・検証台帳

開始: 2026-09-09。作業場所 `C:\home\mimageviewer`、`master`。
開始 HEAD: `dd997b538` (v3.7.0)。次の版番号は公開準備時に確定する。

## 依頼と境界

利用者から、延期した動画の進む・戻るボタンのシーク (§4.2) と、
画像・本のフルスクリーン余白背景色 (§1.203) の開発再開を受領した。
表紙と裏表紙の合成見開き等は今回の範囲へ追加しない。

v3.7.0 は GitHub Release / mikage.to / Vector の反映を完了した。
Microsoft Store は前回審査待ちで利用者判断により保留。
同版の perf smoke / idle-health 省略了承は同版限りであり、次版へ引き継がない。
公開結果の証跡は `target/v370-work/release-v370-final/github-published-verified.json` と
`site-manual-checks.json`。旧 v3.7.0 台帳の「公開前」「修正済み」は各時点の記録として扱う。

## 担当と変更所有

- 親: 設計判断、範囲、統合、本文書。開発は現行 AGENTS.md の担当規則に従う。
- `next_seek` (Sol / xhigh): シークの実装前調査と `next-version-mouse-seek-plan.md`。
  入力所有境界を変える前に根拠と変更ファイルを提示する。通常判断は一括委任する。
- `next_background_design` (Sol / xhigh): 背景色の既存調査をコードと照合。当初は読み取りのみ。
- 独立レビューは実装担当とは別の Sol / xhigh が重要設計とまとまった差分を確認する。
  同一ファイルの編集を並行しない。共有ファイルは先行担当から引き渡した後に次担当が編集する。
- Cargo と確認用ビルドは担当を一人に集約し、ソース変更中の検証や重複実行を避ける。

開始時の未コミット変更 (`AGENTS.md`、`CLAUDE.md`、ビルド・公開運用文書、
`.gitattributes`、sitemap、ui-smoke 関連、他系統の未追跡文書等) は保持する。
旧 detached 文書の ClaudeCode 設計・検収指定は、現行 AGENTS.md の明示した移行規則により
開発の親と独立レビューへ対応させる。§2 の構造制約と §11 の合意記録は省略しない。
次回の公開準備・公開は同規則どおり ClaudeCode Opus へ引き継ぐ。

## シークの不変条件と完了条件

前回の実機ログでは実 scan 付き VK_BROWSER と scan 0 の通知がそれぞれ seek を発火した。
下流の一つの receipt の二重適用ではなく、生成元が未確定の二系列入力が残っている。
WndProc の既定処理抑止だけで解消したという過去の判定は、後続実機報告により未成立。

- 1 クリックは 1 action、素早い 2 クリックは 2 action。時間 debounce で隠さない。
- 長押し開始・反復・解放の意味を明確にし、キーボードのシークとの差を説明・検証する。
- presenter / HUD / backdrop / main / F12、直接 APPCOMMAND 入力機器の同等入口と
  focus / modal / open / close の所有境界を確認する。フォルダ移動も維持する。
- 既存保存値、キーボード・リング・ジェスチャ、シーク秒数、通常ホイールを維持する。
- 当初は根因の裏付け前に候補を復活させない方針だったが、後述の利用者の明示依頼により
  確認用buildでは先行して再公開する。必要な実機計測は未検証として残し、出荷合格としない。

## 背景色の不変条件と完了条件

- 既定黒。画像・本の余白へ共通の RGB 設定を適用し、黒・灰色・白と任意色を選べる UI を設ける。
- 透過画像の黒・白・市松設定は別所有とし、余白色が透過ピクセルへ混入しない。
- 単ページ・見開き・連結・比較、回転・トリム、active / holdover / passive / frozen、
  main / F12 で画像外に一貫して反映する。画像の合成・AI・コピー・書き出しは変えない。
- 既存描画所有境界で分離し、場当たりの背景 fill 追加や detached 判定追加にしない。
- 保存互換・UI 操作・画像内外の色を意味のある regression / snapshot で確認する。

## 検証運用

実装前提検証 → 重要設計の独立確認 → 実装・焦点検証 → 完成差分レビュー →
統合後の必須 gate / 確認用 build の順で進める。成功結果は対象ソースと条件が有効なら再利用する。
重要指摘は解決まで追う。実機でのみ確かめられる部分は自動検証と区別する。

実アプリ起動・実マウス入力は、内容と所要時間を示した事前了承のある枠でのみ実行する。
通常の開発中に予告だけで入力を占有しない。通常プロファイルをエージェントが起動せず、
通常データの変更・実行中アプリ停止もしない。必要時は隔離 portable 手順を守る。

## 2026-09-09 設計チェックポイント

背景色は親と独立 Sol の双方で構造方針へ合意した。単一の Settings RGB 所有、
同じ画像 quad を使う不透明な透過下地、view 単位で capture する frozen 背景と
frame ごとの現在余白色を分離し、detached の lifecycle / predicate / placement は変更しない。
既存 backstop の描画情報を共通表示 payload へ寄せる際、1/2 ページの capture だけでは
連結表示を表せないため、連結用経路を保持するか到達不能の証明とテストを必要とする。
市松 UV は実寸から repeat させ、動画・音楽・ParkedLive media の黒を維持する。
この条件付きで背景色の実装へ進む。完成時の §11 記録は背景色担当が所有する。

初期診断段階では [診断計画](next-version-mouse-seek-plan.md) の入力由来情報を追加し、
候補や実行の抑止を維持した。その後、後述の利用者依頼で試用再公開へ変更した。
生成元の確定と根因修正は引き続き必要であり、過去の実機失敗を自動テスト成功で置き換えない。

共有 `ui_fullscreen.rs` / `lib.rs` は入力担当が診断用の変更を終えた時点で背景色担当へ引き渡す。
背景色担当はその他の Settings / 描画 geometry / frozen payload / preferences を先行する。
両差分の編集が止まった区切りで背景色担当が統合検証と build を所有する。

### 編集事故の復元記録

背景色編集後の整形チェックで、`src/app.rs` の既存関数群約 1,220 行の欠落と
`8platform` という不正文字列を検出した。入力担当は同ファイルを編集しておらず、
確認したタスク一覧にも同じ作業場所の別の稼働中書き手はない。
背景色編集の間に生じた事故として扱う。置換の group 展開の誤りと整合する形だが、
該当コマンドを確定できていないため原因の詳細は断定しない。

親が `target/next-version-work/recovery/app-corrupt-20260909.rs` と
`app-corrupt-diff.patch` に保全した後、担当が壊れた関数境界の範囲だけを HEAD から復元し、
必要な DTO / projection の変更を残した。ファイル全体の checkout / reset は行っていない。
復元後の差分は独立 reviewer も確認した。実行・配布前に発見した事故であり、
この復元を含む最終ソースに対してコンパイルと必須検証を実施する。

## 2026-09-09 使用中の画像ページ送り停止の調査

次版の実装・検証を一時停止し、利用者が使用中のインストール版 v3.7.0
（`dd997b538e1621f555e4c121ee061a1440ac8e68`）を読み取りで調査した。
画像が「読み込み中」のまま、ホイールのページ送りが進まない。一方、上下 HUD と
左右パネルのホバー、別窓の動画再生は動作するとの報告。

ログでは対象画像のデコードが完了し、同じ index / generation に対して
`DisplayReady(LiveCache)` を返している。別窓の描画 frame と UI heartbeat は進み続ける。
停止させない方式のスレッド採取でも画像デコード中の worker は見つからず、
プロセス全体のロック待ちを裏付ける証拠はない。ページ移動の完了待ち状態を調査中。

現時点の有力候補は、寸法未判明時に移動先を見開き 2 ページとして確定した後、
横長画像の寸法が判明して描画側が単ページへ変わる経路。移動待ちが保持する
`target.pages` と実際に描画したページ集合の完全一致を待つため、集合がずれたまま
解除されない可能性がある。保存ログの両ページ読み込みと横長寸法に整合するが、
実行時の待機状態の直接取得や回帰テストによる確定には至っていない。
単に待機を消す修正は行わず、表示単位の更新と移動完了判定の所有境界を確認する。

独立レビューで v3.7.0 の `begin_fs_page_navigation_sequence` がページ集合を保持し、
寸法到着による `landscape_epoch` の更新で描画側だけ再ペアリングする経路を確認した。
正常終了の `observe_fs_navigation_sequence_presented` はページ集合の完全一致を要求する。
このため「寸法不明の 2 ページ → 横長判明後の 1 ページ」の不整合が最有力原因である。
ホイールの `Target` / `Delta` は待機中にログなしで次の移動を拒否するため、
`[fs-nav]` の記録がないこととは矛盾しない。次段階では寸法が順次到着する回帰テストで
確認し、描画単位と navigation target の所有境界で修正する。現状は未修正。

証拠はローカル `target/next-version-work/live-hang-20260909/` に保存し、外部送信していない。
利用者アプリの操作・停止・再起動、通常設定の変更は行っていない。
採取完了後、利用者へ現状を維持する必要はないと伝えた。
大きなメモリダンプは独立分析で不要と判断できたため、今回生成したファイルだけを削除し、
ログとスレッド記録を保持した。

### §1.204 修正の委任と完了条件

利用者が次リリースでの修正を承認したため、背景色の編集を凍結して
`ui_fullscreen.rs` / `app.rs` / 対応テスト / display-pipeline / detached §11 を
入力担当 Sol / xhigh へ引き渡した。独立 Sol / xhigh が設計と完成差分を検収する。
旧 ClaudeCode 担当名は現行 AGENTS の役割対応に従い、親と独立 reviewer が構造判断する。

- 移動先の anchor と context / items 世代の同一性を守り、寸法・回転の到着で変わる
  canonical な表示単位を readiness / presentation / retirement で一致させる。
- 部分表示で早期解除しない。holdover、AI / rendition / failure、フォルダ着地、
  連結、main / detached、close / cancel / 世代切替を確認する。
- 現行実装で失敗する遅延寸法の回帰を先に確認し、片側ずつの到着・遅延回転・
  次のページ送りの受付を検証する。影響する context 境界は回帰で守る。
- 修正担当が焦点テストを所有し、差分凍結・検収後に背景色担当へ検証を戻す。
  残る共通 gate と確認用 build は一度に集約し、有効な成功済み結果は再利用する。
- 実機操作は事前了承の検証枠まで未実施とする。確認用 build は起動せず利用者へ渡す。

再現確認: `navigation_sequence_rebinds_when_unknown_partner_becomes_landscape` が修正前に
`Some([1, 2]) != Some([1])` で失敗した。原ログは
`target/next-version-work/logs/nav-topology-regression-red-20260909.log`。
構造方針は、target が世代・anchor・元の rendition 許可を所有し、phase が検証したページ集合を
所有する形とする。集合が変われば同じ target の readiness を再判定し、完全一致・Live provenance
による終了条件を維持する。親はこの所有境界の修正へ合意した。独立 reviewer は描画前の適用順と
同一 frame の寸法到着、不要な再走査がないことも検収する。

完成差分の独立確認で、同一 frame の decision cache 更新と通常 idle の余分な unit 解決を
修正した。さらに、通常 open を通らない編集側のページ切替が古い Display intent を残す経路を
確認したため、`ui_conceal.rs` / `ui_erase.rs` の切替を既存の
`App::enter_page_edit_single_view` へ集約し、明示的に移動先を変える所有境界で旧 intent を終える。
これは同じ修正範囲内の lifecycle 整合であり、編集機能の制限や context 全体の reset は行わない。
bundle 全体の復元・切替は、同一 context 内のページ切替とは区別して扱う。

### 2 回目の大規模ファイル編集事故と復元

編集切替の追加確認中に `app.rs` の約 2,672 行が 1 行の不正文字列へ置き換わったことを
コンパイルで検出した。全担当を停止し、親と実装担当が破損ファイル・差分を保全した。
同じ作業場所で他の稼働中タスクは確認されず、背景色担当・レビュー担当も読み取りのみと確認した。
実装担当によれば、最後の成功記録以後の同ファイルへの書き込みは、小範囲の multi-file
`apply_patch` 2 回だけ。正確な内部原因は未確定であり、外部の編集と断定しない。

HEAD と保存済み成功差分から `target/next-version-work/recovery/known-good-second/src/app.rs`
へ復元元を構築し、LF 正規化後の SHA-256 が最後の成功記録
`B1D2B91A102752EDBA56305E1B1EF18605A92861935C3D702FA9085259A33C6F` と完全一致した。
現在との差分は、意図した 3 行の変更と 1 か所の破損だけに分離できた。
この破損範囲だけを復元し、意図した変更を保持して独立確認・焦点テストを行う。
失敗ログ `nav-target-lifecycle-green-final2-20260909.log` は名前に green を含むが
コンパイル失敗の証拠であり、成功として再利用してはならない。

以後、巨大な `app.rs` / `ui_fullscreen.rs` の編集は一意なリテラル照合・事前ハッシュ確認付きの
Python 原子的置換を使い、各変更直後に差分と行数を確認する。広い正規表現置換は使わない。
実行・配布前に検出した事故であり、修復後の最終ソースへ必須検証を適用する。

### §1.204 完成差分の検収

復元内容を独立照合し、修復後のソースで再検証した。移動先を変更する共通 open、
ページ編集、隠蔽・消しゴムの左右切替、連結の移動・scroll reanchor は同じ producer 側の
終了処理へ揃えた。実際の items 世代変更では古い Display intent だけを終了し、
FolderItems、同一世代、兄弟 context は保持する。

独立 Sol が完成差分を承認し、重要な未解決指摘はなし。最終証拠の対象は
app hash `0DDC1B…`、ui_fullscreen hash `5FD376…`、completed diff `BAC2FC…`。
完全な値・差分・コマンド・原ログは `target/next-version-work/logs/` に保存した。
焦点テストは target 9、topology 10、sequence 4、still-seek ownership 13、
erase / conceal / page-edit 各 1 の合計 39 件が成功、diff-check は 0。
実機確認は未実施。共有ファイルを背景色担当へ戻し、背景色側のレビュー修正後に
check / fmt / glyph / feature 固有確認 / full gate / 確認用 build を集約する。

最初の全体 gate はライブラリ 7,664 成功 / 4 失敗 / 34 ignored で失敗した。
担当の焦点再実行でも 4 件が再現した。独立確認では、1 件は共有
`fs_display_unit_page_indices` が表示順ではなく数値順を返した実装上の回帰で、RTL の
カラー化表示順に影響する。表示順を維持し、navigation 内の比較用コピーだけを整列する。
残る 3 件はテストが見開きの canonical 設定または current anchor を作らず手動で target を
注入していたため、実際の状態を作る fixture に直す。全ページ readiness / exact Live の
期待値は弱めない。全体 gate は修正・検収後に再実行し、それまでは合格としない。

## §1.205 動画シークバーのホバーサムネイル欠落

2026-09-09 の >>375 と利用者の追試で、表示をオフにしていないのにホバーサムネイルが出ない。
サムネイルストリップを表示しても隠しても発生するため、`HideWithThumbnailStrip` の条件だけでは
説明できない。次リリースの優先修正として、Sol 入力担当が読み取り調査を開始した。

まずリリース済み v3.7.0 の native presenter / HUD、hover target と共有 ThumbnailWorker の要求・
結果・描画・region の各境界を照合する。設定や静止画・動画ストリップとの混同を避ける。
この調査は、承認済みの §1.204 / 背景色の統合検証と並行するが、ソース編集・Cargo 実行は
統合検証担当からの引き渡し後に行う。実アプリの操作は未承認・未実施とする。

設定を優先照合した結果、通常DBの保存値は `always`、serde/default、preferences、
App の初期設定と `SetBarLockState` による更新、presenter の受領と許可判定まで成立していた。
現在の実行ファイルも installed runtime 3.7.0 と確認した。保存ログでは補助デコーダと
画像変換の準備に成功している。

根因は v3.7.0 `dd997b538` の下HUD painter の clip 変更である。同じ painter が
HUD上部のホバーポップアップにも使われ、`native_seek_preview_layout` が返す rect は
ストリップの有無にかかわらず `hud_rect` の上側に離れているため、全て clip される。
HUD本体の制限は維持し、既存 overlay の有効領域を使う preview 専用 painter を分離する。
対象は `video/native_presenter/render_core.rs` と対応テスト・動画設計書に限定し、
設定schema、要求・デコーダ、音声除外、既存popup配置や HWND region は変更しない。
現在の全体gateが終わってから編集し、描画された shape の clip と表示領域の関係を回帰で確認する。

### 実アプリ自動検証への追加条件

今回の欠落はデコード成功や要求完了だけでは検知できない。将来のリリース前実アプリ検証では、
シークバーへ実マウスを置き、画面上にプレビュー画像が見えることを画像で検査する。
ストリップ表示／非表示と Always / HideWithThumbnailStrip / Never の設定を組み合わせ、
表示すべき場合の出現と、隠すべき場合の非表示を確認する。設定変更の反映も対象とする。
これは追加予定の検証条件であり、実アプリ自動テストの実装済み・実行済みを意味しない。
実行は使い捨て portable と専用 fixture を使い、内容・所要時間を示して PC 操作時間枠の
明示了承を得た後に限る。通常の開発中に起動・入力を行わない。

### 修正・焦点検証の引き渡し

§1.205 の painter 分離と、前回全体 gate の 4 件の修正を一括で完了した。
最終ソースは ui_fullscreen `A8DDB8F…`、render_core `C25130D…`。
実 ClippedShape テスト、strip 配置、表示設定 policy、旧 gate 4 件、navigation target 9 件、
fmt / 対象 diff-check が成功した。詳細・完全なハッシュ・失敗試行の区別は
`target/next-version-work/logs/nav-hover-verification-ledger-20260909.md` に記録した。
完成差分の独立検収と、統合担当の最終 full gate / build はこの時点では未完了。

続く独立 Sol の完成検収で承認済みとなり、重要な未解決指摘はなし。
同一ハッシュの source/Cargo 所有を統合担当へ戻した。最終 full gate / build は同担当が実施する。

描画テストは egui Area の最初の sizing frame を破棄し、同じ ID の 2 フレーム目を検査する。
初回は invisible により Shape::Noop となるため、単なる fade 無効化では不足する。
調査途中の「Area-local と global 座標の混同」という説明は誤りであり撤回した。
最終検査は screen 座標で popup の正確な矩形・clip 包含・実画像 mesh と各操作図形を確認し、
旧 HUD clip では popup が範囲外になることも検査する。production の Area 動作は変更しない。

### 最終統合 gate

統合担当が同じ凍結ソースで `test-full.ps1 -SuppressCrashDialogs` を完了し、exit 0 / PASS。
mimageviewer lib は 7,669 passed / 0 failed / 34 ignored、残り workspace と vendor の
egui-wgpu 9 件・eframe 15 件も成功した。test-script 機能の native_ui_smoke 28 件、
通常 core check、fmt、diff-check、UI glyph 検査も成功。記録は
`target/next-version-work/logs/test-full-suppress-crash-dialogs-final.log` と統合 ledger を参照。
確認用ビルドは exact-path process preflight 後に作成する。実機検証は依然未実施である。

`build-dev.ps1` も exit 0 / DONE。事前に staged core / remote の exact executable path が
稼働していないことを読み取り確認し、自動停止経路へ入らず normal feature set の core と
remote を作成した。成果物は `target/dev-runtime/mimageviewer-core.exe`。
記録は `build-dev-process-preflight.log` / `build-dev-final.log`。エージェントは起動していない。
利用者へ通常 APPDATA profile を使うことと設定・データ更新の可能性、installed/tray 終了の
必要性を伝え、`Start-Process -FilePath .\target\dev-runtime\mimageviewer-core.exe` を引き渡す。
実機確認の対象は、動画 hover の strip on/off と設定反映、寸法判明を伴う見開きページ送り、
画像の余白色と透過下地の分離。native 変更のコミットは実機確認後に行う。

## 利用者確認と余白色の追加仕様

利用者が確認用 build で動画ホバーサムネイルの復旧を確認した。画像ページ送りは元の症状の
再現性が低く、しばらく使用して問題なしとの報告。これは継続使用での暫定良好であり、
元の発生条件を実機で再現して消失を確認したとは記録しない。

同時に、動画のレターボックスと静止画サムネイルストリップの空欄にも設定した余白色を
適用する要望を受領した。HUD と左右パネル背景は文字の可読性のため黒固定とする。
従来の「動画は黒固定」はこの依頼による仕様変更対象であり、旧設計の不履行ではない。
既存 RGB 設定の保存キーを再利用し、映像自体に含まれる黒・透明画像の下地・音声画面を
色変換しない。動画は native presenter の birth / update / 複数窓 / 描画の境界を照合し、
static strip は空欄の描画所有だけを変更する。設定 UI とマニュアルの説明も適用範囲に合わせる。

背景色担当 Sol が唯一の source/Cargo 所有者、別 Sol が重要設計と完成差分を独立検収する。
親は本文書とバックログを所有。既成功検証を再利用し、今回の差分に応じた追加・最終検証を行う。
利用者が dev-runtime を使っている可能性があるため、確認用再ビルド前に exact-path preflight を
行い、使用中なら停止せず報告する。実アプリ操作への新たな了承は受領していない。

Shift+B の市松表示についても問い合わせを受け、AI アップスケール有効時のみ黒／白の
2 段である既存仕様と公開 FAQ の記載を照合した。利用者が AI をオフにして市松を確認し、
正常との回答を受領。キー動作は変更せず、余白色との違いと AI 制限を説明する導線を補う。
マニュアル fullscreen のモード切替表の 8 / 9 に kbd 装飾が無い箇所も、同ファイルの担当が修正する。

### 動画余白拡張の構造合意

通常動画 zoom の fit も source 外座標で letterbox を表現するため、DComp root 背景だけでなく
各 resolve の source 外色も同一 RGB とする。親・独立 Sol がこの構造へ合意した。
共有 NIS / Anime4K shader Params の変更は still 側 packer / layout も同時に整合させ、
still は従来の opaque black を渡して画像処理結果を維持する。映像内画素・clamp・panorama の
無効領域色は変更しない。停止中の DisplayResolved は既存 visual-change の再present 経路を使い、
新たな待機 flag は追加しない。非表示から戻る経路、birth / resize / placement 切替でも同色を維持。
音声専用・動画の音声モードは黒を保つ。共通 GPU Params の影響確認として最終 full gate を行う。

追加実装の独立 Sol 検収は承認、未解決指摘なし。設定 birth/live、DComp の成功時更新、
resize/placement、停止・非表示からの再present、複数 context、RGB 順序、zoom source 外、
共有 shader の still 黒固定を確認した。音声切替は非黒 seed から黒への実更新と通常復帰を、
strip は設定色の opaque fill が cell fill より先で panel の黒背景を保つことを回帰で確認。
最終対象は `target/next-version-work/logs/video-canvas-final-source-hashes-20260909.txt`。
親も更新済み preferences snapshot を目視確認した。全体 gate はこの追加差分で再実行する。

追加差分の初回 full gate は snapshot の 4 対象で FAIL（lib 7,673 成功 / 3 snapshot test 失敗、
ui_snapshot 44 成功 / 1 失敗）。strip_content 下地変更に伴う期待画像 11 枚の差分だった。
独立 Sol が全画像を目視し、両端空欄と cell 境界の下地だけの変更で、HUD / panel / popup /
文字 / cell 内容・枠 / main image が不変であることを確認して承認した。
期待 PNG を更新後、失敗した 4 対象は通常比較モードで各 1 件成功。production / test code の
ハッシュは凍結時の manifest と一致するため、その他の full gate 成功結果を再利用する。
初回 script 自体が exit 0 だったとは記録しない。失敗ログと対象再検証を分けて保持する。
記録: `video-canvas-test-full-suppress-crash-dialogs-20260909.log` と
`video-canvas-snapshot-*-final-20260909.log`（いずれも `target/next-version-work/logs/`）。

最終 build preflight では dev-runtime の core / remote が使用中だったため、
`build-dev.ps1` は未実行。利用者アプリの自動停止・実行ファイル差し替えは行わない。
`video-canvas-build-dev-preflight-20260909.log` に記録した。現在の dev-runtime バイナリは
追加拡張前のものであり、新しい色適用の実機確認には利用者が終了した後の再ビルドが必要。

その後、利用者から終了とビルド依頼を受領。再 preflight で core / remote とも 0 を確認し、
`build-dev.ps1` が normal feature set / dev-runtime で exit 0（1 分 44 秒）。
追加拡張を含む core SHA-256 は `EA548F837D13D1F44336457F3A472FAC271E7514C6926C9B0196FAAF2E22C528`。
source manifest 全件一致、アプリ起動・通常 profile 操作なし。
`video-canvas-build-dev-final-20260909.log` と `video-canvas-build-dev-artifact-hashes-20260909.txt`
に記録。上記の build 保留は解消済みで、実機での新しい余白色確認が残る。

## ストリップ右端・動画ストリップ背景と V キー調査

利用者が静止画 strip の鍵専用幅が黒く残ることを報告し、動画のような overlay 表示へ
合わせるよう依頼した。動画 strip のサムネイルが無い部分・波形の無い部分も同じ余白色へ
統一する方針で調査する。bar hidden 時の toggle も overlay に揃え、layout/request/hit の正本を
共通 strip_content とし、control 下で body hover/drag/preview を発火させない。
波形 raster の opaque black を単に背面 fill で隠せると仮定せず、音声解析と表示下地を分離し、
色変更で再解析を増やさない。白い下地でも波形・文字・marker の可読性を保持する。
実装と Cargo は背景担当 Sol のみ、独立 Sol が構造と完成差分を検収する。

V キーは親が read-only 調査を担当。現行 log を
`target/next-version-work/video-v-key-20260909/mimageviewer.log` へ保全した。
経過 159〜181 秒の V は `[fs-key] source=fullscreen` に到達し、その区間の native handler 記録は無い。
193 秒以降は別動画で `[native-video-key] outcome=action:FsPanorama` と直後の
LegacySource / DisplayResolved 切替を確認。これは後続の正常操作かもしれず、利用者へ
不具合動画名・時刻を質問した。100% 不表示だけで原因を断定しない。
コードでは detached native 動画が汎用 `handle_fs_key_input` に到達し得る一方、同経路の V は
`panorama_entry_allowed` / `toggle_panorama_mode` のみで、native 側の通常動画 zoom 分岐を持たない。
入力先による操作処理の不整合を §1.206 として調査し、focus hack や raw key 再注入は行わない。

## 再起動後の再開と次リリース範囲

利用者はサムネイル不具合の早期公開を優先しつつ、延期した進む・戻るボタンのシークも次リリースへ含める方針を再確認した。追加の無関係な機能は増やさない。公開準備は開発検収後に ClaudeCode Opus へ引き継ぐ。

ストリップ右端 overlay／動画空欄・波形の余白色／V 共通 consumer は実装・独立 Sol 検収済み。V は同じ原神動画で失敗時 generic、再open後 native の入口差を実ログで確認し、正規の各入口を現在の動画 context に属する共通処理へ統合した。入力 producer や focus を強制変更していない。証拠は `target/next-version-work/video-v-key-20260909/investigation.md`。

検証正本は `target/next-version-work/logs/strip-v-verification-ledger-20260909.md`。full gate 初回は stale snapshot 2テスト失敗（8枚）。独立目視承認の期待画像更新後に対象2テストが通常比較で成功し、変更のない他targetの成功を再利用する。初回scriptが成功したとは扱わない。最新確認用buildはアプリ使用中のため未実行だったので、再開時にexact-path preflightとsource照合後に作成する。

担当は next_background_design が確認用build・Cargoを一元所有し、その後mouse seekの調査／実装を引き継ぐ。next_independent_review は別担当として新しい重要設計・完成差分のみを検収する。既承認strip/Vを最初から再レビューしない。親は本台帳と範囲管理を所有する。実アプリを起動・操作する新たな検証了承はまだ受領していない。

## 利用者による V 確認と mouse seek 試用再公開

利用者が BF51ED… 確認用buildで V が動作し、今のところ無効になる症状は再現しないと確認した。長期の全条件で再現しないことまで断定しない。

利用者から「シークの選択肢がなく試しづらいので戻してほしい」と明示依頼を受領。これを受け、従来の「fresh traceとroot fix前は候補を非公開」という順序を今回の確認用buildについて変更し、動画マウス割り当ての小・中・大の前後6候補と実行を整合して再公開する。二重入力／長押しの根因は未解決であり、試用可能化を修正完了・出荷検収済みとは扱わない。保存schemaやシーク秒数、非対象操作は維持する。

実装担当 next_background_design が候補とshared resolver、関連tests、mouse計画を一元更新。別担当 next_independent_review が限定差分を検収する。自動チェック後に確認用buildを作成するが、稼働中アプリは停止せず終了待ちとする。実機自動操作の了承は未受領。

試用再公開の実装・独立レビュー・全体gateを完了。`test-full.ps1 -SuppressCrashDialogs` exit 0（lib 7683 passed / 0 failed / 34 ignored）。exact-path preflight CLEAR後 `build-dev.ps1` exit 0、アプリ起動・停止なし。core SHA256 `F4385E13ADC8D27FCCFEE84E074126143089C8AF6E1E030C6C077365CADC9699`。検証正本 `target/next-version-work/logs/mouse-seek-trial-verification-ledger-20260909.md`。二重入力・holdの根因解決と出荷判定は引き続き未完了。利用者自身の診断有効起動と実操作ログで次段へ進む。

## 標準マウスと AHK の実機切り分け

利用者のRival5はSteelSeriesでF23/F24へ変換し、AHK v2 SendMode EventのOneSendでBrowser_Back/Forwardを送っていた。別マウスもAHK共通XButton1/2定義を通るため、初回の別マウス試験は非AHK比較ではなかった。利用者がAHK Suspend・終了を試した後は、raw XButton DOWN/UP/DBLCLKに変わり、二重発動は解消したように見えると報告。約1.942秒のholdで押下receipt442に対するApp dispatch1回、解放receipt443が記録され、標準の押下状態は保持できている。ログ `target/next-version-work/mouse-seek-live/ahk-disabled.log` と observations.md。

利用者は自分のAHK特殊環境への対応を次版必須にしないと明示。標準マウスの一意なclickと長押し連続シークを優先する。AHKのVK+AppCommand二重通知は互換性未解決として保持し、今回の標準動作確認を全入力環境の根因解決とは扱わない。長押し設計は既存入力所有・cancel・context境界を確認し、独立設計レビュー後に実装する。

## 長押し実機で再不合格

DFE41E…buildについて利用者は1click/連打OK、holdは1回のみ、AHK終了でも同じと報告。稼働dev-runtimeをread-only確認、ディスク上core hash一致。fresh log `target/next-version-work/mouse-seek-live/hold-failed.log`：100.290s receipt136 press_mouse_back_routed、102.211s receipt137 release_unmatched。押下/解放ともAppまで到達しており、長押しstateの未登録または解除を切り分ける。前回fullgate/review/build成功は保持するが、長押し実機合格・修正完了とは扱わない。実装者と独立reviewerが実際のproducer/context/validityとテストfixture前提を再検証する。アプリは操作・停止しない。

根因は独立reviewでも確認した。NativeVideoOutputのcommitted_generationは古いcloseイベントを拒否する下限（初期0）であり、初回のrender/inputイベント世代1と一致する値ではない。hold_validだけが一致を要求してarmを拒否していた。テストfixtureも同じ下限値からイベントownerを人工生成して差を隠していた。既存のcontext/source_epoch/HWND等の厳密照合を維持し、世代は既存closeと同じ下限判定（下回る場合を拒否）へ合わせる設計に親・独立reviewerが合意。初回gen1/floor0・同値・古い世代を回帰する。ライフサイクル制限の撤去や一時的なタイマー追加は行わない。

## 修正版の長押し実機確認成功

利用者がC586F409…buildで長押しリピート成功を確認。通常ログを `target/next-version-work/mouse-seek-live/hold-success.log` へread-only保存した。標準raw Extra1/2のhold4回（press receipt22/34/36/38）にそれぞれ22/40/43/22回のapp_mouse_hold_repeatがあり、解放receipt23/35/37/39は全てrelease_hold。各最後の反復は解放より前（19.850<19.859、26.030<26.059、28.840<28.859、30.622<30.641秒）、同pressの解放後反復はない。両方向の中シークを確認した。前回のrelease_unmatchedは今回記録にない。全サイズ・全窓・全cancel経路をこの実機logだけで合格とは扱わず、既存の自動回帰と合わせて記録する。

利用者の追加報告：AHK Suspendでは二重入力が残り、完全終了が必要だった。これはユーザー環境の観測として残し、Suspend全般の仕様と断定しない。AHKのhook/Browser VK/AppCommand互換は別課題、次版の必須対応外という合意を維持。エージェントはアプリ起動・停止・入力操作を行っていない。

## 波形ストリップ描画の出荷前修正

利用者が明るい余白色で波形の黒い塗り・表示範囲文字の重なりを報告。初めは時間範囲内のみ旧配色へ戻す案だったが、利用者が範囲外も旧表示へ戻すことを許容したため、3.7.1では動画波形ストリップ全体を背景色拡張前の暗色下地・配色・描画に戻す方針を採用。サムネイルストリップ、動画letterbox、静止画の背景色対応は維持する。全ファイル巻き戻しは行わずwave限定の変更を実装担当が所有し、旧表示と根因の照合・独立review・必要回帰・確認buildを行う。文字重なりが局所復元で解消するかも確認する。

ClaudeCodeの公開準備は利用者が保留すると回答済み。公開作業自体は引き続きClaudeCode担当。今回の製品修正のみCodex側で扱う。アプリ操作の了承はなく、既存の通常profile起動禁止・使用中アプリ停止禁止を維持する。

波形の根因は、透明foreground化した旧スペクトル色textureへ明背景用BLACK tintを掛け、色を黒に乗算したことと独立確認した。wave modeのみ旧opaque raster/暗色下地/WHITE tint/旧範囲文字とmarker配色へ戻す。サムネイルmodeのcanvas対応は維持。描画入力・解析・cache等の変更は不要。実装担当と独立reviewがこの境界へ構造合意した。

ClaudeCodeから利用者経由で共有された整合指摘も本sliceに含める：fullscreen.htmlの波形余白色説明除外、settings.htmlの検証版暫定注記削除/整合、preferences/pages.rsヒントの存在しないページ名『画像と本』を実際の『表示 › 閲覧表示』へ修正。実装者が現状を照合し3ファイルを所有、独立review対象に含める。更新履歴はClaudeCodeが所有し、本担当は変更しない。Phase1以降は利用者確認用buildまで保留のまま。

## 公開目標を v3.8.0 へ変更（2026-09-10）

利用者判断で今晩の3.7.1公開を取りやめ、dupe worktreeの類似本検索を統合したv3.8.0を9/10夜の公開目標とする。波形バグ修正が完了した後、dupe側セッションがマージを担当する。この担当は波形修正・検証・確認buildと開発証拠の引渡しまでを行い、dupeのマージや版本・更新履歴の公開用編集を勝手に行わない。

利用者は9/10昼の外出中に実機テストを進める意向。時間帯とPCの対話desktop可用性を質問中。実行suite・見込み時間・再試行範囲・使い捨てportableデータ範囲を統合後対象sourceで具体化し、明示了承後にのみ起動・前面変更・入力を行う。外出予定の申告だけを実アプリ操作の許可とは扱わない。

## 9/10 日中の実機検証枠

利用者は9:30〜18:30に外出し、その間PC使用可、ログイン済み・ロックなし、Codex Remoteで会話可能と回答した。具体的suite/再試行/隔離scopeへの確認は別途準備後に行う。9:30の一回限り再開確認をCodex heartbeat `v3-8-0` に登録済み（統合状況と了承済みscopeを確認、未了承のUI操作はしない）。18:30までに入力/後片付けを終了する。予約は具体suiteの実行承認を代替しない。

波形dark復旧sliceは独立review承認・全体gate成功（lib7697、ui_snapshot45等全target）・通常featureのbuild-dev成功。core SHA256 `25B7A608F09CEBCE56458B1FF0B9581B92317F8BC8DE753814F81322B85BC36C`、build前後manifest10/10一致、app起動/停止なし。最終ledger `target/next-version-work/logs/waveform-dark-rollback-verification-ledger-20260910.md`。利用者の最終表示確認は未実施。dupe側へはこの差分と証拠を含めて統合を引き渡す。公開用version/changelogは未変更。

## 動画stripの追加実機不合格（2026-09-10）

利用者の25B7A608…buildとv3.5.0の比較で、thumbnailの間隔文字とwaveformの範囲文字が二重に見えることを確認。waveform配色はOKだが、前sliceの自動gate/レビューはこの文字退行を検出できていなかった。利用者は動画thumbnail背景も旧暗色へ戻すことを要求したため、動画seek stripの両modeを旧暗色配色へ統一する。映像本体のletterboxと静止画stripの設定色は維持する。文字はshadow/foregroundの実際の描画色と旧実装を照合して根因を修正し、その層の回帰を追加する。実装・検証owner next_background_design、独立review next_independent_review、親は本台帳のみ編集。公開/dupe統合への完了引渡しは再修正後とし、未確認を完了扱いにしない。

文字二重の根因を独立reviewerが確認：foreground色でlayout済みのGalleyを2回Painter::galleyへ渡し、shadow色をfallback_colorだけで変更していた。egui 0.33.3ではnon-placeholder glyphの既定色が優先し、両shapeが同色で1pxずれて描かれる。v3.5.0はshadow black220とforeground gray232を別生成していた。正しいoverride色指定または旧2-layoutへ直し、shape数やfallback値だけではなく実glyph/mesh色の回帰で検証する設計に親・reviewerが合意。dev-runtime core/remote稼働中を検知、利用者に終了連絡を依頼済み。build-devの自動停止は使用しない。

## dupe統合のタスク間委任（2026-09-10 夜間）

利用者は就寝後も、今回の修正完了後に本タスクからdupeタスクへマージ作業を指示し、local masterへ統合するところまで進めることを明示依頼した。最新stripの手動表示確認は未実施として残し、自動gate/独立review/build後にコミットと統合を進める。公開は委任対象外。dupeタスク「引き継ぎ状況を整理」（01a07bf1-8a31-79d3-bfae-373d1cc4ccc3）へread-only準備を依頼済み、最終handoffまでmerge/editは禁止と調整した。

dupe HEAD cfb849e361e321d0e52878d9ec0ed3ffece20743には未コミット完成差分が残るため、HEAD単独のmergeでは不完全。dupe側が固定master受領後に自身の差分を保存・master取込・競合解決と独立review・統合gate・local master反映を所有する。本タスクはmaster次版差分を依存が閉じる単位でcommitし、初期からあった別所有dirtyを保持し、残dirty一覧を引き渡す。双方が同時にmasterを編集しない。夜間UI入力の許可はなく、09:30以降も具体suiteの別途了承が必要。

動画strip両mode暗色/文字二重の追加修正を完了。独立review承認、focused回帰・core check・fmt・glyph・diff-check成功、全体gate lib7696 pass/34 ignored・ui_snapshot45・vendor等全target成功（wrapper exit0）。8-file manifestはgate/build後も一致。通常features build-dev成功、core SHA256 `725077E1B938E3E9DD2DB428BDFBEAD9B6580EF4837D43A228504C113D003751`。証拠 `target/next-version-work/logs/video-seek-strip-dark-verification-ledger-20260910.md`。最新実機表示のみ未確認。利用者の夜間統合依頼に従い、依存の閉じる製品差分をcommitしてdupe統合担当へ引き渡す。開発writer終了後はdupeへmaster編集権を渡す。

統合後は利用者依頼により実機テスト自動化実装を継続する。次候補は既存S3b上HUD hover入口/native_top_panoramaの実描画target観測をhover-only scenarioへ通す範囲。調査はread-onlyで完了し、merge後のsourceで前提を確認して実装・独立reviewを行う。クリック/zoom/pan成功をこの段階だけで主張しない。8時頃までは利用者就寝、実アプリの対話実行は引き続き具体suite了承待ち。

## v3.8.0 dupe統合完了と自動検証の継続

2026-09-10夜間、dupeタスクの統合を完了し、master `426df75b9ee9e258139a6179ec8bcca6665dbfcd`（parents b003e6494 + dupe 3110c040b）となった。master treeは全体gateを通過したdupe treeと同一。dupe側の独立Sol検収・全体gate成功（lib8029 pass/43 ignored、UI48、vendor25/9/15等）。初回gateの1失敗はcurrent page/anchorのfixture不一致を実routeへ適応して修正し、独立再reviewと全体再実行で成功。初回失敗記録も保持。

masterで通常build-dev成功、core SHA256 `4087876E36EBA81821542B8EFFA9C513D28A5C2868F84B9A7B0C5B168C837703`。親もHEAD/hashを照合した。正本 `target/next-version-work/logs/duplicate-integration-final-manifest-20260910.json`。詳細 `docs/duplicate-detection-integration-v380-20260910.md`。アプリの起動/停止/input/push/publicationなし。最新strip実機表示・統合実機suiteは未検証のまま。

別所有dirtyは保持された。AGENTS/README/release-operationsはmixed EOLからCRLFへのraw差のみで、semantic内容とdirtyを独立reviewで確認し記録した。development-build-and-testの元追記はincoming vendor節へunion、briefは元byte同一でtrackedへ昇格。他untrackedも保持。元raw復元が可能だった4pathは復元。これを製品機能の変更と混同しない。

dupeからmaster編集/Cargo権を返却後、ユーザー依頼のS3b自動テスト実装を開始。実装/test owner smoke_next_plan（Sol xhigh）、独立review next_independent_review（別Sol xhigh）。上HUD hover入口とnative_top_panorama実Responseのtarget観測をhover-only scenarioへ通す。typed targetの座標契約、final pass/present成功公開、ctor/resize/source epoch/owner寿命の境界を設計合意に含める。クリック/wheel/pan等は本sliceに含めない。既存scripts/ui-smoke.ps1のidle198差分を保持する。実機実行は別途了承待ち、通常確認buildは診断onlyの範囲ならそのまま保持する。

日中実機suiteの具体了承をasyncで依頼済み（回答待ち）。9/10 09:30〜18:30内、実装/review完了後の30〜60分程度、MultiWindowPdf / StillStripDrag / NativeMouseMove / NativeTopPanoramaHover / Idle198Convergence。前面windowとmouse、生成素材とtarget/portable-smoke/dataのみ、各項目初回+原因確認後の再試行1回まで、18:30までに後片付け終了。通常profile/dataは使わない。時間枠の事前了承のみをsuite実行許可へ読み替えず、返答までは実行しない。

S3b hover-only実装を完了。独立Solの最終検収は重要指摘なし。正本 `target/next-version-work/logs/s3b-native-top-panorama-hover-final-manifest-20260910.json` のSHA256 `CF346A74B61F667FCDF715E1D484789C215758601702043FFB73DE529DB1CACD` を承認対象として固定した（JSON内の承認欄は承認前snapshotのためpending表記、最終承認結果は本記録とreviewer引渡しを参照）。非test feature check、native_ui_smoke39件、top_hover8件、関連回帰、PS5.1/7 parser・approval guard、fmt/diffを通過。default full gateは8029/43ignored・UI48・vendor25/9/15成功後の追加修正がtest-script cfg/tests/docsだけで通常active code/default tests不変のため、独立review承認付きで再利用した。

最終のphase契約：BeforeInputはhidden観測/版の完全一致、ReceiptCompletionは入力後presentの版進行とResponse存在、enabledは後続観測。Canvasは独自geometry/ownerを維持して無関係なchrome版へ依存しない。準備前の曖昧候補をready filteringで隠さず、owner/source変化・pre-input show/hide往復・hiddenのままのreceiptを拒否する。touch help/normalize scanningも操作不能として観測する。

診断portable SHA256 `2837643A6E8F3913BECCA0F3413664F5969531EAB19BAE45346D5B46853A86EE`、fingerprint `00a73a21d188762799add761b04f08bca452a69d0dcb271b14dd8021efb4f2fa` を準備済み。通常統合build `4087876E...` は保持。アプリ/UI/SendInputは実行していない。NativeTopPanoramaHoverはlive pendingであり、クリック・zoom・panやthumbnail pixel出力の検証済みとは扱わない。

コミットはS3b candidate10pathと親台帳を対象にする。ui-smoke.ps1は既存idle198差分を残し、`target/next-version-work/logs/s3b-ui-smoke-isolation/ui-smoke-s3b-only-index-ready.patch`（SHA E5EDBDEEF71A726DA4508179A02667CD85C325C89E88E67E28DC114346B4849B）だけをindexへ適用する。既存dirtyを全addしない。実機suiteの具体了承は引き続き回答待ち。

## 利用者確認と通常版の索引作成中の並行検証

利用者が統合確認buildの動画stripの色は大丈夫と確認。類似本検索は通常APPDATA版で索引作成中。通常版を動かしたまま別data-dirで実機検証可能かとの問い合わせを受け、single_instanceのdata-dir別mutex/activation/pipe分離とdiagnostic portable経路をコード確認した。使用するのは既定のtarget/portable-smoke/mimageviewer.exeと専用dataであり、通常版をエージェントが起動/停止したり通常profileを試験へ転用しない。build-portable -SmokeTestScriptは通常processの停止blockを通らず、runnerの終了処理は自身が起動したProcessだけを対象とする。機能テストの並行実行は可能だがCPU/GPU/diskは共有するため索引作成時間と試験時間に影響し得る。性能/idle計測の合否は索引作成の並行負荷と区別する（できれば索引完了後に実施）。実機suiteの実行了承は引き続き別途確認中。

## 索引作成中の類似パネルに前の画像が残る報告（2026-09-10）

通常APPDATA版で索引作成中、test表紙/00表紙.jpgへ移動した後も右パネルの「表示中（更新中）」と候補が前のChatGPT Image PNGのまま残ると利用者から報告。アプリを操作・停止せず、ログをtarget/next-version-work/similar-stale-panel-20260910へ読み取りコピーして保全した。mimageviewer.logでは新画像のDisplayReadyとitems_generation更新を確認。perf_eventsの新画像へのsimilar_item照会seq260はactive=true/stale=false、terminal=not_indexed、wall_ms=0.7877で完了しており、重い照会が未完了のままという説明ではない。索引snapshot未反映と別originの結果保持を分けて調査する。

調査担当similar_stale_investigation（Sol/xhigh）はread-onlyの原因・影響範囲・回帰案を担当。製品コード変更・Cargo・実アプリ入力は未実施。正常な索引作成は継続させる。設計§20.7のorigin key + memory epochによる再利用とComplete snapshot利用は、別画像を現在表示中として残す仕様ではない。

原因をsourceと照合した。src/ui_metadata_panel.rsのSimilarPanelState.last_readyはslotごとの旧Readyを保持し、shown_queries生成でPreparingならorigin identity照合なしに旧Readyへ置換する。src/similar_index.rsのquery_itemは索引Running中のNotIndexedをPreparingとしてUIへ返す。この組合せにより、新画像の未索引応答が返っていても前画像の「表示中」と候補が残る。修正すべき境界はUIの結果保持であり、同じ画像の索引更新待ちだけ旧結果を使い、別画像への切替では旧originと候補を退役させる。見開きの右頁だけ変更にも対応するため、先頭origin変更時の一括clearではなく各ページidentityとの対応が必要。コード修正・回帰・確認buildは未実施。

利用者は類似パネルの修正を依頼し、その後9:30頃から実機テストを進めるよう指定した。実装・非対話検証をsimilar_stale_investigation、独立設計/実装検収をnext_independent_review、実機準備をsmoke_next_planへ委任した。修正はUI結果保持のorigin ownershipへ限定。通常版索引作成は継続し、build-devの自動停止は禁止。

実機予約更新は自動承認審査が「開始時刻の了承のみではexact suite/時間/再試行/隔離scopeの了承不足」として拒否。利用者へ理由を説明し、既提示5scenario（30〜60分、9:30〜18:30、初回+原因確認後再試行1回、生成素材とportable-smoke/data、通常版保持、並行負荷時idle延期）の明示確認をasyncで再送した。返答までは実機操作を行わず、非対話準備を続ける。既存heartbeatは更新されていない。

独立reviewerと親が設計合意。Ready限定のorigin-key所有へ変更し、全current page_keysでkey lookup/reconcileする。Ready内部originと保持key一致、非Ready保持禁止、現在表示にないkey退役、各頁terminalはそのkeyのみ退役、同一origin Refreshingは従来Preparing相当100ms pollを維持。古いカード/候補/操作は描画投影から消すがthumbnail/preview cacheやbook clientの一括resetは行わない。manager/DB/detached runtimeへの変更は不要。

利用者が具体suite再確認後に「この後進めてもらって大丈夫です。記載の実機検証・テスト、進めてください」と明示了承。承認範囲は上述の5scenario/時間枠/所要時間/再試行/隔離data/通常版保持。heartbeat v3-8-0更新も自動審査を通過した。実装のreview/gate→最新portable準備→09:30以降の実機、という順序で専任担当へ引き渡す。承認の阻害は解消。

修正の実装と独立最終検収を完了（重要指摘なし）。source ui_metadata_panel.rs SHA256 E5FDEB11B1D367642A581906494EB0F15117031AA4E4D743A19E198D45F15800、feedback doc SHA256 292154BA7A9E79CBEEBE22B4E11119E594B8B5A4A34DDC6B240B6FFEB1EF5540 を親も照合。focused36 pass/1 ignored、normal core check/fmt/glyph/diff成功、test-full -SuppressCrashDialogs exit0/PASS（本体8034 pass/43 ignored、既存similar snapshotとvendor25/9/15等成功）。初回repaint fixture失敗は初期settling frameを消費していない観測不備で、製品コード不変のまま実frame順へ修正して再検証、独立検収で妥当と確認。証拠target/next-version-work/similar-stale-panel-20260910/verification-summary.txt。通常版と索引は操作せず、使用中のdev-runtimeを上書きするbuild-devは保留。最新sourceから隔離portableを専任担当がbuildし、承認済み実機へ進める。

修正をlocal master 9bb2072b817aad7bbe969bde0d76569e3214e073へコミット。stage対象は製品source/feedback/親台帳の3pathのみで、別所有dirtyを保持。通常core hash4087876E36EBA81821542B8EFFA9C513D28A5C2868F84B9A7B0C5B168C837703は不変。実装/reviewerが終了しCargoを返却後、smoke_next_planへ最新portable buildと承認済み実機suiteの所有を引き渡した。09:15の通常ログにsimilar array updateの継続あり、現時点でidle計測の並行負荷なしとは扱わない。

09:27に最新diagnostic portable準備完了（起動前）。HEAD 9bb2072b817aad7bbe969bde0d76569e3214e073、source fingerprint 1a97c79d1ade164eea30802fd3c063bd9b9ae94131a96c27bf463c9ff50c6d8e、exe SHA256 348B947C2DBFEAD87BD0142CBA1BA05EDFD6EC6E8CD706952229B37F218DE7D9、manifest SHA256 2560924B24DD380914CA6FB7266A55C08F198FAB56AB3EFD8308D12BD373587F。親もhash/manifest内容を照合。build理由は製品source byte変更で、HEAD表記差による再buildではない。通常process非停止、使い捨てdataのmarkerとtest-script buildを確認後09:30から承認済み機能suiteへ進む。

09:30〜09:42の承認済みsuiteは環境不成立で未完了。MultiWindowPdf2回/StillStripDrag2回は最初のPDF別窓描画後、select_root→GridMove*がAwaitingPassで待ち続けtimeout。NativeMouseMove2回/NativeTopPanoramaHover1回はtarget ready後、selected detachedがforegroundでないとしてEnvironmentFailure。7runとも実MouseMove/drag送信前に停止し、7個の起動portable PIDは全て終了確認。Idle198は0回（索引負荷と入力desktop不成立）。証拠target/next-version-work/logs/ui-smoke-live-suite-20260910.json SHA256 BF53AB21B817D42A2BB507E8DD2A57B345E337955CAD3EFFC8CDB256289BE908。

read-only probeでsessionは共に1だが、runner thread desktopがCodexSandboxDesktop-ec2638a1d535cbc058726dec0c80d4d4、active input desktopがDefault、foreground HWND0/PID0と判明。通常版のロック状態や製品機能の故障とは判定しない。実装調査もFocus commandの消失を否定し、OS/backend focus成立イベントが来ないため通常focus guardがaction ackを許可しなかったことを確認。診断Rootがfocus成立をtyped状態で待たず外側120s timeoutになる改善余地はあるが、PDF/detached/product修正で対処しない。UI追加実行は停止。通常の承認付き実行経路でread-only desktop probeが成立するかを次に確認し、desktop切替やlock解除の迂回はしない。

09:52、正規のexec_command require_escalatedによる同一read-only probeが自動審査を通過し、thread/input desktop共にDefault、foreground HWND0x20944/PID12324、session1を確認。環境の変更・desktop切替・アプリ操作はしていない。default execの隔離desktopと承認付きexecの実行先差が今回の不成立原因。製品修正や入力guard迂回は不要。probe証拠target/next-version-work/logs/ui-smoke-approved-read-only-desktop-probe-20260910.json、再利用script ui-smoke-read-only-desktop-probe.ps1 SHA4359CEF94069714FD4BD2265D356B8B56290F31D9D3B6C60A72A96392A76A5F5。既存4scenarioを各1回追加（10〜20分、同じ日中枠・隔離scope・通常版維持）する具体確認をasyncで依頼済み。前回7runを遡及的に無効扱いして上限を回避せず、追加了承待ち。最新source/artifactは不変で再build不要。

10:15、利用者が追加4項目各1回の具体確認に「追加実行okです。進めてください」と明示了承。MultiWindowPdf/StillStripDrag/NativeMouseMove/NativeTopPanoramaHoverを各1回、10〜20分程度、既存9/10 18:30までの枠・隔離scope・通常版維持で再開。正規の承認付きexecでread-only desktop一致確認後、その経路でrunnerを実行する。実機owner smoke_next_plan、親は台帳のみ。前回7runを保持し別manifestへ集約、追加再試行や製品変更はしない。現HEAD b133b301bは文書のみの追加で製品sourceは9bb2072b8と同一、親がexe/manifest hash不変を再照合したため再build不要。Idle198は索引負荷pendingを維持。

10:16〜10:18の追加Default desktop suiteは4/4 PASS（各1回、failure/timeout0）。MultiWindowPdfはsemantic KeyAction配送で2窓PDF分離・片方page/paint/closeとsibling維持を確認。StillStripDragは実アプリへのsynthetic egui pointerでLTR/RTL移動と描画/効果/sibling不変を確認。NativeMouseMoveは実Windows MouseMove2件がexact owner/presenter/source/generationでpump/renderへ届き座標一致。NativeTopPanoramaHoverは実MouseMove1件で上HUDを出し実enabled Response/rect/interact/clip/DPIを観測（click/wheel/key/panは範囲外）。起動PID51816/65448/56248/69956は全終了、親も0processを確認。通常core/source hash不変、通常版とデータ操作なし。最新通常log10:19:48でもsimilar array update継続のためIdle198は未実施。最終manifest target/next-version-work/logs/ui-smoke-default-desktop-suite-20260910.json SHA387A045F05C538A8E4BC2D8A3E2ECA374DDC41C074CC384373408F5D6964CCE6を親も照合。前回環境不成立7runの証拠は不変で保持。再build/Cargo/製品修正は行わず、実機所有は親へ返却された。

同日、利用者は表紙・裏表紙見開きの開発をdupe側タスクへ指示し、本タスクでは残り自動テスト開発を進めるよう依頼。親は既存タスク「引き継ぎ状況を整理」（01a07bf1-8a31-79d3-bfae-373d1cc4ccc3）へ、最新masterからの独立branch・役割付き描画構成・navigation不変・未確定操作scopeを引き継いだ。同タスクから受領と設計開始の返信あり。未完成の見開き機能は今夜のv3.8.0へ混ぜず、master merge/公開は別判断とする。master側はsmoke_next_planがS3b実装・非対話検証を所有し、next_independent_reviewが独立Sol/xhigh検収、親は台帳と範囲調整を担当する。既存の設計・fake試験・live証拠を再利用し、button clickからzoom/pan実適用確認へ進む。新scenarioのlive実行は未了承で、通常版・索引処理を維持する。両worktreeの重いbuild/GPU/実機は実行前に所有調整する。

表紙・裏表紙機能の追加回答は、全体設定の既定ON、本ごとの記憶、連結読みとRemoteも初版対象。親は本overrideを「全体に従う/ON/OFF」とする案を返答し、dupeへ回答を転送した。dupeはcodex/final-cover-spreadで共通role/需要/設定resolverを設計検収し、Phase Aに着手。nav位置を維持する共通構成、連結の同一ページ複数occurrence、Remote group/slot明示指定、設定保存時のoverride保持を検収対象とする。masterがC#/PowerShell作業中のため、dupeへjobs1のPhase A cargo check/狭域試験枠を引渡し、返却前はmaster Cargoを開始しない。

S3bはまず外部button helperの独立checkpointを先に保存する分割とした。ignored草案はtarget/v370-work/s3-button-helper-draftとs3-button-draftに現存し、親が所在を確認して新規全面実装を避けた。既検収reducer/backendを再利用し、未検収host層のtransport failure・App終了・EOF・reply fault・固定join期限・未解放時のApp終了禁止を補正・回帰確認した。実装ownerよりPS5.1/7各16 reducer+52 helper PASS、manifest target/next-version-work/logs/button-helper-host-checkpoint-20260910/manifest.json（SHA B4219B4B43F7166A3BC7FF024213545352B5B85453440420750F8E302EC0A2A6）の引渡し。親は17sourceと3evidenceのhash一致を確認。最初のsandbox pipe ACL例外は環境制約として保持し、正規の承認付き非UI実行で検証。実入力/inserter構築なし、Rust/App/runner/scenarioは未接続。独立検収中。script-only単位のためCargo full/通常確認buildは不要で、クリックやzoom/pan自動確認の完了とは扱わない。

host checkpointの独立Sol最終検収を完了。指摘2件は、実NamedPipeのterminal write failureからCancelAndJoinがGestureFailedへ再分類するend-to-end回帰と、StillRunningのReleaseState=Unknown/SafeToTerminate=falseへの型分離で解消。joinedはNoOwnedDown/ConfirmedReleased/ConfirmedOutstandingを区別する。再検証はPS5.1/7各16 reducer+54 helper PASS。最終manifest SHA6743396E4B0C3AA6FC928F8DE5999954CCC619ED8BA4C2BA65A467F5E957C30Bを親が確認しsource/evidence hash不一致0。既知指摘と影響範囲に絞って再検収、重要未解決なし。後続Rust/App/runner接続ではApp timeout/非0終了のprimary reason保持とgesture/stepごとのexact receiptを引き続き必須とする。通常profile/実入力は操作していない。
