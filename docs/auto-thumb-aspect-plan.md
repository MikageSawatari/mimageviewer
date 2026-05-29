# サムネイル比率の自動選択 — 実装計画

フォルダを開いた時点ではアイテム個別の縦横比情報がほぼ無いという前提で、
サムネイル読み込みが進むにつれて統計的に最適なグリッドセル比率を選び、
**ちらつかずに少ない回数で**確定させる機能を追加する。

実装に入る前のレビュー用ドキュメント。コードは未着手。

## 1. 背景・目的

現状、グリッドのセル比率 (`ThumbAspect`) は 7 段階 (16:9 / 3:2 / 4:3 / 1:1 /
3:4 / 2:3 / 9:16) からユーザーが手動で選ぶ。漫画 ZIP は 2:3 / 9:16、写真は 3:2、
動画は 16:9 が向くが、フォルダごとに毎回手動切替するのは面倒。

「フォルダの中身に合わせてグリッドセル比率を自動で選ぶ」モードを追加する。
ユーザー要望:

- 起動時は手がかりが無いので 1:1 (Square) から始まる挙動でよい
- サムネイル読み込みが進むにしたがって、徐々に適切な比率へ寄せていく
- **頻繁にパカパカ切り替わらないこと** が最重要 (UX 上の根本要件)

## 2. 現状コードの把握

| 項目 | 場所 |
| --- | --- |
| `ThumbAspect` enum (7 段階) | `src/settings.rs:171-208` |
| `Settings::thumb_aspect` フィールド | `src/settings.rs:527` |
| メニュー UI (手動切替) | `src/ui_main.rs:569-579` |
| ツールバー UI (手動切替) | `src/ui_main.rs:906-908` |
| セル寸法計算 | `src/ui_main.rs:102-110` (`compute_cell_size`) |
| 描画呼び出し点 | `src/ui_main.rs:2005-2006` (`height_ratio()` を渡す) |
| `ThumbnailState::Loaded { source_dims }` | `src/grid_item.rs:303-324` |
| カタログ DB 上の元寸法保存 | `src/catalog.rs:36` (`CacheEntry::source_dims: Option<(u32, u32)>`、DB column としては `source_width / source_height`) |
| WebP / JPEG ヘッダ寸法読み取り | `src/catalog.rs:44` (`decode_thumb_dims`) |
| サムネ受信ハンドリング | `src/app.rs:9409-` (`poll_thumbnails`) |
| フォルダロード時の cache_map 読込 | `src/app.rs:7248-` (`start_loading_items`) |
| 元画像 → ColorImage デコード時に `source_dims` 取得 | `src/thumb_loader.rs:1941` |
| セル内 letterbox fit 描画 | `src/app.rs:20524-20557` (`draw_thumb_texture`) |

**重要な気付き**:

- `source_dims` は既に「新規デコード」「カタログ復元」両経路で付いている。
  集計に必要なデータ供給は **追加 worker 不要**。
- 旧キャッシュで `source_dims = None` のものも、保存済み WebP の
  ヘッダだけ `decode_thumb_dims` で読めば比率は得られる (フルデコード不要)。
- セル比率が変わっても、セル内の画像描画はアスペクト保持 letterbox fit。
  **元画像は歪まず、余白の入り方が変わるだけ**。

## 3. 設計判断 (ユーザー確定済み)

| # | 論点 | 決定 |
| --- | --- | --- |
| A | デフォルト挙動 | **既存ユーザーは Manual を維持**。比率メニュー先頭に「自動」項目を追加してオプトイン |
| B | 動画を集計に含めるか | **含める**。動画 Shell サムネの `source_dims` (or ColorImage 比率) を使う |
| C | 縦長・横長混在フォルダ | `log(ratio)` の中央値を最近接バケットに割り当てる方式により自然に 1:1 へ収束。OK (§4.2 参照) |
| D | ヒステリシス幅 | 実機で調整。初期値: log 距離マージン 0.10、同候補勝利継続 750ms、最大 2 切替/folder |
| E | ZIP / PDF を開いている階層 | そのアイテム自身 (`ZipImage` / `PdfPage`) の比率で集計 |
| F | PDF / ZIP ファイルが並ぶ階層 | **代表サムネ (1 ページ目 / 最初の画像) の `source_dims`** をそのまま使う |

E と F は実装的には同一の処理になる: 「`source_dims` が `Some` か、または
ColorImage サイズが取れる Loaded サムネは全部集計に入れる」だけ。種別分岐は不要。

## 4. アルゴリズム

### 4.1 集計対象

| `GridItem` 種別 | 集計 | 比率の出所 |
| --- | --- | --- |
| Image | ✓ | 自身の元画像 |
| ZipImage / PdfPage | ✓ | 開いている階層のアイテム自身 |
| Video | ✓ | Shell サムネの HBITMAP 寸法 (元動画フレーム比とは限らない、詳細: §4.1.1) |
| ZipFile / PdfFile / Folder | ✓ | 代表サムネ (1 ページ目 / 最初の画像) の `source_dims` |
| ZipSeparator | ✗ | サムネ無し |

