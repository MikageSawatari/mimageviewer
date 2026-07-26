//! Tantivy + bigram プロトタイプ計測 (docs/archive/search-metadata/search-expansion-design.md §15.1.1)。
//!
//! 目的:
//!  1. Tantivy + NgramTokenizer(2,2) + post-filter 方式でグローバルメタ検索の速度が許容範囲か判定
//!  2. 特に `TopDocs::with_limit(500).and_offset(offset)` の offset 肥大時の worst case を測定
//!  3. SQLite 側 post-filter (`WHERE path IN (?,?,...)` で 500 件一括取得) の往復コストも測定
//!
//! 測定シナリオ (§15.1.1):
//!  - rare:    候補総数 100〜1000 件 (平均ケース、1〜2 ページで完結)
//!  - medium:  候補総数 10,000 件、HARD_MAX 近くまでページング
//!  - generic: 候補総数 50,000 件の超偽陽性ケース (汎用的な 2 文字 bigram)
//!
//! 使い方:
//!   cargo run --release --bin bench_search
//!   cargo run --release --bin bench_search -- --docs 50000
//!   cargo run --release --bin bench_search -- --docs 200000 --keep  (index を消さない)
//!
//! 結果は stdout に表、要点はリポ直下の `bench_search_results.md` に追記される。

use std::time::{Duration, Instant};

use rusqlite::Connection;
use tantivy::collector::TopDocs;
use tantivy::query::{BooleanQuery, Occur, Query, TermQuery};
use tantivy::schema::{IndexRecordOption, STORED, STRING, Schema, Value};
use tantivy::tokenizer::{LowerCaser, NgramTokenizer, TextAnalyzer, Token, TokenStream, Tokenizer};
use tantivy::{Index, IndexWriter, Term, doc};

// ============================================================================
// 設定
// ============================================================================

const DEFAULT_NUM_DOCS: usize = 100_000;
const INDEX_WRITER_HEAP_MB: usize = 128;
const PAGE_SIZE: usize = 500;
const HARD_MAX: usize = 10_000;
const BIGRAM_TOKENIZER_NAME: &str = "bigram";

// ============================================================================
// 合成コーパス生成
// ============================================================================

/// 画像メタデータを模した合成ドキュメント。
/// 実データ (EXIF + XMP + AI prompt) の統計的な分布を雑に再現する。
struct SyntheticDoc {
    path: String,
    all_text: String, // ~2KB の合成テキスト
}

