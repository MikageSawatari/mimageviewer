# サブフォルダ展開ビュー 設計計画

ステータス: **初期実装済み、大規模一覧の非同期準備とフォルダ単位ソートを追加 (2026-07-17)**。

現在の実ディレクトリ以下にある画像/動画を、その時点のスナップショットとして
フラット一覧化する仮想ビュー。Eagle for Windows の「サブフォルダの内容を表示」に近いが、
mIV はカタログ前提ではないため、常時追従ではなくユーザー明示操作で一時ビューを作る。

関連:
[ui-responsiveness.md](ui-responsiveness.md) /
[async-architecture.md](async-architecture.md) /
[details-view-and-filter-plan.md](details-view-and-filter-plan.md) /
[virtual-folders.md](virtual-folders.md) /
[filename-stack-plan.md](filename-stack-plan.md) /
[rating-list-view-plan.md](rating-list-view-plan.md)

---

## 0. 確定判断

| 論点 | 判断 |
| --- | --- |
| 基本方式 | **スナップショット方式**。ボタンを押した時点で再帰走査し、結果を synthetic flat view として表示する |
| 索引利用 | 初期版では使わない。十分高速であればインデックス依存にしない |
| watcher / 自動更新 | 初期版では行わない |
| 更新ボタン | 初期版では置かない。必要なら通常表示へ戻って再度 `サブ展開` を実行する |
| 画面上の短名 | `サブ展開` を第一候補。ツールチップやメニューでは長い説明を使う |
| 対象 | 通常ファイルシステム上の画像/動画。ZIP/PDF/変換アーカイブの中身は対象外 |
| 永続化 | ビュー状態は保存しない。設定DB / 検索DB / rating DB のスキーマ変更なし |
| synthetic view | `current_folder` は専用 synthetic path にする。実 root は `subfolder_expansion_root` / 走査起点は `subfolder_expansion_roots` / 戻り先は `subfolder_expansion_saved_folder` に保持する |
| 横断ナビ | `Ctrl+↑↓` / `Ctrl+PageUp/Down` は実フォルダ DFS に落とさず no-op + ヒントにする |
| スキャン並列化 | 初期版は single-thread worker + perf 計測。bounded parallel scan は実測後の後続候補 |
| 画像色フィルタ | サブ展開ビュー上でも利用可能にする。実パス画像だけを対象にし、大量件数では確認ゲートを挟む |
| スタック表示 | サブ展開ビュー上でも利用可能にする。別フォルダの同名 prefix が混ざらないよう親フォルダ単位で分類する |
| 並び単位 | 既存互換の `全体で並べる` と、相対フォルダ順を優先する `フォルダごとに並べる` を保存設定として選べる |
| 大量件数 | 10 万件以上は走査完了後に続行確認。ソート・一覧構築・メタデータ取得はキャンセル可能な worker で行い、表示準備中は中止だけ操作できるモーダルを表示する |

実装メモ (2026-06-24):

- `src/app/subfolder_expansion.rs` に single-thread worker と結果適用を追加。
- フォルダバー右側には幅が変わらない `サブ展開` トグルを置く。走査開始から表示準備完了までの
  進捗と `中止` は中央モーダルに統一し、ツールバーの再レイアウトを避ける。展開後のトグルクリックは
  root 通常フォルダへ戻る。
- `subfolder_expansion_synthetic_path()` を `is_synthetic_view_path` に追加し、
  catalog `delete_missing`、`last_folder`、実フォルダ履歴、コンテナ★から切り離す。
- 通常フォルダ表示中はフォルダも Space / Ctrl+クリックでチェックでき、チェックしたフォルダが
  1 件以上ある場合は、そのフォルダ群だけをサブ展開の走査起点にする。チェックなしでは従来どおり
  現在フォルダ全体を展開する。
- 動画サムネイル override はサブ展開ビューで同名動画が衝突しないよう full path key を優先する。
- 完了結果を `SubfolderExpansionSnapshot` として保持し、ソート変更時はメモリ内で再ソートして
  再表示する。ソート変更ではファイルシステムを再走査しない。
