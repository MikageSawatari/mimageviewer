# 書庫内の本を開く・閉じる (§1.675 + §1.676) 設計

状態: **設計 R2 (2026-11-08)**。設計担当 ClaudeCode Opus 5.5。ソース調査と下書きは Codex GPT-6.1 Sol / high
(読み取り専用、`bec01bf11`)。利用者の採択は §9、独立設計レビューの経過は §9 に記録する。
利用者方針 (2026-11-08): 時間がかかるなら v5.1.0 より後に延期してよく、範囲を削らず理想的な形で入れる。
master へ入れる時期は完成時に利用者へ確認する。

報告と制約の正本は [バックログ §1.675](next-release-backlog.md) と §1.676。関連: [仮想フォルダ](virtual-folders.md)
の「ネスト ZIP のツリーナビ」「§2.3」「退出ルーティング」、[内部書庫ツリー設計](nested-zip-tree-plan.md) §3.3・§6、
[detached リワーク計画 §2](detached-rework-plan.md)。§1.678 (子の本だけの ZIP へ Ctrl+↓) と、通常の独立ビューアの
Ctrl+↑↓ の探索順 (物理 DFS が内部 DFS より先) はこの設計の対象外。

## 1. 何を直すか

- §1.675: 「開いたとき、ページを表示」が有効でも、書庫内フォルダ・入れ子書庫 (`GridItem::ZipDir`) を
  ダブルクリック / Enter / パッドで選ぶと、常にページ一覧で止まる。3 つの入口が `zip_nav_enter` を呼び、
  同関数が `StartupListIntent::ExplicitList` 固定で階層を表示するため。
- §1.676: 書庫内の A / B 一覧から A を開いて閉じると、外側の実フォルダまで戻る。
  `resolve_close_return_navigation` の最後の分岐が `effective_folder().parent()` を返し、ZIP 内の階層を見ないため。
- 付随: 内部本の読書位置は保存されていない (`book_resume_recording_folder` が非 root を除外)。内部本を
  「通常の書庫と同じように」開くには、本ごとの読書位置も要る (利用者採択で今回に含める)。

## 2. 決めた挙動

### 2.1 開く (§1.675)

- main の一覧で `ZipDir` セルを利用者が選んだとき (ダブルクリック / Enter / パッド決定。**1 つの共通入口**
  `activate_main_zipdir` を通す)、「ページを表示」の実効値が ON なら、子の階層に入り (既存の冗長ラッパー
  collapse を適用)、その階層に直接表示できるページがあれば本としてページを開く。
- 分類 (案 A、利用者採択): 内部フォルダも入れ子書庫も「ページを表示」だけに従う。どちらも書庫内の本 scope
  (`BookScope::Archive`) で、見開き設定の本キーや Ctrl+↑↓ の DFS も同じ単位で扱っているため。
  「画像のみフォルダは本として扱う」は実フォルダの分類設定のままとし、書庫内には効かせない。
- 直接表示できるかは**二段階**で決める。main の共通入口は、メモリ上のツリーとファイル種類の表示方針から
  「直下にページがある」候補判定だけを行う (未入場の子の本の構成はまだ読まれていない)。最終判定は、
  要求を持つ context で本の構成・★絞り込みを適用した後に行う:
  通常モードで表示ページが 0 なら**その階層の一覧表示**に終える。複数ウィンドウの独立ビューアでは通知して
  初回 open を終える。どちらも最初の子へ潜る DFS・フラット化はしない。
- 開く位置 (利用者採択で今回に含める):
  - 一覧からの直接表示は `book_open_resume`、内部本どうしの手動フルスクリーン DFS は `book_nav_resume` に従う
    (外側コンテナの Ctrl+↑↓ と同じ設定の意味)。スライドショーの自動本送りは先頭のまま。
  - 今の内部 DFS は常に先頭着地なので、`book_nav_resume = 続きから` の利用者には挙動の変更になる。ただし
    内部本の保存値は今まで無いので、更新後の初回は先頭、その後は保存位置になる。
  - 保存位置は PageIdentity で構成適用後の列へ解決する。保存ページが非表示・消失なら既存の通知と先頭の規則。
    外側の root の位置を内部本へ流用しない。外側書庫を開いたときに保存済みの内部本へ自動で潜る機能は足さない。
- 直接表示しない入口は今の意図を保つ: Backspace / `zip_nav_back`、アドレス・パンくず、履歴 Back/Forward、
  refresh、ソート・ピンの再 materialize、★一覧の ZipDir、ブックマークの prefix 復元、Required leaf、
  コレクション、一覧の Ctrl+↑↓、ファイル名スタック、★固定。
- 複数ウィンドウモード (利用者採択で今回に含める): 共通入口が同じ候補判定を行い、本なら ZipFile セルと同じく
  独立ビューアで開く (park・新規窓・位置・focus は既存経路)。main の階層・items・選択・位置・facet・
  表示フィルタは変えず、main の★一時解除もしない。分岐 (子の本だけ) なら今どおり main の一覧で中へ入る。
  詳細は §4.5。

### 2.2 閉じる (§1.676)

- main (と main の bundle を映す linked F12) で、「ページを表示」の実効値が ON のときの通常の閉じる
  (Esc / × / Enter の close 割り当て / 右クリック → `handle_fullscreen_close_request`) は、
  **利用者がその本を選んだ書庫内の一覧**があればそこへ戻る。メモリ上のツリーから一覧を作り直し
  (再列挙しない)、読んでいた本のセルを選び (`zip_nav_back` と同じ前方一致規則)、選んだときの位置
  (セルが画面上端から何行目か) に戻す。
- 対象: §1.675 の直接表示、今の「内部のページ一覧からページを手動で開く」経路、深い階層、collapse 後の
  実効 prefix、同じ一覧の下でのフルスクリーン Ctrl+↑↓ (A → B に移ったら B を選ぶ)。
- 書庫内の一覧で本を選んでいない場合 (外側書庫を直接ページ表示して root ページから DFS で内部本へ、
  実フォルダからの Ctrl+↑↓ で書庫へ、ブックマーク・Required leaf で深い階層へ直接) は今の帰路のまま。
- 変えないもの: フルスクリーンの Backspace (今の本のページ一覧 L2 へ)、通常経路で「ページを表示」OFF の
  close (L2 へ。横断ブックマーク一覧の専用 close が先に立つ既存の例外はそのまま)、独立ビューアの close
  (§1.670 A: main を変えない)、同じ書庫を読む兄弟ビューアの状態。
