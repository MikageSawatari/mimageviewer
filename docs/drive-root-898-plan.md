# §1.898 ドライブのルートと仮想の場所をファイル操作の対象から外す — 設計

正本のバックログ: [next-release-backlog.md §1.898](next-release-backlog.md)。担当: small-bugs-746 (ブランチ `drive-root-898`)。

## 1. 観測と原因

- 観測 (サブ PC 担当、2026-11-25、master 38e8e43c5): ドライブ一覧で C: を選んだ状態で Delete → 削除の確認 → Enter で
  `[delete] failed: C:\: Shell 操作が中断されました`。Shell が断ったので被害なし。
- 原因 (コード読み): ドライブ一覧の項目は実フォルダと同じ `GridItem::Folder("C:\\")` (`App::install_drive_list`)。
  ファイル操作の可否は `GridItem::drag_source_path` / `file_operation_refusal` が項目の種類だけで決めており、
  パスがボリュームのルートかを見ていない。ドライブ一覧で塞いでいたのは右クリック・ドラッグ開始・チェックだけで
  (`ui_main.rs` の `items_are_drive_list` の分岐)、Delete キー・Ctrl+X・ファイル整理先は通る。

## 2. 入口の一覧 (コード読み、2026-11-25)

| 操作 | 入口 | 確認・実行の前に通る共通点 |
| --- | --- | --- |
| 削除 | Delete キー (`handle_delete_key`)、グリッドの右クリック「ゴミ箱へ移動」、コレクションの「元ファイルをゴミ箱へ移動」 | `App::request_delete_confirm` (確認の画面を開く唯一の入口、呼び出し 3 か所) → 確認 → `start_delete_files` → `delete_worker` |
| 切り取り | Ctrl+X (`GridCutFiles`)、右クリックの切り取り | `invoke_shell_clipboard_verb_for_paths_with` (キーとメニューで共通) |
| 名前の変更 | `GridRename` (既定キーなし)、右クリック | `request_rename_dialog` → `start_rename_item_after_confirmation` → `shell_file_ops::rename_item_async` |
| 移動 | ファイル整理先 (キー・リング・右クリック)、グリッドからフォルダ／ツリーへのドラッグ (Shift か同じドライブで移動) | 整理先: `request_file_organize_dialog`。ドラッグ: `main_drop_target` の判定 → `submit_shell_transfer_with`。両方とも worker の `validate_transfer_request` を通る |
| 貼り付け (切り取りの続き) | Ctrl+V・メニュー (`main_drop.rs` → `shell_paste.rs` → Shell の `Paste`) | 元は OS のクリップボード。`validate_transfer_request` を通らない。§3.4 で範囲外とする |

- Shift+Delete (完全削除) の操作は無い。グリッドの削除は常に FOF_ALLOWUNDO。
- 本棚の本の削除・名前の変更 (`books.rs`) は別経路だが、本棚の直下の本だけに限られ、ルートには届かない。
- ブックマーク一覧の Delete はブックマークの登録の削除で、ファイル操作ではない。
- 外部へのドラッグは `DROPEFFECT_COPY` だけで、移動にならない。
- MCP と mIV Remote には、ファイルを削除・名前変更・移動する操作が無い (タグ・評価・コレクションなどだけ)。対象外。
- Windows の Shell の右クリックメニュー (Explorer の削除など) は、mIV の確認を通らない。ドライブ一覧では右クリック自体を
  開かない既存の分岐がある。閲覧履歴など他の一覧にルートが現れる場合も、§3.3 の要求の組立で外す。

## 3. 設計

### 3.1 判定の持ち主を 1 つにする

`src/grid_item.rs` に、パスだけで決まる判定を 1 つ置く:

```rust
/// 削除・切り取り・名前の変更・移動の対象にできないパスなら理由を返す。
pub(crate) fn structural_file_operation_path_refusal(path: &Path) -> Option<FileOperationRefusal>
```

