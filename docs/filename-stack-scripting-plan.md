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
  2 件以上で畳む)。**files に渡るのは画像のみ** (`#{ name, stem, ext, mtime, size }`。フォルダ/
  ZIP/PDF は passthrough、動画は常に単独なので渡さない → `is_video` は公開しない)。戻り値を
  `#{ rule, keys }` にすると採用ルール名をトースト表示。`()` を返したファイルは単独。
- **公開ヘルパー** (全て native = O(n) 高速。全件の判定/集計/並べ替えは必ずこれを使い、Rhai で
  全件ループ/マップ構築しないこと。マップ 1 件ずつ追加は O(n²) で 10 万件 16 秒の罠):
  `regex_is_match` / `regex_capture` / `regex_replace` (regex クレート、線形時間・キャッシュ) /
  `argsort_int` (整数配列→昇順添字) / `stack_all_matched(keys)` (() が無い=全件該当か) /
  `stack_distinct(keys)` (異なるキー数、() 無視)。
- **サンドボックス**: `eval` / `import` 無効 (import 無効で既定 FileModuleResolver 経由の
  ファイル読みを封殺、Codex P1)、`set_max_operations(10M)` ほか各種上限、I/O/OS は非公開。
  戻りキーは文字列 / 数値 / `()` のみ受理 (配列・マップ等はエラー→既定へフォールバック、Codex P2)。
  正規表現コンパイルキャッシュは 1024 件上限 (動的パターン肥大対策、Codex P3)。
- **標準関数 (§1.863)**: 互換のため標準パッケージを維持し、`sleep` と空文字で伸ばす文字列 `pad` を拒否、配列 `pad` の巨大長さによるあふれを事前検査で防止。`print` / `debug` は 1 回の計算で合計 20 行まで mIV のログへ送る (下記「標準関数の扱い」)。
- **既定カスケード** (上から順、「全ファイル該当」最初のルールを採用。汎用ルールはさらに
  「異なるスタックが 2 つ以上」のときだけ採用 = 巨大スタック化を防ぐ):
  1. 命名パターン (mXD 例: `^\d{8}_\d{4}_\d+_\d+_p\d+_m\d+_@`、`_m\d+_@` の手前をキー)
  2. 末尾連番 (`^(.+)_([A-Za-z]*\d+)$` の group1。`_001`/`_p0` 等)
  3. 先頭連番 (`^\d+` で始まり、連番が連続したかたまりを gap で分割)
  4. 連写 (mtime が 5 秒以内に連続。クラスタ 2 つ以上で採用)
  どれも該当しなければ全て単独。動画は常に単独 (Rust 側 `group_by_keys` が一意キーへ上書き)。
- **常にスクリプトで分類 (チェック有無に関わらず)**:
  - 設定 `stack_script_enabled` が **OFF でも内蔵既定スクリプト** (`DEFAULT_SCRIPT` = カスケード)
    で分類する。ON のときユーザーの `stack_rules.rhai` (無ければ内蔵既定)。旧「OFF = 末尾連番
    だけの同期 separator ルール」は廃止 (`build_stack_aggregated` も削除)。`group_media`
    (separator) は **スクリプト失敗時のフォールバック専用**。
  - **どちらもワーカー実行**。ユーザースクリプトも内蔵既定も任意に重くなり得る (実測: 既定
    カスケード release で 10k=72-144ms / 100k=0.7-1.4s) ので UI スレッドで走らせない。
    トグル時はまず通常フォルダを表示し、グループ分けを `stack-script` スレッドで実行、完了後
    `poll_stack_script` が **`start_loading_items` 経由** で集約ビューへ差し替える (動画サムネ起動 /
    キャッシュ保護 / 世代更新を同期パスと同形にするため。`swap_stack_view_items` は不可=Codex P2)。
    フルスクリーン中は差し替えを保留し閉じてから適用。`StackScriptPending` を `cancel`/folder で
    世代管理し、別フォルダ移動・スタック OFF・再ロードで cancel。動画はルール判定に渡さず Rust 側で
    常に単独化 (Codex P2)。失敗 (コンパイル/実行/長さ不一致/不正キー) は組み込み既定へフォールバック
    + トースト + logger。
  - **トグル時のカーソル保持**: ON にした瞬間のカーソル画像パスを `stack_toggle_select_path` に
    控え、集約完了時に `StackView::aggregated_index_for_member_path` でその画像を含むスタックセルを
    選択する。OFF 側は通常フォルダで `select_after_load` (ファイル名) によりその代表画像を選択する。
  - **計測テスト**: `filename_stack_script::tests::perf_10k_default_script` (`#[ignore]`、
    `cargo test --release ... -- --ignored --nocapture` で実行)。`distinct`/`all_matched` は
    Rust native (HashSet/iter)。Rhai 実装だと挿入毎のデータサイズ再計算で O(n²) (10 万件 16 秒)
    になるため native 化必須。`max_string_size` は配列内文字列バイト総量を見るので 256MB、
    `max_operations` は 100M (100k の正当処理 ~数百万 op に対し十分・暴走は阻止)。
