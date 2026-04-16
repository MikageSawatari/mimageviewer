//! AI 画像生成メタデータの読み取りとパース。
//!
//! PNG ファイルの tEXt / iTXt チャンクからプロンプト等の生成情報を抽出する。
//! 対応フォーマット:
//! - A1111 / Forge: key=`parameters` (平文テキスト)
//! - ComfyUI: key=`prompt` (JSON) + key=`workflow` (JSON, optional)
//! - Midjourney: key=`Description` (平文テキスト)

use std::io::Read;
use std::path::Path;

/// Negative Prompt を含みうる AI 生成メタデータの tEXt キー。
/// `build_searchable_from_chunks` の Unknown フォールバックで素の値を
/// 取り込まないよう除外するためのリスト。`detect_and_parse` に新しい AI
/// フォーマットの分岐を追加したら、その起点キーをここにも足さないと
/// 「未検出時に Negative prompt を含んだ生文字列ごと検索対象に入ってしまう」
/// 不具合になる。
const AI_METADATA_KEYS: &[&str] = &["parameters", "prompt", "workflow", "Description"];

// ---------------------------------------------------------------------------
// データ構造
// ---------------------------------------------------------------------------

/// A1111 / Forge / Midjourney 形式のメタデータ
#[derive(Clone, Debug)]
pub struct A1111Metadata {
    pub prompt: String,
    pub negative_prompt: String,
    /// (Key, Value) ペア: Steps, Sampler, CFG scale, Seed, Model 等
    pub params: Vec<(String, String)>,
    /// 元テキスト全体（フォールバック表示用）
    pub raw: String,
}

/// ComfyUI 形式のメタデータ
#[derive(Clone, Debug)]
pub struct ComfyUIMetadata {
    pub prompt_json: serde_json::Value,
    pub workflow_json: Option<serde_json::Value>,
    /// CLIPTextEncode ノード等から抽出した正プロンプト
    pub extracted_prompts: Vec<String>,
    /// 同・負プロンプト
    pub extracted_negatives: Vec<String>,
    /// KSampler ノード等から抽出した生成パラメータ
    pub sampler_params: Vec<(String, String)>,
}

/// 検出されたメタデータのフォーマット
#[derive(Clone, Debug)]
pub enum AiMetadata {
    A1111(A1111Metadata),
    ComfyUI(ComfyUIMetadata),
    /// 未知の tEXt チャンク群（表示はできる）
    Unknown(Vec<(String, String)>),
}

// ---------------------------------------------------------------------------
// PNG tEXt / iTXt チャンク読み取り
// ---------------------------------------------------------------------------

/// PNG ファイルから tEXt / iTXt / zTXt チャンクの (key, value) ペアを読み取る。
/// IDAT の前後どちらに配置されたチャンクも読み取る（png crate は IDAT 前のみ）。
/// 画像ピクセルはデコードしない。
pub fn read_png_text_chunks(path: &Path) -> std::io::Result<Vec<(String, String)>> {
    let data = std::fs::read(path)?;
    read_png_text_chunks_raw(&data)
}

/// バイト列から PNG tEXt / iTXt / zTXt チャンクを読み取る（ZIP 内画像用）。
pub fn read_png_text_chunks_from_bytes(bytes: &[u8]) -> std::io::Result<Vec<(String, String)>> {
    read_png_text_chunks_raw(bytes)
}