- snapshot の entries は `Arc<Vec<_>>` で共有し、再ソートごとに数百万件の走査結果を複製しない。
- `全体で並べる` は従来どおりファイル名を全フォルダ横断で比較する。`フォルダごとに並べる` は
  root からの相対フォルダ順を先に比較し、各フォルダ内にだけ現在のファイルソート順を適用する。
- 走査後のソート、`GridItem` 構築、レーティング・タグ・補正レイヤー・動画ピンの DB 参照は
  `subfolder-view-prepare` worker で完了させる。UI への最終差し替えでは sparse cache を移動し、
  未登録項目ごとの同期 SQLite lookup を行わない。
- サブ展開ビュー上で `スタック` を押すと、保持中のフラット一覧を親フォルダ単位で分類して
  集約ビューにする。スタック OFF では保持済み `SubfolderExpansionSnapshot` を再インストールし、
  ファイルシステムを再走査せず元のフラット一覧へ戻す。

---

## 1. 目的

大量のサブフォルダに分散した画像を、フォルダを移動せずにまとめて確認・整理できるようにする。

想定ユースケース:

- ある親フォルダ以下の未整理画像をまとめて確認する。
- `場所` facet でサブフォルダ単位に絞り込みながら、タグ無し / ★無し / 特定★を潰す。
- 詳細表示で更新日・サイズ・タグ・★を見ながら、配下全体を点検する。
- 一時的に横断表示したいだけで、Eagle のような常時カタログ管理までは要らないケースを扱う。

---

## 2. UI

### 2.1 名称

画面上の短いラベルは **サブ展開** を第一候補にする。

候補:

| 表示 | 評価 |
| --- | --- |
| `サブ展開` | 短い。機能を知った後は意味が取りやすい |
| `配下表示` | 意味は自然だが、通常表示との違いが少し曖昧 |
| `配下展開` | やや硬いが、サブフォルダに限定しないニュアンスはある |
| `下位展開` | 短いが一般ユーザー向けには少し専門的 |

メニューやツールチップでは短名だけに頼らない:

- メニュー: `サブフォルダを展開して表示`
- ツールチップ: `現在のフォルダ以下の画像と動画をフラット表示`。チェックしたフォルダがある場合は
  `チェックした N 個のフォルダ以下の画像と動画をまとめてフラット表示`。
- ビュー見出し: `サブ展開: <root>`。複数起点の場合は root に加えて `Nフォルダ` を併記する。

### 2.2 配置

- フォルダバー右側のフォルダ/表示系の近くに `サブ展開` トグルを置く。
- 初期実装ではメニュー項目は置かない。必要になったら `ファイル` または `表示` メニューへ
  `サブフォルダを展開して表示` を追加する。
- 実ディレクトリ表示中のみ有効にする。合成ビューや ZIP/PDF 内では disabled。
- 実ディレクトリ表示中にフォルダを Space / Ctrl+クリックでチェックしてから `サブ展開` を押すと、
  現在フォルダ全体ではなくチェックしたフォルダ群だけを 1 つのフラットビューにまとめる。

`サブ展開` 中に同じトグルを押すと、元の通常フォルダ表示へ戻る。専用の更新ボタンは作らない。

### 2.3 走査中表示

- 現在の通常フォルダ一覧は表示したまま、走査進捗を中央モーダルに表示する。見つかった画像・動画数、
  確認済みフォルダ数、現在のフォルダを示し、`中止` でキャンセルする。
- 走査開始後は中止以外の操作を遮断する。フォルダバーの表示は常に短い `サブ展開` のままとし、
  件数更新でツールバーの幅や配置を変えない。
- アプリ終了では自動キャンセルする。
- 走査完了後、10 万件以上なら正確な件数を示す確認ダイアログを出す。`続ける` で表示準備、
  `キャンセル` で現在の一覧へ戻る。