`source_dims` の解決は次の優先順:

1. `ThumbnailState::Loaded { source_dims: Some((w, h)), .. }` — 第一候補
2. catalog 内に row があり `CacheEntry::source_dims = Some((w, h))` ならそれ
   (`cache_map` 経由でアイテム反映時に既に取れる)
3. catalog の WebP blob を `decode_thumb_dims` でヘッダ読み (旧 cache 救済)
4. 受信した `ColorImage` の `(w, h)` をそのまま比率ヒントとして使う
   (アスペクト保持リサイズ済みなので比率は保たれる)

### 4.1.1 動画サムネの経路

⚠️ 重要: `VideoThumbDiag::dims` は **元動画のフレーム比ではなく、Shell が
返した HBITMAP の寸法** ([src/video_thumb.rs:147-155](../src/video_thumb.rs))。
`GetObject(hbmp)` の `bmWidth / bmHeight` をそのまま入れているだけで、Shell
側が letterbox 付きの正方形画像を返せばその外形になる。本当に元動画の比率が
欲しい場合は別途 ffprobe / MediaFoundation 等でメタデータを取る必要があり、
今回のスコープでは過剰。

そこで方針:

- **動画は「Shell サムネの比率」として集計する** (= 動画ファイル本来の比率
  ではない可能性を受け入れる)。Shell が letterbox しない / または素直に
  リサイズする一般的なケースでは元比率に近づく。
- 実装は **`VideoThumbDiag::dims` を `ThumbMsg::source_dims` に伝搬** する。
  これが付いていない場合は `ColorImage.size` で代替。
- 実機確認 (§9.4): 16:9 動画オンリーフォルダで本当に Landscape16x9 が選ばれる
  かは Shell の挙動次第。期待外れなら §10 の「動画を集計対象から外す
  フラグ」オプションに退避する。

### 4.1.2 初期シード段階の走査ルールと I/O 予算

⚠️ `cache_map` は `catalog::load_all()` 結果なので **stale なエントリ
(旧ファイル名のレコード等) を含む** ([src/app.rs:7264-7272](../src/app.rs))。
`delete_missing()` は DB 側の掃除であって `cache_map` ハッシュマップは更新
しない。**`cache_map` を直接 for-each すると消えたファイルの比率が混入する**。

正しい走査: **`self.items` を idx 順に走査し、各 item の有効 cache key で
`cache_map` を引く**。

cache key の組み立ては:

- **`make_load_request`** ([src/app.rs:20040](../src/app.rs)) が item 種別ごとに
  `LoadRequest` を組み立てる (`cache_key_override` / `pdf_page` / `zip_entry`
  などのフィールドを埋める)。Container 系は `container_cache_base_key` と
  `apply_folder_thumb_pin` 経由でピン考慮済みのキーが入る
- **thumb_loader.rs:582-593** が `LoadRequest` から実際の DB 検索キー (filename)
  を取り出す: 優先順は `cache_key_override` → `pdf_page_cache_key(page_num)` →
  `zip_entry` → `path.file_name()` というチェーン

**この filename pick ロジックを `thumb_loader.rs` から独立した pub fn に切り
出し、seed フェーズとワーカーの両方から呼ぶ**。新規にキー組み立てを書き
起こさない (組み立てが二重化すると ZipFile / PdfFile / ピンの場合分けで
必ずずれる)。

```rust
// 提案: thumb_loader.rs に追加
// 既存ワーカーは「key 取れなければ空文字 fallback」だが、seed phase からも呼ぶため
// **Option を返して呼び出し側で skip** する形にする (= 事故防止)。
pub fn cache_key_for_request(req: &LoadRequest) -> Option<Cow<'_, str>> {
    if let Some(ref key) = req.cache_key_override {
        Some(Cow::Borrowed(key.as_str()))
    } else if let Some(page_num) = req.pdf_page {
        Some(Cow::Owned(crate::grid_item::pdf_page_cache_key(page_num)))
    } else if let Some(ref name) = req.zip_entry {
        Some(Cow::Borrowed(name.as_str()))
    } else {
        req.path
            .file_name()
            .and_then(|n| n.to_str())
            .map(Cow::Borrowed)
    }
}
```

既存 thumb_loader.rs:584-593 のローカル変数組み立てもこの関数経由に置き換える
(リファクタを一緒に入れる)。既存挙動の `""` fallback は呼び出し側で
`.unwrap_or(Cow::Borrowed(""))` してマイグレーションする。

