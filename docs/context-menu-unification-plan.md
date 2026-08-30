# 右クリックメニューの一本化 — 設計 (正本)

外部ツール起動 ([external-tool-launch-plan.md](external-tool-launch-plan.md)) の実機確認中に、
右クリックメニューが**二重管理されている**問題が表面化した。本書はその整理の正本。

## 1. 決まったこと (2026-08-30、利用者判断)

1. **右クリックは常に OS 見た目のネイティブメニューにする。** 見た目の違う mIV 独自メニューは
   利用者向けの表舞台から降ろす。
2. **ZIP / PDF 内ページなど仮想項目でも、同じ見た目のメニューを出す。** これらは実ファイル
   パスを持たないためシェル項目を混ぜられないので、**mIV 項目だけのネイティブメニュー**を出す。
3. **OS 由来の項目は一番下にサブメニューとしてまとめるのを既定にする。**
   設定で従来どおりの併記へ戻せるようにする。
4. **旧 XMP タグ関連の項目は削除する。**

5. **「最近使ったアプリ」(最大 3 件) は落とす。** 登録済み外部ツールが最上位に出る仕組みが
   できたので役割が重なる。旧メニューの利用者は少ないと見込まれ、需要が出たら再検討する。
   `recent_open_with_apps` はリリース済みデータなのでテーブルと行は残し、読み書きを止める。
6. **旧 XMP タグは利用者向けの明示操作を全廃する** (棚卸し §5.5 の (b))。右クリック 2 項目に加え、
   上部「タグ」メニューの同等項目も落とす。**自動 seed (旧データの救済) は止めない。**
   `xmp:Rating` 書込みなどの共有ヘルパーも残す。
7. **「削除 (ゴミ箱)」は mIV 版を残す。** Shell の削除は mIV 内部のタグ・評価・補正データを
   片付けないため。文言で違いが分かるようにする (下記)。

8. **実項目と仮想項目が混在した選択では、実項目だけに作用させない** (2026-08-30 利用者判断)。
   **トーストで理由を出し、実行しない。** 対象は削除・パスのコピーなど、仮想項目を扱えない操作。

   理由: 「仮想を除いた実ファイルだけに効く」は利用者から見て分からない。件数と実際の対象が
   合わないように見え、不具合と受け取られる。**黙って対象を減らすくらいなら実行しない方がよい。**
   これは CLAUDE.md の「silent fallback を根本原因の代わりに置かない」と同じ判断。

   混在が起きるのは通常フォルダではない。ZIP / PDF の中身は実ファイルと同居しないため、
   **場所を横断するビュー** (レーティング / タグ / 全体検索 / スマートフォルダ) だけで起きる。
   `ZipImage` / `PdfPage` はチェック可能で ([grid_item.rs:315](../src/grid_item.rs:315))、
   レーティング DB は仮想ページの種別を保存する ([rating_db.rs:22](../src/rating_db.rs:22))。

9. **設定「実ファイル/実フォルダの右クリックに Windows のメニューを含める」を廃止する**
   (2026-08-30 利用者判断)。この設定の存在理由は構築が遅かったことで、`QueryContextMenu` を
   サブメニューを開くまで遅らせた今、切る理由が無い。**常に含める。**
   リリース済み設定なのでフィールドは残し (無視する)、UI と検索索引 entry を落とす。
   従来 OFF にしていた利用者には Windows 項目が戻るが、サブメニュー 1 項目に収まるので
   割り込み度は低い。
10. **「アプリケーションで開く…」内の「アプリケーションを追加…」を落とす**
    (2026-08-30 利用者判断)。「外部ツールの設定…」から環境設定へ入れるので、
    **似た入口が 2 つある方が分かりにくい**。登録の頻度は低く、素早さより分かりやすさを取る。
11. **トーストの表示時間を文字数に応じて伸ばす** (2026-08-30 利用者判断)。
    既定 1.2 秒は短い案内文に合わせた値で、混在選択の拒否のような長い文では読み切れない。
    呼び出し側が毎回指定するのではなく、`show_feedback_toast` が本文の長さから決める。

## 1.1 表記の方針 — ブランド接頭辞は付けない

「mIV:」や「(mIV)」を全項目へ付ける案は採らない。理由:

- **OS 項目をサブメニューへ畳むのが既定**なので、最上位はすべて mIV の項目になる。
  所有者は文脈で自明であり、全項目に同じ 4 文字を足すのは情報量のない反復になる。
- 重複が起きるのは併記モードのときの **2 項目だけ** (削除 / 新しいフォルダ)。
  15 項目へ印を付けて 2 項目の曖昧さを解くのは釣り合わない。
- **Windows 自身もシェル拡張もそうしていない。** 利用者環境の Shell 項目は
  「EmEditorで編集」「7-Zip」「WinRAR」のように、**アプリ名か動作名で自分を名乗る**。
  項目の頭に提供元を並べる流儀ではない。
- 接頭辞は動詞を後ろへ押しやり、目で走査しにくくする。付けるなら接尾辞。

代わりに、**重複する項目だけ「何が違うか」を文言で言う**。ブランドではなく動作を書く。

| 現在 | 変更後 |
| --- | --- |
| 削除 (ゴミ箱) | ゴミ箱へ移動 (タグ・評価も整理) |
| 新しいフォルダ... | (Shell 側は「新規作成 ▸ フォルダー」で階層が違うため、変更不要) |

## 1.2 統一の観点 (今回の棚卸しの目的)

利用者方針 (2026-08-30): **メニューを 1 本化するだけでなく、この機会に操作感を揃える。**
統合時に次を揃えること。

- **文言**: 三点リーダーは `...` か `…` のどちらかへ統一する (現在は混在)。
  「〜で開く」のような動詞の付け方も揃える。