- 表示準備は worker で行い、中央のモーダルに処理段階と件数進捗、`中止` を表示する。
  準備中は背景のフォルダ移動・グリッド・ショートカット操作を遮断する。`中止` は現在表示を維持したまま準備結果だけを破棄する。
- ソート、一覧構築、レーティング / タグ / 補正レイヤー / 動画ピンの取得を worker 側で行い、
  完成結果だけを `items` へ差し替える。ソート設定変更でも同じ経路を使い、再走査や続行確認は行わない。
- 表示中の並び順 / 並び単位変更では、現在の sparse cache をパスキーで新しい idx へ割り当て直す。
  レーティング / タグ / 補正レイヤー DB は再読み込みせず、ソートと一覧再構築だけを worker で行う。

完了後にだけ `items` を差し替える。途中結果で一覧を小刻みに置き換えない。

### 2.4 並び単位

サブ展開中はソートメニューとツールバーに並び単位を追加する。

- `全体で並べる` (既定): 従来互換。`a/1.png, b/1.png, a/2.png` のように全体へ現在のソートを適用する。
- `フォルダごとに並べる`: 相対フォルダを Windows に近い名前順で並べ、各フォルダ内へ現在の
  `ファイル名 / 番号 / 日付` ソートを適用する。例は `a/1.png, a/2.png, b/1.png` になる。

並び単位は Settings に保存する。変更時は保持済み snapshot を再利用し、ファイルシステムは再走査しない。

---

## 3. 対象範囲

### 3.1 初期対象

通常フォルダ配下の実ファイル:

- `GridItem::Image(PathBuf)`
- `GridItem::Video(PathBuf)`

画像拡張子判定は通常フォルダ scan と同じく `folder_tree::is_recognized_image_ext` 系に寄せ、
WIC / Susie 対応拡張子とのズレを作らない。動画も通常フォルダ表示と同じ判定を使う。

### 3.2 初期対象外

初期版では以下を出さない。

- `GridItem::Folder`
- `GridItem::ZipFile`
- `GridItem::PdfFile`
- `GridItem::ConvertibleArchive`
- `GridItem::ZipImage`
- `GridItem::PdfPage`
- `GridItem::ZipDir`

ZIP/PDF/変換アーカイブの中身まで展開すると、パスキー、パスワード、変換キャッシュ、ページ列挙、
永続メタの復元規則が一気に増えるため後続に回す。

### 3.3 有効な起点

有効:

- 通常の実ディレクトリ
- 通常の実ディレクトリ直下でチェックした実フォルダ群。チェックしたフォルダが 1 件以上ある場合は、
  現在フォルダ全体ではなくそのフォルダ群を起点として扱う。
- 本棚内の実ディレクトリも、実パスとしては動かせるため許可候補。ただし製本ルート全体への適用は
  大量走査になりやすいので確認ダイアログの対象にする。

無効:

- Drive list
- ZIP/PDF 内
- 読書履歴
- レーティング一覧
- Ctrl+G 検索結果
- Ctrl+S お気に入り検索結果グリッド
- タグビュー
- サブ展開ビュー自身
- snapshot lock 中

UI の enabled 判定だけでなく、実行関数側でも同じガードを行う。キー操作やメニュー以外の経路で
直接呼ばれても、合成ビューから再入したり、★固定スナップショットと状態が競合したりしないようにする。

---

## 4. データモデル

新しい `GridItem` variant は作らない。結果は既存の `Image` / `Video` だけで表現する。

追加する App 状態の例:

```rust
items_are_subfolder_expansion_view: bool,
subfolder_expansion_root: Option<PathBuf>,
subfolder_expansion_roots: Vec<PathBuf>,
subfolder_expansion_saved_folder: Option<PathBuf>,
subfolder_expansion_pending: Option<SubfolderExpansionPending>,
subfolder_expansion_diag: Option<SubfolderExpansionDiag>,
```

`SubfolderExpansionPending`:

```rust
struct SubfolderExpansionPending {
    root: PathBuf,
    roots: Vec<PathBuf>,
    cancel: Arc<AtomicBool>,
    rx: mpsc::Receiver<SubfolderExpansionEvent>,
}
```

worker result:

```rust
struct SubfolderExpansionResult {
    root: PathBuf,
    roots: Vec<PathBuf>,
    entries: Vec<SubfolderExpansionEntry>,
    diag: SubfolderExpansionDiag,
}

struct SubfolderExpansionEntry {
    path: PathBuf,
    is_video: bool,
    mtime: i64,
    file_size: i64,
}
```

synthetic path:

```rust
pub(crate) fn subfolder_expansion_synthetic_path() -> PathBuf;
```

`is_synthetic_view_path` に追加し、検索結果 / 読書履歴 / レーティング一覧と同じく通常フォルダ
ナビゲーションから区別する。

### 4.1 synthetic path の効能

`current_folder` は `subfolder_expansion_synthetic_path()` にする。これにより、既存の synthetic
view と同じガードに乗る。

- `use_full_path_cache_keys_for_folder` が synthetic path を見て full path cache key を使う。
- `start_loading_items` 内の catalog `delete_missing` が synthetic path では走らない。
- `last_folder` / folder history / container rating など、実フォルダ前提の経路から区別できる。

実 root は `subfolder_expansion_root` として別に保持する。チェックした複数フォルダを起点にした場合も、
戻り先や表示上の親はボタンを押した通常フォルダ root とし、実際の走査起点は
`subfolder_expansion_roots` に保持する。Backspace / パンくず / トグル OFF で
戻る先は `subfolder_expansion_saved_folder` (= 通常は root 実フォルダ) とし、synthetic path の
親 (`%APPDATA%\mimageviewer` など) へ落ちないようにする。

Ctrl+S お気に入り検索 / タグビュー / レーティング一覧のように `start_loading_items` で別 synthetic view へ切り替える
一時ビューでは、切り替え直前にサブ展開の snapshot/root/roots を専用の復元 state へ退避する。
`start_loading_items` 自体は通常どおりサブ展開 state をクリアするが、ビューを閉じると退避した
snapshot を再インストールし、サブ展開ビューへ戻す。snapshot が無い場合は退避した root/roots で
再スキャンする。

`existing_keys` は synthetic view では `delete_missing` が走らないため必須ではない。空集合でも
安全だが、サムネイル要求側と cache key 生成側の不変条件を崩さないため、実装では
full-path key の集合を渡しておく方針でもよい。

---

## 5. 構築フロー

```
enter_subfolder_expansion_view(root):
  - root が実ディレクトリか確認
  - チェックしたフォルダがあれば roots = checked folders、なければ roots = [root]
  - 実行禁止ビュー / snapshot lock / 自ビュー再入をガード
  - 必要なら大規模走査確認を出す
  - start_loading_items より前に root / roots / saved_folder を退避
  - 既存 pending を cancel
  - worker を spawn
  - UI は現フォルダ表示のまま進捗表示

worker:
  - roots 以下を single-thread で深さ優先走査
  - DirEntry::file_type() / metadata() を使う
  - 画像/動画だけ SubfolderExpansionEntry にする
  - duplicate filter は worker 側で同一親フォルダ内だけ適用する
  - 定期的に Progress event を送る
  - cancel が立ったら中断
  - Done(result) を送る

poll_subfolder_expansion:
  - stale root / roots mismatch は破棄
  - sort を適用
  - synthetic path で start_loading_items 相当へ入れる
  - items_are_subfolder_expansion_view = true
```

`start_loading_items` へ渡す catalog / existing key の扱いは合成ビューに合わせる。
synthetic path のガードにより、実フォルダ単位の `delete_missing` は走らせない。
これにより、サブフォルダ側のサムネキャッシュを誤って掃除しないようにする。

