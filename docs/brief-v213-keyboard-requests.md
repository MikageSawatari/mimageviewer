# ブリーフ: §4.8 名前の変更 / 更新コマンド + §4.9 削除確認を矢印キーで選ぶ

対象: v2.13.0 出荷スコープ (バックログ最後の 2 件)。実装 = Codex Sol /
レビュー・検収 = ClaudeCode / 実機確認 = 利用者。

正本: [docs/next-release-backlog.md](next-release-backlog.md) §4.8 / §4.9。
**どちらも設計は確定済み**なので、そのとおりに実装する。2 件は独立なので**変更を分離
できる形**にすること。

前提: master。着手前に `git log --oneline -3` で HEAD を確認すること。

---

## 1. §4.8 名前の変更 / 更新コマンドの追加

**既定の割り当ては一切変えない** (利用者判断 2026-08-07)。上段数字・テンキー・F1〜F6 の
レーティングは現状維持。**不足しているコマンドを足し、既定は `none`** にして、使いたい人が
操作カスタマイズで F2 / F5 へ割り当てられるようにする。**非互換なし**。

### 1.1 追加するアクション (2 つ。どちらも既定 `none`)

- **`GridRename`** (`Grid` 文脈): `App::request_rename_dialog` を呼ぶ。
  **単一選択のみ有効** (複数チェック時は無効かトースト)。ダイアログは Win32 なので、
  ダイアログ内のキー操作は Windows 標準のまま得られる。
  **フルスクリーン側 (`FsCommon`) には入れない** (§1.38 の構造課題に触るため、初版では見送り)。
- **`GridReload`** (`Grid` 文脈): 下表のとおりビュー種別で分岐。
  **メニューにも項目を置く** (利用者判断)。キーを割り当てない利用者にも届く導線が今は
  フォルダツリーペインの `↻` しか無いため。

F3 (検索) は**新規アクション不要**。既存 `GlobalLocalSearch` (既定 `Ctrl+F`) に利用者が
F3 を足すだけで済む。**マニュアルの案内対象にだけ含める。**

### 1.2 `GridReload` の分岐は router 1 か所で

現在のビュー種別は `TopLevelGridSurface`
([src/app/top_level_grid_view.rs](../src/app/top_level_grid_view.rs)) が既に 1 つの enum で
持っている。**この enum を router にして 1 か所で分岐する** (述語を各所へ足さない)。
各ビューには冪等な再入場関数が既にあるので、原則として配線作業になる。

| `TopLevelGridSurface` | F5 の意味 | 呼ぶ既存経路 |
| --- | --- | --- |
| `Folder` | 再読み込み | `reload_current_folder_preserving_override` |
| `SmartFolder` | 再スキャン | `open_smart_folder(id, refresh = true)` |
| `SubfolderExpansion` | 再スキャン | `start_subfolder_expansion_scan_roots` + `SubfolderExpansionSnapshot.roots` |
| `Search(Global)` | クエリ再実行 | `spawn_global_search` |
| `Search(Favorite)` | クエリ再実行 | favsearch の spawn |
| `Search(Tag)` | 再クエリ | `open_tag_view_with_query` |
| `Rating { stars }` | 再構築 | `enter_rating_view(stars)` |
| `ReadingHistory` | 再構築 | `enter_reading_history` |
| `Bookmarks` | 再構築 | `enter_bookmark_view` |
| `DriveList` | 再構築 | `enter_drive_list(origin)` |
| `Snapshot` (★固定) | **何もしない** | 凍結が機能の目的 |

フォルダツリーペインが表示中なら `folder_pane.reload_for_active` も同時に走らせる
(ペインの `↻` と同じ)。Ctrl+F の現在地フィルタは再読み込み後に `execute_search` を
再適用する順序だけ守る。

### 1.3 配線以外の実作業

1. **二重起動の防止**: スマートフォルダ / サブ展開は進捗ダイアログ + cancel を持つ非同期走査。
   **走行中の F5 は無視する** (バックログの「無視が安全」を採る)。
2. **再スキャン後の選択・スクロール・チェック**: `Folder` は既存のパス追跡で復元される。
   スマートフォルダ / サブ展開は items が総入れ替えなので、**初版は「先頭へ」で割り切ってよい**。
   どう決めたかを報告すること。
