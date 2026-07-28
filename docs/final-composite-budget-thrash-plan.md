# final composite 予算超過時の再計算スラッシング対策

## 1. 背景 — 観測された不具合

連結読み + スライドショー自動スクロール + カラー化有効で、「次ページの枠は画面下から出ているのに
絵が出ない」空白が発生する。perf ログ (2026-07-28, `perf_events.jsonl`, PDF コミック
2842x4095) の実測:

```
カラー化ジョブ総数        : 950
  初回 (必要な計算)       :  89 件 ( 3.4 s)
  冗長な再計算            : 861 件 (28.0 s)  = 全体の 91%
ワーカー総処理時間        : 31.4 s → うち 89% が無駄
```

同一ページ・同一サイズ・同一パラメータ・`complete=true` のジョブが繰り返し投入されていた:

| idx | 回数 | 期間 | サイズ |
| --- | --- | --- | --- |
| 10 | 124 | 35.1 s | 2842x4095 (不変) |
| 24 | 80 | 17.6 s | 2840x4095 (不変) |
| 23 | 80 | 18.2 s | 2840x4095 (不変) |
| 0 | 39 | 7.4 s | 2840x4095 (不変) |

t=1195..1215 の 20 秒間だけで 151 ジョブ / 13 ページが同時に再計算されていた。

## 2. 根本原因 — 生産側と保持側の権限分離

`complete=true` で完成した結果が `final_composite_cache` から消え、次フレームで同じキーが
再投入される。サイズもパラメータも不変なのでキー変化ではなく **eviction が原因**。

3 つの独立した欠陥が重なっている。

| # | 欠陥 | 現状 |
| --- | --- | --- |
| 1 | **投入側が予算を見ていない** | `prefetch_final_effects` は `ai_prefetch_targets` (枚数指定) だけで対象を決め、`fs_vertical_cache_keep_set` も texel 残量も参照しない |
| 2 | **ヒステリシスが無い** | 追い出し閾値 = 投入閾値 = `VERTICAL_READING_MAX_CACHE_TEXELS` (320M)。境界上で「捨てる↔作る」が振動する |
| 3 | 追い出しが all-or-nothing | 高価な CPU 計算結果 (`pixels`) と、安価に再生成できる GPU テクスチャ (`texture`) を一緒に捨てる |

### 予算計算

2842x4095 ページの場合:

| | texel 数 |
| --- | --- |
| 1 ページ (raw のみ、mip chain 込み x4/3) | 15.5M |
| 1 ページ (raw + カラー化済み composite) | 31.0M |
| 上限 320M で保持できるページ数 | **約 10 ページ** |

一方で保持しようとする範囲は「可視ページ (連結読みは複数枚) + keep_set 前方 6 ユニット +
AI/カラー化先読み 前方 8 枚 (ユーザー設定) + 後方」で、10 ページを大きく超える。

結果として `update_continuous_reading_prefetch_window` の texel トリムが非可視ページを
keep_set から外し、`evict_final_pipeline_cache_for_keep_set` が `final_composite_cache` から
削除し、次フレームで `prefetch_final_effects` が同じキーのミスを見て再計算する。

**先読み枚数を増やすほど悪化する**。8 枚設定は texel 予算が保持できる範囲を超えたため、
先読みが「計算しては捨てる」だけの純粋な無駄に変わっていた。

### 波及

先読みが空回りするのでキャッシュに何も積み上がらず、新しいページが画面に入ったとき
先読み済みのはずのものが存在しない (実測 88 件が `prefetch=false` = 可視になってから着手)。
さらに final-effect ワーカーは同時 1 本の直列なので、無駄な再計算がスロットを掴んでいる間
可視ページの処理が待たされる。これが「枠は出るが絵が出ない」空白の正体。

なおこの現象は**カラー化固有ではない**。`final_composite_cache` を使う経路 (Creative LUT 等)
で同条件が揃えば同様に起きる。

### なぜユーザーが気づけないか

アプリは「遅い」ようにしか見えない。エラーも警告も出ず、CPU 使用率がやや高いだけ。
この観測不能性自体を欠陥として扱い、§3 の F で恒久的に検知可能にする。

## 3. 対策

第 1 段は **状態を増やさない 3 点 (F/B/C)** に限定する。A は F の計測値を見てから要否を判断する。