- **提供範囲**: 同じ操作が grid と fullscreen、実項目と仮想項目で不揃いに出ている箇所を、
  意図して揃えるか、揃えない理由を書く。棚卸し表の「出る条件」列が対象一覧になる。
- **別入口との一致**: キー割り当てや Ring と同じ操作は、同じ文言・同じ可否にする。

## 2. なぜこうするか

### 2.1 二重管理が腐る

mIV には右クリックメニューの実装が 2 つある。ネイティブ (`native_context_menu.rs` +
`ui_dialogs/context_menu.rs` の `miv_items` 構築) と、egui 独自メニュー
(`ui_dialogs/context_menu.rs` の描画関数群)。**同じメニューを 2 か所で手で維持している**ため、
片方だけ更新が止まる。実際に独自メニュー側へ旧 XMP タグの項目が残り、外部ツールも
古い入れ子構造のままだった。

したがって**片方を消すだけでは不十分**で、`1 つの項目定義から両方を描く`構造にする。
独自メニューは廃止せず、**ネイティブメニューを構築できなかったときの描画方法**へ格下げする。
同じ定義から描く限り内容はずれない。

### 2.2 サブメニューを既定にする理由

利用者環境の実測: 右クリック 1 回で **約 54 項目** (mIV 10 + OS 44) が並び、
シェルメニューの構築に **1.2〜1.4 秒**かかっている
(`native_context_menu: slow show_query_shell 1436.4ms`)。

- サブメニューにすれば、開くまでシェルへの問い合わせを遅らせられる。通常の右クリックが速くなる。
- 画面に収まらない場合、Win32 メニューはスクロール矢印付きのスクロールになる。列折り返しはしない。
  1080p ではこの項目数は確実にはみ出す。
- **Windows 11 自身が同じ隠し方をしている** (「その他のオプションを表示」)。独自の工夫ではない。
  mIV が取得できるのは `IContextMenu` 経由の従来メニューだけで、新しい方は外部から取れない。
  つまり mIV は「その他のオプションを表示」を最初から開いた状態を見せていることになる。

既定をサブメニューにするのは、**シェル拡張が多い環境ほど困り、困っている人ほど設定を触らない**
ため。少ない環境では既定のままでも実害が無い。

## 3. 独自メニューにしか出せないものは無い (調査済み)

- 「左に回転 / 右に回転」は `ui.horizontal` で横並びだが、ネイティブ側は既に 2 行で出しており差は無い
- 「アプリケーションで開く…」の折りたたみは Win32 のサブメニューで表現できる
- 無効表示は `MF_GRAYED` で表現できる
- 進捗バーは削除中モーダルのもので、コンテキストメニューではない

## 4. 作業順序

1. **棚卸し** (§5) — 両メニューの項目を突き合わせ、現役か廃止かを決める
2. 現役項目をネイティブ側の項目定義へ移す。旧 XMP など不要なものはここで落とす
3. 1 つの項目定義から両方を描く構造にする
4. 仮想項目でも mIV 項目だけのネイティブメニューを出す
5. シェル項目のサブメニュー化 + 設定

実装状況 (2026-08-30): **Phase A で 2〜3、Phase B で 4〜5 を実装済み**。Phase B では
混在選択を実行時に拒否する §1 の 8 も適用した。仮想項目は mIV 項目だけの native HMENU、
実項目の Shell 項目は既定で末尾の「Windows のメニュー」サブメニューに入る。サブメニューは
`WM_INITMENUPOPUP` まで `IContextMenu::QueryContextMenu` を遅延し、設定
`show_windows_context_menu_inline` を ON にすると従来の同階層表示へ戻る。

Phase B 着手時に確認した件数表示の食い違いは、Phase A の共通定義が同じ
`real_checked_count` に対して、パスコピーは全選択が実項目なら件数を省略し、削除だけ常に件数を
表示していたことが原因だった。削除確認と削除 worker の対象はどちらも仮想項目除外後の
`delete_targets` であり、削除経路だけ PDF ページを数える別経路は無かった。Phase B では両ラベルを
全選択数へ揃え、仮想項目を含むパスコピー / 削除はトーストで拒否して部分実行しない。

## 4.1 次にやること — 機能 × 対象種別の表 (2026-08-30 合意、未着手)

右クリックメニューの棚卸し (§5) で、**同じ操作が対象の種別ごとにバラバラに扱われている**
ことが分かった。特に次の 3 通りが混在している。

| 機能 | ZIP / PDF ページに対して | 根拠 |
| --- | --- | --- |
| レーティング | **ページ単位で付く** | `RatingItemKind::ZipImage` = 6 / `PdfPage` = 7 ([rating_db.rs:22](../src/rating_db.rs:22)) |
| タグ | **コンテナに付く** (フルスクリーンのみ。グリッドでは何も起きない) | [tag_ops.rs:29](../src/tag_ops.rs:29) の `zip_path` / `pdf_path` へのフォールバック |
| スマートフォルダ | 扱わない | — |

**「ページに★を付けられるのが良くない判断だったのでは」という問いには、そうではないと考える。**
★のページ単位は用途が明確 (漫画や写真の ZIP で良いページに印を付ける) で、コンテナ単位では
代用できない。v2.1.0 で出荷済みでもある。**悪いのはページに付けられることではなく、
ページを実ファイルと同じ一覧に並べて実ファイル用の操作を出していること。**
むしろタグの「ページに付けたつもりがコンテナに付く」黙った付け替えの方が分かりにくい。

### やること