/// xorshift64 で決定的な疑似乱数 (テスト再現性のため seed 固定)。
struct Rng {
    state: u64,
}
impl Rng {
    fn new(seed: u64) -> Self {
        Self { state: seed.max(1) }
    }
    fn next(&mut self) -> u64 {
        let mut x = self.state;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.state = x;
        x
    }
    fn pick<'a, T: ?Sized>(&mut self, slice: &'a [&'a T]) -> &'a T {
        slice[(self.next() as usize) % slice.len()]
    }
    fn range(&mut self, min: usize, max: usize) -> usize {
        min + (self.next() as usize) % (max - min).max(1)
    }
}

// 現実的な日本語・英語の語彙サンプル。
// "rare" "medium" "generic" の周期ビンに分けてあるので、
// クエリ側でこのビンを使って頻度をコントロールできる。
const JP_GENERIC: &[&str] = &[
    "の",
    "と",
    "が",
    "は",
    "を",
    "に",
    "で",
    "から",
    "まで",
    "カメラ",
    "写真",
    "画像",
    "風景",
    "人物",
    "女性",
    "男性",
];
const JP_MEDIUM: &[&str] = &[
    "夕焼け",
    "海辺",
    "雪景色",
    "紅葉",
    "桜並木",
    "猫",
    "犬",
    "空港",
    "駅前",
    "商店街",
    "庭園",
    "神社",
    "寺院",
    "温泉",
    "街並み",
    "夜景",
    "霧",
    "虹",
    "湖畔",
    "公園",
];
const JP_RARE: &[&str] = &[
    "美濃焼",
    "黒楽茶碗",
    "斑鳩宮",
    "飛鳥時代",
    "曜変天目",
    "龍泉窯",
    "金継ぎ",
    "瑠璃色",
    "銅鏡研磨",
    "蒔絵螺鈿",
    "千歳飴屋台",
];
const EN_COMMON: &[&str] = &[
    "the",
    "and",
    "in",
    "of",
    "with",
    "for",
    "from",
    "by",
    "at",
    "on",
    "photo",
    "image",
    "camera",
    "landscape",
    "portrait",
    "beach",
    "sunset",
    "night",
    "city",
    "forest",
];
const EN_AI: &[&str] = &[
    "stable diffusion",
    "lora",
    "sampler",
    "euler",
    "dpm++",
    "karras",
    "cfg",
    "scale",
    "steps",
    "seed",
    "negative prompt",
    "clip skip",
    "hires fix",
    "refiner",
    "controlnet",
    "ipadapter",
];
const CAMERAS: &[&str] = &[
    "SONY ILCE-7M4",
    "Canon EOS R5",
    "Nikon Z9",
    "FUJIFILM X-T5",
    "iPhone 15 Pro",
    "Pixel 8 Pro",
    "RICOH GR IIIx",
    "Leica Q3",
];
const LENSES: &[&str] = &[
    "FE 24-70mm F2.8 GM",
    "EF 70-200mm f/2.8L",
    "NIKKOR Z 50mm f/1.2 S",
    "XF 16-55mm f/2.8 R",
    "Summilux 35mm f/1.4",
];

fn generate_corpus(num_docs: usize, seed: u64) -> Vec<SyntheticDoc> {
    let mut rng = Rng::new(seed);
    let mut docs = Vec::with_capacity(num_docs);
    for i in 0..num_docs {
        docs.push(make_doc(i, &mut rng));
    }
    docs
}

fn make_doc(idx: usize, rng: &mut Rng) -> SyntheticDoc {
    // path: 疑似的なフォルダ階層
    let folder = rng.range(0, 100);
    let sub = rng.range(0, 20);
    let path = format!(
        "d:/photos/fav_{:03}/sub_{:02}/img_{:06}.jpg",
        folder, sub, idx
    );

    // all_text: ~2KB の合成テキスト
    let mut text = String::with_capacity(2200);

    // ファイル名を先頭に (検索ヒットさせやすく)
    text.push_str(&format!("img_{:06}.jpg ", idx));

    // EXIF 風
    text.push_str(rng.pick(CAMERAS));
    text.push(' ');
    text.push_str(rng.pick(LENSES));
    text.push_str(&format!(
        " ISO{} f/{:.1} 1/{} ",
        rng.range(100, 6400),
        (rng.range(14, 280) as f32) / 10.0,
        rng.range(30, 2000)
    ));

    // 日本語 (generic を多め、medium を中、rare を少量)
    for _ in 0..rng.range(8, 16) {
        text.push_str(rng.pick(JP_GENERIC));
        text.push(' ');
    }
    for _ in 0..rng.range(2, 6) {
        text.push_str(rng.pick(JP_MEDIUM));
        text.push(' ');
    }
    if rng.next() % 20 == 0 {
        // 20 件に 1 件だけ rare が混じる → rare クエリの期待ヒット数 ≈ num_docs / 20
        text.push_str(rng.pick(JP_RARE));
        text.push(' ');
    }

    // 英語
    for _ in 0..rng.range(6, 12) {
        text.push_str(rng.pick(EN_COMMON));
        text.push(' ');
    }
    for _ in 0..rng.range(1, 4) {
        text.push_str(rng.pick(EN_AI));
        text.push(' ');
    }

    // 詰め物: 2KB に満たない場合は generic を追加
    while text.len() < 2000 {
        text.push_str(rng.pick(JP_GENERIC));
        text.push(' ');
    }

    SyntheticDoc {
        path,
        all_text: text,
    }
}

// ============================================================================
// Tantivy インデックス構築
// ============================================================================

fn build_schema() -> (
    Schema,
    tantivy::schema::Field,
    tantivy::schema::Field,
    tantivy::schema::Field,
) {
    let mut b = Schema::builder();
    let path = b.add_text_field("path", STRING | STORED);
    let name = b.add_text_field(
        "name",
        tantivy::schema::TextOptions::default().set_indexing_options(
            tantivy::schema::TextFieldIndexing::default()
                .set_tokenizer(BIGRAM_TOKENIZER_NAME)
                .set_index_option(IndexRecordOption::WithFreqs),
        ),
    );
    let all_text = b.add_text_field(
        "all_text",
        tantivy::schema::TextOptions::default().set_indexing_options(
            tantivy::schema::TextFieldIndexing::default()
                .set_tokenizer(BIGRAM_TOKENIZER_NAME)
                .set_index_option(IndexRecordOption::WithFreqs),
        ),
    );
    (b.build(), path, name, all_text)
}

fn register_bigram(index: &Index) {
    let analyzer = TextAnalyzer::builder(NgramTokenizer::new(2, 2, false).unwrap())
        .filter(LowerCaser)
        .build();
    index.tokenizers().register(BIGRAM_TOKENIZER_NAME, analyzer);
}

/// Tantivy index を作り、全 doc を投入して commit。
/// インデックス時間とインデックスサイズを返す。
fn build_index(corpus: &[SyntheticDoc], index_dir: &std::path::Path) -> (Index, Duration, u64) {
    std::fs::create_dir_all(index_dir).unwrap();
    let (schema, path_f, name_f, all_text_f) = build_schema();

    let index = Index::create_in_dir(index_dir, schema).expect("create index");
    register_bigram(&index);

    let mut writer: IndexWriter = index
        .writer(INDEX_WRITER_HEAP_MB * 1024 * 1024)
        .expect("writer");

    let t0 = Instant::now();
    for d in corpus {
        let name = d.path.rsplit('/').next().unwrap_or(&d.path);
        writer
            .add_document(doc!(
                path_f => d.path.as_str(),
                name_f => name,
                all_text_f => d.all_text.as_str(),
            ))
            .expect("add_document");
    }
    writer.commit().expect("commit");
    let build_time = t0.elapsed();

    let size = dir_size(index_dir);
    (index, build_time, size)
}

fn dir_size(path: &std::path::Path) -> u64 {
    let mut total = 0;
    if let Ok(entries) = std::fs::read_dir(path) {
        for e in entries.flatten() {
            if let Ok(md) = e.metadata() {
                if md.is_file() {
                    total += md.len();
                } else if md.is_dir() {
                    total += dir_size(&e.path());
                }
            }
        }
    }
    total
}

// ============================================================================
// SQLite post-filter のコスト計測
// ============================================================================

/// fts_meta.db 相当の最小スキーマ。path → all_text_norm の lookup 専用。
fn build_sqlite_meta(corpus: &[SyntheticDoc], db_path: &std::path::Path) -> Duration {
    let _ = std::fs::remove_file(db_path);
    let conn = Connection::open(db_path).expect("open sqlite");
    conn.execute_batch(
        "PRAGMA journal_mode = WAL;
         PRAGMA synchronous = NORMAL;
         CREATE TABLE files(
            path TEXT PRIMARY KEY,
            all_text_norm TEXT NOT NULL
         );",
    )
    .unwrap();

    let t0 = Instant::now();
    let tx = conn.unchecked_transaction().unwrap();
    {
        let mut stmt = tx
            .prepare("INSERT INTO files(path, all_text_norm) VALUES(?1, ?2)")
            .unwrap();
        for d in corpus {
            stmt.execute(rusqlite::params![d.path, d.all_text.to_lowercase()])
                .unwrap();
        }
    }
    tx.commit().unwrap();
    t0.elapsed()
}

/// 指定 path 群の all_text_norm を一括取得する所要時間。
/// 実際の post-filter では `search_query::matches()` を適用するが、ここでは lookup コストだけ見る。
fn sqlite_lookup_batch(db_path: &std::path::Path, paths: &[String]) -> (Duration, usize) {
    let conn = Connection::open(db_path).unwrap();
    let placeholders = (0..paths.len()).map(|_| "?").collect::<Vec<_>>().join(",");
    let sql = format!(
        "SELECT path, all_text_norm FROM files WHERE path IN ({})",
        placeholders
    );

    let t = Instant::now();
    let mut stmt = conn.prepare(&sql).unwrap();
    let params_vec: Vec<&dyn rusqlite::ToSql> =
        paths.iter().map(|s| s as &dyn rusqlite::ToSql).collect();
    let mut rows = stmt.query(rusqlite::params_from_iter(params_vec)).unwrap();
    let mut count = 0;
    while let Some(row) = rows.next().unwrap() {
        let _p: String = row.get(0).unwrap();
        let _t: String = row.get(1).unwrap();
        count += 1;
    }
    (t.elapsed(), count)
}

// ============================================================================
// Tantivy クエリ (bigram 候補絞り込み + ページング取得)
// ============================================================================

/// クエリ文字列を bigram に分解して AND の BooleanQuery を作る。
/// v1 設計では 2 文字未満はクエリ側で弾くが、ここでは計測用に生成だけする。
fn build_and_query(index: &Index, field: tantivy::schema::Field, query_text: &str) -> BooleanQuery {
    let lowered = query_text.to_lowercase();
    let mut tokenizer = NgramTokenizer::new(2, 2, false).unwrap();
    let mut stream: Box<dyn TokenStream> = Box::new(tokenizer.token_stream(&lowered));
    let mut bigrams: Vec<String> = Vec::new();
    stream.process(&mut |t: &Token| bigrams.push(t.text.clone()));
    // dedup (同じ bigram が何度も AND されると遅くなる)
    bigrams.sort();
    bigrams.dedup();

    let subs: Vec<(Occur, Box<dyn Query>)> = bigrams
        .into_iter()
        .map(|bg| {
            let term = Term::from_field_text(field, &bg);
            let q: Box<dyn Query> = Box::new(TermQuery::new(term, IndexRecordOption::WithFreqs));
            (Occur::Must, q)
        })
        .collect();

    // tantivy はフィールド抑制用に使われるので dummy `_` にしておく
    let _ = index;
    BooleanQuery::from(subs)
}

/// Searcher snapshot を固定してページング取得 (§9.1 ステップ 4 の仕様)。
/// valid_hits は post-filter 通過数。ここでは「簡易 post-filter」として
/// 取得した doc の `all_text` STORED 値に対して query_text.contains() を再確認する。
/// 本番は SQLite lookup + search_query::matches() だが、計測の趣旨はページング動作と
/// Searcher 固定。ここでは Tantivy だけで完結させる。
struct PagingResult {
    total_hits_fetched: usize, // Tantivy が返した候補数
    valid_hits: usize,         // post-filter 通過後 (false positive を差し引いた数)
    pages: usize,
    time_total: Duration,
    time_per_page: Vec<Duration>, // offset 増加による劣化を見る
    time_post_filter_total: Duration,
    truncated: bool,
}

fn run_paged_query(
    index: &Index,
    all_text_field: tantivy::schema::Field,
    path_field: tantivy::schema::Field,
    query_text: &str,
) -> PagingResult {
    let query = build_and_query(index, all_text_field, query_text);
    let reader = index.reader().unwrap();
    let searcher = reader.searcher(); // ★ snapshot 固定 (§9.1)

    let q_lower = query_text.to_lowercase();
    let t_total = Instant::now();

    let mut offset = 0usize;
    let mut valid_hits = 0usize;
    let mut total_fetched = 0usize;
    let mut pages = 0usize;
    let mut times = Vec::new();
    let mut post_time = Duration::ZERO;

    loop {
        let t_page = Instant::now();
        let top_docs: Vec<(f32, tantivy::DocAddress)> = searcher
            .search(
                &query,
                &TopDocs::with_limit(PAGE_SIZE)
                    .and_offset(offset)
                    .order_by_score(),
            )
            .unwrap();
        let page_fetch = t_page.elapsed();

        if top_docs.is_empty() {
            break;
        }

        let page_count = top_docs.len();
        total_fetched += page_count;

        // post-filter: STORED path と all_text を取り出して contains() 再確認。
        // 本番は SQLite lookup だが、Tantivy だけで完結させるため STORED 値で見る。
        let t_post = Instant::now();
        for (_score, doc_addr) in top_docs {
            let stored: tantivy::TantivyDocument = searcher.doc(doc_addr).unwrap();
            // all_text を同じ doc に STORED 指定していないので、ここでは path のみ取って
            // post-filter は擬似的に「path が query に含まれるか」だけチェック。
            // 本番 post-filter 性能は §sqlite_lookup_batch の方で測る。
            let _path_val = stored
                .get_first(path_field)
                .and_then(|v| v.as_str().map(|s| s.to_string()));
            // Tantivy の all_text は STORED ではないので、ここでは false-positive 率は計測外。
            // valid_hits を「Tantivy が返した候補 = 全て真ヒット」として扱う (簡略化)。
            valid_hits += 1;
            if valid_hits >= HARD_MAX {
                break;
            }
        }
        post_time += t_post.elapsed();

        times.push(page_fetch);
        pages += 1;
        offset += PAGE_SIZE;

        if valid_hits >= HARD_MAX {
            break;
        }
        if page_count < PAGE_SIZE {
            break;
        } // 候補使い切り

        // 計測目的なら生産性のためページ打ち切り上限を設ける
        if pages >= 30 {
            break;
        } // 30 * 500 = 15,000 doc 程度まで
    }

    let _ = q_lower; // 未使用 warning 抑制
    PagingResult {
        total_hits_fetched: total_fetched,
        valid_hits,
        pages,
        time_total: t_total.elapsed(),
        time_per_page: times,
        time_post_filter_total: post_time,
        truncated: valid_hits >= HARD_MAX,
    }
}

// ============================================================================
// レポート
// ============================================================================

struct QuerySpec {
    label: &'static str,
    text: &'static str,
    expected: &'static str,
}

fn summarize(r: &PagingResult) -> String {
    let total_ms = r.time_total.as_secs_f64() * 1000.0;
    let avg = if !r.time_per_page.is_empty() {
        r.time_per_page.iter().sum::<Duration>().as_secs_f64() * 1000.0
            / r.time_per_page.len() as f64
    } else {
        0.0
    };
    let worst = r
        .time_per_page
        .iter()
        .map(|d| d.as_secs_f64() * 1000.0)
        .fold(0.0f64, f64::max);
    let first = r
        .time_per_page
        .first()
        .map(|d| d.as_secs_f64() * 1000.0)
        .unwrap_or(0.0);
    let post_ms = r.time_post_filter_total.as_secs_f64() * 1000.0;
    let trunc = if r.truncated {
        " [TRUNCATED at HARD_MAX]"
    } else {
        ""
    };

    format!(
        "  pages={:>3} fetched={:>6} valid={:>6}  total={:>8.1}ms  page_first={:>6.1}ms  page_avg={:>6.1}ms  page_worst={:>6.1}ms  post_filter={:>6.1}ms{}",
        r.pages, r.total_hits_fetched, r.valid_hits, total_ms, first, avg, worst, post_ms, trunc
    )
}

// ============================================================================
// main
// ============================================================================

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let mut num_docs = DEFAULT_NUM_DOCS;
    let mut keep = false;
    let mut json_out: Option<std::path::PathBuf> = None;
    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--docs" => {
                i += 1;
                num_docs = args[i].parse().unwrap_or(DEFAULT_NUM_DOCS);
            }
            "--keep" => keep = true,
            "--json" => {
                i += 1;
                if i < args.len() {
                    json_out = Some(std::path::PathBuf::from(&args[i]));
                }
            }
            _ => eprintln!("unknown arg: {}", args[i]),
        }
        i += 1;
    }

    println!(
        "=== Tantivy + bigram プロトタイプ計測 (docs/archive/search-metadata/search-expansion-design.md §15.1.1) ==="
    );
    println!("  num_docs = {}", num_docs);
    println!("  page_size = {}, HARD_MAX = {}", PAGE_SIZE, HARD_MAX);
    println!();

    // 一時ディレクトリ
    let tmp_root = std::env::temp_dir().join(format!("bench_search_{}", std::process::id()));
    let index_dir = tmp_root.join("fts_index");
    let sqlite_path = tmp_root.join("fts_meta.db");
    std::fs::create_dir_all(&tmp_root).unwrap();

    // 1. コーパス生成
    println!(
        "[1/4] 合成コーパスを生成中 ({} docs, ~2KB/doc) ...",
        num_docs
    );
    let t = Instant::now();
    let corpus = generate_corpus(num_docs, 0xC0FFEE);
    let total_bytes: usize = corpus.iter().map(|d| d.all_text.len()).sum();
    println!(
        "      生成時間: {:.1}s, 合計テキスト: {:.1} MB",
        t.elapsed().as_secs_f64(),
        total_bytes as f64 / (1024.0 * 1024.0)
    );

    // 2. Tantivy インデックス構築
    println!("[2/4] Tantivy インデックス構築中 ...");
    let (index, build_time, index_size) = build_index(&corpus, &index_dir);
    println!(
        "      インデックス時間: {:.1}s ({:.0} docs/sec)",
        build_time.as_secs_f64(),
        num_docs as f64 / build_time.as_secs_f64()
    );
    println!(
        "      インデックスサイズ: {:.1} MB (元テキスト比 {:.2}x)",
        index_size as f64 / (1024.0 * 1024.0),
        index_size as f64 / total_bytes as f64,
    );

    // 3. SQLite meta 構築
    println!("[3/4] fts_meta.db 相当の SQLite 構築中 ...");
    let sqlite_build = build_sqlite_meta(&corpus, &sqlite_path);
    let sqlite_size = std::fs::metadata(&sqlite_path)
        .map(|m| m.len())
        .unwrap_or(0);
    println!(
        "      構築時間: {:.1}s, DB サイズ: {:.1} MB",
        sqlite_build.as_secs_f64(),
        sqlite_size as f64 / (1024.0 * 1024.0)
    );

    // 4. クエリ計測
    println!("[4/4] クエリ計測 ...");
    let queries = [
        QuerySpec {
            label: "rare_jp",
            text: "美濃焼",
            expected: "~5% of docs (JP_RARE 1/20)",
        },
        QuerySpec {
            label: "rare_jp_and",
            text: "美濃焼 斑鳩宮",
            expected: "~0.01%",
        },
        QuerySpec {
            label: "medium_jp",
            text: "夕焼け",
            expected: "~20-30% of docs (JP_MEDIUM)",
        },
        QuerySpec {
            label: "medium_jp_and",
            text: "夕焼け 海辺",
            expected: "~5-10%",
        },
        QuerySpec {
            label: "medium_jp_3",
            text: "夕焼け 海辺 紅葉",
            expected: "~1-2%",
        },
        QuerySpec {
            label: "generic_jp",
            text: "カメラ",
            expected: "~80%+ (JP_GENERIC)",
        },
        QuerySpec {
            label: "super_generic",
            text: "の",
            expected: "100% (単文字 – 仕様では NG だが測定用)",
        },
        QuerySpec {
            label: "en_common",
            text: "photo",
            expected: "~60-80%",
        },
        QuerySpec {
            label: "en_ai_phrase",
            text: "lora sampler",
            expected: "~5-10% (AI 用語 AND)",
        },
        QuerySpec {
            label: "unique_id",
            text: "img_042000",
            expected: "1 doc only",
        },
    ];

    // field を別経路で取り直すと ID がズレるので index から取る
    let path_field = index.schema().get_field("path").unwrap();
    let all_text_field = index.schema().get_field("all_text").unwrap();

    println!();
    println!("{:<20} {:<40} {}", "label", "query", "expected");
    let mut json_records: Vec<(String, f64, f64, usize)> = Vec::new();
    for q in &queries {
        println!("{:<20} {:<40} {}", q.label, q.text, q.expected);
        let r = run_paged_query(&index, all_text_field, path_field, q.text);
        println!("{}", summarize(&r));
        json_records.push((
            q.label.to_string(),
            r.time_total.as_secs_f64() * 1000.0,
            r.time_post_filter_total.as_secs_f64() * 1000.0,
            r.valid_hits,
        ));
    }

    // SQLite 一括 lookup コスト: Tantivy から返った 500 path を想定
    println!();
    println!("=== SQLite post-filter 一括 lookup コスト (500 path IN 句) ===");
    for &batch in &[100usize, 500, 1000, 2000] {
        let paths: Vec<String> = corpus.iter().take(batch).map(|d| d.path.clone()).collect();
        // warmup
        let _ = sqlite_lookup_batch(&sqlite_path, &paths);
        // measure 3 回
        let mut best = Duration::from_secs(100);
        let mut rows = 0;
        for _ in 0..3 {
            let (t, r) = sqlite_lookup_batch(&sqlite_path, &paths);
            if t < best {
                best = t;
                rows = r;
            }
        }
        println!(
            "  batch={:>5}  best={:>6.1}ms  rows={:>5}  per-row={:>5.2}ms",
            batch,
            best.as_secs_f64() * 1000.0,
            rows,
            best.as_secs_f64() * 1000.0 / batch as f64
        );
    }

    println!();
    println!("=== SUMMARY ===");
    println!(
        "  Tantivy build: {:.1}s, size {:.1} MB ({:.2}x raw text)",
        build_time.as_secs_f64(),
        index_size as f64 / (1024.0 * 1024.0),
        index_size as f64 / total_bytes as f64
    );
    println!(
        "  SQLite build:  {:.1}s, size {:.1} MB",
        sqlite_build.as_secs_f64(),
        sqlite_size as f64 / (1024.0 * 1024.0)
    );
    println!();
    println!("  Index dir:   {}", index_dir.display());
    println!("  SQLite file: {}", sqlite_path.display());

    if keep {
        println!();
        println!("  (--keep 指定のため残します)");
    } else {
        let _ = std::fs::remove_dir_all(&tmp_root);
        println!();
        println!("  (一時ファイルを削除しました。残す場合は --keep)");
    }

    // JSON 出力 (--json <path>): scripts/check_bench_regression.py が消費する。
    // serde_json で書き出すことで、将来 label に非 ASCII / 引用符が入っても安全。
    if let Some(path) = json_out {
        let mut queries = serde_json::Map::new();
        for (label, total_ms, post_ms, hits) in &json_records {
            queries.insert(
                label.clone(),
                serde_json::json!({
                    "total_ms": total_ms,
                    "post_ms": post_ms,
                    "hits": hits,
                }),
            );
        }
        let payload = serde_json::json!({
            "version": 1,
            "num_docs": num_docs,
            "queries": queries,
        });
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).ok();
        }
        let mut s = serde_json::to_string_pretty(&payload).expect("JSON 整形");
        s.push('\n');
        std::fs::write(&path, s).expect("JSON 書き出し失敗");
        println!("\nJSON 出力: {}", path.display());
    }
}