```rust
// 疑似コード (seed フェーズ)
// 1) read guard 内では集計データを Vec に貯めるだけにし、guard を落としてから
//    record_aspect_sample / 即決判定を行う (borrow と lock の取り回しが楽になる)。
// 2) cache hit 判定は worker と同じく `req.mtime` / `req.file_size` の一致を
//    必須にする (thumb_loader.rs:618 と同じガード)。これが無いと「同じ
//    filename のまま中身が更新されたファイル」で古い source_dims が統計に混ざる。
//    ⚠️ `make_load_request` は container pin 経由で leaf 側の mtime/file_size に
//    書き換えることがあるため、image_metas の生値ではなく **構築後の `req`** を
//    比較対象に使うこと (= worker と同じ参照点)。
// 3) `staged.len() >= min_samples` 到達で早期 break。probe (decode_thumb_dims) は
//    予算 `min_samples * 2` でガード。Some ヒット (HashMap 参照のみ) は zero cost
//    なので予算消費しない。
let min_samples = min_samples_for(eligible_total);
let mut staged: Vec<(usize, f32)> = Vec::new();
let mut probe_budget: usize = min_samples.saturating_mul(2);
{
    let map = cache_map.read().unwrap();
    for (idx, item) in self.items.iter().enumerate() {
        if staged.len() >= min_samples {
            break;  // §4.1.2 の予算: 到達したら seed 終了
        }
        // mtime / file_size は self.image_metas[idx] から取る (Vec<Option<(i64, i64)>>)。
        // None の item (= meta 取得失敗、または Video) は seed ではスキップ。
        let Some((mtime, file_size)) = self.image_metas.get(idx).and_then(|m| *m) else {
            continue;
        };
        let Some(req) = make_load_request(item, idx, mtime, file_size, /* ... */) else { continue };
        let Some(key) = thumb_loader::cache_key_for_request(&req) else { continue };
        let Some(entry) = map.get(key.as_ref()) else { continue };
        // ⚠️ 必須: worker と同じ cache hit 判定 (req 側の値で比較、§注意書き参照)。
        if entry.mtime != req.mtime || entry.file_size != req.file_size {
            continue;
        }
        let ratio = match entry.source_dims {
            Some((w, h)) if w > 0 => h as f32 / w as f32,
            Some(_) => continue,
            None if probe_budget > 0 => {
                probe_budget -= 1;
                // 旧 cache 救済: メモリ上の WebP/JPEG ヘッダだけ読む (追加 I/O なし、§10 参照)
                let Some((w, h)) = catalog::decode_thumb_dims(&entry.jpeg_data) else { continue };
                if w == 0 { continue; }
                h as f32 / w as f32
            }
            None => continue,  // 予算切れ
        };
        staged.push((idx, ratio));
    }
}  // ← ここで read guard が落ちる
for (idx, ratio) in staged {
    self.record_aspect_sample(idx, ratio);
}
self.maybe_apply_auto_aspect();  // seed 直後に即決判定
```

**動画の経路**: `Video` アイテムは `image_metas[idx] = None` で、かつ
`make_load_request` も `None` を返す (動画は通常の thumb worker ではなく
別経路でサムネ取得)。**初期 seed では動画は対象外**で、`poll_thumbnails` が
動画サムネ完了通知を受け取った後の `record_aspect_sample` 経路で集計される。
seed フェーズで「動画も拾えるはず」と思って分岐を足さないこと。

⚠️ `CacheEntry::source_dims` は `Option<(u32, u32)>` という **1 フィールド**
[src/catalog.rs:36](../src/catalog.rs) (`source_width` / `source_height` の
2 つではない)。byte data は `entry.jpeg_data: Vec<u8>` ([src/catalog.rs:33](../src/catalog.rs))。
旧バージョンが JPEG で保存していたエントリも auto-detect で読めるので
WebP / JPEG 混在で問題ない。

予算:

- まず `cache_map` lookup で `CacheEntry::source_dims = Some(..)` が取れる
  分だけ拾う (= 純粋な HashMap 参照、ゼロコスト)。これだけで `min_samples`
  到達することが大半。
- **`min_samples` に達した時点で seed 終了 & 即決判定**。それ以上は読まない。
- 不足する場合のみ、最大 **`min_samples * 2` 件** までを上限に
  `decode_thumb_dims(&entry.jpeg_data)` でヘッダ寸法を probe する。
  **これは追加ファイル I/O ではなく、`catalog.load_all()` で既にメモリに
  読まれた `Vec<u8>` 上の数バイト header decode** ([src/catalog.rs:77-110](../src/catalog.rs)
  で `thumb_data` を `CacheEntry::jpeg_data` として読み込み済み)。
  CPU コストのみで、1 件あたり 1ms 未満を想定 (実測すべき)。
- それでも不足する (= 完全に旧 cache のフォルダ) 場合は seed を諦め、Square
  で描画開始 → 後続の `poll_thumbnails` で段階確定に任せる。

追加 I/O は発生しないため UI スレッド応答性への影響は CPU 時間のみで判断する。
全件 (数千件) で probe しても 1 秒未満で済む見込みだが、念のため
`min_samples * 2` で打ち切る。それでも UI が引っかかる兆候があれば
[docs/ui-responsiveness.md](ui-responsiveness.md) §4 のチェックリストに従って
worker 化する。詳細は §10 オープン論点。

### 4.2 候補選定アルゴリズム