- **設定**: `stack_script_enabled: bool` (既定 false、`#[serde(default)]`)。OFF=内蔵既定
  スクリプト / ON=ユーザー `stack_rules.rhai`。どちらもスクリプトで分類する (OFF が separator
  だけだった旧挙動は廃止)。UI = 環境設定「フォルダ・ファイル」ページ (スクリプトを開く / 既定に戻す /
  ヘルプ)。スタックモードの ON/OFF 自体は v2.0.0 同様 transient。`stack_separator` は失敗時
  フォールバック専用。
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
                                                       (is_video は Rust 側の動画単独化用。
                                                        スクリプトには渡さない)
  → [スタックモード ON のとき] 常に worker で Rhai を実行 (画像のみ渡す)
       ├ スクリプト設定 ON  → ユーザーの stack_rules.rhai (無ければ内蔵 DEFAULT_SCRIPT)
       ├ スクリプト設定 OFF → 内蔵 DEFAULT_SCRIPT (= 既定カスケード。これも worker で実行)
       └ どちらも 失敗/コンパイルエラー/長さ不一致 → トースト + group_media(separator) へ
            フォールバック (= フォールバック専用経路。通常の OFF はここを通らない)
  → keys[] を Rust 側で group 化 (first-appearance 順, member は入力順, 2件以上で畳む)
  → StackView (既存) → 集約グリッド / フラットフルスクリーン (既存、変更なし)
```

- **純関数性は維持**: スクリプトは「メンバー列 → キー列」を返すだけ。I/O 一切なし。
  `StackView` 以降のビュー・ナビ・製本連携は v2.0.0 のまま無改修。
- **グループ組み立ては Rust 側**: スクリプトはキーを返すだけで、畳む/バッジ/動画単独化の
  最終判断は Rust が持つ (スクリプトに invariant を委ねない)。

## 2. データ契約 (スクリプト I/F)

### 入力

現在のソート順に並べた**画像メンバー配列** (動画は渡さない。後述「動画は常に単独」)。
Rhai では object map の配列で渡す:

```rhai
// files: Array of Map (画像のみ。動画・フォルダ・ZIP/PDF は渡さない)
// files[i] のフィールド (すべて無料 = スキャン済みでメモリ上にある):
//   name:     String   ファイル名 (拡張子つき) 例 "20260429_1100_0003_1234567890_p01_m01_@artist.jpg"
//   stem:     String   拡張子を除いた basename 例 "..._p01_m01_@artist"
//   ext:      String   拡張子 (小文字, ドットなし) 例 "jpg"
//   mtime:    i64      更新時刻 (Unix 秒)。連写/時刻ギャップ判定に使う
//   size:     i64      バイトサイズ
// ※ is_video は渡さない: 動画は常に単独なのでそもそもスクリプトに来ない。
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
- **動画は常に単独** (MVP): 動画はそもそもスクリプトに渡さない (画像のみ渡す)。Rust 側
  (`spawn_stack_script_worker` が画像だけ抽出 → 結果適用時に動画を一意キーへ) が単独化を
  保証する (v2.0.0 の invariant 維持)。動画 run をまとめたい要望が出たら設定で解禁を検討。
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

### 標準関数の扱い (§1.863、v5.7.0)

既存の利用者のスクリプトが使う関数を減らさないため、`Engine::new()` の標準パッケージを
残す。§1.363 (操作のスクリプト) は `Engine::new_raw()` + 個別登録だが、スタック用は
互換を優先して揃えない。処理のモーダル化や取消の状態追加では native 関数内の停止を
解消できないため、関数登録の境界で危ない呼び出しをなくす。