/// PNG バイナリを直接パースして tEXt / iTXt / zTXt チャンクをすべて収集する。
fn read_png_text_chunks_raw(data: &[u8]) -> std::io::Result<Vec<(String, String)>> {
    // PNG signature (8 bytes)
    if data.len() < 8 || &data[..8] != b"\x89PNG\r\n\x1a\n" {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "Not a PNG file",
        ));
    }

    let mut chunks = Vec::new();
    let mut pos = 8; // skip signature

    while pos + 8 <= data.len() {
        let length = u32::from_be_bytes([data[pos], data[pos + 1], data[pos + 2], data[pos + 3]]) as usize;
        let chunk_type = &data[pos + 4..pos + 8];
        let data_start = pos + 8;
        let data_end = data_start + length;

        if data_end + 4 > data.len() {
            break; // truncated
        }

        match chunk_type {
            b"tEXt" => {
                let chunk_data = &data[data_start..data_end];
                if let Some(null_pos) = chunk_data.iter().position(|&b| b == 0) {
                    let keyword = String::from_utf8_lossy(&chunk_data[..null_pos]).to_string();
                    let text = String::from_utf8_lossy(&chunk_data[null_pos + 1..]).to_string();
                    chunks.push((keyword, text));
                }
            }
            b"zTXt" => {
                let chunk_data = &data[data_start..data_end];
                if let Some(null_pos) = chunk_data.iter().position(|&b| b == 0) {
                    let keyword = String::from_utf8_lossy(&chunk_data[..null_pos]).to_string();
                    // compression method (1 byte) + compressed data
                    if null_pos + 2 < chunk_data.len() {
                        let compressed = &chunk_data[null_pos + 2..];
                        if let Ok(text) = decompress_zlib(compressed) {
                            chunks.push((keyword, text));
                        }
                    }
                }
            }
            b"iTXt" => {
                let chunk_data = &data[data_start..data_end];
                if let Some(kw_end) = chunk_data.iter().position(|&b| b == 0) {
                    let keyword = String::from_utf8_lossy(&chunk_data[..kw_end]).to_string();
                    // compression flag (1) + compression method (1) + language\0 + translated\0 + text
                    let rest = &chunk_data[kw_end + 1..];
                    if rest.len() >= 2 {
                        let compression_flag = rest[0];
                        let _compression_method = rest[1];
                        let after_method = &rest[2..];
                        // skip language tag (until \0)
                        let lang_end = after_method.iter().position(|&b| b == 0).unwrap_or(0);
                        let after_lang = if lang_end + 1 < after_method.len() {
                            &after_method[lang_end + 1..]
                        } else {
                            &[]
                        };
                        // skip translated keyword (until \0)
                        let trans_end = after_lang.iter().position(|&b| b == 0).unwrap_or(0);
                        let text_data = if trans_end + 1 < after_lang.len() {
                            &after_lang[trans_end + 1..]
                        } else {
                            &[]
                        };

                        if compression_flag == 0 {
                            let text = String::from_utf8_lossy(text_data).to_string();
                            chunks.push((keyword, text));
                        } else if let Ok(text) = decompress_zlib(text_data) {
                            chunks.push((keyword, text));
                        }
                    }
                }
            }
            b"IEND" => break,
            _ => {}
        }

        pos = data_end + 4; // skip CRC
    }

    Ok(chunks)
}

/// zlib (deflate) 圧縮データを解凍する。
fn decompress_zlib(data: &[u8]) -> std::io::Result<String> {
    let mut decoder = flate2::read::ZlibDecoder::new(data);
    let mut out = String::new();
    decoder.read_to_string(&mut out)?;
    Ok(out)
}

// ---------------------------------------------------------------------------
// フォーマット判別 & 高レベル API
// ---------------------------------------------------------------------------

/// ファイルパスからメタデータを抽出する（PNG のみ対応）。
pub fn extract_metadata(path: &Path) -> Option<AiMetadata> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    if ext != "png" {
        return None;
    }
    let chunks = read_png_text_chunks(path).ok()?;
    detect_and_parse(&chunks)
}

/// バイト列からメタデータを抽出する（ZIP 内 PNG 用）。
pub fn extract_metadata_from_bytes(bytes: &[u8]) -> Option<AiMetadata> {
    let chunks = read_png_text_chunks_from_bytes(bytes).ok()?;
    detect_and_parse(&chunks)
}

