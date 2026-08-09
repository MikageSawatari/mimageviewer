# ブリーフ: §3.3 フルスクリーンの idx キャッシュに一覧世代の刻印が無い

対象: v2.13.0 出荷スコープ。実装 = Codex Sol / レビュー・検収 = ClaudeCode /
実機確認 = 利用者。

正本: [docs/next-release-backlog.md](next-release-backlog.md) §3.3。**調査済み**なので、
そこに書いてある方針どおりに実装する。

前提: master。着手前に `git log --oneline -3` で HEAD を確認すること。

---

## 1. 何が問題か (調査済み・再調査不要)

報告 (2026-08-08): PDF を読んでいるとき、2 ページ目に**直前まで開いていた別の PDF の
ページ**が表示された。**再現していない**が、構造の穴は確認済み。

- `fs_cache` / `fs_early_dims` / `fs_upload_backlog` / `fs_pending` は **index だけをキー**にし、
  どの item 一覧のものかを記録していない
- 完了適用 (`poll_fs` 系の `for (key, mut result, load_seq) in completed`) も index だけで行う。
  一緒に運ぶ `load_seq` は perf ログ対応用で、照合には使っていない
- 一括で捨てるのは `close_fullscreen` / `invalidate_idx_state_and_queues` /
  `enter_drive_list` の 3 か所だけ
- **フォルダ・コンテナ読み込みが必ず通る `install_new_items` は `items_generation` を進める
  だけで、これらに触れていない**
- 読み出し側 (`fs_cache.get(&idx)` は 57 か所) にも世代照合が無い

つまり「index N のテクスチャが現在の一覧のものである」ことが、**item を差し替える場所が
全部クリアを覚えていること**に依存していて、構造では保証されていない。

**どの経路が実際に漏れているかは未特定。** 穴が開いていることまでしか分かっていない。

## 2. 方針 (バックログで確定済み)

**経路を探して個別にクリアを足すのではなく、entry に `items_generation` を刻み、
読み出し・適用時に照合する。** 世代違いは現在の一覧のものではないと確定できるので、
**記録したうえで捨てる**。欠けている識別子を足す修正であって、症状隠しではない。

### 2.1 同じ手が今日入っている (参考にすること)

§1.61 で入れた `PageDimsCache` ([src/page_dims.rs](../src/page_dims.rs)) が同型。
generation を持ち、`get` は不一致なら `None` を返す fail-closed。**同じ考え方で揃えること。**

⚠ ただし `items_generation` は **viewer context ごとのカウンタ**で、context 間で番号は
比較できない (`ViewerContextBundle` の `facet_name_cache` のコメント参照)。
`fs_cache` 等は既に bundle 所有なので、**同じ bundle の中で完結する照合**であれば問題ない。
新たに App global へ移さないこと。

### 2.2 対象

- `fs_cache`
- `fs_early_dims`
- `fs_upload_backlog`
- `fs_pending`

読み出しが 57 か所あるので、**個々の `get` に照合を書き足すのではなく、
照合込みのアクセサへ寄せる**こと。生の `HashMap` を直接触れないようにできると、
次に増える読み出しも自動で守られる。**そこまでの構造にできるかは判断してよいが、
判断した内容を報告すること。**

### 2.3 捨てるときは記録する

世代違いを検出したら `crate::logger::log` で 1 行残す (idx / 期待世代 / 実際の世代 /
どのキャッシュか)。**再現しなくても穴は塞がり、再発時は記録が残る**のが今回の狙い。
perf event ではなく通常ログでよい (頻度が低い前提)。もし高頻度で出るなら、それ自体が
別のバグの発見なので**報告すること**。

## 3. やらないこと

- 症状を隠すための一括 clear の追加や、`install_new_items` への clear の追加だけで済ませること
  (それでは「全部の差し替え経路が覚えている」依存が残る)
- 世代不一致を「たぶん大丈夫」で使うこと。**必ず捨てる**
- 読み出し側 57 か所の意味を変えること。世代が一致する通常ケースの挙動は完全に同じにする

## 4. テスト

1. 世代付きアクセサの単体テスト: 一致で `Some`、不一致で `None`、世代更新で捨てられる
2. **状態遷移テスト**: fullscreen を開いたまま `install_new_items` 相当で items を差し替え、
   同じ idx の読み出しが**旧世代の entry を返さない**こと
3. 完了適用の照合: 旧世代の load 完了が着地しても、新しい一覧の同 idx へ**適用されない**こと
   (これが報告の症状そのもの)

## 5. 完了条件

- `cargo fmt` (引数なし)
- `cargo test -p mimageviewer --lib` が全件 / `cargo test -p mimageviewer --test ui_snapshot`
- `cargo check -p mimageviewer --bin mimageviewer-core`
- `python scripts/check_ui_glyphs.py` が 0 件
- 非 Windows を壊さないこと
- **バックログ §3.3 に実装記録を追記**する
- bundle に触れるので [docs/detached-rework-plan.md](detached-rework-plan.md) §2 を読み、
  触れた範囲を同 plan の表へ記録する

## 6. 制約

- **アプリを起動しないこと。** 検証ビルドと実機確認は ClaudeCode と利用者が行う
- **ブランチ操作・コミットをしないこと。** master の作業ツリーで未コミットのまま残す

---

完了したら次を報告すること:

1. 照合をどこへ集約したか。生 `HashMap` への直接アクセスを塞げたか (塞げないなら理由)
2. 世代不一致を捨てる箇所と、記録の書式
3. detached bundle 所有のままである根拠
4. テスト結果
5. **実機で確認してほしいこと** (再現しない不具合なので、通常操作の退行確認が中心になるはず)
