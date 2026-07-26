# 外部メタデータ サイドカーの CJK 表示 — 調査記録と修正

外部メタデータ JSON / TXT サイドカー（`<画像名>.json` 等）を mIV で開いたとき、
CJK（特に簡体字中国語）の値が **崩れて見える** 不具合の調査記録と修正内容。
（命名ポリシーに従い、特定の取得ツール名・投稿サイト名は記載しない。
「外部取得ツールが出力するサイドカー」と表記する。）

関連: [../../sidecar-metadata-ingest.md](../../sidecar-metadata-ingest.md)（サイドカー取り込み設計）、
CLAUDE.md「UI 文字列の Unicode グリフ選定ルール」「文字化け対処」。

---

## 1. 症状

メタデータ／サイドカーのパネルで、`title` / `caption` / `user.name` 等の CJK が
崩れて表示される。一方で `tags`（`R-18` / `1girl` 等の ASCII やローマ字）は正常に
読めていた。

## 2. 原因（確定）— 当初の「CP932 誤読」説は誤り

当初この不具合は「mIV が UTF-8 のバイト列を Windows 日本語環境の ANSI コードページ
（CP932/Shift_JIS）として誤読して mojibake になっている」と推測されたが、
**これは誤りだった**。原因は 2 段階で切り分けられる。

### 2.1 読み取りは UTF-8 で正しい（mojibake ではない）

- サイドカー読み取りは `src/external_metadata.rs` の `read_search_text` /
  `read_for_display` に集約され、**JSON は `serde_json::from_slice`、TXT は
  `String::from_utf8_lossy`** で、どちらも UTF-8 デコード。CP932/ANSI 経由の
  デコードはコードのどこにも無い。
- もし本当に CP932 で誤読していたら、`tags` を含む **全ての** 非 ASCII が崩れるはず。
  `tags` が正常に読めている時点で、読み取りは UTF-8 で正しく動いている。
- 回帰テスト `external_metadata::tests::tc1`〜`tc6`（後述）で、生 UTF-8 / 多バイト
  中国語 / `\u` エスケープ / 4 バイト絵文字 を **正しく復元** することを実証済み。

### 2.2 実際の原因 = フォント被覆（CJK グリフ欠落）

崩れていた `title` / `caption` / `name` は **簡体字中国語**だった。
约・轮・馆・苏・恶・这・噢・柠 等は **簡体字専用の字**で、日本語では使わない
（日本語は 約・輪・館・蘇・悪・這…）。

mIV のフォント fallback 連鎖（`src/ui_fonts.rs`）は

```
japanese (Yu Gothic → Meiryo → MS Gothic)
  → text_symbols (Meiryo) → math (Cambria)
  → emoji / historic / symbols (Segoe) → egui 既定 (Latin)
```

で、**簡体字・繁体字・韓国語のフォントを一切含んでいなかった**。そのため簡体字専用字は
どのフォントにも無く、egui が豆腐（□）や脱落として描画していた。これは
**グリフ被覆の問題**であって、mojibake（エンコーディング誤読）ではない。

> mojibake（縺ｮ…）とグリフ欠落（□）は別問題。前者はバイト列の解釈ミス（コード側）、
> 後者はフォントに字が無いだけ（表示側）。本件は後者だった。

## 3. 修正

### 3.1 CJK フォントを fallback に追加（本修正の主目的）

`src/ui_fonts.rs` の `USER_TEXT_FALLBACKS` に、Windows 同梱の CJK フォントを
**シンボル系 fallback の後ろ**に追加する:

| name | フォント | パス | 対象 |
| --- | --- | --- | --- |
| `cjk_sc` | Microsoft YaHei | `msyh.ttc` | 簡体字中国語 |
| `cjk_tc` | Microsoft JhengHei | `msjh.ttc` | 繁体字中国語 |
| `korean` | Malgun Gothic | `malgun.ttf` | 韓国語（ハングル） |

- **`japanese`（Yu Gothic）より後ろ**に置くので、日本語と共有する漢字は引き続き
  Yu Gothic が拾い、**日本語字形が維持**される。Yu Gothic に無い簡体字専用字 /
  ハングルだけが追加フォントに回る。
- 既存のシンボル／絵文字／数学英字の routing より後ろなので、それらの routing は不変。
- フォントが見つからない環境では `install_fallback_font` が静かにスキップする
  （`std::fs::read` 失敗時 `false`）ので、CJK フォント未導入環境でも安全。
