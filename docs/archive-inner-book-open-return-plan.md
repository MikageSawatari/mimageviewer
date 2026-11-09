# 書庫内の本を開く・閉じる (§1.675 + §1.676) 設計

状態: **設計 R4・S1〜S3 実装の独立レビュー ACCEPT、S4 文書整備完了 (2026-11-08)**。master 未マージ・実機確認待ち。設計担当 ClaudeCode Opus 5.5。ソース調査と下書きは Codex GPT-6.1 Sol / high
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
- S1 の scope 検証は、通常モードで「ページを表示」ON・複数ウィンドウ OFF・直下ページありの
  **本候補になった後だけ**行う。OFF／分岐等の非本候補は scope を作らず、従来どおり一覧へ入る。
  `part:one/` 等で `BookScopeKey` を作れない本候補は、本の構成による許可判定ができないため
  `ExplicitList` でその階層の一覧に終え、typed outcome は `List` とする。理由をログへ出し、
  「この内部フォルダは本として開けないため、一覧を表示します」の情報通知を 1 回出す。
  既存の generation/token に対応する Unsupported 状態を記録し、後続 poll の構成警告と重複させない。
  利用者が選んだ事実である Chosen は維持する。他の本・親・先頭の子への fallback はしない。
  S3 の複数窓でも、scope を生成できない候補は独立本として dispatch せず、この既存一覧終端を保つ。
  独立 descriptor を生成した後の park／open 拒否は、その要求の終端として扱う。
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
- 配送: close を受理した時点で帰還先を確定し、既存の `FullscreenCloseContinuation::LiveZipListReturn(plan)`
  に保持する。配送までに refresh 等で記録が消えても、帰還先を再解決しない。
  `AddressBarNav::ZipListReturn(request)` で準備中 recipe と Live／Terminal の配送 ticket を型で運ぶ。
  準備中の recipe は `PreparedLive`／`PreparedTerminal`、take 後の配送は `Live`／`Terminal` と区別する。
  take は唯一の確定 plan を既存 slot の `IssuedZipListReturn(owner)` へ移し、nav には phase と Weak ticket だけを渡す。
  Issued のまま同じ reader が terminal close されても、teardown は owner の保存済み plan から方針を導く。
  二度目の take は owner を保持して `None` を返す。Issued は未配送の pending ではないため、一覧操作を妨げない。
  **同じ context の新 reader 受理（共通 open の既存 None→Some 境界）で、その context の Zip 帰還を終了する。**
  既存 reader 内のページ送りと兄弟 context の reader 受理は終了条件にしない。
  consumer は現在の slot が持つ exact owner と ticket を照合し、一回だけ plan を取り出して検証する。
  新 reader 受理後の旧 nav は Chosen／ツリー／prefix が一致していてもログを出して捨て、何も install しない。
  plan 自体が stale の場合も配送権限は消費する。準備中 recipe は consumer で実行しない。
  配送 nav がすべて破棄された場合は Weak ticket もなくなるため、後の新しい close を妨げない。
  plan は owner から導いた一回限りの値で、
  別の来歴 owner ではない (中身: 帰還する階層の深さと display prefix、読んでいた本の prefix、`rows_above`、
  ツリーの identity)。`transfer_terminal_close_return` は解決済みの帰還先から effect を選び、`ZipListReturn` なら
  `Navigation` のまま保持する (`BookmarkViewState::Opening` で `BookmarkNavigation` に置き換えない・消費しない)。
- 適用の順序 (Live / Terminal / `handle_fullscreen_close_request_immediate` / in-window 静止画の early-return は、
  すべて既存の `apply_fullscreen_close_nav_immediate` の新しい分岐へ集める):
  embedded の root 入力は通常時／page-turn barrier 中ともグリッド keyboard 処理を抑止し、
  close continuation を既存の描画後 dispatcher まで保持する。
  1. plan が現在の main の同じツリー (`Arc` の同一性) と、まだ `Chosen` の階層を指すことを確かめる。
     外れていたら帰還先だけを捨てて理由をログに出す。**Live で owner の fullscreen がまだ開いていれば、
     `FullscreenCloseDestination::CurrentList` として現在の一覧へ閉じる**。外側の親など別の帰還先は再解決しない。
     Terminal は close 済みなので捨てるだけとし、後から開いた reader を閉じない。別 context への配送も捨てる。
  2. 旧 items が有効なうちに、物理一覧へ戻る既存経路と同じ main context 専用 teardown
     (`change_main_context_for_visible_grid_for_destination`) を実行する。帰還先を引数で渡すだけで、
     App 全体の active detached session を閉じる処理は追加しない。linked viewport の後処理も既存の経路に任せる。
     Terminal は close 済みなので teardown しない。`fullscreen_idx` の有無から Live/Terminal を推定しない。
     Terminal capture 前から plan が失効していた場合も、teardown の宛先導出時に同じ currentness 検証を行い、
     `CurrentList` の方針で閉じる。保存した recipe は再計画せず、後段で stale Terminal として破棄する。
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
| refresh (ツリーの作り直し) | 記録を捨てる (全階層 `Unseen`)。受理済みの Live plan が stale なら帰還先だけを捨て、現在の一覧へ閉じる |
| 同じ階層の再 materialize (ソート・ピン) | stack を変えないので維持 |
| 履歴 Back/Forward | `restore_prefix` で作った祖先は `Unseen` |
| 同じ書庫内へのブックマーク移動・Required leaf (`enter` で既存 stack に積む経路を含む) | 直接 open の意味なので、**全階層の記録を捨てる** |
| ★固定 snapshot | 既存の `saved_zip_nav` 退避・復帰に値ごと同行。固定中は帰路を発行しない |
| 独立ビューアの build / fork / promote | **独立 bundle になる側の `zip_nav` と保存中の `saved_zip_nav` の記録を `Unseen` にする** (§1.670 A と同じく、main への帰還権限を独立 bundle に作らない)。main 側の原本は保持 |
| main admission・Remote 取得・終了 | 既存の close return 失効入口 (`retire_close_returns_for_main_admission` 等) がそのまま配送中の plan を捨てる |
| 同じ context の新 reader 受理 | 未配送／配送済みの Zip 帰還だけを終了。take 済み nav の Weak ticket も失効。別 context と既存 reader のページ送りには影響しない |

## 4. 開く処理の接続 (§1.675、通常モード)

- 共通入口は、候補判定 → (main で動く分岐なら) 出発階層の記録 → 現行の★一時解除
  (`maybe_suppress_rating_filter_for_opened_zip_book`) → clone した nav で子へ入る → 一覧なら `ExplicitList`、
  本として開くなら `PageContinuation` で階層を install する。現在の `zip_nav_enter` は一覧移動用 helper として残し、
  既存の他の caller は変えない。戻り値は「非対象」と「拒否」を区別する typed outcome とする。
- install 直後に先頭 idx を選んで開かない。階層 install の後は既存の `ContinuationOwner::IndexOnly`
  (ページ identity の index 準備) が動くので、その完了動作に型付きの値を持たせる:
  `IndexOnlyAfterPrepare::{None, OpenArchiveBook(DeferredFsReopen)}`。新しい App の pending は作らない。
  true close 用の取消 (`sidecar_restore.rs` の `deferred_fullscreen` と Live continuation の取消) にもこの値を含める。
- `DeferredFsTarget` に `CurrentArchiveBook(ArchiveBookTarget)` を足し、leaf 指定 (Required / Preferred) と区別する。
  `ArchiveBookTarget` (§4.5) は、元の綴りを保った `requested_prefix` (tree lookup 用) と、
  **論理 source + `current_raw()`** による `BookScopeKey` (構成照合用、`book_composition_runtime.rs` と同じ) を
  1 つの値として持つ。通常モード・独立ビューアとも同じ型を使い、descriptor → 変換 → 列挙 → 対象 level の解決 →
  IndexOnly → 構成待ち → 表示まで、この値を単一の owner 間で移す (scope から prefix を復元しない)。