`FuncRegistration::register_into_engine` は `register_fn` と同じく `global_modules[0]` の
global namespace に登録され、標準パッケージより先に解決される。同名・同じ引数型で次を覆う
(Rhai 1.27.0 のソースを照合)。標準と同じく `pad` は非 pure / 非 volatile、
`sleep` は pure / volatile と明示する。`register_fn` の既定 pure のままでは、
標準の `pad` が拒否する定数の変更を許してしまうため使用しない。

| 覆う関数 | 理由と維持する動作 |
| --- | --- |
| `sleep(INT)` / `sleep(FLOAT)` | native 内の `std::thread::sleep` は秒数を制限しない。0 を含め常に `ErrorRuntime`。 |
| `pad(&mut ImmutableString, INT, &str)` | 空文字で現在の文字数より長くすると、標準実装のループが進まない。この場合だけ `ErrorRuntime`。それ以外は 1.27 の文字数単位の繰り返し・末尾切り詰め・非正の長さでの無操作・拡張前の文字数上限と拡張後のバイト数上限を維持する。 |
| `pad(&mut Array, INT, Dynamic)` | 標準の既存不具合: 巨大な長さと入れ子の詰め物で総量の `a + pad + ax * pad` 等があふれ、debug は計算時、release は検査を抜けた後の `resize` で worker が panic し得る。長さが非0の `max_array_size` を超えれば先に `ErrorDataTooLarge("Size of array/BLOB")`。それ以降は従来の無操作条件・総量検査・resize を維持する。 |

定数の文字列への `pad` は、空でない詰め物でも空文字でも、長さを変える必要がなくても
標準と同じ定数変更のエラーで拒否する。空文字のループに入る前の判定である。
`section1863_string_pad_preserves_standard_const_rejection` で標準 `Engine::new()` と
結果・エラー・定数の値を突き合わせる。

配列の総量は `eval/data_check.rs` の数え方を公開 `Dynamic` API で再現する。
配列は要素数と入れ子の総量、マップは値の項目数と総量 (キーの長さは含めない)、文字列は
バイト数、blob は最上位ではバイト数、入れ子では追加の 1 も数える。最上位の Shared は
`read_lock::<Dynamic>` で開き、内部要素に Shared が無い不変条件も標準と同じとする。
長さの事前検査後、総量検査の優先順は文字列→配列/blob→マップ。総量の加算・乗算は saturating 演算にし、
通常上限の前提をホスト側で変えても折り返さない。

上書きの優先と関数ポインタ経由の解決を確認する常設テスト (すべて
`filename_stack_script::tests::` 配下、別スレッド + `recv_timeout(10秒)`):

- `section1863_global_overrides_reject_sleep_int_float_and_zero`
- `section1863_global_overrides_reject_empty_pad_method_and_function`
- `section1863_global_overrides_reject_sleep_function_pointers_and_curry`
- `section1863_global_overrides_reject_empty_pad_function_pointers`

互換は `section1863_safe_string_and_array_pad_remain_compatible`、サイズ検査は
`section1863_string_pad_preserves_character_and_byte_size_checks`、既定ルールは既存の
`default_script_compiles_and_runs_empty` 等で確認する。2026-11-23 に ClaudeCode が
`filename_stack_script::` / `filename_stack` / `grid_edge_buttons_1855::` の 93 件を流し、すべて成功した
(`c938e318f` の変更)。独立レビュー後の登録属性・配列 pad の修正と追加テストを含めた
同じ範囲の 98 件と ignore の 2 件も、同日に ClaudeCode が流してすべて成功した (`092cce4f7`)。

#### StandardPackage の洗い出し (Rhai 1.27.0、コード読み)

`pkg_std.rs` / `pkg_core.rs` から `src/packages/*.rs` の全構成をたどった。
上記以外に、入力サイズや interpreter の操作上限と無関係に待機・無限ループ・
ブロックする I/O を行う公開関数は見つからなかった。次は覆わない。

