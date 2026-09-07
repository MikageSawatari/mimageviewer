# ブリーフ: 重複検出 Step 2a — PDF ページ展開と本単位の関係判定 (計測まで)

対象: 「別バージョンの発見」機能の Step 2a。
実装 = Codex Sol / レビュー・検収 = ClaudeCode / 実データ計測 = ClaudeCode。

正本: [docs/duplicate-detection-plan.md](duplicate-detection-plan.md)。
着手前に **§0 (目的)、§6 (本単位)、§13 (レビュー)、§15 (実測)、§16 (目的転換)** を読むこと。
**この機能は削除しない。** 発見と移動のための機能である (§0)。

前 Step: [brief-dupe-step1-signatures.md](brief-dupe-step1-signatures.md) (実装済み)。

作業ツリー: **`C:\home\mimageviewer-dupe`** (branch `duplicate-detection`)。
master では作業しないこと。着手前に `git log --oneline -3` で HEAD を確認する。

**コミットしないこと。** この worktree の git メタデータは
`C:/home/mimageviewer/.git/worktrees/` にあり sandbox の書き込み範囲外で、
広げると別セッションが作業中の master の git 状態に触れてしまう。
ファイルを書き、`cargo fmt` とテストを通したら止まる。**コミットは ClaudeCode が行う。**

---

## 0. なぜこれを先にやるのか

利用者の本は**大半が PDF** で、`d:\home\scan\comic` (178) /
`d:\home\scan\illust` (145) / `e:\share\18\bookscan` (787) の**計 1,110 冊**ある。
§6 が扱おうとしている関係 — **総集編と単話、前後編と統合版、DL版と書籍版** —
は全てここに集中しているのに、Step 1 はルーズな画像しか読めないため
**1 冊も測れていない**。

したがってこの Step の成果物は機能ではなく **測定結果**である。
`similar.db` も UI もまだ作らない。

---

## 1. 作るもの

```
src/dupe/book.rs          本単位の関係判定 (純ロジック・テスト対象)
src/bin/bench_dupe.rs     pdf-scan / pdf-selfcheck / books / books-report を追加
```

`src/dupe/` は引き続き**純ロジック**。PDF を開く I/O は bin 側に置く
(`dupe::book` はページ署名の配列を受け取るだけで、PDF を知らない)。

**範囲外**: `similar.db`、App / UI / 設定、ZIP のページ展開、削除、
全件スイープの製品実装。

---

## 2. bin から PDF を読む — 先に知っておくべき 2 点

### 2.1 `bench_dupe` は `--pdf-worker` を処理しなければならない

`pdf_loader` のワーカープールは `std::env::current_exe()` を
`--pdf-worker` 付きで起動する ([pdf_loader.rs:2258](../src/pdf_loader.rs) 付近)。
`bench_dupe.exe` から使うと **`bench_dupe.exe --pdf-worker` が起動される**ので、
bin の引数解析の**一番先頭**で次を行うこと。処理しないと usage を出して即死し、
プールが無言でタイムアウトする。

```rust
if std::env::args().any(|a| a == mimageviewer::pdf_loader::PDF_WORKER_ARG) {
    mimageviewer::pdf_loader::run_worker_process();
    return;
}
```

[src/lib.rs](../src/lib.rs) の同等処理が先例。

### 2.2 レンダリング API

```rust
pdf_loader::render_page(
    path, page_num, target_px, password,
    None,                                // cancel
    JobPriority::Normal,
    0,                                   // context_epoch: background は 0
    CancelWaitPolicy::AbortOnCancel,
)
```

ページ数は `pdf_loader::get_document_info(path, password)`。
**パスワード必須の PDF はスキップし、件数を集計して出す** (無言で捨てない)。

---

## 3. PDF ページの正準プロキシ — ここが実験の核心

### 3.1 低解像度でレンダリングしてはいけない

プロキシは 64x64 だが、**PDFium に 64px でレンダリングさせてはならない**。
そうすると **PDFium 自身の縮小がハッシュに焼き付き**、同じ内容が
JPEG として存在する場合のハッシュと一致しなくなる。

**固定の長辺 (初期値 1024px) でレンダリングし、縮小は必ず `dupe::proxy::build` に行わせる。**
これで PDF 経路と画像経路が同じ縮小規則を通る。

### 3.2 しかし「一致するはず」で済ませない — `pdf-selfcheck` で測る

上は設計上の主張にすぎない。**測ってから信じる。**
`pdf-selfcheck` モードで次の 2 つを実測し、距離分布を出す。

1. **解像度安定性**: 同じページを長辺 512 / 1024 / 2048 でレンダリングし、
   それぞれのハッシュ間距離を出す。**ここが 0 付近でなければ、
   レンダリング解像度がハッシュを左右しているので設計が壊れている。**
2. **経路間の一致**: 同じページを 1024 でレンダリング → JPEG q95 で保存 →
   **画像経路 (`scan` と同じ道)** でハッシュ → PDF 経路のハッシュとの距離を出す。
   **これが大きければ、PDF ページとルーズ画像は比較できない**ことになり、
   §6 の前提が崩れる。その場合は実装を進めず報告すること。

出力は `synth` と同じ形式の JSONL + `report` で読める形にする。

---

## 4. `bench_dupe pdf-scan`

```
bench_dupe pdf-scan --dir DIR [--recursive] --out books.jsonl
                    [--render-long-edge N] [--limit-books N] [--max-pages-per-book N]
```

- PDF を列挙し、各ページをレンダリングして全 `Algo` の署名を出す
- 1 行 1 ページ: `book_path`, `page_index`, `page_count`, 署名群, `quality`,
  レンダリング寸法, 所要時間 (レンダリングと署名を分けて記録)
