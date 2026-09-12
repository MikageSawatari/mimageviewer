# 次リリース検討バックログ

このファイルは、**いま着手できる**作業候補だけを置く恒久バックログ。
完了した項目はコミット履歴・リリースノート・個別設計メモに任せ、このファイルからは削除する。

判断待ち・再現待ち・見送りは [backlog-on-hold.md](backlog-on-hold.md) へ分けてある。
着手できないものをここに混ぜると、次に手を付けるものを探しにくくなるため。

運用ルール:

- 着手前に `docs/README.md` から該当領域の設計ドキュメントを読む。
- 着手中のものだけ `対応中` と明記してよい。完了したらこのファイルから削除する。
- 着手できなくなったら (再現しない / 利用者の返答待ち / 見送り判断)、節ごと
  [backlog-on-hold.md](backlog-on-hold.md) へ移す。番号は変えない。
- 判断保留・見送りの理由は、次に再判断する人が困らない最小限だけ残す。
- **節番号は既存の最大値の次を使う**。他のドキュメントやコミットから §番号で参照されるので、
  重複させると照合できなくなる (2026-09-03 に §1.169 / §1.170 で実際に起きた)。
- **§1.200 は欠番**。2026-09-08 に 2 つの節が同じ番号を取ってしまい、2026-09-12 に
  それぞれ **§1.224** (一時表示ナビゲーターの未開始クリック) と **§1.225**
  (詳細表示からの静止画シーク・画質設定) へ振り直した。**1.200 は再利用しない。**
  v3.7.0 期の作業台帳など当時の記録に残る §1.200 は、原則 §1.225 を指す。
- **設計の正本を個別の plan へ移した項目も、バックログのどちらかに節を残す**。作業候補の
  一覧はこのファイルと [backlog-on-hold.md](backlog-on-hold.md) の 2 つだけで見ているので、
  plan にしか無い項目は存在ごと見落とす。plan が正本のときは、ここには現状と残りだけを
  短く書いて plan へリンクする。
- 依存ライブラリ更新は `CLAUDE.md` のリリース手順チェックリスト Phase 2 と整合させる。

---

## 1. 優先候補

2026-09-12指定の直列実装順は [v3.9.0後の開発計画](post-v3.9.0-sequential-work.md) を参照。
§1.115 → PDFium/FFmpeg → §1.226（追加指定）→ §1.223 → §1.222 → §1.218 → §1.212 → §1.164 → §1.221 → §1.220。
以下の節の配置順は着手順ではない。
同日の追加判断で、明日9月13日公開予定のv3.9.1は§1.226と完了済み依存更新で区切る。
§1.223以降の追加項目は後続版へ延期し、元の直列順を維持する。

### 1.226 初訪問時の旧XMPタグ自動取り込みを廃止し、復元ダイアログの表示を正す — 利用者報告 (2026-09-12)

- v3.9.0、新しく生成した通常画像フォルダの初訪問で復元ダイアログが一瞬出る。再訪問では出ない。
  例: `G:\home\comfyui\202609-21\_ランウェイのモデル`。
- 現行は確認開始100ms後からモーダルを表示し、見出しは全phaseで「サイドカーから設定を復元中」固定。
  本文が「サイドカーを確認中」でも、復元対象が存在すると誤解させる。利用者は次版改善を希望。
- 方針: 存在・同期状態の確認と実際の復元を見出しでも区別する。確認だけで復元実行と伝えない。
  待機中の非同期処理・操作保護・実データ保護は維持し、単に待ち時間を隠すための閾値変更はしない。
- 利用者は未移行の旧v1.0 XMPタグを今後救済しないことを了承。全入口の自動legacy seedとその待ちを
  撤去する。現在のタグDB、mimageviewer.dat、削除済みタグの復活防止台帳tag_item_state、一般XMP処理は維持。
  手動旧XMP取込UIは8/30に既に削除されており、再導入しない。起動時一度きりの旧Tantivy移行は
  日常フォルダ訪問コストではないため今回の撤去範囲に含めない。
- ログ照合: 初回の待機922ms中に旧XMPタグseedが300件を読み、300件をタグなしとして決定済みにした。
  Quiescingはこのworker完了を待つ。完了後約22msでサイドカー確認等が終わり操作可能になっている。
  再訪は25ms/16ms。初回は旧タグ確認、再訪は決定済み記録で読み取りを省略する差であり、
  代表サムネイル選定やサイドカー存在チェックだけの0.9秒ではない。
  実際のサイドカー取り込み有無は現成功ログだけでは断定できない。
- v3.9.1対象・対応中。実装・対象回帰・全体gate・確認用build完了。利用者の操作感確認待ち。
  [調査記録](sidecar-first-visit-investigation-20260912.md)に時系列とコード境界を記載。

### 1.223 比較ワイプの開始時に境界を操作できることを示す — 利用者要望 (2026-09-12)

- `X`で比較対象をセットし、`Shift+C`でワイプ比較を開始した直後、既定の境界線がホバー時のみ
  表示されるため、境界を動かして比較できることに気づきにくい。
- 改善案: ワイプ比較へ入ったときは境界線・操作ハンドルを表示し、境界へ初めて意図的に
  ホバーするかドラッグ操作した後に、従来のホバー時のみの表示へ移す。
  「境界をドラッグして比較」の短い案内も候補。具体的な消去条件は実装前に確定する。
- 表示中の画像全体へのホバーだけでは案内を消さない。開始時に偶然カーソルが境界上にある場合も、
  最初から案内が見えなくならないよう扱う。ドラッグ中の線表示とCtrl併用時の線非表示は維持する。
- 同じ比較開始操作のキー・メニュー等で統一し、初期案内は各viewerの比較セッションに属させる。
  比較を終了して入り直したときは再表示する案を基本とし、別窓の操作で消えないようにする。
- 既存の画像合成、比較対象、境界位置、ドラッグの入力判定は変えない。
  境界線判定の現入口は `compare_wipe_line_visible`（`src/ui_fullscreen.rs`）。
- 確認: 比較開始時の発見しやすさ、初回ホバー/ドラッグ後の消去、再入場、複数窓、
  明暗画像での視認性、通常の比較操作の維持。
- 規模 / 優先度: Small想定 / P2。後続の利用者指定で次版の直列実装対象となった。

### 1.222 viewer context audit の CI が 2 件で落ちたまま — v3.9.0 公開時に残した (2026-09-12)

- 出典: v3.9.0 公開作業での CI 確認 (run `34677345883` / HEAD `5de2b6d4e`)。
  **`cargo fmt --check` と `cargo check (ubuntu / non-Windows cfg)` は緑**。v3.8.0 で残っていた
  `cfg(windows)` 漏れ 3 件と A6 誤検知 49 件は `9ce433709` で解消済み。残るのは次の 2 件だけ。
- **どちらも配布バイナリの挙動には影響しないので、v3.9.0 は赤のまま公開した** (利用者判断)。
  修正は `tools/viewer_context_audit/src/lib.rs` で、公開担当の範囲外として開発側へ差し戻す。

#### (1) A2b `src/app.rs` `resume_loading_items_after_sidecar`

> A2b src/app.rs:26713 resume_loading_items_after_sidecar: 7 distinct ViewerContextBundle fields
> occur in mem::swap/replace/take arguments: adjustment_page_params, comic_pages, conceal_pages,
> export_crop_page_settings, local_adjust_pages, mask_pages, view_trim_page_overrides

- **コードは変わっていない。** v3.8.0 では同じ 7 行が `start_loading_items_inner` の中にあり
  (v3.8.0 の `src/app.rs:26378`〜`26488`、関数は `25801` から)、その関数は A2b の
  `ALLOWLIST_ENTRIES` に登録済みだった。§1.209 で取り込みを UI スレッドから外すために
  `9c9df532c` が関数を 2 つへ割った結果、登録済みの後半が新しい関数名へ移り、
  (ファイル + 関数名) をキーにする監査の対象から外れた。
- 7 件はいずれも `self.<field> = std::mem::take(&mut metadata.<field>)` /
  `(&mut prepared.<field>)` で、受け側は prepared payload であって別の App や context ではない。
  既存 `start_loading_items_inner` の理由書き (prepared payload から mounted projection へ
  完了したロードを設置するもので、既存 context の抽出・移譲ではない) がそのまま当てはまる。
- 対応候補は allowlist への追加だが、**機械的に足さず**、分割後も同じ不変条件
  (取り込み完了までの世代整合、取消・フォルダ移動・別窓での所有境界) が保たれているかを
  検収してから登録する。分割前後で監査の網が緩まないかも見る。

#### (2) A4 `src/app/viewer_context_registry.rs:1166` `video_seek_strip_test_script_snapshot`

> A4 ... unexpected public API fingerprint: inherent fn #[cfg(windows)] <'a> ContextRef<'a> ::
> #[cfg(feature = "test-script")] pub(in crate::app) fn video_seek_strip_test_script_snapshot(self,)
> -> crate::test_script::TestScriptSeekStripSnapshot

- `095c82276` (動画 strip 寿命の S4 自動テスト) で追加された API。`#[cfg(windows)]` +
  `#[cfg(feature = "test-script")]` + `pub(in crate::app)`。
- **`test-script` feature は `build-portable.ps1 -SmokeTestScript` (使い捨て smoke 専用) でしか
  有効にならない**ので、`build-dist.ps1` が作る 4 成果物にはこの API は存在しない。
- A4 は「公開 API の意味を検収してから登録する」ルールなので、`PUBLIC_API_ALLOWLIST` への
  追加は snapshot が読み取り専用で、他 context の状態を触らないことを確認してから行う。

#### 関連 (同じ CI では落ちないが、ビルドで毎回出る警告)

- `clashing_extern_declarations`: `GetProcessMemoryInfo` が `src/similar_index.rs:6593` と
  `src/similar_db/full_inventory_benchmark_tests.rs:922` で、それぞれ `ProcessMemoryCounters` /
  `ProcessMemoryCountersEx` を取る別シグネチャとして宣言されている。**両方とも `cfg(test)` 配下**で、
  各呼び出しは自分の構造体と `cb` を渡しているので配布物には影響しない。宣言を 1 か所へ寄せると消える。

- 規模 / 優先度: Small / P2。CI を緑へ戻す。2 リリース連続で赤のまま出しているので、次版では先に片付ける。

### 1.221 右クリックメニューの mIV 項目を表示・並べ替えできるようにする — >>378 (2026-09-12)

- 出典: >>378。利用者から、右クリックメニューの項目を編集したいとの要望。
- 対象は、グリッドとフルスクリーンの共通モデル (`context_menu_model`) が生成する
  **mImageViewer側の項目**。環境設定に一覧を設け、項目ごとの表示ON/OFFと順序変更、
  既定へ戻す操作を用意する。既定値は現在の表示内容・順序を維持する。
- 同じ並び設定をグリッド / フルスクリーンで共有し、その場面で利用できない項目は従来どおり省く。
  表示対象を減らしても、キー割り当てや上部メニューなど別の入口は変更しない。
- `MenuCommand` の表示名ではなく、永続化用の安定IDで順序と非表示を保存する。新しい項目は
  既存利用者の並びを壊さず既定位置へ追加し、未知・廃止IDは安全に無視する。
- 登録済み外部ツール、関連付けアプリなどの動的な項目は、個々をこの画面で並べ替えず
  **グループ単位の固定枠**として扱う。個別の並びは各機能が現在持つ順序を使う。
- **Windows Shell側の項目は個別の識別・並べ替えをしない。** Shellが返す項目群は、現在の
  サブメニュー / 併記設定を保った1つの固定枠とし、内部の表示内容と順序はWindowsへ任せる。
- 実装時は [右クリックメニュー統一設計](context-menu-unification-plan.md) を正本として更新する。
  共通モデルを設定で解決してからnative Win32 / egui fallbackの両描画へ渡し、描画側ごとに
  フィルタや並べ替えを重複実装しない。非表示後の空サブメニューと不要な区切り線も正規化する。
- 回帰確認: 実項目 / 仮想項目 / 混在選択、グリッド / フルスクリーン、native / egui fallback、
  外部ツール、Windows項目のサブメニュー / 併記、設定round-trip、新規ID追加時の補完。
- 規模 / 優先度: Medium / P2。

### 1.220 切り取り中の実ファイル・フォルダを半透明で表示する — >>378 (2026-09-12)

- 出典: >>378。エクスプローラーのように、切り取り中の項目を一覧で半透明にしてほしいとの要望。
- Shellクリップボードが `CF_HDROP` と移動 (`Preferred DropEffect = MOVE`) を示している間、
  対象の実ファイル・実フォルダをサムネイル表示と詳細表示の両方で半透明にする。
  ZIP/PDF内ページなどの仮想項目、フルスクリーンの画像本体、実ファイル自体は変更しない。
- 半透明化してもカーソル、チェック、hover、選択枠などの操作状態は判別できるようにする。
  パス比較はWindowsの大文字・小文字差と表記揺れを吸収する正規化済み実パスで行う。
- mIVから切り取った直後だけのboolを各項目へ足さない。切り取り直後に共通のクリップボード状態を
  更新し、貼り付け完了、コピー / 別の切り取り、外部アプリによるクリップボード変更で解除する。
  Windowsのclipboard change通知を入口にし、描画フレームごとの同期clipboard読み取りは行わない。
- 複数ウィンドウでは同じシステムクリップボード状態を共有し、片方での切り取り・貼り付け・
  上書きを全グリッドへ反映する。移動後の一覧再読み込みと競合して古い項目を暗く残さない。
- 回帰確認: 単一 / 複数 / フォルダ混在、コピーでは暗くならない、外部clipboard上書き、
  貼り付け成功 / 失敗 / 中止、フォルダ移動後の再表示、複数ウィンドウ、詳細 / サムネイル表示。
- 規模 / 優先度: Small-Medium / P2。Windows clipboard通知と複数ウィンドウ共有まで含めて確認する。

### 1.218 見開きの先頭・末尾にある単ページを本来の側へ配置する — >>384-386 (2026-09-11)

- 出典: >>384 で、見開き時の末尾へ白・黒・任意色のページを添えて位置を揃えたいとの要望。
  >>385 で目的を確認し、>>386 で「単色画像は手段で、目的は位置を揃えること」と確定した。
  末尾だけでなく、単話などで中央表示される先頭の表紙も、本来の左右位置へ寄せたいとのこと。
- **既存の「末尾に表紙を添える」とは目的も表示内容も異なるため、別機能・別設定にする。**
  既存機能は表紙と裏表紙がつながった絵を見せるために先頭ページを末尾へ補助表示するもの。
  今回は画像や仮想ページを追加せず、先頭・末尾の単ページの配置だけを変える。
- 利用者向け仕様:
  - 見開き表示で、先頭または末尾の表示単位が1ページだけの場合、現在の開き方向と
    見開き規則から決まる側へ配置し、空いている側は現在の背景色のままにする。
  - 対象は本の先頭・末尾だけ。途中にある横長ページなど、単独表示になる通常ページは中央表示を維持する。
  - ページ数、ページ送り、シークバー、読書位置、ブックマーク、編集対象、サムネイル列は変えない。
    表示レイアウトだけの機能とする。
  - 1ページだけの本は先頭ページとして扱い、先頭側の配置規則を優先する。
- 設定:
  - 環境設定に「見開きの先頭・末尾の単ページを片側に配置」を追加する。既存挙動との互換性のため既定OFF。
  - 本ごとにも `全体設定に従う / 配置する / 中央表示` を保存できるようにする。
    既存の末尾表紙補助の本別設定とは混ぜない。
  - 末尾表紙補助によって実際の2ページ構成になった末尾には位置補正を重ねない。先頭側には独立して適用できる。
- 実装方針:
  - ダミーの `GridItem` やページ occurrence は作らない。偽のページを足すと、画像読み込み、PDF、AI処理、
    Remoteの要求や履歴へ波及するため、表示 composition / layout に `Center / Left / Right` 相当の
    型付き配置情報を持たせる。
  - ページ表示、縦連結、F12別ウィンドウ・複数ウィンドウ、mIV Remote が同じ判定を使うよう、
    見開き構成の共通境界で配置を決定する。
  - 本別設定は `spread.db` 系の既存ライフサイクルに合わせ、移動・名前変更・削除・メタデータ転送も確認する。
- 回帰確認:
  - 左開き / 右開き、先頭だけ / 末尾だけ / 両方、奇数 / 偶数ページ、1ページ本。
  - 途中の横長単ページは中央のままであること。
  - 末尾表紙補助ON/OFFとの併用と、ページ数・シーク・履歴・ブックマーク・編集対象が不変であること。
  - 通常ページ表示、縦連結、F12別ウィンドウ・複数ウィンドウ、mIV Remoteで配置が一致すること。
- 規模: **Medium**。画像読み込みは増えないが、表示経路を横断するためレイアウトの回帰確認が必要。優先度P2。

### 1.213 通常動画のナビゲーターを出せるか — 描画経路が静止画と違う (2026-09-11)

- 出典: 利用者要望 (2026-09-11)。v3.7.0 の通常動画ズームを使ってみて、静止画のナビゲーター
  (拡大中に全体と現在位置を示す小窓) が動画にも欲しくなった。「負荷が高くなりそうですかね？」
- **利用者が心配していた「OS 側で拡大する設定があるから重ねられないのでは」は当たらない (確認済み)。**
  ズーム中は OS 任せの設定が自動的に上書きされる:

  ```rust
  fn effective_video_scale_filter(filter: VideoScaleFilter, video_zoom_active: bool) -> VideoScaleFilter {
      if video_zoom_active && filter == VideoScaleFilter::OsDefault {
          VideoScaleFilter::Standard
  ```
  ([render_core.rs:2104](../src/video/native_presenter/render_core.rs:2104))

  サーフェスも表示領域全体を取る (`full_display_region = panorama_active || video_zoom_active`、
  [surface_policy.rs:177](../src/video/native_presenter/surface_policy.rs:177))。
  テスト `video_zoom_uses_the_full_display_region_even_with_os_default` あり。
  **ズーム中は必ず mIV のシェーダが表示領域いっぱいのサーフェスへ解決している。**

- **本当の論点は「どこに描くか」。**
  - **presenter (D3D11) 側**: フレームは既に手元にあるので、いま拡大表示で出している矩形の
    隣にもう 1 枚描くだけ。**毎フレームのコストは小さい見込み**。ただし静止画のナビは egui 実装
    (`fs_navigator_*` 群、[ui_fullscreen.rs:2356](../src/ui_fullscreen.rs:2356) 以降) なので、
    枠・ドラッグ・表示根拠 (Fixed / Hold) を**別実装で作り直す**ことになる
  - **egui overlay 側**: UI はそのまま使えるが、**動画のフレームは egui のテクスチャではない**。
    毎フレーム渡す経路が要る。**安くない**
- **未確認**: presenter 側に描いた場合の実コスト。上は構造からの見込みで、測っていない。
- **関連**: §1.224 (一時表示ナビゲーターの未開始クリックが消える) は静止画側の既知の不具合。
  動画へ広げるなら、その所有境界の整理を先に済ませたほうがよい。
- 規模 / 優先度: Medium / P3。**どちらに描くかを決めるまで見積もらない。**

### 1.212 通常動画をウィンドウより小さく縮小できない — 見送り判断の記録 (2026-09-11)

- 出典: 利用者要望 (2026-09-11)。静止画はウィンドウより小さくできるが、動画は画面に
  合わせた状態が下限になっている。「画像と同じようにできると不都合が出ますか」。
- **方針 (2026-09-11 利用者判断): 見送る。** 静止画と動きが揃うこと以外に用途を思いつかず、
  必要な場面が出てから判断する。**安いことと要ることは別**という整理。
- **ただし「複雑になるから」ではない。以下は確認済み。**
  - 止めているのは `VIDEO_ZOOM_MIN_SCALE = 1.0` ([zoom_view.rs:8](../src/video/zoom_view.rs:8))
    だけで、使用は 2 か所 ([:154](../src/video/zoom_view.rs:154) の clamp、
    [:244](../src/video/zoom_view.rs:244) の guard)
  - **映像より広い範囲を指して外側を黒で埋める経路は既にある。**doc に明記
    ([:57](../src/video/zoom_view.rs:57)): 「At fit scale one extent can exceed the encoded image
    and its origin becomes negative. Those coordinates deliberately describe letterbox pixels;
    the resolver returns black outside the source」。**等倍でも毎回この経路を通っている**
    (縦横比が違えば必ず letterbox になるため)
  - パンも既に正しい。`clamped_source_center` は `extent >= source_axis` のとき軸を中央固定
    ([:226](../src/video/zoom_view.rs:226))。縮小時に欲しい挙動そのもの
  - つまり作業は「下限定数を下げる」「下限値を決める」「リセット時の初期値と極端に小さい倍率での
    縮小フィルタを確認する」程度。**Small**
- やるときに決めること: 下限値。極端な縮小での見た目 (背景の面積) をどこまで許すか。
- 規模 / 優先度: Small / P3 (**現時点では実施しない**)。

### 1.207 フォルダバーをツールバーの並べ替え対象へ統合する — 外部SNS要望 (2026-09-10)

- 出典: X 上の「フォルダバーを他のツールバーと同じ列に表示したい」という要望。
  現状のフォルダバーはメインツールバーとは別の `TopBottomPanel` で、常に独立した1行になる。
- フォルダバー全体を `ToolbarSectionId` の1セクションとして扱い、ほかのセクションと同じ
  `ドラッグで並べ替えを許可` と `行頭に表示` の対象にする。フォルダバー内の各ボタンを
  個別セクションへ分解するのではなく、アドレス入力欄を含む1つのまとまりとして移動する。
- **既定の見た目は変えない。** 既定順序は従来のツールバー各セクションの後、
  `行頭に表示=ON` とし、更新前と同じくフォルダバーだけの行にする。既存設定の移行でも
  新しいフォルダバーセクションを順序末尾と行頭集合へ1回だけ補い、利用者がその後OFFにした
  値を起動のたびに戻さない。
- 行頭をOFFにすると直前のセクションと同じ行へ置き、アドレス入力欄がその行の残り幅を使う。
  ただし入力欄を操作不能な幅まで潰さない。残り幅が所定の最小幅に足りない場合は、
  フォルダバー全体を次の行へ折り返す。フォルダバーより後ろにセクションがある場合は、
  可変幅欄が後続と重ならないよう後続を次行へ送るか、同等に安定した割当規則を設計する。
- 現在の `render_address_bar` は navigation 戻り値、IME対応TextEdit、右寄せボタン、検索・snapshot
  ごとの有効状態を所有する。描画内容を `&mut egui::Ui` に描ける単位へ分け、既存の機能と
  入力経路を保ったままメインツールバーの折り返しレイアウトへ載せる。単なる設定追加ではない。
- 旧設計では `desired_width(f32::INFINITY)` の入力欄が前後セクションを潰すため統合を見送っていた
  ([toolbar-customization-plan.md](toolbar-customization-plan.md) §2.3)。実装時は可変幅セクションの
  最小幅・折り返し・DPI/狭幅ウィンドウのsnapshotを先に固定し、この判断を更新する。
- 規模 / 優先度: Medium / P3。通常幅・狭幅・高DPI、行頭ON/OFF、先頭/中間/末尾への並べ替え、
  全補助ボタン表示時と非表示時、アドレス欄のIME入力・Enter移動を確認する。

### 1.224 一時表示ナビゲーターの未開始クリックが、同passのfocus lossで消える (2026-09-08)

- P2、コード確認済み・実機再現未確認。固定表示OFFでholdキーにより表示している間、
  既存gestureの無い状態からpress/release→focus loss（またはhold release）が同passに届くと、
  `handle_fs_navigator_input` 冒頭の最終focus/現在ModifierHold gateが先行操作まで捨てる。
- duplicate-detection のR4入力修正で発見した既存欠陥。変更前HEADでも同じgateを確認した。
  今回の類似長押し修正に伴う回帰ではなく、別変更として扱う。
- WindowsではRightCtrl/RightShift/RightAltも設定できるため、左右を失うegui event Modifiersだけへ
  置換しない。現在OS levelを偽の過去focus permitで読んだり、gate全撤去で非表示領域を操作可能にしない。
- 設計候補は、viewer-owned navigatorに実描画済みhit領域・表示根拠Fixed/Holdを所有させ、
  次入力のfocused区間で開始し、既存gesture終端と最終表示許可を分ける。未実装・要設計レビュー。
  現在の表示/修飾カスタム、Hold由来のkeyboard owner制約、modal、page/source/close失効を保つ。
- 経緯と範囲判断は [レビュー修正記録](duplicate-detection-review-fixes-20260907.md) の
  「R4 ordered Flat接続の先行レビュー」とその後の範囲訂正を参照。

### 1.206 動画の V キーが入力先によってズーム切り替えへ届かない疑い (2026-09-09)

**2026-09-10更新：共通の動画操作経路への修正・独立レビュー・自動回帰を完了。利用者も現在は動作し、再発していないと確認。** 以下は調査時の記録。

**P1・調査中。** 利用者が、通常動画で V を押しても 100% 表示が出ず、開き直すと復旧したと報告。
HUD の hover には [V] が出ていた。現在ログの一部で V が通常 fullscreen の egui 経路へ届き、
後続の別動画では native handler を通って表示 surface が切り替わることを確認した。
通常経路の V は 360 判定だけで処理し、native 側の通常動画 zoom 分岐を使っていない。
実不具合の動画・時刻との対応付けと handler レベルの再現確認を進める。
強制フォーカスや重複したキー注入で回避せず、入力所有と共通の操作処理の境界を確認する。
記録は [次版開発台帳](next-version-development.md) を参照。

### 1.0i Remote の寸法 probe を「近傍だけ」にするか — R-23 の残り (2026-08-30)

**一括・並列化は v3.3.1 で済んだ** (R-23)。実ファイル画像の寸法読みが 1 件ずつだった
のを、ZIP (書庫を 1 回開く) / PDF (worker 1 往復) と同じ「まとめて 1 回」へ揃え、
rayon の既定プールで並べた。**答えは 1 つも変えていない** (等価性テストあり)。

**残っているのは「件数そのものを減らすか」という判断で、これは答えが変わる。**
`build_remote_spread_page_groups` は横長ページを境に以降の組み方が変わるので、
近傍だけ読んで残りを後から埋めると、**埋まった瞬間に見開きの組が組み替わる**。

**方針決定 (2026-08-30、利用者判断)**: **B は採らない。** 表示中にページの組が変わるのは
体験として悪い。直すなら **C (永続化)** か、待たせる間の**進捗表示**で体験を改善する方向。

- **A. 現状維持**: 全件読む。並列化で 3〜4 倍速いが件数に比例する。**通常のフォルダでは
  実質ゼロ**なので、当面はこれで足りる。
- **B. 近傍だけ + 非同期補完**: ~~初期応答は一定~~ → **不採用**。横長ページを境に以降の
  組み方が変わるので、補完が届いた瞬間に見開きが組み替わる。
- **C. 永続化**: 一度読んだ寸法を残す。**答えを変えない**のでこちらが本命。
  設計上の論点は「どこへ書くか」— カタログ DB には寸法列があるが、remote service は
  別プロセスなので所有権をどうするか。サムネイル blob を持たない**寸法だけの行**を
  許すかどうかも含めて決める。
- **D. 進捗表示**: 待ち時間そのものは減らないが、無言で固まるのをやめる。C と併用可。

**行が無い条件を正確に**: 既定の `Auto` 方針は「2MB 以上」または「decode+display が 25ms
以上」でキャッシュする。つまり**小さくて速い画像は、一度見た後もカタログ行が無い**。
「未閲覧のフォルダだけの問題」ではない。

**計測について**: 合成ファイル (1200x1800 PNG/JPEG、OS キャッシュ温) では 1 件 32µs、
並列化で 3.28 倍。レビューの実測は初回 454µs/件。**合成の数字を実機の見積もりに使わない**
(過去に 30 倍外している)。NAS のように I/O が支配する環境ほど並列化の効きは大きい。

### 実は大半が既にある

[post_operation_selection.rs](../src/post_operation_selection.rs) の `decide` は、
**要求を再読込またぎで保持し、集合が増えていれば再適用する**。`note_applied` が期限も
延ばすので、遅れて届く分は本来拾える。

失うのは 1 か所だけ — 「前回適用した集合から増えていない」ときの `Step::Drop`。これは
**利用者が自分で変えた選択を、無関係な再読込のたびに奪わない**ために置かれている
(コメントに明記あり)。守りたいものは正しいが、判定が代理になっている。

**利用者の言い方のほうが正確**: 止める条件は「集合が増えなかった」ではなく
「**利用者が選択を変えた**」。置き換えると:

| 状況 | 今 | 直した後 |
| --- | --- | --- |
| 集合が増えた | Apply | Apply |
| 増えていない・利用者は触っていない | **Drop** (= 以後を失う) | Wait (10 秒の期限で切れる) |
| 利用者が選択を変えた | Drop (偶然) | **Drop** (明示的に) |

**実装 (2026-08-30)**: 見立てとは置き場所が変わった。`decide` に現在の選択を渡す形には
できない — **再読込が `checked` を消す**ので、`decide` が走る時点の選択は利用者の意思を
表していない。判定は再読込より**前**、`check_external_folder_changes` が持つ。

3 つが噛み合っている:

1. `decide` の「増えていない」は `Drop` ではなく `Wait` (期限まで)。これが R-09 の本体
2. 再読込の直前に `still_owns_selection` を見て、違っていれば要求を取り下げる
3. 自動再読込の選択保存を止めるのは「**まだ 1 度も適用していない**要求がある間」だけに
   変えた。適用済みなら「元の選択」はこちらが置いたものなので、通常どおり保存して戻す

**テストが 1 つ設計欠陥を捕まえた**: 出力が見えている限り上の期限切れ判定は通らないので、
「増えていない = Wait」だけにすると要求が不死身になり、自動再読込の選択保存を永久に
止めてしまう。期限を明示的に見る形にした。

変異 4 種 (常に所有を主張 / カーソルを見ない / 件数だけ比べる / 期限切れしない) はすべて
落ちる。「件数だけ」は最初生き残ったので、**件数が同じまま別項目へ入れ替える**ケースを
足した。

### 分割到着はどれくらい起きるか