**機能 × 対象種別の表を作り、それを契約にする。** 各セルは
**対応 / 非対応 (拒否) / コンテナへ寄せる** の 3 択で、**「黙って何もしない」を無くす**。
現状は空白 (何も起きない) と「コンテナに寄る」が混ざっていて、どちらも利用者から見えない。

対象機能: レーティング / タグ / 削除 / コピー・移動 / 外部ツール起動 /
スマートフォルダの対象 / 検索の対象。
対象種別: 実ファイル / ZIP 内ページ / PDF ページ / Stack / フォルダ。

**「保存先」の列を足す。** ただし前提を 1 つ訂正する。**タグはファイルに書かれない。**
正本は `tags.db` で、通常のタグ操作はメディア本体も XMP も書き換えない
([tag_write_worker.rs](../src/tag_write_worker.rs))。`dc:subject` へ書いていたのは v1.0 の
旧仕様で、今は旧 XMP タグの取り込み経路ごと本ブランチで削除済み
(`xmp_writer::apply_tag_op` は呼び出し元が無い)。**ファイルを書き換える経路は
★ の `xmp:Rating` だけ**で、それも `write_rating_to_xmp` が ON
(**既定 OFF**、[settings.rs:6077](../src/settings.rs:6077)) かつ実ファイルの JPEG / PNG / WebP
かつ製本ページでないときに限られる ([app.rs:47673](../src/app.rs:47673) `rating_xmp_target_for_idx`)。

したがって **「実ファイルは持ち出せる / ページは mIV に閉じる」という対比は、既定では成り立たない。**
既定ではどちらも mIV の DB にしか残らない。実際の差は次の 2 点に縮む。

- タグのサイドカーミラー (`mimageviewer.dat`) は**実ファイルにしか作れない**
  ([tag_write_worker.rs:21](../src/tag_write_worker.rs:21) `sidecar_target_for_real_file`)。
  これも `tag_sidecar_backup_enabled` が既定 OFF
- ★ の XMP 書き出しを ON にしたとき、**書ける対象が実ファイルの画像だけ**

表の「保存先」列は、この訂正後の事実を書く。**「他のソフトから見える」と読める書き方をしない。**
利用者に見せるべきは「mIV のデータとして保存される (既定ではファイルは変更されない)」であって、
可搬性の約束ではない。

### 進め方

1. 棚卸し (§5) と同じやり方で、**現状を機械的に洗い出す**
2. 表を見ながら、どこを揃えるかを決める
3. **タグのページ対応**を実装する。★ は既にメタデータパネルでページとコンテナを 2 行に
   分けて出している ([ui_metadata_panel.rs:329](../src/ui_metadata_panel.rs:329))。タグも同じ形にすれば、
   今の黙った付け替えが無くなる。
   **ただし ★ 一覧と同じ「混在した横断リスト」がタグビューにも増える**ので、
   §1 の 8 (混在選択の拒否) が入った後にやること
4. マニュアルにも「ZIP・PDF のページでできること / できないこと」として保存先付きで載せる

## 5. 棚卸し

### 5.1 調査範囲と読み方

ここでいう A は `native_grid_context_menu_items` が作る **mIV 項目だけ**であり、後から
`IContextMenu::QueryContextMenu` が足す Shell 項目は含めない。B は grid / fullscreen の
egui フォールバックメニューである。ブックマーク一覧専用メニューと、グリッド空白以外の
ツールバー等の右クリックメニューは対象外とした。

以下はコード上の事実である。

- `use_native_shell_context_menu` は既定 `true`。実ファイル / 実フォルダでは A を先に試し、
  成功・キャンセル・Shell コマンド実行のいずれでも B へ進まない。B へ進むのは設定 OFF、
  HWND 不在、native 構築失敗、または native target を作れない場合である
  (`src/settings.rs:3950`, `src/settings.rs:5983`,
  `src/ui_dialogs/context_menu.rs:1253`, `src/ui_dialogs/context_menu.rs:1344`)。
- A の単一 target は `GridItem::drag_source_path()` が必須である。実 target を作れるのは
  `Folder` / `Image` / `Video` / `ZipFile` / `PdfFile` / `ConvertibleArchive`
  （および本表外の `Audio`）だけで、`ZipImage` / `PdfPage` / `Stack` は作れない。
  複数選択も 1 件でも仮想項目を含むと A 全体を諦める
  (`src/ui_dialogs/context_menu.rs:1377`, `src/grid_item.rs:270`,
  `src/grid_item.rs:288`)。
- 閲覧履歴 grid は実 target でも A を意図的に呼ばず B を描く。fullscreen はこの bypass を
  持たない (`src/ui_dialogs/context_menu.rs:721`,
  `src/ui_dialogs/context_menu.rs:1914`)。
- 表の「検索」は Ctrl+S / Ctrl+G / お気に入り検索、「タグ」はタグ結果とその drill を指す。
  `フォルダに移動` の `in_search` には検索とタグが入り、レーティング / 閲覧履歴は入らない
  (`src/ui_dialogs/context_menu.rs:690`)。
- `has_checked` は grid だけにある。A は全チェック項目が実ファイルの場合だけ構築できる。
  B は仮想項目を含む場合にも出るが、削除対象収集は `drag_source_path()` のある項目だけを
  残す (`src/ui_dialogs/context_menu.rs:684`,
  `src/ui_dialogs/context_menu.rs:1389`, `src/ui_dialogs/context_menu.rs:2311`)。

表中の「除外なし」は、検索 / タグ / レーティングの view flag による明示的除外がない、
という意味である。ただし前記のとおり閲覧履歴 grid は A 全体を bypass する。また A/B の
「ある」は、表の条件のどこかで実際に表示可能という意味にした。A の項目生成関数内に汎用分岐が
あっても、仮想 target を作れず実行時に到達しない場合は条件欄へ明記した。

