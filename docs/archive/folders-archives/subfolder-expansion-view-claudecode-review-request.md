# ClaudeCode レビュー依頼: サブフォルダ展開ビュー設計

Reviewer target: ClaudeCode

目的: `サブフォルダ展開ビュー` の実装前設計レビュー。コード実装はまだ行わず、設計上の抜け、
既存アーキテクチャとの衝突、性能・キャンセル・UI状態遷移のリスクを洗い出してください。

---

## 対象ドキュメント

主対象:

- `docs/subfolder-expansion-view-plan.md`

関連:

- `docs/next-release-backlog.md` の `2.1 サブフォルダ展開ビュー (スナップショット方式)`
- `docs/ui-responsiveness.md`
- `docs/async-architecture.md`
- `docs/details-view-and-filter-plan.md`
- `docs/virtual-folders.md`
- `docs/rating-list-view-plan.md`
- `docs/filename-stack-plan.md`

必要なら実コードも確認してください:

- `src/app.rs`
- `src/grid_item.rs`
- `src/folder_tree.rs`
- `src/ui_main.rs`
- `src/thumb_loader.rs`
- `src/filename_stack_ui.rs`
- `src/global_search_ui.rs`

---

## 背景

Eagle for Windows には「サブフォルダの内容を表示」があり、任意フォルダ配下の画像を
フラットに表示して、タグやレーティングで整理できます。mIV は Eagle のような常時カタログ型ではなく、
直接ファイルシステムを読むビューアなので、初期版は索引や watcher 追従を使わず、ユーザーが
`サブ展開` を押した時点のスナップショットとして実装する方針です。

現在の設計判断:

- 初期版は通常ファイルシステム上の画像/動画だけを対象にする。
- ZIP/PDF/変換アーカイブの中身は初期対象外。
- watcher / 自動更新 / 更新ボタンは初期版では持たない。
- `GridItem::Image` / `GridItem::Video` の既存 variant だけで表現し、新しい `GridItem` variant は作らない。
- synthetic view として扱い、既存の詳細表示、★、タグ、場所 facet、Ctrl+F、ファイル操作を流用する。
- UI スレッドで `read_dir` / metadata / recursive scan をしない。

---

## 特にレビューしてほしい論点

### 1. 並列スキャンの是非

展開時の再帰スキャンは、SSD/NVMe や多数サブフォルダでは並列化で高速化できそうです。一方で、
HDD、ネットワーク共有、クラウド同期フォルダでは、並列 I/O が逆に遅くなったり UI 系ワーカーと
競合したりする可能性があります。

現時点の候補:

- `std::thread` の短命大量 spawn は避ける。
- ディレクトリ単位の work queue + 小さな固定 worker pool にする。
- worker 数は小さく抑える。例: 2〜4、または `available_parallelism` と I/O profile から上限を決める。
- `GlobalIoSemaphore` を使い、他のサムネイル / PDF / インデクサ I/O と奪い合わない。
- cancel token を各 directory pop / read_dir loop / result push で確認する。
- 共有 result Vec へ逐次 lock するより、worker local buffer を最後に merge する。
- HDD / network で遅い場合に備え、初期実装は single-thread worker から始め、計測後に bounded parallel へ進める案もあり得る。

確認したいこと:

- 初期版から bounded parallel scan を入れるべきか、それとも single-thread worker + perf 計測を先にすべきか。
- `GlobalIoSemaphore` の priority は Normal でよいか。インデクサと同じ Low に寄せるべきか。
- 並列化する場合、既存の `search_walker` / folder pane scan / name indexer から流用できる構造はあるか。
- reparse point / symlink / junction の visited 管理は並列化でどのように持つべきか。
- result sort / duplicate filter を worker 側で済ませるべきか、UI install 時に済ませるべきか。

### 2. synthetic view の lifecycle

以下が既存の閲覧履歴 / レーティング一覧 / Ctrl+G 検索結果 / スタック表示と矛盾しないか確認してください。

- `items_are_subfolder_expansion_view` の追加位置
- `subfolder_expansion_synthetic_path()` を `is_synthetic_view_path` に足す方針
- Backspace / トグルOFF / パンくずで root 通常表示へ戻る方針
- フォルダ移動、検索開始、ZIP/PDF open、アプリ終了時の pending cancel
- `start_loading_items` を使う場合に、既存の fullscreen close / cache clear / zip_nav clear / stack state clear が
  過剰に働かないか

### 3. サムネイルキャッシュと existing key

合成ビューでは複数フォルダの実ファイルが混ざるため、サムネイル cache key と cleanup が重要です。

確認したいこと:

- full path cache key を強制する必要があるか。
- `existing_keys` に何を渡すべきか。
- 実フォルダ単位の `delete_missing` が、サブフォルダ側のサムネイルを誤削除しないか。
- 動画サムネイル / folder thumb pin / `folder_pin_map` との関係に問題がないか。

### 4. フィルタ / 場所 facet / 詳細表示

初期版の価値は、`場所` facet と既存詳細表示をそのまま使えることです。

確認したいこと:

- `FacetFilter` が synthetic view に対して期待どおり動くか。
- `場所` facet の表示を root 相対に寄せるべきか、既存の親フォルダ表示で十分か。
- `rating_filter` / `items_are_rating_view` の特別扱いと衝突しないか。
- 画像色フィルタや遅延列 worker をサブ展開ビューで許可してよいか、初期版では制限すべきか。

### 5. ファイル操作後の整合性

対象は実ファイルなので、★ / タグ / rename / delete / drag out / A-B quick move などを許可する想定です。

確認したいこと:

- delete / move / rename 成功後に該当行だけをメモリ上から除去・更新する方針で足りるか。
- Shell native menu など結果追跡が難しい操作では、再走査なしの自然整合でよいか。
- Undo/Redo や rating/tag cache invalidation の既存経路に追加対応が必要か。

### 6. フルスクリーンナビゲーション

サブ展開ビューのフラット順でページ送りする想定です。

確認したいこと:

- Ctrl+↑↓ を通常フォルダ移動に出さず、合成ビュー内では no-op または root 通常表示へ戻す案は妥当か。
- フルスクリーン中にファイル操作や検索/フィルタ変更が入った場合の idx stale 対策。
- detached viewer / 動画 native presenter に追加リスクがあるか。

---

## 期待するレビュー出力

以下の形式でお願いします。

1. P0/P1/P2/P3 の findings。可能なら該当ファイル・節・想定コード境界を示す。
2. `parallel scan` について、初期版に入れるべきか / 後続にすべきかの推奨。
3. 実装前に設計へ追記すべき決定事項。
4. 実装時に最初に作るべきテスト。
5. 問題なしなら、その旨と残る実測リスク。

レビューでは、単なる好みよりも以下を優先してください。

- UI スレッドを止めないこと。
- キャンセルと stale result 破棄が明確であること。
- 既存 synthetic view / fullscreen / thumbnail cache の不変条件を壊さないこと。
- 初期版のスコープが大きくなりすぎないこと。

---

## Codex 側の暫定見解

並列スキャンは高速化の余地がありますが、初期版で無理に入れるより、まず single-thread worker で
perf event を入れて実測し、必要なら bounded worker pool へ進める方が安全かもしれません。
ただし、サブフォルダ数が多いSSD環境では single-thread が体感で遅い可能性もあるため、
ClaudeCode にはこの判断を重点的に見てほしいです。