| 対象ファイル / 関数群 | 判断 |
| --- | --- |
| `lang_core.rs` のその他 (`exit`、`take`、tag、`parse_json`、関数メタデータ) | 有限データの操作・解析、または即時終了。JSON は入力文字列上限、反射は有限の AST が対象。ファイル・ネットワーク I/O はない。 |
| `arithmetic.rs` / `bit_field.rs` / `logic.rs` / `math_basic.rs` | 固定幅数値の計算・比較、文字列上限内の数値解析。指数は固定幅または明示上限。decimal は現行の依存 feature では無効 (実装も確認)。 |
| `string_basic.rs` / `string_more.rs` の char 版 `pad` と空文字以外の文字列版 `pad` | `max_string_size = 256 MiB` の拡張前後の検査で縛られ、ループが必ず進むので終わる。文字数とバイト数は異なり、バイト検査は拡張後。 |
| 同上の検索・置換・split・大文字小文字変換・文字列化 | 入力文字列・配列・マップが有限で、サイズ上限により対象量が縛られる。Rust の走査や split は空の検索文字列でも進む。結果サイズ検査が native 呼び出し後のものもあり、処理中の厳密な時間・一時メモリ上限は保証しない。 |
| `array_basic.rs` の `pad` 以外 (deprecated の別名も含む) | 走査・sort・dedup・drain・retain は `max_array_size = 8,000,000` 内の要素が対象。削除ループは添字増加か配列縮小で進む。スクリプトのコールバックは interpreter の操作上限対象。配列 `pad` は初回監査で総量計算のあふれを見落としており、上の「覆う関数」へ移した。 |
| `blob_basic.rs` | `blob` の生成・`pad` は `max_array_size` で拡張前検査。他の走査・変換は有限の blob / 文字列が対象。`write_*` はメモリ内のバイト列を更新する関数で、外部 I/O ではない。 |
| `map_basic.rs` | `max_map_size = 8,000,000` 内のマップを走査・結合。コールバックは interpreter の操作上限対象。JSON 変換も有限データが対象。 |
| `iter_basic.rs` | range 自体は列挙せず、`next` 1 回で 1 要素。スクリプトの `for` は操作上限対象。chars の準備は文字列上限内の走査、bits は固定ビット幅。 |
| `fn_basic.rs` / `time_basic.rs` | 関数ポインタの名前・属性取得、現在時刻の取得・差・加減算。待機はしない。`call` / curry は組み込みの解決経路で、上書きした関数へ到達する。 |
| `debugging.rs` | 現行は debugging feature 無効。有効時の `back_trace` も有限の呼び出しスタック走査だけ。 |
| `mod.rs` / `pkg_core.rs` / `pkg_std.rs` | パッケージの構成・登録のみ。 |

上限内でも 1 回の native 処理が重くなり得ることは残る。今回は厳密な時間制限を新設せず、
既存の `set_*` の上限値とキャンセル確認の `ops & 0xFFFF` は変更しない。

#### 整数引数のあふれの再監査 (独立レビュー後、コード読み)

全標準パッケージの長さ・回数・位置の計算を、負数・`INT::MIN`・`INT::MAX` と
加算／減算／乗算の順序から確認した。追加で覆う必要があるものは配列 `pad` のみ。

| 関数群 | あふれの観点の確認 |
| --- | --- |
| 配列／blob の `extract`・`splice`・`chop`・`truncate`・`split`・`drain`・`retain`・get/set/remove | `calc_offset_len` / `calc_index` は `unsigned_abs` と実体長への制限を使う。切り出し後の `start + len` は実体長以下。閉区間は終端を `INT::MAX - 1` 以下へ制限し、`end - start + 1` はあふれない (開始が MAX なら長さ 0)。 |
| blob の生成／pad／数値・文字列の read/write | 生成／pad は長さの上限を割り当て前に検査。read/write の範囲は実体長に制限し、数値は固定バイト幅にも制限。 |
| 文字列の `sub_string`・`crop`・`split`・get/set/index_of・truncate/pop・両 pad | 負位置は `unsigned_abs`、範囲の加算は `checked_add` または実体長による制限。閉区間の終端も制限。pad の文字数加算は既存の 256 MiB の入力／目標長の範囲内。 |
| `get_bit(s)` / `set_bit(s)` | ビット位置は固定幅内へ制限し、`bits + start` は `checked_add`。閉区間は終端の固定幅検査が加減算より先。全幅の mask 計算はその前に分岐して避けるため、冪乗・符号付き整数への変換もあふれない。 |
| range / chars / bits の iterator | 整数 step の加算は checked。文字列の位置は `unsigned_abs` / `saturating_sub`、ビット数は固定幅と `checked_add`。長い列挙も interpreter の操作上限対象。 |
| 数値演算・parse・tag・時刻・その他 | 整数演算は checked、shift の負数は `checked_abs`、型変換・tag は範囲を検査。時刻の整数の負数は `unsigned_abs`、加減算は checked。残りのパッケージに利用者指定の長さで行う未検査の割り当て計算は見つからなかった。 |

