# §1.373 本棚への追加直後の一回取り消し

2026-10-12、`next-list` / `ea060b66e` 基点。**設計レビュー待ち・未実装**。
正本項目: [next-release-backlog.md §1.373](next-release-backlog.md#1373-本棚ツールバーの右クリックで本棚へ追加した操作を-ctrlz-で取り消す--利用者の要望-2026-10-11)。
アプリは起動していない。実ファイルの削除を行う実装には着手しない。

## 1. 正本の前提と実装の差異

`rg -n 'book_membership|membership_db' src Cargo.toml` は一致 0 件。
`compile-book-plan.md` §11 は「設計のみ・未実装」であり、この基点には登録済みバッジ、
membership DB、同じ source の重複 skip がない。バックログ §1.373 の「更新している」前提は
この worktree の実装と一致しない。§11 の実装済み扱いへの変更や、新 DB のついでの実装はしない。
他ブランチにその機能があるなら、統合後の実コードで仕様・削除境界を再レビューする。

既存の追加は `ui_main.rs:11340` → `add_grid_selection_to_named_book`
(ui_fullscreen.rs:46926 付近) → `start_book_append_to_named` (app.rs:49377)
→ `append_pages_at_with_raw` (books.rs:780) → `BookAppendSummary`
→ `poll_book_op_pending` (app.rs:49828)。チェック優先、Stack は各 member に展開、
Image／ZIP entry／PDF page 等を `BookPageSource` に固定する。
通常 byte copy／必要な headless 合成の source 選択は維持する。
`bake_stage.rs` は焼き込み段の語彙を所有し、ファイル追加の成否・membership は所有していない。

worker は番号付きページを末尾に書き、summary は件数、最初の path、編集／semantic copy
mapping しか返さない。全出力の identity は返らない。途中の write が失敗すると Err になり、
既に作ったページがあっても summary は届かない。この例外に rollback は追加しない。
`poll_book_op_pending` は metadata copy、件数、必要な一覧更新を行う。

Undo は `undo_stack.rs` の軽量 metadata の undo/redo スタックで、ファイル操作を含めない。
`undo_ops.rs:209` は同期的に entry を適用して redo へ積むため、書庫の read／Shell削除／
hashing をそこへ直接足せない。現在のメタ Undo キーは `handle_meta_undo_keys` (:335) の
`consume_fixed_chord`。消しゴム／隠蔽／テキストには個別 `KeyAction` と Undo がある。

## 2. 提案する操作仕様

全履歴・Redo・番号再計算を持つファイル Undo は作らず、利用者が許容した
**最後の本追加一回だけの token** とする。本棚右クリックと、同じ append に到達する
Ctrl+B／メニュー／viewer 追加は同じ成功 receipt を使う。別本 transfer は対象外。

- 成功した一回の追加 (複数ページ・Stack member を含む) を Ctrl+Z 一回で取り消す。
  元画像、元 ZIP／PDF、既存本ページ、本フォルダ自体は削除しない。
- コピー追加を Redo で再実行しない。metadata Undo/Redo の既存履歴は保持する。
- 対象本への再追加、並べ替え、移動、コピー、削除、改名、後段編集、タグ／★／しおり等の
  mutation が入ったら token を失効させる。別の通常 Undo 操作が入った場合も失効し、
  Ctrl+Z はその新しい操作を既存 stack から戻す。
- カーソル／check／scroll の変更だけでは失効しない。別場所への移動・source一覧の置換・
  viewer の通常 Undo 終了境界では失効。アプリ終了で捨て、永続化しない。
- 外部で本の構成／ファイルが変わった場合、検証で検出し「本の内容が変わったため追加を
 取り消せません」と通知して token を消費する。**その Ctrl+Z で古い metadata Undo を
 続けて実行しない**。一回の入力が二つの意味を持たないようにする。
- 全 skip (将来統合される場合)、失敗、partial write の Err は token を作らない。
  ログと通知、本を開いて残ったページを整理する既存導線で扱う。再試行や部分 Undo は作らない。

### 削除方式 (レビューで確定する判断)

提案は **検証した今回の生成コピーだけを handle で完全削除**。
`book_fs_journal.rs:329` の `remove_file_if_identity` は、exclusive open → 同じ handle で
SHA-256検証 → `FileDispositionInfo` による削除を既に実装している。
今回もこの所有境界を再利用し、名前照合→handle close→path ベース削除に戻さない。
これは元ファイルの Delete のごみ箱仕様を変更するものではなく、新しい取り消しの提案である。

ごみ箱案では、検証後に handle を閉じて Shell へ path を渡す間に外部が同名を差し替えると、
違うファイルを削除する余地が残る。安全な staging／所有権移管を別設計せず、既存 recycle
worker に path を渡すだけの案は採らない。ごみ箱を必須にするか、生成物限定の完全削除でよいかは
設計責任者／利用者がレビュー時に確定する。**この判断前には実装しない**。

## 3. 所有と受付・完了

単純化の検討: 追加全体を新たに modal にすると、現在許されている別の閲覧操作を制限する。
その変更は採らない。追加 worker は既存 `book_op_pending` に載せ、後続操作が起きたら
取り消し token を失効させる。取消対象の追跡・再開・移動先への追従はしない。
新しい Undo の検証／削除中だけ modal にして、追加・編集・移動・履歴・窓切替を入れない。
OS の外部変更は modal で排除できないので、削除対象の handle による同定が別に必要。

`BookAppendUndoState = Empty | Appending { operation, origin, mutation_epoch }
| Ready { receipt } | Undoing { receipt, receiver }` の一つの owner にまとめる。
App に独立 pending bool と ready Option を並べない。既存 book pending の intent/result と
接続し、処理状態を二重に所有しない形を実装レビューで確認する。
受理時の operation ID と Undo boundary epoch を、非同期成功結果まで引き継ぐ。

receipt は root と本の実 folder identity、全 created paths／file object identity／出力 hash、
成功後の本一覧 stamp、必要な metadata revision、source viewer context／undo epoch を持つ。
path は worker が実際に作った絶対 path を記録し、UI の変更後 settings／active_book_name で
作り直さない。worker は既存全件列挙の情報に変更 stamp を加え、出力 hash は書込時に算出する。
常時全ページをデコード／再 hash しない。初期／事後 snapshot の比較で外部差分が混ざっていたら
追加は成功しても token は作らず、通常の追加完了として扱う。

1. append の受理時に前 token を退役。既存 Undo/Redo の履歴は捨てない。
2. 追加中の後続 mutation／Undo boundary は epoch を進める。成功が遅れて来ても古い epoch の
   token を復活させない。ファイル追加と通常成功 UI は既存どおり反映する。
3. metadata copy の受付／完了も同じ append の終端へ含め、成功した自分の内部 copy や同じ
   operation の一覧 refresh を「他操作」と誤分類しない。read-only一覧準備を一般の失効
   除外にせず、operation ID の同一性で限定する。
4. Ctrl+Z の新操作は `KeyAction` へ配送する。既存メタ Undo の固定 Ctrl+Z の入口も同じ
   Action (`UndoLastAction` 仮称) に集約し、Readyならこの一回、なければ既存メタ Undoへ。
   Redoと編集専用 Undo の優先順、IME／dialog gate は維持する。新しい raw `Key::Z` 判定は作らない。
5. 検証と削除は worker。root と folder の reparse／移動／同名再作成を拒否し、追加後の
   **本全体の構成・変更 stamp**、mIV metadata epoch を照合する。
   `BookFileIdentity` は bytes の同定だけなので、同じ bytes の別 object を許可する代用にしない。
   同定が利用できない filesystem では安全に取り消せないと通知する。
6. 消す全出力を先に exclusive handle で開いて identity/hash を検証し、全部合格してから削除へ。
   同じ handle を削除まで保持する。構成検証の確定より後の外部変更は後続変更として扱い、
   いずれの場合も receipt 外のファイルに削除命令を出さない。
7. worker terminal は実際に消した exact paths を返す。既存 metadata purge／presence／
   read cache／book count 更新の成功 path 境界を再利用する。全部終端後 token は Empty、Redoなし。
   UI thread に read_dir、hash、delete、DB全照会を入れない。

外部の同名差し替え、改名後の旧 path 再作成は、普通に到達する race なので構造で拒否する。
書込・DB・削除のまれな失敗は割り切り (ログ + 通知 [+ 作り直し])。自動 rollback／retry／
partial resume は追加しない。削除途中のまれな失敗時も成功 path だけを反映し、残りを通知する。
失敗を理由にユーザーの本構成・しおり・設定を作り直さない。

## 4. 並べ替え・membership・実装範囲

Undo 対象は末尾追加だけ。番号の詰め直しは行わない。既存ページ名／順序は unchanged。
並べ替え dialog の draft を変更する時点で token を失効 (flush 完了まで待たない)。
並べ替え中には既存 dialog 入力 ownership を優先し、本追加 Undo を実行しない。

membership が別工程で導入された場合は、同じ receipt の `created paths` に対応する登録行だけ
削除し、その worker 完了後にバッジを更新する。source 全体／本全体の provenance を消さない。
現基点には該当 DB がないため、今回の実装に membership schema やバッジは追加しない。
この前提差を coordinator が採否判断する。

予定実装: `books.rs` (出力receipt)、`book_fs_journal.rs` (同定済みhandleの再利用)、
`app.rs` (book operation intent / terminal)、`undo_ops.rs`、`keymap.rs`、
`ui_main.rs` / `ui_fullscreen.rs` の追加／mutation入口、並べ替え dialog の実所有箇所。
`undo_stack.rs` の metadata entry にファイル削除は追加しない。
文書は compile-book、keymap spec／plan、architecture、必要なら virtual-folders を同時更新。
metadata mutation／Undo boundary の inventory は独立レビューで確定してから編集する。

## 5. 回帰・受入

予定 (未実行): 単枚／複数／Stack／ZIP／PDF／合成の一回 undo、元と既存ページが残ること、
二度目の Ctrl+Z と Redo、古い metadata 履歴、後続タグ／編集／別追加／並べ替え draft／
navigation／context切替、遅延成功結果、同じ operation の refresh、外部追加／同名再作成／
内容上書き／本改名／reparse、検証→削除間の差し替え拒否、途中write失敗、purge成功path。
membership は統合された場合に専用回帰を追加。

検証所有者: 実装担当。targeted、fmt、通常／portable core check、項目末尾 full gate 一回。
今回: テスト追加 0 件、実行 0 件、build 0 件。アプリ起動・コミットなし。