### 5.2 項目表

| 項目 (表示文言) | A にある | B にある | 出る条件 (GridItem 種別 / surface / checked / view flags) | file:line | 判定案 |
| --- | --- | --- | --- | --- | --- |
| 新しいフォルダ... / 新しいフォルダ… | ある | ある | `Folder` の grid 背景、`has_checked=false`。現在の実フォルダを解決できる時だけ。検索 / タグ結果では除外。レーティングは明示除外なし。閲覧履歴 grid は A bypass、B も target が無ければ disabled。 | `src/ui_dialogs/context_menu.rs:699`, `src/ui_dialogs/context_menu.rs:893`, `src/ui_dialogs/context_menu.rs:1454` | **既にある** — 表記の三点リーダーを揃えて共通定義にする。 |
| 貼り付け | ある | ない | `Folder` の grid 背景、`has_checked=false`、現在のお気に入り実フォルダと target が一致する時。検索 / タグ結果では除外。Ctrl+V という別入口あり。 | `src/ui_dialogs/context_menu.rs:1454`, `src/ui_dialogs/context_menu.rs:1687`, `docs/keymap-spec.md:320` | **要判断** — A の現役項目を共通定義へ残せば B にも増える。フォールバック時にも出すのが自然だが、B 追加扱いを確認する。 |
| 名前の変更... | ある | ない | 単一の実 `Folder` / `Image` / `Video` / `ZipFile` / `PdfFile` / `ConvertibleArchive`、grid / fullscreen、`has_checked=false`。view 除外なし。仮想 3 種には独立実体がないため対象外。grid には既定キーなしの `GridRename` もある。 | `src/ui_dialogs/context_menu.rs:1475`, `src/ui_dialogs/context_menu.rs:1707`, `docs/keymap-spec.md:284` | **要判断** — A の現役項目として保持する。fullscreen にも現状出るため、共通化時に surface を変えないか確認が必要。 |
| パスをコピー / このフォルダのパスをコピー | ある | ある | A: 単一の実 6 種、grid / fullscreen、または grid 背景。B: それらに加え `ZipImage` / `PdfPage`、grid の `Stack`（別文言は別行）。`has_checked=false`。view 除外なし、閲覧履歴 grid は B。 | `src/ui_dialogs/context_menu.rs:812`, `src/ui_dialogs/context_menu.rs:861`, `src/ui_dialogs/context_menu.rs:913`, `src/ui_dialogs/context_menu.rs:967`, `src/ui_dialogs/context_menu.rs:1023`, `src/ui_dialogs/context_menu.rs:1484`, `src/ui_dialogs/context_menu.rs:1948` | **既にある** — 仮想表示文字列の resolver を共通定義へ含める。 |
| 選択項目のパスをコピー | ある | ない（代わりに disabled の「パスをコピー」） | grid、`has_checked=true`。A はチェックがすべて実 `Image` / `Video` / `ZipFile` / `PdfFile` / `ConvertibleArchive` の時だけ。仮想を 1 件でも含むと A は構築不能になり、B の disabled 行しか出ない。view 除外なし。 | `src/ui_dialogs/context_menu.rs:755`, `src/ui_dialogs/context_menu.rs:764`, `src/ui_dialogs/context_menu.rs:1423`, `src/ui_dialogs/context_menu.rs:1716` | **移す** — A の実動作を正本にし、B の disabled 専用行は落とす。仮想複数の合成パスをコピーするかは別途要判断。 |
| ファイル名をコピー | ある | ある | A: 単一実 6 種（`Folder` も含む）、grid / fullscreen、`has_checked=false`。B: `Image` / `Video` / `ZipFile` / `PdfFile` / `ConvertibleArchive` と `ZipImage`、grid / fullscreen（`Folder` には無し）。view 除外なし。 | `src/ui_dialogs/context_menu.rs:818`, `src/ui_dialogs/context_menu.rs:918`, `src/ui_dialogs/context_menu.rs:977`, `src/ui_dialogs/context_menu.rs:1044`, `src/ui_dialogs/context_menu.rs:1495`, `src/ui_dialogs/context_menu.rs:1954` | **既にある** — `Folder` と仮想 entry の差を共通 target resolver で明示する。 |
| ページ名をコピー | ない | ある | `PdfPage`、grid / fullscreen、`has_checked=false`。view 除外なし。A は仮想 target を作れない。 | `src/ui_dialogs/context_menu.rs:1023`, `src/ui_dialogs/context_menu.rs:2006` | **移す** — PDF 仮想ページの現役コピー機能。仮想用 native menu に必要。 |
| 代表画像のパスをコピー | ない | ある | `Stack`、grid のみ、`has_checked=false`。view 除外なし。A は仮想 target を作れない。 | `src/ui_dialogs/context_menu.rs:1013` | **移す** — Stack の現役最小操作。仮想用 native menu に必要。 |
| 画像をクリップボードにコピー | ある | ある | A: `Image`、grid / fullscreen、`has_checked=false`。B: `Image` と `ZipImage`、grid / fullscreen、`has_checked=false`。`PdfPage` / `Stack` には無し。view 除外なし。画像 fullscreen では RingAction の別入口もある。 | `src/ui_dialogs/context_menu.rs:827`, `src/ui_dialogs/context_menu.rs:981`, `src/ui_dialogs/context_menu.rs:1510`, `src/ui_dialogs/context_menu.rs:1963`, `src/ui_dialogs/context_menu.rs:1998`, `src/ring_shortcut.rs:1299` | **既にある** — `ZipImage` の展開コピーも共通 action に含める。 |
| 編集内容をコピー | ある | ある | A: 実 `Image` の grid / fullscreen、`has_checked=false`。B: `Image` / `ZipImage` / `PdfPage` の grid / fullscreen、`has_checked=false`。view 除外なし。A の `ZipImage` / `PdfPage` 分岐は target 構築前に落ちるため現状到達不能。別 KeyAction / RingAction は見つからず、右クリックが唯一の起動入口。 | `src/ui_dialogs/context_menu.rs:1094`, `src/ui_dialogs/context_menu.rs:1518`, `src/ui_dialogs/context_menu.rs:1747`, `src/ui_dialogs/context_menu.rs:2044` | **既にある** — 仮想 target を作れるようにすれば A 側の既存 command をそのまま使える。 |
| 編集内容を貼り付け | ある | ある | 対象種別 / surface は「編集内容をコピー」と同じ。A は clipboard がある時だけ項目を追加、B は常時表示して clipboard 無しなら disabled。別 KeyAction / RingAction は見つからず、右クリックが唯一の起動入口。 | `src/ui_dialogs/context_menu.rs:1103`, `src/ui_dialogs/context_menu.rs:1528`, `src/ui_dialogs/context_menu.rs:1753`, `src/ui_dialogs/context_menu.rs:2053` | **既にある** — 「無い時は隠す / disabled」の表現差だけを決める。推奨は発見性を保つ disabled。 |
| ページを開く | ある | ある | `ZipFile` / `PdfFile` / `ConvertibleArchive`、grid、`has_checked=false`。検索 / タグ / レーティング除外なし。閲覧履歴 grid は B。既定キーなし `GridOpenSelectedAsPage` という別入口あり。 | `src/ui_dialogs/context_menu.rs:927`, `src/ui_dialogs/context_menu.rs:1053`, `src/ui_dialogs/context_menu.rs:1537`, `docs/keymap-spec.md:286` | **既にある** — 共通 container action に統合する。 |
| 一覧を開く | ある | ある | `ZipFile` / `PdfFile` / `ConvertibleArchive`、grid、`has_checked=false`。view 条件は「ページを開く」と同じ。既定キーなし `GridOpenSelectedAsList` という別入口あり。 | `src/ui_dialogs/context_menu.rs:935`, `src/ui_dialogs/context_menu.rs:1061`, `src/ui_dialogs/context_menu.rs:1550`, `docs/keymap-spec.md:286` | **既にある** — 共通 container action に統合する。 |
| フォルダに移動 | ある | ある | 単一の実 6 種、grid、`has_checked=false`、検索 / タグ view（結果または drill）だけ。レーティング / 閲覧履歴では出ない。B は本表外の `SearchContainer` にも出る。仮想 `ZipImage` / `PdfPage` / `Stack` には出ない。 | `src/ui_dialogs/context_menu.rs:690`, `src/ui_dialogs/context_menu.rs:835`, `src/ui_dialogs/context_menu.rs:873`, `src/ui_dialogs/context_menu.rs:942`, `src/ui_dialogs/context_menu.rs:1007`, `src/ui_dialogs/context_menu.rs:1557` | **既にある** — `SearchContainer` を含む target 解決だけ共通化する。 |
| この本のフォルダに移動 | ない | ある | `Folder` / `ZipFile` / `PdfFile` / `ConvertibleArchive`、閲覧履歴 grid、`has_checked=false`。A は閲覧履歴 grid で呼ばれない。 | `src/ui_dialogs/context_menu.rs:880`, `src/ui_dialogs/context_menu.rs:947`, `src/ui_dialogs/context_menu.rs:1073` | **移す** — 閲覧履歴の現役導線。履歴用共通定義へ残す。 |
| 左に回転 (L) | ある | ある | 単一: A は `Image` の grid / fullscreen と `Video` の grid、B は `Image` / `Video` の grid（本表外の `Audio` も grid）。`ZipImage` / `PdfPage` の単一右クリックには無し。複数: grid `has_checked=true` で両方にあるが、仮想を含むと A は構築不能で B だけ。view 除外なし。L の `GridRotateCcw` / `FsRotateCcw` が別入口。 | `src/ui_dialogs/context_menu.rs:767`, `src/ui_dialogs/context_menu.rs:844`, `src/ui_dialogs/context_menu.rs:1430`, `src/ui_dialogs/context_menu.rs:1565`, `src/keymap.rs:5369`, `src/keymap.rs:5447` | **既にある** — surface と target 差を共通 predicate にする。仮想単一にもメニューを出すかは要判断。 |
| 右に回転 (R) | ある | ある | 「左に回転」と同じ。R の `GridRotateCw` / `FsRotateCw` が別入口。 | `src/ui_dialogs/context_menu.rs:775`, `src/ui_dialogs/context_menu.rs:849`, `src/ui_dialogs/context_menu.rs:1436`, `src/ui_dialogs/context_menu.rs:1574`, `src/keymap.rs:5370`, `src/keymap.rs:5446` | **既にある** — 左回転と同じ共通 predicate にする。 |
| 📌 代表サムネに固定 / 📌 代表サムネ固定を解除 | ある | ある | grid / fullscreen、`has_checked=false`。A は実 `Folder` / `Image` / `Video` / `ZipFile` / `PdfFile` / `ConvertibleArchive`、B はそれらに加え `ZipImage` / `PdfPage`（`Stack` は不可）。A は検索 / タグ / レーティング / 閲覧履歴を明示除外。B は検索 / タグを明示除外し、grid 呼出側が閲覧履歴を除外、両者とも synthetic container と一部 archive override を除外。P の `GridPin` / fullscreen pin が別入口。 | `src/ui_dialogs/context_menu.rs:1115`, `src/ui_dialogs/context_menu.rs:1588`, `src/ui_dialogs/context_menu.rs:1636`, `src/ui_dialogs/context_menu.rs:2073`, `src/app.rs:26121`, `docs/keymap-spec.md:307` | **既にある** — B の仮想 pin が失われないよう target と view predicate を一本化する。 |
| 📌 代表サムネ固定: 変換後に設定可能 / 📌 代表サムネに固定 (disabled) | ある | ある | 未変換 `ConvertibleArchive`、grid / fullscreen、`has_checked=false`。pin の view 除外を通った場合だけ disabled 表示。文言と tooltip は A/B で異なる。 | `src/ui_dialogs/context_menu.rs:1655`, `src/app.rs:26146` | **既にある** — 文言と disabled reason を共通定義に持たせる。 |
| 📌 現在のフレームを動画サムネに設定 | ある | ある | `Video`、fullscreen、`has_checked=false`。view 除外なし。grid の動画では代表サムネ pin / 回転になり、この項目は出ない。 | `src/ui_dialogs/context_menu.rs:1581`, `src/ui_dialogs/context_menu.rs:2065` | **既にある** — fullscreen-video 専用 action として統合する。 |
| このフォルダをエクスプローラで開く | ある | ある | 単一: A は実 6 種、B は実 6 種に加え `ZipImage` / `PdfPage` / `Stack`、grid / fullscreen。複数: grid で現在の単一実フォルダを解決できる場合だけ（検索 / タグ結果では通常出ない）。閲覧履歴 grid は B。 | `src/ui_dialogs/context_menu.rs:711`, `src/ui_dialogs/context_menu.rs:1135`, `src/ui_dialogs/context_menu.rs:1442`, `src/ui_dialogs/context_menu.rs:1596`, `src/ui_dialogs/context_menu.rs:2081`, `src/ui_dialogs/context_menu.rs:2964` | **既にある** — 仮想 item では元コンテナの親を返す resolver を共通化する。 |
| `<登録ツール名>で開く`（登録済み外部ツール） | ある | ある | 実 `Image` / `Video` / `ZipFile` / `PdfFile` / `ConvertibleArchive`、grid / fullscreen、単一と複数（複数時も現状は右クリック対象 1 件）。B は `ZipImage` / `PdfPage` / `Stack` で編集用ツールだけ disabled 表示、A は仮想 target を作れず到達不能。`show_in_context_menu=true` の登録だけ。view 除外なし。 | `src/ui_dialogs/context_menu.rs:1450`, `src/ui_dialogs/context_menu.rs:1608`, `src/ui_dialogs/context_menu.rs:2187`, `src/external_tool.rs:137`, `src/external_tool.rs:351` | **既にある** — 現在の共通 `external_tool_menu_items` をそのまま項目定義の子にする。 |
| `<最近使ったアプリ>で開く`（最大 3 件） | ない | ある | 実 `Image` / `Video` / `ZipFile` / `PdfFile` / `ConvertibleArchive`、grid / fullscreen。B の open-with block を通る単一または複数。view 明示除外なし。仮想 3 種は `real_file()` が失敗するため出ない。既定 ON の通常実ファイルでは B へ届かず、事実上到達不能。 | `src/ui_dialogs/context_menu.rs:2167`, `src/settings.rs:4368`, `src/settings.rs:8231` | **要判断** — 現役履歴に見える一方、設定コメントは「次リリースまでの legacy 候補、削除予定」。残すなら下記 open-with submenu の先頭へ移す。 |
| アプリケーションで開く…（折りたたみ親） | ない | ある | B の open-with block を呼ぶ `Image` / `Video` / `ZipFile` / `PdfFile` / `ZipImage` / `PdfPage` / `Stack` / `ConvertibleArchive`、grid / fullscreen（`Stack` は grid のみ）、単一または複数。view 明示除外なし。実ファイルでは既定 ON により B の親自体が事実上到達不能。 | `src/ui_dialogs/context_menu.rs:792`, `src/ui_dialogs/context_menu.rs:840`, `src/ui_dialogs/context_menu.rs:958`, `src/ui_dialogs/context_menu.rs:2148`, `src/ui_dialogs/context_menu.rs:2186` | **移す** — Win32 submenu として共通項目ツリーに追加する。 |
| `<関連付けアプリ名>`（システム関連付けアプリ） | ない | ある | 実 `Image` / `Video` / `ZipFile` / `PdfFile` / `ConvertibleArchive`、grid / fullscreen、B の open-with block 内。view 明示除外なし。仮想では出ない。既定 ON の通常実ファイルでは事実上到達不能。Shell 側にも環境依存の open-with 項目があり得るが、mIV の履歴更新はこの B 経路だけ。 | `src/ui_dialogs/context_menu.rs:2226`, `src/open_with.rs:1` | **移す** — mIV の recent 履歴と同じ command 経路を維持するなら A に必要。Shell 項目だけに任せる仕様へ変えるなら利用者判断が必要。 |
| アプリケーションを追加… | ない | ある | B の open-with block が出る全 target、grid / fullscreen、単一または複数、view 明示除外なし。仮想でも親の中には表示される。実ファイルでは既定 ON により事実上到達不能。環境設定「外部ツール」の「実行ファイルを選んで追加 / 関連付けアプリから追加」という別入口あり。 | `src/ui_dialogs/context_menu.rs:2255`, `src/ui_dialogs/preferences/pages.rs:481`, `src/ui_dialogs/preferences/pages.rs:518` | **移す** — quick-add として残し、open-with submenu の管理項目へ置く。環境設定だけで十分とするなら落とす余地はある。 |
| 外部ツールの設定… | ない | ある | B の open-with block が出る全 target、grid / fullscreen、単一または複数、view 明示除外なし。仮想でも表示。実ファイルでは既定 ON により事実上到達不能。環境設定の「外部ツール」ページという別入口あり。 | `src/ui_dialogs/context_menu.rs:2285`, `src/ui_dialogs/preferences/pages.rs:481` | **移す** — open-with submenu 末尾の管理項目として残す。 |
| 削除 (ゴミ箱) | ない（Shell 側には通常削除がある） | ある | 単一の実 `Folder` / `Image` / `Video` / `ZipFile` / `PdfFile` / `ConvertibleArchive`、grid、`has_checked=false`。fullscreen B には無し。view 明示除外なし、閲覧履歴 grid でも物理削除行が履歴削除行とは別に出る。Delete の `GridDelete` が別入口。 | `src/ui_dialogs/context_menu.rs:795`, `src/ui_dialogs/context_menu.rs:856`, `src/ui_dialogs/context_menu.rs:907`, `src/ui_dialogs/context_menu.rs:962`, `src/ui_dialogs/context_menu.rs:1087`, `docs/keymap-spec.md:321` | **要判断** — A に移すと Shell の削除と重複する。mIV の確認・metadata purge 経路を優先するか、Shell 項目へ任せるかを利用者に聞く。 |
| 削除 (ゴミ箱) `[N件]` | ない（Shell 側には通常削除がある） | ある | grid、`has_checked=true`。チェック可能な実 5 種と仮想 `ZipImage` / `PdfPage`。仮想混在で A が fallback した場合も表示件数は全 checked 数だが、実際の削除対象は実パスのある項目だけ。view 明示除外なし。Delete の `GridDelete` が別入口。 | `src/ui_dialogs/context_menu.rs:755`, `src/ui_dialogs/context_menu.rs:795`, `src/ui_dialogs/context_menu.rs:2311`, `src/ui_dialogs/context_menu.rs:2477` | **要判断** — 単一削除と同じ重複判断が必要。仮想混在時の件数と対象の不一致は統合時に放置しない。 |
| 選択解除 (Ctrl+D) | ない | ある | grid、`has_checked=true`、全 item 種別。view 明示除外なし。全 checked が実項目なら既定 ON で A が先に消費するため事実上到達不能。仮想を含み A が fallback した時は到達可能。Ctrl+D / Ctrl+Shift+A の `GridDeselect` が別入口。 | `src/ui_dialogs/context_menu.rs:805`, `src/app.rs:67412`, `src/keymap.rs:5346` | **移す** — 選択状態を見て解除できる発見可能な導線として A へ移す。 |
| 旧XMPタグを取り込む `(N)` | ない | ある | `Image` / `Video`、grid / fullscreen、単一または複数。書込対応画像または動画 sidecar 対象だけ。view 明示除外なし。実 target 必須なので既定 ON では事実上到達不能。ただし上部「タグ」メニューに同じ別入口がある。 | `src/ui_dialogs/context_menu.rs:393`, `src/ui_dialogs/context_menu.rs:783`, `src/ui_dialogs/context_menu.rs:2107`, `src/ui_main.rs:5578` | **落とす** — 利用者合意済み。削除境界は §5.5 のとおり。 |
| 旧XMPタグを取り込んでファイルから削除 `(N)` | ない | ある | 「旧XMPタグを取り込む」と同じ。取り込み DB 反映成功後に旧 `#` 要素だけを消し、空の動画 sidecar だけを削除する。上部「タグ」メニューに同じ別入口がある。 | `src/ui_dialogs/context_menu.rs:2135`, `src/ui_main.rs:5596`, `src/tag_legacy_xmp_worker.rs:149` | **落とす** — 利用者合意済み。破壊的 worker の巻き添え範囲は §5.5 で分離する。 |
| 履歴から削除 | ない | ある | 閲覧履歴 grid、`has_checked=false`、folder 背景以外。`Folder` / `ZipFile` / `PdfFile` / `ConvertibleArchive` を含む履歴 item（コード上は全 variant の match 後に追加）。A は閲覧履歴 grid で呼ばれない。 | `src/ui_dialogs/context_menu.rs:1126` | **移す** — 閲覧履歴の現役管理操作。履歴用共通定義へ残す。 |