fn detect_and_parse(chunks: &[(String, String)]) -> Option<AiMetadata> {
    if chunks.is_empty() {
        return None;
    }

    // ComfyUI: "prompt" キーの存在で判定
    let prompt_json = chunks.iter().find(|(k, _)| k == "prompt");
    if let Some((_, json_str)) = prompt_json {
        // ComfyUI の prompt は JSON 形式
        if let Ok(val) = serde_json::from_str::<serde_json::Value>(json_str) {
            if val.is_object() {
                let workflow = chunks
                    .iter()
                    .find(|(k, _)| k == "workflow")
                    .and_then(|(_, s)| serde_json::from_str::<serde_json::Value>(s).ok());
                return Some(AiMetadata::ComfyUI(parse_comfyui(val, workflow)));
            }
        }
    }

    // A1111 / Forge: "parameters" キー
    if let Some((_, raw)) = chunks.iter().find(|(k, _)| k == "parameters") {
        if let Some(meta) = parse_a1111(raw) {
            return Some(AiMetadata::A1111(meta));
        }
    }

    // Midjourney: "Description" キー
    if let Some((_, raw)) = chunks.iter().find(|(k, _)| k == "Description") {
        if let Some(meta) = parse_a1111(raw) {
            return Some(AiMetadata::A1111(meta));
        }
        // Description があるが A1111 形式でない場合は Unknown として表示
        return Some(AiMetadata::Unknown(vec![
            ("Description".to_string(), raw.clone()),
        ]));
    }

    // 何らかの tEXt チャンクはあるが既知フォーマットに一致しない
    // → Unknown として返す（ユーザーが内容を確認できるよう）
    let interesting: Vec<(String, String)> = chunks
        .iter()
        .filter(|(k, _)| {
            // PNG 標準チャンク (Software, Creation Time 等) は除外
            !matches!(
                k.as_str(),
                "Software" | "Creation Time" | "Author" | "Comment" | "Source"
            )
        })
        .cloned()
        .collect();
    if interesting.is_empty() {
        None
    } else {
        Some(AiMetadata::Unknown(interesting))
    }
}

// ---------------------------------------------------------------------------
// 検索対象テキスト構築
// ---------------------------------------------------------------------------

/// メタデータから検索対象文字列を構築する。
///
/// **Negative prompt は除外される**。
/// - A1111 / Forge / Midjourney: `prompt` + `params` (Steps, Sampler, Model 等)
/// - ComfyUI: `extracted_prompts` + `sampler_params`
/// - Unknown: 全チャンク値 (正負の区別ができないため全部含める)
///
/// 各値は改行区切りで連結される。`search_query::matches` に渡せば内部で
/// 小文字化されるので呼び出し側での前処理は不要。
pub fn build_searchable_text(meta: &AiMetadata) -> String {
    let mut out = String::new();
    match meta {
        AiMetadata::A1111(m) => {
            append_line(&mut out, &m.prompt);
            for (k, v) in &m.params {
                append_kv(&mut out, k, v);
            }
        }
        AiMetadata::ComfyUI(m) => {
            for p in &m.extracted_prompts {
                append_line(&mut out, p);
            }
            for (k, v) in &m.sampler_params {
                append_kv(&mut out, k, v);
            }
        }
        AiMetadata::Unknown(chunks) => {
            // 未知フォーマットは正負の分離ができないので全部入れる
            for (_, v) in chunks {
                append_line(&mut out, v);
            }
        }
    }
    out
}

fn append_line(out: &mut String, s: &str) {
    if s.is_empty() {
        return;
    }
    if !out.is_empty() {
        out.push('\n');
    }
    out.push_str(s);
}

fn append_kv(out: &mut String, k: &str, v: &str) {
    if !out.is_empty() {
        out.push('\n');
    }
    out.push_str(k);
    out.push_str(": ");
    out.push_str(v);
}