### F. スラッシング検出 (最初に入れる)

「completed 済みの同一キーを短時間内に再計算した」回数を数える。正常時は 0 のはずで、
閾値判断が要らない**純粋な無駄の指標**。

- 完成後に cache から落ちたキーを、時刻付きの有界マップ (上限 256 件程度、LRU) に記録する
- final-effect job の spawn 時にそのマップを引き、一定時間内 (例 30s) なら再計算カウンタを
  増やして perf イベントを出す
- カウンタが窓内で閾値を超えたら `logger` に警告を 1 行出す (毎回は出さない)
- perf イベント: `fs` / `final_effect_recompute` — `idx` / `age_ms` / 直前の drop 理由

これを先に入れることで B/C の効果が数値で検証できる (861 → ほぼ 0 になるはず)。

### B. 投入時の入場管理

生産者は投入前に保持権限者へ問い合わせる、という原則にする。

- 連結読みが有効な間、`prefetch_final_effects` の対象を `fs_vertical_cache_keep_set` との積に絞る
- keep_set が保持しないページは**そもそも先読みしない**
- ページ送り (paged) 時は従来どおり `ai_prefetch_targets` のみで判定する (keep_set は連結読み専用)

CLAUDE.md 「context 固有の resource は create / mutate / drain / cancel / invalidation / drop が
所有 context だけに作用することを確認する」に沿う。現状は生産と破棄の所有者が分離している。

### C. 二段の水位 (ヒステリシス)

B だけでは keep_set が毎フレーム再計算されるため、境界でまだ振動し得る。

- `VERTICAL_READING_MAX_CACHE_TEXELS` を HIGH とし、LOW (例 HIGH の 75%) を追加する
- texel トリムは HIGH 超過で発火し、**LOW まで**落とす (現状は HIGH まで)
- 新規の投機的生産 (先読み) は **LOW 未満のときだけ**許可する
- 可視ページは従来どおり予算判定をバイパスする (表示は止めない)

「追い出したらすぐ作る」が構造的に起こらなくなる。

### 第 1.5 段 — 連結読みの準備帯

F/B/C 導入後の同一シナリオ実測では、冗長な再計算 861 件は `final_effect_recompute` 0 件、
カラー化ジョブ 950 件は 79 件まで減った。一方、先読み成功は 94% から 11% (9/79) に低下し、
70 件がページ可視化後の `prefetch=false` だった。keep-set が最大 20 ページまで raw を保持すると
約 310M texel となり、LOW 240M を恒常的に超えて投機的な final-effect 先読みを止めるためと
推定する。この拒否理由は後述の計装で検証可能にする。

#### 1. 描画対象と処理対象の分離

連結読みの描画対象は従来どおりマージン 0 の厳密可視ユニットだけとする。final composite / comic
composite の処理対象だけ、厳密可視範囲の前後 1 ユニットを「準備帯」として加える。見開きでは
1 ユニットが 2 ページになる。準備帯も同じ offset と unit size から矩形を計算し、現在ページ、
viewport 中心からの距離の既存優先順で 1 フレーム 1 ページずつ final pipeline に流す。

`VERTICAL_READING_MAX_VISIBLE_PAGES` の判定とズーム調整は厳密可視集合だけを使う。準備帯を描画、
`FullscreenPageLayout`、可視ページ数上限へ混ぜず、keep-set eviction の可視バイパス集合にも
流用しない。

#### 2. 準備帯への予算優先権

準備帯は `fs_vertical_cache_keep_set` 内であることを引き続き必須とする一方、投機的な
final-effect admission の LOW 水位をバイパスする。texel トリムでも厳密可視ユニットと同じく
`removable` から除外し、遠方の raw / 派生キャッシュを先に退去させる。準備帯は前後 1 ユニットに
固定されるため、この優先権だけで保持範囲が無制限に広がることはない。

#### 3. admission 拒否理由の計装

final-effect admission は bool ではなく `Allow` / `NotInKeepSet` / `OverLowWatermark` を返す。
拒否時は perf イベント `fs.final_effect_prefetch_blocked` に `idx` / `reason` / `loaded_texels` /
`low_watermark` を記録する。同じ idx・同じ理由は 1 秒に 1 回へ間引き、理由が変わった場合は
直ちに記録する。許可へ戻った idx は間引き状態を解除する。状態更新とイベント生成は
`crate::perf::is_enabled()` の内側だけで行う。

