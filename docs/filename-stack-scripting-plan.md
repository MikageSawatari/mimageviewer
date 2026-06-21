# ファイル名スタック スクリプト化 設計計画 (Rhai)

ステータス: **実装済み (2026-06-21、master)**。v2.0.0 で実装済みの
ファイル名 prefix スタック ([filename-stack-plan.md](filename-stack-plan.md)) を、
**ユーザー定義スクリプトで任意の分割ルールを書ける**ように拡張した。言語は **Rhai**
(純 Rust・crt-static 両立・操作上限つきサンドボックス)。

## 実装サマリ (最終形が正本)

- **モジュール**: ランナー [src/filename_stack_script.rs](../src/filename_stack_script.rs)
  (lib+bin、`group_keys(media, source) -> GroupingResult{keys, rule}` / 内蔵
  `DEFAULT_SCRIPT` / `script_path` / `active_script_source` / `ensure_user_script_exists` /
  `reset_user_script`)。グループ組み立ては [src/filename_stack.rs](../src/filename_stack.rs)
  の `group_by_keys` (スクリプト経由も組み込み既定 `group_media` も共有)。
- **既定スクリプト**: [assets/stack_rules.default.rhai](../assets/stack_rules.default.rhai)
  を `include_str!` で内蔵。ユーザーは `<data_dir>/stack_rules.rhai` で上書き
  (通常版/単体exe版=`%APPDATA%\mimageviewer\`、ポータブル版=exe 隣の `data\`。`data_dir` が吸収)。
- **契約**: `fn group(files)` が files と同じ長さのキー配列を返す (同キー=同スタック、
  2 件以上で畳む)。`files[i] = #{ name, stem, ext, mtime, size, is_video }`。戻り値を
  `#{ rule, keys }` にすると採用ルール名をトースト表示。`()` を返したファイルは単独。
- **公開ヘルパー**: `regex_is_match` / `regex_capture` / `regex_replace` (regex クレート、
  線形時間・コンパイル結果キャッシュ) と `argsort_int` (整数配列→昇順添字)。
- **サンドボックス**: `eval` / `import` 無効 (import 無効で既定 FileModuleResolver 経由の
  ファイル読みを封殺、Codex P1)、`set_max_operations(10M)` ほか各種上限、I/O/OS は非公開。
  戻りキーは文字列 / 数値 / `()` のみ受理 (配列・マップ等はエラー→既定へフォールバック、Codex P2)。
  正規表現コンパイルキャッシュは 1024 件上限 (動的パターン肥大対策、Codex P3)。
- **既定カスケード** (上から順、「全ファイル該当」最初のルールを採用。汎用ルールはさらに
  「異なるスタックが 2 つ以上」のときだけ採用 = 巨大スタック化を防ぐ):
  1. 命名パターン (mXD 例: `^\d{8}_\d{4}_\d+_\d+_p\d+_m\d+_@`、`_m\d+_@` の手前をキー)
  2. 末尾連番 (`^(.+)_([A-Za-z]*\d+)$` の group1。`_001`/`_p0` 等)
  3. 先頭連番 (`^\d+` で始まり、連番が連続したかたまりを gap で分割)
  4. 連写 (mtime が 5 秒以内に連続。クラスタ 2 つ以上で採用)
  どれも該当しなければ全て単独。動画は常に単独 (Rust 側 `group_by_keys` が一意キーへ上書き)。
- **実行場所**: フォルダ読込時 (`build_stack_aggregated`、UI スレッド。既存 `group_media` と
  同所)。スクリプトは CPU 処理 + 操作上限で、別フォルダナビではスタック自動解除のため頻発
  しない。動画はルール判定に渡さず Rust 側で常に単独化 (Codex P2)。失敗 (コンパイル/実行/
  長さ不一致/不正キー) は組み込み既定へフォールバック + トースト + logger。
  (将来、極端に重いユーザースクリプトでも UI を止めないよう走査 worker への移設は候補。)
- **設定**: `stack_script_enabled: bool` (既定 false、`#[serde(default)]`)。UI = 環境設定
  「フォルダ」ページ (スクリプトを開く / 既定に戻す / ヘルプ)。ON/OFF 自体は v2.0.0 同様 transient。
- **マニュアル**: 独立ページ `htdocs/mimageviewer/manual/stack.html` (既定動作 + カスタマイズ +
  AI 依頼テンプレート)。全 23 ページのサイドバーに追加。
- **未リリース新機能なのでマイグレーション不要** (v2.0.0 ごと出荷前)。

---

## 0. 背景と目的

v2.0.0 のスタックは「stem の末尾区切り文字の前」固定 (`prefix_of`)。これは可変部が
**末尾トークン**にある命名 (pixiv `12345678_p0`) しか畳めない。実運用で以下が漏れる:

- **mXD (mxdownloader)**: `..._p01_m01_@artist.jpg` — 可変部 `m{mm}` の後ろに固定の
  `@username` が付くため、末尾区切り規則だとキーに `m01/m02` が残って畳めない。
- **Danbooru 系 (imagedl)**: `0001_<hash>.jpg` — 連番もハッシュも毎回ユニークで、
  prefix 規則では全ファイルが単独化する。まとめたいのは**連番が連続した run** で、
  区切りは「連番の不連続 (gap)」。文字列の共通キーでは表現できない (数値隣接でしか
  切れない)。
- **写真整理**: 「撮影が n 秒以内に連続したショットをまとめる」= 時刻ギャップ判定。

これらは「キー抽出型」「連番 run 型」「時刻バースト型」と**異なるアルゴリズム**で、
固定ルールや正規表現だけでは 1 機構に収まらない。コード (スクリプト) なら 3 種とも
1 つの仕組みで書ける。

### 方針の要点

- **ダウンローダ各社の命名規則をユーザー向け文書/UI に書かない**。既定挙動はスクリプトで
  実装し、内蔵プリセットとして同梱する (規則名や対象サイト名は UI に出さない)。
- **上級ユーザーは外部スクリプトファイルを編集して任意ルールを定義できる**。配置は
  `data_dir` 配下 (通常版/単体exe版=APPDATA、ポータブル版=exe 隣の `data/`)。
- **10k ファイルでも瞬時** を要件にする。そのため既定で渡すのは**追加 I/O 不要の
  「無料フィールド」のみ** (下記 §3)。EXIF 等の重いメタはオプトインの将来拡張 (§9)。

## 1. アーキテクチャ概要

```
フォルダ走査 (worker, 既存)
  → Vec<StackMember> {path, mtime, size, is_video}   ← すべてスキャン済み、追加 I/O なし
  → [スタックモード ON のとき]
       ├ スクリプト有効 & コンパイル成功 → Rhai でグループキー配列を算出 (worker 上)
       │     └ 失敗/タイムアウト/長さ不一致 → トースト + 組み込み既定ルールへフォールバック
       └ スクリプト無効 → 既存 group_media(separator) (組み込み既定)
  → keys[] を Rust 側で group 化 (first-appearance 順, member は入力順, 2件以上で畳む)
  → StackView (既存) → 集約グリッド / フラットフルスクリーン (既存、変更なし)
```

- **純関数性は維持**: スクリプトは「メンバー列 → キー列」を返すだけ。I/O 一切なし。
  `StackView` 以降のビュー・ナビ・製本連携は v2.0.0 のまま無改修。
- **グループ組み立ては Rust 側**: スクリプトはキーを返すだけで、畳む/バッジ/動画単独化の
  最終判断は Rust が持つ (スクリプトに invariant を委ねない)。

## 2. データ契約 (スクリプト I/F)

### 入力

現在のソート順に並べた**メンバー配列**。Rhai では object map の配列で渡す:

```rhai
// files: Array of Map
// files[i] のフィールド (すべて無料 = スキャン済みでメモリ上にある):
//   name:     String   ファイル名 (拡張子つき) 例 "20260429_1100_0003_1234567890_p01_m01_@artist.jpg"
//   stem:     String   拡張子を除いた basename 例 "..._p01_m01_@artist"
//   ext:      String   拡張子 (小文字, ドットなし) 例 "jpg"
//   mtime:    i64      更新時刻 (Unix 秒)。連写/時刻ギャップ判定に使う
//   size:     i64      バイトサイズ
//   is_video: bool     動画か
```

> **重いメタ (EXIF 撮影時刻・カメラ機種・解像度・AI prompt・PNG/XMP テキスト) は
> 既定では渡さない**。1 ファイルずつ読む I/O が要り 10k で秒〜分かかるため。必要に
> なったら §9 のオプトインで追加する。

### 出力

`files` と**同じ長さの文字列配列** (各ファイルのグループキー)。

```rhai
fn group(files) {
    // 例: 既定 (末尾区切りの前)
    files.map(|f| prefix_before_last(f.stem, "_"))
}
```

- 同じキー = 同じスタック。
- **2 件以上**揃ったキーだけ「畳んだスタック」(バッジ表示)。1 件はバッジなし通常セル。
- **動画は常に単独** (MVP): スクリプトが何を返しても、Rust 側で `is_video` のメンバーは
  一意キーへ上書きして単独化する (v2.0.0 の invariant 維持)。動画 run をまとめたい要望が
  出たら設定で解禁を検討。
- **グループ表示順** = キーの初出順 (入力=ソート順に従う)。メンバー順 = 入力順。
- **長さ不一致・非文字列・null** はエラー → フォールバック。

### なぜ「並列キー配列」契約か

per-file の純キー関数 (隣を見られない) では連番 run / 時刻バーストが書けない。全件
リストを渡し**長さ一致のキー配列**を返す形なら、3 パラダイムすべてが表現できる:

```rhai
// 連番 run (gap で切る): stem 末尾の数値を取り、不連続で run 番号を進める
fn group(files) {
    let idx = files.map(|f| trailing_number(f.stem));   // 末尾の数値 (helper)
    let order = sort_indices_by(idx);                    // 数値昇順の添字
    let keys = files.map(|_| "");
    let run = 0; let prev = ();
    for i in order {
        if prev != () && idx[i] - prev > 1 { run += 1; }
        keys[i] = "run_" + run; prev = idx[i];
    }
    keys
}

// 時刻バースト (mtime が n 秒以内で連続): 写真の連写まとめ
fn group(files) {
    let order = sort_indices_by(files.map(|f| f.mtime));
    let keys = files.map(|_| "");
    let burst = 0; let prev = ();
    for i in order {
        if prev != () && files[i].mtime - prev > 5 { burst += 1; }
        keys[i] = "burst_" + burst; prev = files[i].mtime;
    }
    keys
}

// mXD (末尾 @user を無視し _m\d+ の手前をキーに): 正規表現 helper
fn group(files) {
    files.map(|f| regex_capture(f.stem, "^(.+)_m\\d+_@", 1))  // group 1 or 全体 fallback
}
```

## 3. スクリプトエンジン: Rhai

### 依存追加

```toml
# Cargo.toml [dependencies]
rhai = { version = "1", features = ["sync"] }   # sync: 型を Send+Sync にして worker 実行可
regex = "1"                                      # 正規表現 helper の裏付け (純 Rust, crt-static OK)
```

- 純 Rust。C ビルド/crt-static の検証不要 (Cargo.toml:138 の「純Rust で crt-static 両立」
  方針に一致)。
- `regex` も純 Rust。コンパイル済みパターンは `Mutex<HashMap<String, Regex>>` でキャッシュ。

### サンドボックス & 暴走対策

自動実行される**ユーザー編集**スクリプトなので、機械的な上限で防御する:

```rust
let mut engine = Engine::new();
engine.disable_symbol("eval");                 // eval 禁止
engine.set_max_operations(50_000_000);         // 無限ループ backstop
engine.set_max_call_levels(64);
engine.set_max_expr_depths(64, 64);
engine.set_max_string_size(64 * 1024);
engine.set_max_array_size(2_000_000);          // 10k×数倍の余裕
// file/io/process は一切 register しない (Rhai は deny-by-default)
// register する helper だけ:
//   prefix_before_last(s, sep) / regex_capture(s, pat, n) / trailing_number(s)
//   sort_indices_by(arr) など §2 の例で使う純関数
```

### 実行場所 (UI 応答性)

- **スクリプト経路はフォルダ走査 worker 上で実行**し、キー配列を結果に同梱して UI へ返す
  (CLAUDE.md「UI スレッド同期 I/O は即 worker 化」)。10k の map 構築 + Rhai 評価は数〜
  数十 ms 想定だが、UI スレッドでは行わない。キャンセルは既存の走査 cancel に相乗り。
- **組み込み既定ルール (`group_media`) は安価なので従来どおりインラインで可**。スクリプト
  経路だけ worker 化する。
- perf::event を挿す: `stack/script_eval` (件数 + 所要 + ops + フォールバック有無)。
  タイムアウト (例 200ms 相当の ops 到達) でフォールバック。

### フォールバック

スクリプトの「コンパイル失敗 / 実行エラー / タイムアウト / 出力長不一致」はすべて
**組み込み既定ルール (separator 方式) に倒し、トーストで通知**する。スタックが
無効化したり panic したりしない。

## 4. スクリプトの配置・既定・編集

- **パス**: `data_dir::get().join("stack_rules.rhai")`。`data_dir` が環境を吸収するので
  特別扱い不要 (通常版/単体exe版=`%APPDATA%\mimageviewer\`、ポータブル版=`<exe_dir>\data\`
  — [data_dir.rs:162-170](../src/data_dir.rs))。
- **既定スクリプト**: `include_str!("../assets/stack_rules.default.rhai")` で exe に内蔵。
  ファイルが無くても常に動く。
- **ユーザーファイルは勝手に上書きしない**。環境設定に:
  - 「スクリプトを開く」: 無ければ既定をその場に書き出してから関連付けエディタで開く
    (opener)。
  - 「既定に戻す」: 内蔵既定で上書き (確認ダイアログ)。
  - 「再読込/テスト」: 現在フォルダに対してドライラン → グループ数/最大スタック/エラーを表示。
- **プリセット**: 内蔵既定スクリプトの中に「キー抽出」「連番 run」「時刻バースト」の関数を
  コメントつきで同梱し、`group()` で呼び分ける雛形にする (ユーザーは 1 行差し替えで切替)。
  規則名・対象サイト名は書かない (中立な用途説明のみ)。

## 5. 設定項目 (settings)

v2.0.0 自体が**未リリース**なので `stack_separator` 含めマイグレーション不要・破壊的変更可
(CLAUDE.md「永続データ・スキーマ変更時の判断」)。

| フィールド | 型 | 既定 | 意味 |
| --- | --- | --- | --- |
| `stack_separator` | char | `_` | **組み込み既定ルール用**の区切り文字 (スクリプト無効時に使用)。既存のまま流用。 |
| `stack_script_enabled` | bool | `false` | true かつスクリプトファイルが有効なら Rhai 経路、それ以外は組み込み既定。`#[serde(default)]`。 |

- スクリプトパスは `data_dir` 由来で**設定に保存しない** (環境追従のため)。
- スタックモードの ON/OFF 自体は従来どおり transient (永続化しない)。

## 6. パフォーマンス

- **10k 瞬時の根拠**: 渡すのは全部スキャン済みの無料フィールド (追加 I/O ゼロ)。Rhai 評価は
  worker 上 + 操作上限つき。
- **ボトルネック注意**: Rhai はインタプリタで per-op オーバーヘッドがある。10k×重い
  per-item 処理 (毎回 regex + 文字列連結) で数十 ms。許容範囲だが:
  - helper (regex_capture / trailing_number) は **Rust 側 register 関数**にして per-item
    コストを Rust に寄せる。
  - profiling で不足なら、入力を map 配列 → **並列プリミティブ配列** (`names[]`, `stems[]`,
    `mtimes[]` …) に切替えて map 確保を削る (契約はそのまま、helper でラップ)。
- **計測**: `scripts/analyze_perf.py` に `stack` 集計を追加。`stack/script_eval` の
  件数別 latency を見る。

## 7. 動画・横断系の整合 (v2.0.0 から不変)

- 動画は常に単独 (§2)。
- 集約セルを開く→フラット読書、Shift+↓↑ ジャンプ、製本連携 (`stack_member_paths`)、
  自動解除はすべて v2.0.0 のまま。スクリプトは「キーの作り方」だけを差し替える。
- Ctrl+F / facet の集約セル照合 (隠れメンバー未評価) も v2.0.0 制限を踏襲。

## 8. テスト

- **純ロジック (Rust)**: keys[] → group 化 (first-appearance 順 / 2件で畳む / 動画単独化 /
  長さ不一致でエラー)。
- **Rhai 評価 (Rust から実行)**: 既定スクリプトが `group_media` と同一結果になる回帰
  (separator 各種)。mXD/連番 run/時刻バーストの代表入力で期待グループ。
- **サンドボックス**: `eval` 禁止 / 無限ループが max_operations で止まる / file 系 helper が
  未登録であること。
- **フォールバック**: 構文エラー・出力長不一致・例外で組み込み既定へ倒れる。
- **置き場所**: `data_dir` override 下でスクリプト読込 (TestDataDirGuard)。

## 9. 将来拡張: Tier 2 (重いメタのオプトイン)

- 設定 `stack_use_capture_time: bool` (既定 false) 等で、EXIF `DateTimeOriginal` を
  **ワーカーで先読み + キャッシュ + キャンセル** (既存 `start_metadata_load` 系) してから
  追加フィールド (`exif_time: i64` など) としてスクリプトに渡す。
- 既定 OFF。ON 時のみ重い経路に入る。10k では「初回だけ索引化に時間、以後キャッシュ」と
  明示する。
- これにより「ファイルシステム mtime では不正確な撮影時刻」も厳密に扱える。MVP では
  mtime ベースの連写まとめで十分なケースが多い。

## 10. 実装ステップ (v2.0.0 マージ後)

1. `rhai` + `regex` 依存追加、`assets/stack_rules.default.rhai` 内蔵。
2. `filename_stack` に「keys[] → Vec<StackGroup>」純関数を追加 (現 `group_media` を
   keys 経由に内部リファクタ; 既定 separator ルールはその keys を生成する薄い関数に)。
3. Rhai ランナー (engine 構築 + helper register + サンドボックス + 実行 + 検証 +
   フォールバック)。worker 実行配線。
4. 設定 (`stack_script_enabled`) + 環境設定 UI (開く/既定に戻す/テスト)。
5. perf 計装 + `analyze_perf.py stack`。
6. テスト一式 (§8)。
7. ドキュメント更新: [spec.md](spec.md) (設定項目) / [filename-stack-plan.md](filename-stack-plan.md)
   (スクリプト経路への言及) / [architecture-overview.md](architecture-overview.md) (新依存) /
   htdocs マニュアル・製品ページ (内部用語・ダウンローダ名を出さず「任意ルールで
   まとめられる」程度の中立表現)。

## 11. 言語選定の記録

Rhai 採用 (2026-06-21)。比較:

| | Rhai (採用) | Lua (mlua vendored) |
| --- | --- | --- |
| 同梱 | Cargo 1 行、C ビルド不要 | C を cc でビルド |
| crt-static | 問題なし (純 Rust) | /MT 整合の検証要 (VCRUNTIME140 非依存; Vector 要件) |
| 暴走対策 | `set_max_operations` 等を標準装備 | 命令フックを自前設定 |
| サンドボックス | deny-by-default | io/os を読み込まない運用 |
| 速度 | インタプリタ (worker + 無料フィールドで 10k 十分) | ほぼネイティブ |
| 知名度 | 低 | 高 |

本プロジェクトの「純 Rust で crt-static 両立」方針 (Cargo.toml:138) と、自動実行される
ユーザー編集スクリプトの暴走を機械的に止められる点から Rhai を選定。Lua の唯一の利点
(知名度) はプリセット同梱で吸収する。