- 判定の前に、パスを**ファイルシステムに問い合わせずに**整える (`lexical_normalize`)。`\\?\` で始まらないパスは
  `.` を除き、`..` で直前の通常成分を取り除く (ルートより上へは上がらない)。`\\?\` 形式は Windows が `.`・`..` を
  解釈しない約束なので、そのまま使う。リンク先・ジャンクションの実体は調べない (普通のジャンクションを断らない)。
- `VolumeRoot`: 整えたパスにドライブ指定 (prefix) またはルートがあり、かつ
  `path_key::is_drive_or_share_root` (= `parent().is_none()`)。空・相対パス (`.`、`foo\..`、`foo` など) は
  `VolumeRoot` にしない。`C:\`、`\\?\C:\`、
  `\\server\share`、`\\?\UNC\server\share\`、`\\?\Volume{...}\`、subst のドライブ (`X:\`)、`C:\photos\..`、
  `\\server\share\dir\..` が当たる。`C:` (ドライブ相対の指定) も当たるが、実ルートと断定するのではなく
  「どこを指すか決まらない指定」として断る。`C:\foo\..\bar` は通常のフォルダなので通す。
  ファイルシステムへ問い合わせないので、UI スレッドで呼んでよい。
- `VirtualLocation`: 整えたパスで `App::is_synthetic_view_path(path)` (`%APPDATA%\mimageviewer\__drive_list__` などの合成パス)。
  閲覧履歴などから合成パスが `Folder` 項目として出た場合も、実ファイルとして扱わない。
  (`is_synthetic_view_path` は今 `app.rs` の自由関数。判定の持ち主を変えずに呼べるよう `pub(crate)` のまま使う。
  比較は既存の `folder_tree::path_eq` (大文字小文字・区切りの表記を同一視) を使い、**比べる両側に同じ
  `lexical_normalize` をかける**。`--data-dir .\target\dev-runtime\data` のように `data_dir` 自体が `.`・`..` を含むと、
  比較先の合成パスもその表記で作られるため。`is_synthetic_view_path` とスマートフォルダーの合成パスの判定
  (`smart_folder::is_smart_folder_synthetic_path`) を、両側を整えて比べる形に変える。一致の範囲が広がる方向だけの
  変更で、既存の利用箇所 (実在パスを要求する汎用ガード) にとっても正しい。合成パスは mIV 自身が `data_dir` から作る
  表記だけを対象とし、`\\?\` の別名まではテストで保証しない。)

入力は 1 回だけ正規化し、正規化済み入力用の共通判定へ渡す。`data_dir` も 1 回だけ正規化し、
親と既知の合成名を比べる。合成パス 6 個を先に作らない。スマートフォルダーは ID 解析を
唯一の解析関数とし、真偽判定もその結果から導く。正規化済み入力と `data_dir` を受け取る内部解析を
汎用の合成パス判定とも共有する (2026-11-25 利用者の修正依頼による追補)。
基点の比較は `path_key::eq_synthetic_path_base` を両方の判定で共有し、末尾区切りの有無を同一視する。
verbatim の `.`・`..` は解釈せず、ドライブ相対の `C:` とルートの `C:\` は区別する。

`FileOperationRefusal` に `VolumeRoot` と `VirtualLocation` を足し、文言もここで持つ
(「ドライブや共有のルートは{action}できません」「仮想の場所は{action}できません」)。

項目からは `GridItem::structural_file_operation_refusal(&self)` を使う。中身は
`file_operation_refusal()` の結果、無ければ `drag_source_path()` に上の判定をかけた結果。

**コピー (Ctrl+C・ドラッグのコピー・パスのコピー・整理先のコピー) は対象外**。今の挙動を変えない
(ドライブ一覧の Ctrl+C を保つ既存テスト、`context_menu.rs` の 4189 行付近もそのまま通す)。

### 3.2 断る場所 (確認の画面を出す前)

| 操作 | 断る場所 | 断り方 |
| --- | --- | --- |
| 削除 | `request_delete_confirm` の先頭で全対象を判定。1 件でも当たれば確認を開かず、通知を出して戻る (一部だけ消す動きにはしない) | `show_feedback_warning(reason.message("削除"))` |
| 切り取り | `invoke_shell_clipboard_verb_for_paths_with` で verb が `Cut` のとき、全パスを判定 | 同上、「切り取り」 |
| 名前の変更 | `request_rename_dialog` の先頭 (今の `file_name()` が無いときの分岐より前) | 同上、「名前の変更」 |
| 移動 (整理先) | 窓の中で「移動」を選んで実行したとき (`submit_file_organize_with` の受付)。「コピー」は通す。整理先の窓は削除の確認ではないので、窓は開いてよい | 同上、「移動」。窓は閉じずに残す |
| 移動 (ドラッグ) | ドロップ時の判定 (`main_drop_target`) で効果が移動になるとき。コピーになるときは通す | 同上、「移動」 |

ドライブはチェックできないので (ドライブ一覧ではフォルダのチェックを禁止済み)、ドライブとファイルが混ざった選択は
起きない。混ざった場合も「全体を断る」で一貫させる。

### 3.3 最後の防ぎ (worker 側)

UI の入口を後から足しても漏れないよう、Shell を呼ぶ直前でも同じ判定をかける。当たったら実行せず、
エラーとして記録し通知する (黙って成功扱いにしない)。

- `start_delete_files` (確認の後、worker を起こす前)
- `shell_file_ops::validate_transfer_request` で移動のとき
- `shell_file_ops::rename_item_async` (実行経路が必ず通る 1 か所。`run_rename_item` には重ねない)
- Windows の右クリックメニューを開く要求 (`NativeContextMenuRequest.paths` の組立) で、当たるパスを含む選択なら
  Windows のメニューを使わず mIV のメニューにする。`context_menu_target` の `shell_paths` 自体は変えない
  (Ctrl+C がこの値を使うため。コピーを止めない)

### 3.4 採らなかった案・範囲外

- **貼り付け (Ctrl+V・背景メニュー) の元の検査**: 元は OS のクリップボードで、mIV は行き先のフォルダだけを渡し、
  移動は Shell の `Paste` が行う (Explorer の貼り付けと同じ部品)。mIV 自身の切り取りでルートを入れないので、
  mIV 発のルートの移動は起きない。Explorer はドライブの切り取りを出さないので、ルートを移動用に載せるのは
  別のアプリだけで、その扱いは Explorer で貼り付けたときと同じになる (OS に合わせる)。クリップボードの中身を
  読んで検査する仕組みは足さない。

- **ドライブ一覧のときだけ Delete 等を止める (`items_are_drive_list` を見る)**: 一覧の種類で止めると、同じルートが別の場所
  (閲覧履歴・検索・将来のビュー) に出たときに漏れる。パスで決めれば一覧の種類を問わない。ドライブ一覧の既存の UI 分岐
  (右クリック・ドラッグ・チェック) はそのまま残す。
- **`drag_source_path` 自体がルートで `None` を返す**: コピー・パスのコピー・ドラッグのコピーまで止まり、今の挙動が変わる。
- **フォルダにマウントしたボリューム (`C:\mnt\vol`) も止める**: `GetVolumePathNameW` の問い合わせが要り、UI スレッドで I/O になる。
  Explorer もこのフォルダは通常のフォルダとして扱う (挙動を OS に合わせる)。今回の範囲外。

まれな失敗は割り切り (ログ + 通知)。

## 4. テスト (旧コードで落ちるもの)

破壊的な操作のテストでは、本物の Shell を実パスに呼ばせない。実ドライブのルートは判定の関数にだけ渡し、
実行経路は一時フォルダ内の偽パスと差し替え関数で確かめる。確認・窓の要求だけの handler はルートを使ってよいが、
拒否が常に None でも実行へ進まない作りを保つ。差し替えがあっても実行関数にはルートを渡さない。
最後の防ぎの配線は `<temp>/__drive_list__` の `VirtualLocation` 拒否理由と偽 worker 呼出ゼロで検査する。
通常項目・コピーの受理も一時フォルダ内のパスと偽 worker で検査する。
スマートフォルダーは表記違いで判定と ID が一致し、resident root の復元と open が通ることも検査する。

handler-level (App のテスト環境):

1. ドライブ一覧 (`install_drive_list` 相当で `Folder("C:\\")` を並べ、カーソルを置く) で Delete キー →
   `show_delete_confirm == false`、`VolumeRoot` の文言の通知 (旧: 確認が開く)。
2. 同じ状態で Ctrl+X → キー要求と純粋な受付で `VolumeRoot` を検査する。Ctrl+C は純粋な受付で通る。
   実行経路の通知と呼出回数は、一時フォルダの合成パス (Cut: 0 / Copy: 1) と通常項目で検査する。
3. `request_rename_dialog(C:\)` → 窓が開かず、`VolumeRoot` の文言の通知 (旧: 「この項目は名前を変更できません」)。
4. ファイル整理先でドライブを選び、純粋な移動受付で `VolumeRoot` を検査する (コピー受付は通る)。
   submit の配線は一時フォルダの合成パスで拒否理由・偽転送の呼出ゼロ・選択保持を検査し、コピーは偽転送 1 回を検査する。
5. ドラッグのドロップ: 既存の祖先関係の検査では通る**別ドライブへの Shift 移動**で、ルートを断り通知が出る。同じ組み合わせのコピーは通る。
6. 合成パス (`__drive_list__`) の `Folder` 項目で Delete → 確認が開かず `VirtualLocation` の通知。
7. ルートと通常のフォルダの混在を `request_delete_confirm` へ渡す → 全体を断る。
8. 普通のフォルダ・`foo\..\bar`・普通のファイルを一時フォルダ内に置き、確認・窓の要求と
   差し替え先の呼出で従来の受付を検査する。`C:\foo` 等の表記は純粋なパス判定だけで検査する。

unit: `structural_file_operation_path_refusal` に `C:\`、`C:`、`\\?\C:\`、`\\server\share`、`\\server\share\`、
`\\?\UNC\server\share\`、`\\?\Volume{GUID}\`、`X:\`、`C:\photos\..`、`\\server\share\dir\..` → `VolumeRoot`、
`C:\foo`、`C:\foo\..\bar`、`\\server\share\dir` → `None`、合成パス (末尾区切りあり・大文字小文字違い) → `VirtualLocation`。`data_dir` 自体が `.`・`..` を含むとき (例 `.\data`、`C:\x\..\data`) の合成パスも、入力の表記によらず `VirtualLocation`。スマートフォルダーの合成パスも同じ。
通常／verbatim の `data_dir` の末尾区切りあり・なしの 4 通りで、mIV の生成関数が返した合成パスが
判定され、スマートフォルダーの ID も取り出せることを純粋なテストで検査する。

worker 側: ルートの拒否理由は上の純粋なパス判定で検査する。実行の最後の防ぎは一時フォルダ内の
合成パスで検査し、`start_delete_files_with` / `rename_item_async_with` の偽 worker が呼ばれず、
`VirtualLocation` の理由になることを確かめる。`validate_transfer_request` も一時フォルダ内の合成パスを
移動で拒否し、コピーでは通す。通常の一時フォルダ／ファイルの worker 受付は差し替えで計数する。

## 5. 文書

- [item-kind-capability-matrix.md](item-kind-capability-matrix.md) にルートと仮想の場所の行を足す。
- マニュアルはドライブ一覧の説明に「ドライブそのものは削除・切り取り・名前の変更・移動できない」を 1 文 (該当箇所があれば)。