**通常は起きない。** フォルダ監視の debounce は 700ms で、しかも**イベントが来るたびに
期限が延びる** (`CURRENT_FOLDER_WATCH_DEBOUNCE_MS`、`poll_current_folder_watch`)。
コピー中はファイル作成と書き込みのイベントが続くので期限が延び続け、**700ms 静かに
なって初めて 1 回だけ再読込**する。普通の貼り付けは 1 回の tranche にまとまる。

分けるには**コピーの途中に 700ms 以上の空白**が要る。現実的なのは Shell の確認ダイアログ
(「置き換えますか?」) で利用者が考えている時間くらい。あとは、増えていない再読込
(既存ファイルの更新、一時ファイルの消滅、リネーム) が挟まると上の表の 2 行目に当たる。

**これは仕組みからの推論であって実測ではない。** 頻度を根拠に優先度を決めるなら、
実際に大量貼り付けをして再読込回数を数えるべき。

### 1.0k テキスト注釈の編集が重いのは「毎フレーム全画面を焼き直す」ため (2026-08-30 調査)

**発端**: 利用者から「テキスト注釈の編集が重い。1/2・1/4 解像度にする設定を入れて
しのいでいるが、解像度を落とさずに軽くならないか。R-07 / R-14 でそれが直るのか」。

**答えは直らない。R-07 / R-14 は別サブシステム。** 形は同じ (大きな値を UI スレッドが
持って複製する) だが、直す場所が重ならない。

| | R-07 / R-14 | この項目 |
| --- | --- | --- |
| 対象 | 補正レイヤーのマスク | テキスト注釈 (comic overlay) |
| 触るファイル | `ui_adjustment_panel.rs` / `local_adjust_db.rs` / `edit_bundle.rs` / `sidecar.rs` | `app.rs` の同期ベイク / `crates/comic-core/src/raster.rs` / `comic_overlay.rs` |
| 重さの正体 | マスクの直列化・圧縮・DB 保存、文書の複製 | 全画面のベイク + composite + GPU upload |

### 何が毎フレーム走っているか (コード確認、[app.rs](../src/app.rs) `ensure_comic_composite_texture`)

編集中 (`text_mode`) は **意図的に同期ベイク**している。非同期にすると、ドラッグ中は
毎フレーム `comic_generation` が進んで常に未完 → 下地 (注釈なし) にフォールバックし、
枠だけ動いてテキストが消える。その退行を避けるための同期である。**この判断自体は正しい。**

同期で走る中身は 4 つとも**全画面サイズ**:

1. `bake_annotation_layers` — レイヤーを画像と同じ w×h で確保して描く (Multiply が混ざると複数枚)
2. `composite_annotation_layers` — 下地と合成してもう 1 枚 w×h
3. `clamp_for_gpu(..).into_owned()` — さらに 1 枚コピー
4. `ctx.load_texture` — w×h を毎フレーム GPU へ upload

`text_preview_scale` (1/2 … 1/8) はこの 4 つ全部を 1/N&sup2; にする。よく効くのは当然で、
**裏を返すと「全画面を焼き直している」ことが重さの全部**ということでもある。

### 解像度を落とさず速くできるか — できる見込みはある

**ドラッグ中に実際に変わるのは、動かしているオブジェクトの旧位置 ∪ 新位置だけ**である。
下地 (`base`) は `Arc::ptr_eq` が保たれていて変わらない (= 作り直していない)。
つまり全画面を焼き直す必要が無い。

- 部分アップロードの手段は**ある**: epaint 0.33.3 に `TextureHandle::set_partial(pos, ..)`
  (`epaint/src/texture_handle.rs:78`、`ImageDelta::partial`)。確認済み
- 規模は**中**。`bake_annotation_layers` に「原点つきの窓」を渡せるようにする契約変更 +
  composite の窓化 + `app.rs` 側の dirty rect 追跡

**着手前に潰すこと**:

- 影 / 縁取り / ぼかしは幾何 bbox の外へ出る。padding は**推測せず bake 側から出す**
- 窓に重なる他オブジェクトは z 順で焼き直す必要がある (窓に clip して全件通す)
- Multiply レイヤーは画素ごとなので窓化しても結果は変わらない
- 文字の入力・改行は 1 オブジェクトの bbox が変わるだけなので同じ枠組みに乗る

**まだ測っていない。** §1.0 の表にある 70.6ms / 27.7ms は**補正レイヤーの数字**であって
この経路のものではない。着手時に、原寸で 1 フレームのどこに時間が行っているかを先に測る
(4 段のどれが支配的かで、窓化だけで足りるか upload の形まで変えるかが決まる)。

### 並行開発できるか — できる

`external-tool-launch` worktree が触る `src/` 20 ファイルのうち、この項目の対象
(`crates/comic-core/`、`comic_overlay.rs`、`ui_text.rs`) と R-07 の対象
(`ui_adjustment_panel.rs` ほか) は **0 件**。唯一重なるのは `app.rs` だが、向こうのハンクは
10701〜15786 / 26212 / 60220〜60549 / 66930〜67594 で、注釈ベイク (59300 台) とは重ならない。
他の 3 worktree (pano / r2e / video-strip) は master に対して `src/` の差分が無い (= マージ済み)。

### 1.0f 実機確認で見つかった 3 件 (2026-08-30)

**(a) F12 連打で動画再生が止まり、別ウィンドウが閉じる。** 利用者報告。
**v3.3.0 でも起きる (連打を続けると再現)。退行ではなく、起きやすくなった。**

**原因はログで確定した。私が最初に立てた仮説 (`push_detached_release` が移譲した lease を
畳む) は外れ。**実際は **自分で送った `ViewportCommand::Close` が、再利用された同じ
ViewportId の新しい viewport に届き、利用者が × を押したのと同じ扱いになる**。

実ログ (`mimageviewer.log`、transition 29 → 30):

```
1545.452  transition=29 target=fullscreen action=Destroy ... viewport_command=Close  ← 自分で送る
1545.532  F12 → transition=30 target=detached phase=begin host=0x0
1545.564  [detached-viewer] show viewport: ... host=hwnd=0        ← 同じ ViewportId を再表示
1545.564  [detached-viewer] viewport close_requested: presentation=None host=hwnd=0  ← ★
1545.566  active-detached-session action=clear reason=handle_fullscreen_close_request
1545.575  presentation-transition id=30 target=DetachedWindow effect=Destroy hwnd=0x0
1545.594  [decoder-lifecycle] video-decode thread exit: live_count=0                 ← 再生停止
```

★ の行は **`presentation=None` かつ `host=hwnd=0`** — まだ窓が無い viewport なので、
**利用者が閉じられるはずがない**。112ms 前に自分が送った Close が届いている。

[ui_fullscreen.rs:14591](../src/ui_fullscreen.rs) は
`ctx.input(|i| i.viewport().close_requested())` を無条件に `close_fs = true` にしており、
**自分が送った Close と利用者の × を区別できない**。

**なぜ v3.3.0 より起きやすいか**: R-27 の lease 移譲で同じ window_id / ViewportId が
そのまま後継へ渡るため、viewport の再表示が早く・同一 id で起きる。stale な Close が
新しい viewer に当たる確率が上がる。**Codex が案 B を否定したときに挙げた危険
(「`ViewportEvent::Close` を利用者の close と解釈して後継ごと閉じる」) が、
現行経路にも存在していた**ということ。

**v3.3.1 で修正済み** (`edf1c5ed`)。方針は Codex と合意 ([review §10.7](review-v3.3.0/README.md)、ブリーフ = [close-identity-brief.md](review-v3.3.0/close-identity-brief.md))。私が出した 2 案はどちらも Codex が証拠つきで否定した (egui では内部 Close と利用者の × が同じイベントになり照合不能 / incarnation と ViewportId の採番順が循環)。採るのは **内部 teardown で Close を送らず、terminal になった ViewportId を再利用しない** 第三案。

**(b) 動画を別ウィンドウで開いているとき、gamepad の十字キーでシーク操作ができない。**
利用者報告 (2026-08-30、v3.2.0 / v3.3.0 で同じなので退行ではない)。静止画では動く。

- **当初の見立て「動画面の分岐に入っていない」は否定済み** (`75cdfd9b0`、v3.4.0)。十字キーの
  配り先を `DpadRoute` の 1 値へ集約したうえで計装したところ、mount 済みの detached context が
  動画を `fullscreen_idx` に持つ状態では、判定は正しく `Video` を返していた。
  **この前提で調べると空振りする。**
- **2026-09-04 の実機ログでは、右キーでシークできている**。利用者が「今は効く」と報告し、
  `mimageviewer.log` に裏付けがあった:

  ```
  [gamepad] dpad route: Video(10) surface=Viewer fs_idx=Some(10) is_music=false
            detached_context_at_rest=false session_detached_or_switching=true
  [native-video-key] seq=1 virtual_key=0x27 ... fs_idx=10 presentation=detached
            outcome=action:seek_forward_5s
  ```

  **`session_detached_or_switching=true` かつ `at_rest=false`** — root projection が
  `fullscreen_idx` を持ったまま detached 表示している側では、配り先も下流も通っている。
- **残る疑いは `at_rest=true` の側**。active viewer context が AtRest のとき、batch は root では
  配られず、`gamepad_batch_goes_to_active_context()` (= `at_rest && surface == Viewer`) を通って
  `update_active_viewer_context` が mount した中で配られる。**この経路を通ったときの
  `dpad route:` 行は、まだ 1 度も記録されていない。**
- **次にやること**: 効かない状況を再現し、`[gamepad] dpad route:` を見る。Video 行が出なければ
  batch が mount 済み context へ届いていない (行に並ぶ述語が、どれで落ちたかを示す)。Video 行が
  出ていれば下流で、`[native-video-key]` の `outcome=` が続きを語る。**どちらか分かるまで
  直さない。** 窓が 2 つ以上あるとき / 直前にメインを触ったとき / F12 直後かどうかで
  `at_rest` は変わるので、再現時はその条件も記録する。

**(c) R-02 の症状は利用者環境では再現しなかった。** 別ウィンドウを開いたままメインを
前面にして十字キーを押すと、**v3.3.0 でもメイン一覧が動いて見えた**との報告。
§10.2 で確認したのは「配り先と面の判定が別の情報源を使っており、食い違い得る」ことまでで、
`active_detached_context_is_at_rest()` が真になる条件は限られる。修正 (`7f064a57`) は
2 つの判定を 1 つにするもので構造的には正しいが、**利用者に見えていた症状の説明としては
私の記述が証拠より強かった**。§10.2 の書き方を弱める。

### 1.0 v3.3.0 レビューからの持ち越し (2026-08-29 に判断)

正本は [docs/review-v3.3.0/README.md](review-v3.3.0/README.md)。v3.3.0 出荷時に
「複数ウィンドウの不具合解消」を優先し、以下を次版へ回した。**いずれも実測または
コード確認のうえで判断しており、推測で落としたものは無い。**

| ID | 内容 | 退行か | 実測 / 根拠 |
| --- | --- | --- | --- |
| R-07 (d) | 補正マスクの圧縮・DB 保存が UI スレッド | v3.2.0 以前から。**v3.3.0 が 5.8 倍速く**した後 | 24MP で 70.6ms。**ストローク単位ではなくフレーム単位だった** (`Slider::changed()` はドラッグ中毎フレーム真)。**対応済み** (`976aef4a` / `d6a591b2`) |
| (b) | 図形のキー移動 / 回転がキーリピートごとに全文書を保存 | v3.2.0 以前から | リピート 1 回ごとに 70ms + undo 2 文書。**対応済み** (`368499d9` / `9ccbb934` / `d75bed4c`) |
| (c) | 塗りの毎フレームに文書 1 複製が残る | v3.2.0 以前から | 24MP で 27.7ms/フレーム (もう 1 枚は `3315fd05` で削除済み)。**対応済み** (`116ca8cd` / `a9f69d14`) |
| R-18 | Remote の ZIP 寸法 probe が 64KiB で打ち切る | いいえ (経路ごと v3.3.0 の新規) | v3.2.0 は寸法を一切持たず見開きが効かなかった。**見送っても v3.2.0 より悪くならない** |
| R-12 | detached parked fallback が UI で `parent().is_dir()` | いいえ | クリック 1 回につき 1 syscall。コード側にも「恒常経路ではない」と明記。**v3.3.1 では見送り** — レビューの修正方向 (parked snapshot を async build/commit/abort まで保持) は detached の lifecycle 変更で凍結ルールの対象。1 syscall のために所有権境界を動かす取引が見合わない (切断された共有だけが遅い) |
| R-11 | トレイ退避が sidecar 書き込み完了を UI で待つ | いいえ | `df245720` で writer 側の複製 1 回分は減った。**対応済み・実機確認済み** (2026-09-01。編集直後に退避 → 復帰で編集が残ることを確認) — `PersistScope` で「待つ」を終了経路だけの選択にした。待っていた理由はプロセスが止まることだけで、トレイ退避では writer も読み手も生きたまま (読み手は pending を先に見る) |
| R-19 | 360 動画の roll が描画へ渡らない | いいえ | 該当素材のみ。shader 配線 + 実機検証が要る |
| R-10 / R-13 / R-16 / R-22 | P3 4 件 | いいえ | R-10 は §1.141 で既に P3 裁定済み |

**R-07 / R-14 の結果 (2026-09-01)**: 保存は worker へ移り (UI スレッドの同期 I/O は
p95 0.3ms)、**24MP の筆のフレームは 101ms → 9.2ms (8.5 → 73 fps)** になった。ただし
**原因は 3 回とも予想と違った** — 保存でも塗りでもなく、①マスク重ね描きの毎フレーム
複製 ②補正パネル編集器の毎フレーム複製 ③プレビューの直列再構築 だった。どれもこの
表にも review にも無く、計装を足して初めて見えた。経緯・実測・未修正で残した 1 件
(detached context と App-global な保留、BA-7 系として
[detached-rework-plan.md](detached-rework-plan.md) へ報告) は
[briefs/local-adjust-ownership-progress.md](briefs/local-adjust-ownership-progress.md)。

**(c) は見積もりをもう一度外していた** (2026-08-30 に調査。着手前に読むこと):

前提が違った。**あの複製は無駄ではなく、正しさを支えている。** 16 個の closure が通る
`mutate_local_adjust_layer_from_canvas_impl` は、文書を丸ごと複製してから closure に渡し、
closure が `false` を返したら複製ごと捨てる。つまり「`false` を返した = 文書は変わっていない」
という不変条件を、**複製の破棄そのものが作っている**。その場で書き換える形にすると、
`false` を返した編集の書き込みが残る。

書き込みは 16 か所に散らばってはいない。**1 か所に集まっている** —
`local_adjust_target_raster_vector_mask_mut`
([ui_adjustment_panel.rs](../src/ui_adjustment_panel.rs))。これが `false` の前に文書を書く
経路は 3 つあり、`create` 引数で止まるのは 1 つ目だけ:

1. `create = true` のときスロットへ空マスクを作る (バックログが元から挙げていたもの)
2. **`create` に関係なく** `Raster` を `RasterVector` へ変換する (形式の昇格)
3. **`create` に関係なく** `resize_to(width, height)` を呼ぶ

その後で呼び出し側は `source.size != [mask.width, mask.height]` や「塗ったが 1 画素も
変わらなかった」で `false` を返せる。つまり **`create=false` で呼んでいる経路も安全ではない。**

集約されているのは朗報 (直す先が 1 つ) だが、**必要なのは契約の変更**であって複製の削除
ではない。方向としては、材質化・昇格・リサイズを「それ自体が変更である」ものとして
closure の外へ出し、`mutate` は変更の有無だけを返す純粋な編集にする。そのうえで seam は
文書を **move** して (複製せずに) 渡す。

**(d) と R-26 は同じ境界の話。** (d) の「保存を worker へ出す」は、R-26 で入れた
「durable な保存の成功を公開の境界にする」(`b8cb3ce5`) を開け直さないよう設計する必要がある。
別々に考えない。

**(b) に着手するときの注意** (見積もりを 1 度外している):

- (c) の詳細は上へ移した (調査で前提が変わったため)。
- (b) はキーを離したときだけでなく、**ページ移動・モード終了・フルスクリーン終了など
  編集状態を畳む全箇所**で確定が要る。ブラシは破棄でよいがキー移動は完了した編集なので
  破棄できない。取りこぼし防止の監査テストを付けること。

### 1.7 detached 中の発火面解決の残り (? / トースト / スタック) — BA 報告

- 残り (P3、発火面の window_id 粒度が必要):
  1. **? キーのショートカット一覧**: detached 中にグリッドから開くと FS 用一覧が
     出る。修正には発火元の面情報を consume サイトから通す必要がある。
  2. **トーストの面粒度**: 発火面のビューアを閉じた後に完了したバッチのトーストが
     不可視のまま消える / active detached + main fullscreen 同時表示時に Viewer 面が
     両ビューアに出る (2 値 enum の粒度の限界)。許容中。
  3. **複数ウィンドウ×スタックの理想形**: スタックをクリックするとメイン一覧が
     フラット読書ビューに切り替わる (フル機能と同じ挙動)。理想は「メインは集約のまま、
     窓側だけフラット文脈を持つ」だが、窓が自前のフラット items 文脈 (bundle 化された
     stack 状態) を持つ設計が必要 (複数ウィンドウの bundle 化と同系統)。現行でも
     Shift+↓↑ ジャンプは動作する。
- 優先度: P3。

### 1.9 parked 窓のリソース制御 (サムネ pipeline 停止 / VRAM 合算予算) — 角度⑥送り

- 出典: v2.3.0 角度⑥レビュー (Sol/Terra 一致、docs/archive/review-v2.3.0/sol-angle-reviews.md)。
  いずれも bounded な効率問題でデータ喪失は無し。park/再活性ライフサイクルの構造に
  踏み込むため、リリース直前パッチではなく detached リワーク後続で設計対応する。
  1. **P2: park してもサムネ pipeline が止まらない**: `pause_background_work_keep_current_frame`
     は fullscreen/AI 系のみ cancel し、bundle の `cancel_token` / `reload_queue` /
     `heavy_io_queue` に触れない。park 時点で積まれていたデコードが走り切り、結果
     `ColorImage` が誰も poll しない rx に溜まる (窓の再活性化 / close まで保持)。
     対応案 = park 時に queue を drain (worker pool は殺さない)。ただし Requested 状態の
     サムネが再活性化時に再要求される仕組みの確認が必要 (state が Requested のまま
     queue から消えると復帰後にロードされない恐れ)。
  2. **P2: サムネ VRAM 上限が文脈単位**: `update_keep_range_and_requests` の予算は
     mounted 文脈にしか効かず、parked bundle N 個がそれぞれ上限近くまで保持し得る
     (動画サムネは eviction 対象外なのでフォルダサイズ分)。対応 = 全 bundle 合算の
     予算会計 + cross-bundle eviction (リワークの資源予算ステージ)。
  3. P3: `display_px_shared` が App-global のため、detached 文脈のデコード解像度が
     メイングリッドの表示密度に引きずられる (適用ミスは無し、品質/CPU の無駄のみ)。
- 関連: v2.3.0 で対応済みの境界 = bundle Drop の解放 (角度①でクリーン確認)、
  文脈別 channel/cancel/世代 (P2-9)。

### 1.10 終了時の削除 worker 未調整 — 角度⑤送り (P2、実害小)

- 出典: v2.3.0 角度⑤ Sol P2。数百件削除の実行中に終了すると、実行中の
  IFileOperation チャンクは完走するが後続チャンクは開始されず、部分削除の最終報告と
  完了後クリーンアップ (resume purge 等) が走らない。一覧は次回起動の走査で実態に
  収束するため破壊は無し。対応案 = 終了時に cancel を立てて現行チャンクの完了を
  短時間待つ + 次回起動時の注意トースト。v2.2.0 出荷時からの既存挙動。

### 1.12 detached 静止画窓から音声/動画へのフォルダ内ナビ不可 (メディア昇格導線)

- 出典: v2.3.0 実機検証 §5 #33 (2026-07-10)。別窓 (静止画) で ↑↓ ナビ中、音声/動画の
  アイテムへは移動できない (静止画窓はメディア再生セッションを持てない設計境界)。
  音楽ビュー→画像の方向は移動できるため非対称。ユーザー許容済み (「一旦これでもよさそう」)。
- 対応案: detached リワークの後続ステージで「静止画窓がメディアに到達したらメディア
  セッションへ昇格 (または既存メディア窓へ委譲)」の導線を設計。凍結ルール下では
  症状パッチを入れない。

### 1.13 duration 不明/不正 MPEG-PS のシークバー不能

- 背景: `sample.mpg` で実機確認済み。duration が不明または不正な MPEG-PS は
  シークバーで移動できない。VLC は同じファイルをバイトシーク + PTS 再スキャン方式で
  シークできる。
- 対応案: decode EOF で実尺を学習して `VideoInfo` / HUD の duration へ反映するか、
  duration を信頼できない場合にバイト位置シークへフォールバックする。
- 裁定: v2.2.0 以前から同挙動で、ユーザー裁定により次版送り。優先度 P3。
  final-report 追補 5 の P3 参照。

### 1.17 UI レイアウト崩れ (文字はみ出し / 見切れ) の自動検出 QA → その後に英語対応を再検討

- 背景: 英語対応 (UI ローカライズ) を検討したが、AlternativeTo は「英語 UI 必須」
  (FAQ: DB 登録アプリは英語対応が必須) のため、日本語のみの現状では掲載できない。
  一方、多機能ゆえに英語 UI を実機で全画面目視 QA するのは負担が大きく、しかも日本語でも
  文字はみ出し / 見切れのレイアウトバグが繰り返し発生している。そこで **まず言語非依存の
  「レイアウト崩れ自動検出」QA ツールに投資**し、日本語バグに即効かつ将来の英語化の
  「目視 2 倍」問題を先に解消する。英語対応の可否はその後に再判断する。
  (2026-07-13 相談。ユーザー方針: QA 投資を先行 → その後に英語対応を再検討。今は別作業中のため別途着手)
- 現状 (確認済み):
  - egui は文字配置時に galley サイズが取れ、painter は clip rect を持つため
    「galley 幅 > 割り当て幅」で見切れを機械判定できる。
  - UI スナップショット基盤 `tests/ui_snapshot.rs` (egui_kittest) が既にある (回帰検出向け)。
  - 文字列は現状ハードコード (i18n 未 externalize)。見切れ計装は現行日本語のままでも動く
    ので、i18n とは分離して先行できる。
- 方針 (QA ツール先行 → 英語再検討):
  - QA ツール (言語非依存・日本語バグにも効く):
    1. **見切れ計装**: 描画時に galley 幅 vs clip/available 幅を比較し、超過した文字列 +
       ウィジェット位置をログ列挙する。全画面を人が見なくても崩れ候補の一覧が出る。
    2. **疑似ローカライズ (幅ストレス)**: UI 文字列を ~1.4x 長い擬似文字列に置換したビルドで
       幅不足のウィジェットを炙り出す (英語が長くなる前提の先行テスト。日本語のままでも
       将来はみ出しやすい箇所を潰せる)。
    3. 既存 egui_kittest スナップショットを併用 (変化検出。最終は差分画像を目視)。
    4. (任意) スクショ自動キャプチャ + ビジョン判定で「切れ / 重なり」を triage。
    - 限界: 得意なのは見切れ / はみ出し / 行あふれ。位置ズレ・不格好な折り返し・パネルの
      重なりは取りこぼしがあり、多少の目視が残る。
  - 英語対応 (QA ツール整備後に可否判断):
    - 前工程 = 文字列の externalize (i18n 化)。
    - AI 一括翻訳 + **用語集** (見開き=Two-page spread 等を先に固定) + 各文字列の文脈付与で
      用語ゆらぎ / 品詞ズレを抑える。
    - **変数整合チェック**: `%s` / `{}` / 改行 の個数・種類が元と英訳で一致するか機械照合。
    - **逆翻訳 QA**: 重要文字列 (削除 / 上書き / 警告 / エラーダイアログ) に限定して英→日へ
      訳し戻し、元日本語と比較する。英語力が無くても意味ズレを検知できる (日本語同士の比較)。
    - 実機確認は見切れ計装 + 疑似ロカライズで大半を機械化し、目視は残差のみ。
    - "English is machine-assisted; feedback welcome" と明示し、OSS の Issue / PR で継続改善。
- AlternativeTo 連携: 英語 UI が入れば掲載可。申請素材 (Tagline / Description の v2.3.0 版
  = 音楽再生 + VST3 入り / Website / Source / License Free+OSS(MIT) / 画像 = icon.png + ss_*.png
  含む ss_music.png / Alternative to = NeeView・MangaMeeya・Leeyes・ZipPla・Honeyview・
  ImageGlass・IrfanView・XnView MP・nomacs・FastStone) は準備済み。GitHub Topics / 英語 README
  サマリ / egui Showcase / awesome-egui PR は UI 言語非依存の枠なので実施済み。
- 規模 / リスク: QA 計装 = Medium (描画経路への hook + 幅バジェット定義)。英語化 = Large
  (externalize + 翻訳 + QA 工程)。QA ツールは日本語バグにも効くため単独で投資回収がある。
- 優先度: P2 candidate (QA ツール)。英語対応は QA ツール整備後に再判断 (現時点は保留)。

### 1.21 自動選定した親コンテナ代表サムネイルに編集プレビューを反映

- 出典: v2.5.0 リリース前実機確認 (2026-07-17)。ZIP 内画像を編集しても、親階層の
  ZIP セルで自動選定された代表サムネイルには編集結果が反映されない。
- v2.6.0 で手動ピン経路は対応済み。cascade 解決後の固定 leaf (直接 Image / ZipEntry /
  PdfPage、ネスト ZipDir、変換アーカイブ) は canonical page key を親要求へ渡し、編集
  preview を catalog より優先する。固定 leaf の個別色調補正も注釈前の下地へ適用し、
  保存 / 無効化通知を親セルへ伝播する。
- 残件は **自動代表選定だけ**。worker 内で leaf を選定した後に page key を組み立て、
  対応する編集 preview があれば raw decode / catalog より優先する。自動選定結果と親
  キャッシュの無効化条件も同時に設計する。
- 不変条件:
  - 現行の編集 preview 内容 (erase / local_adjust / conceal / crop / comic 注釈) だけを
    自動選定 leaf へ反映する。自動代表へどの色調補正を適用するかは別途決め、手動固定済みの
    個別色調補正規則を変えない。
  - UI スレッドへ ZIP/PDF 列挙や SQLite 単件照会を追加しない。
- 規模 / 優先度: Medium〜Large / P3。

### 1.31 wndproc の内側で GPU 待ちのある描画をする構造

- 出典: §1.30 の解析 (2026-08-01、Codex Sol の独立レビュー)。**v2.9.1 の範囲外**。
- **進捗 (2026-08-16)**: **§1.31-A は完了、§1.31-B は未着手**。A で vendored eframe に
  per-window / viewport / surface-generation 単位の typed scheduler と about_to_wait 外側
  paint drain を新設し、通常 RedrawRequested、bootstrap、AccessKit、不可視窓の要求済み work が
  message dispatch 中に renderer へ入る経路を除去した。inline 例外は event provenance が
  非ゼロ Resized の InteractiveResize frame と、その親 frame が作る immediate viewport render
  subtree だけ。reducer 11契約、既存 hidden throttle 3件、別 thread の SendMessageTimeoutW が
  RedrawWindow で同期 damage を発生させる Windows process test を gate 化した。
  **A が除去したのは「同期メッセージ自身が GPU 待ちを開始する経路」だけ**であり、外側 paint 中の
  GPU 待ちに後着の同期メッセージが巻き込まれる問題は B のまま残る。
- 何が問題か: winit は `WM_PAINT` 内で `RedrawRequested` を**同期 dispatch** し、eframe はそこで
  `run_ui_and_paint` を直接呼ぶ。さらに Windows の resize では flicker 防止のため意図的に同期 paint
  する。結果として **wndproc の内側で swapchain acquire と Present を行う**。Microsoft も
  `Present` が message-pump thread を待ち得ると明記している。
  main surface は `AutoVsync` / maximum frame latency **1** なので、前フレームが retire しない限り
  次の acquire が待たされる。
- 効果: 窓の owner / z-order / visibility を動かす操作、DWM 合成の切替 (Independent Flip / MPO)、
  device-loss 周辺のいずれかが絡むと、**同期メッセージ 1 通で UI が任意時間停止し得る**。
  Stage 4 で閉じた側 (presenter スレッドが窓を持ったままブロックする) に対し、こちらは
  「UI スレッドが自分の窓のメッセージ処理中に GPU で止まる」。**同じ上位の破綻の別の面**で、
  Stage 4 の pump 分離では届かない。
- Stage 4 以降の運用制約 (維持すること): pump thread では GPU API を呼ばない。held frame の
  再提示にも re-arm 用 `AcquireSync` を追加せず、reader key の維持と producer 側回復を使う
  ([video-architecture.md](video-architecture.md) の `FramePresentationState` 節)。
- 方向 (Codex 案):
  - wndproc は damage / resize / redraw 要求を記録して**即 return** する。
  - 実描画は外側の event-loop 境界へ post して coalesce する。
  - `Idle / Painting / Pending` の単一 typed render state を置き、**再入 acquire を禁止**する。
  - surface acquire は短時間・nonblocking にし、間に合わなければその frame を捨てる。
  - msg / paint depth / wait handle / timeout / Present 前後 / cloak / visibility を 1 つの trace に残す。
  - 同期 `SendMessage` 中に presentation availability を故意に止め、**wndproc が有限時間で返る**
    Windows 回帰テストを追加する。
  - 必要なら eframe / winit / wgpu を vendor patch し、upstream へ最小再現を出す。
- 併せて直す観測の穴: 今回 `ui-heartbeat-watchdog` は生きていたのに `panic.log` へ何も書かれ
  なかった。**active native fullscreen 中は main HWND が隠れていても watchdog を suspend しない**
  ようにする (危険な操作の最中こそ黙る、という現状は逆)。
- **v3.0.0 の判断 (2026-08-13)**: **構造修正は v3.0.0 をブロックしない**。
  - 根拠: この問題が実際に牙を剥いた唯一の事例は §1.30 で、それは **VST エディタの開閉が
    窓の owner / z-order を動かす**ことが引き金だった。v3.0.0 の Web 配信は、HTTP サーバと
    トランスコードを**UI スレッド外**で回すもので、**窓の所有関係にも同期メッセージにも
    手を出さない**。§1.31 の露出面を新たに増やさない。
  - その §1.30 も revert 後は再現していない (上記のとおり保留)。
