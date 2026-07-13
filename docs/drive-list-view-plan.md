# ドライブ一覧ビュー 実装プラン

**ステータス: 設計確定・Codex 実装反映済み (2026-06-09)**

ドライブルート (`C:\` 等) でグリッドの <kbd>BS</kbd> を押したとき、また起動時に、
「PC に接続された全ドライブを並べた一覧ビュー」を表示できるようにする。

ユーザー確定の方針 (2026-06-09、Codex 相談で確定):

1. **サムネイル**: 既定は固定ドライブアイコン。ユーザーが手動でピン留めした
   代表サムネが **DB にあるドライブだけ**、既存カタログ / 動画ピン DB にサムネが
   残っていれば表示する。ピン先がフォルダの場合も、親 catalog に残っている
   そのフォルダタイルの既存サムネだけを使う。通常のドライブルート表示中に
   ピン代表セルが生成された場合は、`CachePolicy` に関わらずそのセルだけ親 catalog に
   残して、次回のドライブ一覧 seed に使えるようにする。ドライブルート catalog は
   ドライブ別に分離し、正しい root catalog にサムネが無い場合は子 catalog を掘らず
   固定アイコンへ戻す。**ドライブ一覧表示中は
   ドライブルートの自動再帰スキャン・代表画像探索・read_dir・メタデータ確認・
   ピン先デコードを一切行わない**。
2. **起動方法**: 環境設定「起動時に開く場所」に専用モード **「ドライブ一覧」**
   (`StartupFolderMode::Drives`) を追加する。
3. **ラベル**: **ドライブ文字のみ** (例: `C:`、`D:`)。**ボリューム名は取得しない**。

### 設計ゴール: 一覧を開いただけでデバイスに触らない ⚠️

最重要の非機能要件。ドライブ一覧を**表示しただけ**では、各ドライブのファイル
システム/メディアに一切問い合わせない。これにより、スリープ中の外付け HDD を
起こす・空の光学ドライブで待つ・低速 USB/SD で遅れる・ネットワーク先へ問い合わせる
といった副作用を避ける。

- `GetVolumeInformationW` (ボリューム名) は FS/メディアへ問い合わせ得るので**使わない**
  (今回の用途ではメリットが小さい)。
- ラベルはパス文字列から純粋に導出 (`C:\` → `C:`)。syscall ゼロ、非同期解決も不要。
- ドライブ列挙の `available_drives()` が呼ぶ `GetDriveTypeW` はマウント種別を返すだけで
  メディアにアクセスしない (= 既にアドレスバー「場所▼」で使用中で実績あり)。安全。
- サムネは「DB にピンがある時だけ」既存カタログ / 動画ピン DB を cache-only で参照。
  フォルダをピンしている場合も、親 catalog に残るフォルダタイル用キャッシュだけを
  参照する。通常のドライブルート表示中にピン代表セルが worker で生成された場合は、
  そのセルだけ保存を強制して cache-only seed の材料にする。ドライブ一覧表示時に
  キャッシュ未作成なら即アイコン (read_dir / metadata / decode もしない)。
- ドライブルート catalog は `C:\` と `D:\` を別 DB にする。直下同名項目 (`Photos`
  など) のサムネ取り違えを防ぐため。サブフォルダ catalog は従来どおりドライブ文字を
  落とし、リムーバブルドライブのレター変更耐性を維持する。
- デバイスに実際に触れるのは、ユーザーが**そのドライブをクリック/Enter で開いた
  とき**だけ (= 明示操作なので許容)。

---

## 1. 現状調査 (なぜこの設計になるか)

### 1.1 BS の現挙動

グリッドの BS ハンドラ ([src/app.rs](../src/app.rs) `update_keyboard` 内、約 14217-14259):

```rust
if (backspace && !ctrl_held) || alt_up {
    // ... snapshot / global_search / favsearch / local_search の各モードを先に処理 ...
    if let Some(ref cur) = self.effective_folder() {
        if let Some(parent) = cur.parent() {
            self.select_after_load = cur.file_name()...;          // 元フォルダ名を選択
            return Some(AddressBarNav::Direct(parent.to_path_buf()));
        }
        // ← parent() == None (ドライブルート) のときは空振りして何も起きない
    }
}
```

`Path::new("C:\\").parent()` は `None` を返すため、**ドライブルートで BS は no-op**。
ここに「ドライブ一覧へ」の分岐を足す。`parent().is_none()` がドライブルート判定
そのもの (UNC 共有ルート `\\server\share` も同様に None)。

### 1.2 既存の再利用部品

| 部品 | 場所 | 再利用内容 |
| --- | --- | --- |
| `available_drives() -> Vec<PathBuf>` | [src/known_folders.rs:135](../src/known_folders.rs) | `C:\`, `D:\` … を列挙済み。`DRIVE_UNKNOWN`/`DRIVE_NO_ROOT_DIR` は除外済み。アドレスバー「場所▼」でも使用中 ([src/ui_main.rs:1612](../src/ui_main.rs)) |
| `startup_folder()` + `StartupFolderMode` | [src/known_folders.rs:37](../src/known_folders.rs) / [src/settings.rs:868](../src/settings.rs) | 起動時の場所決定が 1 関数 + enum に集約。ここに `Drives` を足すだけ |
| フォルダ代表サムネ + 手動ピン | `folder_thumb_pins` / [src/thumb_loader.rs:1709](../src/thumb_loader.rs) | `GridItem::Folder` に対し既に動く。ピンのキーは正規化パスなのでドライブルートでも成立 |
| 仮想ビューの前例 | `items_are_global_search_view` フラグ + 合成パス ([src/app.rs:2461](../src/app.rs)) | 「実体フォルダの無いビュー」のフラグ運用パターンを踏襲 |
| グリッド空表示 | [src/ui_main.rs:898](../src/ui_main.rs) | `items` を埋めれば通常描画され空表示にはならない |

### 1.3 表現方法の決定: `GridItem::Folder` を再利用する

ドライブ一覧の各セルは **`GridItem::Folder(ドライブルート)`** として並べる。

- クリック/Enter で開く・並び替えなどの基本挙動を既存 Folder 経路から再利用できる。
  ただし drive-list では D&D/コンテキストメニュー/レーティング等を明示的に止める。
- 新 `GridItem::Drive` 変種を足すと、CLAUDE.md が繰り返し警告する
  「Folder/ZipImage/PdfPage の 3 分岐」を全 match 箇所 (描画/クリック/サムネ/
  D&D/コンテキストメニュー/レーティング…) に追加することになり、分岐漏れの温床。
  ドライブはセマンティクス的に「中に入れるコンテナ」= フォルダそのものなので、
  Folder 再利用が正しい。

**懸念とその解決**:

- `Path("C:\\").file_name()` は `None` → ラベルが空になる。
  → §3 の `drive_display_label()` でドライブ文字 + ボリューム名を補う。
- ドライブルートを通常フォルダ扱いすると、代表サムネ生成が `C:\` を
  `folder_thumb_depth` (既定 3) で再帰スキャンしてしまう。heavy_io worker なので
  UI は止まらないが、システムドライブで無意味な画像を拾う/遅い。
  → §3 の「ピンのみ解決」でドライブルートの自動スキャンを抑止する。

### 1.4 current_folder に履歴用 synthetic marker を持たせる

ユーザーの「場所が空欄 = ドライブ一覧」というメンタルモデルに対応させ、
アドレス表示は従来どおり `address = ""` とする。一方、フォルダ履歴の ←/→ で
ドライブ一覧への出入りを表せるよう、`current_folder` には実在しない
`drive_list_synthetic_path()` (`__drive_list__`) を持たせる。
通常フォルダ処理へ synthetic path を流さないため、`items_are_drive_list: bool` 中の
`effective_folder()` は従来どおり `None` を返す。親移動、Ctrl+↑↓、D&D、rating、代表サムネ
探索などの既存 drive-list ガードは維持し、履歴コードだけが marker を現在地として扱う。

---

## 2. 実装フェーズ

### Phase 1 — ドライブ一覧ビューの土台

- `App` に `items_are_drive_list: bool` を追加 (`Default` で false)。
- ドライブ表示名キャッシュ用フィールドを追加 (§3.2 参照、UI スレッドで
  `GetVolumeInformationW` を叩かないため)。
- **`enter_drive_list(&mut self, origin: Option<PathBuf>)`** を新設。
  `start_loading_items` ([src/app.rs:9473](../src/app.rs)) を参考に:
  - サイドカー flush / フルスクリーン close / 進行中 nav cancel / 各種キャッシュ clear。
  - `self.current_folder = Some(drive_list_synthetic_path()); self.address = String::new();`
  - `self.items = available_drives().map(GridItem::Folder)`、`image_metas` は全 None。
    `install_new_items` で割り当て (これで `items_are_global_search_view=false` も倒れる)。
  - `self.items_are_drive_list = true;` (install_new_items の**後**に立てる)。
  - `origin` が `Some` なら、そのドライブの index を `selected` にする
    (ドライブルートはファイル名が無いので `select_after_load` (名前一致) では復元
    できない。`enter_drive_list` 内で**パス完全一致**して index を直接 set する)。
  - ドライブ表示名の非同期解決をキック (§3.2)。
- `install_new_items` ([src/app.rs:10266](../src/app.rs)) で
  `self.items_are_drive_list = false;` を追加 (検索フラグと同じ位置)。
- `AddressBarNav` に `DriveList` 変種を追加 ([src/ui_main.rs:39](../src/ui_main.rs))。
  中央ディスパッチャ ([src/app.rs:27933](../src/app.rs)) の match に:
  ```rust
  crate::ui_main::AddressBarNav::DriveList => { self.enter_drive_list(None); None }
  ```
  (この match の各アームは `Option<PathBuf>` を返すので `None` でよい。
  `enter_drive_list` 自身が全状態遷移を行うため path 経路には流さない)。

### Phase 2 — 入口 (BS と起動)

- **BS**: [src/app.rs:14249](../src/app.rs) の Folder 遡上ブロックを変更:
  ```rust
  if let Some(ref cur) = self.effective_folder() {
      if let Some(parent) = cur.parent() {
          self.select_after_load = cur.file_name()...;
          return Some(AddressBarNav::Direct(parent.to_path_buf()));
      } else {
          // ドライブルート → ドライブ一覧へ。来たドライブを選択状態にする。
          return Some(AddressBarNav::DriveList /* origin は別途保持 or 専用変種 */);
      }
  }
  ```
  来たドライブを選択させたいので、`DriveList(Option<PathBuf>)` にして origin を運ぶか、
  `self.select_after_load` ではなくドライブ専用の選択ヒント
  (`drive_list_select_origin: Option<PathBuf>`) を立てて `enter_drive_list` が読む。
- **起動**: `StartupFolderMode::Drives` を追加 ([src/settings.rs:868](../src/settings.rs)、
  `label()` に "ドライブ一覧")。
  - `startup_folder()` ([src/known_folders.rs:37](../src/known_folders.rs)) は
    `PathBuf` を返す関数なのでドライブ一覧 (実体パス無し) を表現できない。
    起動シーケンス ([src/app.rs:27165](../src/app.rs)) 側で先に
    `if self.settings.startup_folder_mode == Drives { self.enter_drive_list(None); }`
    と分岐し、それ以外を従来どおり `startup_folder()` に委譲する。
  - 設定 UI: [src/ui_dialogs/preferences/pages.rs:104](../src/ui_dialogs/preferences/pages.rs)
    `page_startup_folder` に「ドライブ一覧」ラジオを追加。

### Phase 3 — 描画とラベル

- `draw_cell` の Folder アーム ([src/app.rs:29693](../src/app.rs)):
  - ラベル: `path.file_name()` は空 (= ドライブルート) なので、`C:\` → `C:` を
    **パス文字列から純粋に導出**する小ヘルパー (`drive_letter_label(path)`) を呼ぶ。
    ボリューム名は取得しないので syscall も非同期解決も不要。`draw_cell` 内 (または
    呼び出し元) で `items_are_drive_list && file_name 空` のとき差し込むだけ。
    → 当初案の `display_name_override` 引数 / `drive_labels` キャッシュ / 解決スレッドは
    **不要になった** (ボリューム名廃止の効果)。
  - アイコン: **固定ドライブアイコン**を painter プリミティブで描く。絵文字の
    ドライブ記号 (💾 / 🖴 等) は Yu Gothic で tofu 化リスク
    (CLAUDE.md「UI 文字列の Unicode グリフ選定ルール」、過去に 🎚 / ✕ で実害) があるため、
    UI 表示には使わない。
- **サムネ = ピン済み cache-only (read_dir せず、自動代表探索にフォールバックしない)**:
  - 可視ドライブのサムネ要求を組み立てる箇所で、`items_are_drive_list` かつ Folder の
    とき、`folder_thumb_pin_db.lookup(drive)` を引く。**None なら load 要求を出さず
    即アイコン** (read_dir/再帰/メタデータ確認をしない)。
  - ピンがあるときも `std::fs::metadata` / デコードは行わない。DB 行の
    source kind + relative path から pinned cache key の prefix だけを作り、合成
    drive-list catalog に一致する既存サムネがあれば表示する。
  - 表示前に、通常フォルダ catalog / ZIP・PDF 仮想 catalog / `video_pins.db` にある
    既存サムネを合成 drive-list catalog へ seed する。読み元・書き先はいずれも
    `%APPDATA%` 配下の SQLite / BLOB で、対象ドライブのファイルシステムへは触れない。
  - ピン先が Folder leaf に落ちる場合は代表探索に倒さず、親 catalog に残っている
    そのフォルダタイルの既存サムネだけを seed する。未キャッシュならアイコンに戻す。
  - ドライブルートを通常フォルダとして表示している間は、ピン代表そのもののセルだけ
    `LoadRequest::force_cache` を立てる。Auto/Off 設定でフォルダタイルが保存されず
    seed できない取りこぼしを防ぐためで、ドライブ一覧表示時の I/O は増やさない。
  - root 直下フォルダのタイルが root catalog に無い場合は、子フォルダ catalog に
    cascade して拾わない。サブフォルダ catalog はドライブ文字を落として共有されるため、
    誤画像を出すより固定アイコン fallback を優先する。
  - これでピン尊重 (ユーザー希望) と「一覧表示でデバイスに触らない」を両立する。

### Phase 4 — 操作制限 / 周辺の整合

操作制限の方針 (確定): ドライブ一覧では「クリック / Enter でドライブを通常フォルダ
として開く」だけを基本とし、D&D・チェック (複数選択系)・レーティングは無効にする。
カーソル選択 (単一 selected) は Enter で開く前提なので**残す**。

下記はコード調査 (2026-06-09) で確認した「自然に無効か / 明示ゲートが要るか」の切り分け:

- **チェック / 複数選択 (Space, Shift+矢印)** → **既に自然に無効**。追加対応不要。
  `is_checkable()` は `file_operation_path().is_some()` 依存で、`Folder` は
  `file_operation_path()==None` ([src/grid_item.rs:206](../src/grid_item.rs)) のため
  ドライブ (= Folder) はチェック対象外。Shift+矢印も `is_checkable()` ガード
  ([src/app.rs:13998](../src/app.rs)) を通るので効かない。
- **ファイル D&D 送出 → 明示ゲートが要る**。`drag_source_path()` は **Folder を含む**
  ([src/grid_item.rs:228](../src/grid_item.rs)) ので、ドライブはドラッグ可能なまま。
  放置するとドライブ丸ごとコピー送出になり危険。検索ビューと同じガード
  (`items_are_global_search_view || favsearch.on_results_grid()` の箇所、
  [docs/file-drag-drop-design.md](file-drag-drop-design.md)) に
  `|| items_are_drive_list` を合流させる。
- **レーティング → 明示ゲートが要る**。`Folder` は `is_container_ratable()==true` ゆえ
  `accepts_rating()==true` ([src/grid_item.rs:140](../src/grid_item.rs)) で、
  F1〜F5 (`apply_rating_to_selection` → `ratable_targets`、[src/app.rs:17382](../src/app.rs))
  は **selected なドライブにコンテナ★を書いてしまう** (rating.db にドライブルートパスで
  記録)。`apply_rating_to_selection` 冒頭で `if self.items_are_drive_list { return; }`
  を足す。Shift+F1〜F5 (`set_current_folder_rating`) は `current_folder==None` で
  既に false 返し ([src/app.rs:16836](../src/app.rs)) なので追加不要。
  F6 (★解除) も `apply_rating_to_selection` 経由なので上記ゲートでカバーされる。
- **履歴**: スクロール復元用の `folder_history` には記録しない。一方、フォルダバー / Alt+←→ の
  back/forward 履歴には `drive_list_synthetic_path()` を保持する。メニュー / BS / 場所アクションから
  入るときだけ「直前の場所 → ドライブ一覧」を記録し、履歴 dispatch からの再入場では二重記録しない。
  ドライブ一覧では親移動 (BS / ⬆) は引き続き no-op だが、back/forward target があれば ←/→ は有効。
- **Ctrl+↑↓ / Ctrl+PageUp/Down**: `effective_folder()==None` なので
  `start_folder_nav` は自然に no-op (追加対応不要、念のためテストで確認)。
- **`last_folder` の保存**: ドライブ一覧表示中に終了したときは、
  **空パス sentinel** を `last_folder` に保存する。`StartupFolderMode::Previous`
  (= 前回終了した場所) では、この空パスをドライブ一覧として復元する。
  実フォルダの `last_folder` は通常フォルダへ移動したときに上書きされるため、
  最近使ったフォルダ履歴にはドライブ一覧 synthetic path を入れない。
- **ドキュメント同時更新** (CLAUDE.md 必須):
  - `docs/keymap-spec.md` — BS のドライブルート挙動 (ドライブ一覧へ) を追記。
  - `docs/spec.md` — `StartupFolderMode` に `Drives` を追記 ([docs/spec.md:609](spec.md))。
  - `htdocs/mimageviewer/manual/grid.html` — BS 説明にドライブ一覧を追記。
  - `htdocs/mimageviewer/manual/settings.html` — 起動時に開く場所の選択肢を更新。
  - `htdocs/mimageviewer/index.html` — 必要なら機能紹介に一言。
  - バージョン固有表記は書かない (CLAUDE.md「記述方針」)。

### Phase 5 — テスト

- `src/app/tests.rs` (テストは `cargo test --bin mimageviewer-core` で実行、
  `[[reference_test_target_app_tests]]` メモ参照):
  - 既存 BS テスト ([src/app/tests.rs:902](../src/app/tests.rs)) に倣い、
    「ドライブルートで BS → `items_are_drive_list==true` かつ items がドライブ群」。
  - 「`StartupFolderMode::Drives` で起動 → ドライブ一覧」。
  - 「ドライブをダブルクリック → `load_folder(drive)` で通常フォルダに入る」。
  - 「ドライブ一覧中はスクロール復元用 folder_history に記録されない」。
  - 「フォルダ → ドライブ一覧 → ← で元フォルダ、→ でドライブ一覧へ戻る」。
  - 「ドライブ一覧からフォルダを開いた後、← でドライブ一覧へ戻る」。
- 手動検証 (`/run` または実機): BS 往復・起動モード・ピン留めサムネ・
  ボリューム名表示・空の光学/リムーバブルドライブで UI がハングしないこと。

---

## 3. 詳細メモ

### 3.1 ドライブルート判定

`path.parent().is_none()` がドライブルート/UNC 共有ルートの判定。BS ハンドラの
既存分岐 (`if let Some(parent) = cur.parent()`) の else がそのまま該当する。
専用ヘルパー `fn is_drive_or_share_root(p: &Path) -> bool { p.parent().is_none() }`
を `folder_tree` か `known_folders` に置いてもよい。

### 3.2 ボリューム名は取得しない (廃止)

当初案ではボリューム名 (`C: (Windows)`) を非同期スレッドで解決する設計だったが、
`GetVolumeInformationW` が FS/メディアへ問い合わせ得る (スリープ HDD を起こす / 空の
光学ドライブで待つ等) ため、**仕様から外した** (§1「設計ゴール」)。

- ラベルは `C:\` → `C:` のパス文字列導出のみ。syscall ゼロ、非同期解決スレッド不要、
  `drive_labels` キャッシュも不要。
- `available_drives()` の `GetDriveTypeW` はマウント種別を返すだけでメディアに触れない
  (既存のアドレスバー「場所▼」で実績あり)。一覧表示でデバイスに触らない要件を満たす。

### 3.3 `enter_drive_list` が触る状態 (start_loading_items との対応)

`start_loading_items` の処理のうち実フォルダ前提のもの (catalog の
delete_missing、`current_folder_last_mtime`/signature、prewarm_rating 等) は
ドライブ一覧では不要 or None 相当。共通の「フルスクリーン close / nav cancel /
キャッシュ clear / install_new_items / rebuild_visible_indices」だけを行う
薄い版にする。重複を避けるため共通部分を小さなヘルパーに切り出してもよいが、
まずは `enter_drive_list` 内に必要分だけ書くのが安全 (start_loading_items は
巨大で副作用が多く、安易な共有はリグレッションを生みやすい)。

### 3.4 アドレスバーからの再入

アドレスバーが空欄の状態で適当なパスを打って Enter すれば通常 nav に入る
(挙動不変)。空欄 Enter は no-op のままでよい。「場所▼」メニューに
「ドライブ一覧」項目を足すかは任意 (あれば導線が増える)。

---

## 4. 影響範囲まとめ (差分予想)

| ファイル | 変更 |
| --- | --- |
| `src/app.rs` | `items_are_drive_list` フィールド、`enter_drive_list`、BS 分岐、起動分岐、`install_new_items`/ディスパッチャ/履歴 への合流、`apply_rating_to_selection` のゲート、`draw_cell` のドライブ文字ラベル + 固定アイコン、ドライブの enqueue ゲート (ピンのみ) |
| `src/ui_main.rs` | `AddressBarNav::DriveList` |
| `src/settings.rs` | `StartupFolderMode::Drives` + `label()` |
| `src/known_folders.rs` | (任意) ドライブルート判定ヘルパー `drive_letter_label` |
| `src/file_drag.rs` 周辺 | D&D 送出ガードに `items_are_drive_list` 合流 |
| `src/thumb_loader.rs` 周辺 | drive-list 用 pinned request は cache-only。prefix 一致の既存カタログ行だけ返し、miss はアイコンへ戻す |
| `src/ui_dialogs/preferences/pages.rs` | 起動モードのラジオ追加 |
| `src/app/tests.rs` | ユニットテスト |
| `docs/*`, `htdocs/*` | ドキュメント同時更新 |

中核は Phase 1-3。ボリューム名廃止で非同期解決スレッド/キャッシュが消え、当初見積より
やや軽い。コードはテスト/ドキュメント込みで概ね 200-300 行規模、リスク低。

---

## 5. 補足 / 今後の判断

- **ドライブの固定アイコンの見た目**: painter で簡単な HDD 形状を自前描画する。
  フォント非依存で、絵文字ドライブ記号の tofu リスクを避ける。
- **BS でドライブ一覧に戻ったとき、来たドライブを選択するか**: パス完全一致で
  index を selected にする。
- **ホットプラグ追従**: 一覧表示中に USB 挿抜しても自動更新しない (watcher 無し)。
  当面は許容。必要なら手動 refresh (再 BS / 場所メニュー) で再列挙。