⚠️ **2026-05 実装着手時に方式変更**: 設計レビュー段階では「フィット指標
`min(r/a, a/r)` の平均最大化」を採用していたが、単体テストで「半数 r=0.5 +
半数 r=2.0」のような **bimodal 対称分布で Landscape16x9 / Portrait9x16 が
選ばれる**ことが判明 (両端バケットで「合う側 0.889」が「合わない側 0.281」を
mean で上回り、Square の 0.5 を勝つため)。これはユーザー決定 C「混在 → Square」
と矛盾するため、**log(ratio) の中央値 → 最近接バケット**方式に切り替えた。

採用アルゴリズム:

```
1. 各サンプル r_i = h_i / w_i から s_i = log(r_i) を計算 (不正値は除外)
2. m = median(s_i)
3. best = argmin over a in ThumbAspect::all() of |m - log(a.height_ratio())|
```

性質:
- **対称な縦横混在は自然に m=0 (= log 1.0) → Square** へ収束
- 外れ値に頑健 (中央値ベース)
- 全 7 バケットのスキャンは線形、計算量 O(N log N) (ソート支配)

`fit_score(r, a) = min(r/a, a/r)` 関数は **診断・テスト用に残存** している。
セル内 letterbox 充填率の意味で実用上の解釈に便利だが、選択ロジックでは使わ
ない (詳細: [src/auto_aspect.rs](../src/auto_aspect.rs) module docstring)。

### 4.3 サンプル下限

```
min_samples = min(eligible_total, max(8, eligible_total / 4)).min(24)
```

意味:
- 基本は **アイテム総数の 25% または 8 件のうち大きい方**、上限 24 件
- ただし **`eligible_total` 自体が下限になるよう clip** (= 5 件しか
  画像がないフォルダなら 5 件揃えば確定する)

(eligible_total = フォルダ内の集計対象アイテム総数)

「画像が 0 件」のフォルダ (動画のみ・PDF のみで動画/PDF 比率も取れない等)
だけ判定保留 = 既存値のまま。それ以外の小フォルダでも揃えば確定する。

### 4.4 切替条件 (anti-flapping)

| 条件 | 値 |
| --- | --- |
| サンプル数下限 | `min_samples` 以上 |
| 改善幅 | log 距離マージン `> 0.10` (= `\|median - log(current.a)\| - \|median - log(best.a)\| > 0.10`) |
| 同候補勝利継続 | **750ms** または **+8 サンプル** の間、同じ `best` であり続ける |
| 切替後 cooldown | 直近切替から **2 秒** は再評価しない |
| フォルダごとの上限 | **2 回** まで (3 回目以降は許可しない) |
| 入力ゲート | スクロール / キー入力後 **500ms** は適用保留、idle になってから反映 |

ユーザーの「あまり頻繁に切り替わらないように」という要求はこの 6 条件の
組み合わせで担保する。実機で違和感があれば §10 で挙げる調整パラメータを
触る。

切替回数を 2 回までに制限する理由: 1 回目は catalog 既存分で即決、その後
decode が進んで分布が大きく変わったら 2 回目で修正、それ以降はフラフラ感の
方が害が大きい。

### 4.5 初期確定の高速化

`start_loading_items` の時点で catalog から比率情報が十分量取れることが多い
(キャッシュ済みフォルダ)。この場合、**初回フレーム描画前に同期で 1 回判定**
して `effective_thumb_aspect` を確定させる。これが「Square で開いてからすぐ
切り替わる視覚的フリッカ」を防ぐ最大の手段。

未キャッシュフォルダではこの即決は失敗し、Square で描画開始 → decode 進行に
合わせて段階的に切り替わる。

### 4.6 フォルダ別の前回結果キャッシュ (2026-05 追補)

`%APPDATA%/mimageviewer/auto_aspect_cache.db` に、フォルダ / ZIP / PDF コンテナごとの
前回 auto-aspect 確定値を保存する。再訪時は `reset_and_seed_auto_aspect` の直後、
catalog seed より前に `auto_aspect.current` をこの値で初期化し、従来の
「未確定なら 1:1」から始まる表示切替を減らす。

- 保存対象は `ThumbAspect` と診断用の sample 数 / eligible_total のみ。代表サムネ対象や
  サムネ画像 BLOB は既存 `catalog.db` / `folder_thumb_pins.db` の責務から動かさない。
- キャッシュ値は楽観的な初期値であり、後続の catalog seed / `poll_thumbnails` で集まる
  実統計が既存の 6 段ゲートを通れば補正される。ただしキャッシュ復元時は、保存時の
  `sample_count` と同等以上 (`min(sample_count, 現在の eligible_total)`) の実測 sample が
  集まるまでは上書きしない。前回より少ない部分統計だけで比率が即変更されるのを避けるため。
- Ctrl+G / Ctrl+S の検索結果ビューや `__search_results__` 合成パスは、クエリ依存で
  中身が変わるため保存・復元対象外。

## 5. アーキテクチャ

### 5.1 設定モデル