`start_loading_items` は `current_folder` を更新し、fullscreen / zip_nav / stack 状態にも影響する。
root / saved_folder の退避は必ずその前に済ませる。synthetic view へ入った後に `effective_folder()`
を読むと synthetic path が返るため、戻り先の計算に使わない。

---

## 6. 走査ルール

### 6.1 再帰

- 深さ優先で十分。表示順は最後に sort するので、走査順は UI 表示に依存させない。
- `Path::is_dir()` / `Path::is_file()` を read_dir ループ内で使わない。
- `DirEntry::file_type()` と `DirEntry::metadata()` を使う。
- アクセス拒否 / 消滅競合は診断カウンタに入れて続行する。
- 初期版では single-thread worker で走査する。並列スキャンは実測で不足が見えた後に追加する。

### 6.2 reparse point / symlink

ディレクトリ symlink / junction は、`search_walker` と同じ方針に揃える。独自の簡易実装を作らない。

- `canonicalize` 由来の visited key でループを防ぐ。
- depth limit は `search_walker` と同じ 40 に揃える。
- reparse point を追うか skip するかも、検索インデクサと不一致にならないようにする。

将来 bounded parallel scan を入れる場合、visited set は `Mutex<HashSet<_>>` で共有し、
directory pop / visited insert の短い区間だけ lock する。`try_lock + sleep` は使わない。

### 6.3 大規模走査ガード

以下のような起点では、将来は開始前確認を検討する。

- ドライブ直下
- ユーザープロファイル直下
- ネットワーク共有の root
- 前回同 root の走査で大量件数 / 長時間だった場合

初期版では開始前確認やハード上限での黙った打ち切りは置かず、大きいツリーは常時表示の
進捗と明示 `中止` で扱う。実測で誤操作や UI コストが問題になった場合だけ、確認ダイアログ、
件数上限、結果ページングを後続で検討する。

### 6.4 duplicate filter

通常フォルダ表示の同名ファイルフィルタを尊重する場合でも、適用範囲は **同一親フォルダ内** に限定する。
サブフォルダをまたいで同名ファイルを消すと、別作品・別日付の画像を誤って隠す可能性が高い。

実装は worker 側で `(parent_dir, name)` をキーにグルーピングしてから適用する。フラット化後の
全体リストに通常の duplicate filter をそのままかけない。

---

## 7. 表示 / フィルタ / ソート

### 7.1 表示

通常のサムネイルグリッド / 詳細表示をそのまま使う。サムネイル要求は結果に含まれる実パスへ出す。

合成ビューなので cache key は full path ベースを強制する。別フォルダに同名・同サイズ・同mtimeの
ファイルがあるケースでサムネが衝突しないようにする。

### 7.2 フィルタ

既存の表示内フィルタを使う。

- Ctrl+F 現在地フィルタ
- ★フィルタ
- タグ facet
- 場所 facet
- 種類 / 拡張子 / 日付 / サイズ / 状態 facet

この機能の価値は `場所` facet と組み合わせると強い。`場所` は各アイテムの親フォルダを
root 相対で見せられると分かりやすい。

場所 facet 自体は既存の親フォルダ判定で動く。初期版のラベルは既存どおり絶対パスでよい。
root 相対表示は polish として後続候補にする。共通の `facet_place_label_for_path` を変える場合は、
他ビューへ影響しないようサブ展開ビュー限定の分岐にする。

画像色フィルタはサブ展開ビューでも利用可能にする。サブ展開ビューは大量件数になりやすく、
実ファイル decode / 色抽出を伴うため、通常フォルダと同じ大量時確認 UI を使い、
scope signature は `subfolder_expansion` として通常フォルダと分ける。動画は既存の画像色フィルタ対象外。

### 7.3 ソート

既存の `SortOrder` を適用する。

- ファイル名順 / 数値順 / 日付順は通常表示と同じ。
- 同値の tie-break は root 相対の親フォルダ + ファイル名で安定化する。
- ソート変更時はボタン押下時点の `SubfolderExpansionSnapshot` を再ソートするだけにし、
  ファイルシステムを再走査しない。これによりスナップショット方式の一貫性を保つ。