### A. 降格 (第 2 段、F の計測後に判断)

`FinalCompositeEntry` は `pixels: Arc<ColorImage>` と `texture: TextureHandle` を両方持ち、
texel 予算が縛るのは GPU 側だけ。予算超過時に `texture` だけ落として `pixels` を残せば、
再生成はアップロードのみ (約 15-30ms / 11.6MP) で済み、**同時 1 本の final-effect ワーカーを
消費しない**。

- コスト: RAM 46.6MB/ページ (2842x4095)。CPU 側専用の予算 (texel 予算とは別枠) が必要
- B+C で病的ループが消えた後に残る「正当な eviction の再計算コスト」を下げる最適化であり、
  バグ修正ではない。F のカウンタが有意に残る場合のみ着手する

### 見送り

- **D. コスト考慮の追い出し順序** — A を入れれば「GPU のみ / pixels 保持 / 完全破棄」の階層が
  自然にでき、D が狙う効果は A に内包される。独立したコスト関数は状態パターンを増やすだけなので
  採用しない
- **E. 設定値の clamp と実効値表示** — B が入れば予算超過分は投入されないため、8 枚設定は
  無害になる (単に効かないだけ)。実効値の可視化は将来の任意改善として残す

## 4. 実装順序と検証

1. **F** を単独で入れ、現状の再計算回数がログに出ることを確認する (ベースライン取得)
2. **B** を入れ、カウンタが激減することを確認する
3. **C** を入れ、境界付近でも振動しないことを確認する
4. **第 1.5 段**として、厳密可視と準備帯を分離し、準備帯の LOW 水位バイパスと
   admission 拒否理由の計装を追加する

### 検証方法

`--perf-log` 付きで連結読み + スライドショー + カラー化を実行し、以下を確認する。

```
python -c "..."  # kind=final_effect_worker / final_effect_prefetch_blocked を集計
```

- 完成済み同一キーの再計算が **ほぼ 0** のまま維持されること
- `prefetch=false` (可視になってから着手) の比率が準備帯導入前より下がること
- `final_effect_prefetch_blocked` の理由別件数から、keep-set と LOW 水位のどちらが支配的か確認できること
- 先読み枚数を 8 に上げても再計算が増えないこと

回帰テストは純関数レベルで:

- 入場判定が paged、keep-set 外、LOW 水位超過を理由付きで区別すること
- 準備帯が LOW 水位をバイパスし、keep-set 外は拒否されること
- 準備帯が厳密可視の前後 1 ユニットへ clamp され、空 unit 列を扱えること
- 準備帯導入前後で厳密可視の描画対象が変わらないこと
- texel トリムが厳密可視と準備帯を候補から除外すること
- ヒステリシスが HIGH で発火し LOW まで落とすこと
- スラッシング検出が「同一キー・短時間・完成済み」だけを数えること
- admission 拒否ログが理由変更時または 1 秒間隔だけ発火すること

実機の perf 再計測では、遠方 raw の退去と準備帯 final composite の完成順を併せて確認する。
今回の変更では検証バイナリを作らず、既存の perf 採取手順での次回計測に委ねる。

## 5. 同時更新するドキュメント

- `docs/final-composite-budget-thrash-plan.md` — F/B/C 実測値、第 1.5 段、検証観点
- `docs/display-pipeline.md` — 厳密可視の描画対象と準備帯の処理対象の分離
- `docs/preset-and-adjustment.md` — 準備帯の admission / trim 優先権
- `docs/async-architecture.md` — `final_effect_prefetch_blocked` の属性と間引き
- `docs/downscale-moire-lod-plan.md` — HIGH / LOW 水位と保護集合
## 6. 第 2 段 — mImageViewer 全体の VRAM 予算統一

### 6.1 単一 pool と設定互換

GPU メモリ予算は `vram_budget.rs` が単独で所有する。検出した専用 VRAM を `V`、環境設定の
割合を `p` とすると、共有 pool は `P = floor(V * p / 100)` byte。VRAM 取得失敗時は
`V = 4 GiB` を使い、`p = 0` は上限なしとして `P` 自体を持たない。