```rust
// src/settings.rs
pub struct Settings {
    /// 既存フィールド。ユーザーが手動で選んだ比率。
    /// Auto モードでも **書き換えない** (= Manual に戻したときに直前の手動値が
    /// 復活するよう保持)。Auto 未確定時の effective 値ではない (§5.3 参照)。
    pub thumb_aspect: ThumbAspect,

    /// 新規。デフォルト false (既存ユーザー保護)。`#[serde(default)]` を付ける。
    pub thumb_aspect_auto: bool,
    // ...
}
```

`bool` で十分 (enum 化はオーバーキル)。

### 5.2 ランタイム状態

```rust
// src/app.rs の App 構造体に追加
pub struct AutoAspectState {
    /// このフォルダの items_generation。世代が変わったら全リセット。
    pub items_generation: u64,

    /// 集計済みサンプル: idx -> ratio (h/w)。重複追加防止のため idx キー。
    pub samples: HashMap<usize, f32>,

    /// 確定済み (or 仮確定) の自動比率。
    /// None なら未確定 = `effective_thumb_aspect()` は **常に Square** を返す
    /// (auto モード時。直前の手動値に引きずられない仕様。§5.3 参照)。
    pub current: Option<ThumbAspect>,

    /// このフォルダで何回切り替えたか (0..=2)。
    pub switches_done: u8,

    /// 直近切替時刻 (cooldown 判定用)。
    pub last_switch_at: Option<Instant>,

    /// 連勝中の候補: (aspect, since_when, sample_count_when_started)
    pub streak: Option<(ThumbAspect, Instant, usize)>,
}
```

### 5.3 主要関数

#### 純関数 (新規 `src/auto_aspect.rs`)

テスト容易性のため、判定ロジックは副作用なしで切り出す。

```rust
pub fn fit_score(ratio: f32, candidate: ThumbAspect) -> f32 {
    let r = ratio;
    let a = candidate.height_ratio();
    (r / a).min(a / r)
}

pub fn pick_best(samples: &[f32]) -> Option<ThumbAspect> {
    // §4.2 のアルゴリズム: log(ratio) の中央値 → 最近接 ThumbAspect。
    // samples が空 / 全部不正値 なら None。
}

pub fn min_samples_for(eligible_total: usize) -> usize {
    // §4.3 の式: アイテム総数の 25% または 8 のうち大きい方、上限 24、
    // ただし eligible_total 自体を超えないよう clip (= 小フォルダ対応)。
    let ideal = (eligible_total / 4).max(8).min(24);
    ideal.min(eligible_total)
}

pub enum AspectDecision {
    Hold,
    Switch(ThumbAspect),
}

/// サンプルとヒステリシス基準だけで「切り替えるべきか」を決める純関数。
/// **cooldown / switches_done / streak / scroll-idle は呼び出し側 (App) で判定する**。
/// この関数の責務はサンプル下限到達 + log 距離マージン判定のみ。
///
/// `log_margin` はバケット間距離の log 空間でのマージン。例: 0.10 で「current
/// バケットより best バケットの方が log_margin 以上中央値に近い」場合のみ switch。
///
/// テスト容易性のため副作用なし。`§9.1` のヒステリシステストはこの関数を呼ぶ。
pub fn decide_auto_aspect(
    samples: &[f32],
    eligible_total: usize,
    current: ThumbAspect,
    log_margin: f32,  // 例: 0.10
) -> AspectDecision {
    if samples.len() < min_samples_for(eligible_total) {
        return AspectDecision::Hold;
    }
    // 中央値を計算 (有効サンプルのみ、ソートして median)
    let median = /* log(ratio) の中央値 */;
    let best = nearest_bucket_to_log_ratio(median);
    if best == current {
        return AspectDecision::Hold;
    }
    let curr_dist = (median - current.height_ratio().ln()).abs();
    let best_dist = (median - best.height_ratio().ln()).abs();
    if curr_dist - best_dist > log_margin {
        AspectDecision::Switch(best)
    } else {
        AspectDecision::Hold
    }
}
```

`pick_best` / `decide_auto_aspect` ともに O(N log N) (ソート支配)。実装は
[src/auto_aspect.rs](../src/auto_aspect.rs) を参照。

#### `App` メソッド (副作用あり)

```rust
impl App {
    /// 実描画で使う比率。
    /// - Manual モード: settings.thumb_aspect をそのまま返す
    /// - Auto モード未確定: **常に Square** (1:1 開始の仕様、直前の手動値に
    ///   引きずられないように明示的に固定)
    /// - Auto モード確定後: auto_aspect.current
    pub fn effective_thumb_aspect(&self) -> ThumbAspect {
        if self.settings.thumb_aspect_auto {
            self.auto_aspect.current.unwrap_or(ThumbAspect::Square)
        } else {
            self.settings.thumb_aspect
        }
    }

