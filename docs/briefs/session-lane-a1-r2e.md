# セッション指示書: レーン A-1 — R2e (所有の型化) 第 3 版の設計

体制: このセッションは**設計だけ**を行う。実装は第 3 版が合意されてから別ステージで出す。
レーン構成の正本は [next-cycle-work-lanes.md](../next-cycle-work-lanes.md)。

## 0. 先に読む (順番どおり)

1. [detached-rework-plan.md](../detached-rework-plan.md) **§2 (憲法)** — 全ステージ共通の禁止事項
2. 同 **§9.1 (現況表)** — R2b が部分完了、R4 未着手。ステージ判定はこの表だけを使う
3. 同 **§9.5** — R2 残件の再定義と、第 1 版 / 第 2 版が落ちた理由
4. [briefs/detached-r2e-ownership-design.md](detached-r2e-ownership-design.md) **全文**
   (特に §6 の BLOCKER 4 件と §6.5 の「3 回踏んだ同型の失敗」)

## 1. 目的

`ViewerContextBundle` の**所有を型で表す**設計を第 3 版として書き直す。
第 1 版は BLOCKER 5 件、第 2 版は 4 件で破棄されている。**同じ失敗を繰り返さないこと。**

## 2. 第 3 版が説明しなければならないこと

| # | 要求 | 出典 |
| --- | --- | --- |
| 1 | 所有の **transaction**。`begin_build(reserved_id)` → `commit_and_restore_previous`。`Vacant` / `Building` は transaction の内部状態にし、**生 bundle を返す API を作らない** | §6-1 |
| 2 | main をマウントしたまま 2 個目を作る **atomic な fork / insert_unmounted** | §6-1 |
| 3 | 終端削除は **drop 前に中身を読む**必要がある (bookmark 照合 / media teardown)。「アンマウント後に slot を消す」では足りない | §6-1 |
| 4 | **`window_id → ViewerContextId` の対応表をどこに置くか**。window_id からは推論できない (フォルダ再オープンで再利用される) | §6-2 |
| 5 | **コンパイルできるステージ分割**。①registry の状態機械 + build transaction を production の保管を切らずに定義・テスト → ②保管・owner 参照・生プリミティブ・終端 teardown・直接消費者を**一括で**切替 → ③手書き巡回の単純化 → ④非同期要求の identity 変換 | §6-3 |
| 6 | 「Building には identity が無い」は**誤り**。serial は load 開始前に払い出される。正しくは**予約済みだが未 commit** | §6-4 |
| 7 | ある識別子 (window / context / owner) を渡したら、**その bundle が今どこにあるか** (parked / mounted / 別 producer が保持中) を**型で返す**一級の問い合わせ | §6.5 |

§6.5 の 3 件 (keep-alive backstop / `right_drag_pointer_pos` / `paused_bundle.is_none()`) は
すべて「`None` が『存在しない』ではなく『今は別の場所にある』を意味していた」ケース。
**第 3 版はこの 3 件を一つの説明で覆えること。** 覆えないなら設計が足りない。

## 3. 完了条件の考え方

- 完了確認は **grep ではなくコンパイラ + AST allowlist**。生成 / exhaustive destructure /
  生の抽出 / 所有を動かす `mem::swap` を registry モジュール private にし、`syn` ベースの
  CI 監査でモジュール外の `ViewerContextBundle` 生成・返却・保管を弾く方針 (§6-3)。
  第 3 版はこの監査が**何を許可し何を弾くか**を具体的に書くこと。
- 設計文書に「実装ステージごとの、その時点でコンパイルが通る状態」を明記する。

## 4. スコープ外

- **実装しない。** コードを変更するのは第 3 版が合意された後。
- **§1.115 / §2.20 (レーン A-0) には触らない。別セッションが進行中**
  (`af2a1673` / `97d1ee98` / `2dc50cbd` で原因を「最大化中の placement が実ジオメトリを
  表していない」まで絞り込み済み)。**その成果は第 3 版の入力として読むこと** — placement の
  現在ジオメトリ / restore ジオメトリの分離は、所有の型化と同じ「1 個の値に 2 つの意味を
  同居させた」問題である。
- 症状パッチを入れない (憲法)。

## 5. 運用

- master の共有作業ツリーで進む。設計段階はコードを触らないので A-0 と衝突しない。
- コミットは pathspec commit (`git commit -- docs/briefs/detached-r2e-ownership-design.md`)。
  `git add -A` は使わない。
- 第 3 版が書けたら Codex にレビューを出し、**BLOCKER 0 になるまで往復**する。
  同一タスクの往復は `codex exec resume --last` で同じセッションを継続する。

## 6. 出口

第 3 版が合意されたら、§6-3 のステージ①の実装指示書を
`docs/detached-rework-stage-r2e-1.md` として起こす。