/// PNG の生 tEXt チャンク群から検索対象文字列を直接構築する高レベルヘルパ。
///
/// - AI メタデータ (A1111 / ComfyUI / Midjourney) が認識できた場合は
///   **Negative prompt を除外した** テキストを採用する。
/// - Author / Comment / Software など AI 以外のチャンクは常に含める。
/// - AI メタデータが認識できなかった場合は全チャンクの値を素通しで含める。
pub fn build_searchable_from_chunks(chunks: &[(String, String)]) -> String {
    let meta = detect_and_parse(chunks);
    let mut out = String::new();

    if let Some(ref m) = meta {
        append_line(&mut out, &build_searchable_text(m));
    }

    // A1111 / ComfyUI: AI キーの生値には Negative が残っているので再掲しない。
    //                  非 AI チャンク (Author, Comment 等) だけ追加する。
    // Unknown:         build_searchable_text ですでに全チャンクを含めた。
    // なし:            チャンクを素通しで含める (取りこぼし回避)。
    let include_non_ai_chunks = match meta {
        Some(AiMetadata::A1111(_)) | Some(AiMetadata::ComfyUI(_)) => true,
        Some(AiMetadata::Unknown(_)) => false,
        None => true,
    };

    if include_non_ai_chunks {
        for (k, v) in chunks {
            if AI_METADATA_KEYS.contains(&k.as_str()) {
                continue;
            }
            append_line(&mut out, v);
        }
    }

    out
}

/// PNG ファイルパスから Negative Prompt を除外した検索対象文字列を取得する。
/// 読み取りに失敗した場合 / 有効な tEXt チャンクが無い場合は空文字列を返す。
pub fn build_searchable_from_path(path: &Path) -> String {
    let chunks = read_png_text_chunks(path).unwrap_or_default();
    if chunks.is_empty() {
        return String::new();
    }
    build_searchable_from_chunks(&chunks)
}

/// PNG バイト列から Negative Prompt を除外した検索対象文字列を取得する。
pub fn build_searchable_from_bytes(bytes: &[u8]) -> String {
    let chunks = read_png_text_chunks_from_bytes(bytes).unwrap_or_default();
    if chunks.is_empty() {
        return String::new();
    }
    build_searchable_from_chunks(&chunks)
}

// ---------------------------------------------------------------------------
// A1111 / Forge パーサー
// ---------------------------------------------------------------------------

/// A1111 形式のテキストをパースする。
///
/// フォーマット:
/// ```text
/// <positive prompt>
/// Negative prompt: <negative prompt>
/// Steps: 20, Sampler: Euler, CFG scale: 7, Seed: 12345, ...
/// ```
pub fn parse_a1111(raw: &str) -> Option<A1111Metadata> {
    if raw.trim().is_empty() {
        return None;
    }

    let raw = raw.to_string();

    // "Negative prompt: " で分割
    let (prompt, negative_prompt, params) = if let Some(neg_pos) = raw.find("\nNegative prompt: ") {
        let p = raw[..neg_pos].trim().to_string();
        let after_neg = &raw[neg_pos + "\nNegative prompt: ".len()..];

        if let Some(params_pos) = find_params_line(after_neg) {
            let np = after_neg[..params_pos].trim().to_string();
            let prm = parse_params_line(&after_neg[params_pos..]);
            (p, np, prm)
        } else {
            let np = after_neg.trim().to_string();
            (p, np, Vec::new())
        }
    } else if let Some(params_pos) = find_params_line(&raw) {
        let p = raw[..params_pos].trim().to_string();
        let prm = parse_params_line(&raw[params_pos..]);
        (p, String::new(), prm)
    } else {
        (raw.trim().to_string(), String::new(), Vec::new())
    };

    Some(A1111Metadata {
        prompt,
        negative_prompt,
        params,
        raw,
    })
}

/// テキスト中の「パラメータ行」の開始位置を見つける。
/// パラメータ行は "\nSteps: " で始まる最後の行。
fn find_params_line(text: &str) -> Option<usize> {
    // 複数の "\nSteps: " がある場合は最後のものを使う
    let mut last_pos = None;
    let mut search_from = 0;
    while let Some(pos) = text[search_from..].find("\nSteps: ") {
        last_pos = Some(search_from + pos + 1); // +1 で '\n' の次を指す
        search_from = search_from + pos + 1;
    }
    // テキストの先頭が "Steps: " で始まる場合
    if last_pos.is_none() && text.starts_with("Steps: ") {
        last_pos = Some(0);
    }
    last_pos
}