    /// items_generation が変わったらリセット + catalog の既存比率を一括投入。
    /// 十分量あれば即決もここで行う。
    ///
    /// **呼び出し位置**: `start_loading_items` 内の
    /// **`catalog.delete_missing()` 完了後、`spawn_thumbnail_workers()` 起動前**。
    /// 現コード ([src/app.rs:7329-7354](../src/app.rs)) でいうと `sli_catalog_delete_missing`
    /// perf event の直後、`spawn_t0` ローカル変数を作る直前あたり。
    /// この時点で `self.items` / `image_metas` / `cache_map` がすべて確定しており、
    /// かつ worker spawn 前なので `display_px` 更新やワーカー競合とも干渉しない。
    fn reset_and_seed_auto_aspect(&mut self, cache_map: &CacheMap);

    /// poll_thumbnails で新着 Loaded を反映するときに呼ぶ。
    fn record_aspect_sample(&mut self, idx: usize, ratio: f32);

    /// 毎フレーム末尾 or poll 後に呼ぶ。条件を満たしたら切替を実施。
    fn maybe_apply_auto_aspect(&mut self);
}
```

#### 永続キャッシュ (`src/auto_aspect_cache.rs`)

`AutoAspectCacheDb` は `auto_aspect_cache.db` に `folder_key -> aspect` を保存する。
`App::reset_and_seed_auto_aspect` は Auto モードかつ検索結果ビューでない場合に
前回値を復元し、`maybe_apply_auto_aspect` は実際の切替または no-op 確定時に
現在の確定値を upsert する。書き込みは 1 フォルダあたり通常 1〜2 回で、毎フレームの
DB アクセスは行わない。
復元した値は、保存時の `sample_count` と同等以上の実測 sample が集まるまで
`cached_sample_gate` で保護される。
リセットはサムネイルキャッシュ管理ダイアログの削除操作に連動する:
現在フォルダ削除は該当 `folder_key`、古いキャッシュ削除は `updated_at` が指定日数より
古い行、全削除は全行を削除する。

### 5.4 描画側の置き換え

`ui_main.rs:2005-2006` を含む全 `self.settings.thumb_aspect.height_ratio()`
参照を `self.effective_thumb_aspect().height_ratio()` に置換する。
箇所は数えるほどしか無いはずだが、grep で機械的に確認する:

```bash
rg 'settings\.thumb_aspect[^_]' src/
```

(`thumb_aspect_auto` を巻き込まないよう `[^_]` 後置)

## 6. 切替時のスクロール位置補正

セル比率変更は `cell_h` を変えるので、放置すると現在見えている行が
画面外に飛ぶ。手動切替でも今ガクッと飛んでいるはず (= 既存バグ)。
**今回の修正でついでに直す**。

`App` には既に **`last_cell_size`** (= cell_w) と **`last_cell_h`** が
あり、前フレーム描画時の値が入っている ([src/app.rs:1724-1726](../src/app.rs))。
新セルの cols は変わらないので、`old_cell_h / new_cell_h = old_ratio /
new_ratio` の関係で `scroll_offset_y` を比例補正できる:

```rust
/// 比率変更時のスクロール補正だけを行う pure helper。
/// auto / manual どちらの経路からも呼ぶ (state 書き換えは呼び出し側)。
fn fixup_scroll_for_aspect_change(&mut self, new_aspect: ThumbAspect) {
    let old_cell_h = self.last_cell_h.max(1.0);
    let new_cell_h = (self.last_cell_size * new_aspect.height_ratio()).round().max(1.0);
    if (new_cell_h - old_cell_h).abs() < 0.5 {
        return; // 変化なし
    }
    // 画面先頭の row index を維持
    let anchor_row = (self.scroll_offset_y / old_cell_h).floor();
    self.scroll_offset_y = anchor_row * new_cell_h;
    // (描画ループ側の clamp に任せる)
}
```

呼び出し経路:

- **自動切替** (`maybe_apply_auto_aspect` 内):
  `fixup_scroll_for_aspect_change(new)` → `self.auto_aspect.current = Some(new)`
  → cooldown / switches_done を更新
- **手動切替 (auto → manual)** (`ui_main.rs:569`, `:906` の個別比率クリック):
  `fixup_scroll_for_aspect_change(picked)` → `settings.thumb_aspect_auto = false`
  + `settings.thumb_aspect = picked`
- **「自動」を選択** (`ui_main.rs:569`, `:906` の「自動」クリック):
  current が None なら effective は Square (§5.3 参照)。Square と前比率が
  違う場合は `fixup_scroll_for_aspect_change(Square)` を呼ぶ。
  `auto_aspect.current = None` + `thumb_aspect_auto = true` でリセット。

auto state 自体の更新ロジックは経路ごとに違うため、scroll 補正だけを
分離した helper にして共有する。これで自動・手動どちらの切替でも視点が
保たれる。

## 7. UI

### 7.1 メニュー (ui_main.rs:569 周辺)

```
サムネイル比率 ▼
  ✓ 自動         ← 新規、最上部
  ──────
    16:9
    3:2
    4:3
    1:1
    3:4
    2:3
    9:16
