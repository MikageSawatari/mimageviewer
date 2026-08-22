# A4: 台帳スキーマの移行と、既存編集の遡り記録

**正本は [docs/edit-content-identity-plan.md](../edit-content-identity-plan.md)。**
A1 / A2 / A3a / A3b は実装済み。**このブリーフは、その 4 段で作り込んだ 2 つの欠陥を直す。**

## 1. 観測された失敗

利用者報告 (2026-08-22) の再現手順:

1. `h:\home\mimageviewer_old\testimage\ChatGPT Image ... (2).png` を開いて編集が反映されているのを確認
2. 一覧で Ctrl+C
3. `h:\home\mimageviewer_old\testimage\y` を開いて Ctrl+V
4. 一覧は更新されるが、**復元ウィンドウが出ない**

ログ (`%APPDATA%/mimageviewer/logs/mimageviewer.log`) に 1 行だけ:

```
content_identity: detection index load failed:
  no such column: has_restorable_content in SELECT file_key, size, head_hash, ...
```

実機の `content_identity.db` を読んだ結果:

```
columns: file_key, size, head_hash, full_hash, hashed_mtime, kind, last_edit_at
rows   : 14        (最終書き込み 2026-08-21 14:51 = A1 ビルド時)
```

## 2. 欠陥 1 — `CREATE TABLE IF NOT EXISTS` では列は増えない

A2 の修正で `has_restorable_content` 列を**正本スキーマの `CREATE TABLE IF NOT EXISTS` に
足しただけ**だった。既存テーブルには何もしないので、**A1 ビルドを一度でも動かした環境では
列が存在しない**。その結果:

- 起動時の index load が失敗 → **検出が完全に死ぬ**
- `upsert` も失敗 → **記録も死ぬ** (DB の最終書き込みが A1 時点で止まっている)

**そして症状は起動時のログ 1 行だけ。** 利用者から見れば「ウィンドウが出ない」としか分からない。

> 「未リリースだから migration 不要」と判断したのは**利用者環境については正しい**が、
> **開発機は A1 ビルドを動かしていて実データを持っていた**。未リリースの store でも、
> **自分がそのビルドを走らせた時点で移行対象が生まれる**。

### 直し方

- **スキーマの用意を 1 つの関数に集約し、過去の形から現在の形へ上げる。**
  `PRAGMA user_version` で版を持つ。
- 今回の具体的な移行: `has_restorable_content` が無ければ
  `ALTER TABLE edit_origin ADD COLUMN has_restorable_content INTEGER NOT NULL DEFAULT 1`。
  **DEFAULT は 1 が正しい** — A1 時代の行はすべて実際の記録 (Edit / ViewingState) から
  生まれており、それはまさに復元元だから。検出 cache 行は A1 には存在しない。
- **スキーマを期待する形にできなかったときは、無言で空 index にしない。**
  「台帳が使えない」ことが型で表現され、テストできること。ログ 1 行に落とさない。
  既存の PDF ワーカー notice ([ai/runtime.rs](../../src/ai/runtime.rs) →
  [ui_dialogs/](../../src/ui_dialogs/) の型付き notice) と同じ形が使えるなら使う。
  **新しい汎用イベントバスは作らない。**

### テスト

- **A1 の形 (列なし・行あり) の DB を作って開き、列が増え、既存行が `1` になること。**
- 現行の形の DB を開いても壊れないこと (冪等)。
- スキーマを用意できないとき、検出が「候補 0 件」ではなく**失敗として観測できる**こと。

## 3. 欠陥 2 — 既存編集の遡り記録が実装されていない

正本 §3.3 は次を明記している:

> **既存編集の遡り (backfill) は一括スキャンしない**。「編集を持つページを含むフォルダを
> 開いたときに、台帳に無いものだけ裏でハッシュする」で自然に埋まる。

**この後半が実装されていない。** 記録の起点は編集の確定点 22 箇所だけで、
**機能が入る前から編集を持っているファイルは永久に台帳に載らない**。

正本自身が開発機の実測として **編集を持つ物理ファイルは 921 件** と書いている。
つまり現状の機能は、**入れた後に編集した物しか救えない**。上の再現手順が失敗したのも、
コピー元の PNG に台帳の行が無いからで、欠陥 1 を直しても**これだけでは直らない**。

### 直し方

**物理フォルダを開いたとき、次の条件を満たす項目を記録キューへ入れる:**

1. その項目の編集キーが**メモリ上の presence 集合**にある
   (`adjusted_page_keys` / `mask_page_keys` / `conceal_page_keys` /
   `local_adjust_page_keys` / `comic_page_keys`。ZIP / PDF は `<容器キー>::` prefix)
2. その物理ファイルが**台帳に無い**

- **A2 の検出と同じ土俵に乗せる**: `GlobalIoSemaphore` の `IoPriority::Low`、
  フォルダ切替でキャンセル、UI スレッドから I/O しない。
- **一括スキャンをしない。** 開いたフォルダの範囲だけ。§3.3 のとおり数セッションで埋まる。
- **trigger は `ViewingState`。** 記録経路を通るので `has_restorable_content` は 1 になり、
  `last_edit_at` は 0 (= 編集時刻不明) のまま残る。**これが正しい** — 実際にいつ編集したかは
  分からないので、時刻が分かっている候補より後ろに回るのが正直な既定順。
  **新しい trigger を足さない。**
- **設定 OFF のときの扱いを決めて報告すること。** §7 は「OFF = 照合のためのファイル読み取りを
  一切行わない」であり、backfill はまさにファイル読み取りである。一方 §7 は「記録側は OFF でも
  継続する」とも言っている。**この 2 つが衝突するのは backfill だけ**なので、
  どちらに倒すか決めて正本へ書き戻す (推奨: OFF のときは backfill もしない。
  文言が「読み取りを一切行いません」と約束しているため)。

### テスト

- 編集を持つが台帳に無い項目が、フォルダを開いたときにキューへ入ること。
- **台帳に既にある項目は入らない**こと。
- 編集を持たない項目は入らないこと。
- ZIP / PDF は容器単位で 1 件だけ入ること (ページごとに何度も入らない)。
- フォルダ切替でキャンセルされること。
- 上で決めた設定 OFF 時の挙動。

## 4. 直った後に通ること

**§1 の再現手順が通ること。** 具体的には:

1. 編集を持つファイルがあるフォルダを開く → backfill でそのファイルが台帳に載る
2. そのファイルを別フォルダへコピーする
3. コピー先を開く → 復元ウィンドウが出る

**A3b までのテストは全部緑のままであること。**

## 5. 制約

- **時間窓・sleep・retry で吸収しない。**
- **一括 backfill をしない。**
- 既存の A1 / A2 / A3a / A3b の**挙動を変えない** (スキーマ移行と backfill の追加だけ)。
- 新しい設定項目を足さない。

## 6. 完了条件

- `cargo fmt` 済み / `cargo test -p mimageviewer --lib` が緑
- `cargo test --test ui_snapshot` が緑
- `cargo check -p mimageviewer --bin mimageviewer-core` が通る
- **報告に、設定 OFF 時の backfill の扱いと、その理由を書く**