/// "Steps: 20, Sampler: Euler, CFG scale: 7, ..." 形式の行をパースする。
fn parse_params_line(line: &str) -> Vec<(String, String)> {
    let line = line.trim();
    let mut params = Vec::new();

    // "Key: Value" ペアをカンマで分割
    // ただし値にカンマを含む場合があるため、既知のキー名で分割する
    let known_keys = [
        "Steps",
        "Sampler",
        "Schedule type",
        "CFG scale",
        "Distilled CFG Scale",
        "Seed",
        "Face restoration",
        "Size",
        "Model hash",
        "Model",
        "VAE hash",
        "VAE",
        "Denoising strength",
        "Clip skip",
        "ENSD",
        "Hires upscale",
        "Hires steps",
        "Hires upscaler",
        "Lora hashes",
        "TI hashes",
        "Version",
        "RNG",
        "ADetailer model",
        "ADetailer confidence",
        "ADetailer dilate erode",
        "ADetailer mask blur",
        "ADetailer denoising strength",
        "ADetailer inpaint only masked",
        "ADetailer inpaint padding",
    ];

    // 簡易パース: "Key: " パターンで分割
    let mut remaining = line;
    while !remaining.is_empty() {
        // 現在位置のキーを特定
        let mut found_key = None;
        for &key in &known_keys {
            let prefix = format!("{key}: ");
            if remaining.starts_with(&prefix) {
                found_key = Some((key, prefix.len()));
                break;
            }
        }

        if let Some((key, prefix_len)) = found_key {
            let value_start = prefix_len;
            let rest = &remaining[value_start..];

            // 次のキーの位置を探す
            let mut next_key_pos = rest.len();
            for &nk in &known_keys {
                let pat = format!(", {nk}: ");
                if let Some(pos) = rest.find(&pat) {
                    if pos < next_key_pos {
                        next_key_pos = pos;
                    }
                }
            }

            let value = rest[..next_key_pos].trim().to_string();
            params.push((key.to_string(), value));

            if next_key_pos < rest.len() {
                remaining = &rest[next_key_pos + 2..]; // skip ", "
            } else {
                break;
            }
        } else {
            // 既知のキーに一致しない → スキップして次のカンマを探す
            if let Some(pos) = remaining.find(", ") {
                remaining = &remaining[pos + 2..];
            } else {
                break;
            }
        }
    }

    params
}

// ---------------------------------------------------------------------------
// ComfyUI パーサー
// ---------------------------------------------------------------------------