```

- 「自動」のチェックは `settings.thumb_aspect_auto == true` のとき
- 「自動」クリック → `thumb_aspect_auto = true`、`auto_aspect.current = None` で再評価
- 個別比率クリック → `thumb_aspect_auto = false` + `thumb_aspect = 選択値`
  (自然な「手動指定したらマニュアル」操作感)

### 7.2 ツールバー (ui_main.rs:906 周辺)

ツールバーは radio button 群なので、「自動」をその先頭に同様に追加する。
ラベル文字列の長さに注意 (既存 UI のレイアウトに干渉しないか実機確認)。

## 8. 実装手順

依存順:

1. `src/auto_aspect.rs` 新規作成 (pure logic + 単体テスト)
2. `Settings::thumb_aspect_auto: bool` を `#[serde(default)]` 付きで追加
   (§11 参照、schema migration は不要)
3. `AutoAspectState` 構造体定義 + `App` フィールド追加 + `Default` 実装
4. `effective_thumb_aspect()` 追加 + 描画側の全置換
5. `reset_and_seed_auto_aspect` を `start_loading_items` 内の
   `catalog.delete_missing()` 完了後 / `spawn_thumbnail_workers()` 起動前で呼ぶ
   (§5.3 参照)
6. `record_aspect_sample` を `poll_thumbnails` の `Loaded` 反映直後で呼ぶ
7. `maybe_apply_auto_aspect` を `poll_thumbnails` 末尾で呼ぶ
8. `fixup_scroll_for_aspect_change` ヘルパーを作り、手動切替経路
   (`ui_main.rs:569` / `:906`) からも呼ぶよう改修 (§6 参照)
9. UI 改修 (メニュー・ツールバーに「自動」項目追加)
10. テスト (§9 参照)
11. ドキュメント更新

## 9. テスト

### 9.1 単体テスト (`tests/auto_aspect.rs` 新規)

| ケース | 期待 |
| --- | --- |
| 全件 r = 1.0 | `Square` が選ばれる |
| 全件 r = 1.5 (= 2:3) | `Portrait2x3` が選ばれる |
| 全件 r = 9/16 ≈ 0.5625 | `Landscape16x9` |
| 半数 r = 0.5, 半数 r = 2.0 (混在) | `Square` (最大 mean fit) |
| eligible_total=20, samples=3 | `min_samples_for(20) = 8` 未達なので App 側で保留 |
| eligible_total=5, samples=5 (小フォルダで揃った) | `min_samples_for(5) = 5` 到達 → 確定 |
| eligible_total=5, samples=4 | `min_samples_for(5) = 5` 未達なので保留 |

`min_samples_for` の境界値テスト (式の意図を固定する):

| eligible_total | 期待値 | 意図 |
| --- | --- | --- |
| 0 | 0 | 集計対象 0 件 = App 側で常に判定保留に分岐 |
| 1 | 1 | 全件揃えば確定 (`ideal=8` だが eligible でクリップ) |
| 5 | 5 | 同上 |
| 8 | 8 | 下限ぴったり |
| 20 | 8 | `20/4=5` < 8 なので下限 8 が勝つ |
| 32 | 8 | `32/4=8` でちょうど下限と一致 |
| 36 | 9 | `36/4=9` で下限を超えたので 25% ルール |
| 96 | 24 | `96/4=24` で上限ぴったり |
| 100 | 24 | 上限 24 でクリップ |
| 1000 | 24 | 上限維持 |

ヒステリシステスト (`decide_auto_aspect(samples, eligible_total, current,
log_margin=0.10)` の戻り値):

| ケース | 期待 |
| --- | --- |
| サンプル数 `< min_samples_for(eligible_total)` | `AspectDecision::Hold` |
| best == current | `AspectDecision::Hold` |
| `log(median)` がほぼ Square と Portrait3x4 の中間 (改善 ≈ 0.012) | `AspectDecision::Hold` |
| 全件 r=1.0, current=Portrait9x16 (log 距離 0.575) | `AspectDecision::Switch(Square)` |
| bimodal 対称 (r=0.5, r=2.0 半々), current=Portrait9x16 | `AspectDecision::Switch(Square)` (混在 → Square) |

### 9.2 状態遷移テスト

純関数だけでは扱えないロジック (cooldown / switches_done / streak 更新 /
seed 後の即決) を、`AutoAspectState` を直接 fixture で組み立てて `App` の
helper メソッド (例: `record_aspect_sample`, `maybe_apply_auto_aspect`) を
呼ぶ統合テストでカバーする:

- seed で `min_samples` 到達 → 即決して `current = Some(...)`、`switches_done = 1`
- 2 回目の切替が `cooldown` 経過前 → 据え置き
- 2 回目の切替が cooldown 経過後 → 適用、`switches_done = 2`
- 3 回目の切替候補が出ても無視 (`switches_done` 上限)
- スクロール / キー入力直後 (`last_input + 500ms` 内) → 適用保留
- `fixup_scroll_for_aspect_change` を呼ぶと `scroll_offset_y` が比例補正される