補足: B は本表外の `Audio` / `ZipDir` / `SearchContainer` も扱う。統合作業では exhaustive な
`GridItem` match を維持し、本表の 9 種だけを列挙した結果としてそれらを消さないこと。

### 5.3 B にしかない項目の到達性

事実として、既定 ON・通常の HWND・native 構築成功という通常経路では、実ファイルの A が
B より先にメニューを消費する。このため次は **実ファイルでは事実上到達不能**である。

- 最近使ったアプリ（最大 3 件）
- mIV が列挙するシステム関連付けアプリ
- 「アプリケーションを追加…」と「外部ツールの設定…」
- 旧 XMP 2 項目
- 全項目が実ファイルである複数選択時の「選択解除」
- mIV 独自の「削除 (ゴミ箱)」（ただし native menu の Shell 側削除と Delete キーは別入口）

一方、B-only でも現在到達可能なものを混同しない。

- 閲覧履歴 grid は A を bypass するので「この本のフォルダに移動」「履歴から削除」は現役。
- `ZipImage` / `PdfPage` / `Stack` は A target を作れないため、ページ名 / 代表画像パスのコピー、
  仮想ページの編集内容コピー・貼り付け、代表サムネ pin 等は B で現役。
- 「アプリケーションを追加…」「外部ツールの設定…」は仮想 item の B にも出るため完全な
  dead code ではない。ただし最近使ったアプリと関連付けアプリは実ファイルを要求するため、
  仮想 item からは出ず、通常設定では本当に到達不能である。
