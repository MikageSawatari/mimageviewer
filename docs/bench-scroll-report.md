# Thumbnail Scroll Benchmark Report

**Date:** 2026-04-13  
**Version:** v0.4.0 (development)  
**System:** Windows 11, 24-core CPU, RTX 4090  
**Grid:** 5 cols x 4 rows (20 visible items), scroll to middle, 22 worker threads

## Summary Table

All times in **ms after scroll stop** (scroll time excluded).

| Folder | Type | Items | Cold 1st | Cold All | Warm 1st | Warm All | Hot 1st | Hot All |
|--------|------|------:|:--------:|:--------:|:--------:|:--------:|:-------:|:-------:|
| VNCG.org | HDD ZIP+Folder | 1492 | 52 | 2496 | 10 | 973 | 10 | 1315 |
| ComfyUI | SSD AI Images | 2000 | 125 | 383 | 105 | 0 | 11 | 0 |
| Photos | SSD EXIF | 154 | 1215 | 2223 | 531 | 0 | 0 | 0 |
| 18movie | HDD Video+Image | 2474 | 53 | 0 | 32 | 0 | 10 | 0 |
| scan/doc | HDD PDFs | 734 | 1441 | 2363 | 1353 | 0 | 140 | 0 |
| Monobeno.zip | HDD ZIP内部 | 5225 | 586 | 1095 | 451 | 0 | 10 | 0 |

- **1st** = First visible thumb (ms) -- 画面上に最初の1枚が出るまで
- **All** = All visible complete (ms) -- 画面上の20枚が全て揃うまで (0 = スクロール中に完了済)

## Detailed Results

### 1. HDD - ZIP+Folder (E:\share\18\VNCG.org)

1492 items (746 folders + 746 ZIPs). フォルダサムネイルと ZIP サムネイルが交互に並ぶ最も過酷なケース。

| Condition | First Visible | All Visible | All Prefetch | Cache Hit |
|-----------|:------------:|:-----------:|:------------:|:---------:|
| Cold (no cache) | 52 ms | 2496 ms | 2528 ms | 0% |
| Warm (partial cache) | 10 ms | 973 ms | 1017 ms | 39% |
| Hot (OS+app cache) | 10 ms | 1315 ms | 1326 ms | 49% |

**分析:** Cold で All Visible に 2.5 秒かかるのは ZIP/Folder サムネイルの I/O セマフォ直列化のため。Warm/Hot では cache hit で最初の 1 枚は 10ms で表示。ただし cache hit 率が 39-49% と低い（スクロール中に通過したアイテムのみキャッシュ生成されるため）。

### 2. SSD - AI Images (ComfyUI output, 2000 images)

純粋な画像ファイル 2000 枚、SSD 上。

| Condition | First Visible | All Visible | All Prefetch | Cache Hit |
|-----------|:------------:|:-----------:|:------------:|:---------:|
| Cold (no cache) | 125 ms | 383 ms | 1411 ms | 0% |
| Warm (partial cache) | 105 ms | 0 ms | 105 ms | 71% |
| Hot (full cache) | 11 ms | 0 ms | 11 ms | 100% |

**分析:** SSD + 画像のみなので Cold でも 383ms で可視範囲完了。Hot では 11ms で即表示。理想的な結果。

### 3. SSD - EXIF Photos (154 images, large RAW/JPEG)

154 枚の高解像度カメラ写真（EXIF 付き大容量 JPEG）。

| Condition | First Visible | All Visible | All Prefetch | Cache Hit |
|-----------|:------------:|:-----------:|:------------:|:---------:|
| Cold (no cache) | 1215 ms | 2223 ms | 5660 ms | 0% |
| Warm (partial cache) | 531 ms | 0 ms | 531 ms | 86% |
| Hot (full cache) | 0 ms | 0 ms | 0 ms | 100% |

**分析:** Cold で 1.2 秒かかるのは高解像度 JPEG のデコードコスト（1 枚 40-50ms × 20 枚）。キャッシュ効果が顕著で Hot では完全に即時表示。

### 4. HDD - Videos+Images (2474 items, mixed)

画像 1164 + 動画 1308 の混在フォルダ。動画はサムネイル非対応のためスキップ。

| Condition | First Visible | All Visible | All Prefetch | Cache Hit |
|-----------|:------------:|:-----------:|:------------:|:---------:|
| Cold (no cache) | 53 ms | 0 ms | 53 ms | 0% |
| Warm (cache) | 32 ms | 0 ms | 32 ms | 98% |
| Hot (cache) | 10 ms | 0 ms | 10 ms | 99% |

**分析:** 可視範囲が動画のみの場合、サムネイルロード対象がないため即完了。Cold でも 53ms。

### 5. HDD - PDFs (734 documents)

734 個の PDF ファイル。PDFium でのページレンダリングが必要。

| Condition | First Visible | All Visible | All Prefetch | Cache Hit |
|-----------|:------------:|:-----------:|:------------:|:---------:|
| Cold (no cache) | 1441 ms | 2363 ms | 6629 ms | 0% |
| Warm (partial cache) | 1353 ms | 0 ms | 1353 ms | 75% |
| Hot (high cache) | 140 ms | 0 ms | 140 ms | 87% |

**分析:** PDF は PDFium レンダリング (100-200ms/ページ) + セマフォ直列化で Cold が最も遅い。Hot でもキャッシュが 100% にならないのはスクロール中の先読み不足。

### 6. HDD - ZIP 内部 (Monobeno, 5225 images, 3.3GB)

巨大 ZIP を仮想フォルダとして開いた場合。5225 枚の画像。

| Condition | First Visible | All Visible | All Prefetch | Cache Hit |
|-----------|:------------:|:-----------:|:------------:|:---------:|
| Cold (no cache) | 586 ms | 1095 ms | 3893 ms | 0% |
| Warm (partial cache) | 451 ms | 0 ms | 451 ms | 47% |
| Hot (cache) | 10 ms | 0 ms | 10 ms | 66% |

**分析:** ZIP 内画像は ZIP open + エントリ読み取りが必要だが、直列化により競合は抑制。Hot で 10ms は cache hit のおかげ。

## Key Findings

1. **キャッシュ効果は極めて大きい**: Hot (全キャッシュ) では全テストケースで First Visible が 0-140ms。Cold との差は最大 1200ms 以上。
2. **SSD vs HDD**: SSD (AI Images) は Cold でも 383ms で All Visible 完了。HDD (VNCG.org) は Cold で 2496ms。
3. **PDF が最もボトルネック**: PDFium レンダリングは Cold で 1441ms。改善の余地あり。
4. **EXIF 写真は高解像度デコードがボトルネック**: Cold 1215ms は JPEG デコード自体のコスト。
5. **ZIP フォルダサムネイル**: I/O セマフォ直列化で安定。Cold でも First Visible は 52ms と高速。
6. **キャッシュヒット率がスクロール速度に依存**: 高速スクロールではキャッシュが構築されないため partial cache にとどまる。