- 帰路の優先順位 (利用者採択): **記録済みの書庫内一覧を最優先**にし、その後に既存の専用帰路
  (Everything 結果、コレクション、ブックマーク、閲覧履歴、★一覧、サブフォルダ展開、スマートフォルダ) と
  物理親が今の順で続く。書庫内一覧の記録は、専用帰路で書庫を開いた**後に**利用者が書庫内で本を選んだときにしか
  作られないので、「本を選んだ一覧へ戻る」がより新しい事実である。内部一覧へ戻るときは外側の専用帰路の情報
  (`BookmarkViewState::Opening` など) を消費せずに残し、その後の Backspace で書庫の root、さらに外側の既存帰路
  (コレクションの物理子・実親など) へ今どおり戻れるようにする。内部一覧で本を選んでいない場合の専用帰路は変わらない。

## 3. 所有: 「どの一覧から本を選んだか」は `ZipNavState` の階層が持つ

既存の `ZipNavLevel { display, raw }` (実効 prefix スタックの 1 段) に、排他的な値を 1 つ足す。

```rust
enum ZipListChoice {
    /// 生成しただけ (enter / restore_prefix / DFS / refresh)。一覧を見た証拠にならない。
    Unseen,
    /// 利用者がこの階層の一覧でセルを選んだ。帰還時の位置もこの値が持つ。
    Chosen { rows_above: Option<u32> },
}
```

- **producer は共通入口 `activate_main_zipdir` の「main の一覧で入る / main で本を開く」分岐だけ**。
  一覧でセルを選んだこと自体が、その一覧を見た証拠になる。出発階層 (現在の stack 末尾) を
  `Chosen { rows_above }` にしてから子へ入る。`rows_above` は既存の `ListCursorHint` と同じ
  「選択セルが表示上端から何行目か」(`selected_rows_below_view_top`)。複数ウィンドウで独立ビューアに渡す分岐は
  main が動かないので記録を作らない。
- 内部のページ一覧 (L2) からページを手動で開く経路は、新しい記録を作らない。その本を選んだ親階層の
  `Chosen` が残っているので、それを帰路に使う。
- `startup_list_restore` (保存される「最後に明示的に開いた一覧」) は使わない・変えない。プロセスをまたぐ保存値で、
  今のツリーとの相関も DFS で作った階層の未表示性も示せないため。
- 帰路の解決: close の時点で、**現在の本を含む最も深い `Chosen` の厳密な祖先階層**を探す。
- 配送: 既存の `FullscreenCloseContinuation` / `CloseReturnEffect::Navigation` に、typed な nav
  `AddressBarNav::ZipListReturn(plan)` を載せる。plan は owner から導いた一回限りの値で、
  別の来歴 owner ではない (中身: 帰還する階層の深さと display prefix、読んでいた本の prefix、`rows_above`、
  ツリーの identity)。`transfer_terminal_close_return` は解決済みの帰還先から effect を選び、`ZipListReturn` なら
  `Navigation` のまま保持する (`BookmarkViewState::Opening` で `BookmarkNavigation` に置き換えない・消費しない)。
- 適用の順序 (Live / Terminal / `handle_fullscreen_close_request_immediate` / in-window 静止画の early-return は、
  すべて既存の `apply_fullscreen_close_nav_immediate` の新しい分岐へ集める):
  1. plan が現在の main の同じツリー (`Arc` の同一性) と、まだ `Chosen` の階層を指すことを確かめる。
     外れていたら**plan を捨ててログを出すだけ**にする (別の帰路へ置き換えない)。
  2. 旧 items が有効なうちに、既存の close teardown (`close_fullscreen`、linked viewport の退役、
     旧ページの編集プレビュー捕捉) を完了する。Terminal はすでに teardown 済みなので、共通の consumer で区別する。
  3. 帰還する階層まで stack を pop し、`zip_nav_show_current_level(ExplicitList)` で表示する。
  4. 読んでいた本のセルを選び、`set_row_anchor(rows_above)` と `set_reveal_requested(true)` を**常に組で**設定する
     (install は位置を 0 にして reveal を解除し、anchor は reveal 時にだけ消費されるため)。実際の layout で
     適用されるので、offset は行の高さの整数倍になる。

### 3.1 生成・失効

| 出来事 | 扱い |
| --- | --- |
| 共通入口でセルを選ぶ | 出発階層を `Chosen` に更新 (同じ階層で別の本を選び直せば上書き) |
| 同じ書庫内の本の切替 (フルスクリーン DFS) | DFS は clone 上で stack を pop / push して確定するだけなので、共通の祖先階層の値はそのまま残る。新しく push した階層は `Unseen` |
| Backspace で階層を戻る | pop するだけ。親の `Chosen` は残る |
| 別の書庫・フォルダの採用 | `zip_nav` ごと破棄される (既存) |
| refresh (ツリーの作り直し) | 記録を捨てる (全階層 `Unseen`)。書庫の中身が読書中に変わるのはまれなので、今の帰路へ戻る割り切り |
| 同じ階層の再 materialize (ソート・ピン) | stack を変えないので維持 |
| 履歴 Back/Forward | `restore_prefix` で作った祖先は `Unseen` |
| 同じ書庫内へのブックマーク移動・Required leaf (`enter` で既存 stack に積む経路を含む) | 直接 open の意味なので、**全階層の記録を捨てる** |
| ★固定 snapshot | 既存の `saved_zip_nav` 退避・復帰に値ごと同行。固定中は帰路を発行しない |
| 独立ビューアの build / fork / promote | **独立 bundle になる側の `zip_nav` と保存中の `saved_zip_nav` の記録を `Unseen` にする** (§1.670 A と同じく、main への帰還権限を独立 bundle に作らない)。main 側の原本は保持 |
| main admission・Remote 取得・終了 | 既存の close return 失効入口 (`retire_close_returns_for_main_admission` 等) がそのまま配送中の plan を捨てる |

## 4. 開く処理の接続 (§1.675、通常モード)