### 9.3 UI スナップショット (限定的に)

[docs/ui-snapshot-policy.md](ui-snapshot-policy.md) の方針に沿い、grid 全体の
スナップショットは取らない (dummy items を流す煩雑さと方針衝突)。代わりに:

- 「サムネイル比率」メニューに **「自動」項目が追加されている** ことと、
  Auto モード時に **チェックマークが「自動」側に付いている** ことを
  toolbar / menu 単体のスナップショットで確認する (= 既存の小さな
  UI snapshot と同じ粒度)

### 9.4 実機確認チェックリスト

- 漫画 ZIP を開く → 2:3 か 9:16 に落ち着く
- 写真フォルダ (横向きスマホ写真) → 3:2 か 4:3
- スクショ・SNS 縦画像混在 → Square
- 動画オンリーフォルダ → 16:9
- フォルダ・ZipFile が並ぶ階層 → 代表サムネ次第 (実用上は集計対象が少ない
  ことが多く保留になるはず)
- スクロール中に切替が走らないこと
- 1 フォルダ内で 3 回以上は切り替わらないこと
- 手動で「16:9」を選んだ後、別フォルダを開いても勝手に自動には戻らないこと
- 「自動」を選んだ後、別フォルダを開くと再評価が走ること

## 10. オープン論点 (実装中・実機調整)

- **ヒステリシス幅** (`log_margin = 0.10`): 実機で「微妙な改善で切り替わって
  しまう」感があれば 0.12〜0.15 に上げる (隣接バケット間 log 距離の下限が約 0.117 なので、0.15 以上だと「2 バケット跨ぐ場合のみ」に近づく)。
- **同候補勝利継続** (750ms): 短すぎると flapping、長すぎると「全然変わらない」
  感。実機で詰める。
- **切替最大回数** (2): 1 で十分かもしれない。実機で「2 回目が要らない」と
  感じれば下げる。
- **catalog からのシード処理**: `decode_thumb_dims` 自体は `catalog.load_all()`
  で既にメモリに乗っている `entry.jpeg_data` 上の header probe なので
  **追加ファイル I/O は無い**。CPU 時間のみ、数千件で 1 秒未満を想定。
  既存の `load_all` 自体のコスト (SQLite から全 row + BLOB を読む) は
  `sli_catalog_load_all` perf イベントで既に計測されており、auto-aspect は
  ここに「便乗」する形。実測 → 16ms/frame を脅かすようなら
  [docs/ui-responsiveness.md](ui-responsiveness.md) §4 のチェックリストに沿って
  worker 化する。
- **動画サムネの代表性**: Shell サムネが必ずしも動画フレームと同比率では
  ないケースがあるかも (16:9 動画の縦長 Shell サムネ等)。実機で違和感あれば
  動画は集計対象から外すフラグを追加。
- **「自動」表示時のラベル**: 確定済みなら "自動 (2:3)" のように現在採用中の
  比率を併記すると親切。実装は容易だが UI レイアウトに影響するかもしれない
  ので最初は無しでスタート。

## 11. リリース判断 / 互換性

- `Settings::thumb_aspect_auto` は **新規フィールド**、デフォルト `false`。
  既存ユーザーは Manual を維持。
- **`settings_db` の schema migration は不要**。`settings_db` は
  `serde_json::to_value(&settings)` で全フィールドを `settings_kv (key, value)`
  テーブルに JSON 値そのまま格納する設計 ([src/settings_db.rs:1-26](../src/settings_db.rs))。
  したがって新フィールドの追加は **`Settings` 構造体に `#[serde(default)]`
  属性を付けて足すだけ** で、旧 DB を新コードで開いても安全に既定値が入る
  (= 既存環境では自動的に `thumb_aspect_auto = false` で起動)。
- 代わりに **設定 roundtrip テスト** を追加する: `Settings { thumb_aspect_auto:
  true, .. }` を save → load して値が保たれることを確認。
  追加先は **`src/settings.rs` の `mod tests::phase3_sqlite`** 系
  (`save_load_roundtrip` 等の既存テスト群 — [src/settings.rs:4045](../src/settings.rs))。
  既存テストと同じ `setup_backup_env()` パターンを再利用する。
- `Settings::thumb_aspect` 自体は既存リリース済みフィールドなのでセマンティクス
  は変更しない (自動切替で書き換えないことで担保 — §5.1 参照)。
- ロールアウト時の説明は `htdocs/mimageviewer/manual/` の該当ページに 1 段落
  追加すれば足りる (バージョンタグは付けない — [CLAUDE.md のマニュアル記述方針](../CLAUDE.md))。

## 12. 触らないと決めた領域

- `catalog` スキーマの拡張 (`source_dims = None` 救済は `decode_thumb_dims`
  既存関数で十分)
- 新しい worker thread の追加 (既存 thumb 経路で十分量のデータが流れる)
- `ThumbAspect` 候補の追加 (7 種で十分カバー)
- 「ユーザーごとに学習したカスタム比率」のような ML 系拡張 (オーバーキル)