- 旧 XMP 2 操作には上部「タグ」メニューという別入口がある。右クリック項目は到達不能でも、
  worker 自体は到達可能である。

「編集内容を貼り付け」は B-only ではない。A にも `PasteEditBundle` が実装済みで、実 `Image` では
現在使える。ただし `ZipImage` / `PdfPage` は A target を作れないため B に落ちる。コード全体を
`KeyAction` / `RingAction` / 呼出メソッド名で検索した範囲では別ショートカット入口は無く、
右クリックが唯一の入口である。

### 5.4 A にしかない項目と仮想 item

A-only のうち、仮想 item で失われている / いないを分ける。

- 「名前の変更...」は A-only だが、`ZipImage` / `PdfPage` / `Stack` は独立した実ファイルでないため、
  現仕様では仮想 item に移す対象ではない。
- grid 背景の「貼り付け」も実フォルダ専用であり、仮想 item へ出さないのが正しい。
- 「選択項目のパスをコピー」は A-only。仮想 item を含む複数選択では A が構築不能になり、B は
  「パスをコピー」を disabled にするため、合成パスの複数コピーは現在使えない。これは仕様判断が
  必要である。
- 単一仮想ページの右クリックには回転項目がない。real `Image` fullscreen では A-only だが、
  `ZipImage` / `PdfPage` fullscreen では B にも無い。ただし L / R の KeyAction は別入口として使える。