- 共通入口は、候補判定 → (main で動く分岐なら) 出発階層の記録 → 現行の★一時解除
  (`maybe_suppress_rating_filter_for_opened_zip_book`) → clone した nav で子へ入る → 一覧なら `ExplicitList`、
  本として開くなら `PageContinuation` で階層を install する。現在の `zip_nav_enter` は一覧移動用 helper として残し、
  既存の他の caller は変えない。戻り値は「非対象」と「拒否」を区別する typed outcome とする。
- install 直後に先頭 idx を選んで開かない。階層 install の後は既存の `ContinuationOwner::IndexOnly`
  (ページ identity の index 準備) が動くので、その完了動作に型付きの値を持たせる:
  `IndexOnlyAfterPrepare::{None, OpenArchiveBook(DeferredFsReopen)}`。新しい App の pending は作らない。
  true close 用の取消 (`sidecar_restore.rs` の `deferred_fullscreen` と Live continuation の取消) にもこの値を含める。
- `DeferredFsTarget` に `CurrentArchiveBook { scope: BookScopeKey }` を足し、leaf 指定 (Required / Preferred) と区別する。
  scope は表示 prefix ではなく、既存の**論理 source + `current_raw()`** による `BookScopeKey`
  (`book_composition_runtime.rs` と同じ) とする。
- 完了時に scope・context・items 世代が一致することを確かめてから、`CurrentArchiveBook` 専用の解決を行う:
  保存位置 (`book_open_resume` が続きからのとき) を PageIdentity で解決し、無ければ
  **本の構成順と現在の可視集合 (`visible_indices`、★・ファイル種類) の両方を満たす最初の直下ページ**を選ぶ。
  目的は先頭なら `BookTargetPurpose::FirstInBook`、保存位置なら `Explicit`。既存の本の構成待ち・表示許可を通って
  開く (`from_explicit_open = true`、`HistoryTrigger::UserChosen`、resume notice は要求に同行)。
- 候補が無ければ、型付きの結果で**その階層の一覧表示**に終える。既存の `DeferredFsTarget::None` / Required の終端と
  §1.678 の `NoPlayableItem`・独立窓の終端の意味は変えない。
- 取消・失敗・古い完了: 既存の sidecar / 構成 owner が要求を捨てる。キャンセル後に勝手に開かない。
- Enter で開いた直後に同じ Enter が close に流れない既存の抑制 (`fs_suppress_enter_close_until_release`) を適用する。
- フルスクリーン DFS (`zip_nav_dfs_fullscreen`) も、次の本の先頭 idx を先に選ぶ順序をやめ、着地確定 → 既存の
  navigation sequence / holdover → `PageContinuation` で install → index 準備・resume fence・構成 →
  `book_nav_resume` で解決 → 表示許可 → 既存の DFS 完了処理、とする (§4.2「内部DFS」)。

### 4.1 内部本の読書位置キー

#### 型と保存形式

実パスと保存キーを区別する共通型を追加する。名称は実装時に確定するが、意味は以下に固定する。

```rust
enum BookResumeKey {
    /// 出荷済みroot／実フォルダのキー。既存normalizeをそのまま使用。
    LegacyPath(String),

    /// 非rootの書庫内本だけに使用。
    ArchiveLevel {
        logical_source: PathBuf,
        raw_prefix: String,
    },
}
```

生成入口：

```rust
BookResumeKey::legacy_path(path)
BookResumeKey::archive_level(logical_source, canonical_raw_prefix)
App::current_book_resume_key()
```

`ArchiveLevel` の保存文字列は次に固定する。

```text
miv:zipbook:v1:<hex(UTF-8 canonical logical source)>:<hex(UTF-8 canonical raw prefix)>
```

- hexは小文字・2桁固定。
- logical sourceは `BookScopeKey` と同じドライブ保持・区切り統一・大小文字正規化を使う。
- prefixは `ZipNavLevel.raw` から `BookScopeKey::archive` で検証・正規化する。
- prefixは非空。root／collapse済みrootには使用しない。
- ハッシュではなく可逆な符号化とし、rename／deleteのfamily照合で元sourceを取得できる。
- 符号化済み文字列を `PathBuf` に偽装して通常のpath処理へ渡さない。

Windowsの実パス由来の既存キーは、ドライブ除去後のpathまたはUNC等である。新名前空間の `miv:zipbook:v1:` は有効なWindows実パスにならず、root／実フォルダキーと衝突しない。二成分はhexに含まれない `:` で区切るため、sourceとprefixの境界にも曖昧さがない。

単純な `outer.zip/prefix` 連結は採らない。既存pathキーとの区別とfamily処理を保存文字列の偶然に依存させないためである。

#### root互換性

- ZIP root・実フォルダ・PDF等の既存キーは変更しない。
- **変換書庫のrootも現在のbackingキーを維持する。** 内部本だけがlogical sourceの新キーを使う。
- 内部本の保存値がない場合、rootの位置を読まない。
- PageIdentityは既存の `Archive { container, entry_name }` を使い、`container` はlogical source、`entry_name` は外側からの完全なentry名とする。新しいPageIdentity variantは追加しない。

根拠：

- `src/book_resume_db.rs:492`：既存キーのドライブ除去normalize。
- `src/book_scope.rs:61`：scopeの検証・正規化。
- `src/zip_tree.rs:328`、`:425`：displayとrawの分離。
- `src/app/book_composition_runtime.rs:533`：構成対象のraw prefix。
- `src/app/book_resume_meter.rs:359`：変換rootの既存backingキー。
- `src/book_resume_db.rs:50`：PageIdentity生成。

### 4.2 読書位置のproducer／consumer統一