- view-local な専用ソートは初期版では増やさない。

---

## 8. ナビゲーション / 操作

### 8.1 戻る

- `サブ展開` トグル OFF、Backspace、またはパンくずの戻りで root の通常フォルダ表示へ戻る。
- `subfolder_expansion_saved_folder` は root を保持し、通常の nav_stack と混ぜすぎない。
- `grid_parent_nav_target` / `resolve_grid_parent_nav` / `resolve_return_to_parent_nav` 系では、
  synthetic path の `parent()` を使わず、`subfolder_expansion_saved_folder` へ `Direct` で戻す。
- アドレスバー / パンくずは synthetic path を見せず、`サブ展開: <root>` を表示する。
- フォルダバーの履歴 ←/→ には synthetic path も保持する。サブ展開から実フォルダへ
  移るときは `start_loading_items` が active state を破棄する前に snapshot/root/roots を
  履歴復帰用 state へ退避し、履歴から synthetic path を pop したときに同じ snapshot を
  再インストールする。保持 state が無い場合は no-op とし、synthetic path を実フォルダとして
  ロードしない。

### 8.2 フルスクリーン

フルスクリーンでは、サブ展開ビューのフラット順にページ送りする。

- 通常の前後移動: フラット一覧内を移動
- Home/End: フラット一覧の先頭/末尾
- Ctrl+↑↓: 通常フォルダ移動に出さず、no-op + ヒント表示にする。
- Ctrl+PageUp/Down: 通常フォルダの兄弟移動に出さず、no-op にする。

グリッド側 / フルスクリーン側のどちらも、fall-through で `effective_folder()` →
`start_folder_nav(...)` に落とさない。`items_are_subfolder_expansion_view` の明示分岐を、
local search / tag view と同じく実フォルダ DFS の前に置く。フルスクリーンでは
`FsNavNoOpReason` にサブ展開用 variant を追加し、検索一覧と同じ no-op ヒントを使う。

### 8.3 ファイル操作

対象は実ファイルなので、以下は通常どおり許可する。

- ★ / タグ
- 外部アプリで開く
- rename
- delete to recycle
- drag out / copy
- A/B クイック移動系

ファイル操作で対象パスが消えた場合、ビュー全体を再走査せず、成功したパスだけメモリ上の
items から除去するのを基本にする。move / rename の新パスが分かる操作ではその行だけ更新する。
Shell native menu など結果追跡が難しい操作では、サムネイル失敗や次回 `サブ展開` で自然に整合させる。

行を除去 / 更新する新経路を作る場合は、`visible_indices`、`selected`、`fullscreen_idx`、
先読み対象を clamp / 再計算する。フルスクリーン中に対象行が消えるケースは回帰テストを用意する。

### 8.4 スタック表示

サブ展開ビューではフォルダバーの `スタック` トグルを有効にする。通常フォルダのスタックと同じ
集約グリッド / フラット読書フルスクリーンの挙動を使うが、分類は親フォルダ単位で行う。

- スクリプト分類は親フォルダごとに `group(files)` を呼ぶ。スクリプトから見える `files` は
  従来どおり同一フォルダ内の画像だけなので、先頭連番・末尾連番などの判定が別フォルダの画像で
  乱れない。
- Rhai のコンパイルは 1 回だけ行い、コンパイル済み AST を親フォルダごとの呼び出しで使い回す。
- 返されたキーには親フォルダスコープを内部的に付ける。同じ `post_p0.jpg` / `post_p1.jpg` が
  別フォルダにあっても、別スタックとして扱う。
- スクリプト失敗時の組み込みフォールバックも親フォルダスコープ付き prefix ルールを使う。
- OFF にすると `SubfolderExpansionSnapshot` を再インストールして元のサブ展開フラット一覧へ戻す。
  スナップショットが無い場合だけ再走査へフォールバックする。