- それ以外の実用項目（パス / 名前コピー、`ZipImage` の画像コピー、編集内容コピー・貼り付け、
  代表サムネ pin、Explorer、外部ツール）は B が仮想 item 用実装を持つ。したがって
  **A-only だから仮想 item で失われている必須機能は、現時点では上の複数パスコピー以外に
  確認できなかった**。

### 5.5 旧 XMP の削除境界

旧 XMP の明示操作は次の層に分かれている。

1. 右クリック表示: `legacy_xmp_context_path`、3 か所の呼出、
   `draw_legacy_xmp_context_entries`
   (`src/ui_dialogs/context_menu.rs:393`, `src/ui_dialogs/context_menu.rs:783`,
   `src/ui_dialogs/context_menu.rs:854`, `src/ui_dialogs/context_menu.rs:1973`,
   `src/ui_dialogs/context_menu.rs:2107`)。
2. 上部「タグ」メニューの同じ 2 項目
   (`src/ui_main.rs:5578`)。
3. 選択 / path 入口、pending、完了 poll / toast
   (`src/tag_ops.rs:164`, `src/tag_ops.rs:181`, `src/tag_ops.rs:185`,
   `src/tag_ops.rs:794`, `src/app.rs:10599`, `src/app.rs:66594`)。
4. 明示 import / remove worker (`src/tag_legacy_xmp_worker.rs:13`,
   `src/tag_legacy_xmp_worker.rs:69`, `src/tag_legacy_xmp_worker.rs:87`)。