| 経路 | 現行 | 今回の変更・根拠 |
|---|---|---|
| `book_resume_recording_folder` | 非root ZIPはNone | `current_book_resume_key` に置換。rootは従来値、内部はlogical＋raw。`src/app.rs:62741` |
| `record_book_resume` | rootだけ記録 | 内部本も同helperで保存。構成のcanonical raw index、既存PageIdentity、meterを維持。`src/app.rs:62750` |
| dedup／`last_book_resume` | tupleの先頭がPathBuf | 同じfieldを `BookResumeKey` に変更。並列cacheは作らない。`src/app.rs:18035` |
| `resume_page_for_container`／`resolve_book_resume_for_container` | `current_folder` で照会 | 現在本の同helperで照会。任意pathを読む既存APIはlegacy adapterとして残す。`src/app.rs:62987`、`:62998` |
| DB／writer | RecordはPathBuf、read fenceは文字列 | 内部APIはtyped keyを受ける。出荷済みpath APIはLegacyPathへ委譲。既存FIFOとsnapshotを共用。`src/book_resume_db.rs:501`、`:821`、`:858` |
| spread restore fence | 保存側と同じroot資格判定 | 同helperでcutoff時のキーを捕捉。`FencedResume` も同キーで照合。`src/app/spread_shift_owner.rs:526`、`:562`、`:943` |
| composition editorのread | 保存対象folderを取得 | 同helperからキーを捕捉して既存FIFO readへ。`src/app/book_composition_editor.rs:362` |
| editorのwrite intent | 同folderを保存意図へ保持 | `ResumeWriteIntent` のキーをtyped化。editor ownerのscope照合を維持。`src/app/book_composition_editor.rs:682`、`:1347` |
| meterの記録・全件読取 | path normalizeでmap更新 | typed keyのstorage文字列で更新。rootと内部の値を混ぜない。`src/app/book_resume_meter.rs:242` |
| meterの削除・reload | path prefixだけを照合 | 新キーをdecodeしてlogical sourceのfamilyで照合。既存reloadでwriterの位置snapshotも更新する。`src/app/book_resume_meter.rs:101`、`src/book_resume_db.rs:652` |
| thumbnail meter | ZIP内部では非表示 | 今回は描画対象を増やさない。rootのバーへ内部位置を流用しない。保存件数・clearには内部行も含む。`src/app/book_resume_meter.rs:314` |
| resume notice | path＋拒否理由 | 保存キーと表示名を分離し、logical source／内部prefixを通知・logに使う。符号化キーやcache pathを表示しない。`src/app.rs:1900`、`:63030` |
| 読書履歴 | 非rootは外側行をtouchし、位置を付けない | 維持。内部resume保存成功を理由に外側履歴の進捗を上書きしない。`src/app.rs:62853` |
| metadata export/import | resumeを転送しない | 維持。新キーをexport対象へ加えない。manifestにresume sectionはない。`src/metadata_transfer.rs:368`、`:8577` |
| cache削除 | cache本体・cache DB行だけを削除 | 維持。内部resumeのlogical source行は消さない。再変換後も同じキー。`src/archive_cache.rs:544` |

`BookResumeMeters` の実パスmemoは媒体用にも使われるため、これを符号化キーのpath memoへ転用しない。内部行には `record_key/get_key` を用意し、既存の媒体path処理は維持する。

#### 内部DFS

内部DFSでは、次本の先頭idxを選んでからresumeを適用する順序をやめる。

```text
clone navで着地確定
 → 既存navigation sequence／holdover開始
 → PageContinuationで対象levelをinstall
 → index準備・resume fence・構成準備
 → book_nav_resumeで保存位置／先頭を解決
 → 提示許可
 → 既存DFS完了処理
```

- `BookDfsContinuation::Navigation` は `book_nav_resume`。
- `Slideshow` は先頭固定。
- 保存位置へ着地するときは `BookTargetPurpose::Explicit`、先頭探索は `FirstInBook`。現行の `FirstInBook` 固定を残さない。
- 全ページ非表示等は既存DFS continuationの扱いへ戻す。grid activationの「一覧へ留まる」と混同しない。
- resume noticeも既存deferred／permission要求に同行させる。

根拠：`src/app.rs:40617`、`src/app/book_composition_permission.rs:1440`、`src/ui_fullscreen.rs:41685`。

**範囲の注意：** 通常の独立readerのCtrl+↑↓は、現在は内部DFSより先に物理DFSへ進む。今回その探索順は変更しない。既に内部DFSを使うmain／linkedとCollection-origin経路に上記設定を適用する。独立reader一般の内部DFS優先化や§1.678の移動先仕様は別判断である。根拠：`src/ui_fullscreen.rs:42700`、`:42718`。

### 4.3 Remoteの読書位置

Remoteも同じ内部本キーを読み書きする。PCの現在表示やcache pathからscopeを推測しない。

現行では：

- ZIP列挙のresume照会をrootに限定：`src/remote_ipc/container.rs:6320`
- progress検証の `record_resume` もroot限定：同`:4398`
- UI側保存先は外側container path：`src/remote_ipc/ui.rs:3313`
- read要求はpathだけを保持：`src/remote_ipc/session.rs:1150`

変更は以下とする。

1. workerがraw treeと要求prefixから `ZipNavState::restore_prefix` 相当の共通scope resolverを使い、PCと同じroot判定・raw prefix・`BookResumeKey` を生成する。表示policyでcollapseしたprefixだけをキーにしない。
2. resume readの内部queueをpathからtyped keyへ変更する。既存 `read_position_after` のFIFO／cutoffを使う。`src/remote_ipc/ui.rs:2059`
3. progressの検証結果に、検証済みキー・PageIdentity・canonical raw indexを保持する。クライアント提供のscopeや符号化キーを信用しない。
4. `QueuedRemoteWrite`／`ClaimedRemoteWrite` の既存payloadを、通常writeと検証済みprogressを区別するenumへ拡張する。新しいApp pendingは作らない。`src/remote_ipc/session.rs:1142`
5. 内部本にもresume pageを返し、ページ表示設定が有効なら内部本の続き／先頭を使う。子本だけのbranchはGridのまま。
6. **resume記録と履歴進捗を分離する。** 現行UIは `record_resume` を履歴のpage/count付与にも使っている。内部resumeを有効にしても、外側履歴へ内部のordinalを保存しない。履歴touchの資格・抑制は維持する。`src/remote_ipc/ui.rs:3394`

HTTP／RemoteAddress／`RecordReadingProgress` のwire形式は変更しない。新しい検証済みscopeは本体内queueの値であり、IPC版変更は不要とする。

### 4.4 rename／move／deleteと保存行の寿命

新名前空間はgenericな実パスprefix SQLでは移行できない。**保存開始と同じ段階でfamily adapterを実装する。**