---

## 9. 応答性 / 計測

UI スレッドで行わないもの:

- `read_dir`
- metadata 取得
- recursive traversal
- 画像デコード
- SQLite 大量問い合わせ

worker は以下を持つ。

- cancel token
- progress event
- `GlobalIoSemaphore` の Normal priority
- depth limit / reparse point guard
- perf events:
  - `subexpand_begin`
  - `subexpand_progress`
  - `subexpand_done`
  - `subexpand_cancelled`
  - `subexpand_install`

`GlobalIoSemaphore` は Normal priority を使う。ユーザーが明示的に開始し、進捗を見ながら待つ
フォアグラウンド処理なので、インデクサと同じ Low にはしない。一方、可視サムネイルなどの High I/O
には譲る。`ActivityGate` は付けない。ユーザーがスクロールや軽い操作をしただけで走査が止まると、
明示実行した処理として分かりにくいため。

進捗イベントは N 件または 100ms 程度で間引く。件数更新ごとに repaint / lock を発生させない。

mtime / size は初期版では `DirEntry::metadata()` から収集する。日付/サイズソート、詳細表示、
facet の一貫性を保つため。ただしネットワーク共有などで metadata が支配的コストになる可能性は
perf で確認し、問題が見えたら「名前順 + 詳細 OFF では遅延収集」の後続最適化を検討する。

完了後の `items` 差し替えと sort は UI スレッドで走るため、結果件数が非常に大きい場合はここも
計測対象にする。`subfolder.install_sort` / `install_build_items` / `install_existing_keys` /
`install_start_loading` / `install_rebuild_visible` / `install_end` を出し、どこで止まっているかを
ログで切り分ける。サブ展開 synthetic view では catalog `delete_missing` がスキップされるため、
`existing_keys` は空集合を渡し、50 万件級の巨大な存続キー集合を作らない。必要なら worker 側で
sort key 生成まで済ませる。

`facet_place_counts` など、表示集合に対する facet 件数再計算も O(n) になり得る。1 万件超の
サブ展開でフィルタ操作時にヒッチが出ないか、`ui.facet_place_counts_build` で初回キャッシュ構築時間を
計測対象に入れる。`start_loading_items` は `nav.sli_prewarm_rating` / `sli_prewarm_tags` /
`sli_rebuild_visible_indices` を分けて記録し、rating / tag / visible index のどれが支配的かを見る。
大量サブ展開では `場所` 件数キャッシュを遅延構築し、場所フィルタ自身の変更では
キャッシュを破棄しない。これにより、チェック操作直後の再描画で同じ O(n) 集計を繰り返さない。
`場所` メニューは候補が多い場合に可視行だけを `show_rows` で描画し、
`ui.facet_place_menu_render` でメニュー描画時間を計測する。初回クリック時の件数構築がまだ
体感上重い場合は、メニューオープン時の処理中表示や worker 化を後続で検討する。

---

## 10. 実装ステップ

1. [x] `subfolder_expansion` の worker / 純ロジックを追加する。
2. [x] App state と pending / poll を追加する。
3. [x] synthetic path と `items_are_subfolder_expansion_view` を既存 synthetic guard に追加する。
4. [x] Backspace / パンくず / アドレスバー / `Ctrl+↑↓` / `Ctrl+PageUp/Down` の明示分岐を追加する。
5. [x] フォルダバーへ `サブ展開` を追加し、UI enabled と実行関数側の両方で入場ガードする。
6. [x] 進捗 + キャンセル UI を追加する。
7. [x] 画像色フィルタをサブ展開ビューでも利用可能にし、大量件数は確認ゲートで扱う。
8. [x] ソート変更時に snapshot を再利用し、再帰走査を避ける。
9. [x] ★ facet に表示中 snapshot 由来の件数を出す。
10. [x] チェックした複数フォルダを 1 つのサブ展開ビューにまとめる。
11. [x] 大量件数の表示反映前に中央処理中オーバーレイを出し、詳細 perf イベントを追加する。
12. [x] サブ展開ビュー上でスタック表示を有効化し、親フォルダ単位で分類する。
13. [ ] 実機レビューで詳細表示 / facet / ★ / タグ / file operation / スタックの動作を確認する。
14. [ ] perf log と大規模フォルダの実測を取る。