追加の常設テスト (すべて `filename_stack_script::tests::`):
`section1863_array_pad_rejects_overflow_lengths_without_panicking`、
`section1863_array_pad_matches_standard_results_and_aggregate_limits`、
`section1863_data_sizes_match_standard_for_top_shared_and_nested_blobs`。
報告例の長さと `INT::MAX` の即時 Err、通常値・入れ子・既存要素・blob・Shared・各総量上限の
ちょうど／1 超過を標準エンジンと比較する。通常上限内の重い処理は従来のままとする。

#### エラーと出力

- エラー本文は `スタックのスクリプトでは sleep は使えません (処理が終わらなくなるため)`、
  `スタックのスクリプトでは、空の文字列を詰める pad は使えません (処理が終わらなくなるため)`。
  既存の `スクリプトの実行に失敗: {e}` で包まれ、既存の失敗経路でログ・
  「スタックのスクリプトでエラー。既定ルールで表示します (詳細はヘルプ参照)」の
  トーストと既定ルールへのフォールバックに進む。新しい失敗経路は足さない。
- `on_print` / `on_debug` を `crate::logger::log` へ配線し、行頭を
  `stack script print: ` / `stack script debug: ` とする。debug は source / position も記録する。
  1 `CompiledStackScript` (親フォルダごとの複数呼び出しも共有) の合計 20 行まで。
  複数行の print も行ごとに数え、超過時だけ 1 回
  `stack script print/debug: 21 行目以降は省略しました` を記録し、以後は捨てる。
  新しいコンパイルで枠が戻る。出力先を差し替える同じ配線 helper で 30 回の print、
  print/debug 混在・親別呼び出し・source/position・複数行と枠の再作成をテストする。
- 修正前の再現専用 ignore テスト:
  `section1863_legacy_sleep_ignores_cancel_and_operation_limit` /
  `section1863_legacy_empty_pad_ignores_cancel_and_operation_limit`。
  `Engine::new()` + 従来と同じ上限と on_progress を再現し、操作上限を 64 に下げ、
  native 直前の通知後に cancel を立てても 3 秒で戻らないことを検査する。
  sleep の待機スレッド / CPU を 1 コア回し続ける pad のスレッドを残すため通常は実行しない。
  2026-11-23 に ClaudeCode が nextest の `--run-ignored only` で 2 件を各 1 回流し、
  どちらも成功した (= 修正前の構成では、cancel を立てても操作数上限 64 でも 3 秒以内に戻らない)。
  nextest はテストごとに別プロセスなので、残ったスレッドはプロセスの終了で回収される。
  対照の常設テスト `section1863_legacy_script_loop_observes_cancel_and_terminates` も追加した。
  同じ修正前 helper で `loop {}` の開始後に cancel を立て、10 秒以内に
  `ErrorTerminated` になることを検査する (操作上限超過による停止とは区別する)。

### 実行場所 (UI 応答性)

- **スクリプト経路はフォルダ走査 worker 上で実行**し、キー配列を結果に同梱して UI へ返す
  (CLAUDE.md「UI スレッド同期 I/O は即 worker 化」)。10k の map 構築 + Rhai 評価は数〜
  数十 ms 想定だが、UI スレッドでは行わない。キャンセルは既存の走査 cancel に相乗り。
- **通常経路はスクリプト設定 ON/OFF とも worker 上の Rhai 実行**。OFF でも内蔵 `DEFAULT_SCRIPT`
  を worker で走らせる (= かつての「OFF=インライン separator」経路は廃止)。
- **`group_media` は失敗時フォールバック専用**で、スクリプトがコンパイル/実行/長さ不一致で
  失敗したときだけ結果適用時 (UI スレッド) に走る。安価なのでこの稀な経路はインラインで可。
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
| `stack_separator` | char | `_` | **失敗時フォールバック (`group_media`) 用**の区切り文字。スクリプトが失敗したときだけ使う。既存のまま流用。 |
| `stack_script_enabled` | bool | `false` | true ならユーザーの `stack_rules.rhai` (無ければ内蔵 `DEFAULT_SCRIPT`)、false でも内蔵 `DEFAULT_SCRIPT` を worker 実行。どちらも失敗時のみ `group_media` へフォールバック。`#[serde(default)]`。 |

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