| 操作 | 現行 | 今回 |
|---|---|---|
| mIV管理のrename | `STORES` のexact・`/`・`::` family移行 | book_resume用adapterが新キーをdecodeし、logical sourceを実パス境界で移行して再encode。`src/rename_key_migration.rs:1339`、`:1866`、`:2729` |
| 製本等の管理move／copy | 同store registryのmove／copy | 同adapterを使用。移動先優先／source優先等、各入口の既存衝突規則を維持。`src/rename_key_migration.rs:1940`、`:2036`、`:3023` |
| archive／親folderの削除 | Shell成功scopeだけhard purge | decodeしたlogical sourceを既存Exact／Tree scopeに照合し、該当内部行も削除。`src/rename_key_migration.rs:2363`、`:2600` |
| メモリ上のresume／meter | path比較で退役 | `BookResumeKey` のsource family判定を使う。`src/app.rs:48509`、`:48590` |
| DB移行後 | meter reload | 既存reloadを使い、writer snapshotにも新キーを公開。`src/app.rs:50850`、`src/book_resume_db.rs:652` |
| 不在metadataの掃除 | DriveStripped storeは安全上除外 | この扱いは維持。符号化キーを実ファイルとしてstatしない。`src/metadata_cleanup.rs:668` |
| 通常Shell整理／外部Explorer移動 | metadataを自動移行しない | 維持。今回だけ新しい追跡・移行機構を追加しない。`src/ui_dialogs/file_organize.rs:519` |
| cache削除／再生成 | 元書庫の利用データとは別寿命 | 内部行を保持。source＋raw prefixが同じなら再利用 |

renameではキーだけでなく、新しい内部行の `PageIdentity::Archive.container` も同transactionで更新する。`entry_name` は書庫内の完全entry名なので、外側archiveの改名では変更しない。

既存 `rewrite_book_store_values` はImage identityだけを書き換えるため、内部Archive行への対応が必要である。`src/rename_key_migration.rs:2180`

adapterは元のドライブ保持pathを受け取る。既存のDriveStripped文字列から元ドライブを逆算しない。新キーfamilyの照合はdecodeしたsourceに対して行い、hex文字列の前方一致で代用しない。

metadata bundle転送は引き続きresume対象外である。store移行／content-identity復元と、利用者が行うmetadata export/importを混同しない。


### 4.5 detachedの内部本scope指定

#### descriptor

```rust
enum DetachedZipTarget {
    RootBook,
    BookScope(ArchiveBookTarget),
    RequiredPage { entry_name: String },
}

struct ArchiveBookTarget {
    /// 選択セルのliteral prefix。元の綴りを保持し、tree lookupに使用。
    requested_prefix: String,
    /// logical source＋canonical raw prefix。scope一致確認・resumeに使用。
    scope: BookScopeKey,
}

// ViewerContextDescriptor::Zip の entry_name を target に置換
Zip {
    path: PathBuf,                         // 読取backing
    archive_source_override: Option<PathBuf>, // 元source
    target: DetachedZipTarget,
}
```

`RootBook` は既存 `entry_name=None`、`RequiredPage` は既存 `Some(entry)` と同じ意味を保つ。`BookScope` を代表画像のRequiredPageへ変換してはならない。

`requested_prefix` は存在確認用、`scope.level_prefix` は正規化済みidentity用であり、後者の小文字化文字列でcase-sensitiveなentry lookupをしない。

根拠：現行型は `src/app.rs:4109`、Required leaf解決は `src/app.rs:63436`、内部leafのmaterializeは `src/app/snapshot_ops.rs:2523`。

#### mainからのdispatch

1. 三入力ともgeneric detached routerより前にZipDirを共通入口へ渡す。rating-view ZipDir等の既存専用入口は維持。
2. clone navで対象levelを解決し、通常モードと同じ直下ページ候補predicateを使う。
3. branchならmainで従来の一覧移動を実行し、その共通入口だけが出発levelを `Chosen` にする。
4. detached本ならmainを移動せずdescriptorを生成する。mainの★一時解除も行わない。main内帰路が不要なので、この分岐では新しい `Chosen` を作らない。
5. `DetachedGridItemOpenPlan::Descriptor` の既存実行経路へ渡す。park失敗／拒否はその入力の終端として通知し、通常main openへfallthroughしない。

park／placement／focusの実行先は `src/app.rs:67646`。共通入口の戻り値も「非対象」と「拒否」を区別するtyped outcomeとする。

#### 独立bundle内の準備と要求の単一所有

scope指定は**rootをinstallした後で潜る方法ではなく、最初のZIP一覧生成前**に適用する。rootの位置・構成・初回表示を誤って採用しないためである。

```text
descriptor
 → 既存park／placement決定
 → start_active_detached_book_context_with_start
 → build_viewer_context
 → ZIP列挙
 → requested_prefix存在確認
 → restore_prefix
 → current_rawから生成したscopeと要求scopeを照合
 → 対象levelをPageContinuationとしてinstall
 → sidecar／identity index／resume fence／composition
 → book_open_resume
 → 提示許可
 → 初回表示・focus
```

接続する既存型を次のように拡張する。

```rust
enum ZipEnumerationPresentation {
    Existing(StartupListIntent),
    ArchiveBook(DeferredFsReopen),
}
```

- 既存 `ZipEnumeratePending.restore_intent` をこの型へ置き換え、scope要求を列挙ownerが保持する。
- `ArchiveBook` の `DeferredFsTarget` はscope targetと完了動作を持つ。root用の `pending_auto_fs_open` と重複予約しない。
- prepared ZIPの生成時にscopeを解決して対象levelだけをmaterializeする。
- 採用後は要求を既存sidecar continuationのZIP first-display動作へ**移す**。`restore_intent` の一覧採用上の意味は `PageContinuation`。
- 通常モードの軽量階層installでは、同じ要求を§4の `IndexOnlyAfterPrepare::OpenArchiveBook` が持つ。
- hydration／index完了後、共通deferred openへ移す。構成待ちがあれば既存permission ownerへ移す。
- 同じ要求を列挙slotと `fs_nav_after_pdf_enumerate` の両方へ保存しない。

主な接続先：

- `src/app.rs:39560`：ZIP列挙pending
- `src/app.rs:39777`：階層materialize
- `src/app.rs:39928`：準備済みZIP install
- `src/app.rs:40032`：ZIP first-display
- `src/app.rs:41538`：deferred open
- `src/app/sidecar_restore.rs:209`、`:2577`：継続ownerとIndexOnly完了

#### Converted archive／password／conversion

