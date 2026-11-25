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
| 貼り付け (切り取りの続き) | Ctrl+V・メニュー | 元は OS のクリップボード。mIV が切り取りでルートを入れなければ、mIV 発のルートの移動は起きない |

- Shift+Delete (完全削除) の操作は無い。削除は常に FOF_ALLOWUNDO。
- MCP と mIV Remote には、ファイルを削除・名前変更・移動する操作が無い (タグ・評価・コレクションなどだけ)。対象外。
- Windows の Shell の右クリックメニュー (Explorer の削除など) は、mIV の確認を通らない。ドライブ一覧では右クリック自体を
  開かない既存の分岐があり、他の場所にはルートの項目が出ない。下の §3.3 で渡すパスからも外す。

## 3. 設計

### 3.1 判定の持ち主を 1 つにする

`src/grid_item.rs` に、パスだけで決まる判定を 1 つ置く:

```rust
/// 削除・切り取り・名前の変更・移動の対象にできないパスなら理由を返す。
pub(crate) fn structural_file_operation_path_refusal(path: &Path) -> Option<FileOperationRefusal>
```

- `VolumeRoot`: `path_key::is_drive_or_share_root(path)` (= `parent().is_none()`)。`C:\`、`C:`、`\\?\C:\`、
  `\\server\share`、`\\?\UNC\server\share\`、`\\?\Volume{...}\`、subst のドライブ (`X:\`) はすべてこれに当たる。
  ファイルシステムへ問い合わせないので、UI スレッドで呼んでよい。
- `VirtualLocation`: `App::is_synthetic_view_path(path)` (`%APPDATA%\mimageviewer\__drive_list__` などの合成パス)。
  閲覧履歴などから合成パスが `Folder` 項目として出た場合も、実ファイルとして扱わない。
  (`is_synthetic_view_path` は今 `app.rs` の自由関数。判定の持ち主を変えずに呼べるよう `pub(crate)` のまま使う。)

`FileOperationRefusal` に `VolumeRoot` と `VirtualLocation` を足し、文言もここで持つ
(「ドライブや共有のルートは{action}できません」「仮想の場所は{action}できません」)。

項目からは `GridItem::structural_file_operation_refusal(&self)` を使う。中身は
`file_operation_refusal()` の結果、無ければ `drag_source_path()` に上の判定をかけた結果。

**コピー (Ctrl+C・ドラッグのコピー・パスのコピー) は対象外**。今の挙動を変えない。

### 3.2 断る場所 (確認の画面を出す前)

| 操作 | 断る場所 | 断り方 |
| --- | --- | --- |
| 削除 | `request_delete_confirm` の先頭で全対象を判定。1 件でも当たれば確認を開かず、通知を出して戻る (一部だけ消す動きにはしない) | `show_feedback_warning(reason.message("削除"))` |
| 切り取り | `invoke_shell_clipboard_verb_for_paths_with` で verb が `Cut` のとき、全パスを判定 | 同上、「切り取り」 |
| 名前の変更 | `request_rename_dialog` の先頭 (今の `file_name()` が無いときの分岐より前) | 同上、「名前の変更」 |
| 移動 (整理先) | `request_file_organize_dialog` で集めた対象を判定 (整理先は移動とコピーを窓の中で選ぶので、窓を開く前に両方とも断る) | 同上、「整理」 |
| 移動 (ドラッグ) | ドロップ時の判定 (`main_drop_target`) で効果が移動になるとき。コピーになるときは通す | 同上、「移動」 |

ドライブはチェックできないので (ドライブ一覧ではフォルダのチェックを禁止済み)、ドライブとファイルが混ざった選択は
起きない。混ざった場合も「全体を断る」で一貫させる。

### 3.3 最後の防ぎ (worker 側)

UI の入口を後から足しても漏れないよう、Shell を呼ぶ直前でも同じ判定をかける。当たったら実行せず、
エラーとして記録し通知する (黙って成功扱いにしない)。

- `start_delete_files` (確認の後、worker を起こす前)
- `shell_file_ops::validate_transfer_request` で移動のとき
- `shell_file_ops::rename_item_async` / `run_rename_item`
- 右クリックの Shell メニューへ渡す `shell_paths` (`context_menu_target`) から、当たるパスを含む選択では `None` にする

### 3.4 採らなかった案

- **ドライブ一覧のときだけ Delete 等を止める (`items_are_drive_list` を見る)**: 一覧の種類で止めると、同じルートが別の場所
  (閲覧履歴・検索・将来のビュー) に出たときに漏れる。パスで決めれば一覧の種類を問わない。ドライブ一覧の既存の UI 分岐
  (右クリック・ドラッグ・チェック) はそのまま残す。
- **`drag_source_path` 自体がルートで `None` を返す**: コピー・パスのコピー・ドラッグのコピーまで止まり、今の挙動が変わる。
- **フォルダにマウントしたボリューム (`C:\mnt\vol`) も止める**: `GetVolumePathNameW` の問い合わせが要り、UI スレッドで I/O になる。
  Explorer もこのフォルダは通常のフォルダとして扱う (挙動を OS に合わせる)。今回の範囲外。

まれな失敗は割り切り (ログ + 通知)。

## 4. テスト (旧コードで落ちるもの)

handler-level (App のテスト環境):

1. ドライブ一覧 (`install_drive_list` 相当で `Folder("C:\\")` を並べ、カーソルを置く) で Delete キー →
   `show_delete_confirm == false`、警告の通知が 1 件 (旧: 確認が開く)。
2. 同じ状態で Ctrl+X → クリップボードを書く関数が呼ばれない (`_with` の差し替えで数える)、通知 1 件 (旧: 呼ばれる)。
3. `request_rename_dialog(C:\)` → 窓が開かず、`VolumeRoot` の文言の通知 (旧: 窓は開かないが理由が違う/無い — 実装時に旧挙動を確認して書く)。
4. ファイル整理先をドライブで要求 → 窓が開かない、通知 1 件 (旧: 開く)。
5. ドラッグのドロップで、移動になる組み合わせのルートは断り、コピーは通る。
6. 合成パス (`__drive_list__`) の `Folder` 項目で Delete → 確認が開かない。
7. 普通のフォルダ (`C:\foo`) と普通のファイルでは、上のどれも今までどおり確認・窓が開く (退行が無いこと)。

unit: `structural_file_operation_path_refusal` に `C:\`、`C:`、`\\?\C:\`、`\\server\share`、`\\server\share\`、
`\\?\UNC\server\share\`、`\\?\Volume{GUID}\`、`X:\` → `VolumeRoot`、`C:\foo`、`\\server\share\dir` → `None`。
worker 側: `start_delete_files` にルートを直接渡すと worker を起こさない、`validate_transfer_request` が移動のルートを `Err`。

## 5. 文書

- [item-kind-capability-matrix.md](item-kind-capability-matrix.md) にルートと仮想の場所の行を足す。
- マニュアルはドライブ一覧の説明に「ドライブそのものは削除・切り取り・名前の変更・移動できない」を 1 文 (該当箇所があれば)。