- 完了時に scope・context・items 世代が一致することを確かめてから、`CurrentArchiveBook` 専用の解決を行う:
  保存位置 (`book_open_resume` が続きからのとき) を PageIdentity で解決し、**それが要求 context の最終的な可視集合
  (`visible_indices`、★・ファイル種類・構成) に属するときだけ**採る。属さなければ既存の通知を付けて先頭探索へ進む。
  先頭探索は**本の構成順と可視集合の両方を満たす最初の直下ページ**を選ぶ。
  目的は先頭なら `BookTargetPurpose::FirstInBook`、保存位置なら `Explicit`。既存の本の構成待ち・表示許可を通って
  開く (`from_explicit_open = true`、`HistoryTrigger::UserChosen`、resume notice は要求に同行)。
- resume fence: IndexOnly の末尾と、独立ビューアの初回 open が通る Live continuation (ZIP first-display の後) は、
  既存どおり fence を消費する。**どちらも fence を消す前に同じ capture 処理を通し**、fence を通した保存位置
  (PageIdentity) を要求の値へ写し取る。「cutoff 時点で保存なし」も確定結果として写し取る。構成待ちの後はその写しを
  構成適用後の列へ解決し、DB を再照会しない (構成待ちの間に fence を開いたままにしない)。
- **S1 では内部本は先頭固定・保存なし**とする (既存の container resume 照会は `current_folder` を読み、外側 root の位置を
  内部へ適用し得るため、S1 では resume 照会そのものを呼ばない)。S2 で typed な読取と保存を同時に有効にする。
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
- prefixは `ZipNavLevel.raw` を `BookScopeKey::archive` と同じ規則で**検証**するが、**大文字・小文字は元のまま保持**する
  (ZIP ツリーでは `A/` と `a/` が別の階層なので、case-fold すると別の本が同じ行を上書きする)。構成照合用の
  case-fold 済み `BookScopeKey` とは別の値として扱う。区切りの統一 (`\` → `/`、末尾 `/` の扱い) だけを行う。
- prefixは非空。root／collapse済みrootには使用しない。
- ハッシュではなく可逆な符号化とし、rename／deleteのfamily照合で元sourceを取得できる。
- 符号化済み文字列を `PathBuf` に偽装して通常のpath処理へ渡さない。

Windowsの実パス由来の既存キーは、ドライブ除去後のpathまたはUNC等である。新名前空間の `miv:zipbook:v1:` は有効なWindows実パスにならず、root／実フォルダキーと衝突しない。二成分はhexに含まれない `:` で区切るため、sourceとprefixの境界にも曖昧さがない。

単純な `outer.zip/prefix` 連結は採らない。既存pathキーとの区別とfamily処理を保存文字列の偶然に依存させないためである。

#### root互換性

- ZIP root・実フォルダ・PDF等の既存キーは変更しない。
- **root の `LegacyPath` は各経路の出荷済みキーをそのまま維持する。** 変換書庫の root は、PC では backing キー、
  Remote では公開住所由来の logical キー (`remote_ipc/container.rs` の `resolved.logical`、`remote_ipc/ui.rs` の保存) を
  今までどおり使う。PC と Remote で共通化するのは内部本の `ArchiveLevel` だけ。互換試験では PC の backing 行と
  Remote の logical 行を両方 seed する。
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
   **復元拒否時**: 内部本は「通知 + 先頭ページ」とする (PC と同じ)。現行サーバーは拒否を `Grid` にし
   (`remote_ipc/container.rs` の拒否分岐)、クライアントは拒否理由があれば open mode に関係なく一覧へ戻す
   (`crates/remote-web/web/app.js` の resume 処理) ので、サーバー応答・クライアント動作・通知文をまとめて定義する。
   **root の既存契約 (拒否 → Grid) は変えない**。応答の形が変わる場合は [リモート設計](web-remote-plan.md) の IPC 版規則に
   従って版を上げ、同文書を更新する。内部本と root の両方の拒否を試験する。
6. **resume記録と履歴進捗を分離する。** 現行UIは `record_resume` を履歴のpage/count付与にも使っている。内部resumeを有効にしても、外側履歴へ内部のordinalを保存しない。履歴touchの資格・抑制は維持する。`src/remote_ipc/ui.rs:3394`

RemoteAddress／`RecordReadingProgress` のJSON形は変更しないが、`context_address` の意味を表示階層から
要求したraw本scopeへ変えるため、strict IPC を **v73 → v74** に更新する。旧Remoteと新core（逆も同様）は
接続handshakeで拒否する。Webは `X-mIV-Remote-Protocol: 74` を送信し、progress POSTは版欠落・不一致を
HTTP 409 `protocol_version_mismatch` で保存受付前に拒否する。asset tokenの更新案内だけには依存しない。

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

adapterは元のドライブ保持pathを受け取る。既存のDriveStripped文字列から元ドライブを逆算しない。
既存 dispatch は DriveStripped 後に一致するもの (ドライブだけ異なる move / copy) を先に除外し
(`rename_key_migration.rs` の dispatch と製本移行の正規化済み copy / raw move)、新 family adapter はこの**除外の前**の
元の move / copy を受け取る位置に接続する。`C:/Books/A.zip → D:/Books/A.zip` の move・copy を試験する。新キーfamilyの照合はdecodeしたsourceに対して行い、hex文字列の前方一致で代用しない。

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
    ArchiveBook(DeferredFsReopen), // target は CurrentArchiveBook(ArchiveBookTarget) で literal prefix を含む
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
- 保存schemaの互換性と通信の意味互換性は別である。S2はIPC v74を要求し、旧Remoteの接続と
  旧Webのprogress POSTを拒否する。DB downgradeの互換性を通信相手の混在許可へ拡張しない。

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
- `section1675_inner_resume_case_distinct_books`  
  同じ書庫に `A/` と `a/` (内部フォルダ / 入れ子 ZIP)。交互に保存・再 open しても互いの位置を上書きしない。
- `section1675_inner_resume_saved_page_hidden_by_filter`  
  ★一時解除が発動しない条件で「保存ページだけ除外」「全ページ除外」。保存位置を採らず通知付き先頭 / 一覧。
- `section1675_inner_resume_fence_across_composition_wait`  
  IndexOnly (通常モード) と Live continuation (独立ビューアの初回 open) で parameter 化し、保存あり・保存なしの両方で、
  fence の解除 → 構成待ち → 表示の間に保存が入る cutoff。写し取った値を使い、待機後に DB を再照会しない。
- `section1675_inner_resume_remote_refusal_contract`  
  Remote で内部本の復元拒否は通知 + 先頭、root の拒否は従来どおり Grid。
- `section1675_inner_resume_root_rows_pc_and_remote`  
  PC の backing root 行と Remote の logical root 行を両方 seed し、どちらも従来どおり読まれ、内部行と混ざらない。
- `section1675_remote_collapsed_root_progress_keeps_root_row`
  `cover.png`＋`A/01.jpg`＋`A/02.jpg`、PNG除外でroot表示がAへcollapseしてもFile要求の進捗はroot行だけを更新する。
- `protocol_v74_rejects_pre_raw_book_scope_reading_progress_clients`／
  `reading_progress_refuses_old_web_protocol_before_write_admission`
  IPC v73を拒否し、Webの版欠落・v73・不正値をHTTP ownerで拒否。v74は通常validationへ進み、scriptの版定数も照合する。
- `section1675_inner_resume_cross_drive_maintenance`  
  `C:/Books/A.zip → D:/Books/A.zip` の move・copy で内部行が正しく移る / 複製される。
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
- `section1675_multiwindow_scope_literal_prefix`  
  大文字を含む prefix で、非同期列挙と prepared 同期経路の双方が literal prefix で対象 level を解決する。
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
- 独立設計レビュー R2 (`7fa8fc6f1`): REVISE。R1 の指摘は解消済みと確認。P2 5 件 (読書位置キーの大文字小文字保持 /
  root キーは PC backing・Remote logical の各既存キーを維持 / literal prefix を含む `ArchiveBookTarget` を単一 owner で移す /
  保存位置も可視集合と照合 / Remote の内部本の復元拒否契約) と P3 2 件 (ドライブ違いの move・copy の接続位置 /
  S1 は先頭固定) と未決 1 件 (構成待ちをまたぐ resume fence) を R3 で反映した。detached 部分は構造的修正であることに同意。
- 独立設計レビュー R3 (`72fcc9a9a`): REVISE。R2 の全項目は解消と確認。P2 1 件 (resume fence の写し取りを独立ビューアの
  Live continuation にも適用し、「保存なし」も確定結果として持つ) を R4 で反映した。追加の利用者判断は不要。
  detached 部分は構造的修正であることに引き続き同意。
- 独立設計レビュー R4 (`69570a9c3`): **ACCEPT** (P1/P2/P3 なし)。detached 部分を「症状パッチではなく構造的修正」とすることに同意。
- 設計担当 ClaudeCode も同じ主張に合意 (2026-11-08)。実装後の完了レビューで改めて合意を取り、detached 計画 §11 に記録する。

### S1 実装記録（2026-11-08、未コミット）

- 共通 `activate_main_zipdir`、`PageContinuation` → `IndexOnlyAfterPrepare::OpenArchiveBook` →
  `CurrentArchiveBook` の継続、構成順と可視集合の先頭探索を接続した。S1 では container resume を照会せず、
  内部位置の保存も有効化しない。複数窓・★固定は一覧移動のまま。
- `ZipNavLevel` の Chosen と同一ツリーの帰還 plan、通常／即時／Terminal close の共通 consumer、
  旧 items の teardown → pop → ExplicitList → 選択 → anchor/reveal の順序を接続した。
  bookmark／Required jump、独立 build／fork／promote の失効も、退避中 nav を含めて追加した。
- teardown は同期引数 `FullscreenCloseDestination::{CurrentList, ChosenZipList}` で帰還先を区別する。
  Live は検証済み plan の consumer から、Terminal は既存の Navigation effect から導出する。
  `ChosenZipList` のみ外側の Bookmark owner を保持し、Backspace 等は従来どおり破棄する。
  App の保存状態・pending は追加しない。独立 reviewer はこの修正にも新規 P1/P2 なしと確認した。
- 独立実装レビューの初回評価は **REVISE**。全体方針は構造的修正と確認したが、以下の実コードとの
  前提差分があり、初回引継ぎでは未完了とした。追補の設計担当判断で、(1) は別件、(2) は §2.1 の境界に確定した。
  1. Everything は**検索前の folder/origin がない場合**、`folder_nav_current_entry()` がなく、既存の
     `zip_nav_show_current_level` が階層 install 前に戻る。master 既存の不具合と確認し、S1 では修正しない（下記別件報告）。
     初回レビューの「Everything は常に install できない」という断定は撤回した。origin がある通常条件では
     open・内部帰還が成功する。Backspace は nav を返さず既存 Parent 処理をその場で実行するため、
     テストは `None` の後の再投影完了・Everything root・選択 anchor を検証する。
  2. `part:one/` 等は従来の ZIP 一覧で閲覧可能だが `BookScopeKey` が拒否する。構成非対応と
     一覧移動の可否を分け、本候補のみ scope 検証し、非対応候補は明示的な一覧終端＋情報通知とする実装へ修正した。
- headless の共通入力・構成・取消・帰還・記録寿命・linked／独立／兄弟不変の回帰を追加。
  初回 42 件は 36 成功・6 失敗。上記 prefix、origin 有り Everything の戻り値期待、空構成の同期完了、
  fork/build の snapshot 所有に関する fixture、Bookmark teardown を切り分け、期待を弱めず修正した。
  teardown 修正前の指定既存回帰は 344/344 成功。修正後の統合 387 件は 383 成功・4 失敗。
  修正途中の S1 43 件は 40 成功・3 失敗（exit 100）。別 archive 要求後に旧返信を保持したまま
  新 adoption 完了を待つ fixture の循環を直し、S1 の最終 43 件は **41 成功・2 失敗**（exit 100）。
  初回引継ぎ時の失敗は `section1675_unsafe_structural_prefix_keeps_existing_list_navigation` と
  `section1676_everything_without_origin_adopts_internal_level`。その時点では削除・ignore・期待の引下げはしていない。
  修正後の既存回帰 344 件は全成功（統合実行の内訳）。その後は新規 fixture のみを直しており、同じ製品コードの成功証跡を再利用する。
  製品起動は行わない。既存テストの期待値は変更していない。

初回引継ぎまでの検証コマンド（`MSBUILDDISABLENODEREUSE=1`、`CARGO_BUILD_JOBS=1`、`TEMP`/`TMP` は worktree の
`target/codex-tmp`。初回 native 準備で VLM worker の source hash を既存スクリプトで検証して実体復元し、
FFmpeg DLL を `target/debug/deps` へ実体コピーした）:

| コマンド／filter | 初回引継ぎ時の結果 |
| --- | --- |
| `cargo check -p mimageviewer --bin mimageviewer-core` | exit 0。初回 native 準備と新 variant の match 漏れを修正後、最終 teardown 変更後も成功 |
| `cargo nextest run -p mimageviewer --lib -E 'test(/section167[56]/)'` | 43 件、41 成功（2 slow）・上記 2 失敗、exit 100 |
| `cargo nextest run -p mimageviewer --lib -E 'test(/zip_nav_back_selects_book_cell_left_from\|zip_nav_ctrl_updown_\|section1335_\|section1328_\|collection_zip_details_auto_close\|everything_close_return\|section1678_\|close_return_tests\|index_only\|visibility_current_listing\|visibility_live_page_index/)'` | 単独 344/344、exit 0。最終製品コードでも下記統合実行内の 344 件が成功 |
| 上記二つの正規表現を `\|` で結合した focused 実行 | 387 件、383 成功・4 失敗、exit 100。その後直した新規 fixture 2 件は S1 再実行で成功 |

初回 nextest のテストコンパイルは import／型の取り違え 2 件で実行前に失敗（コマンド exit 1、Cargo 子プロセス 101）。
修正後に上記を実行した。全 lib／workspace gate・検証 build・実機確認は実施せず、S4 に残す。
初回引継ぎ時は merge／リリース可能とは扱わず、上記 2 条件の設計判断と再レビューを必要とした。
最終 `cargo fmt`（引数なし）、`python scripts/check_ui_glyphs.py`、
`git -c core.safecrlf=false diff --check` はすべて exit 0。グリフ違反は 0 件。

#### S1 追補（設計担当判断を反映）

- scope 検証を本候補の後へ移した。非対応候補は `ExplicitList`／`List`／Chosen 維持、ログ＋情報通知 1 回。
  既存 Unsupported の状態公開を共通 helper へ抽出し、通常の一覧 projection は master の Warning 契約を維持する。
  新しい App field・通知用 latch・pending は作らない。
- `section1675_unsafe_structural_prefix_keeps_existing_list_navigation` は ON/OFF × 複数窓 × 分岐の
  8 条件で、一覧・Chosen・非 fullscreen・候補のときだけ情報通知 1 回・後続の重複警告なしを検証する。
- 設計担当の指示により、origin 無しの desired-behavior variant は S1 のテストから外した。
  通常 origin 有りの Everything open／close／Backspace 回帰は維持し、既存挙動の調査テストを別 namespace へ移した。
- 追補の独立レビューは **ACCEPT（限定差分、新規 P1/P2 なし）**。症状パッチではなく構造修正という評価を維持。
  追補の自動検証は実装担当が下記を実行した。製品起動・commit は行っていない。

追補の検証（上記と同じ環境変数・native 依存物を使用）:

| コマンド／filter | 結果 |
| --- | --- |
| `cargo check -p mimageviewer --bin mimageviewer-core` | 成功、exit 0 |
| `cargo nextest run -p mimageviewer --lib -E 'test(/section167[56]/)'` | **42/42 成功、失敗 0、exit 0** |
| 上記の指定既存回帰 filter（変更なし） | **345/345 成功、失敗 0、exit 0**。既存 344 件＋下記の master 挙動調査 1 件。nextest の leaky 表示 4 件あり |
| `cargo nextest run -p mimageviewer --lib -E 'test(/g3_unsupported_source_notifies_once\|g3_unsafe_and_ambiguous_actual_archives\|unsafe_or_unproven_archive_source_does_not_infer_scope/)'` | **3/3 成功、失敗 0、exit 0** |

既存テストの期待値は変更せず、S1 外の origin 無し desired-behavior variant の分離だけを設計担当の指示どおり実施した。
通常 origin 有りの Everything 帰還、非対応 prefix の OFF/ON 一覧移動・通知、既存 Unsupported 契約は成功。
追補の最終 `cargo fmt` は exit 0（Rust ファイルの追加変更 0）、グリフ検査は違反 0・exit 0、diff check も exit 0。
追補の差分・検証は完了し、設計担当の検収・commit 待ち。S1 単独を S2/S3 を含む機能全体やリリースの完成とは扱わない。

#### 別件報告: Everything の検索前 origin がない場合の ZIP 階層採用（S1 対象外）

- 別件の追跡先は[バックログ §1.738](next-release-backlog.md)。以下は S1 調査時の記録。
- master `bec01bf11` の `zip_nav_enter`（`src/app.rs:40520`）、`zip_nav_show_current_level`（`:40190`）、
  `folder_nav_current_entry`（`:25237`）は S1 worktree と同一の関数本体であることを `git show master:src/app.rs` で確認した。
- Search の owner は `TopLevelGridView` の `return_to` を物理履歴 origin として使う。検索前 origin がないと、
  `current_top_level_restore_snapshot` は Search surface から `None` を返す（master `:26465`／Search 分岐 `:26512`）。
  `folder_nav_current_target`（master `:26269`）→ `folder_nav_current_entry` も `None` となる。
- `zip_nav_enter` は先に nav を A へ進めるが、main の `zip_nav_show_current_level` は履歴採用に必要な source entry を
  取得できず master `:40207`〜`:40208` で return する。items・items generation・address の install は実行されず、
  **nav は A、表示は元の A/B 一覧**という不整合になる。「ページを表示」OFF の既存経路でも成立し、S1 が作った不具合ではない。
- 調査テストは実装レビュー R1 の指示により、望ましい「表示と nav が一致する」契約
  `everything_close_return::investigation_master_everything_no_origin_zipdir_rows_match_nav` へ変更した。
  `#[ignore = "pre-existing master bug: Everything without search origin; separate backlog item"]` とし、
  不具合のある挙動を正常な契約にはしない。根因コメントを残し、別 backlog で修正したときに有効化する。
  今回、Everything の製品コードは変更していない。

### S1 実装レビュー R1 修正（`875166c1b` に対する追補）

- レビュー要約の P2／P3 3 件はすべてコードで確認した（指摘本文ファイルは worktree に見つからず、依頼文の要約を使用）。
- P2: 内部一覧の consumer だけが App 全体の active detached session を終了していた。
  物理一覧へ戻る経路の main context 専用 teardown を共用し、帰還先の Bookmark 方針だけ同期引数で渡す。
  これで main の Live 配送までに別 reader が active になっても、その session／runtime／viewport／bundle に触れない。
- 設計担当の追加判断: close 受理時に `LiveZipListReturn(plan)` へ確定し、配送時に新しい帰還先を作らない。
  nav の payload は `ZipListReturnRequest::{Live, Terminal}` とし、stale Live は現在の一覧への close を完了する。
  stale Terminal は破棄するだけで、後発 reader を閉じない。新しい App field／pending は追加しない。
- P3-1: scroll 回帰を `render_grid` の実 headless frame へ置換した。details、6→8 列、最終行について、
  選択セル・描画 geometry による offset・anchor と reveal の消費を検証する。in-window は pending 無しから
  実 App update に Esc を渡し、embedded 描画内で生じた close を描画後に配送する。
- stale Live の実 update 試験により、既存の embedded root routing が予約を `handle_keyboard` に先取りさせ、
  embedded early-return で失う経路も確認した。embedded は通常時／page-turn barrier 中とも入力を所有し続け、
  予約は既存の描画後 dispatcher に残す。同じ continuation の消費箇所を揃える構造修正であると独立 reviewer と合意した。
- P3-2: 上記の origin 無し Everything 調査を ignored な期待契約へ変更した。正常な S1／Everything 回帰は維持する。
- 既存テストの期待変更: `section1676_real_refresh_clears_choice_and_rejects_old_plan` を
  `section1676_real_refresh_stale_live_plan_closes_to_current_list` へ変更し、stale Live でも閉じる採択仕様を検証する。
  `section1676_choice_independent_close_preserves_main_live_terminal_and_sibling_queues` は main の Live variant が
  確定 plan を持つことを検証する。調査テストの改名・ignore は上記のとおり。弱化・削除は行わない。
  `embedded_container_close_request_allows_parent_return_to_be_consumed` は、グリッドへの委譲を期待していた箇所を
  embedded owner と pending の保持へ変更する。帰還そのものの既存 assertion は維持する。
- 独立 Sol/xhigh reviewer は本追補を **ACCEPT（追加 P1/P2 なし）**、所有境界を正す構造修正と評価した。
  自動検証結果は下記に追記する。HWND／rect capture、viewport identity、placement storage、timer は変更しない。
- 途中版 S1 は 46 件中 43 成功・3 失敗（exit 100）。うち embedded の予約先取りによる 2 件は、
  コンパイル中に上記の入力所有修正と実 Esc 試験へ更新した。残る兄弟 fixture は一覧用の設定 OFF のまま
  independent descriptor を呼び、通常 fullscreen の表示方針を選んでいた。実 caller と同じ独立表示設定を
  main close 受付後に設定し、descriptor 前に deferred viewport を有効化した。実 sibling の fullscreen／
  DetachedWindow／独立 owner の assertion を追加し、session／runtime／viewport／items の不変条件は維持した。
  最終 core check は入力所有修正後も成功（exit 0）。途中版のテスト結果を最終成功証跡には使わない。
- 追加の限定レビューで、失効済み frozen Live を Terminal に移した後の teardown が
  `ChosenZipList` 方針を使い続ける契約漏れも確認した。宛先導出を `App::fullscreen_close_destination` に集約し、
  配送と同じ plan currentness helper を参照する。失効済みなら `CurrentList` として Bookmark owner を破棄し、
  Terminal recipe 自体は変えず後段で捨てる。新しい状態・再計画は追加しない。
  独立 reviewer は限定差分を ACCEPT、新規 P1/P2 なしと確認した。以下の 47 件等はこの追加補修前の有効証跡であり、
  補修後の指定検証を別途記録する。

レビュー R1 修正の追加 Terminal 補修前の検証（`MSBUILDDISABLENODEREUSE=1`、`CARGO_BUILD_JOBS=1`、
`TEMP`／`TMP` は worktree の `target/codex-tmp`、既存 native 依存物を再利用）:

| コマンド／filter | 結果 |
| --- | --- |
| `cargo check -p mimageviewer --bin mimageviewer-core` | 最終製品コードで成功、exit 0 |
| `cargo nextest run -p mimageviewer --lib -E 'test(/section167[56]/)'` | **47/47 成功、失敗 0、exit 0**。新規の in-window 実 Esc、barrier、stale Live 実 update、兄弟 session／runtime／viewport 不変を含む |
| 上記の指定既存回帰 filter（変更なし） | **344/344 成功、失敗 0、exit 0**。origin 無し Everything の既知不具合契約 1 件は指定の理由で ignore |
| `cargo nextest run -p mimageviewer --lib -E 'test(/detached_binding_identity\|detached_window_manager::tests\|main_context_change\|main_context_preserve_hands_off_active_viewport_to_passive\|main_non_detached_open_does_not_close_active_detached_context_session\|active_detached_session_owns_visible_viewport_until_closed\|detached_viewer_close_ends_session_but_keeps_mode_enabled\|embedded_container_close_request_allows_parent_return_to_be_consumed\|page_turn_.*barrier/)'` | **48/48 成功、失敗 0、exit 0** |
| `cargo fmt`／`cargo fmt --check`／`git -c core.safecrlf=false diff --check` | すべて exit 0 |
| `python scripts/check_ui_glyphs.py` | 違反 0、exit 0 |

追加 Terminal 補修後の最終検証（同じ環境変数、filter、native 依存物）:

| コマンド／filter | 結果 |
| --- | --- |
| `cargo check -p mimageviewer --bin mimageviewer-core` | 成功、exit 0 |
| `cargo nextest run -p mimageviewer --lib -E 'test(/section167[56]/)'` | **48/48 成功、失敗 0、exit 0**。失効済み frozen Live→Terminal capture→CurrentList teardown の回帰を追加 |
| 上記の指定既存回帰 filter（変更なし） | **344/344 成功、失敗 0、exit 0**。origin 無し Everything の既知不具合契約 1 件は ignore |
| 上記の追加 detached／main context／barrier filter（変更なし） | **48/48 成功、失敗 0、exit 0** |
| `cargo fmt`／`cargo fmt --check`／`git -c core.safecrlf=false diff --check` | すべて exit 0 |
| `python scripts/check_ui_glyphs.py` | 違反 0、exit 0 |

確認用の `powershell -NoProfile -ExecutionPolicy Bypass -File .\scripts\build-dev.ps1 -PreserveRuntime` は、
core と全 companion のコンパイル・FFmpeg／CRT 配置後に、worktree の `vendor/miv-sound` が欠けており exit 1。
`C:\home\mimageviewer_vendor_backup\miv-sound-0.15.0-miv.7\artifact` を現在の追跡済み manifest で全ファイル照合し、
復元先の workspace 内 containment／reparse point 不在を確認して実体コピーした。
コピー後も manifest・VERSION・PE・通知の検査に成功。新しい候補の承認や追跡ファイルの更新は行っていない。
復元後、同じ `build-dev.ps1 -PreserveRuntime` は **exit 0** で完了した。
core・remote・EPUB PDF worker・MCP adapter・music-video editor と必要 DLL／承認済み bundle を
`target/dev-runtime` に配置し、最終 PE 検査も runtime 4 本／PE 5 本で成功。
core は normal feature set（portable 無効）であり、製品起動・通常 profile のデータ操作・全 lib／workspace suite は行っていない。
S1 の追補差分は未コミット。S2／S3 の内部 resume・複数窓の内部本 activation は未変更。

### S1 実装レビュー R2 修正（`75f796a6e` に対する追補）

- P2 をコードで確認した。Terminal の take は nav を slot 外へ渡して origin を捨てるため、
  同じ A 階層のページを再 open しても tree／prefix／Chosen の検証だけでは旧帰還を識別できなかった。
- reader 受理の共通 owner `open_fullscreen_with_metadata_boundary` の既存 None→Some commit 境界で、
  **その context の Zip 帰還だけを終了する**。他 context、通常のページ送り、拒否／defer 中の要求には適用しない。
  `input_seq` は App 共通の perf 相関値、items generation は同階層の再 open で不変なので流用しない。
- 既存 `FullscreenCloseContinuation` に排他的な `IssuedZipListReturn(owner)` を加えた。
  strong identity と唯一の確定 plan は slot だけが保持し、nav は phase と Weak ticket を持つ。
  take 済み Live／Terminal を同じ規則で扱う。Issued 中の terminal teardown も保存済み plan を使う。
  同じ slot の再 take は owner を保持して None。新 reader・main admission・Remote／終了の clear は owner を捨てる。
  consumer は exact owner を一回消費してから plan を検証し、stale plan も権限を終端する。
  nav がすべて drop された場合は outstanding ticket がなくなり、後の新しい close を受理できる。
  ticket の実生存数を使い、時刻・世代・fullscreen の有無から推測しない。
  新しい App field／bool／Option／pending、viewport／HWND／配置／timer の変更はない。
- Chosen を維持した実 Enter／実 render_grid ダブルクリックで、Terminal take 後に同じ A の別ページを開き、
  旧 nav が items／fullscreen index／tree／prefix／Chosen／generation を変更しない回帰を追加した。
  take 前の reader 受理、二度目の take、拒否 open、二重配送、兄弟 reader 受理も検証する。
- 既存テストの変更は transport の API 更新だけ。Terminal の tuple pattern を issued struct pattern へ変更し、
  `section1676_real_refresh_stale_live_plan_closes_to_current_list` の synthetic Live は既存 slot に準備して take する。
  最終的な close／rows／階層／選択の期待値は変えない。テストの削除・弱化は行わない。
- 独立 Sol/xhigh reviewer は上記 owner 設計を ACCEPT。検証の実行担当は実装担当である。

R2 検証（`MSBUILDDISABLENODEREUSE=1`、`CARGO_BUILD_JOBS=1`、`TEMP`／`TMP` は
worktree の `target/codex-tmp`。既存 native 依存物を再利用。製品起動と全 lib／workspace suite は行わない）:

| コマンド／filter | 結果 |
| --- | --- |
| `cargo check -p mimageviewer --bin mimageviewer-core` | 成功、exit 0 |
| `cargo nextest run -p mimageviewer --lib -E 'test(/section167[56]/)'` | **57/57 成功、失敗 0、exit 0**。追加9件を含む |
| R1 と同じ指定既存回帰 filter | **344/344 成功、失敗 0、exit 0**。既知の origin 無し Everything 1件は ignore |
| R1 と同じ detached／main context／barrier filter | **48/48 成功、失敗 0、exit 0** |
| `powershell -NoProfile -ExecutionPolicy Bypass -File .\scripts\build-dev.ps1 -PreserveRuntime` | 成功、exit 0。core・全 companion・DLL／bundle 配置と最終 PE 5本／CRT 4本の検査まで完了。製品起動なし |
| `cargo fmt`／`cargo fmt --check`／`git -c core.safecrlf=false diff --check` | すべて exit 0 |
| `python scripts/check_ui_glyphs.py` | 違反0、exit 0 |

#### 別件報告: 他 AddressBarNav の Terminal にもある再 open 後の旧配送（今回は修正しない）

- 別件の追跡先は[バックログ §1.739](next-release-backlog.md)。以下の行番号は R2 開始時点。
- R2 開始時点の `src/app/close_return.rs:333` 以降は Terminal effect から nav を返して origin を捨てる。
  `src/app.rs:71343` の reader 受理は close return を終了せず、main list admission の終了入口も通らない。
- 例えば root ページから直接読んでいた ZIP を Terminal close → teardown →親フォルダ Direct nav を take →
  同じ context のページを再 open →旧 Direct を適用すると、`src/app.rs:24887` の Direct 分岐は受付寿命を照合せず
  load を開始し、後発 reader を置換する。S1 の ZipListReturn より前からある同型の穴と確認した。
- 実装前に設計担当へこの範囲外の穴を報告した。今回は Zip の queued／issued 帰還だけを終了させ、
  Direct 等の reader 受付・take・consumer の契約は変更しない。横展開は別の設計判断とする。

R2 の source と追加9テストは独立 reviewer が ACCEPT（新規 P1/P2 なし）。指定検証は上記のとおりすべて成功。
変更は未コミットで設計担当の検収・commit 待ち。S2／S3 の機能と永続スキーマは変更していない。

### S2 実装記録（2026-11-08、自動検証完了・未コミット）

- `BookResumeKey` の追加名前空間と既存 root adapter を実装。テーブル・schema version は変更せず、
  既存 PC backing／Remote logical root のキーを維持する。内部 raw prefix は case を保持する。
- 既存 deferred request の resume 指示を排他的な `First / AwaitingFence / Captured / Refused` に変更。
  IndexOnly と Live の双方で、構成待ちの前に cutoff の saved／unsaved を request へ固定する。
  保存ページだけが `Explicit`、先頭探索は `FirstInBook`。明示 Required leaf の失敗契約は維持する。
- 実コードの fence 開始条件は case-fold した構成 scope の変化だけだったため、`A/` → `a/` でも
  本 open request の受理が既存 restore owner に読取を依頼するようにした。App pending は増設しない。
- 保存・meter・editor・Remote と managed rename／move／copy／delete の family adapter を同時に実装。
  新キーと Archive PageIdentity の書換えは同一 transaction、cross-drive の除外前に元 source を渡す。
  読書履歴・metadata transfer・cache 削除の寿命は変更しない。
- Remote の応答は既存 `resume_refusal` と `open_mode` で内部「通知＋Page」と root「通知＋Grid」を区別する。
  JSON の形は維持するが、context の意味変更を伴うため IPC v74 と Web progress の版照合を要求する。
  検証済み progress と typed key は core 内部 queue の値とする。
  Web の進捗送信・batch identity は `requestedAddress` を使い、wrapper collapse 後の表示用 address と
  保存用の raw book scope を分離する。同じ表示階層でも別の本の進捗は batch を共用しない。
- S3 の detached BookScope target、HWND／rect 捕捉、viewport identity、placement、timer は変更しない。
- 実行順によって構成全非表示の DFS が閉じてしまう回帰を確認し、構成採用前に request を既存
  `AwaitingProjection` owner へ移す順序に修正した。採用末尾は、現在の内部本 request を Activation／DFS
  とも保全する。保存ページの拒否も同 owner に戻し、採用途中で後発 reader を同期 open しない。
  root／Required／Preferred の終了契約は維持する。worker hold を使った全非表示 DFS と、
  Activation／DFS × Checking／Ready の保存拒否再投影を決定的な回帰として追加した。
  新しい App field・待ち時間・retry・repaint は追加せず、独立 source review で修正を ACCEPT と確認した。

既存テストの仕様上の期待変更：

- `section1675_s1_ignores_outer_resume_and_does_not_record_inner` は
  `section1675_inner_resume_keeps_outer_position_separate` へ変更。内部保存を確認し、外側行の不変を維持する。
- `record_book_resume_skipped_inside_zip_book_level` は
  `record_book_resume_uses_separate_key_inside_zip_book_level` へ変更。内部行を保存する S2 の仕様に合わせる。
- `remote_nested_page_without_resume_preserves_previous_root_meter` は
  `remote_inner_progress_saves_its_key_without_overwriting_root_resume_or_history_position` へ変更。
  内部行の保存と外側 meter／履歴資格の不変を同時に検証する。
- Web の `container opening resumes the matching page and otherwise falls back safely` は、拒否理由だけで
  Grid に強制せず server の open mode に従う期待へ変更。root の Grid 期待は維持する。
- `all_hidden_zip_level_keeps_dfs_continuation_for_manual_and_slideshow` は、移動先の index owner を
  破棄して即時 `Checking` を注入する旧 fixture を、実際の Index／FIFO／構成／permission 順へ更新。
  pause 後に書込を admit して cutoff を進め、完了済み snapshot の正当な即時応答では待機を仮定しない。
  未資格時の timer／anchor／進捗不変と、最終 reader／source／選択／slideshow の assertion は維持する。
- これ以外の既存 fixture は typed key／resume intent の型合わせと、検証済み progress queue への接続。
  既存の保存先・位置の意味は変更せず、テストを削除・ignore・弱体化していない。

#### S2 最終検証

PowerShell の共通環境は以下。TEMP／TMP は worktree 内の空ディレクトリを使用した。
全 lib／workspace suite と製品起動は実施していない。

```powershell
$env:MSBUILDDISABLENODEREUSE='1'
$env:CARGO_BUILD_JOBS='1'
$env:TEMP=(Join-Path (Get-Location) 'target/codex-tmp')
$env:TMP=$env:TEMP
```

| コマンド | 結果 | exit code |
|---|---|---|
| `cargo check -p mimageviewer --bin mimageviewer-core` | 成功 | 0 |
| `cargo nextest run -p mimageviewer --lib -E 'test(/section167[56]/)'` | 92 passed、0 failed | 0 |
| 下記の既存回帰 filter | 1,235 passed、0 failed | 0 |
| `cargo test -p mimageviewer-remote` | 142 passed、0 failed、既存の1 ignored | 0 |
| `node --experimental-test-isolation=none --test crates/remote-web/web/*.test.mjs` | 540 passed、0 failed | 0 |
| `cargo fmt` / `cargo fmt --check` | 成功 | 0 |
| `python scripts/check_ui_glyphs.py` | 危険 glyph 0 | 0 |
| `git -c core.safecrlf=false diff --check` | 成功 | 0 |
| `powershell -NoProfile -ExecutionPolicy Bypass -File .\scripts\build-dev.ps1 -PreserveRuntime` | core と companion の build／配置成功、起動なし | 0 |

```powershell
cargo nextest run -p mimageviewer --lib -E 'test(/zip_nav_back_selects_book_cell_left_from|zip_nav_ctrl_updown_|section1335_|section1328_|collection_zip_details_auto_close|everything_close_return|section1678_|close_return_tests|index_only|visibility_current_listing|visibility_live_page_index|book_resume|record_book_resume|rename_key_migration|remote_ipc::|spread_shift_owner|spread_shift_composition|book_composition_editor::|book_composition_permission|book_composition_runtime_tests|composition_transfer_folder_owner_and_document_number_round_trip|book_append_producer_tests::section1373/)'
```

Web の通常 `node --test` は sandbox の子プロセス spawn が EPERM となるため、同じ test files を
process isolation 無効で実行した。最終ログは `target/s2-core-final.log`、`target/s2-s1s2-verified.log`、
`target/s2-regressions-verified.log`、`target/s2-remote-verified.log`、`target/s2-web-verified.log`。
途中の fixture／compile 失敗と構成採用順の不具合は修正し、上記の確定 source で再検証した。
追加の schema migration、既存 root 行の rekey はない。IPC version はレビュー R1 で v74 へ更新する。独立 source review は
最終修正を ACCEPT（残存 P1/P2 なし）。設計担当の検収・commit は未実施。

#### S2 変更ファイル

- 共通 key／保存／maintenance：`src/book_resume_key.rs`（新規）、`src/book_resume_db.rs`、
  `src/rename_key_migration.rs`、`src/metadata_transfer.rs`、`src/lib.rs`。
- 本 open／fence／構成／meter：`src/app.rs`、`src/app/archive_inner_book.rs`、
  `src/app/sidecar_restore.rs`、`src/app/spread_shift_owner.rs`、`src/app/book_composition_editor.rs`、
  `src/app/book_composition_permission.rs`、`src/app/book_composition_runtime.rs`、`src/app/book_resume_meter.rs`。
- Remote：`src/remote_ipc/container.rs`、`session.rs`、`pipe.rs`、`ui.rs`、
  `crates/remote-web/web/app.js`、`app-runtime.test.mjs`。
- typed resume intent への既存 producer の型合わせ：`src/app/collection_navigation.rs`、
  `src/app/smart_folder.rs`、`src/app/snapshot_ops.rs`、`src/filename_stack_ui.rs`、`src/ui_dialogs/epub_convert.rs`。
- テスト：`src/app/archive_inner_book_s2.rs`（新規）、`src/app/tests/archive_inner_book_s1.rs`、
  `src/app/book_append_producer_tests.rs`、`src/app/book_composition_editor/{resume_tests,tests}.rs`、
  `src/app/book_composition_permission_tests.rs`、`src/app/book_composition_revision_tests.rs`、
  `src/app/book_composition_runtime_tests.rs`、`src/app/book_resume_meter_tests.rs`、
  `src/app/list_order_virtual_tests.rs`、`src/app/rar_archive_cache_tests.rs`、`src/app/rating_membership_tests.rs`、
  `src/app/similar_navigation_tests.rs`、`src/app/tests.rs`、`src/app/tests/startup_notices.rs`、
  `src/ui_dialogs/archive_convert/rar_cache_tests.rs`、`src/ui_fullscreen/continuous_quality_tests.rs`。
- 文書：本書、`docs/virtual-folders.md`、`docs/nested-zip-tree-plan.md`、
  `docs/web-remote-plan.md`、`docs/detached-rework-plan.md`。

### S2 実装レビュー R1（2026-11-08）

- S2本体は `e7804a2ea` にcommit済み。本節のR1修正は未コミットで設計担当へ引き渡す。
- P2を確認。v73は旧Remoteを受け入れ、旧Webのcollapse後の `ZipDirectory` を内部本として保存してしまう。
  `context_address` は形が同じでも意味が変わっており、版変更不要とした§4.3／§6／本節の判断を訂正した。
- strict IPCをv74へ更新し、v73の拒否をhandshakeテストに追加。既存asset tokenは内容hashで更新されるが、
  storage利用不可等では通知だけで旧scriptが継続するため、Webのprogress POSTに版を付ける。
  欠落／旧版／不正値はHTTP ownerで409＋通知理由を返し、coreへのwrite受付に到達させない。
- `cover.png` と `A/01.jpg`・`A/02.jpg`、PNG除外の実ZIPで、root要求→A表示へのcollapseと
  root行更新・A内部行未作成を検証。実Web producerもraw File要求と版headerを検証する。
- DB schema／既存row／保存先resolverは変更しない。S3の範囲とdetached/native経路も変更しない。

R1最終検証（環境変数・既存回帰filterは上記S2最終検証と同じ。製品起動なし）：

| コマンド | 結果 | exit code |
|---|---|---|
| `cargo check -p mimageviewer --bin mimageviewer-core` | 成功 | 0 |
| `cargo nextest run -p mimageviewer --lib -E 'test(/section167[56]/)'` | 93 passed、0 failed | 0 |
| 上記S2既存回帰filter | 1,236 passed、0 failed（1 slow・2 leaky） | 0 |
| `cargo test -p mimageviewer-remote` | 143 passed、0 failed、既存1 ignored | 0 |
| `cargo test -p mimageviewer-ipc` | 69 passed、0 failed、doc test 0 | 0 |
| `node --experimental-test-isolation=none --test crates/remote-web/web/*.test.mjs` | 540 passed、0 failed | 0 |
| `cargo fmt` / `cargo fmt --check` | 成功 | 0 |
| `python scripts/check_ui_glyphs.py` | 危険glyph 0 | 0 |
| `git -c core.safecrlf=false diff --check` | 成功 | 0 |

最終ログ：`target/s2-r1-core.log`、`target/s2-r1-sections.log`、`target/s2-r1-regressions.log`、
`target/s2-r1-remote.log`、`target/s2-r1-ipc.log`、`target/s2-r1-web.log`。
追加HTTP fixtureの初期compile／wire構築の誤りは修正済み。既存テストの期待変更は版数の固定assertion
（73→74）だけで、既存Webのraw-scopeテストには版headerのassertionを追加した。削除・ignore追加はない。
`powershell -NoProfile -ExecutionPolicy Bypass -File .\scripts\build-dev.ps1 -PreserveRuntime` も成功
（exit 0）。v74のcore／Remoteとcompanionを `target/dev-runtime` に配置し、VCRT/PEチェックも成功
（runtime=4、pe=5）。製品は起動していない。

### S3 実装記録（2026-11-08、実装・自動検証完了・未コミット）

- 三入力を汎用detached router／一覧出発記録より前の共通activationへ集め、本候補は既存plan実行へ渡す。
  mainの階層・items・選択・位置・facet・★抑制・Chosenを変更しない。分岐／★固定は既存一覧移動。
  scope非対応名は独立本の要求を作らず、S1の承認済み境界どおりChosen＋ExplicitList＋Info通知1回を保つ。
- `DetachedZipTarget::{RootBook, BookScope, RequiredPage}` をdescriptor・変換ownerへ接続。
  literal prefixを列挙完了後、初回materialize前に解決し、raw scope一致を検証する。root／Required leafの意味は維持する。
- `ZipEnumerationPresentation::ArchiveBook` が要求を列挙中に所有し、installで
  `FirstDisplayAction::ArchiveBook` へ移す。hydrationのLive fenceがsaved／unsavedを捕捉し、既存permissionへ渡す。
  root用auto-open予約・外部deferred slotとの二重所有は作らない。初回取消は同leaseのsidecar要求も消費する。
- `ArchiveBookOpenOutcome` で初回の結果を区別。消失・scope変更・表示ページなし・失敗は通知＋既存初回終端、
  staleは破棄し後発readerを閉じない。scope workerのspawn失敗はログ＋通知で終え、UI同期列挙へ回復しない。
- source/backingは最初のinstall前に渡す。aliasを持つscopeとdirect RARは実productionのarchive ownerへ渡し、
  workerがcache／RAR direct probe／再変換を解決する。同じtargetとleaseをpassword／確認／変換完了まで保持する。
  workerのcache採用は `lookup_with_visibility` とpolicy snapshotを使い、来歴が必要な旧cacheを迂回採用しない。
- 新bundleは既存registry buildの規則で通常／退避stackともUnseen。独立closeはmain帰還を発行しない。
  共有nested bytesの一括clearをscope openでは行わず、既存のarchive identity検証を使う。
  HWND／rect、viewport identity、registry transaction、placement保存、timer、新しいApp fieldは変更しない。

独立source reviewの初回はREVISE。productionの変換ownerへBookScopeを生成する接続不足と、
非RAR workerへcache照会を広げた際のvisibility／origin条件の不足を修正した。
追レビューはACCEPT（追加P1/P2なし）、所有境界に沿った構造修正であることにも合意。
設計の前提矛盾はなく、既存workerの条件を維持した接続として実装した。

新規headlessは三入力matrix、literal／prepared／raw collapse、resume設定、消失・変更・hidden、
enum／hydration／構成待ち取消・遅着、実変換owner／password取消、park再open、main／兄弟／close不変を検査する。
非RAR workerはcache hitが内容scanより先であることと、origin0旧cacheのnested除外時再変換・非削除を検査する。
既存fixtureはtyped target／presentationとctx引数を型合わせした。
既存 `section1675_unsafe_structural_prefix_keeps_existing_list_navigation` の通知件数だけは、S1限定の
`auto && !multi_window` からS3の実効ページ表示設定へ更新する。複数窓でも本候補を判定するためInfoが1回必要となる。
一覧・Chosen・非fullscreenのassertionは維持し、テストの削除・ignore追加・期待の弱体化はしていない。
途中版の最初の実行は109件中104成功・5失敗（exit 100）。非対応prefixの旧拒否分岐を既存Listへ戻し、
新規fixtureでは構成serviceの明示install、実park可能な兄弟の生成、hydrationとprojectionの待機境界を切り分ける。
既存の動作期待を弱めず、確定sourceで指定filterを再実行する。最終結果は完了後に本節へ追記する。
fixture修正後のfocused実行は8件中7成功・1失敗（exit 100）。残りは上記S1の通知件数であり、S3の契約へ更新した。

S3最終検証（共通環境：`MSBUILDDISABLENODEREUSE=1`、`CARGO_BUILD_JOBS=1`、
`TEMP`／`TMP` はworktreeの `target/codex-tmp`）：

| コマンド | 結果 | exit code |
|---|---|---|
| `cargo check -p mimageviewer --bin mimageviewer-core` | 成功 | 0 |
| `cargo nextest run -p mimageviewer --lib -E 'test(/section1675_multiwindow_scope_no_main_return\|section1675_multiwindow_same_archive_sibling_unchanged/)'` | 2 passed、0 failed | 0 |
| `cargo nextest run -p mimageviewer --lib -E 'test(/section167[56]/)'` | 110 passed、0 failed | 0 |
| 下記の既存detached／close／archive回帰filter | 1,085 passed、0 failed | 0 |
| `cargo fmt` / `cargo fmt --check` | 成功 | 0 |
| `python scripts/check_ui_glyphs.py` | 危険glyph 0 | 0 |
| `git -c core.safecrlf=false diff --check` | 成功 | 0 |

```powershell
cargo nextest run -p mimageviewer --lib -E 'test(/detached_|viewer_context_registry|section1670|multiwindow|zip_nav_back_selects_book_cell_left_from|zip_nav_ctrl_updown_|section1335_|section1328_|collection_zip_details_auto_close|everything_close_return|section1678_|close_return_tests|index_only|visibility_current_listing|visibility_live_page_index|archive_convert::|rar_archive_cache|book_resume|spread_shift_owner/)'
```

通常closeを初回Preparing取消と分けるため、実viewport描画完了consumerでReady／Activeへ進めるassertionを追加。
兄弟のpark前とclose前にも同じ確認を入れ、補強後の確定sourceで上記2件・110件・1,085件をすべて再検証した。
ログは `target/s3-core-final.log`、`target/s3-ready-close.log`、`target/s3-sections-verified.log`、
`target/s3-regressions-verified.log`。全lib／workspace suiteと製品起動は実施しない。
`powershell -NoProfile -ExecutionPolicy Bypass -File .\scripts\build-dev.ps1 -PreserveRuntime` も成功
（exit 0）。coreとcompanionを `target/dev-runtime` に配置し、VCRT／PEチェック成功（runtime=4、pe=5）。
製品は起動していない。S3 実装引継ぎ時は未コミットで設計担当の検収待ちだった。その後 `7b3960558` に commit 済みで独立 ACCEPT。

S3変更ファイル：

- 製品コード：`src/app.rs`、`src/app/archive_inner_book.rs`、`src/app/sidecar_restore.rs`、
  `src/app/gamepad_input.rs`、`src/ui_main.rs`、`src/app/startup_ops.rs`、`src/app/smart_folder.rs`、
  `src/ui_dialogs/archive_convert.rs`。
- テスト：`src/app/archive_inner_book_s3.rs`（新規）、`src/app/archive_inner_book_s2.rs`、
  `src/app/book_composition_runtime_tests.rs`、`src/app/multiwindow_scenario_tests.rs`、`src/app/tests.rs`、
  `src/app/tests/archive_inner_book_s1.rs`、`src/app/tests/ctrl_down_close_1678.rs`、
  `src/app/tests/detached_binding_identity.rs`、`src/app/tests/startup_notices.rs`。
- 文書：本書、`docs/virtual-folders.md`、`docs/nested-zip-tree-plan.md`、`docs/detached-rework-plan.md`。

### S4 文書・バックログ整備（2026-11-08）

S1〜S3 の独立 ACCEPT を受け、公開マニュアルの設定・本・フルスクリーン・操作一覧・グリッド・検索・読書手順を現在の挙動へ合わせる。
開く設定の内部階層への適用、選択元の内部一覧への帰還と選択・位置、内部本ごとの続きから、複数窓で main を変えない点を説明する。
起動時の `startup_list_restore` と読書終了時の帰還は別の契約として明記する。
キーの action／割当は追加せず、keymap-spec の旧「常に実親へ直帰」の説明だけを訂正する。
製品ページに今回の挙動と矛盾する説明はなく、カードや公開履歴を増やさない。

バックログ §1.675／§1.676 は旧報告・調査を残して branch 実装済み、独立 ACCEPT、master 未マージ・実機確認待ちを追記する。
既存 master の別件は §1.738／§1.739 へ分離する。detached 計画 §11 は S1〜S3 の変更範囲と構造修正の合意を確定記録とする。
S4 は文書だけの未コミット差分であり、製品コードの変更・製品起動は行わない。既存の実装検証証跡を再利用し、文書検査の結果を引継ぎに報告する。
実機確認と master への取り込み時期の利用者確認は別途残る。

S4 文書検査：`python scripts/check_ui_glyphs.py` は危険 glyph 0・exit 0。
worktree 内の一時検査 `python target/s4-doc-check.py` は HTML 8 ページの構造差分・ID と追加ローカルリンク 25 件、
新規番号 §1.738／§1.739、§1.675／§1.676 の旧記録保持、変更 12 ファイルが文書だけであることを確認し exit 0。
`git -c core.safecrlf=false diff --check` も exit 0。製品ページは変更しないためカード検査は対象外。
文書のみなので Rust の再テスト・検証バイナリの再ビルドは行わず、製品起動・公開・commit もしない。

## 10. 実装段階と規模

| 段階・commit境界 | 内容 | 主な検証 |
|---|---|---|
| **S1：共通activationとmain帰還（完了・独立 ACCEPT）** `875166c1b`・`75f796a6e`・`73a478805` | 採択済みOption A、Chosen、row anchor、通常／即時／early-return close、typed内部open継続。S1 時点では内部本を先頭固定とし、resume は S2 で追加 | §9 の入口・close・deep／collapse・専用帰路・linked／独立不変、実装レビュー R1／R2 の回帰成功 |
| **S2：内部resumeの保存契約（完了・独立 ACCEPT）** `e7804a2ea`・`871439372` | key helper、DB/writer adapter、read/write/fence、内部DFS、editor、Remote、maintenance、互換性、IPC 版照合 | §9の最終検証成功。保存開始とmaintenance対応を同じcoherent commitに含めた |
| **S3：複数窓の内部本open（完了・独立 ACCEPT）** `7b3960558` | descriptor target、列挙前scope解決、source alias、hydration／構成／resume、conversion・初回終端 | §9の110件＋既存1,085件成功、core check・検証build成功 |
| **S4：文書・バックログ整備（本変更、未コミット）** | manual／keymap-spec の整合、バックログ状態更新・既存別件の起票、detached §11 と本節の確定記録。製品コード変更なし | S1〜S3の有効証跡を再利用。公開文書のグリフ・差分・リンクを検査し結果を引継ぎに記録。実機確認・マージ時期の確認は別途 |

各段階で独立レビューを行い、修正済みfindingの影響範囲だけを再検証する。S1単独を今回要求の完成・リリース可能状態とは扱わない。

S1 本体は `875166c1b`、実装レビュー R1 の修正は `75f796a6e` に commit 済み。
実装レビュー R2 の修正は `73a478805` に commit 済みで独立 ACCEPT。S2 本体は `e7804a2ea` に commit 済み。
S2 レビュー R1 の版照合修正は `871439372` にcommit済みで独立ACCEPT。
S3 は `7b3960558` に commit 済みで独立 ACCEPT。S4 は今回の文書変更として完了し、未コミットで引き渡す。
branch `fix-1675-1676-zipdir-book`（worktree `G:\home\mimageviewer-zipdir-book`）で実装済みだが master へは未マージ。
取り込み時期は利用者に確認し、v5.1 より後になる場合がある。実機確認は未実施であり、今回の文書整備をリリース受入の完了とは扱わない。

追加対象には `book_resume_key` 相当の共通module、`book_resume_db`、`rename_key_migration`、Remoteのcontainer/session/ui、descriptorを構築する `startup_ops`、archive conversion continuationを含む。

**全体概算：製品コード1,500〜2,500行、テスト1,800〜3,000行、実装・自動検証8〜13人日。** 独立レビュー・修正往復・承認付き実機検証は別。今回の読み取り専用調査では実行時間の実測はしていない。