- descriptorの `path` はbacking、overrideとscopeのrootはlogical sourceとする。
- source／backingの組を**install前に**渡す。現行の「load後にoverrideをset」だけでは、prepared同期完了時のscope生成には遅い。
- 既存のProvenBacking経路とlogical-source付きprepared loadを使い、UI上でcache lookup／statを追加しない。`src/app.rs:39243`、`:39278`
- backingの再解決・再変換が必要な場合は既存archive open ownerを使う。`DetachedGridArchiveOpenRequestOwner` にtyped targetを同行させ、`ArchiveConvertCompletionPolicy::DetachedGridArchive` が同じlease／requestを保持する。
- 変換完了で現在の `RootBook` 相当へ戻してはいけない。元のBookScope targetをdescriptorへ引き継ぐ。`src/app.rs:67666`、`src/ui_dialogs/archive_convert.rs:75`
- RAR等のpassword／変換確認は既存modalを使う。取消はその要求を終了する。
- 通常ZIPに新しいpassword UIは追加しない。既存loaderで読めない書庫は通常の読取失敗として通知・終了する。
- `requested_prefix` が消失した場合、またはraw collapse結果が変わって別scopeになった場合は、rootや最初の子へfallbackしない。

#### 型付き結果と終端

```rust
enum ArchiveBookOpenOutcome {
    Opened,
    Waiting,
    ScopeMissing,
    ScopeChanged,
    NoDisplayablePages,
    Cancelled,
    Stale,
    Failed,
}
```

これは要求の結果であり、Appに追加する状態fieldではない。

| 結果 | 処理 |
|---|---|
| Opened | 既存初回表示完了・focus |
| Waiting | 同じ要求ownerが継続 |
| ScopeMissing／ScopeChanged | 「書庫内の本が見つかりません。元の一覧を再読み込みしてください」等を通知し初回終端 |
| NoDisplayablePages | 表示可能ページなしを通知し初回終端。子探索なし |
| Cancelled | 要求を消費し既存leaseの取消・終端へ |
| Stale | 採用せず破棄。後発readerを閉じない |
| Failed | ログ＋通知し初回終端 |

初回終端は `terminate_active_detached_open_before_viewport`／`cancel_detached_initial_open_for_lease` を使う。既に別readerへ交代したstale完了からこのhelperを呼ばない。根拠：`src/app.rs:63669`、`:63709`。

§1.678の既存Ready readerのナビ失敗やcleanup条件を、この新しい初回scope要求の結果に読み替えない。

#### main／兄弟不変

- 新bundleの `ZipListChoice` は全level `Unseen`。
- mainのLive／Terminal close continuationを継承しない。
- independent closeはmainのnavを発行・消費しない。
- immutableなtreeの共有は許すが、items・queue・cancel・generationは各bundle所有。
- 読取りopen／closeからarchive mutation通知や兄弟reloadを発行しない。
- mainのtreeが後でrefreshされても、独立readerの準備・表示を巻き戻さない。


## 5. 簡素化の確認

- 3 つの入口の予約を、1 つの共通入口と 1 つの型付き値にする。
- 帰路は「一覧でセルを選んだ」という 1 種類の事実だけから作る。一覧採用の通知や保存記録との連動を足さない。
- refresh・履歴・直接 jump・独立化では記録を捨てて今の帰路へ戻す。記録の移し替え規則を増やさない
  (DFS で共通祖先が残るのは stack の性質で、移し替えではない)。古い plan は捨てるだけで、別の帰路へ置き換えない。
- 開く要求は、通常モードでは IndexOnly → 構成 → 表示、独立ビューアでは列挙 → hydration → 構成 → 表示と、
  既存の owner 間で 1 つの値を移すだけにする。同じ要求を 2 か所へ保存しない。
- 読書位置のキーは 1 つの helper (`BookResumeKey`) から作り、保存・読取・fence・メーター・名前変更の全経路で共用する。
- モーダルは不要: 内部階層の列はメモリ上のツリーから作り、非同期は既存の index 準備・構成待ちとその取消を使う。
  変換・パスワードの既存モーダルは維持する。
- まれな失敗は既定の割り切り (ログ + 通知)。再試行・回復用の状態は作らない。

## 6. 永続化

`book_resume` は出荷済みデータである。今回変更するのは**内部本用の行キーの追加**であり、テーブル・列・既存行の形式は変更しない。

- DDL追加、schema version変更、既存root行の再キー化・削除はしない。
- `LegacyPath` adapterで旧データを従来どおり読む。内部位置は従来保存されていないため、移行元の内部行はない。
- 新行のPageIdentityも既存Archive variantを使用する。旧版の全行読取が未知JSON variantで失敗しない。
- 旧版は新名前空間をroot／実フォルダのopenで照会しない。通常の旧版利用では新行を保持し、再更新後に再利用できる。
- 明示的な「本の読書位置をすべてクリア」は旧版・新版とも全行削除であり、内部行も対象。
- **downgrade中の制限：** 旧版のrename／moveは新名前空間を移行できない。旧版中に元書庫を改名・移動した場合、新版へ戻しても旧sourceの内部位置は自動追随しない。これは既存root値の破損ではなく、新機能の旧版非対応として明記する。
- 新版では保存開始と同時にrename／copy／delete adapterを提供する。
- `startup_list_restore` V1と帰路の保存規則は変更しない。`Chosen` はruntimeだけ。

これはCLAUDE.mdの出荷済みデータ方針に対する、**既存形式の読み替えを維持した追加互換**である。「schema変更なし」だけを理由に互換試験を省略しない。

根拠：`CLAUDE.md:415`、`src/book_resume_db.rs:197`、`:336`、`:365`。

## 7. detached §2 との関係

今回触れる範囲：

| 境界 | 変更 |
|---|---|
| descriptor判定 | `src/app.rs:63381`、`:63436`。BookScopeとRequiredPageを区別 |
| plan実行・build | `src/app.rs:67413`、`:66993`。既存park／placementへscope指定を配送 |
| descriptor load／継続 | `src/app.rs:66900`、`:66943`。対象scopeを列挙・hydrationへ渡す |
| cache／conversion完了 | `src/app.rs:67666`、`src/ui_dialogs/archive_convert.rs:75`。targetを失わず同じleaseへ配送 |
| reopen descriptor | `src/app.rs:63428`、`src/app/startup_ops.rs:1838`。既存leaf／bookmarkの意味を維持 |
| 参照中source判定 | `src/app.rs:15643`。backing／logical sourceの既存削除判定を維持 |
| registry fork | `src/app/viewer_context_registry.rs:4293`。複製navのChosenを消去 |
| 待機・取消述語 | `src/app/sidecar_restore.rs:806`、`:819`。新typed継続も同じownerから待機／取消を答える |
| viewportの既存consumer | `src/app.rs:63918`、`src/ui_fullscreen.rs:26446`。初回Preparing／既存待機述語を利用 |
| close | main／linkedの帰路配送、独立の初回終端。独立closeのmain不変を維持 |