/// ComfyUI の prompt JSON + workflow JSON からメタデータを抽出する。
fn parse_comfyui(
    prompt_json: serde_json::Value,
    workflow_json: Option<serde_json::Value>,
) -> ComfyUIMetadata {
    let mut extracted_prompts = Vec::new();
    let mut extracted_negatives = Vec::new();
    let mut sampler_params = Vec::new();

    // prompt JSON はノード ID → ノード定義のマップ
    if let Some(nodes) = prompt_json.as_object() {
        // まず KSampler ノードを見つけて positive/negative の入力元を特定
        let mut positive_refs: Vec<String> = Vec::new();
        let mut negative_refs: Vec<String> = Vec::new();

        for (_node_id, node) in nodes {
            let class = node.get("class_type").and_then(|c| c.as_str()).unwrap_or("");

            match class {
                "KSampler" | "KSamplerAdvanced" | "SamplerCustom" => {
                    // 生成パラメータを抽出
                    if let Some(inputs) = node.get("inputs").and_then(|i| i.as_object()) {
                        for &key in &["steps", "cfg", "sampler_name", "scheduler", "seed", "denoise"] {
                            if let Some(val) = inputs.get(key) {
                                let val_str = match val {
                                    serde_json::Value::Number(n) => n.to_string(),
                                    serde_json::Value::String(s) => s.clone(),
                                    _ => continue,
                                };
                                sampler_params.push((key.to_string(), val_str));
                            }
                        }

                        // positive/negative の参照先ノードIDを記録
                        if let Some(pos) = inputs.get("positive") {
                            if let Some(arr) = pos.as_array() {
                                if let Some(ref_id) = arr.first().and_then(|v| v.as_str()) {
                                    positive_refs.push(ref_id.to_string());
                                }
                            }
                        }
                        if let Some(neg) = inputs.get("negative") {
                            if let Some(arr) = neg.as_array() {
                                if let Some(ref_id) = arr.first().and_then(|v| v.as_str()) {
                                    negative_refs.push(ref_id.to_string());
                                }
                            }
                        }
                    }
                }
                "CheckpointLoaderSimple" | "CheckpointLoader" => {
                    if let Some(inputs) = node.get("inputs").and_then(|i| i.as_object()) {
                        if let Some(name) = inputs.get("ckpt_name").and_then(|v| v.as_str()) {
                            sampler_params.push(("model".to_string(), name.to_string()));
                        }
                    }
                }
                _ => {}
            }
        }

        // positive/negative 参照先からプロンプトテキストを抽出
        // 参照先が CLIPTextEncode ならテキストを取得
        for ref_id in &positive_refs {
            if let Some(node) = nodes.get(ref_id.as_str()) {
                extract_text_from_node(node, nodes, &mut extracted_prompts);
            }
        }
        for ref_id in &negative_refs {
            if let Some(node) = nodes.get(ref_id.as_str()) {
                extract_text_from_node(node, nodes, &mut extracted_negatives);
            }
        }

        // 参照関係が解決できなかった場合、全 CLIPTextEncode からテキストを集める
        if extracted_prompts.is_empty() && extracted_negatives.is_empty() {
            for (_node_id, node) in nodes {
                let class = node.get("class_type").and_then(|c| c.as_str()).unwrap_or("");
                if class.contains("CLIPTextEncode") {
                    if let Some(text) = node
                        .get("inputs")
                        .and_then(|i| i.get("text"))
                        .and_then(|t| t.as_str())
                    {
                        if !text.trim().is_empty() {
                            extracted_prompts.push(text.to_string());
                        }
                    }
                }
            }
        }
    }

    ComfyUIMetadata {
        prompt_json,
        workflow_json,
        extracted_prompts,
        extracted_negatives,
        sampler_params,
    }
}