5. worker が共有する legacy `#` タグ判定と XMP 書換 helper
   (`src/tags_db.rs:1038`, `src/xmp_writer.rs:142`,
   `src/xmp_writer.rs:157`, `src/xmp_writer.rs:221`)。

削除の意味を二段階に分ける必要がある。

- **右クリック 2 項目だけを落とす**なら 1 だけを除去する。上部「タグ」メニューと worker は残り、
  処理の巻き添えは無い。
- **旧 XMP の利用者向け明示操作を全面廃止**するなら 1〜4 と、`lib.rs` の明示 worker module、
  runtime cancel / busy 判定、metadata transfer の pending 判定を整理できる。ただし 5 をファイル単位で
  消してはいけない。`tags_db::miv_legacy_tags` は別 worker の `tag_legacy_seed_worker` も共有しており、
  `xmp_writer` は `xmp:Rating` の書込みにも使われる
  (`src/tag_legacy_seed_worker.rs:101`, `src/tags_db.rs:1041`,
  `src/xmp_writer.rs:238`)。自動 seed という旧データ救済まで廃止するかは、本節の「右クリック項目を
  落とす」とは別判断である。

本計画 §1 の合意は「旧 XMP タグ関連の項目は削除」であり、自動 seed 廃止までは明記していない。
したがって現時点の判定案は、少なくとも 1 と 2 の利用者向け項目を落とし、3〜5 のコード整理は
自動 seed の継続方針を確認してから行う、である。

### 5.6 「アプリケーションで開く」の native 構造案

これは調査結果ではなく **提案**である。現在の `NativeMivMenuItem` は flat な leaf だけで、
`AppendMenuW(..., MF_STRING, ...)` を順に追加しているため submenu を表現できない
(`src/native_context_menu.rs:39`, `src/native_context_menu.rs:60`,
`src/native_context_menu.rs:403`)。共通項目定義を leaf / submenu の tree にする。

推奨構造は次のとおり。

```text
<登録済み外部ツール>で開く        ← show_in_context_menu=true の明示 quick action は最上位維持
アプリケーションで開く…           ← submenu
  <最近使ったアプリ>で開く         ← 最大3件、残す判断をした場合だけ先頭 section
  ─────────────
  <システム関連付けアプリ>
  ─────────────
  アプリケーションを追加…
  外部ツールの設定…
```

登録済み外部ツールは利用者が「右クリックメニューに表示」を明示した quick action なので最上位を
維持する。最近使ったアプリは最大 3 件なので、残すならもう一段の submenu を増やさず
「アプリケーションで開く…」直下の先頭 section にする。システム関連付けアプリも同 submenu に
入れ、追加 / 設定は separator 後の管理項目にする。この構造なら A の現在の quick action を
深くせず、B の open-with 群だけを一か所へ収められる。