`active_detached_transition_outstanding` は既存Preparingを初回要求の根拠としている。空idx・描画済みflag・時間経過から新しいopen／close判定を作らない。

HWND／rect capture、viewport ID、registry transaction、placement保存、timer、再生成方策は変更しない。通常独立readerの物理DFS優先述語も変更しない。

本変更は、**入口でscopeを型として確定し、同じ要求を準備・提示・取消・終端まで所有させる構造修正**である。keep-aliveへのZipDir特例やclose後のmain復元ではない。実装前の構造合意と、実装後の独立レビュー・§11記録の両方を必要とする。

設計段階で設計担当と独立レビューの双方から「症状パッチではなく構造的修正」への合意を取り、実装後の完了レビューでも
同じ合意を取って [detached 計画 §11](detached-rework-plan.md) に記録する。

## 8. テスト (headless)

既存の `AppTestEnv`、`startup_restore.rs` の `write_zip`/`settle_book`、`ctrl_down_close_1678.rs`、
`everything_close_return.rs` の fixture を使う。実アプリ・OS の窓・キー入力は使わない。
変換 RAR は変換済み cache を使う headless 検証であり、実 RAR の変換そのものの検証とは分けて書く。

### S1: 共通入口・開く・閉じる

- `section1675_zipdir_activation_routes_all_inputs`: ダブルクリック handler・Enter・パッドが同じ共通入口を 1 回通る
  (内部フォルダ / 入れ子 ZIP)。
- `section1675_zipdir_setting_matrix`: 「ページを表示」ON/OFF × フォルダ / 書庫。直接ページの有無、子への DFS なし。
- `section1675_branch_and_collapsed_wrapper`: 子の本だけの分岐は一覧、単一・多段ラッパーの collapse 後に正しい本。
- `section1675_projection_gates_open`: 構成あり + ★一時解除なしで「先頭だけ除外」「全ページ除外」、順序変更、
  ファイル種類の除外。表示許可の前に開かない。全ページ除外なら一覧に終わる。permission 待ち中の再投影。
- `section1675_converted_zipdir_uses_logical_source`: 変換 cache (RAR/7z を source とする) で論理 source と backing を区別。
- `section1675_stale_and_cancel_do_not_open`: index 準備中・構成待ち中それぞれに別の階層・別の書庫へ移る、取消、
  古い完了で開かない。
- `section1676_direct_and_manual_open_return`: 直接表示 / L2 から手動でページを開く。Esc・×・Enter close・右クリックの
  共通 handler で A/B 一覧、A を選択、位置が戻る。ZIP と変換 RAR。Live / Terminal / immediate / in-window early-return の
  それぞれで、consumer の適用後にフルスクリーンが終わり、帰還が 1 回だけ行われる。
- `section1676_scroll_restored_after_layout`: anchor を設定した直後ではなく、実 layout 後の選択・offset・anchor の消費。
  詳細表示、列数の変更、最終行を含める。
- `section1676_deep_and_collapsed_return`: 多段・collapse。literal なセルを選び、見ていない中間階層へ戻らない。
- `section1676_dfs_selects_current_sibling`: A/B 一覧 → A → フルスクリーン DFS → B → close で B を選択。
- `section1676_unseen_parent_keeps_old_return`: 外側の直接表示 → DFS、実フォルダからの Ctrl+↓、ブックマーク。今の帰路。
- `section1676_backspace_and_auto_off_keep_l2`: フルスクリーン Backspace と「ページを表示」OFF は今の本の L2。
- `section1676_choice_lifetime`: 履歴移動・refresh・同じ書庫内のブックマーク / Required jump の後は記録なし。
  ソート・ピンの再 materialize では維持。★固定中の fork → 固定解除、promote で独立側に記録が無い。古い plan は捨てられ、
  開かない・戻らない。
- `section1676_special_return_precedence_and_back_out`: Everything・コレクション・閲覧履歴・ブックマーク・★一覧・
  サブフォルダ展開・スマートフォルダ経由で書庫を開き、内部で本を選んだ場合は内部一覧へ (Live と Terminal の対)。
  そこから Backspace で root → 外側の既存帰路へ戻れる。内部で選ばない場合は専用帰路のまま。
- `section1676_linked_f12_returns_main_level`: linked F12 の close で main の bundle に 1 回だけ戻る。
- `section1676_independent_close_never_mutates_main`: 独立ビューア (fork 含む) の close で main の一覧・選択・位置が不変。
  main に Live / Terminal が残っている状態での独立 close も含める。
- `section1676_same_archive_sibling_is_unchanged`: 同じ書庫を読む兄弟の items・世代・キュー・取消が不変。
- 既存回帰: `zip_nav_back_selects_book_cell_left_from`、`zip_nav_ctrl_updown_*`、`section1335_*`、`section1328_*`、
  `collection_zip_details_auto_close`、`detached_close_keeps_main_results_in_both_window_modes`、`section1678_*`
  (調査テスト・ignored 契約は変えない)。

### S2: 内部本の読書位置

- `section1675_inner_resume_key_namespace`  
  root／実フォルダ／UNC／別ドライブ／区切り文字を含む名前／日本語を比較。新キーの可逆性・非衝突・raw prefix採用を検査。
- `section1675_inner_resume_root_keys_unchanged`  
  通常ZIP・wrapper root・変換rootの旧キー／値が不変。
- `section1675_inner_resume_independent_books`  
  A/B、folder/nested ZIP、多段をparameter化。各PageIdentityが別行に保存され、direct openで各位置へ戻る。
- `section1675_inner_resume_navigation_policy`  
  `book_open_resume`／`book_nav_resume` 全組合せ。手動内部DFSとslideshowの差、旧設定の意味を検査。
- `section1675_inner_resume_fence_after_rapid_reopen`  
  既存writer pause fixtureを使用。保存直後の再openで旧snapshotを採らない。