3. Ctrl+G は「クエリ再実行」であって**索引の作り直しではない**。マニュアルにその旨を書く。

### 1.4 keymap の作法

CLAUDE.md のとおり `ini_name()` / `context()` / `trigger()` / `default_chords()` (= `none`) /
`ALL_ACTIONS` / 呼び出し側 helper / `docs/keymap.ini.default` を揃える。

---

## 2. §4.9 削除確認ダイアログを矢印キーで選べるようにする

現状 `show_delete_confirm_modal`
([src/ui_dialogs/context_menu.rs](../src/ui_dialogs/context_menu.rs)) は Y / N / Esc しか
受けず、矢印での選択も Enter での決定も無い。Tab は `egui_focus_policy` がアプリ全体で
traversal を止めているため、キーボードでフォーカスを動かす手段が無い。

### 2.1 確定した仕様 (利用者判断 2026-08-10)

- **初期フォーカス**: `DeleteConfirmKind::RecycleBin` → **「削除」**、
  `MayPermanent` → **「キャンセル」**。戻せない削除を Enter 連打で通さないため
- **左右キーで選択を移動**。**上下も同義で受ける** (ボタンは横並びだが、迷いにくくする)
- **適用範囲は削除確認だけ**。回転情報リセット / TensorRT パック削除 / 編集用追加ファイル
  削除 / 音量測定値削除などの同型モーダルには**広げない** (リリース直前なので変更面を絞る)。
  ただし**後から共通 helper へ集約できる形**にしておくこと

### 2.2 実装上の約束

- 判定は既存の純関数 `resolve_delete_confirm_action` に**選択位置を引数として足す**。
  純関数の単体テストで固定できる
- Y / N / Esc の既存挙動は**変えない**
- **Enter は `dialog_enter_pressed` を使う** (CLAUDE.md の IME 定型。日本語変換確定の
  Enter を奪わない)。矢印はモーダル中だけの固定入力
- 背面へのキー漏れは `show_delete_confirm` が `common_modal_dialog_open` に入っているので
  既存ゲートで足りる。矢印 / Enter も同じ `consume_delete_confirm_action` の中で consume する
- 選択中ボタンの見た目は `response.request_focus()` で egui のフォーカス枠を出す。
  Tab 抑止ポリシーは focus 移動 API 自体を止めていないので影響しない
- **矢印 / Enter は keymap 対象外**。[docs/keymap-spec.md](keymap-spec.md) に
  **固定である理由を追記**する (CLAUDE.md の要求)

---

## 3. 完了条件

- `cargo fmt` (引数なし)
- `cargo test -p mimageviewer --lib` が全件 / `cargo test -p mimageviewer --test ui_snapshot`
- `cargo check -p mimageviewer --bin mimageviewer-core`
- `python scripts/check_ui_glyphs.py` が 0 件
- 非 Windows を壊さないこと
- **バックログ §4.8 / §4.9 に実装記録を追記**
- `docs/keymap.ini.default` と `docs/keymap-spec.md` を更新
- **マニュアルを更新**: `htdocs/mimageviewer/manual/` に
  「エクスプローラーと同じ F2 / F3 / F5 で使いたい場合の手順」と、更新コマンドのビュー別の
  意味、Ctrl+G が索引を作り直さないことを書く。バージョン番号や内部用語は書かない
  (CLAUDE.md「マニュアル・製品ページの記述方針」)

## 4. 制約

- **アプリを起動しないこと。** 検証ビルドと実機確認は ClaudeCode と利用者が行う
- **ブランチ操作・コミットをしないこと。** master の作業ツリーで未コミットのまま残す
- 既定のキー割り当てを 1 つも変えないこと。判断が要る事態になったら止めて報告すること

---

完了したら次を報告すること:

1. `GridReload` の分岐を router 1 か所に収められたか (できないなら理由)
2. 再スキャン後の選択・スクロールをどう決めたか
3. 削除確認の初期フォーカスが 2 種類で変わることをテストで固定したか
4. テスト結果
5. **実機で確認してほしいこと**