---

## 11. テスト

unit test:

- 再帰走査で画像/動画だけ拾う。
- ZIP/PDF/フォルダ/変換アーカイブを初期版で除外する。
- 同名ファイルフィルタが親フォルダをまたがない。
- symlink / junction / depth limit で無限再帰しない。
- cancel が立ったら早期終了する。
- アクセス拒否 / 消滅競合を診断カウンタに入れて走査継続する。

App-level test:

- 実フォルダから `サブ展開` に入り、synthetic view になる。
- `is_synthetic_view_path(subfolder_expansion_synthetic_path())` が true。
- synthetic view では full path cache key が使われ、catalog `delete_missing` が走らない。
- 合成ビューから戻ると root 通常表示へ戻る。
- Backspace / パンくず / pending return-to-parent が synthetic path の親ではなく root 実フォルダへ戻る。
- Ctrl+↑↓ / Ctrl+PageUp/Down が `start_folder_nav` に落ちず no-op になる。
- 通常フォルダでチェックした複数フォルダだけを起点にサブ展開できる。
- 通常フォルダへ戻ると synthetic view state と full path cache key mode が解除される。
- stale worker result を捨てる。
- ★ / タグ / facet が実パスに効く。
- 場所 facet で特定サブフォルダだけに絞れる。
- 画像色フィルタがサブ展開ビューで画像だけを対象にし、大量件数では確認ゲートを挟む。
- サブ展開ビューでスタック ON/OFF でき、同じ prefix の画像が別フォルダ間で混ざらない。
- サブ展開ビューのスタック OFF でスナップショットからフラット一覧へ戻り、FS 再走査しない。
- delete / move 後に対象行をメモリ上から除去できる。

手動確認:

- 1万件程度のSSDフォルダ。
- HDD / ネットワーク共有。
- アクセス拒否フォルダを含むツリー。
- サブ展開中キャンセル、フォルダ移動、アプリ終了。
- フルスクリーン連続ページ送り。

---

## 12. 後続候補

初期版の実測と使い勝手を見てから判断する。

- ZIP/PDF コンテナ自体を結果に含める。
- ZIP/PDF の中身まで展開する。
- 索引ONのお気に入り配下では `fts_meta.db` から高速構築する。
- bounded parallel scan。ディレクトリ単位 work queue + 2〜4 worker、共有 visited set、I/O semaphore 維持を前提にする。
- サブ展開ビュー専用の `場所順` ソート。
- 場所 facet の root 相対ラベル。
- 名前順 + 詳細 OFF 時の metadata 遅延収集。
- 大量結果のページング / 結果キャッシュ。
- `サブ展開` の前回 root / 前回結果件数を軽く記録し、大規模確認に使う。

---

## 13. ClaudeCode レビュー反映メモ

2026-06-24 の ClaudeCode 設計レビューで、P0 は無し、P1 として以下を採用した。

- `Ctrl+↑↓` / `Ctrl+PageUp/Down` が synthetic path から実フォルダ DFS に fall-through しないよう、
  サブ展開ビュー専用の no-op 分岐を明記した。
- Backspace / パンくず / アドレスバーが synthetic path の親へ飛ばないよう、
  root 実フォルダへの明示復帰を明記した。
- root / saved_folder は `start_loading_items` より前に退避し、入場ガードを UI と実行関数の
  両方に置く方針を明記した。

P2 は概ね採用し、画像色フィルタは確認ゲート付きで解放、場所 facet の root 相対ラベルと bounded parallel
scan は後続扱いにした。metadata 収集は初期版では維持し、perf で問題が出た場合に遅延化する。