- **観測の穴は解消済み (v3.0.0 前)**。`ui_heartbeat_should_stay_active_while_hidden`
  ([app.rs](../src/app.rs)) が keep-alive の述語に **mounted media session** (フルスクリーンの
  動画 / 音声、動画→音声モード、`FsCacheEntry::Video`) を含めるようになり、ネイティブ動画
  フルスクリーン中に main HWND が隠れても watchdog が armed のままになる。以下は当時の記録。
- ~~ただし観測の穴だけは v3.0.0 の前に埋める (Small)~~。§1.30 のとき
  `ui-heartbeat-watchdog` は生きていたのに `panic.log` へ何も残らなかった。
  [app.rs:62646 付近](../src/app.rs:62646) を見ると、main HWND が不可視になった時点で
  watchdog を suspend し、**例外は `viewer_session_is_detached_or_switching()` だけ**。
  ネイティブ動画フルスクリーン中は main HWND が隠れるため、**最も危ない操作の最中に
  watchdog が黙る**。keep-alive の述語にアクティブなメディアセッションを含める。
  これだけ入れておけば、次に野生で固まったときにログが残る。
- message-dispatch / render 位相分離は §1.31-A として完了。nonblocking acquire / Present と
  message service latency の上限制約は §1.31-B として後続する。
- **着手順の確定 (2026-08-16、ClaudeCode / Codex Sol 合意)**:
  `§1.85-A → §1.86 → §1.31 本体` の 3 段。根拠と逆順の失敗モードは
  [codex-texture-delivery-test-brief.md](briefs/codex-texture-delivery-test-brief.md) §0。
  §1.31 は frame drop を通常経路にするので、その経路の delta 配送を先に契約化する。
  **§1.86 は障害影響としては P3 のままだが、本項の依存関係上は必須前提**として扱う。
- **前提充足 (2026-08-16)**: §1.86 の単一 delta transaction owner と non-submit outcome の
  finalization 契約が完成し、その上で §1.31-A の位相分離まで完了。通常 frame drop と readiness
  wake を足す §1.31-B は未着手。