- **y_offset は egui の配置式込みで導出**する。CJK は全角グリフで、egui は fallback
  glyph を「fallback font の ascent + primary row height との差分中央寄せ」で置いた後に
  `FontTweak` を足す。単純な glyph 中心合わせだけでは YaHei/JhengHei/Malgun と
  Yu Gothic の ascent / row-height 差が残るため、`AlignRowVisualCenter` で実 glyph bounds
  と row metrics の両方から補正値を計算する。
  混在テキスト（日本語+簡体字+ハングル）を実描画して残差を測り、cjk_sc=0.090 /
  cjk_tc=0.035 / korean=0.150 に調整した（factor はフォントサイズ相対なので DPI 非依存）。
- コスト: 3 つの大型 TTC/TTF（計 ~55MB）を起動時にロード。メモリと起動パースが
  わずかに増えるが、あらゆる言語の CJK サイドカーを表示できる堅牢性を優先（ユーザー合意）。

### 3.3 右パネルのスクロールバー重なり（同時修正）

CJK の長い `caption` で本文が折り返されると、`ui_metadata_panel.rs` のメタデータパネルの
**フローティング縦スクロールバーが本文に重なって**いた。本文をパネル右端いっぱいに
描画していたのが原因。`SCROLLBAR_GUTTER`（14px）分だけ本文幅を狭め、折り返しが
スクロールバーの手前で止まるようにした（`ui.set_width(inner_rect.width() - GUTTER)`）。

回帰テスト `ui_fonts::tests::user_text_covers_cjk_scripts`:
`文`（共有漢字）→ `japanese`、`约`（簡体字専用）→ `cjk_sc`、`한`（ハングル）→ `korean`
に routing されることを確認。

### 3.2 UTF-8 BOM 対策（副次的な堅牢性）

読み取り自体は正しいが、**先頭に UTF-8 BOM（`EF BB BF`）が付いた**サイドカーだけは
別問題で崩れる:

- JSON: `serde_json::from_slice` は BOM をスキップしないため、BOM 付き JSON は
  **パース失敗 → セクション非表示**になる。
- TXT: 先頭に `U+FEFF` が残り、表示・検索が崩れる。

JSON は RFC 8259 上 BOM を付けない規定だが、一部の Windows ツールが付与するため、
`external_metadata::strip_utf8_bom` で **パース前に先頭 BOM を剥がす**
（`read_search_text` / `read_for_display` の両方に適用）。

## 4. テストケース（`src/external_metadata.rs` の `#[cfg(test)] mod tests`）

入力 → パース後の期待値で `assert_eq!` / `assert!`。

| TC | 入力 | 期待 |
| --- | --- | --- |
| TC1 | 生 UTF-8・BOM 無し JSON | `title`/`name`/`tags` が正しく復元（CP932 誤読しない） |
| TC2 | 多バイト中国語 JSON | そのまま一致 |
| TC3 | `\uXXXX` エスケープ（純 ASCII ファイル） | パーサが解釈して `原神` / `風景` を復元 |
| TC4 | UTF-8 BOM 付き JSON / TXT | BOM を無視してパース。キー名に `U+FEFF` が混ざらない |
| TC5 | 4 バイト UTF-8（絵文字） | `🎨art` を復元 |
| TC6 | 不正バイト混入 JSON / TXT | クラッシュしない（JSON は None、TXT は lossy） |

> 注: CJK の **表示**（フォント被覆）は描画レイヤの問題なので、`tests/ui_snapshot.rs` と
> `ui_fonts::tests` の routing テストで担保する。読み取り（本セクション）とは別レイヤー。

## 5. 関連メモ（任意・別経路）

JSON サイドカー（常に UTF-8）とは別に、画像本体の埋め込みテキストは仕様で
エンコーディングが異なる:

- PNG `tEXt` チャンク … **Latin-1 (ISO-8859-1)**
- PNG `iTXt` チャンク … **UTF-8**
- EXIF `UserComment` … 先頭の文字コード指定子に従う（ASCII/JIS/Unicode 等）

これらを読む経路では、チャンク／フィールド種別ごとに正しいエンコーディングで解釈する
（本件のサイドカーとは別問題だが、同様の取り違えに注意）。