Rust フィールドは意味に合わせて `gpu_memory_percent` とする一方、settings.db 内の serde key は
リリース済みの `thumb_vram_cap_percent` を維持する。旧 key の値を新フィールドへ deserialize し、
再 serialize 後も旧 key だけが出る後方互換テストを置く。`retained_final_ai_cache_max_mib` は
`Arc<ColorImage>` 用の CPU メモリ (RAM) 上限であり、key・値・予算軸を変えない。

### 6.2 モード別配分と連結読み水位

共有 pool は利用中の画面へ 80%、従となる画面へ 20% を配る。

| モード | サムネイル | フルスクリーン表示系 |
| --- | ---: | ---: |
| 一覧 | `floor(P * 80 / 100)` | `floor(P * 20 / 100)` |
| フルスクリーン | `floor(P * 20 / 100)` | `floor(P * 80 / 100)` |

連結読みの RGBA8 水位は、フルスクリーン配分を `F` byte として
`HIGH = floor(F / 4)` texel、`LOW = floor(HIGH * 3 / 4)` texel とする。固定 320M/240M は廃止する。
0% では HIGH/LOW を `None` とし、texel trim と遠方先読みの LOW admission を実質無効化する。
可視ページと厳密可視の前後 1 ユニットの準備帯は従来どおり LOW をバイパスし、trim 候補にも
含めない。描画対象は厳密可視集合だけのままとする。

既定 50%、raw + final composite が約 31.0M texel/ページの実測例では次の目安になる。
ページ数上限 20 も同時に効くため、括弧内を実効保持目安とする。

| 専用 VRAM | HIGH | LOW | HIGH 到達ページ / 実効 | LOW 退去目標ページ / 実効 |
| ---: | ---: | ---: | ---: | ---: |
| 24 GiB | 2,576,980,377 texel | 1,932,735,282 texel | 約 83 / **20** | 約 62 / **20** |
| 8 GiB | 858,993,459 texel | 644,245,094 texel | 約 27 / **20** | 約 20 / **20** |
| 4 GiB | 429,496,729 texel | 322,122,546 texel | 約 13 / **13** | 約 10 / **10** |

VRAM 検出失敗時も 4 GiB 行と同じ HIGH 約 429M texel となり、旧固定 HIGH 320M より極端に
小さくならない。

### 6.3 実テクスチャ会計と計装

`fs_cache`、final / adjustment / AI / erase / local-adjust / conceal / comic / edit の各 cache、
連結 transition、サムネイル 3 系統を 1 つの accountant へ登録する。`TextureHandle::size()` と
完全な mip chain を使い、cache をまたぐ同一 `TextureId` は 1 回だけ数える。表示ピクセル数からの
推定式は使わない。小さく有界な checker、stamp、font preview、mask preview、音楽 timeline 等は
対象外とする。

perf が有効なときだけ約 1 秒間隔で cat=`gpu` / kind=`vram_accounting` を出し、全 texel/byte、
pool とモード別上限、subsystem ごとの texel/byte を記録する。この第 2 段では全 Tier 共通の
逐次調停は追加せず、上記 2 モード配分だけを eviction/admission へ接続する。

### 6.4 サムネイル復帰帯

フルスクリーン中のサムネイル保持帯は、凍結した一覧スクロール位置ではなく `fullscreen_idx` の
display list 上の位置を中心にする。ロード済みサムネイルの実 texture 会計が 20% 配分を超えたら
現在位置を必ず残して前方約 2/3・後方約 1/3へ自然に縮める。フルスクリーン移行専用の破棄状態は
追加しないため、一覧へ戻る周辺を温めつつモード配分だけで収束する。

### 6.5 実装単位

1. 全 cache の accountant と `gpu.vram_accounting`（報告・計装のみ）
2. Rust フィールド名と UI 意味の一本化（永続 key と AI RAM 設定は互換維持）
3. 連結読み HIGH/LOW の共有 pool 由来化と無制限規則
4. 80/20 モード配分と `fullscreen_idx` 追従のサムネイル保持帯

A（GPU texture だけを降格して CPU pixels を保持）は引き続き実施しない。F/B/C と準備帯の
再計測で、正当な eviction 後の再計算コストがなお支配的と確認できた場合にだけ再検討する。