- **本体を A / B に分割 (2026-08-16、ClaudeCode / Codex Sol 合意)**:
  - **§1.31-A**: message-dispatch 位相と render 位相の分離。**同期メッセージ自身が GPU 待ちを
    開始する経路**を除去する。vendored eframe だけで完結する (winit / wgpu は vendor しない)。
    ブリーフ = [codex-render-phase-separation-brief.md](briefs/codex-render-phase-separation-brief.md)。
  - **§1.31-B**: presentation と message service latency の上限制約。
    **A を merge しても §1.31 は完了しない**。UI スレッドが外側の paint で GPU を待つ間も
    message pump は止まるため、他スレッドの `SendMessage` は依然ブロックする。A が消すのは
    「同期メッセージが GPU 待ちを**開始させた**」経路だけ。
  - **B の実現可能性は確認済み (2026-08-17、コードで実地確認)**。acquire 側は
    **wgpu の vendor 不要**で組める:
    - `Dx12UseFrameLatencyWaitableObject` の既定は **`Wait`**
      ([wgpu-types instance.rs](https://docs.rs/wgpu-types/27.0.1/))。つまり現状の
      `acquire_texture` は frame-latency waitable object を待ち、wgpu-core の
      `FRAME_TIMEOUT_MS = 1000` で打ち切られても**その結果を捨てて先へ進む**。
    - `DontWait` は doc に「**This is useful if the application wants to wait for the
      waitable object itself**」と明記されており、まさに B の用途。swapchain は
      waitable flag 付きで作られ、handle は取れるが wgpu は待たない。
    - handle への到達経路も public: `wgpu::Surface::as_hal::<Dx12>()` (surface.rs) →
      `wgpu_hal::dx12::Surface::waitable_handle()`。
    - → 設計: 起動時に `DontWait` を設定し、paint 前に `WaitForSingleObject(handle, 0)` で
      非ブロッキング判定。未 signal なら damage を保持したまま `DeferredNotReady` として
      その frame を捨て、signal されたら wake する。**時間窓ではなく OS の事実で判定する**
      ので憲法 5 に適合する。
    - 注意: DX12 専用。backend が DX12 でない場合 `as_hal` は None を返すので、その場合は
      現行挙動へフォールバックする (新しい失敗経路を作らない)。
    - 参考: mIV は既に `MIV_WGPU_FRAME_LATENCY` で `desired_maximum_frame_latency` を
      設定しており ([lib.rs](../src/lib.rs))、native presenter は自前 swapchain で
      `DXGI_SWAP_CHAIN_FLAG_FRAME_LATENCY_WAITABLE_OBJECT` を既に使っている
      ([render_core.rs](../src/video/native_presenter/render_core.rs))。機構は既知。
  - **Present 側は別問題で、これは vendor 不要では閉じない**。`SurfaceTexture::present()` は
    戻り値が `()` で、HAL の FIFO Present は interval 1 かつ `DXGI_PRESENT_DO_NOT_WAIT` 無し
    (`DO_NOT_WAIT` は blocked 時に `DXGI_ERROR_WAS_STILL_DRAWING` を返す契約)。
    厳密に bounded にするなら wgpu / core / hal の patch か render thread 分離が要る。
  - ~~**B は acquire だけ先に閉じて、Present は A 後の実測を見てから判断する**のが妥当。~~
    **2026-08-17 撤回。acquire も先に測る**。下記参照。
- **B は「計測 (B0) が先」に変更 (2026-08-17、Codex Sol の設計レビューで差し戻し)**。
  正本 = [codex-acquire-readiness-gate-brief.md](briefs/codex-acquire-readiness-gate-brief.md)。
  差し戻しの決め手 2 つ (どちらもコードで実地確認済み):
  - **acquire を閉じても message service latency は上限化されない**。非ゼロ `Resized` は
    message-dispatch 中に `on_window_resized` → `configure_surface` を通り、wgpu-hal DX12 の
    `configure` は `ResizeBuffers` の前に `wait_for_present_queue_idle()` を呼ぶ。その中身は
    **`WaitForSingleObject(event, INFINITE)`** (`wgpu-hal-27.0.4/src/dx12/device.rs`)。
    unconfigure / drop 側にも同じ待ちがある。**§1.31-A が残した inline resize 例外は、
    acquire より手前で無期限 GPU wait に入れる。** 「acquire は最大 1 秒」も強すぎた
    (1000ms は frame-latency wait の timeout であって acquire 全体の上限ではない)。
  - **§1.31-A の後、acquire が実害を起こしている証拠が無い**。§1.30 の実スタックが本当に
    この wait だったかも未確定のまま。`configure` の `INFINITE` / acquire / Present の
    3 候補があり、どれが効いているか不明な状態で 1 つを選んで直さない。
  - 旧 acquire gate 案には他に P0 が 4 件あった (ブリーフ §3 に訂正付きで保存):
    `waitable_handle` は `unsafe` で「swap chain が生きている間だけ有効」→ worker へ raw handle を
    渡すのは未定義動作 (`DuplicateHandle` が要る) / UI と worker を同時 waiter にすると signal の
    取り合いになる / §1.31-A の `surface_generation` は handle generation の正本ではない
    (`RecreateSurface` で増えず、`set_window(None)` は全 surface を clear する) /
    immediate viewport は親の pass 内で自前 surface を acquire するので outer gate では捕まらず、
    gate の実体は `egui-wgpu::Painter` の per-surface seam に要る。
  - **API 到達性の結論は正しかった**: wgpu 27 に `hal` feature は無いが native build では
    `cfg(wgpu_core)` で `wgpu::hal` が re-export され、`Surface::as_hal` も public (`unsafe`)。
    mIV の依存構成で `wgpu/dx12` / `wgpu-hal/dx12` が有効、`wgpu-hal 27.0.4` 一つに解決される。
    **acquire handle へ到達するための wgpu vendoring は不要**。
  - **`NativeEguiOverlay` は別の `wgpu::Instance`** (`src/video/native_presenter/render_core.rs`)
    で既定 `Wait` のまま動く。`src/lib.rs` の設定は届かないので、
    「mIV の acquire 全体を直した」とは言えない。
- **§1.31-A 着手前の 2026-08-16 に訂正した認識**
  (前セッションの読み違い。同じ誤りを繰り返さないため履歴として残す):
  - **外側の paint 位相は現在存在しない**。`check_redraw_requests` は
    [run.rs:198](../vendor/eframe/src/native/run.rs:198) `handle_event_result` の末尾と
    [run.rs:396](../vendor/eframe/src/native/run.rs:396) `new_events` から呼ばれ、
    `WinitAppWrapper` には `about_to_wait` の実装が無い (trait 既定の no-op)。
    不可視窓の direct paint を「既に外側」と読まないこと。
  - **`RepaintNow` は resize 専用ではない**。producer は初回 `resumed` (bootstrap) /
    非ゼロ `Resized` / AccessKit initial tree の 3 つで、`EventResult::RepaintNow(WindowId)` は
    理由を失っている。位相を分けるには reason の型付けが要る。
  - **無限 `WM_PAINT` の懸念は撤回してよい**。winit は `RedrawRequested` を送った後に
    `DefWindowProcW(WM_PAINT)` を呼び、これが update region を validate する。
    eframe が描画せず戻っても再送ループにはならない。
  - **厳密な `MARKER_IN_SIZE_MOVE` 限定は採らない**。winit の非公開状態であり、
    取りに行くと winit の vendor patch か全 HWND subclass が要る。A の inline 例外は
    「非ゼロ `Resized`」とする (現行は全 redraw が inline なので、これでも厳密に狭い)。
- **設計時に確定すべき点 (2026-08-16 に追加で判明したもの)**:
  - **`WM_PAINT` 内の同期 `RedrawRequested` だけでは足りない**。Windows の resize では
    [eframe run.rs:121](../vendor/eframe/src/native/run.rs:121) が `RepaintNow` を受けて
    その場で `run_ui_and_paint` を直接呼んでいる (flicker 対策)。同期 paint の入口は 2 つある。
  - **acquire を短くしても Present 側の上限が未規定**。wgpu-core の acquire timeout は
    `FRAME_TIMEOUT_MS = 1000` 固定 (present.rs)。DX12 backend は frame-latency waitable object の
    `wait(timeout)` が返す `false` を捨てて先へ進む (`unsafe { sc.wait(timeout) }?;` で bool を破棄)。
    Present は FIFO なら interval 1 で `DXGI_PRESENT_DO_NOT_WAIT` 無し。
    **公開 wgpu API の設定だけでは短時間 try-acquire にできない**。lower-layer patch /
    waitable-object の明示 readiness gate / 非ブロッキング Present 戦略のいずれかまで設計に含める。
    なお §1.30 の「wait は本当に無限か」は、コード上は 1000ms 上限側と整合する
    (= デッドロックより飢餓)。ただし実際に観測された `WaitForSingleObjectEx` がこの wait かは
    `latency_waitable_object` の設定次第なので、live evidence の項目は開いたままにする。
  - 通常の frame drop は `Skipped` と**別の outcome** (`DeferredNotReady` 等) にする。
    通常動作なので warning 対象ではなく、damage の保持と readiness wake が要る。
  - `Idle / Painting / Pending` を global scalar にすると複数 viewport の dirty 状態を失う。
    viewport ID / dirty set / 最新 resize / surface 世代を持たせる。
  - drop 後に即 repost すると spin する。waitable signal / event / 明示的 scheduler wake で再 arm する。
  - `WM_PAINT` を即 return する際の `BeginPaint` / `EndPaint` または validation 規則を決めないと
    無限 `WM_PAINT` になる。
  - immediate viewport は UI callback 中に同期再帰する
    ([wgpu_integration.rs](../vendor/eframe/src/native/wgpu_integration.rs) の nested 経路)。
    フレーム全体を単純に `Painting` にすると detached / immediate viewport を塞ぐので、
    state の適用範囲を定義する。
  - dropped frame でも `textures_delta` は配送する (= §1.86 の契約)。screenshot request は
    成功まで保持 / 再投入する。
  - Windows 回帰テストは 2 つの性質を分ける: (a) `WM_PAINT` / 同期 message handler が有限時間で
    返ること (b) presentation 不能中も外側 render step と message-pump の service latency が有限で
    あること。ハング防止に raw `SendMessage` ではなく `SendMessageTimeout` を使い、実 DWM stall では
    なく決定的な readiness gate を止めて再現する。
- 規模 / 優先度: **残るのは §1.31-B の上限制約 = Large / P1 candidate (基盤)**。
  watchdog の穴 (Small / P2) は上記のとおり解消済み。

### 1.35 リネーム移行ジャーナルの部分失敗が再試行されない

- 出典: v2.9.1 出荷後の他セッションレビュー (2026-08-01、Codex)。source inspection で確認済み。
- 機構 (仕様判断あり): worker が `panicked` を返したときだけ
  ジョブを `rename_migration_boot_retry` へ戻す。`report.errors` に個別ストアの失敗が
  入った場合はトーストを出すものの、ジョブは完了扱いでジャーナルから消える。一時的な
  DB ロックや I/O エラーだと、編集結果やパス依存設定の一部が旧パスに残ったまま再起動でも
  回復しない。
  - **これは現状のコードコメントが明記している意図的な選択**である
    (「per-store エラーは通常経路と同じ best-effort = 再試行しない」、v2.3.0 角度⑤ 検収時の判断)。
    したがって着手前に「一時エラーと恒久エラーを区別できるか」「再試行の上限をどう置くか」を
    決めること。無条件の再試行は、決定的に失敗するストアで無限ループになる。
  - 移行処理自体は冪等に設計されているので、失敗ストアを保持して再試行する形にはできる。
- 規模 / 優先度: Medium / P2 (仕様判断を先に決める)。もう 1 件あった「UI スレッドでの同期 I/O」は
  v2.10.0 で latest-value worker へ移して close 済み。

### 1.37 トレイ常駐再生まわりの所有境界 2 件

- 出典: v2.9.1 出荷後の他セッションレビュー (2026-08-01、ClaudeCode)。source inspection で確認済み。
  v2.9.1 で「トレイ格納中も再生を続ける」経路を新設したことで、いずれも**この版から初めて
  実際に通る**組み合わせになった。
  1. **P3: 再生継続中に video processor cache を落とす**。
     [tray_integration.rs](../src/tray_integration.rs) の `release_gpu_resources` は
     detached 早期 return より前で `release_idle_pools()` を呼び、その中の
     `processor_cache.take()` は**無条件**である (`hw_frames_pool.clear()` と
     `shared_output_pool.retain(in_use)` は refcount 上安全)。
     ⚠ **呼び出し順自体は v0.9.1 (`287eea9f`, 2026-05-19) からで、v2.9.1 で動かしたわけではない**。
     新しいのは「格納中も再生が続く」ことのほうで、その結果この経路が再生中に走るようになった。
     トレイ格納の瞬間に video processor の作り直しが 1 回入るはずで、実害は小さい。
  2. **P3: 外部 hide 検出が tray 所有権に追随していない**。[app.rs](../src/app.rs) の
     `IsWindowVisible` 追従分岐は `viewer_session_is_detached_or_switching()` だけで heartbeat
     suspension を決め、`sync_media_presenter_visibility_for_tray()` /
     `sync_retained_viewport_visibility_for_tray()` を呼ばない (これらは `hide_to_tray` /
     復帰経路にしか無い)。同フレーム後半の `sync_tray_resident_media_wake()` が**値が変化した
     ときだけ**自己修復するので実害は出にくいが、hide の所有者が 2 つある状態である。
- 規模 / 優先度: どちらも Small / P3。3 件目だった「WM_PAINT ブリッジに背圧が無い」は
  v2.10.0 で claim / ack 構造へ変えて close 済み。

### 1.38 ネイティブ名前ダイアログが `App::update` の内側でモーダルを回す

- 出典: v2.9.1 出荷後の他セッションレビュー (2026-08-01、ClaudeCode)。source inspection で確認済み。
- 何が問題か: [rename_item.rs](../src/ui_dialogs/rename_item.rs) の
  `show_rename_dialog_window` は `loop` の中で `native_name_dialog::prompt_name`
  (`DialogBoxIndirectParamW` = 独自のモーダルメッセージループ) を呼び、バリデーション失敗の
  たびに再表示する。`App::update` は `WM_PAINT` → `RedrawRequested` の同期 dispatch から
  呼ばれるので、**wndproc の内側で無制限長のモーダルが回る**。
  §1.31「UI スレッドが自分のメッセージ処理中に任意時間止まる」構造の一種なので、
  **§1.31 の設計を決めるときに一緒に見る**のが筋が良い。
  なお rename 自体は `rename_item_async` で worker 化されており、ここは**ダイアログの
  モーダル span だけ**の問題である。
- **副次的な退行は対応済み (2026-08-01)**: 既存 `rename_pending` を左下の進捗 overlay に接続し、
  シェル rename 実行中の「変更中...」表示を復帰した。モーダル構造は変更していない。
- 規模 / 優先度: Medium / P2 (§1.31 に合流)。残るのはモーダル span の構造だけ。

### 1.39 `fs_page_load_state` が PDF ページで毎フレーム String を作る (`9590b661` の続き)

- 出典: v2.9.1 出荷後の他セッションレビュー (2026-08-01、ClaudeCode)。source inspection で確認済み。
- 機構: `has_retained_pdf_final_ai_for_current_params` → `retained_pdf_page_key_for` →
  `metadata_cache_key(idx)` が String を生成し、HashMap を引く。
  ⚠ **レビュー報告より実コストは小さい**: `effective_params` / `final_ai_key_for_pixels` は
  retained cache に**該当エントリがあるときだけ**走る (無ければ `?` で抜ける)。
  非 PDF は `items.get` + `matches!` で即 return なので無コスト。
  とはいえ PDF フルスクリーンでは `update_prefetch_window` の current、prefetch ターゲット
  全件、連結読み keep 範囲ループ全件、見開きパートナーから毎フレーム走るため、
  key を 1 回作って使い回す形にできるかは見ておく。
- 規模 / 優先度: Small / P3。

### 1.62 お気に入り編集を開いている間、進捗が動いていなくても 100ms ごとに repaint する

- 出典: v2.13.0 の idle health 測定中に判明 (2026-08-11)。退行ではなく既存仕様。
- 観測: `favorites_editor.rs:752` が `ctx.request_repaint_after(100ms)` を**無条件で**呼ぶため、
  ダイアログを開いている間ずっと 11〜12 fps で `App::update` が回る。perf log で
  `prev_frame_causes=['src\\ui_dialogs\\favorites_editor.rs:752']` が連続して確認できる。
- 実害: 利用者は**起動後の索引作成が終わるのを確認するためにこのダイアログを開いたままにする**
  運用をしており、索引作成は 5 分ほどかかる。その間ずっと起きている。ノート PC やタブレットの
  電池には無視できない。v2.13.0 でタッチ対応を入れた以上、タブレット運用は増える。
- 現行コードのコメントは「active が空でも notify-rs が動き出した瞬間に拾えるよう常に呼ぶ」と
  理由を書いており、**意図的**である。直すなら意図を保ったまま頻度を落とす:
  - active が空の間は 100ms ではなく 500ms〜1s へ落とす (動き出しの検出遅れは許容範囲)
  - または watcher 側から `ctx.request_repaint()` を呼び、ポーリング自体をやめる (構造的)
- 同型の確認: 他にも「進捗表示のために無条件 `request_repaint_after`」を持つダイアログが
  無いか探すこと。あれば同じ方針で揃える。
- 規模 / 優先度: Small / P2。

### 1.69 変換対象アーカイブのキャッシュパスが未解決のとき、ZipDir タイルだけ黙って失敗し得る

- 出典: §1.66 の修正 (`ff56abea`) の周辺を洗って見つけた。**利用者報告ではなく、
  実際に到達するかも未確認**。今回の不具合ではなく以前からある形。
- 形: `converted_archive_cache_paths` (= 変換キャッシュ ZIP へのパス表) は worker が
  非同期に作るので、表が空の窓がある。この窓での振る舞いが item 種別ごとに違う。
  - `GridItem::ConvertibleArchive`: `?` で **LoadRequest を作らない** → 後のフレームで
    再要求される。安全。
  - フォルダピン経由: `ff56abea` で「表が届いたら依存タイルだけ組み直す」ようにした。
  - **`GridItem::ZipDir`: `unwrap_or(zip_path)` で元アーカイブのパスへ落ちる**。RAR は
    `zip_loader` が `rar_loader` へ振り分けるので読めるが、**7z / ソリッド RAR は ZIP として
    開こうとして失敗**し、そのサムネイルは失敗のまま残る。
- 到達性: コードのコメントによると、`ZipDir` が元 RAR/7z/LZH のパスを持つのは
  **レーティング一覧に復元されたとき**。実際にその状態を作って確認していない。
- 方針: **まず到達するかを確かめる。** 到達しないなら、3 経路で fallback の作法が違うこと
  自体をコメントで明示して閉じる。到達するなら `ff56abea` の依存追跡
  (`pin_archive_dependencies`) を ZipDir にも広げる。**確認前に直さないこと。**
- 規模 / 優先度: Small / P3。

### 1.79 比較表示中の元画像表示 (押している間だけ両側を元画像にする) — 利用者要望

- 出典: 利用者実機確認 2026-08-13。比較 (ワイプ) 表示中に元画像表示 (`FsOriginalPreviewHold`、
  既定 RightCtrl) が効かない、という報告。**期待する挙動は「ワイプはそのままに、ワイプ元と
  比較対象の両方が元画像になる」** (利用者判断)。片側だけ元画像にしたり、比較をやめて
  元画像 1 枚を出すのは求められていない。
- **現状 (source inspection、2026-08-13)**: 比較表示中は**全モードで**元画像表示が効かない。
  `compare_requested` は `compare_view_mode != Off` だけで決まり、
  `CompareFramePrimaryDraw::resolve` は元画像表示を入力に取らない
  ([ui_fullscreen.rs:11708](../src/ui_fullscreen.rs:11708))。今回の Ctrl まわりの変更以前からこう。
- **実装コストは Medium。小さくない。** 比較の合成は加工済みピクセルから作られており、
  「元画像版の対」を新たに用意する必要がある。
  1. **prepared pair は非同期 worker が作る**。identity は
     `(current_idx, pinned_source_idx, output)` なので、**加工済み / 元画像の次元を足す**必要がある。
  2. **比較対象 (pin した側) は、pin した時点の加工済みピクセルの snapshot** (`slot.pixels`) で
     保持している ([ui_fullscreen.rs:22982](../src/ui_fullscreen.rs:22982))。元画像版は存在しないので、
     **pin 時に未加工の snapshot も持つ (メモリ倍) か、押された時点で再デコードする (遅延)** かの
     選択が要る。再デコード対象は現在ページとは限らず、`fs_cache` に無いこともある。
- 方針の案:
  - 元画像版の対は**押されたときに遅延生成**し、準備できるまでは**加工済みの比較を出したまま**にする
    (途中で別の絵に切り替わるちらつきを作らない)。既存の `WaitingForSource` / 「比較表示を準備中」
    の枠組みに乗る。
  - 一度作った元画像版は、対の identity が変わるまで保持して 2 回目以降を即時にする。
- 着手時に読む: 上記 `ensure_compare_prepared_pair` / `ComparePrepareOutput` / `pinned_compare_slot`。
- 関連: 線を消す修飾は左 Ctrl 限定にしてあるので (`88b028e1`)、既定 RightCtrl の元画像表示とは
  物理的に競合しない。この項目を入れても修飾キーの整理は不要。
- **当面の決着 (2026-08-13、利用者判断、`42212e9d`)**: 実装コストが高いので、
  **比較表示中は元画像表示を明示的に無効**にした。有効なままだと画面は変わらないのに
  「比較表示を準備中」トーストが出るなど副作用だけが起きるため、`original_preview_active`
  の入口で断っている。本項目を実装するときはこの早期 return を外す。
- 規模 / 優先度: Medium / P3 (比較表示は加工結果を見比べる機能で、元画像表示が効かないことが
  データを壊すわけではない)。

### 1.95 OS 側でコピー・移動したファイルへ編集内容を引き継ぐ — 残りは同計画の未着手分

- 正本: [edit-content-identity-plan.md](edit-content-identity-plan.md)。
  Phase 1 (A1-A6) は v3.3.0 で出荷済み。
- **残り**: 同計画の「未着手」節 — アプリ内コピー / ページ移動の同じ失効漏れ、
  復元を通っていないファイルの注釈サムネイル。
- 規模 / 優先度: 小〜中 / P3 (実害は余計な確認ダイアログ。データは失われない)。

### 1.105 動画のフルスクリーン上部バーにも「…」オーバーフローを出す — 利用者要望

- 出典: 利用者要望 (2026-08)。静止画のフルスクリーン右上に「…」を追加したので、動画にも
  同じものがあると UI として揃う、という指摘。
- 現状: [ui_fullscreen.rs](../src/ui_fullscreen.rs) の `overflow_available` が
  `!state.is_video && !is_music_view && !panorama_mode_active_now && ...` で、
  **画像のときだけ**「…」を出す。動画・音楽・パノラマでは出ない。
- 仕様の論点: 動画の上部バーには既に動画専用ボタン (タイル一覧 / Perf グラフ 等) が並ぶ。
  「…」に何を入れるかを決める必要がある。
- **1.104 とは切り離す (2026-08-20 決定)**。「ズレ許容」をフルスクリーン中から変えられると
  体感を確かめながら調整できるが、動画側の上部バーへ overflow を通す修正量が大きい。
  1.104 の初回リリースには**含めない**。設定は環境設定からのみ変更できる形で出す。
  報告者へもこの件は伝えていないので、実施を約束したものとして扱わない。
- 規模 / 優先度: Medium / P3。

### 1.108 開くときは黒地、切り替えるときは前の画像 — 表示先の占有を routing 境界で判定する

- 出典: 2026-08-20 の利用者報告 (グリッドで PDF をダブルクリックすると、1 ページ目が遅いときに
  無反応に見える) と、それに続く仕様整理。
- **確定した規則 (利用者判断)**: **表示先に既に中身があるか**で分ける。
  - **A (中身が無い)**: 新しいウィンドウが開く / F12 デタッチで新規窓 / 同一ウィンドウ内で
    ファイルを開く → **黒地を挟んでフルスクリーンへ移る**。開く操作の結果が即座に返る
  - **B (中身がある)**: Ctrl+↑↓ / メイン側から既存 detached を切り替え → **前の画像を保持**。
    黒を挟むとちらつく
  - どちらも待ちが 500ms を超えたら中央に「読み込み中」(v3.1.3 で実装済み)
- **却下された導出**: `FsNavigationSequence::previous.is_none()` では分離できない。詳細と
  全経路の分類は [docs/briefs/fs-open-black-vs-holdover.md](briefs/fs-open-black-vs-holdover.md)
  の「第 1 段階の結果」。要点:
  - `previous` は「直前の表示単位を texture として捕捉できたか」であり、表示先の占有ではない
  - **A の production 経路のほとんどに navigation sequence が無い** (`open_fullscreen` を直接通る)
  - **B の多くにも sequence が無い** (Home/End、スタック Shift+↑↓、連結読みのシーク、
    スライドショー、native 動画の前後移動、passive snapshot クリック、remote 復元 等)
  - 反例: グリッド由来・`from_explicit_open == true` でも、linked detached の既存窓を
    更新するなら B
- **正しい判定地点**: teardown 後の `open_fullscreen` ではなく、**表示先の surface / context を
  選ぶ routing 境界**。`ViewerPresentation`、viewer session、detached runtime state、
  mounted bundle の `fullscreen_idx`、native presenter の owner を見る必要がある。
- **リワーク R2 (状態の集約) が所有する領域と重なる。**独立した作業として着手しない。
  `previous.is_none()` を viewport 側へ足す実装は §2 の症状パッチに当たる (Codex 判定)。
  条件を増やす形の部分対応も、単一の所有者を持たない routing 判断へ条件を足すことになるため
  採らない (利用者判断)。
- 現状: 中央の「読み込み中」により**無反応に見える問題は解消済み**。残るのは
  「グリッドが見えたまま待つ (黒地にならない)」という見た目の差のみ。
- 規模 / 優先度: 中〜大 / P3 (実害は見た目。R2 で routing の所有者が決まってから)。

### 1.110 別ウィンドウの題が、表示ページ不定のときに一覧の先頭ファイル名になる

- 出典: 2026-08-20、keep-alive backstop の所有権修正 (`detached-rework-plan.md` §11) の
  フェーズ 1 調査で Codex が指摘。**その修正とは別件なので、同時に直さず切り離した** (憲法 7)。
- 機構: detached の viewport builder は題の index を
  `self.fullscreen_idx.unwrap_or(0)` で決める ([ui_fullscreen.rs:12791](../src/ui_fullscreen.rs:12791))。
  **表示ページが定まっていない正規のギャップ** (列挙待ち、パスワード待ち、フォルダ移動の途中など) では
  `None` になり、**その一覧の先頭ファイル名**が題になる。
- 症状: 一瞬だけ、開いているものと無関係なファイル名がタイトルバーに出る。
  backstop の所有権修正で「別 context の名前が出る」方は消えたが、**こちらは残る**
  (同じ context の中で、たまたま先頭のファイル名が出る)。
- 直す方向: **`unwrap_or(0)` をやめる。** 表示ページが定まっていないことは「先頭ページ」ではない。
  ギャップ中は直前に表示していた題を保つか、アプリ名だけにするかを決める。
  **`Option` を握りつぶさずに、ギャップを型で表す**のが筋。
- 同型の入口: 静止画スナップショット窓の題
  ([app.rs:40534](../src/app.rs:40534) 付近の `detached_image_snapshot_title_for_idx`) も
  `items.get(idx)` が空振りしたとき `"mimageviewer"` に落ちる。こちらは先頭へ落ちないので
  挙動は穏当だが、**ギャップの表し方を揃えるなら一緒に見る**。
- 規模 / 優先度: 小 / P3 (実害は一瞬の見た目。所有権の方が先)。

### 1.111 フルスクリーンで動画へ入る瞬間に前面を失い、押しっぱなしのキーが他アプリへ流れる — 利用者報告

- 出典: 利用者報告 (2026-08-21、v3.1.2 で確認)。
  - 全画面モードで静止画を表示し、**上下キーを押しっぱなしで高速送り**している最中、
    次が動画だと**一瞬ウィンドウが消え、フォーカスが別のウィンドウへ移り**、押しっぱなしの
    上下キーがそのウィンドウへ入力される。
  - **ウィンドウモード / 別ウィンドウモード / 別ウィンドウのフルスクリーンでは起きない。**
  - 併発: ランダムに**カーソルが mIV 上で非表示のまま**になる。次の画像 / 動画へ移ると戻る。
- 原因の見当 (コード確認済み、未確定):
  - フルスクリーン表示では静止画が **egui フルスクリーンビューポート**、動画が
    **別 HWND の native presenter** (D3D11 + DirectComposition)。静止画→動画の遷移で
    前者を `ViewportCommand::Visible(false)` で隠し ([ui_fullscreen.rs:12739](../src/ui_fullscreen.rs:12739))、
    後者を別途 materialize する。
  - **この症状は既に疑われていて計装がある**。同じ場所の直前に
    `log_main_flash_probe("fs_visible_false", …)` ([ui_fullscreen.rs:12735](../src/ui_fullscreen.rs:12735))。
  - [native_video.rs:2842](../src/app/native_video.rs:2842) のコメントが症状そのものを書いている:
    「foreground 奪還まで 80ms 待つと、**その間だけ外部ウィンドウが見えることがある**」。
  - つまり **viewport を隠してから presenter が foreground を取るまで、mIV のどのウィンドウも
    前面を持っていない**。Windows は z-order 上の次のウィンドウへ前面を渡すので、キーリピートが
    そちらへ流れる。報告と一致する。
  - 他モードで起きない説明も付く。ウィンドウ内動画はメインウィンドウの子として扱われ
    (`set_in_window_video_child`、[native_window_host.rs:536](../src/video/native_window_host.rs:536))、
    専用フルスクリーンビューポートと presenter HWND を入れ替える構造になっていない。
  - **カーソル非表示の固着も同根**。`cursor_hidden` はラッチで、解除は (a) egui フルスクリーン
    フレーム内で観測したポインタ操作、(b) HUD が出て `clean` が false になったとき、の 2 つだけ。
    presenter 側は `update_cursor_icon` で別経路の解決をしており、**カーソルの所有者が 2 つある**。
    次の項目へ移ると直るのは、そこで `open_fullscreen` がラッチを落とすため。
- **2026-08-27 追試: 再現しなかった (利用者、v3.3.0 開発版)。** 単一ウィンドウの通常
  フルスクリーン、動画は全画面再生、背後にエディタを置いて ↓ 押しっぱなしで高速送り。
  **動画の再生がすぐ始まり、背後のエディタへキーは 1 つも入らなかった。**
  Windows のキーリピートは初回遅延の後およそ 30ms 間隔なので、1 つも漏れないということは
  前面の空白がほぼ無いことを意味する。
  - main window の cloaking (`native_video_main_cloaked`) は `f2215fd2` (2026-05-13) で
    **v3.1.2 に既に入っている**ので、これが後から効いたわけではない。
  - 未確定なもの: (a) v3.1.2 以降のどこかで解消した (b) より遅く開く動画 (大きい 4K/HEVC、
    低速ドライブ、起動後 1 本目) が必要 (c) 報告者の環境依存。
  - **確かめる手段は既にある**。`MIV_DETACHED_WINDOW_DEBUG=1` で起動すると
    `main_flash_probe stage=fs_visible_false` が窓の状態ごと出る
    ([app.rs](../src/app.rs) `log_main_flash_probe`)。抑制条件は
    `fullscreen_idx.is_some()` を含む広いものなので、この経路なら必ず出る。
    1 回の再現で前面の空白の有無が確定する。
  - **既知の問題ページには載せない** (見せられないものは載せない)。再現を取れたら載せる。
- **着手条件: 複数ウィンドウ / キー入力所有権の整理 (別 worktree、`docs/briefs/modifier-ownership-design.md`) の完了待ち。**
  症状パッチ (遷移前後の追加 `SetForegroundWindow`、遅延、キーリピート抑止) を入れない。
  問われているのは「viewer の遷移中、どのウィンドウが前面と入力とカーソルを所有するか」であり、
  1.100 や過去のキー所有権報告と同型。整理後に、遷移を**所有権の受け渡しが切れない 1 手**として
  設計し直す。
- 回帰確認の観点: 静止画→動画・動画→静止画の双方向、キー押しっぱなし中と単発、
  4 表示モード (フルスクリーン / ウィンドウ / 別ウィンドウ / 別ウィンドウのフルスクリーン)、
  遷移後のカーソル可視状態。
- 規模 / 優先度: Medium / P1 (実害あり)。**整理待ち**。
- **v3.2.0 からは外すと確定 (利用者判断 2026-08-22)。** 着手条件 (キー入力所有権の整理) が
  未達で、症状パッチを入れれば整理時に解きほぐす手間が増えるため。次リリースの筆頭候補として残す。

### 1.114 PDF の表示解像度合わせが毎フレーム自分を殺し合ってライブロックする

- 出典: 2026-08-21 の実機確認中に遭遇 (別ウィンドウで PDF の先頭 / 末尾へジャンプ)。
  症状は左下の「PDF 再レンダリング中...」が消えず、そのページの高解像度化が永久に完了しない。
- **今回の detached 修正が原因ではない。** 同じループが `mimageviewer.log.bak`
  (2026-08-20 12:09、detached 作業の着手前) に **2051 回**記録されている。
  別の PDF / 別の日 / 同じ機構。今日のログでは 93 秒間で **15883 回**。
- **機構 (ログで確定)**:
  1. ページを読み込み、保持ページキャッシュへ `long=1179` で格納する
  2. 表示解像度合わせが `target_px=1702` を要求 → キャッシュは 1179 なので
     `reason=request_pdf_rerender/resolution_mismatch` で miss
  3. [`request_pdf_rerender_at_target`](../src/app.rs:51535) が
     **進行中の再レンダを無条件にキャンセルして**新しいリクエストを投げる
     (「既に同じ (idx, target_px) が飛んでいるか」を見ていない)
  4. 呼び出し元の `ensure_pdf_display_resolution`
     ([ui_fullscreen.rs:13897](../src/ui_fullscreen.rs:13897) / [:13906](../src/ui_fullscreen.rs:13906)) は
     **毎フレーム**評価される
- PDF のレンダは数百 ms かかるのに、リクエストは約 6ms ごとに投げ直される。
  **どのリクエストも完了する前に次に殺される。**
  ログの裏付け: `pdf rerender done` が **1 回も出ておらず**、
  spawn された全スレッドが `pdf rerender: cancelled (channel closed)` で終わっている。
- **修正方針**: 「同じ `(idx, target_px)` のリクエストが既に in-flight なら投げ直さない」判定を入れる。
  時間窓ではなく、**進行中リクエストの identity** で判定する。
  in-flight を無条件に潰す現在の実装 ([app.rs:51535](../src/app.rs:51535) 付近) がライブロックの直接原因。
  - 副次的に確認すること: 1 回でも再レンダが完了すれば miss が解消するのか。
    今回は一度も完了していないため、要求解像度 1702 を満たす結果が返るかは**未確認**。
    dedup を入れた後に、完了した結果の long がまだ target を下回るなら別の原因が残っている。
- **⚠ 着手前に調整が要る**: 2026-08-20〜21 に master 側で PDF 周辺が活発に動いている
  (`f2c88357` 同時 open 数の上限 / `c817a0d3` 列挙の相乗り / §2.13〜§2.16)。
  同じ領域を触る別作業と衝突しないか確認してから着手する。
- 規模 / 優先度: Small / P1 (利用者の目に見える固着)。**別の PDF 修正が落ち着いた後に再確認する** (利用者判断 2026-08-21)。

### 1.118 任意の実ファイルを参照だけで束ねる「コレクション」 — 外部SNSでの移行検討者の指摘

- **2026-09-12追加要件**: 順序付き集合として使えるようにし、順序なしも選択可能にする。
  切替UI、手動順と通常ソートの関係、順序なしへ切り替えた後に手動順を復元する扱いは設計時に検討する。
  同一項目の重複登録はしない。import/exportの行順との関係も明示する。
  スライドショーと動画の連続再生を想定し、次/前や自動送りはコレクション内の対象を移動する。
  元ファイルが置かれたフォルダへ再生対象が逸れないよう、閲覧・再生の所有contextを揃える。
  フォルダ項目の展開範囲・終端/ループ・再生中の編集と順序切替は実装設計時の確認対象。
  着手は [直列開発計画](post-v3.9.0-sequential-work.md) の§1.220までが終わった後の候補。
- 出典: 2026-08-24。ZipPla / NeeView からの移行に「仮想ディレクトリ」が不足するとの外部SNS投稿。
  直接受けた要望ではなく、相手が期待する詳細操作は未確認。ZipPla の仮想フォルダや NeeView の
  プレイリストに相当する、手動で選んだ分散ファイルの参照一覧を想定して候補化する。
- **既存機能との差**:
  - スマートフォルダは複数 root と条件から毎回結果を作るため、ZipPla の「スマートフォルダ」相当。
  - 製本は追加時点のページを実ファイルとしてコピー / 焼き込みするため、元ファイルへの参照一覧ではない。
  - 任意項目を手動登録し、元の場所に置いたまま1フォルダのように見せる機能は無い。
- MVP 案:
  - 内部名も ZIP / PDF の `virtual folder` と衝突させず、利用者向け名称を「コレクション」とする。
  - `TopLevelGridSurface::Collection` を追加し、専用 DB worker が
    `CollectionDefinition` / `CollectionEntry { id, collection_id, source_path, order }` を所有する。
    設定 JSON の巨大配列や UI thread の同期存在確認にはしない。
  - 対象はまず実フォルダ / 実ファイル / アーカイブ本体のみ。コレクション入れ子と ZIP / PDF 内の
    仮想ページは対象外。追加、手動順序、コレクションから外す、元の場所を開くを提供する。
  - 「コレクションから外す」と「元ファイルをごみ箱へ移す」を明確に別操作・別確認にする。
- **テキストのインポート / エクスポートを初期実装に含める**（2026-09-12利用者指定）:
  - フォルダ / ファイル一覧をテキストで用意して使いたいとの要望が複数ある。
    1行1件のパスを扱うUTF-8テキスト（BOMあり/なし、CRLF/LF）を基本形式とし、行順を保持する。
    ファイルとフォルダを混在でき、フォルダの内容はインポート時に再帰展開しない。
  - 絶対パス、および一覧テキストの保存場所を基準にした相対パスを受け付ける。
    空行は無視し、Windowsの「パスのコピー」に対応して行全体を囲む二重引用符を許容する。
    環境変数・ワイルドカード・コマンドは展開せず、URLやデバイスパスは受け付けない。
    `#`等はパス名として扱い、暗黙のコメント構文は設けない。
  - インポートは専用操作から行い、追加先（新規 / 既存）、件数、解決後のパス、形式不正の行を
    確認してから参照を登録する。同一パスの重複は先頭を残し、除外件数を表示する。
    存在しない項目はmissingとして保持し、勝手に別ファイルへ結び付けない。
  - 確認前のプレビューは文字列の解析に留め、存在確認・サムネイル生成・リンク解決などの
    対象パスへのI/Oを行わない。UNC等のネットワーク参照は明示し、対象へのアクセス了承を得てから扱う。
    drive mappingやreparse point経由の参照も含め、アクセス境界は実装設計時に確認する。
    リストを開いただけの自動登録・自動実行はしない。登録後も既存の対応形式と閲覧権限を維持する。
  - 解析・登録・存在確認はworkerで行い、大きい一覧にも進捗と取消を提供する。
    元ファイルのコピー・移動・変更は行わず、コレクション参照だけを追加する。
  - エクスポートはコレクション順の絶対パスを同じ1行1件形式で出力し、missing項目も保持する。
    これは参照一覧の受け渡しであり、元ファイルやタグ・補正設定等のバックアップではない。
  - 入出力の往復で対象と順序を維持する回帰、日本語・空白・引用符・相対パス・重複・missing、
    ネットワーク参照の確認前非アクセス、大量入力・取消を検証する。
- identity / lifecycle:
  - mIV 内の rename / move は既存 path migration の同じ transaction から entry を更新する。
  - OS 側で移動された項目は推測で別ファイルへ結び付けず、missing 表示と削除 / 再リンクを提供する。
  - 外部リストは上記の明示importでDBへ取り込み、元のテキストの外部編集には自動追従しない。
- 規模 / 優先度: 物理項目だけの MVP Medium〜Large (1〜2週間)、仮想ページ・移動追跡・完全な
  D&D 管理まで Large (2〜4週間) / P3。着手前に利用者が期待する「仮想ディレクトリ」の操作例を確認する。
  上記は入出力追加前の概算であり、初期実装にはimport/exportのUI・アクセス境界・回帰分も含めて再見積もりする。

### 1.122 F12 で動画をメイン ⇄ 別ウィンドウへ往復させると重い — 主因は解消、残り 1 件

- 主因 2 つ (16ms pump のイベント駆動化、`egui_overlay` の wgpu device 共有) は
  v3.3.0 で出荷済み・実機確認済み。switch total 231.8 → 198.1ms、動画を開いている間の
  UI フレーム 121.7/秒 → 48.0/秒。詳細な計測記録はコミット履歴を参照。
- **残り: `overlay_first_render` (37.2ms)**。overlay ごとに新しい `egui::Context` を
  作るので font atlas を毎回建て直している。次に見るならここか `publish` (`SetFocus`)。
- `d3d11_device` の共有は**見送り確定**。1 device に immediate context は 1 本しか作れず、
  `Present` まで同じ submission owner に寄せる必要がある。共有 `GpuVideoDevice` で
  immediate context 競合による hard-stuck を既に経験済みなので、26ms のために戻らない。
- ⚠ **guard / delay / retry で待ち時間を隠さない。**
- 規模 / 優先度: Medium / P3 (残りは体感上実用域)。

### 1.123 スレッド終了中の heap 例外を、例外ハンドラ自身が二次クラッシュで握り潰す — 実機クラッシュ

- 出典: 2026-08-25、R2e ステージ④の smoke 準備中。タグを付けて上のフォルダへ移動した直後に
  `mimageviewer-core.exe` が **0xc0000005 で異常終了**。`panic.log` に記録なし
  (= Rust panic ではない)。**R2e とは無関係の既存不具合**
  ([logger.rs](../src/logger.rs) も [lib.rs](../src/lib.rs) の handler も
  このブランチでは 1 行も変更していない)。
- **WER のフルダンプから取得したスタック** (`cdb -z <dump> -c ".ecxr; kn"`)。下から読む:

  ```
  0b ntdll!RtlUserThreadStart
  0a kernel32!BaseThreadInitThunk
  09 ntdll!RtlExitUserThread          ← スレッドが終了処理に入っている
  08 ntdll!LdrShutdownThread          ← DLL_THREAD_DETACH / TLS デストラクタ実行中
  07 ntdll!LdrShutdownThread
  06 ntdll!RtlFreeHeap+0x726          ← ここで一次例外が発生
  05 ntdll!KiUserExceptionDispatcher
  02 mimageviewer_core!mimageviewer::native_exception_handler+0xa0   ← 自前ハンドラが走る
  01 mimageviewer_core!mimageviewer::logger::current_thread_id_num+0x19
  00 mimageviewer_core!std::thread::current::current+0xf   ← 二次 AV (rcx=0)
  ```

- **問題は 2 つある。**

  **(A) 一次例外**: スレッド終了中の `RtlFreeHeap` で例外。ヒープの不整合か、
  TLS デストラクタ順序に依存した解放。直前のログは**フォルダ移動でワーカー 12 本を
  停止して 12 本を起動**した箇所 (`w0 stopped` … `spawning 10 regular + 2 I/O workers`)。

  **(B) 自前ハンドラが、その文脈で走れない**:
  [lib.rs:693](../src/lib.rs:693) の `native_exception_handler` は
  `logger::current_thread_id_num()` ([logger.rs:272](../src/logger.rs:272)) を呼び、
  その中の `std::thread::current()` が **TLS 依存**である。
  `LdrShutdownThread` 以降は TLS が破棄済みなので `rcx=0` を deref して二次 AV になる。
  **ハンドラが自分で死ぬので `panic.log` に何も残らない。**

- ⚠ **(B) を直しても (A) は直らない。だが (B) を直さないと (A) は永久に見えない。**
  例外ハンドラが証拠ごと消していたため、**(B) を先に直した。**
- **(B) は修正済み** (2026-08-25、`7e16fadde`、v3.3.0)。`logger::current_thread_id_num()` は
  Windows では `GetCurrentThreadId()` を直接呼ぶ (`std::thread::current()` は非 Windows 枝だけ)。
  ハンドラが TLS に触れて自分で死ぬことはなくなり、(A) が起きればログに残る。
- **(B) の残り**: ハンドラ経路はまだヒープを使う。[lib.rs](../src/lib.rs) の
  `native_exception_handler` は `format!` と `append_panic_log_entry` を通り、同ファイルの
  コメントがこれを残存リスクとして自認している。ヒープ破損の最中に走る以上、
  **固定バッファ + `write!` + 起動時に開いた handle へ書く**のが本来の姿。
- (A) の手掛かり: 一次例外は `RtlFreeHeap` の中。`Application Verifier` の page heap か
  `_NT_GLOBAL_FLAG` の heap tail checking を有効にして再現すると、壊した側が特定できる。
  ダンプは `C:\Users\<user>\AppData\Local\CrashDumps\` に残る (今回は 59.6 GB)。
- 規模 / 優先度: Small (B) / Medium (A)。**P2** — 通常操作 (タグ付け → フォルダ移動) で落ちた。

### 1.128 ★固定で範囲外になった別窓を、閉じず自前の一覧へ切り出す — 仕様提案

- 出典: 2026-08-26、§1.125 の実機確認。複数ウィンドウモードで動画再生中に
  ★固定を押し、**そのファイルが固定範囲外だったため動画窓が閉じた**。
  **§1.125 の設計どおりの動作**であり、不具合ではない。ただし体験としては
  **グリッドのボタンを押したら別の窓が消えた**に見える。
- 利用者の希望: **再生はなるべく継続**し、前後移動も納得できる形で残す。

#### 前提の確認 (2026-08-26、コードで裏取り済み)

「★固定していないときは実 FS 順で前後移動する」という想定は **実装と違う**。

```rust
fn build_nav_indices(items: &[GridItem], visible_indices: &[usize]) -> Vec<usize>
```

`get_nav_indices()` は `current_grid_order()` (= `visible_indices`、Details なら `details_order`)
を渡す ([ui_fullscreen.rs:7621](../src/ui_fullscreen.rs:7621) /
[app.rs:46141](../src/app.rs:46141))。つまり **★固定の有無にかかわらず、
前後移動は常にグリッドの表示順・絞り込み結果を辿る**。実 FS 順を辿るモードは存在しない。

したがって ★固定は「別の並びに切り替える」操作ではなく、
**その時点の並びを凍結する**操作である。

#### 提案 — 別窓を自前の context へ切り出す

現状、複数ウィンドウモードの動画窓は **同じ context を別の窓で描いている**
(実機ログ: `main_fs_idx=Some(121) mounted=true`)。だから親のグリッド操作が直撃する。

**器は既にある**:

- 2026-08-26 に `snapshot` を `ViewerContextBundle` の field にしたので、
  **context ごとに別の並び・別の凍結状態を持てる**
- `fork_mounted_live_media_context` が「再生中メディアを自前の context へ切り出す」既存の仕組み

よって §1.125 の miss 経路を「閉じる」から「**切り出す**」へ変えることで、
窓は自分の items を持ち続け、再生も前後移動も維持される。

#### 詰めるべき点

- 切り出した context の一覧は **固定前の並び** を保持する。それを利用者にどう示すか
  (窓のタイトル / HUD に何か出すか、無言でよいか)
- 親が ★固定を**解除**したとき、切り出した窓を親へ戻すのか、独立のままにするのか
- 切り出した context の寿命 (窓を閉じるまで / 親のフォルダ移動まで)
- 静止画でも同じにするか、メディア限定にするか
- ⚠ detached 述語に触れるので §2 の手続きが必要

- 規模 \\ 優先度: Medium / P3 (不具合ではなく仕様改善)。

### 1.135 動画レンダースレッドがパニックすると、以後どの動画も再生できなくなる

- 出典: 2026-08-27 実機。シークバーのストリップを開いた直後に
  `native video render fault: native video render thread panicked` が出て、**以後どの動画を
  開いても同じエラー**になった。アプリを再起動するまで戻らない。
- **直接の原因になったパニックは §1.112 の作業中に直した** (波形テクスチャ幅が GPU 上限を
  超えていた。`WaveSpanRequest::pixel_width` に上限が無く 4K 幅 × 3 = 11520px を要求していた)。
  **本項はその先の構造の話**: パニックの原因が何であれ、一度死んだら復旧しないこと。
- 観測できた事実:
  - panic.log に**同じ形の `Device::create_texture` パニックが 2026-08-10 (8320px) と
    08-14 (9000px) にも残っている**。つまり以前から起きており、そのたびに再生不能に
    なっていたはず。利用者が「動画が再生できない」と気付いた時点では、原因のストリップ
    操作から離れているので、結び付けにくい。
  - スレッドは `catch_unwind` で包まれており ([video/mod.rs:1922](../src/video/mod.rs))、
    パニック自体は捕まえてログに残る。しかし**その後スレッドを作り直さない**ので、
    presenter が居ないまま `native video render fault` を返し続ける。
- 判断が要る点:
  1. **作り直してよいのか。** パニックの原因が GPU リソースの不整合だと、作り直しても
    同じ場所で落ちてループする。無条件の再生成は「症状パッチ」になり得る。
  2. 作り直すなら**回数を制限し、理由を型で持つ**必要がある (`VideoScaleFallbackReason` と
    同じ作法)。何度目で諦めるか、諦めたら何を見せるかを決める。
  3. **少なくとも利用者への見せ方は変えられる。** 現在の「動画を再生できません:
    native video render fault」は内部語で、再起動すれば直ることが伝わらない。
- 参考: 静止画側には同種の「GPU 経路が死んだら次のフレームで作り直す」機構が無いので、
  前例として使えるものは無い。
- 規模 / 優先度: Medium / P2 (実害はデータ喪失ではないが、**再起動するまで動画機能が
  丸ごと死ぬ**。原因側を潰しても別のパニックで再発し得る)。

### 1.134 common modal がツールバー / メニューバーのポインターナビを止めていない — 未対応

- 出典: 2026-08-27、§1.133 の調査中に Codex が発見。**archive convert 固有ではなく、
  全 common-modal window に関わる入力ゲートの不統一**。

`common_modal_dialog_open` に登録された window は、キーボードショートカット /
グリッド open / 背面 wheel / フルスクリーン入力を止めるが、**ツールバーの
お気に入りボタンなどのポインターナビは止まらない** (`any_dialog_open()` で
無効化されていない)。

変換進捗ウィンドウ表示中でもお気に入りクリックでナビできるのはこれが原因。
これ自体は §1.133 の修正対象ではない (preflight が拒否するので安全) が、
**モーダルの意味が入力種別で違う**のは設計として不揃い。

⚠ `archive_convert` を `archive_convert.is_some()` で一括登録するのは**誤り**。
意図的に非表示にしている `Scanning` 中まで操作不能になる。現行の
`archive_convert_dialog_visible()` 経由の登録が正しい。

- 規模 \\ 優先度: Medium / P3 (出荷ブロッカーではない)。

### 1.136 detached placement のテストが、実機のモニター配置に依存して揺れる — 未対応

- 出典: 2026-08-27、video-strip セッションがちらつき調査中に見つけ、
  `docs/detached-close-flicker-handoff.md` §8 で引き継いだもの。同資料は「壊れているのでは
  なく揺れている」と正しく区別した上で、**何が揺らしているかは未特定**としていた。

```
app::tests::still_window_mode_key_tests::detached_builder_placement_latch_does_not_follow_live_drag_updates
```

実行ごとに結果が変わる (単独実行で FAILED → 直後に 3 回連続 ok、フル実行でも両方あり)。
失敗時の値は `{x:80, y:80, w:960, h:720}` = 設定既定。

#### 原因 (2026-08-27 特定)

**共有状態でも実行順でもなく、テストが実機のディスプレイ構成を見ている。**

`set_detached_window_runtime_placement` は `detached_window_runtime_placement_is_usable` を通らない
placement を **無言で return** する ([app.rs](../src/app.rs) `runtime_placement_rejected` は
debug ログのみ)。その判定は:

```rust
placement.is_sane() && crate::monitor::title_bar_on_some_monitor(placement.x, placement.y, placement.w)
```

`title_bar_on_some_monitor` ([monitor.rs:61](../src/monitor.rs:61)) は **`MonitorFromPoint` を
`MONITOR_DEFAULTTONULL` で直接呼ぶ**。テスト用の差し替えは無い。

テストの `initial` は `x:1564, y:240.66667, w:1167` なので、検査点は
**(2147, 255)** 。ここがどのモニターにも乗らない瞬間 (ディスプレイのスリープ /
構成変更 / DPI 変更 / ロックなど) に実行されると placement が保存されず、
`active_detached_seed_placement` の `unwrap_or_else` が設定既定へ落ちる。
**観測された失敗値と一致する。**

#### 直す方向

- latch の振る舞いを見るテストが OS 問い合わせを通るのが問題。
  `title_bar_on_some_monitor` にテスト override を与えるか、placement 検証を注入可能にする。
- **無言の拒否を残さない**。`set_detached_window_runtime_placement` の拒否は debug ログだけなので、
  探す側には何も見えない。型で返すか、少なくともテストから見える形にする。
- 同形の依存が他のテストにも無いかを見る (`title_bar_on_some_monitor` /
  `MonitorFromPoint` / モニター列挙を通る検証全般)。

- 規模 \\ 優先度: Small ～ Medium / P2 (出荷ブロッカーではないが、
  **リリース前の全体テストが理由なく赤くなる**ので早めに閉じる)。

### 1.141 Susie のクラッシュ対象 ID が「エントリ名 + 長さ」止まり

- 出典: 2026-08-27 の出荷前レビュー (Codex、機能別評価の Susie 行)。
- 機構: [susie_loader.rs](../src/susie_loader.rs) の `decode_bytes` は、クラッシュした
  入力を二度と同じプラグインへ渡さないための識別子を
  `format!("{filename_hint}#{}", bytes.len())` で作る。ZIP 内画像はパスを持たないため、
  名前だけでは別書庫の同名エントリを巻き添えにする — その対策として長さを足した経緯が
  コメントに残っている。
- 残る穴: **同名かつ同サイズで中身が違う**エントリは、まだ同一視される。
  片方でプラグインが落ちると、もう片方も開かれなくなる。
- 実害の大きさ: 落ちるのは既にクラッシュした後の話で、結果は「開けたはずの 1 枚が
  開かれない」。クラッシュや破損ではない。**同名同サイズは再配布された同一ファイルである
  ことが多く、その場合は巻き添えではなく正しい判断**になる。
- 直し方の候補: バイト列のハッシュを識別子に含める。全体ハッシュは decode 経路 (数 MB) に
  数ミリ秒を足す。先頭 + 末尾の固定長だけを混ぜる案なら実質ゼロだが、**どちらも実際の
  クラッシュを再現できないと検証できない**。
- 2026-08-27 の判断: **出荷前には入れない。** 失敗は限定的で自己修復可能 (再起動で解除)、
  一方で修正は decode のホットパスに触れ、実機で確かめる手段が無い。
- 規模 / 優先度: 小 / **P3**。

### 1.140 超広幅ウィンドウで波形ストリップの鮮鋭さが落ちる (分割描画の検討)

- 出典: 2026-08-27 の出荷前レビュー (Codex P2) をきっかけに判明。
- 経緯: `WaveSpanRequest::pixel_width` は「可視幅そのものが GPU 上限 (8192px) を超えても
  丸めない」を明示的な設計としており、コメントは**「テクスチャ生成側の責務として残す」**と
  書いていた。しかし [render_core.rs](../src/video/native_presenter/render_core.rs) の
  `sync_seek_strip_textures` は分割も縮小もせず `load_texture` を呼ぶだけで、
  **その責務は実装されないまま残っていた**。到達すれば必ずレンダースレッドが
  パニックし、以後どの動画も再生できなくなる (§1.135 と同じ終わり方)。
- 2026-08-27 の対応: 上限を要求側で無条件に効かせた
  (`an_oversized_visible_width_is_capped_at_the_texture_ceiling`)。
  **パニックは消えるが、可視幅が 8192px を超える構成では波形がわずかにぼやける** (伸縮のため)。
  位置は時刻由来の UV で決まるのでずれない。
- 残っていること: 鮮鋭さを取り戻すなら**分割描画**しかない。RGBA を N 枚のテクスチャへ分け、
  `waveform_texture_slice` の UV をタイル境界で割り、N 回 `painter.image` する。
  継ぎ目と、raster revision ごとの N 枚アップロードのコストを見る必要がある。
- 到達条件: 可視ストリップ幅 (物理ピクセル) が 8192 を超える構成。4K 2 面 (7680) では届かず、
  3 面またぎや 8K で届く。**開発機の仮想デスクトップ幅は 6001px なので、ここでは再現しない。**
- 規模 / 優先度: 中 / **P3** (パニックは解消済み。残るのは限られた構成での画質)。

### 1.169 360 を保持したまま通常画像を経由すると、衝突モードが同時に開く (2026-09-02)

**症状**: フォルダ内で 360 画像 → 通常画像 → 360 画像とめくると、通常画像のところで
補正パネルや分析モードを開いていても、360 画像へ戻った時点でそれらが開いたまま 360 が
表示される。<kbd>V</kbd> で入り直せば衝突モードは閉じる。

**原因**: 360 の state は「保持しつつ非アクティブ化」する設計
([panorama-360-view-plan.md §6.3](panorama-360-view-plan.md))。`panorama_state` は残り、
`is_panorama_mode_active(fs_idx)` が素材ごとに有効 / 無効を切り替える。**再アクティブ化は
state の有無だけで決まり、`toggle_panorama_mode` の ON 経路 (= 衝突モードを止める後始末)
を通らない**。通常画像の側では 360 が非アクティブなので、抑止 gate も開いている。

**360 のセッション意図 (旧 §1.145、v3.5.0 で実装) より前からある挙動**で、その復帰経路
(`reconcile_panorama_session_intent`) は
V キーと同じ入口・同じ後始末を通るので、こちらとは別の話。Codex レビューが同型の経路として
指摘した (2026-09-02)。

**決めること**: 「360 が有効になる瞬間」を state 生成時ではなく**アクティブ化時**に移すか
(= 保持設計の作り直し)、通常画像側で開ける物を制限するか。前者が構造的に正しいが、
`is_panorama_mode_active` の呼び出し全域に影響する。**症状パッチ (再アクティブ化を検出して
その場でパネルを閉じる guard) は入れない** — 同じ判定が 2 か所に生まれる。

規模 / 優先度: 中 / P3。目に見えるが、V で入り直せば直る。

### 1.170 切り離した動画窓のヘルプが、メインウィンドウに出る (2026-09-02)

**症状**: F12 で切り離した動画ウィンドウで `?` (ショートカット一覧) を押すと、**メイン
ウィンドウ側にヘルプが表示される**。押した窓には出ない。利用者報告 (2026-09-02、右情報パネルのロック (旧 §1.158) の
実機確認中に発見)。

**原因は未特定**。調べる人向けの取っかかりだけ残す:

- native presenter はヘルプを自分で持っている (`shortcut_help_open`、中央モーダル)。
  [docs/video-architecture.md](video-architecture.md) の「`WM_CHAR` / Text はヘルプ開閉には
  使わず」の段に、presenter 側が effective chord へ追従する仕組みが書いてある。
- App 側は `consume_context_shortcuts_help_key` を root ([app.rs](../src/app.rs) 34109 付近) と
  フルスクリーン ([ui_fullscreen.rs](../src/ui_fullscreen.rs) 19142 / 19606 / 19917 付近) で
  消費する。**切り離した窓のキーが presenter ではなく root 側へ届いている**か、両方が
  反応して root 側だけが描いている、のどちらかが疑わしい。まず**どちらが消費したか**を
  観測してから直すこと (推測で分岐を足さない)。
- **[1.162](#1162) と同じ家系**: 切り離した窓での操作が、メインウィンドウ側の owner へ
  落ちる。あちらは owner 窓が `main_hwnd` 固定という特定済みの原因なので、同じ形か
  どうかを最初に確かめると早い。

⚠️ detached 経路なので、着手前に [detached-rework-plan.md](detached-rework-plan.md) §2
(憲法) を読むこと。症状パッチ (押した窓を推測する guard、時間窓、二重消費の抑止) を
入れない。

規模 / 優先度: 小〜中 / P3。ヘルプが出ないわけではなく、出る場所が違う。

### 1.164 修飾キーで、枠の内外を問わず新しい切り取り枠を引けるようにする

- 出典: 利用者提案 (2026-09-02)。初めて切り取りを設定するとき、欲しい範囲を取るには
  ハンドルまでカーソルを運ぶ必要がある。
- **前提の確認 (コードと一致)**: 切り取りモードに入った時点で枠が未設定だと、既定は
  `CropRect::full` (画像全体) ([ui_crop.rs:466](../src/ui_crop.rs:466))。その状態では
  `target_at` が画像内のどこを押しても `CropHandle::Body` を返すため、ドラッグは常に
  「枠の移動」になり、**新規作成の経路へ到達できない**。
- **対応**: 修飾キーを押している間は hit test を飛ばして create-drag に固定する。
  Space + ドラッグのパンが既に `KeyAction::CropSpacePan` として keymap 経由なので、
  **これも `KeyAction` として追加**し、`ini_name()` / `context()` / `trigger()` /
  `default_chords()` / `ALL_ACTIONS` / [docs/keymap.ini.default](keymap.ini.default) /
  context shortcuts help ([context_shortcuts.rs:189](../src/ui_dialogs/context_shortcuts.rs:189))
  を揃える。既定は Ctrl + ドラッグ。
- **決めること**: Space パンと同時に押されたときの優先順位。ドラッグ開始後に修飾キーを
  離した場合の扱い (開始時の判定を保つ)。
- 規模 / 優先度: Small / P2。

### 1.165 切り取り済みの編集プレビューを代役に使うと、寸法の説明と絵が食い違う

**利用者報告 2 件 (2026-08-31 / 2026-09-02) と、こちらで見つけた連結読みの症状は
すべてこの 1 つが原因。** 症状ごとに対症で塞がない。

#### 症状

1. **単ページ**: 切り取り済みのページをフルスクリーンで開くと、本画像が来るまでの間に
   切り取り枠と暗転が**切り取り済みのサムネイル**の上へ乗り、二重に切り取ったように見える
   (利用者報告)。
2. **連結読み**: 切り替えた直後、ページ全体フィットのはずが**少し拡大して見える**ページと、
   **切り取られた絵のまま出る**ページが混在する (こちらで確認)。
3. 本テクスチャが届いた瞬間に絵が**広がる**。

#### 原因 (確定)

**切り取りがピクセルへ適用されているのは編集プレビューキャッシュ (= 一覧のサムネイル) だけ**で、
表示用テクスチャ (raw / processed / final composite) には入っていない
([edit_preview_cache.rs:1023](../src/edit_preview_cache.rs:1023)、サムネ側は
[thumb_loader.rs:1061](../src/thumb_loader.rs:1061) が `source_dims` へ**切り取り後の寸法**を
入れる)。crop を変えても表示 cache は保持され、失効するのは編集プレビュー WebP だけ。

そのうえで、表示側は**代役としてこのサムネイルを使う**。

- 単ページ / 見開き: [fs_display_texture_choice](../src/ui_fullscreen.rs:25985) の
  `tex.or(thumb_tex)`
- 連結読み: [ui_fullscreen.rs:27873](../src/ui_fullscreen.rs:27873) の
  `fs_thumbnail_texture_for_display`

さらに連結読みでは、**ページの大きさを決める処理と、貼るテクスチャを選ぶ処理が別**である。

- 大きさ: [vertical_reading_base_size](../src/ui_fullscreen.rs:26917) →
  [choose_continuous_page_layout_size](../src/ui_fullscreen.rs:4657)。
  解決順は fs_cache → `fs_early_dims` → **サムネイルの `source_dims` (切り取り後)**
- 貼る絵: そのあとの `page_textures` ループ ([ui_fullscreen.rs:27803](../src/ui_fullscreen.rs:27803) 以降)
- 流し込みは [from_resolved_rect](../src/displayed_image_transform.rs:195)。**矩形をそのまま
  使い倍率を逆算する**ので、2 つが別の絵を指していれば差がそのまま拡大 / 変形として出る

`fs_early_dims` は**フル解像度の読み込みを開始したページにだけ**入る
([app.rs:64579](../src/app.rs:64579)、worker の `DimsOnly`)。したがって連結読みへ入った直後は
次の 2 状態が同居する。

| ページの状態 | 大きさの出どころ | 貼る絵 | 見え方 |
| --- | --- | --- | --- |
| 読み込み開始済み・未到着 | 元画像の寸法 | 切り取り済みサムネ | **拡大して見える** |
| 読み込み未開始 | 切り取り後の寸法 | 切り取り済みサムネ | 大きさは合うが**切り取られた絵** |
| 本テクスチャ到着後 | 元画像の寸法 | 元画像 | 正しい (ここで広がる) |

**普段この食い違いが出ないのは、サムネイルと元画像の縦横比が同じだから**である。代役にしても
解像度が落ちるだけで大きさの意味は変わらない。**切り取りはその前提を壊す唯一の編集**で、
だから切り取り済みページでだけ現れる。

同じ教訓が隣に既に書かれている ([ui_fullscreen.rs:27803](../src/ui_fullscreen.rs:27803)
「貼るテクスチャは描画より前に 1 回だけ決める … 最初の実装が実際にそれで、連結読みだけ
直っていなかった」)。見開きの間隔合わせでは直したが、**基準寸法の解決はまだ選んだ
テクスチャと別経路**で、同型が 1 段上に残っている。

#### 対応

**(a) 即時 — 枠と暗転を、代役を貼っている間は描かない。** overlay の条件
([ui_fullscreen.rs:15510](../src/ui_fullscreen.rs:15510)) は `!original_preview_active` は
見ているが、**本画像が来ているかを見ていない**。呼び出し地点に `state.tex` があるのでそこで塞ぐ。
利用者へ「次で直す」と回答済み (2026-09-02)。**症状 1 の枠だけが消え、絵の食い違いは残る**
ことを承知のうえで先に出す。

**(b) 構造 — 寸法の説明と貼る絵の出どころを 1 つにする。** 基準寸法を、実際に選んだ
テクスチャから引く。`ThumbnailState::Loaded` は既に **`from_edit_preview: bool`** を持つ
([grid_item.rs:487](../src/grid_item.rs:487)) のに、代役を返す
[fs_thumbnail_texture_for_display](../src/app.rs:14993) はそれを見ていない。所有を 1 つにすれば
切り取り以外の代役でも構造的に安全になる。**(a) の後に (b) を入れ、(a) の条件を見直す。**

**(c) 代替案 (採らない場合の比較用)**: 切り取り済みプレビューを代役に使わない
(`from_edit_preview` を見て断る)。実装は最小だが代役が減り「絵が出るまでが遅い」方向へ振れる。
1.166 で表示へ反映する方針を採る場合、食い違いはそちらで消えるので (b) の形が変わる。

#### 回帰確認

- 切り取り済みページを一覧から開き、本画像到着の前後で**絵の大きさと位置が動かない**。
- 連結読みへ切り替えた直後、ページ全体フィットで**拡大されて見えるページが無い**。
- 連結読みを速くスクロールして戻ったとき、ページが「広がる」瞬間が無い。
- 見開き、分割、回転、表示トリム、AI アップスケール併用でレイアウトが変わらない。
- 切り取りの無いページの表示と代役の出方が従来どおり (代役を減らしていない)。

- 規模 / 優先度: (a) Small / (b) Small〜Medium。**P1** (見えている絵が正しくない)。

### 1.177 一括書き出しの準備が、1 件でも重ければその間 UI が止まる (v3.5.0 レビュー R08)

- Ctrl+E の準備はフレーム予算 (6ms) で分割したが、**予算を見るのは 1 件を終えた後**。
  マスクの DB 読み出しと展開、補正レイヤーの読み込み、注釈フォントの準備、AI runtime の
  初期化は同期のまま UI 上にある。1 件で 100ms 以上かかればその全部の時間止まり、
  ダイアログの初回表示もキャンセルも待たされる。
- 多数件を 1 フレームで処理しなくなった効果はあるが (v3.5.0 の F04)、**UI 応答性の
  境界そのものは動いていない**。
- 直し方: UI は軽量な identity と未保存編集 override だけを固定し、重い snapshot 作成を
  worker へ出す。`materializer::load_page_edits` が既に worker 側で同じことをしているので、
  合流させるのが筋。遅い DB / 大きなマスク / 未初期化 runtime を注入して、準備中の UI tick と
  キャンセル応答を確かめる。
- 関連: §1.170 (見開き合成) と同じ「source snapshot → worker composite」境界。

### 1.172 関連付けの cache miss で、右クリックが同期 Shell 列挙へ戻る (v3.5.0 レビュー R10)

- 先行準備 (フォルダー読み込みごと、最大 8 拡張子) が届いていない拡張子では、メニュー組み立て
  中に `enumerate_handlers` を UI から呼ぶ。先行 worker が同じ拡張子を列挙中でも待機状態を
  区別せず重複列挙する。
- v3.5.0 で「毎回の右クリックで列挙し直す」のは直った (拡張子ごとの一覧を保持)。残るのは
  **最初の 1 回**と、準備が間に合わなかった拡張子。
- 直し方: 拡張子ごとの準備状態 (未要求 / 実行中 / 準備済み) を worker が所有し、メニューへ
  非同期に候補を渡す。**項目を落として軽くする対処はしない。** メニューは同じフレームで
  組み立てるので、非同期化には「準備できるまで関連付けの項目を出さない」等の UX 判断が要る。

### 1.173 同じページを開いた別ウィンドウへ、編集の失効が届かない (v3.5.0 レビュー R11 / BA-7)

- A と B の独立 viewer で同じページを開き、A で一括貼付 / リセットをすると、DB と A は
  更新されるが B の `adjustment_page_params`・マスク・補正レイヤー・注釈の保持状態と派生
  キャッシュへ mutation が配送されない。B は古い表示を使い続ける。
- **v3.5.0 の退行ではない。** 単一ページの貼り付けにも同じ穴があり、
  [docs/detached-rework-plan.md](detached-rework-plan.md) §11 にも未実装と記録済み。
  v3.5.0 では「要求を出した context へ結果を戻す」ところまでを直した (F08)。
- 直し方: page-key 単位の mutation 通知を該当 context へ配送する。別ページ・別 context は
  失効させない。2 窓同一ページと 2 窓別ページの対になる回帰テストが要る。detached リワークの
  所有境界 (BA-7) に属するので、リワーク側と整合を取ってから着手する。
- **同じ層にある残件 (2026-09-03、v3.5.0 レビュー T01 の修正時に確認)**: 別の窓で Ctrl+Z を
  押したときの補正 Undo。復元先をページ識別子にしたので、**別ページの正本を書き換えることは
  無くなり、正本 (DB) はそのページへ正しく戻る**。ただし、そのページを開いている窓の
  `adjustment_page_params` は変更後のまま残るので、その窓が読み直すまで表示に反映されない。
  ここで場当たりの通知を足すと本項の設計と二重になるため、**本項でまとめて配送する**。

### 1.176 一括補正の適用と取り消しで、ページ数ぶんの DB 書き込みが UI を数秒止める

- **症状**: 一覧で多数チェックして <kbd>Ctrl</kbd>+<kbd>1</kbd>〜<kbd>0</kbd> のスロット一括適用、
  および その <kbd>Ctrl</kbd>+<kbd>Z</kbd> / <kbd>Ctrl</kbd>+<kbd>Y</kbd> で、UI が数秒固まる
  (利用者確認 2026-09-03)。
- **v3.5.0 の退行ではない。** v3.4.0 の `apply_slot_to_grid_selection` と
  `apply_adjustment_change_to_app` も同じく 1 ページずつ `set_page_params` を呼んでいる
  (`git show v3.4.0:src/app.rs` / `:src/undo_ops.rs` で確認)。v3.5.0 の U01 で直したのは
  復元先の**検索**コストで、こちらは元からある**書き込み**コスト。

**計測** (`cargo test -p mimageviewer --lib bulk_adjust_cost -- --ignored --nocapture`、
2,000 ページ、開発機 / 一時フォルダの DB):

| 区間 | 時間 |
|---|---:|
| 適用 (`capture_adjust_full` + `set_page_params` × N) | 5,584 ms |
| 取り消し (`apply_meta_undo`) | 4,139 ms |
| やり直し (`apply_meta_redo`) | 3,770 ms |
| — DB を 1 行ずつ (いまの発行のしかた) | 3,367 ms |
| — DB を 1 トランザクション (既存 `set_page_params_bulk`) | **11 ms** |
| — 復元先の解決 (U01 で直した部分) | 5 ms |

- **分かっていること**: `set_page_params` は 1 ページごとに `conn.execute` を呼ぶので、
  **ページ数ぶんの暗黙トランザクション**になる。同じ 2,000 行を 1 トランザクションで書くと
  11 ms なので、**約 300 倍**の差。U01 で直した検索はもう支配的ではない (5 ms)。
- **分かっていないこと**: 適用 5,584 ms のうち DB が 3,367 ms として、**残り約 2,200 ms の
  行き先はまだ特定していない** (キャッシュ無効化 / `effective_params` / サイドカー /
  `capture_adjust_full` の差分取り、のどれか)。**先にここを内訳へ落としてから**設計する。
  DB だけ直して「半分速くなりました」で終わらせない。

**直し方の候補** (内訳が出てから決める):

- 既に `AdjustmentDb::set_page_params_bulk` / `remove_page_params_bulk` があり、
  「全画像に適用」(`app.rs` の 1 箇所) だけが使っている。スロット一括適用と Undo / Redo の
  復元も、**同じページ集合をまとめて 1 トランザクションで書く**経路へ寄せる。
- ただし現状の書き込みは 1 ページごとに「標準と一致するなら行を消す」判定 (`matches_default`)
  と sidecar 反映を挟むので、単純に bulk API へ差し替えるだけでは意味が変わる。
  **set と remove の 2 群へ振り分けてから**それぞれ 1 トランザクションにする形になる。
- キャッシュ無効化 (`clear_caches_for_param_change` / `bump_adjustment_generation`) を
  ページごとに回している分も、まとめて 1 回にできるか見る。

- 計測用の道具は `src/app/tests.rs` の `bulk_adjust_cost_tests` に置いてある
  (`#[ignore]`、合否判定はしない)。直したら同じ道具で前後を比べる。
- 規模 / 優先度: Medium / P2。

### 1.175 `ui_snapshot` のテスト実行体が、たまにアクセス違反で落ちる / 進まなくなる

**2026-09-08 再観測:** v3.7.0向け動画高さ設定追加後の全体gateで、
`ui_snapshot-6bb93fba2c7d6bab.exe` が45件の途中で `0xc0000005 / STATUS_ACCESS_VIOLATION` により終了した。
利用者が報告した「wgpu Device Class」のWindowsメモリreadエラーダイアログとexe名が一致する。
証跡は`target/v370-work/video-height/test-full-final-1.log`（8847〜8867行、全体exit101）。
既存報告と同型だが、今回の変更との因果関係やfault moduleは未確定で、GPU競合とは断定しない。
通常の再実行を止め、他suiteの成功を保持して未完了のsnapshot検証を区別する。
同ログには`test-full.ps1`末尾のvendor2suiteが到達していないため、全体gateを成功扱いしない。
以下の頻度・原因推測・リリース判断は9/3以前の観測記録であり、今回の合格を保証しない。

同日、非表示CDB下で同じtest exeを1回だけ実行したところ45/45が成功（10.07秒）、
second-chance AVはなくdump未生成、debugger exit0、終了後に対象processは残っていなかった。
証跡は`target/v370-work/video-height/cdb-ui-snapshot-final-2.pipe.log`と同`-2.log`。
初回のCDB引数エラーはdebuggee起動前に終了しており、別途`-1.log`へ保持した。
debug outputにはD3D12の`INVALID_SUBRESOURCE_STATE #538`と
`RESOURCE_BARRIER_BEFORE_AFTER_MISMATCH #527`（wgpu内部external texture params buffer）が記録された。
OBSの`graphics-hook64.dll`とShadowPlayの`nvspcap64.dll`のloadも観測したが、
今回はAVを再現しておらず、いずれもクラッシュの原因とは断定しない。GPU backend・driver・
外部hook・device生成破棄のどこに原因があるかは、例外時stackを得てから判断する。
snapshotの今回不足分は同一exeの45/45で補完できたが、本項目の根因は未解決のまま残す。

- **症状は 2 つある。同じものかは未確定。**
  - **アクセス違反**: 43 件のうち十数件を通過したところで、テスト実行体が
    `exit code 0xc0000005 (STATUS_ACCESS_VIOLATION)` でプロセスごと死ぬ。assertion 失敗では
    ないので、どのテストが原因かも harness の出力からは分からない。
    2026-09-03 の配布ビルドで発生 (`ui_snapshot-cd01fab322c84861.exe`)。
  - **停滞**: 43 件のうち 20 件を通過した後、CPU を使い続けたまま完了しなくなる。
    2026-09-03 の第 6 回レビューで発生し、15 分後に手動停止。
- **頻度**: 2026-09-03 の同一 HEAD 付近で観測した範囲では、6 回中 1 回程度。
  逐次実行 (`--test-threads=1`) と、その後の並列 2 回はいずれも 4 秒台で完走した。
  **「逐次なら出ない」とは言えない** — 逐次の試行回数が足りていない。
- **9/3当時、v3.6.0の変更以前にも観測していた**。落ちた HEAD で 2 回連続 PASS し、同じ HEAD の
  別ビルドでも並列のまま 4.81 秒で PASS している。以前の版でも起きている。

**調査できる。手段は揃っている** (「再現しないから無理」ではない):

- `cdb.exe` が入っている: `C:\Program Files (x86)\Windows Kits\10\Debuggers\x64\cdb.exe`。
- テスト実行体の隣に **PDB がある** (`target/debug/deps/ui_snapshot-<hash>.pdb`) ので、
  スタックはシンボル付きで解決できる。
- 実行体のパスは毎回ハッシュが変わるが、
  `cargo test -p mimageviewer --test ui_snapshot --no-run --message-format=json` の
  `executable` フィールドから取れる。
- WER の LocalDumps は **この PC で既に使っている** (`mimageviewer-core.exe` に対して
  DumpType=2 / `%LOCALAPPDATA%\CrashDumps` / 10 世代)。ただし exe 名が毎ビルド変わるので、
  同じ「実行ファイル名ごと」の設定は使いにくい。既定 (全プロセス) を有効にすると
  無関係なアプリの dump まで集まる。**cdb で回すほうが素直。**

**手順の案** (上から):

1. 実行体のパスを cargo の JSON から取り、`cdb -g -G -c "sxe -c \"!analyze -v; .dump /ma
   <out>.dmp; q\" av; g"` で包んで**繰り返し起動**する。6 回に 1 回なら、20〜30 分回せば
   1 本は捕まる見込み。落ちたところのスタックが出る。
2. スタックが `wgpu` / D3D12 / OpenGL のどれを指しているかで、次の切り分けが決まる。
3. `--test-threads=1` を **数十回**回して、並列でしか出ないのかを確かめる
   (現状は試行が足りず「並列で 1 回出た」だけ)。
4. `WGPU_BACKEND` を `dx12` / `gl` / `vulkan` で固定して、backend 依存かを見る。
5. `egui_kittest` (`snapshot` + `wgpu` feature、`Harness` ごとに device を作る) の
   upstream に同種の報告があるかを確認する。43 件が並列で device を作っては壊すので、
   **driver / backend 側の競合を疑ってはいるが、まだ何の根拠もない**。

**なぜ今すぐやらないか**: リリースを止める理由にならない (引き直せば通る、成果物には
影響しない) が、**毎回の配布ビルドで一定確率で止まる**ので、放置すると
「テストゲートが赤 = とりあえず再実行」が習慣になる。それが一番まずい。

- 規模 / 優先度: Medium / P2 (v3.6.0 で 1 度は時間を取る)。

### 1.192 動画のシークとストリップ close が、離すフレームの最後の移動を取りこぼす (2026-09-07)

- 出典: v3.6.0 出荷前レビュー ([review-v3.6.0/seek-strip-findings.md](review-v3.6.0/seek-strip-findings.md) F3)
  で静止画側に見つかった問題。**同じ形が動画側にもある**ことを、その修正
  (`99593e029`) の実施中に確認した。**動画は今回変更していない。**
- 症状 (コードから確定、実機未確認):
  - 横シークが既にスクラブに決まった状態で、「最後の移動 + 離す」が同じフレームに届くと、
    **最後の位置へシークしない**
  - シークストリップの下方向 close も同じで、**最後の移動で境界を越えて離すと閉じず、
    その位置で commit される**
- 機構: egui の release フレームは `drag_stopped=true` / `dragged=false` になる。
  判定が `dragged()` の間だけを見ていると、そのフレームのポインタ位置が使われない。
- 静止画側の直し方が参考になる: release フレームでもそのフレームの座標で
  Seek / close を判定する (`src/ui_fullscreen.rs` の
  `handle_still_seek_track_response` / `handle_still_seek_strip_response`)。
  **押下原点を XY で保持する**必要がある点も同じ。
- **今回入れなかった理由**: 動画は現在正常に動いており、静止画の修正に動画を巻き込むと
  1 つの変更で 2 つの面を触ることになる。分けて扱う。
- 規模 / 優先度: 小 / P2。


### 1.199 native 動画の同一batchで領域を跨ぐと、先のwheel操作が失われる可能性 (2026-09-07)

- **コード上の候補、実行再現は未実施。調査優先度 P2。** §1.197の独立Astra調査と親の照合で発見。
- strip wheelはegui MouseWheelへ積まれるが、その後のcanvas wheel/moveでpointer_posが変わると、
  draw_native_seek_stripは最後のpointerだけを使い、先のstrip wheelを消費しない可能性がある。
  panelにも最終hoverへの集約が及ぶ。Zoom固有ではなく、既存360/通常navigationにもある境界。
- GPU不要の既存strip描画ハーネスへ、`PointerMoved(strip), MouseWheel, PointerMoved(canvas)`と
  逆順を与え、Window spanのStepSeekStripRangeが各1回出るか確認できる。先にこの再現を取る。
  panelは実ScrollAreaを通す別確認が必要で、booleanの領域分類だけをスクロール成功と扱わない。
- 正しい修正はeventごとの位置/領域所有と一括egui入力の統合を要する。§1.197で調査中の
  VideoZoomWheel commandからraw wheelへの二重転送とは別の根因であり、variant追加では直らない。
- §1.197の自動化は一入力ごとにreceiptを待つ範囲を先に完成させ、混在batch保全まで成功と表明しない。
  詳細: [ui-smoke-automation-plan.md](ui-smoke-automation-plan.md) S3b。

### 1.225 詳細表示からの静止画シーク・画質設定の不具合 2 件と、サムネイル列「最大」 — >>367 (2026-09-08)

- **追加実装済み・実機確認待ち（20:02buildへの利用者報告）:** サムネイル動作は良好との部分確認を得た。
  静止画・本の右下stripアイコンへ表示ON/OFFと5段階の高さを選ぶメニューを実装した。
  各段階の高さ数値は既存設定を使い、環境設定へ戻らず閲覧中に選択できる。
  メニュー5件・静止画seek関連86件の焦点検証が成功。追加差分の独立検収・全体gateも成功し、22:08に確認用buildを更新した。実機確認待ち。
- **静止画側は実装・自動検証・独立検収済み、実機確認待ち。** 以下は報告時の症状と調査方針。
  同日の追加指定で、動画にも「最大」と5段階それぞれの高さ数値設定を追加した。
  動画側も実装・独立Sol検収・必要な自動検証の限定補完と20:02の確認用build更新を完了し、実機確認待ち。
  静止画・本と動画の高さは独立設定、既定144／104／72／48／36、範囲36〜320（100%時px相当）。
  動画追加前の確認用buildと追加後の結果を [作業台帳](v3.7.0-priority-work.md) で区別する。
- **次バージョン対応予定。2 件とも手元で再現済み。規模 / 優先度: 小〜中 / P2。**
- **詳細表示から開くと静止画シークのサムネイルを読み込めない:**
  - ファイル選択画面を詳細表示にした状態でフォルダまたは ZIP を開くと、静止画 / 本の
    シークバーにサムネイルが表示されず、ホバー時のプレビューも「読み込み中…」のままになる。
  - サムネイル表示から開いた場合は発生せず、いったんサムネイル表示へ切り替えてから詳細表示へ
    戻すと表示される。
  - 詳細表示では通常のグリッド用サムネイルを抑制・破棄するが、シーク表示が要求する限定範囲の
    サムネイルは表示形式に関係なく読み込める必要がある。`still_seek_thumbnail_pages` の keep / enqueue
    だけでなく、詳細表示から直接開いた場合の画像寸法・メタデータ準備、要求状態、完了反映までを
    一連の経路として確認し、サムネイル表示を一度経由することが前提になっている箇所を直す。
  - 回帰確認は、詳細表示から実フォルダと ZIP をそれぞれ開き、サムネイル列とホバープレビューが
    読み込まれること、表示形式の往復を必要としないこと、通常の詳細表示で全件サムネイルを
    読み始めないことを固定する。
- **詳細表示から「サムネイル画質…」を開くと画面が崩れる:**
  - 詳細表示の状態で `設定 → サムネイル画質…` を開くと、プレビューを含む初期レイアウトが崩れる。
    サムネイル表示から開いた場合も、メインウィンドウ幅によっては右側が見切れる。
  - 現在の初期サイズはグリッドの最終セル寸法を基準にしているため、詳細表示では意味のある値に
    ならない。ダイアログの初期サイズとリサイズ後の矩形を表示領域へ収め、タイトルバー・閉じる操作・
    右下のリサイズ部分が常に操作できるようにする。狭い場合は内容を縮めるかスクロールさせ、
    固定幅や大きな最小幅で画面外へ押し出さない。
  - サムネイル表示 / 詳細表示の両方を起点に、通常幅と狭い幅で UI snapshot または kittest を追加する。
- **静止画 / 本のサムネイル列へ高さ「最大」を追加:**
  - 現在の「大」(104pt) より一段高い選択肢を追加する。正確な高さは実装時の見た目で決める。
  - 当初は静止画 / 本向けに限定した。動画への追加は同日の利用者依頼により明示的に対応する。
    共通enumの変更だけでなく、動画の描画・入力・映像予約・ドラッグの寸法を統合して検証する。
  - 固定 / 非固定表示、下部シークバー、左右パネル、狭い画面、縦横比の異なるサムネイル、
    タッチ操作で画像や他の HUD と重ならず、操作領域を確保できることを確認する。

### 1.201 見開きで次の左ページだけ AI・カラー化の先読み対象外になる — >>369 (2026-09-08)

- **報告者の手元で再現済み。利用者指定により v3.7.0 対応予定。規模 / 優先度: 小 / P2。**
- 本開発系統で実装・自動検証・独立検収済み。左右開き・表紙ありの前後表示単位と、実DBでの
  一度限りの移行・利用者が2へ戻した後の再読込を確認した。実機確認は未実施。
  現在のCargo版3.6.0では旧設定の移行と3.7.0告知は発火しない。版更新後の確認対象として残す。
  作業・検証状況は [v3.7.0-priority-work.md](v3.7.0-priority-work.md) に記録する。
- 症状: 右開きの見開きでページをめくると、左ページだけ元画像または低解像度の代役が先に
  表示され、その後 AI アップスケール等の最終結果へ切り替わって鮮明になる。
- 原因:
  - フルサイズ画像の先読み既定値は後方 4 / 前方 12 枚だが、AI・カラー化の先読み既定値は
    後方 1 / 前方 2 枚。
  - 右開き見開きでは現在の右ページが anchor、現在の左ページが 1 枚先、次の見開きは
    右ページが 2 枚先、左ページが 3 枚先として数えられる。
  - そのため前方 2 枚では次の左ページだけ final AI / final effect の先読み対象外になる。
    手元でも前方 2 枚へ戻すと、ページ移動後に片側だけ AI アップスケールが走ることを確認した。
- 対応方針:
  - 新規設定の `ai_upscale_prefetch_forward` 既定値を 2 から 3 枚へ変更し、次の見開きの両側を
    先読み対象に含める。
  - 旧版からの更新時は、保存値が旧既定値の 2 枚である場合だけ、一度だけ 3 枚へ移行する。
    `last_seen_version` を使って対象版以前からの更新に限定し、3 枚以上・0/1 枚・移行後の値は
    上書きしない。
  - 保存値だけでは「旧既定値のまま」と「利用者が明示的に 2 枚を選択」を区別できない。
    後者も一度だけ 3 枚になる制約を許容し、更新履歴へ既定値変更を記載する。2 枚自体は
    引き続き設定可能とする。
  - 逆方向へページを戻す場合は後方 1 枚で前の見開き全体を覆えるかも確認する。今回の変更で
    後方値まで自動変更するかは、実際の display-unit / anchor の並びをテストしてから決める。
  - **2026-09-08 コード照合後の設計判断:** anchor は読み順で先のページであり、前の見開きは
    anchor から -2 / -1 に位置するため後方 1 枚では不足する。新規設定の後方既定を 2 枚へ変更する。
    既存設定の後方値は移行せず維持する。前方値の移行と新規後方既定の変更を区別して検証する。
- 回帰確認:
  - 右開き / 左開きの見開きで、前進時に次の表示単位を構成する両ページが対象になること。
  - 新規設定は前方 3 枚、対象旧版 + 値 2 は 3 へ移行し、その他の値と現行版設定は維持すること。
  - AI アップスケール、AI ノイズ除去、カラー化、Creative LUT の各 final 経路で、先読み済みの
    ページが移動後に再処理されないこと。

### 1.202 長い動画タイトルが上部 HUD の操作ボタンに重なる (2026-09-08)

- **v3.7.0 対応予定。優先度: P2。利用者の画像で症状を確認、未実装・実アプリ再現と原因調査は未実施。**
- 症状: 動画の長いタイトルが上部 HUD の右側ボタン群まで描画され、文字とアイコンが重なる。
  OS のウィンドウタイトルバーとは別の、アプリ内タイトル表示の問題。
  証跡は 2026-09-08 の利用者添付 `codex-clipboard-60762f48-2ab2-4e3f-9bdd-5159bdac1619.png`。
- 修正要件:
  - 実際のボタン群の占有領域を確保し、残りの幅へタイトル表示を収める。省略・クリップ等の
    具体策は現行の描画・入力経路を確認して決め、全文を確認する手段も維持する。
  - ボタンの表示・機能・当たり判定を維持し、タイトル長によって操作領域へ侵入しないこと。
    ボタンを隠す、操作領域を縮める等で回避しない。
  - native 動画のタイトルと上部 HUD のレイアウト所有を調べる。共有経路がある場合は、
    通常窓・F12／複数窓・全画面や音声表示等の影響先を列挙して統合する。
- 完了条件: 長短タイトル、狭幅、DPI／文字倍率、HUD の固定／非固定で文字と操作領域が
  重ならず、既存操作とメタデータ表示が維持される。可能な範囲でレイアウト回帰・snapshot を追加する。
  実アプリ起動・操作による確認は、明示了承を得たリリース前の検証枠で行う。
- 作業・検証状況は [v3.7.0-priority-work.md](v3.7.0-priority-work.md) に記録する。

### 1.197 実機確認を、アプリを起動して操作する自動テストへ置き換える (2026-09-07)

- **2026-09-08 追加の利用者判断:** §1.225 (当時の §1.200) など通常閲覧の修正を早く届けるため、
  自動化の全面完成は v3.7.0 のリリース条件にしない。未完成の動画操作自動化と
  §1.198 の測定 runner 改良は後続へ回し、必要な製品検証は既存自動テストと
  了承済み枠での手動・ログ確認を組み合わせて実施する。既存草案・成功記録は保持する。
- **2026-09-08の利用者指定:** 実アプリ起動・操作は原則リリース前の検証枠へ集約し、
  対象・所要時間・PC使用範囲への明示了承後だけ実施する。通常開発では実装・非対話テストを進める。
  予告や過去のマウス方式選択は実行了承の代わりにならない。
  [実アプリ検証の実行確認](interactive-release-verification.md)を既存の必須検証にも適用する。
- 2026-09-07 の実装前提検証・独立レビューを反映した段階設計は
  [ui-smoke-automation-plan.md](ui-smoke-automation-plan.md)。S0の診断portable隔離を
  先行し、S1の複数窓、S2のegui pointer、S3のnative入力を別々に検証する。
  既存の`target_rendered`は入力callback到達であり、PDFのpaint成功を表す値ではない。
- 進捗: S0隔離scriptに続き、`b20a1e502`でS1aのread-only窓一覧・実texture由来のpaint証跡を実装。
  S1bの対象配送とbackend窓識別も実装し、37+29+7+3件の焦点テスト・feature有無core check・
  独立レビュー成功。2026-09-08に隔離portableの対話desktop実行で複数窓PDFがexit 0。
  手作業3項目のうち動画zoomが残る。S2本体は独立Astraレビューとfeatureあり焦点テスト・
  featureあり/なしcore check・通常lib焦点テストを通り、隔離portable実行で静止画列dragが成功。
  2026-09-08の最終scenarioでは左右方向・最終Up・別trackのpage着地・PDF sibling不変を確認し、
  複数窓PDFの回帰実行も成功した。実mode条件待ちと量子化に対応した座標を用いる。
  S3aはsource epoch0の誤判定、送信tokenの幅・非再利用、PowerShell間のbuild照合を修正した。
  2026-09-08 06:36の隔離liveで、新prefixの2点moveが要求座標・受信値の完全一致で
  WM/pump/renderを通過しexit 0。S3bのbutton/wheel/panと負例の自動化は未完了。
  以下の部品表は開始時点の棚卸しを保持する。
- 動機: リリースごとの手作業確認が**リリース間隔を延ばす主因**になっている。
  v3.6.0 では、レビュー指摘 1 件につき 1 項目という誤った単位で 15 項目まで膨らんだ。
  正しい単位は「**自動テストが届かない経路の数**」で、そこまで絞ると 3 項目になる
  (複数ウィンドウで PDF / 列のドラッグ / 動画のズーム)。**その 3 つも自動化したい。**

#### 既にあるもの

| 部品 | 実体 |
| --- | --- |
| スクリプトランナー | `src/test_script.rs` (1,751 行)。`--test-script <path>` で起動、Rhai |
| 操作 / 待ち / 失敗 | `run_action` / `wait_until` / `hold_key` / `release_key` / `fail` |
| 状態観測 | `TestScriptSnapshot` (`is_fullscreen` / `fs_idx` / `items_len` / `pending_thumbs` / `modal_open` ほか) |
| 判定 | 成功 / 失敗 / **環境不成立** を区別した終了コード |
| キー入力 | `src/key_input.rs`。egui の `Event::Key` と `GetAsyncKeyState` の**両方**へ同じ押下状態を答える (物理入力と同じ 2 表現) |
| 隔離 | `--data-dir`、`scripts/prepare-portable-smoke.ps1` |
| 前例 | `scripts/page-turn-smoke.ps1` |

#### 足りないもの

1. **egui 面のポインタ合成**。`SyntheticNavigationKey` は方向 / PageUp・Down / Home・End /
   Enter / Esc の 10 個だけで、マウスが無い。`Event::PointerMoved` / `PointerButton` /
   `MouseWheel` を、キーと同じ timeline へ載せる。
   **これで一覧・ダイアログ・静止画フルスクリーン (シークバーとサムネイル列を含む) が回る。**
   実機でしか出なかった列の不具合 (押下フレームで `drag_started` が立たない / 離すフレームは
   `dragged=false` / `interact_pointer_pos` は押している間だけ / RTL で送る向きが反転) は
   **すべて egui のイベント層**なので、ここで捕まる。

2. **動画 (native presenter) は別の継ぎ目が要る。** ⚠️ **egui への注入は届かない。**
   HUD・シークバー・ストリップ・ズームは Win32 + D3D11 の面で、入力は
   `src/video/native_window.rs` の wndproc (`WM_MOUSEMOVE` / `WM_LBUTTONDOWN` /
   `WM_MOUSEWHEEL`) から入り、`NativeVideoOutputEvent` として App へ渡る
   (`src/app/native_video.rs` の `apply_native_video_zoom_wheel` / `_drag` は生の
   `mouse.x` / `mouse.y` を物理 px で受け取っている)。
   - `NativeVideoOutputEvent` へ直接注入すると**presenter 自身の当たり判定を飛ばす**。
     「ストリップや左右パネルの上ではそれぞれの操作を優先する」という、まさに
     確認したい部分が抜ける
   - 2026-09-07 の調査で、per-HWND sinkから両routeへ注入するだけでも不十分と判明。
     実OSのcursor/button/captureを読む定期監視が、合成dragを上書きするため。
     診断入力providerの追加設計との比較を説明し、**利用者は実マウス入力を選択**した。
     テスト中は前面とマウスを使用し、使い捨てportableの既存Windows入力経路を通す。
     exact host・実描画矩形・配送/処理receiptを設計し、無配送を「優先判定成功」と誤認しない。
     実際に到達・観測したOS経路だけを確認済みとし、GPU scanout等は別の限界として残す。

3. **座標の指定方法**。座標直書きはレイアウト変更で総崩れになる。
   `TestScriptSnapshot` は今は真偽値と数値だけなので、**名前付きの矩形**
   (`seek_bar` / `strip_cell(i)` / `video_canvas` / `grid_cell(i)`) を載せ、
   スクリプトは `drag("seek_bar", from: 0.2, to: 0.8)` のように**論理指定**する。

4. **複数窓の指定と切替**。ホスト側は現在、対象 PID の「最大の可視窓」を前面にするだけで、
   窓を選べない。registry が持つ窓 identity / host claim で指定できるようにする。
   `TestScriptSnapshot` も現在は mount 中の投影中心なので、**窓ごとの context / ページ /
   表示状態**を読む入口が要る。

#### これで消える手作業

| 手作業 | 必要なもの |
| --- | --- |
| 複数ウィンドウで PDF を開く | (4) だけ。**ポインタ無しで自動化できる** — `run_action` + `wait_until` で足りる |
| 静止画の列をドラッグ | (1) + (3) |
| 動画のズーム | (2) + (3) |

#### 見積もりと注意

- ポインタ無しの smoke 基盤: 2〜4 日。複数窓の指定・切替まで含めて約 1 週間 (Astra 見積)。
  egui のポインタ合成と矩形公開で +1〜2 日。動画側の継ぎ目は設計次第。
- `build-portable.ps1` は現在 `portable` feature だけなので、**smoke 専用に
  `portable,test-script` を作る**必要がある。診断版を配布物へ混ぜない出力分離も要る。
- `run_action` は**キー入力層を迂回する**。F12 の動作は `ToggleDetachedViewerMode` で
  確認できるが、**物理 F12 の配送までは検証したことにならない**。
- **自動化できないまま残るもの**: 実 HWND のフォーカス・重なり順、OS レベルの入力配送、
  GPU の実出力、通常プロファイル固有の移行、launcher の展開。
- **エージェントは通常データディレクトリでアプリを起動しない**。隔離コピー
  (`target\portable-smoke\`) の起動だけが許される。
- 出典: v3.6.0 出荷前の検討 (Codex Astra、読み取り専用)。
  提案 3 (実アプリ smoke) をポインタ対応まで広げたもの。
- 規模 / 優先度: 大 / **P1** (毎リリースの手作業を減らす投資。次版の頭で着手する)。

### 1.195 detached binding panic の既修正照合と現行再検証 (2026-09-07)

- 出典: `panic.log` の棚卸し (§1.196)。**初稿の「残る2種は原因未特定」は履歴との
  照合漏れだった**。独立 Astra の再調査と親のコード照合で、次の同一根因・検出場所変更を確認。
  - `src/ui_fullscreen.rs:13611:21` / `active detached backstop window # has no context binding`:
    2026-08-28 09:23:56。`29ecf9429` の親のソース行と一致。
  - `src/app/viewer_context_registry.rs:2935:32` / `window # has no viewer-context binding`:
    同日11:10:24。11:07:56の `29ecf9429` が backstop を共通 mount helper へ移した後の検出場所。
- **根本修正は同日13:24:46の `786992a34`**。追加計装で native
  `apply_video_presentation_switched` が binding を作らず session を公開したと確定し、
  native / egui F12 / book の公開境界を修正した。現在の master と v3.3.0～v3.6.0 の tag に含まれる。
  正本の [detached-rework-plan.md](detached-rework-plan.md) の
  「R2e active detached session / binding lifetime follow-up」と §11 に当時の根拠がある。
- v3.6.0 の `c40716b37` は別の PDF 二重窓ID問題を修正し、窓IDを registry binding /
  build reservation から導出する。これと8/28の修正を同一視しない。
- **現行再検証済み**。Sol が session 公開前の binding、release/retire の所有順、
  backstop の owner mount、PDF等の10件の identity 回帰を含む22件を再実行し、すべて成功。
  runtime追加修正は現時点で不要。結果は [v3.7.0 作業台帳](v3.7.0-priority-work.md) に記録した。
- 既存 panic.log に同 fingerprint の後続記録は無いが、それだけで実 HWND・F12連打の
  実アプリ再確認済みとは扱わない。§1.197 の自動化と実機検証の限界を区別する。
- 元優先度 **P1** (クラッシュ)。修正済み根因への症状パッチは加えない。

### 1.196 panic.log に 5 か月ぶんの未処理クラッシュが 12 種たまっていた (2026-09-07)

- 経緯: v3.6.0 の出荷直前に、複数ウィンドウで PDF を開くと必ず落ちる不具合が実機で出た。
  そのとき `panic.log` を開いたら、**2026-04 以降のクラッシュが 15 種 52 件記録されていて、
  どれもバックログにも既知の問題ページにも載っていなかった**。
  出荷前チェックリストには R8「セッション終了後に `panic.log` を確認」が既にあり、
  **手順の抜けではなく実行しなかったこと**が原因である。
- 対策として `scripts/check-panic-log.ps1` を入れた。記録されたすべての panic に
  `docs/panic-acknowledged.tsv` の disposition (fixed / filed / external) を要求し、
  無いものがあれば exit 1 する。リリース手順 Phase 2 の先頭で回す。
- **本項は、その seed で `filed` にした 12 種の棚卸しそのもの。** 1 種ずつ、
  現行版で再現するか / 既に直っているか / 依存側の問題かを判定して処理する。

| 種別 | 場所 | 最終 | 件数 |
| --- | --- | --- | --- |
| index out of bounds | `src/ui_main.rs:1013` | 2026-04-21 | 3 |
| index out of bounds | `src/ui_main.rs:1026` | 2026-04-21 | 1 |
| RefCell already mutably borrowed | `src/video/dsp/gui.rs:221` | 2026-05-03 | 7 |
| RefCell already borrowed | `src/video/gpu_renderer/d3d11_device.rs:759` | 2026-05-14 | 1 |
| `Option::unwrap()` on None | ffmpeg-the-third `resampling/context.rs:189` | 2026-05-14 | 14 |
| wgpu Out of Memory | wgpu `wgpu_core.rs:2015` / `:2568` | 2026-05-15 | 各 1 |
| min > max, or either was NaN | `core/num/f32.rs:1434` (clamp) | 2026-05-21 | 1 |
| texture size と texel count の不一致 | egui-wgpu `renderer.rs:615` | 2026-05-27 | 1 |
| wgpu Validation Error | wgpu `wgpu_core.rs:1970` | 2026-08-14 | 9 |
| wgpu Validation Error | wgpu `wgpu_core.rs:1588` | 2026-08-27 | 8 |
| egui layout indent | egui `ui.rs:2569` | 2026-04-14 | 1 |

- **自前のコードのものを先に見る**: `ui_main.rs` の添字 2 件、`video/dsp/gui.rs` と
  `d3d11_device.rs` の RefCell 2 件、clamp の NaN 1 件。
- wgpu の Validation Error は **2026-08 まで続いている**ので、直近の版でも起きている可能性が高い。
  同時刻の `mimageviewer.log` と突き合わせて操作を特定する。
- disposition を `filed` から動かすときは `docs/panic-acknowledged.tsv` も更新する
  (fingerprint 列は script の出力からコピーする。手で書くと正規化がずれる)。
- 規模 / 優先度: 中 / P2 (1 種ずつ独立して進められる)。

### 1.193 編集バッジがファイル名スタックへ集約されない (2026-09-07)

- 出典: v3.6.0 リリース差分レビュー (Codex Astra) の指摘 7。**v3.6.0 の退行ではない。**
- 症状: 非代表メンバーに保存済みの編集があっても、スタックの集約セルにバッジが出ない。
  レビューは新しい「切」バッジについて報告したが、**確認したところ 6 種すべてが同じ**
  (補 / レ / 消 / 隠 / 文 / 切)。
- 機構: `App::item_has_one_level_edit_key` (`src/app.rs:48200` 付近) は Folder / ZipFile /
  PdfFile / ConvertibleArchive / ZipDir / SearchContainer を扱い、
  **`GridItem::Stack` は `_ => false` に落ちる**。フォルダと書庫への集約は実装済みなので、
  スタックだけが抜けている。
- **今回入れなかった理由**: 6 種すべてに関わるので、「スタックが編集済みページを含むとは
  何か」を 1 つ決める話になる。出荷直前に広げる範囲ではないと判断した。
- 判断が要る点: スタックは代表画像 1 枚を見せる集約セルなので、
  **代表だけを見るか、畳んだ全メンバーを見るか**。フォルダ / 書庫の「1 つ上の見える親にだけ
  表示し、さらに上位へは伝播しない」規則との整合も要る
  (`htdocs/mimageviewer/manual/grid.html` の状態バッジの説明)。
- 規模 / 優先度: 小〜中 / P3。
**2026-09-07 に一度実装し、性能を測って取り下げた。**

- 実装した形: `item_has_one_level_edit_key` に `GridItem::Stack` の腕を足し、
  `StackView` へ「メンバーの正規化 page key → group index」の写像を持たせて、
  **フォルダ配下の編集済み key 側を走査**してグループ所属を引いた
  (メンバー数ではなく編集済みページ数に比例させる狙い)。
  スクリプトで作った任意のグループでも正しく、修正前に落ちる回帰テストも付けた。
- **測ったら破綻していた。** debug ビルド、50 セル × 7 バッジ × 100 フレームの 1 フレーム平均:

| そのフォルダの編集済みページ数 | スタック | フォルダ (既存) |
| ---: | ---: | ---: |
| 0 | 1.66 ms | 1.43 ms |
| 400 | 32.2 ms | 1.61 ms |
| 2,000 | 127.3 ms | 1.48 ms |
| 10,000 | 197.5 ms | 1.48 ms |

- **フォルダ側は平ら**で、スタック側だけ線形に伸びる。理由は探索範囲:
  フォルダは `sub000\` という prefix で範囲が絞れるのに対し、**スタックのキーは
  範囲を狭めない**ので、フォルダ全体の編集済みキーを歩くことになる。
  スクリプト由来のグループは prefix 関係とは限らないため、prefix で絞る手は使えない。
- **したがって、素朴な走査では入れられない。** 必要なのは
  **group → 「編集を含むか」の索引**で、7 つのキー集合それぞれの更新箇所で失効させる。
  出荷直前の変更としては大きすぎたので v3.6.0 では見送った。
- 実装と測定テスト (`measure_stack_badge_cost`) は commit していない。
  作り直すときは、上の表を再現するところから始めると判断が早い。
- **これは退行ではない。** リリース済みの版でもスタックへは集約していない。

### 1.194 入れ子 ZIP のキャッシュヒットが、外側 ZIP の更新を検証しない (2026-09-07)

- 出典: v3.6.0 リリース差分レビュー (Codex Astra) の指摘 8。
  **レビュー自身が「v3.5.0 にも存在し、今回の退行ではない」と明記している。**
- 症状: 内側 ZIP bytes のキー (`src/zip_loader.rs:510` 付近) は
  **外側 path と内部 path だけ**で、mtime / サイズを持たない。キャッシュヒットの経路
  (`src/zip_loader.rs:1113` 付近) は目次キャッシュにも到達しないため、
  **外側 ZIP を同じ path で差し替えても、`clear_nested_cache()` が走るまで旧内容を返す。**
- v3.6.0 で目次キャッシュ側は `(path, mtime, len)` で検証するようになった
  (レビュー指摘 5 の修正、`436109a62`) が、**内側 bytes の identity は揃っていない**。
  同じ修正で `clear_nested_cache()` の全消去も無くなっていないので、
  ナビゲーションで消える現状の逃げ道は残っている。
- 直す方向: 内側 bytes のキーも `ArchiveCacheKey` と同じ identity へ揃える。
  そうすれば「外側を切り替えたら全部消す」に頼らずに済む。
- 規模 / 優先度: 小 / P2 (差し替えは頻度が低いが、返る内容が古いのは静かな誤りである)。
### 1.180 外部ツールに登録できる拡張子を .exe だけに縛らない — 実装済み (2026-09-04)

- **正本は [external-tool-launch-plan.md](external-tool-launch-plan.md)**。`.py` などの
  スクリプトを登録できるようにした。`Command::spawn` が `ERROR_BAD_EXE_FORMAT` を返したときだけ
  `ShellExecuteEx` の `open` verb へ回すので、実行ファイルの起動経路は変えていない。選ぶ側に
  「すべてのファイル」を足し、ツールごとに「コンソール窓を表示する」(既定 OFF) を持たせた。
- 実装中に、**出荷済みの `external_tools` テーブルを `DROP TABLE` し得る開発中の schema reset
  経路**が残っているのを見つけて撤去した。列追加は `ALTER TABLE` で行い、登録が失われないことを
  回帰テストで固定した。
- 実機確認済み (2026-09-05)。§1.174 / §1.181 の記述は master 側の整理で閉じられたため復活させない。

### 1.191 X 予約投稿 (設計合意済み・未実装) (2026-09-06)

**正本は [x-scheduled-post-plan.md](x-scheduled-post-plan.md)。**ここには現状と残りだけ書く。

mIV から X へ指定時刻に自動投稿する。**X 専用**。予約は `x_posts.db`、投稿は Windows
タスクスケジューラが 5 分ごとに起こす `mimageviewer.exe --x-post-worker` の一発プロセス
だけが行う (mIV を一度も起動しなくても投稿される)。外部ツールからの予約は投函フォルダ
(`<data_dir>/x_inbox` へ JSON manifest を置く)。作成 UI・プレビュー・費用警告・
課金台帳・OS 起動時の最小化自動起動を含む。

- 状態: 設計合意済み・**未着手**。別 worktree (branch `x-scheduled-post`) で進める。
- 利用者判断 (2026-09-06): 既定 OFF の上級者向けとして配布 / タスクスケジューラ主体で
  常駐は前提にしない / v1 は画像 1〜4・動画・GIF・長文 (連投は入れない) / 予約の受け口は
  投函フォルダ (ローカル HTTP API は作らない) / pixiv は対象外 / 実装順は任せ。
- **有料 API を使うので at-most-once が最優先**。結果不明の送信は自動再送せず、
  X へ照合してから人に判断させる。plan §6 が中心で、P2 がその実装。
- 失敗・期限切れは**定期実行時にポップアップで知らせる** (plan §6.7)。mIV を起動
  しない運用が前提なので、一覧の印だけにしない。worker はブロックさせない。
- 未決は plan §14 (プロフィール取得 / `needs_review` の気付き方 / 動画の先行アップロード)。
- pixiv は規約を読んだうえで見送り。理由は plan §11.1 に記録済み (再提案しない)。

- 規模 / 優先度: Large / P2 (利用者本人の運用要望。閲覧機能の不具合より後)。

## 2. 一覧 / サムネイル / フォルダ走査

### 2.1 folder pane scan worker の thread 構成判断

- 背景: `scan_real_subfolders` はノードごとに短命 thread を spawn する。
- 現状: `folder_pane/scan_subfolders` perf event で ms / entry 数 / dir 数 / cancel / error を記録済み。
  cancel 付きで thread leak は見えていない。
- 方針:
  - 低速共有や大量ノード展開で遅い scan / concurrent scan が見えた場合だけ、dispatcher / pool 方式へ寄せる。
- 優先度: P3。

### 2.2 サムネイルバッジのレーン配置共通化

- 背景 / 現象:
  1. ZIP / PDF / RAR 等の形式バッジだけを従来比 70% に縮小したため、従来サイズを
     維持している緑色のフォルダ名バッジが相対的に大きく見える。フォルダ名は可変長で
     セル幅の 80% まで使うため、面積差も目立つ。
  2. ブックマーク一覧では、通常セルの左上バッジ列 (編集状態 / pin / タグ) と
     ブックマーク時刻を別々に同じ左上座標から描画している。後から描く時刻がタグの
     先頭を覆い、見た目では隠れた領域がタグのクリック判定として残る可能性もある。
  3. 動画の `UP` バッジも左上を独自使用しており、タグや編集状態バッジと同じ衝突を
     起こし得る。右上のチェック / スタック枚数、左下のコンテナ形式 / フォルダ名 /
     評価、右下の絞り込み件数も、現在は個別の座標計算または相互の高さ推定で配置している。
- 原則:
  - 共通化の単位は「全バッジを同じ見た目で描く巨大関数」ではなく、セル四隅の使用領域を
    割り当てる純粋なレイアウトとする。形式、フォルダ名、時刻、評価など意味の異なる
    バッジは、共通の計測 / padding / 配置部品を使いつつ個別スタイルを維持する。
  - 中央の再生アイコンや代替アイコンなど、隅のバッジではないコンテンツ overlay は
    この仕組みに含めない。
- 実装案:
  1. セルの `inner` と表示対象を受け、`TopLeft` / `TopRight` / `BottomLeft` /
     `BottomRight` の各レーンに実測済み `BadgePlacement` (矩形、文字 / style、優先度) を返す
     `ThumbnailOverlayLayout` 相当の純粋なレイアウト層を設ける。
  2. **v2.9.1 完了**: 第1段階は左上を移行し、ブックマーク時刻 → 編集状態 / pin → タグの順に幅を予約する。
     `UP` も同じレーンへ入れ、残り幅が不足する場合だけタグを実測で省略または非表示にする。
     描画と hover / click 判定は同じ `BadgePlacement` の矩形を使い、再計算をなくす。
  3. **v2.9.1 完了**: 第2段階は左下を移行し、フォルダ名 / ZIP / PDF / 変換アーカイブ、評価、ファイル名
     プレートの予約幅・縦積みを同じレイアウト結果から決める。フォルダ名バッジは形式
     バッジと機械的に同じ 70% へ揃えず、長い名前の可読性を残したコンパクトな専用
     スタイルをスナップショット比較で決める。
  4. **未着手**: 第3段階で右上 / 右下も同じ所有境界へ移し、チェックとスタック枚数の排他条件、
     絞り込み件数の配置を明示する。各段階を独立コミット可能にし、全経路を一度に
     書き換えない。
- v2.9.1 実装記録 (2026-07-31):
  - `src/thumb_overlay_layout.rs` に painter 非依存の純粋レイアウトを新設。セルごとに 1 回だけ
    構築した左上 / 左下の配置結果を、通常描画、タグ hit-test、`UP`、ブックマーク時刻へ渡す。
  - 左下は形式 / フォルダ名とファイル名プレートの実測矩形を下段へ置き、その行の上へ評価を
    4px gap で積む。旧 `lower_left_container_badge_height` の高さ推定は撤去した。
  - フォルダ名は旧フォントの 85%、8.5pt 下限・13.5pt 上限、横 padding 0.30em・縦
    padding 0.12em の専用 style とした。長い CJK 名の可読性を残しつつ、70% 形式バッジとの
    面積差を抑える値を snapshot で確認した。
  - 純関数 unit test 5 件と、フォルダ / ZIP / PDF / RAR、ブックマーク時刻 + タグの snapshot を
    追加・更新して目視確認済み。第3段階の右上 / 右下は未着手のまま残す。
  - 左下の item 種別 → コンテナバッジ / ファイル名プレートの対応は `bottom_left_content`
    として painter 非依存に切り出した。旧 `cell_has_lower_left_container_badge` の
    unit test 4 件 (フォルダは Loaded まではファイル名、アーカイブは常に形式バッジ) を
    同じ規則の検証として移設してある。ここは「フォルダ名と ★ が重なる」報告の退行ガード。
- **第3段階と一緒に片付ける残件 (v2.9.1 では未対応)**: 左上レーンは cursor を右へ送るだけで、
  **セル幅でクランプしていない**。時刻 + `UP` + 編集バッジ 6 個が同時に立つ狭いセルでは
  レーンが右へはみ出す (タグだけは実測で省略される)。第2段階以前からある挙動だが、
  時刻を同じレーンへ入れたぶん到達しやすくなった。直すには「溢れたときどのバッジを落とすか」の
  方針が要るので、右上 / 右下を同じ所有境界へ移す第3段階と合わせて決める。
- 完了条件 / 回帰テスト:
  - ブックマーク時刻と `#` から始まるタグが重ならず、表示矩形とクリック矩形が一致する。
  - `UP` + 編集状態 / pin + タグ、狭いセル、長い CJK タグでもバッジ同士が重ならない。
  - 長いフォルダ名 + 評価、ZIP / PDF / RAR + ファイル名 + 評価で重なりやセル外描画がない。
  - レイアウト純関数で矩形の非交差・優先順位・狭幅時の省略を unit test し、代表的な
    フォルダ / 形式バッジとブックマーク時刻 + タグを `docs/ui-snapshot-policy.md` に従う
    snapshot で確認する。
- 規模 / 優先度: Medium / P2 candidate。現在の重なりだけを座標ずらしで直す症状パッチは
  入れず、まず左上レーンの単一所有化から着手する。

### 2.7 RAR の header 全走査が、一覧判定とサムネイル生成で 2 回走る

- 出典: v3.1.2 で入れた RAR 判定の逐次公開 (旧 §2.3) の caller 監査 (2026-08-19)。その brief の
  §1.4 として着手したが、範囲を超えると判断して停止条件どおり切り出した。**v3.1.2 で閉じたのは
  「全件判定を待つ表示遅延」であって、判定そのものの重複ではない。**
- 機構: フォルダ一覧の eligibility 判定が `inspect_for_direct_read_cancelable` で RAR の
  header を全走査した後、**書庫を開くときの `zip-enumerate` worker** が
  `enumerate_image_entries_detailed` で**同じ RAR の header をもう一度全部**列挙する。
  30,000 entry 級の RAR が多いフォルダでは、この 2 回目がそのまま体感時間に乗る。
- **訂正 (2026-09-04)**: 元の記述は 2 回目の主体を thumbnail worker としていたが、**現在の
  thumbnail 経路は全走査していない** — `zip_loader::read_first_image_bytes` →
  `rar_loader::read_first_image_bytes` で最初の画像エントリを見つけた時点で打ち切る。
  同じ header を 2 回読む事実は残るが、2 回目は open 側。着手時はここを見に行くこと。
- なぜ今回入れなかったか: 1 回へ統合するには `RarInspection` が entry / 代表情報を保持し、
  thumbnail と open 双方の列挙契約を変える必要がある。逐次公開とは独立した変更で、
  同じ commit に混ぜると切り分けられなくなる。
- 直す方向:
  - `rar_loader` の判定結果が、そのフォルダ generation の間だけ代表画像選択に必要な情報も
    持つようにする。`DECISION_CACHE_CAPACITY` (32、full inspection 保持) を単に増やす修正には
    しない。メモリが増えるだけで、判定回数は減らない。
  - 統合後は **同一 RAR の header 判定回数を計装または test double で 1 回に固定する**
    (逐次公開側の完了条件として書かれていた項目をここへ引き継ぐ)。
- 計測の起点: `C:\tmp\miv-rar-thumbnail-test-100` (30,000 entry の RAR を 30 個複製)。
  v3.1.2 時点の実測は visible 12 件が 0.801〜0.860 秒、候補 #130 が 4.021 秒
  (worker 累積 3,257.5ms)。2 回目の走査を消せばここがさらに下がるはず。
- 規模 / 優先度: 中 / P2。表示は既に出るようになったので体感の急所ではないが、
  同じ I/O を 2 回やっている事実は残っている。

### 2.10 グリッド以外の egui ダブルクリック消費箇所に場所条件が無い

- 出典: v3.1.2 のダブルクリック対応の追補として行った `Response::double_clicked()` 全箇所棚卸し
  (2026-08-19)。egui 0.33.3 は
  前回クリック位置を持たないため、同じ context 内の離れた widget / 領域のクリックも時間内なら
  ダブルクリックとして各 consumer へ届き得る。
- 今回は利用者報告のあったグリッドセルだけを修正し、本項の consumer は変更しない。現存する
  8 分岐は、詳細表示の列幅 best-fit、360° navigator の視点移動、通常 navigator の zoom 中心移動、
  分析表示の zoom reset、通常 fullscreen の transform reset、360° canvas の視点 reset、native
  presenter egui overlay の音量 reset と再生速度 reset。
- native HWND が `WM_*BUTTONDBLCLK` から作る `double_click` は Windows 自身の時間・距離判定を
  通るため本項の対象外。着手時は一律のピクセル閾値を足さず、各 consumer の意味単位 (同じ
  handle / navigator / canvas / slider など) と、別 widget クリックで pair を切る owner を決める。
- 優先度: P2。latent 誤操作の棚卸し項目で、実害報告が出た consumer から個別に再現確認する。

### 2.18 リモート: サムネイルカタログが開けないだけでページ表示が失敗する — 利用者報告

- 出典: 利用者報告 (2026-08-23、v3.2.0 出荷前の実機確認)。リモートで PDF を開いたとき、
  端末に「ページ表示グループの読み込みに失敗しました。」が出た。前後ページ移動で復帰。
- **v3.2.0 の退行ではない。** この経路は `732420e3` (2026-07-31) 由来で v3.1.3 以前から出荷済み。
- **サーバ側ログで確定** (`%APPDATA%\mimageviewer-remote
emote-web-log.jsonl`):

  ```
  /api/page  status=500  page.ipc_status="miv_media_internal"
  → client: spread_load_error 「ページ表示グループの読み込みに失敗しました。」
  ```

  本体ログ (`mimageviewer.log`) の同時刻:

  ```
  [216.063s] remote_ipc: container catalog open failed: disk I/O error
  [216.063s] media_operation operation=page source_kind=pdf outcome=error
  ```

- 機構: [container.rs:5222](../src/remote_ipc/container.rs:5222) が**ページ要求のたびに
  `CatalogDb::open` し、失敗するとページ全体を `MediaErrorCode::Internal` で落とす**。
  しかしこのカタログは `decode_remote_source` に渡す**キャッシュの高速化用**で、
  ページ描画には必須ではない。**「速くするための仕組みが開けなかった」だけで表示が失敗している。**
- 発生条件: 直前の 214.28〜214.34s に PDF サムネイル要求が十数件同時にカタログを読んでおり、
  その混雑中に 1 回だけ SQLite が `disk I/O error` (`SQLITE_IOERR`) を返した。
  ログ全体でこの失敗は 1 回だけ。次の要求では成功しており、利用者の「前後移動で復帰」と一致する。
- **同時刻の `/api/page` 503 は別物**。`admission_busy` = IPC 同時実行上限 (heavy 4/4、
  prefetch 3/3) による正常な背圧なので、500 と混同しない。
- 直す方向: この経路でカタログを**省略可能にする** (`Option<Arc<CatalogDb>>`)。
  開けなければログを残してキャッシュ無しで続行する。表示は遅くなるが成功する。
  - **リトライ・待ち時間・閾値調整で吸収しない。**問題は「任意の加速機構の失敗を
    致命的に扱っていること」であって、開けなかったこと自体ではない。
  - 同じ形が他の remote 経路にもないか確認する (`CatalogDb::open` の `?` 伝播)。
  - ついでに、ページ要求ごとに開き直す構造自体も見直す価値がある (§2.13 と同型)。
- 規模 / 優先度: 小 / P2 (頻度は低いが、体験は「表示できない」なので重い)。

### 2.12 サムネイルキャッシュのクリアが `-wal` / `-shm` を残す — 実測 1.4 GB

- 出典: 2026-08-20、利用者がキャッシュクリア後に計測しようとして発見。
- **実測** (`%APPDATA%\mimageviewer\cache` 配下、クリア + 再起動後):

  | 種別 | 件数 | サイズ |
  | --- | ---: | ---: |
  | `.db` (本体) | 2 | 0.2 MB |
  | `.db-wal` | 1,410 | **1,394.8 MB** |
  | `.db-shm` | 1,410 | 44.1 MB |

  サンプルした `-wal` はすべて**対応する `.db` が存在しない孤児**だった。
- 機構: カタログは `cache/<hash 先頭 2 桁>/<hash>.db` (`catalog.rs` の `db_path_for`)。
  クリアが**その `.db` だけ**を消していると思われる。SQLite は正常な close で `-wal` / `-shm` を
  自分で削除するので、**閉じずにファイルを消すと残る**。
- 実害:
  1. **容量が解放されない。**「クリアしたのにディスクが空かない」= クリアがほぼ無意味に見える。
  2. 同じフォルダを再訪すると、その path に新しい `.db` が作られる一方、古い `-wal` が残っている。
     SQLite は salt 不一致の WAL を無視するので即座の破損は起きにくいが、**消したはずの物が残った
     状態で新規 DB を作る**のは設計として健全でない。
- 直す方向: **削除前に接続を確実に閉じる**のが構造的に正しい (閉じれば SQLite が自分で消す)。
  閉じられない経路があるなら、その理由を確認してから `-wal` / `-shm` の明示削除を足す。
  **どの経路が `.db` を掴んだまま消しているのかを先に特定する** (症状として `-wal` を消して回らない)。
- 既存の 1.4 GB は、アプリ終了後に `cache` 配下の孤児 `*.db-wal` / `*.db-shm` を削除すれば安全に回収できる。
- 規模 / 優先度: 小〜中 / P2 (実害は容量。利用者が明示的に「クリア」を選んだのに効いていない)。

### 2.14 PDF のフルスクリーンが PDFium 経路を 2 本持っている

- 出典: 2026-08-20、ワーカー起動失敗の調査 (フェーズ 1) で判明。
- **機構**: 同じフルスクリーン表示なのに、初回ページと再レンダリングで別の PDFium を使う。

  | | 初回ページ | 再レンダリング |
  | --- | --- | --- |
  | 入口 | `render_page_for_display` ([app.rs:50276](../src/app.rs:50276)) | `render_page_async` ([pdf_loader.rs:3502](../src/pdf_loader.rs:3502)) |
  | 実行場所 | ワーカープロセス 5 個 | **メインプロセス内の 1 スレッド** |
  | Critical 予約 | 効く | 効かない |
  | レーン分割・昇格 | 効く | 効かない |
  | context epoch の間引き | 効く | 効かない |

  再レンダリングはズームだけでなく、`ensure_pdf_display_resolution` による表示解像度合わせ
  (現ページ + 見開き相方、[ui_fullscreen.rs:13561](../src/ui_fullscreen.rs:13561)) も通る。
  つまり**通常の PDF 閲覧が日常的にこの経路を使う**。
- **`render_page_async` はフォールバックではない**。`worker_count` を見ずに常に
  `get_worker()` を使う専用経路で、呼び出し元は [app.rs:50765](../src/app.rs:50765) の 1 つだけ。
  ワーカー起動失敗の対応 ([briefs/pdf-worker-startup-failure.md](briefs/pdf-worker-startup-failure.md))
  では**意図的に残した**。
- 移すかどうかの論点 (2026-08-20 に整理):

  **ズーム連打では並列化は効かない。** 新しいズーム要求は前の要求をキャンセルする
  (`fs_pending.remove(&idx)` → `cancel.store(true)`、[app.rs:50710](../src/app.rs:50710) 付近) ので
  キューに積み上がらず、無駄になるのは実行中の 1 件だけ。その実行中の PDFium レンダリングは
  **プールでも中断できない** (`CancelWaitPolicy` は結果の扱いであって PDFium を止める話ではない)。
  要求は互いに上書きするので、並列に走らせても欲しいのは最新の 1 件だけ。

  **効く可能性があるのは 2 ケース:**
  1. **AI 先読みのレンダリング中にズームしたとき。** in-process は `priority_rx` を先に drain
     するが、それはキューの順序を変えるだけで、**既に走っている非 priority の先読みが
     終わるまで待つ** (実測で 1.5 秒級)。プールなら先読み = Normal、ズーム = Critical で、
     予約により最低 1 ワーカーが空いているので即座に始まる。
  2. **見開きの左右ページ。** 2 ページとも必要な独立した処理なのに今は 1 スレッドで直列。
     Critical は lane cap の対象外なので、プールなら同時に走れる。

  **払うコスト: ビットマップの IPC 転送。** ワーカーの応答は**無圧縮の生 RGBA**
  ([pdf_loader.rs:1219](../src/pdf_loader.rs:1219))。ズーム再レンダリングは長辺 8192px まで
  行くので、A4 比なら 8192 × 5794 × 4 ≒ **190 MB をパイプに流す**。in-process は mpsc で
  ポインタを渡すだけ。プールが `serialize_us` / `write_us` / `wire_bytes`
  ([pdf_loader.rs:494](../src/pdf_loader.rs:494)) を計測しているのは、このコストが効くから
  (初回ページも 4K ビューポートで約 33 MB 払っている)。

  | | 得るもの | 払うもの |
  | --- | --- | --- |
  | プールへ移す | 先読み実行中の待ち解消、見開きの並列化 | ズーム解像度で最大 190 MB の直列化 + パイプ転送 |

- **有力な第 3 案: in-process にいる 2 つを分ける** (2026-08-20 追記)。
  `render_page_async` を使っているのは性質が正反対の 2 つで、まとめて扱う理由が無い。

  | 用途 | 呼び出し元 | 性質 |
  | --- | --- | --- |
  | ズーム / 表示解像度合わせ | `request_pdf_rerender(.., true)` ([ui_fullscreen.rs:6960](../src/ui_fullscreen.rs:6960)) | **利用者操作に同期。低遅延が要る。最大 8192px と巨大** |
  | AI 先読み用の native 再レンダ | `prefetch_final_ai` → `request_pdf_rerender(idx, 1.0, false)` ([app.rs:52845](../src/app.rs:52845)) | **完全にバックグラウンド。遅延は問題にならない** |

  なお**通常のページ先読みは既にプール側**なので競合しない
  (`ensure_fs_page_load` → `start_fs_load` → `render_page_for_display`、
  [app.rs:56551](../src/app.rs:56551))。in-process にいるのは上の 2 つだけ。

  **AI 用 native 再レンダだけをプールへ移す**と:
  - ズームは in-process のまま = レスポンス劣化の懸念が無い
  - in-process スレッドがズーム専用になり、**先読みに塞がれなくなる** (= 上の「効く 2 ケース」の
    ①がプール移行なしで解消する)
  - IPC 転送コストは AI 側が払うが、そちらは元々バックグラウンド
  - 見開き並列 (②) は解決しない

  **①が主因だと実測で分かったら、この案を先に検討する。**

- **測るもの** (上の表がそのまま測定項目になる):
  1. **先読み実行中のズーム要求の待ち時間** — in-process (先読み完了まで待つ) vs
     プール (予約ワーカーで即開始)。
  2. **ズーム解像度での IPC 転送コスト** — 既存の `WorkerRenderMetrics` がそのまま使える。
     長辺 2048 / 4096 / 8192 で `serialize_us` + `write_us` を取る。
  3. **見開き 2 ページの完了までの時間** — 直列 vs 並列。

  1 と 3 の得が 2 の損を上回るかで決める。§2.13 の計装と同時に入れると安い。
- 規模 / 優先度: 中 / P3 (現状で壊れてはいない。実測して有意差が出たときに動かす)。

### 2.19 バックグラウンドインデクサの残り時間が「まだ出せない」と「壊れている」を区別できない — 利用者報告

- 出典: 2026-08-23。お気に入り編集ダイアログの「🔄 バックグラウンドインデクサ」に残り時間が出ておらず、
  機能を消したのか不具合かという問い合わせ。しばらく待つと `[残り 02:12 (215件/秒)]` が出た
  (= スキャンから取込フェーズへ進んだ)。**現行仕様どおりの挙動で、退行ではない。**
- 機構: 残り時間は「残件数 ÷ 直近レート」で出しているので、**全体件数 N が確定しているフェーズ**でしか
  計算できない。

  | フェーズ | 呼び出し | 残り時間 |
  | --- | --- | --- |
  | 削除 (n/N) / 取込 (n/N) | `set_msg_and_count` ([ingest_worker.rs:218](../src/ingest_worker.rs:218), [:253](../src/ingest_worker.rs:253), [name_bulk_indexer.rs:150](../src/name_bulk_indexer.rs:150)) | 出る |
  | スキャン: `<dir>` (n 件) | `set` ([search_walker.rs:246](../src/search_walker.rs:246)) | 出ない |
  | 取込待ち (writer 使用中) | `set` ([indexer_supervisor.rs:534](../src/indexer_supervisor.rs:534)) | 出ない |
  | 名前索引の更新 / 更新スキャン | `set` ([name_index_supervisor.rs:386](../src/name_index_supervisor.rs:386), [:673](../src/name_index_supervisor.rs:673)) | 出ない |

  counted フェーズに入った直後もサンプル 2 点かつ件数が進むまでは `remaining_secs=None`、
  取込完了時は `clear_count()` で消える ([indexer_progress.rs](../src/indexer_progress.rs))。
- 問題: 「まだ計算できない」「そもそも総数の概念がない」「壊れている」が**すべて同じ無表示**で、
  利用者から区別できない。大きいフォルダ / HDD・NAS / 操作中で ActivityGate が walk を止めている
  条件ではスキャンが数分続き、その間ずっと空欄のままになる。
- 直す方向: 空欄をやめ、フェーズに応じた理由を出す (例: スキャン中 = `[件数集計中…]`、
  counted 直後 = `[残り 計測中…]`、差分更新 = 表示しない)。
  - **理由は型で持たせる。** 現状 UI は `eta: Option<EtaSnapshot>` の有無しか見ておらず、None の理由
    (カウント未設定 / サンプル不足) を区別できない。`ProgressReporter` 側に理由を持たせ、UI はそれを
    表示するだけにする。**メッセージ文字列の前方一致で "スキャン:" を判定するのはやらない**
    (文言を変えた瞬間に静かに壊れる)。
  - `aggregate_total_eta` ([favorites_editor.rs:938](../src/ui_dialogs/favorites_editor.rs:938)) は
    「残り = max(各 remaining)、速度 = Σ(各 rate)」で集約している。理由付き None が混ざったときの
    集約規則 (1 つでも計測中なら全体も計測中扱いにするか) を決める。
  - タイトルバー側の ETA 表示 ([app.rs:10296](../src/app.rs:10296) のキャッシュ経由) も同じ扱いに揃える。
- 規模 / 優先度: 小 (表示のみ。索引の動作自体は変えない) / P3。

### 2.20 別ウィンドウで動画が切り替わると、静止画ウィンドウの画像が縦横比の狂った状態で固着する — 利用者報告

> ⚠ detached viewer リワーク中の領域。**症状パッチを入れない** (CLAUDE.md 凍結ルール)。
> 分類は **BA-7** (bundle 外の App-global 状態が別 context から汚染される)。
> findings-12 D3 (book open が main context 経由で `auto_aspect` を汚し、メイングリッドが
> アスペクトリフローした) と**同型**。

- 出典: 2026-08-23。静止画をフルスクリーンで見ている最中に、別ウィンドウで動画が切り替わった
  タイミングで、静止画が**圧縮されて縦横比の狂った状態**で表示された。**画像を開き直すと復帰**
  (= 1 フレームのちらつきではなく、再解決まで残る状態)。
- ログで確認できた事実 (`mimageviewer.log`、セッション経過 6284〜6296s):
  - 独立した 2 つの viewer session が同時に開いていた。
    session 31 = 静止画 (`g:\home\comfyui\...`、items_gen=24 / 300 件、表示中 idx=96 =
    `mistblossom_gpt_4_6...png` **896x1152**)、session 4 = 動画 (`c:\home\youtube\...`、
    items_gen=6 / 171 件)。
  - **動画側の切り替えが App-global の `open_fullscreen` を通っている。**
    `[6285.031s] === open_fullscreen: idx=2 ===` は利用者が静止画側で操作した記録ではなく、
    動画ウィンドウの swap ([app.rs:44508](../src/app.rs:44508))。
    9 秒間に 6 回 (6284.9 / 6285.8 / 6290.7 / 6291.7 / 6292.3 / 6293.0) 発生している。
  - つまり静止画ウィンドウの描画中に、`fullscreen_idx` を含む App-global が動画ウィンドウの
    都合で書き換えられていた。
- **コード読みで確認できた構造** (この症状の原因と断定はしていない。候補の絞り込み):
  - `DisplayedImageTransform::resolve` は `full_image_rect = display_size * total_scale` の
    一様スケールなので、**この経路では縦横比は歪められない**
    ([displayed_image_transform.rs:167](../src/displayed_image_transform.rs:167))。
  - 歪みが表現できるのは `from_resolved_rect` だけ。呼び出し側が渡した矩形に対して
    `scale_x` / `scale_y` を別々に出し、**食い違っていても平均して受け入れる**
    ([displayed_image_transform.rs:183](../src/displayed_image_transform.rs:183))。
  - この穴は認識されていて、`resolve_fs_transform_in_layout_rect` が「レイアウト矩形は枠に
    すぎないので、実テクスチャから最終矩形を出し直す」ガードとして用意されている
    ([ui_fullscreen.rs:4094](../src/ui_fullscreen.rs:4094) の doc comment が
    "no caller can publish a non-uniform pixel scale" と明記)。
  - **単ページ ([ui_fullscreen.rs:24707](../src/ui_fullscreen.rs:24707)) と連結読み
    ([:29060](../src/ui_fullscreen.rs:29060)) はこのガードを通る。通っていない呼び出しが 2 つある**:

    | 場所 | 渡している矩形 | 由来 |
    | --- | --- | --- |
    | 見開き左右 ([:10381](../src/ui_fullscreen.rs:10381) / [:10397](../src/ui_fullscreen.rs:10397)) | `rects.left_rect` / `right_rect` | `spread_layout_geometry(left_size, right_size, ...)` = **ページ寸法**。`texture_size` には実テクスチャを渡すので、両者の比が食い違えば歪む |
    | detached の凍結スナップショット ([:10172](../src/ui_fullscreen.rs:10172)、`detached_continuous_frozen_pages_for_snapshot`) | `page.rect` | レイアウトが決めた矩形をそのまま渡している。直前に一様な `logical_scale` を計算しているのに、矩形側には反映していない |

  - 後者は **detached ウィンドウが凍結描画に落ちるときの経路**で、「別ウィンドウ側の操作で
    こちらが凍結スナップショットに切り替わる」という今回の状況と形が合う。**ただし利用者が
    単ページ / 見開き / 連結読みのどれで見ていたかはログに出ていない** (`spread` / `連結` の
    ログ出力が 1 件も無い) ので、経路は特定できていない。

- **上の 2 経路は塞がった (2026-09-04 に確認)。この節に残るのは Codex 追補の側だけ。**
  - 見開き左右も detached の凍結スナップショットも、いまは `resolve_fs_spread_page_transform`
    という 1 つの入口を通る。レイアウト矩形を `from_resolved_rect` へ直接渡さず、
    `fit_display_size_in_rect` で一様に letterbox してから `resolve_fs_transform_in_layout_rect`
    を呼ぶ (`e2987744c` / `76148e06a` = v3.4.0、`00680be94` = v3.5.0)。
  - **ただし `from_resolved_rect` 自体は今も非一様を無言で受け入れる。** `scale_x` と `scale_y`
    を別に出して平均するだけで、拒否もログもない。「次にやること」に書いた
    `|scale_x - scale_y|` の計装は未実施。**別の呼び出しが増えたときに同じことが起きる。**
  - **Codex 追補の placement 所有境界はそのまま**。最大化時の placement 更新は実測 `outer_rect`
    を捨てて seed placement に `maximized = true` を立てて書き戻し、passive 側は正規化矩形を
    現在の `full_rect` へ X/Y 独立に戻すだけで、`image_dims` からの contain / 一様 scale の
    再解決を通らない。
- **Codex 追補 (2026-08-23、ClaudeCode 分析との差分)**:
  - perf ログでは、静止画を開いた直後の `896x1152` final composite は
    `scale_x=scale_y=1.188666`、Lanczos も `1597x2054` で、画像データ・texture・通常の
    transform は縦横比を保っていた。6293.759s の開き直しも同じ cache の hit だけで直っており、
    cache 内容の破損ではない。
  - 時系列では、動画の実 source swap (6285.031s) より先に、ParkedLive 動画窓のクリック復帰が
    6283.984s に queue されている。この復帰入口は、現在の静止画を
    `park_and_close_current_active_detached_viewer_for_media_handoff` で passive snapshot にしてから
    動画 bundle を active にする ([app.rs:40136](../src/app.rs:40136) →
    [:40414](../src/app.rs:40414) → [:42678](../src/app.rs:42678))。したがって、利用者からは
    「動画切替時」に見えても、歪んだ表示矩形が作られる候補時点は source swap より約 1 秒前の
    **active still → passive snapshot handoff** である。
  - この legacy still park は `build_active_detached_image_window_snapshot(None)` を呼ぶ
    ([app.rs:42583](../src/app.rs:42583))。`ctx=None` なので `frozen_continuous_pages` は必ず空になり、
    passive 描画は mode にかかわらず `window.image_rect_norm` の単画像 fallback を使う
    ([app.rs:41752](../src/app.rs:41752)、[ui_fullscreen.rs:10641](../src/ui_fullscreen.rs:10641))。
    今回の経路については、ClaudeCode 分析が未確定としていた見開き / 連結の
    `from_resolved_rect` 候補より、この fallback の方が直接対応する。
  - 静止画 host は 6282.159s に `rect=(-11,-11 3862x2110)` で登録されており、Windows の最大化
    outer rect と整合する。一方、maximized の placement 更新は実測 client size ではなく
    restore 用 seed の `w/h` を runtime に保持する ([app.rs:43781](../src/app.rs:43781))。
    snapshot bake はその placement の `w/h` で画像矩形を fit して正規化する
    ([app.rs:41689](../src/app.rs:41689)、[ui_fullscreen.rs:10242](../src/ui_fullscreen.rs:10242)) が、
    passive draw は正規化矩形を現在の `full_rect` へ X/Y 独立で戻すだけで、保存済み
    `image_dims` による contain / 一様 scale の再解決を行わない。restore 窓と最大化 viewport の
    アスペクトが違えば、この最後の写像だけで texture が非一様に圧縮され得る。
  - したがって、App-global `open_fullscreen` の書換えは handoff の起動条件ではあるが、それ自体が
    passive 静止画の geometry を汚染した証拠はまだない。Codex の第一候補は **BA-7 の global
    state 汚染ではなく、maximized 時の restore placement と live viewport を混ぜた snapshot
    geometry ownership の不一致**。数値の最終確認には、既存案の `from_resolved_rect` 計装に加え、
    snapshot bake の `placement/maximized/image_rect_norm` と passive draw の
    `full_rect/復元後 image_rect/scale_x/scale_y` を同じ window/session id で記録する必要がある。
    `from_resolved_rect` は bake 前の一様な矩形だけを見て正常終了し、その後の
    `rect_from_normalized` で生じる歪みを捕捉できない可能性がある。
- **次にやること (直す前に)**: 無言で受け入れている所を鳴らす。
  `from_resolved_rect` で `|scale_x - scale_y|` が閾値を超えたら、`page_idx` / `texture_size` /
  渡された矩形 / session を添えてログに出す。加えて active still → passive snapshot では、bake
  時の `placement/maximized/image_rect_norm` と draw 時の `full_rect/image_rect/scale_x/scale_y` を
  同じ window/session id で記録する。**推測で直さない** — 上の候補のどれでもない可能性が残って
  いる。再現手順は「静止画ウィンドウを開いたまま、別ウィンドウで動画を数回切り替える」で、
  利用者側では再現している。
- **直す方向**: 特定できたら、`from_resolved_rect` の一様でない矩形を**受け入れずに拒否**するか、
  全呼び出しを `resolve_fs_transform_in_layout_rect` 経由に寄せる。ガードが既に存在して
  文書化までされているのに 2 経路が素通りしている状態を、分岐追加ではなく入口の一本化で閉じる。
- 規模 / 優先度: 中 / P2 (表示の破綻だが開き直せば復帰する。凍結領域なので構造修正が前提)。
- **2026-08-23 追記 — §1.115 と根が同じ。** 最大化された別ウィンドウが表示されている間、
  placement 更新が**毎フレーム走り、82/82 が中身の変わらない書き込み**であることを
  `MIV_DETACHED_WINDOW_DEBUG` のログで確認した。maximized 分岐が実測 rect を捨てて restore 用
  seed の w/h を書き戻すため ([app.rs:43782](../src/app.rs:43782))、最大化中の placement は
  実ジオメトリを表さない。**Codex がここで指摘した「bake は placement の w/h、draw は最大化
  viewport の `full_rect`」というズレの供給源がこれ。** 直し方は §1.115 の案 A
  (現在ジオメトリと restore ジオメトリを別に持つ) で、両方の入力が同時に正しくなる。
- 関連: §1.115 (**同じ根**。最大化中のちらつき)、
  [docs/detached-rework-plan.md](detached-rework-plan.md) §2 (憲法) / BA-7、
  findings-12 D3 (同型の App-global 汚染)。

### 2.23 タグの付与時刻で並べる / 見せる — 利用者要望 (2026-08-30)

**背景**: 「間違えて付けたタグを取り消したい」ときに、付けた順で見たい。レーティングの
★設定時刻まわり (旧 §1.142 / §1.143、v3.5.0 で実装済み) の調査中に、同じ話がタグにもあると
分かった。

**データはもうある**: `tags.db` の `item_tags.applied_at` (item×tag 単位、NOT NULL) と
`tag_item_state.decided_at` (item 単位) — [tags_db.rs](../src/tags_db.rs)。スキーマ変更は
要らない。

**無いのは並べ替えと表示**: タグビューの結果は [tag_view.rs](../src/tag_view.rs) で
`entries.sort_by(|a, b| a.path.cmp(&b.path))` とパス昇順に固定されていて、付与時刻ソートも
詳細表示の列もツールチップ行も無い。

**着手前に決める論点**:

- 複数タグを持つ項目で、どの `applied_at` を出すか。**そのビューで検索対象になっている
  タグの `applied_at`** (AND / OR なら最大値) が自然。「全タグの最大値」にすると、無関係な
  タグを後から足しただけで先頭へ来てしまう。
- タグビューにビュー固有ソート (レーティングの `RatingViewSort` 相当) を持たせるかどうか。
  持たせるなら**レーティング一覧と同じ構造をなぞる**。実装は v3.5.0 で入っているので、
  設計を起こさずコードを読めばよい:
  - 時刻ソート中はカテゴリ再配置を通さない — `RatingViewSort::arranges_by_category` と
    [grid_item.rs](../src/grid_item.rs) `materialize_view_rows`
  - 詳細列と sort key はビュー限定 — `DetailsColumnId::RatedAt` /
    `DetailsSortKey::RatedAt` と `App::details_sort_key_visible`
  - ビューへ入るとき列ソートの所有権を返す — `App::reset_details_sort_to_toolbar`
    (メニュー経路と履歴で戻る経路の両方に置く)
  - **新しい enum variant を settings.db へ書かない** — `stash_details_rated_at_for_persist`
    と同じ退避が要る (released 版が読めなくなるため。[settings.rs](../src/settings.rs))
- タグビューの対象は実ファイルのみ (ZipImage / PdfPage は入らない) — 既存仕様。

**優先度**: P3 / 余裕があるとき。

## 3. 補正 / AI

### 3.1 local-adjust layers の入場時同期 DB 読み

- 背景: フルスクリーン入場初回フレームで `LocalAdjustDb::get_layers` を同期実行する。
- 現状: フォルダ open 一括読みを避けるための意図的 tradeoff。
- 方針:
  - 数十 MB 級ページで hitch が報告 / 計測された場合に worker 化する。
  - read-only 経路の not-loaded は現状どおり None 返しを維持する。
- 優先度: P3 monitor。

### 3.3 ページ送り中の AI アップスケールを待たない / 打ち切る

- 出典: 元画像プレビュー中の見開き 1 ページずらし停止 (2026-08-17 修正済み) の実 perf log。
  停止中に AI upscale 1.3s × 3 回が
  逐次実行され、5.22s / 4.30s の停止時間の大半を占めた。
- その修正は「元画像を描く frame が加工済み source を readiness として待つ」不整合だけを直した。
  AI コストそのものは変更しておらず、元画像表示以外でも通過先 / stale target の推論待ちが起こり得る。
- 方針: ページ送りで表示 target から外れた AI upscale を待たず、context-owned producer の cancel / 打切りと
  着地点だけの再開を ownership・generation・完了回収まで含めて設計する。時間窓や `Awaiting` の強制解除、
  AI 結果の silent fallback では直さない。
- 規模 / 優先度: 中 / P2 performance。

## 4. 入力カスタマイズ / マウス / ゲームパッド

### 4.1 Shift / Alt + ホイールのカスタマイズ再設計

- 背景: v1.7.0 のリングショートカット / マウスボタン実装中に、Shift / Alt + ホイールのペアバインドを
  追加候補にしたが、実機確認で動画まわりの退行リスクが高いと判断した。
- 方針:
  - v1.7.0 では公開 UI / 入力経路から外し、通常ホイール、Ctrl+ホイール、中ボタンドラッグの既存挙動を維持する。
  - 将来再開する場合は、グリッド / 画像フルスクリーン / 動画フルスクリーンを別々に設計する。
  - native video overlay の consumed wheel、modifier 転送、動画タイルの Ctrl+ホイール、編集パネル / スクロールパネルとの
    優先順位を先に決める。
- 実装メモ: `ring_shortcuts.shift_wheel_pair` / `alt_wheel_pair` は互換読み込み用フィールドとして残すが、
  現行 UI / 入力経路からは参照しない。
- 補足: マスク編集モードで筆系ツールを選び、キャンバス上にカーソルがある場合だけの
  `Shift+ホイール` は筆半径変更の固定入力として別途実装する。一般の入力経路や上記互換フィールドへ
  接続せず、パネル上・筆以外・動画では消費しないため、本項の横断カスタマイズ再設計を再開したものではない。
- 規模 / リスク: Medium / 中。動画系の手動確認を含めて別タスクで扱う。

### 4.2 動画の通常ホイールとマウス進む・戻るボタンの割り当て範囲を拡張する — 利用者要望

- **利用者判断でマウスseek割り当てのみ次版へ延期（2026-09-08）。**
  [将来版バックログ](backlog-on-hold.md)に未解決の通知重複と再開条件を登録した。
  v3.7.0には通常ホイール音量と小・中・大シークの秒数設定を残す。
  保存済みマウス割り当て値を保持しつつ、6種seekの新規選択と実行を止める変更を進める。
  以下は延期前の調査・実装履歴であり、現行リリース範囲とは区別する。
- **22:08buildでも二重シーク未解消、再調査中。** 利用者は1クリックで依然2回発動し、
  押しっぱなしも左右キーと違うと報告した。フォルダ移動は1回に見えるとの比較情報があり、
  入力数とnavigation側の抑制を切り分ける。以下の修正・自動検証は製品解消の証拠とは扱わない。
- **前回の追加修正記録（20:02buildへの利用者報告）:** 通常ホイールの音量切替は良好。
  進む・戻るへシークを割り当てると1クリックで約2回進むと報告された。
  既存実ログでbrowserキーと、その標準処理が生成したAPPCOMMANDの二重配送を確認した。
  browserキーの押下をnative入口で処理済みにし、直接APPCOMMANDの経路は維持する修正を実装した。
  入力経路の焦点検証4件が成功。追加差分の独立検収・全体gateも成功し、22:08に確認用buildを更新した。実機確認待ち。
  以下の既存自動検証の成功だけでは、この実機報告が解消したとは扱わない。
- **実装・非対話検証完了、実機確認待ち（2026-09-08）。** 通常ホイールは項目移動／音量調整、
  マウスボタンは小・中・大の前後シークを選択できる。秒数は各1〜600秒から設定可能。
  独立レビュー、焦点回帰、統合 `test-full.ps1` と確認用buildは成功済み。
  実アプリ確認の明示了承待ちのため本節を保持する。以下の「着手前の状況」は旧状態である。
- **2026-09-08 利用者指定: 次版 v3.7.0 の対応対象へ追加。** このタスクの既存優先作業の後に
  続けて設計・実装する。進行順と検証待ちは [作業台帳](v3.7.0-priority-work.md) で管理する。
  実アプリの起動・操作を伴う検証は、リリース前等に内容・所要時間を示し明示了承後だけ実施する。
- 出典: 専用スレ >>361 (2026-09-06)。通常ホイールを音量調整、マウスの進む・戻るボタンを
  2 秒程度の短いシークへ割り当てたいという要望。
- 着手前の状況:
  - 動画のキー操作は操作カスタマイズに対応している。
  - マウスの進む・戻るボタンもグリッド / 画像 / 動画ごとに割り当てを保存できるが、動画用の候補に
    短いシークが無いため、希望する割り当てにはできない。
  - 動画表示中の修飾キーなしホイールは前後ファイル移動に固定され、公開設定から変更できない。
  - `WheelPairActionId::VideoVolumeUpDown` と Shift / Alt 用の互換フィールドは残っているが、現行の
    UI / 入力経路には接続されていない。これをそのまま再公開せず、§4.1 の競合条件を確認して設計する。
- 要求:
  - 動画表示中の通常ホイールを、少なくとも現行の前後ファイル移動と音量調整から選べるようにする。
  - 動画フルスクリーンのマウス進む・戻るボタンへ、短いシークを割り当てられるようにする。
  - 既定値は現行動作を維持し、既存利用者のホイール / マウスボタン操作を自動変更しない。
- 設計時に決めること:
  - ホイール設定を動画専用の限定的な選択肢にするか、汎用のペアアクションとして再設計するか。
  - 既存の「動画を 1 秒戻す / 進める」をマウスボタン候補へ加えるだけにするか、短いシーク秒数を
    設定可能にするか。要望の「2 秒くらい」は厳密な 2 秒指定とは限らないため、設定を増やす前に確認する。
  - パネル / シーク表示上のスクロール、動画ズーム、360 度表示、動画タイルの Ctrl+ホイールなどが
    所有する入力を優先し、それらで消費されなかった通常ホイールだけへ割り当てを適用する。
  - native presenter、メイン内表示、F12 / 複数ウィンドウで同じ設定と優先順位になるよう、入口だけでなく
    各経路の消費結果まで確認する。
- 検証:
  - 設定の保存 / 読み戻し / sanitize、動画文脈の候補一覧、進む・戻るボタンの action dispatch をテストする。
  - 通常動画、ズーム、360 度、タイル、パネル / シーク表示上、F12 / 複数ウィンドウでホイール競合を確認する。
- 規模 / 優先度: 進む・戻るボタンへの短いシーク追加は Small。動画専用の通常ホイール設定は
  Small-Medium。全画面・全修飾キーを含む汎用ホイール再設計は §4.1 と同じ Medium / P2 enhancement。

## 5. リリース前確認 / 依存更新

> 版ごとに実際に取った測定値 (perf smoke / idle health / bench / 依存確認) は
> [release-verification-records.md](release-verification-records.md) にある。ここには
> **手順そのものの未解決点**だけを置く。

### 5.0 次版の「重要な変更点」に載せるもの

[src/version_highlights.rs](../src/version_highlights.rs) の `TABLE` へ書く候補をここに貯める。
**既定の挙動が変わったもの**は `must_read` に入れる (更新後初回起動で自動表示される。
ここに載せないと利用者に伝わらない)。リリース手順 Phase 1 の 5.5 で `TABLE` へ書き写し、
**書いたらこの節から消す**。

| 変更 | 区分 | 入る版 |
| --- | --- | --- |
| 右クリックの最上位に「切り取り」「コピー」を追加 (一覧・フルスクリーン)。<kbd>Ctrl</kbd>+<kbd>X</kbd> / <kbd>C</kbd> を操作カスタマイズで変更・解除できるようにした | 新機能 | 次版 |
| 「非表示 N 件」の内訳で、設定で無視した RAR / 7z / LZH を分けて表示し、内訳ごとに対応する設定への導線を出すようにした | 新機能 | 次版 |
| 静止画・漫画のページシークバーに、サムネイル列とマウスオーバープレビューを追加 (プレビューは既定 ON、列は既定 OFF)。動画側にもプレビューと通常バーの表示設定を追加 | 新機能 | 次版 |
| Ctrl+F の AI プロンプト検索を高速化 (必要な範囲だけ読む + 8 並列) | 新機能 | 次版 |
| フルスクリーンで JPEG を開くときに、ファイルを 2 回読んでいたのをやめた (初めて開くフォルダのページ送りが速くなる) | 新機能 | 次版 |

v3.5.0 ぶんは記載済み (必読 3 件 = 右クリックの Windows 項目の移動 / 時刻順で種類をまとめない /
`Shift+S` 巡回が 5 段階、新機能 4 件 = 外部ツール連携 / シークストリップ全体表示 /
情報パネル固定 / 一括エクスポート)。候補に挙げていた「隠蔽加工プリセット出力」「編集内容の
一括貼付・リセット」「360 の引き継ぎ」「★設定時刻列」は、ダイアログを短く保つため
**載せない判断**をした (挙動の変更ではなく追加機能なので、更新履歴で足りる)。


### 5.13 マニュアルの右クリックメニュー画像を撮り直す

- 対象: `htdocs/mimageviewer/manual/images/tut-file-ops-1.webp`。
- §1.179 で右クリックメニューの先頭に「切り取り」「コピー」が入ったが、この画像は
  それ以前のメニューのまま。**alt テキストは現物に合わせて据え置いてある**ので、
  撮り直したら本文・alt の両方を新しいメニューに合わせて直す。
- 撮り方は [screenshot-howto.md](screenshot-howto.md)。実ファイルを右クリックした状態で撮る。
- 規模 / 優先度: 小 / リリース前。

### 5.12 署名の事前チェックが、鍵の使えなさを検出できない

- 出典: v3.2.0 の配布ビルド (2026-08-23)。`Assert-MivSignReady` を通過したのに、
  実署名の 1 本目 (`vendor/pdfium/bin/pdfium.dll`) が
  `SignTool Error: No certificates were found that met all the given criteria.` で失敗した。
- 機構: SimplySign のクラウド鍵セッションが切れると、**証明書の公開情報は
  `CurrentUser\My` に残ったまま秘密鍵だけが外れる**。`Assert-MivSignReady` は
  証明書の存在を見るだけなので通過してしまう。
- 実害: `build-dist.ps1` は clean からビルドしてから署名するので、
  **40 分ビルドしてから落ちる**。しかも中途半端な成果物は残らないので、
  丸ごとやり直しになる。
- 直す方向: 事前チェックを「証明書が見えるか」から「**実際に署名できるか**」へ変える。
  使い捨ての小さな PE を 1 本コピーして `Invoke-MivSign` し、成功したら消す。
  数秒で済み、セッション切れならビルド前に分かる。
  - 署名対象を汚さないよう、必ず一時ディレクトリのコピーに対して行う。
  - タイムスタンプ取得でネットワークへ出るので、失敗理由を
    「鍵が使えない」「TS サーバに届かない」で区別できると望ましい。
- 回避策 (現状): 配布ビルドの前に手で 1 本署名してみる。
- 規模 / 優先度: 小 / P2 (リリースのたびに 40 分を失うリスク)。

### 5.1 ネイティブ依存

| 対象 | 現状 / 次の確認 | 注意点 |
| --- | --- | --- |
| VST3 SDK / bridge | C++ ソース変更がなければ再ビルド不要 | 更新時は商用プラグインで実機確認 |
| PDFium | v3.4.0 サイクルで `chromium/8035` へ更新済み (`vendor/pdfium/VERSION` が正) | 更新後は通常 / パスワード付き PDF の表示とページ数を実機確認 |
| FFmpeg | v3.4.0 サイクルで `n7.1.5-16-g9a4bb2c579` へ更新済み (**DLL の ProductVersion が正**。`vendor/ffmpeg/VERSION` はローリング名を掴んで腐る) | DLL・VERSION・LGPL 対応ソース・製品ページの FFmpeg 節を同じ commit に揃えて動画 / 音声を実機確認 |

### 5.2 Rust クレート

- 互換範囲の一括更新とは分け、次のメジャー更新 / rc 脱出は個別判断する:
  - `ort`
  - `pdfium-render`
  - `ffmpeg-the-third`
  - `image`
  - `zip`
  - `sevenz-rust2`
  - `delharc`
  - `unrar`
  - `turbojpeg`
- 更新後に確認するもの:
  - `cargo test`
  - 検索 bench 回帰
  - perf smoke
  - `dumpbin /dependents` で不要な VC runtime DLL が復活していないこと
- **quick-xml の非推奨 API 追随** (v3.0.0 で 0.39 → 0.41、advisory 対応)。
  `src/xmp_reader.rs` の 4 箇所が非推奨警告を出す:
  `decode_and_unescape_value` → `decoded_and_normalized_value`、
  `unescape_value` → `normalized_value`。
  **0.41 では非推奨側が新実装へ委譲済み**なので、いま呼んでいる限り挙動は
  `normalized_*` と同じ (= XML 仕様どおり属性値の改行 / タブが空白になる)。
  影響するのは**属性値だけ**で、`xtw:*` (ツイート情報の表示用テキスト) と
  `rdf:resource` / `xmp:Rating` / GPano (いずれも URI か数値) にとどまる。
  **タグ (`dc:subject`) は `Event::Text` 経由**で非推奨メソッドを通らないため無関係。
  次の更新で削除される前に呼び出しを置き換える。

### 5.9 リリース手順: 配布ビルド前に core / remote の存在が要る

- 出典: v3.0.0 リリース前確認 (2026-08-14)。`scripts/test-full.ps1` が
  **テストを 1 件も実行しないままビルドエラーで落ちた**。
- 原因: `--workspace` に launcher package が入り、その build.rs が内包対象の
  `target\release\mimageviewer-core.exe` と `mimageviewer-remote.exe` の存在を検査する。
  remote は v3.0.0 で増えたので、それ以前の成果物しか無い環境で初めて表面化した。
- `build-dist.ps1` は test-full を `cargo clean` の**前**に回すため、直前のビルドが
  残っている通常のリリース機では起きない。新しい clone や `cargo clean` 直後に起きる。
- 当座の回避と前提は CLAUDE.md「開発中のビルド・テスト選択」に記載済み。
- 直す方向: test-full が不足分を先に build するか、launcher を `--workspace` の
  テスト対象から外す (launcher 自身に unit test があるかを確認してから決める)。
- 規模 / 優先度: 小 / P3。

### 5.3 リリース / テストスクリプトの並行実行耐性とクリーン環境再現性

- 出典: v2.8.0 リリース前確認 (2026-07-26)。別セッションで
  `scripts/build-release.ps1` を実行中に通常並列の `scripts/test-full.ps1` を実行すると、
  `mimageviewer` のライブラリテストが assertion / panic を出さず `0xffffffff` で終了した。
  リリース処理の完了後、同じコミットを競合プロセスなしで再実行すると正式ゲートは
  `[test-full] PASS` で完走したため、製品テストの並列不具合ではなくリリースツール間の
  干渉と判定。v2.8.0 の出荷は止めず、次版でツールを堅牢化する。
- **P2: `build-release.ps1` の停止対象が広すぎる (v2.9.1 対応済み)**:
  - 原因はプロセス名の前方一致 `mimageviewer*`。Cargo のテストハーネスは
    `target\debug\deps\mimageviewer-<hash>.exe` で、**リポジトリ配下にあるため path 判定も
    通過**し、別セッションの `cargo test` ごと `Stop-Process -Force` していた。
  - 停止対象を `mimageviewer` / `mimageviewer-core` / `mimageviewer-vst3-host` /
    `mimageviewer-susie32` の完全名 allowlist に変更した。`build-portable.ps1` も同じ
    欠陥を持っていたので揃えた (`build-dev.ps1` は元から完全名指定で影響なし)。
  - path を読めないプロセス (昇格 / 保護) は従来どおり停止するが、allowlist を通った
    ものだけになったのでテストハーネスは対象外。
  - リポジトリ単位の test / release 相互排他ロックは**入れていない**。名前の取り違えが
    原因だったので、まず allowlist だけで様子を見る。再発したらロックを検討する。
- **P2: クリーン環境の正式テストゲートが release core に依存する**:
  - `scripts/test-full.ps1` の workspace test には launcher が含まれるが、launcher の
    build script は既存の `target\release\mimageviewer-core.exe` を要求する。このため
    release core がないクリーン checkout ではテストゲートを開始できず、既存成果物が
    ある開発環境でだけ成功する。
  - 対応案 = launcher のテストを正式ゲート内で別段に分けて必要な core を明示的に用意
    するか、launcher のテストビルドを埋め込み用 release core から分離する。
  - 完了条件 = `target\release\mimageviewer-core.exe` が存在しないクリーン環境から
    `scripts/test-full.ps1` が完走し、launcher のテストも省略されないこと。
- 規模 / 優先度: Small〜Medium / P2。いずれも製品 runtime の品質問題ではなく、
  同一 worktree で複数セッションを使うリリース運用とクリーン再現性の改善。

### 1.84 表示キャッシュに item-context 世代の刻印が無い (latent、ABA 危険)

- 出典: v3.0.0 出荷前の holdover 調査 (2026-08-14) で Codex が併せて指摘。
  **今回の不具合 (`docs/briefs/pdf-page-turn-and-stale-composite-plan.md`) の原因ではない**
  が、同じ調査で見つかった同型の穴。
- **5 項目のうち 3 つは済んでいる** (2026-08-14、`4ee159439` / `4c0779be6`、v3.0.0 で出荷)。
  `EditResultKey` は `item_id` と `items_generation` を持ち、`FinalCompositeKey` はそれを内包する。
  `current_edit_result_texture` は `idx` / `items_generation` / `item_id` の 3 条件で照合し、
  `current_final_composite_texture` は `edit_key_describes_current_page` を通る。
- **残っているのは 2 つ**。どちらも `idx` だけで引いており、item 文脈を持たない:
  - `current_comic_composite_texture` — `comic_cache.get(&idx)` の直引き。裏も
    `HashMap<usize, ComicCacheEntry>` のまま (`ItemsGenerationMap` ではない)
  - `conceal_cache` — `HashMap<usize, ConcealCacheEntry>`。`ConcealCacheEntry` が持つのは
    global な conceal 世代だけで、読み側も `idx` + その世代しか見ない
- **今は `close_fullscreen` が各キャッシュをクリアするので救われているだけ**
  (src/app.rs 49757)。軽量な items 差し替え経路がそのクリアを通らないと ABA になる。
- **意図された正しい形が同じファイルにある**: `ContinuousPageTransition` は
  `items_generation` を自分で持ち、`keep_set_evict` でも照合している (src/app.rs 6434)。
- 直すときの注意 (Codex): 「current 系 helper 3 つに条件を足す」では**不十分**。
  exact-key ヒットが src/app.rs 55292 / 55640 にもあるため、
  **key / read / insert のライフサイクル全体**に item 文脈を通す必要がある。
- 規模 / 優先度: 中 / P2。リリース直前に触る範囲ではない。

### 1.87 セッション最初の拡大で UI スレッドが 0.5 秒止まる (拡大パイプラインの初回作成)

- 出典: v3.0.0 リリース前の perf smoke (2026-08-15)。
- 実測: `gpu/lanczos_regenerate` は同セッション 16 回のうち **1 回目だけ
  `encode_submit_cpu_ms=495.1`**。残り 15 回は 0.2〜7.8ms (中央値 0.5ms)。
  同フレームの `ui/fs_viewport_breakdown` は `central_ms=495.3` / `media_ms=495.2` で、
  495ms 全部がフルスクリーン中央の描画に乗っている。セッション中で 50ms を超えた
  UI スレッド作業はこの 1 件だけ。
- 読み: 初回だけという分布は wgpu の pipeline / shader を最初の 1 回で作っている形。
  利用者から見ると「その日はじめて画像を拡大表示した瞬間に 0.5 秒固まる」。
- **今回入った退行ではない**。既定の高品質拡大は v2.12.0 からで、warm-up の構造は
  そのとき入っている (§1.46)。v3.0.0 のブロッカーとしては扱わない。
- 直すなら: 起動後の暇なフレーム、または一覧描画の最初の 1 回で pipeline を先に作る。
  どのタイミングなら利用者の操作とぶつからないかは計測してから決める
  (起動直後に足すと今度は起動が遅くなる)。
- 規模 / 優先度: 小〜中 / P3。

### 1.94 音声モードの退出が期限切れで detach+attach+seek に落ち、音が途切れる

- 出典: 利用者報告 (2026-08-18)。メインウィンドウへ結合した状態で <kbd>F11</kbd> と <kbd>Z</kbd> を
  連打すると、切り替えのたびに**メインウィンドウのグリッドが一瞬見えるちらつき**と、
  **音の途切れ**が起きる。
- **同日 (2026-08-18) に入れた「別ウィンドウの動画再生中に外部アプリから戻ると Z だけ効かない」の
  修正が原因ではない**。その経路は `source=egui_key` で記録されるが、実ログ中に
  2 回しか無く (別ウィンドウでの成功例)、後述のタイムアウト 7 件のいずれとも時刻が重ならない。
  失敗時の enter はすべて既存の `source=native_key`。ちらつき自体はリリース版 v3.1.1
  (本修正を含まないプロセス) でも観測されている。
- **実測した機構** (1 セッションで 7 回発生、`mimageviewer.log`):

  ```
  180.776 exit: placement changed, re-placing via SwitchPlacement
  180.877 exit ignored reason=exit_pending  deadline_remaining_ms=1099
  181.409 exit ignored reason=exit_pending  deadline_remaining_ms=567
  181.986 exit timed out; falling back to detach+attach+seek
  181.986 fallback exit: re-attached presenter, seek=32.298
  ```

  placement 切替を伴う退出が **`VideoAudioExitPending.deadline` 内に完了せず**、
  detach → attach → **seek** のフォールバックへ落ちる。この seek が**音の途切れ**、presenter の
  付け直しが**ちらつき**。連打すると毎回この経路に入る。
- **構造上の問題**: 退出の完了判定に**時間窓 (`deadline`) を使っている**。憲法 §2 規則 5
  「races を時間窓で吸収しない」に当たる。本来は placement 切替の完了イベントで確定すべきところを
  経過時間で見切っているため、切替が遅い状況では必ずフォールバックする。
- **直す方向**: deadline を伸ばさない。`SwitchPlacement` の完了 (または presenter の再配置完了) を
  typed なイベントとして受け、それで退出を確定させる。フォールバックは「イベントが来ない」ことが
  確定した場合の最後の手段に落とす。着手前に、退出の producer / consumer と
  open / switch / close / cancel / error の各 lifecycle を列挙すること。
- detached / native video のリワーク凍結領域。症状パッチを入れず、原因に対応付けてから直す。
  着手時は [detached-rework-plan.md](detached-rework-plan.md) §2 を読み、
  ClaudeCode / Codex 双方で「症状パッチではない」ことの合意を取る。
- 再現手順: メインウィンドウのフルスクリーンで動画再生 → <kbd>F11</kbd> と <kbd>Z</kbd> を連打。
  判定はログの `timed out; falling back to detach+attach+seek` の有無で機械的にできる。
- 規模 / 優先度: 中 / P2 (音が途切れるので体感は悪いが、連打しない通常操作では出ない)。

### 5.10 入力テストの共有ロックが poison して、失敗 1 件が全滅に見える

- 出典: 動画補正スロットの入力 parity 確認 (2026-08-19) の mutation 確認中。可視性ガードを外して
  1 件だけ落とすつもりが、
  同じフィルタの 5 件すべてが FAILED になった。実際に落ちたのは 1 件で、残り 4 件は
  `fullscreen fixed-key test lock poisoned: PoisonError { .. }` だった。
- 機構: `crate::key_input::TEST_INPUT_LOCK` は入力テストを直列化するためだけの
  `Mutex<()>` だが、12 箇所すべてが `.expect(...)` / `.unwrap()` で取得している。1 件が
  panic するとロックが poison し、以降このロックを使う全テストが**本来の失敗とは無関係な
  理由で**落ちる。**最初の 1 件を読まないと原因が分からない状態**になり、CI ログでも
  mutation 確認でも切り分けの手間が増える。
- 直す方向: 取得を 1 つの helper に集約し、`unwrap_or_else(|e| e.into_inner())` で poison から
  回復する。この Mutex はデータを保護しておらず、panic で壊れる不変条件を持たないので
  回復が正しい。**ただし共有フレーム状態の後始末は別問題**なので、helper 化のときに
  「各 call site が取得直後に `clear_test_frame()` するか、guard の Drop で戻すか」を
  揃えて確認する (現在は `FullscreenFixedKeyTestGuard` だけが Drop で戻している)。
- 対象: `src/key_input.rs` (定義 + 6 箇所)、`src/app/tests.rs` (5 箇所)、`src/keymap.rs` (1 箇所)。
- 規模 / 優先度: Small / P3。製品には影響しないテスト基盤の可読性問題。

### 5.8 検索ベンチのゲートに絶対時間の下限が無い

- 出典: v3.0.0 リリース前確認 (2026-08-14)。`check_bench_regression.py` が 10 件中 7 件を
  劣化と報告したが、**製品側の劣化ではなかった**。
- 何が起きたか: 判定が `+30%` の**比率だけ**で、対象クエリの絶対時間を見ていない。
  実測はこう:

  | クエリ | baseline | 3 回の実測 | 実行間ばらつき |
  | --- | --- | --- | --- |
  | `rare_jp` | 1.03ms | 3.61 / 1.13 / 0.89 | **+304%** |
  | `super_generic` | 0.01ms | 0.02 / 0.01 / 0.01 | +184% |
  | `rare_jp_and` | 0.15ms | 0.31 / 0.42 / 0.23 | +79% |

  **同一バイナリの実行間ばらつきが閾値 (+30%) を大きく超える**。1 回目は 25 分の
  配布ビルド直後で、全クエリが一様に遅い (ディスクキャッシュと CPU の状態)。
  3 回の最良値なら 10 件中 9 件が合格し、3 件は baseline より速かった。
- 唯一残る `rare_jp_and` も 0.15ms → 0.23ms で、**差は 0.08 ミリ秒**。この規模に
  比率の閾値を当てても意味がない。
- ずれている前提: ゲートは「1 回の計測が真の性能」と仮定しているが、ミリ秒未満の
  クエリでは計測ノイズのほうが大きい。
- 直す方向 (組み合わせる):
  1. **絶対時間の下限**を入れる。`+30%` かつ `+N ms` (N=1〜2 程度) の両方を満たしたときだけ
     劣化とする。ミリ秒未満のクエリが騒がなくなる
  2. ベンチ側で **1 クエリを複数回まわして最良値**を JSON に書く (現在も batch 内では
     `best=` を出しているので、外側にも同じ考え方を広げる)
  3. baseline に**測定条件** (日付・機種・同時実行の有無) を残す。現在の
     `vendor/bench_baseline.json` は `num_docs` と `version` しか持たず、
     いつ何の上で取った値か分からない
- 当座の運用: 失敗したら**同じバイナリで 3 回まわして最良値で判断する**。
  ノイズと判断した場合に `--save` で baseline を上書きしないこと (ノイズを基準値にすると
  次から本物の劣化を見逃す)。
- 規模 / 優先度: 小 / P3 (リリースのたびに人が判断すれば回るが、毎回時間を取られる)。

## 6. 着手時に読み直す関連ドキュメント

| 領域 | ドキュメント |
| --- | --- |
| UI 同期 I/O / worker 化 | `docs/ui-responsiveness.md`, `docs/async-architecture.md` |
| サブフォルダ展開 / フラット仮想ビュー | `docs/subfolder-expansion-view-plan.md`, `docs/ui-responsiveness.md`, `docs/async-architecture.md`, `docs/details-view-and-filter-plan.md`, `docs/virtual-folders.md` |
| ZIP / PDF / 変換アーカイブ | `docs/virtual-folders.md`, `docs/shell-file-operations-context-menu-plan.md` |
| フォルダ移動 / Ctrl+↑↓ | `docs/fullscreen-navigation-consistency.md`, `docs/keymap-spec.md` |
| 入力カスタマイズ / マウス / ゲームパッド | `docs/keymap-spec.md`, `docs/key-customization-impl-plan.md`, `docs/ring-shortcut-plan.md`, `docs/operation-customize-share-plan.md` |
| フルスクリーン / F12 別ウィンドウ / 連結読み | `docs/display-pipeline.md`, `docs/detached-viewer-implementation-plan.md`, `docs/fullscreen-navigation-consistency.md` |
| 表示 / AI / 補正 | `docs/display-pipeline.md`, `docs/preset-and-adjustment.md` |
| 詳細表示 / スマートフィルタ | `docs/details-view-and-filter-plan.md`, `CLAUDE.md` の UI / スクロール節 |
| タグ / フルスクリーン右パネル / 動画 overlay | `docs/tag-catalog-redesign-plan.md`, `docs/display-pipeline.md`, `docs/video-architecture.md`, `docs/detached-viewer-implementation-plan.md` |
| リリース / 依存更新 | `CLAUDE.md` のリリース手順、各 native 依存管理節 |