- **PDFium はスレッドセーフでない**が、プールがプロセス分離しているので
  `render_page` を並列に呼んでよい。ただしプールサイズは既定のままにする
- `--limit-books` は **§14.3 と同じ理由で先頭 N ではなく等間隔サンプリング**
  (`stride_sample` が既にある)
- パスワード必須・破損・0 ページは件数を集計して最後に出す

## 5. `dupe::book` — 本単位の関係判定 (純ロジック)

入力は「本 ID → ページ署名の並び」。PDF も ZIP も画像フォルダも同じ形に落として渡す。

```rust
pub struct BookPage { pub book: u32, pub index: u32, pub quality: u8, pub sig: Sig }

pub struct Params {
    pub radius: u32,          // ページ一致とみなすハミング距離
    pub max_books_per_page: u32, // K: これを超える冊数に出るページは識別力なし
    pub min_quality: u8,      // これ未満は featureless
    pub coverage_threshold: f32,
}

pub enum Relation { Same, Contains { whole: u32 }, Unrelated, Undecidable }

pub struct BookPair {
    pub a: u32, pub b: u32,
    pub matched: u32,
    pub distinctive_a: u32, pub distinctive_b: u32,
    pub coverage_a: f32, pub coverage_b: f32,
    pub relation: Relation,
    pub alignment: Vec<(u32, u32)>,   // 対応ページ対。§14.2 のページ帯の元データ
}
```

規則 (計画 §6 と §13.1 のとおり):

1. **識別力の判定** — `quality < min_quality` は除外。
   さらに**そのページの半径 r 近傍が何冊にまたがるか**を数え、`K` を超えたら除外。
   **単連結クラスタリングをしない** (§13.1、連鎖で巨大クラスタになる)。
   直接近傍の冊数を数えるだけにする。
2. **除外は分子と分母の両方から**行う (§6.2)。片方だけだと
   クレジットページが 1 枚増えた版が一致率を落とす。
3. **対応付けは集合ではなく順序整合で取る** (§13.1-I)。
   候補ページ対から**重み付き最長単調増加列**を求める。これで
   先頭へのページ挿入に強く、かつ**連続区間が証拠として残る**。
4. **カバー率は両方向**。分類は §6.3 の表のとおり。
5. **識別力のあるページが 0 枚の本は `Undecidable`**。0% でも 100% でもない (§6.5)。

**`Params` の値をコード内で決め打ちしないこと。** 既定値は置いてよいが、
bin の引数で振れるようにする。閾値はまだ確定していない。

## 6. `bench_dupe books` / `books-report`

```
bench_dupe books --in books.jsonl --out relations.jsonl
                 [--radius N] [--k N] [--min-quality N] [--coverage X]
bench_dupe books-report --in relations.jsonl --out report.md
```

`books-report` が出すもの:

- 関係の内訳 (Same / Contains / Undecidable の件数)
- **`Contains` の一覧**: どの本がどの本の何ページ目〜何ページ目に入っているか。
  **総集編・分割・版違いがここに出るはず**で、この Step の主目的である
- カバー率の分布、識別力のあるページ数の分布
- **除外されたページの内訳** (featureless / K 超え) と件数
- `Undecidable` になった本とその理由

## 7. テスト

`cargo test -p mimageviewer --lib dupe::book` で走る単体テストとして、
計画 §12 の 6 ケースを**ページ署名の並びだけで**書く (PDF 不要):

1. 前編 (100p) + 後編 (100p) と統合版 (200p) → **`Contains` 2 件**。
   ページ数の近さで候補を絞る実装に退行したら落ちる
2. クレジットページを 1 枚足した版 → **`Same`** (一致率が下がらない)
3. クレジットページを**先頭に挿入**した版 → **`Same`** (位置合わせ退行で落ちる)
4. 同じクレジットページしか共有しない無関係な 2 冊 → **`Unrelated`**
5. 識別力のあるページが 0 枚 → **`Undecidable`** (0% でも 100% でもない)
6. ページを並べ替えただけの版 → 順序整合を使うので `Same` にはならない。
   **`Same` か別分類かを固定する** (仕様として決め、テストに書く)

加えて:

7. **K を超えるページが分子と分母の両方から外れること** — 分母に残す実装だと
   ケース 2 が壊れるので、それを直接検出するテストを書く
8. **単連結でクラスタが育たないこと** — 鎖状に少しずつ違うページを並べ、
   端と端が同じクラスタに入らないことを確認する

## 8. 完了条件

- `cargo fmt` 済み、`cargo check -p mimageviewer --bin mimageviewer-core` が通る
- `cargo test -p mimageviewer --lib dupe::` が緑
- `cargo build --release --features dev-tools --bin bench_dupe` が通る
- `pdf-selfcheck` が動き、**§3.2 の 2 つの距離分布が出る**
- `pdf-scan` が実 PDF フォルダで動く
- `books` / `books-report` が動く
- 既存アプリの挙動に変更がない (`src/dupe/`、`src/bin/bench_dupe.rs`、
  `src/lib.rs` の宣言行、`Cargo.toml` の範囲に収まっている)

## 9. 判断に迷ったとき

- **閾値を決めない。** 迷ったら bin の引数に出す。
- **silent fallback を作らない。** パスワード PDF・破損・0 ページは
  件数を集計して報告する。
- **§3.2 の測定結果が悪ければ止まる。** PDF 経路と画像経路のハッシュが
  一致しないなら、§6 の設計前提が崩れているので、
  実装を先に進めずに数値を添えて報告すること。