/// ノードからテキストを抽出する。CLIPTextEncode ならテキストを直接取得。
/// それ以外なら入力の参照先を再帰的にたどる。
fn extract_text_from_node(
    node: &serde_json::Value,
    all_nodes: &serde_json::Map<String, serde_json::Value>,
    out: &mut Vec<String>,
) {
    let class = node.get("class_type").and_then(|c| c.as_str()).unwrap_or("");

    if class.contains("CLIPTextEncode") {
        if let Some(inputs) = node.get("inputs").and_then(|i| i.as_object()) {
            // text が文字列ならそのまま取得
            if let Some(text) = inputs.get("text").and_then(|t| t.as_str()) {
                if !text.trim().is_empty() {
                    out.push(text.to_string());
                }
            }
            // text が参照 [node_id, output_idx] の場合もある
            if let Some(arr) = inputs.get("text").and_then(|t| t.as_array()) {
                if let Some(ref_id) = arr.first().and_then(|v| v.as_str()) {
                    if let Some(ref_node) = all_nodes.get(ref_id) {
                        extract_text_from_ref_node(ref_node, all_nodes, out);
                    }
                }
            }
        }
    } else {
        // 条件分岐ノード等の場合、入力を追跡
        if let Some(inputs) = node.get("inputs").and_then(|i| i.as_object()) {
            for (_key, val) in inputs {
                if let Some(arr) = val.as_array() {
                    if arr.len() == 2 {
                        if let Some(ref_id) = arr.first().and_then(|v| v.as_str()) {
                            if let Some(ref_node) = all_nodes.get(ref_id) {
                                let ref_class = ref_node
                                    .get("class_type")
                                    .and_then(|c| c.as_str())
                                    .unwrap_or("");
                                if ref_class.contains("CLIPTextEncode")
                                    || ref_class.contains("Conditioning")
                                {
                                    extract_text_from_node(ref_node, all_nodes, out);
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

/// 参照先ノードからテキスト値を抽出（STRING 出力ノード等）。
fn extract_text_from_ref_node(
    node: &serde_json::Value,
    _all_nodes: &serde_json::Map<String, serde_json::Value>,
    out: &mut Vec<String>,
) {
    if let Some(inputs) = node.get("inputs").and_then(|i| i.as_object()) {
        // テキスト系ノード: "text", "string", "value" 等のキーを探す
        for &key in &["text", "string", "value", "text_positive", "text_negative"] {
            if let Some(text) = inputs.get(key).and_then(|t| t.as_str()) {
                if !text.trim().is_empty() {
                    out.push(text.to_string());
                    return;
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// テスト
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_a1111_basic() {
        let raw = "beautiful landscape, high quality\n\
                    Negative prompt: ugly, blurry\n\
                    Steps: 20, Sampler: Euler, CFG scale: 7, Seed: 12345, Size: 512x512, Model: sd_xl_base";
        let meta = parse_a1111(raw).unwrap();
        assert_eq!(meta.prompt, "beautiful landscape, high quality");
        assert_eq!(meta.negative_prompt, "ugly, blurry");
        assert!(meta.params.iter().any(|(k, v)| k == "Steps" && v == "20"));
        assert!(meta.params.iter().any(|(k, v)| k == "Seed" && v == "12345"));
        assert!(meta.params.iter().any(|(k, v)| k == "Model" && v == "sd_xl_base"));
    }

    #[test]
    fn test_parse_a1111_no_negative() {
        let raw = "a cat sitting\nSteps: 30, Sampler: DPM++ 2M, CFG scale: 5, Seed: 999";
        let meta = parse_a1111(raw).unwrap();
        assert_eq!(meta.prompt, "a cat sitting");
        assert!(meta.negative_prompt.is_empty());
        assert!(meta.params.iter().any(|(k, v)| k == "Steps" && v == "30"));
    }

    #[test]
    fn test_parse_a1111_empty() {
        assert!(parse_a1111("").is_none());
        assert!(parse_a1111("   ").is_none());
    }

    #[test]
    fn test_parse_comfyui_basic() {
        let json_str = r#"{
            "1": {
                "class_type": "CheckpointLoaderSimple",
                "inputs": {"ckpt_name": "sd_xl_base_1.0.safetensors"}
            },
            "2": {
                "class_type": "CLIPTextEncode",
                "inputs": {"text": "a beautiful sunset", "clip": ["1", 1]}
            },
            "3": {
                "class_type": "CLIPTextEncode",
                "inputs": {"text": "ugly", "clip": ["1", 1]}
            },
            "4": {
                "class_type": "KSampler",
                "inputs": {
                    "steps": 20,
                    "cfg": 7.0,
                    "sampler_name": "euler",
                    "seed": 42,
                    "positive": ["2", 0],
                    "negative": ["3", 0],
                    "model": ["1", 0],
                    "latent_image": ["5", 0]
                }
            }
        }"#;
        let val: serde_json::Value = serde_json::from_str(json_str).unwrap();
        let meta = parse_comfyui(val, None);
        assert!(meta.extracted_prompts.contains(&"a beautiful sunset".to_string()));
        assert!(meta.extracted_negatives.contains(&"ugly".to_string()));
        assert!(meta.sampler_params.iter().any(|(k, v)| k == "steps" && v == "20"));
        assert!(meta.sampler_params.iter().any(|(k, v)| k == "model" && v == "sd_xl_base_1.0.safetensors"));
    }

    #[test]
    fn test_detect_comfyui() {
        let chunks = vec![
            ("prompt".to_string(), r#"{"1": {"class_type": "KSampler", "inputs": {"steps": 10}}}"#.to_string()),
        ];
        let result = detect_and_parse(&chunks);
        assert!(matches!(result, Some(AiMetadata::ComfyUI(_))));
    }

    #[test]
    fn test_detect_a1111() {
        let chunks = vec![
            ("parameters".to_string(), "hello world\nSteps: 20, Sampler: Euler".to_string()),
        ];
        let result = detect_and_parse(&chunks);
        assert!(matches!(result, Some(AiMetadata::A1111(_))));
    }

    #[test]
    fn test_build_searchable_excludes_negative_a1111() {
        let raw = "beautiful landscape, high quality\n\
                    Negative prompt: ugly, blurry, low quality\n\
                    Steps: 20, Sampler: Euler, CFG scale: 7, Seed: 12345, Model: sd_xl_base";
        let meta = parse_a1111(raw).unwrap();
        let text = build_searchable_text(&AiMetadata::A1111(meta));
        let lower = text.to_lowercase();
        assert!(lower.contains("beautiful landscape"));
        assert!(lower.contains("high quality"));
        assert!(lower.contains("sd_xl_base"));
        // Negative prompt 由来のトークンは含まれない
        assert!(!lower.contains("ugly"));
        assert!(!lower.contains("blurry"));
        assert!(!lower.contains("low quality"));
    }

    #[test]
    fn test_build_searchable_excludes_negative_comfyui() {
        let json_str = r#"{
            "1": {
                "class_type": "CheckpointLoaderSimple",
                "inputs": {"ckpt_name": "sd_xl_base_1.0.safetensors"}
            },
            "2": {
                "class_type": "CLIPTextEncode",
                "inputs": {"text": "a beautiful sunset", "clip": ["1", 1]}
            },
            "3": {
                "class_type": "CLIPTextEncode",
                "inputs": {"text": "ugly blurry", "clip": ["1", 1]}
            },
            "4": {
                "class_type": "KSampler",
                "inputs": {
                    "steps": 20,
                    "cfg": 7.0,
                    "sampler_name": "euler",
                    "seed": 42,
                    "positive": ["2", 0],
                    "negative": ["3", 0],
                    "model": ["1", 0],
                    "latent_image": ["5", 0]
                }
            }
        }"#;
        let val: serde_json::Value = serde_json::from_str(json_str).unwrap();
        let meta = parse_comfyui(val, None);
        let text = build_searchable_text(&AiMetadata::ComfyUI(meta));
        let lower = text.to_lowercase();
        assert!(lower.contains("beautiful sunset"));
        // Negative 由来は含まれない
        assert!(!lower.contains("ugly"));
        assert!(!lower.contains("blurry"));
    }

    #[test]
    fn test_build_searchable_unknown_passthrough() {
        let meta = AiMetadata::Unknown(vec![
            ("foo".to_string(), "bar baz".to_string()),
            ("qux".to_string(), "quux".to_string()),
        ]);
        let text = build_searchable_text(&meta);
        assert!(text.contains("bar baz"));
        assert!(text.contains("quux"));
    }

    #[test]
    fn test_build_from_chunks_keeps_author_excludes_negative() {
        // A1111 `parameters` + 非 AI チャンク (Author, Comment) の混在
        let chunks = vec![
            (
                "parameters".to_string(),
                "masterpiece scene\n\
                 Negative prompt: bad anatomy, worst quality\n\
                 Steps: 30, Sampler: DPM++ 2M, Model: my_model"
                    .to_string(),
            ),
            ("Author".to_string(), "alice".to_string()),
            ("Comment".to_string(), "my favorite".to_string()),
        ];
        let text = build_searchable_from_chunks(&chunks);
        let lower = text.to_lowercase();
        // 正プロンプト + params は検索可能
        assert!(lower.contains("masterpiece scene"));
        assert!(lower.contains("my_model"));
        // Negative prompt は除外
        assert!(!lower.contains("bad anatomy"));
        assert!(!lower.contains("worst quality"));
        // 非 AI チャンクは残る
        assert!(lower.contains("alice"));
        assert!(lower.contains("my favorite"));
    }

    #[test]
    fn test_build_from_chunks_no_ai_passthrough() {
        // AI メタデータなしの場合は全チャンク素通し
        let chunks = vec![
            ("Author".to_string(), "bob".to_string()),
            ("Comment".to_string(), "hello".to_string()),
        ];
        let text = build_searchable_from_chunks(&chunks);
        assert!(text.contains("bob"));
        assert!(text.contains("hello"));
    }
}