- `section1675_inner_resume_projection_and_notice`  
  順序変更・hidden・種類除外・消失。identity照合、notice一回、外側位置非流用。
- `section1675_inner_resume_editor_intents`  
  editorの既存read／save／reorder fixtureで内部キーと書込先を検査。
- `section1675_inner_resume_remote_matches_pc`  
  PC→Remote／Remote→PC、policy collapse差、変換cache差。raw scopeキー一致・クライアント偽造拒否・外側履歴進捗不変。
- `section1675_inner_resume_maintenance`  
  managed rename／親folder move／copy／削除、既存衝突規則、Archive identityのcontainer更新、兄弟・別ドライブ不変。
- `section1675_inner_resume_cache_rebuild`  
  cache削除・別backing再生成後にlogicalキーを再利用。root行は従来どおり。
- `section1675_inner_resume_legacy_roundtrip`  
  旧schema fixtureを既存初期化へ通す。旧読取／旧page書込／新版再読取でrootと新行が保持される。
- `section1675_inner_resume_transfer_excluded`  
  metadata export/importでresumeが追加転送されない。
- `section1675_inner_resume_meter_clear_and_reload`  
  rootと内部のmeter混同なし、削除後snapshot更新、全件clear。

既存fixture：`book_resume_db.rs` の `pause_for_test`／`pause_before_next_completion_for_test`、`src/app/book_resume_meter_tests.rs`、`src/app/rar_archive_cache_tests.rs`、composition editorのresume fixture、Remoteの `remote_nested_page_without_resume_preserves_previous_root_meter`。最後のテストは新仕様に合わせて「内部行を保存し、旧rootを保持」の期待へ更新する。

### S3: 複数ウィンドウ

既存 `section1675_multiwindow_zipdir_keeps_main_list` は、branch据え置きとbook独立openの二つへ分ける。

- `section1675_multiwindow_zipdir_book_all_inputs`  
  三入力×folder/nested ZIP×通常/変換。独立bundle一つ、mainのitems・generation・prefix・選択・scroll・facet不変。
- `section1675_multiwindow_zipdir_branch_stays_main`  
  子本だけではsession／parkを作らずmain一覧へ。
- `section1675_multiwindow_scope_before_first_install`  
  rootページと内部ページを同居。最初から対象scopeでinstallされ、rootのresume／構成／履歴位置を採らない。
- `section1675_multiwindow_scope_collapse_and_raw`  
  多段wrapperとpolicyによるdisplay/raw差。要求scope一致・正しいresume。
- `section1675_multiwindow_scope_missing_changed_hidden`  
  enum前のprefix消失、collapse変化、全ページhidden。typed outcome・通知・初回Closing、mainへのfallbackなし。
- `section1675_multiwindow_scope_conversion_continuation`  
  cache hit／miss、RAR probe、password取消、変換完了。targetとlogical sourceが最後まで一致。
- `section1675_multiwindow_scope_stale_lease`  
  初回取消・窓交代・Remote acquire・exit後の遅着。後発readerを開かない／閉じない。
- `section1675_multiwindow_scope_park_and_reopen`  
  既存park／always-new fixtureでplacement・window ID方策・RequiredPage reopenを維持。
- `section1675_multiwindow_scope_no_main_return`  
  全level Unseen、main returnなし。Esc／×close後もA/B一覧・選択・scroll不変。
- `section1675_multiwindow_same_archive_sibling_unchanged`  
  同じ書庫を読む兄弟のitems・queue・cancel・generation不変。
- `section1675_multiwindow_scope_wait_is_owned`  
  列挙→hydration→構成待ちの各境界で初回Preparingが維持され、取消で要求が消費される。

§1.678の既存調査テスト・ignored契約は変更しない。

## 9. 採択・レビュー

- 利用者の採択 (2026-11-08): (1) 分類は案 A (内部フォルダ・入れ子書庫とも「ページを表示」だけ)、
  (2) 閉じると本を選んだ内部一覧へ戻す、(3) 内部本ごとの「続きから」を今回に含める、
  (4) 複数ウィンドウモードも今回に含める。範囲を削らず、v5.1.0 に間に合わなければ延期してよい。
- 独立設計レビュー R1 (Codex GPT-6.1 Sol / xhigh、`7e30b572f`): REVISE。P1 なし、P2 4 件・P3 2 件。
  P2 (close teardown の順序 / Terminal のブックマーク上書き / 構成と★を満たす先頭と FirstInBook / anchor と reveal の組) と
  P3 (同じ書庫内の直接 jump・fork / promote / 保存中 nav の記録消去 / stale plan は捨てる) を R2 で反映した。
  detached 部分を「症状パッチではなく構造的修正」とする方向には同意 (R2 の範囲での再確認待ち)。
- 独立設計レビュー R2: 未実施。

## 10. 実装段階と規模

| 段階・commit境界 | 内容 | 主な検証 |
|---|---|---|
| **S1：共通activationとmain帰還** | 採択済みOption A、Chosen、row anchor、通常／即時／early-return close、typed内部open継続 | 現§8の入口・close・deep／collapse・専用帰路・linked／独立不変 |
| **S2：内部resumeの保存契約** | key helper、DB/writer adapter、read/write/fence、内部DFS、editor、Remote、maintenance、互換性 | 上記A全件。保存開始とmaintenance対応を同じcoherent commitに含める |
| **S3：複数窓の内部本open** | descriptor target、列挙前scope解決、source alias、hydration／構成／resume、conversion・初回終端 | 上記B全件、既存detached／RAR cache／Required leaf回帰 |
| **S4：統合受入** | docs／spec／manualの最終整合、必要な全体gate、検証build | S1〜S3の有効証跡を再利用し、統合差分と必須gateを検査 |

各段階で独立レビューを行い、修正済みfindingの影響範囲だけを再検証する。S1単独を今回要求の完成・リリース可能状態とは扱わない。

追加対象には `book_resume_key` 相当の共通module、`book_resume_db`、`rename_key_migration`、Remoteのcontainer/session/ui、descriptorを構築する `startup_ops`、archive conversion continuationを含む。

**全体概算：製品コード1,500〜2,500行、テスト1,800〜3,000行、実装・自動検証8〜13人日。** 独立レビュー・修正往復・承認付き実機検証は別。今回の読み取り専用調査では実行時間の実測はしていない。
