//! AI 画像生成メタデータの読み取りとパース。
//!
//! PNG ファイルの tEXt / iTXt チャンクからプロンプト等の生成情報を抽出する。
//! 対応フォーマット:
//! - A1111 / Forge: key=`parameters` (平文テキスト)
//! - ComfyUI: key=`prompt` (JSON) + key=`workflow` (JSON, optional)
//! - NovelAI: key=`Description` + key=`Comment` (JSON)
//! - InvokeAI / SwarmUI / Fooocus 系: JSON 形式の PNG tEXt / EXIF UserComment
//! - Midjourney: key=`Description` (平文テキスト)

use std::io::Read;
use std::path::Path;

// ---------------------------------------------------------------------------
// データ構造
// ---------------------------------------------------------------------------

/// 正/負プロンプトと生成パラメータに正規化できる AI 生成メタデータの出自。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AiToolKind {
    A1111,
    NovelAI,
    InvokeAI,
    SwarmUI,
    Fooocus,
    EasyDiffusion,
    Midjourney,
    /// EXIF UserComment に保存された A1111 互換テキスト。
    JpegExif,
}

/// メタデータの格納元。検索時に EXIF UserComment の二重取り込みを避けるために使う。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MetadataOrigin {
    PngText,
    /// 認識した AI メタデータが EXIF UserComment 由来。Negative を除外した検索文字列を
    /// 別途 index するので、生 UserComment タグは EXIF 索引から外す。
    ExifUserComment,
    /// EXIF UserComment が未知の JSON object だった。Positive/Negative を分離できず
    /// AI prompt として index しないが、生 JSON (negative 含む) を EXIF 索引へ流すと
    /// negative がリークするため、UserComment タグを索引から外す
    /// (PNG 側の `suppress_unknown_raw` と同じ方針)。
    ExifUserCommentSuppressed,
}

impl MetadataOrigin {
    /// この origin のとき、生 EXIF UserComment タグを検索索引から除外すべきか。
    /// AI 認識済み (Negative 除外テキストを別途 index) と未知 JSON (negative リーク防止)
    /// の両方で true。
    pub fn suppresses_exif_user_comment(self) -> bool {
        matches!(
            self,
            Self::ExifUserComment | Self::ExifUserCommentSuppressed
        )
    }
}

/// A1111 / Forge / Midjourney 形式、および他ツールを正規化したメタデータ
#[derive(Clone, Debug)]
pub struct A1111Metadata {
    pub tool: AiToolKind,
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

#[derive(Clone, Debug)]
struct ParseOutcome {
    meta: AiMetadata,
    /// このフォーマットが解釈に使ったチャンクキー。検索テキスト構築時に、
    /// Negative を含みうる生値を非 AI チャンクとして再混入させないために使う。
    consumed_keys: Vec<&'static str>,
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
        let length =
            u32::from_be_bytes([data[pos], data[pos + 1], data[pos + 2], data[pos + 3]]) as usize;
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
///
/// 細工された zTXt/iTXt チャンクが無制限に膨らむ zlib bomb への防御として、解凍出力に
/// 上限を設ける (untrusted な PNG を読むため。archive_converter の `copy_capped` と同じ
/// 「超過したら拒否」方針)。AI メタデータ (prompt / workflow JSON) の正規サイズは数 KB〜
/// 数 MB なので、16 MiB 上限は十分広く、爆弾だけを弾く。
fn decompress_zlib(data: &[u8]) -> std::io::Result<String> {
    const MAX_DECOMPRESSED: u64 = 16 * 1024 * 1024; // 16 MiB
    let decoder = flate2::read::ZlibDecoder::new(data);
    let mut buf = Vec::new();
    // +1 バイト多く読み、上限超なら zlib bomb として拒否する。
    decoder.take(MAX_DECOMPRESSED + 1).read_to_end(&mut buf)?;
    if buf.len() as u64 > MAX_DECOMPRESSED {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "zlib decompressed output exceeds cap (possible zlib bomb)",
        ));
    }
    String::from_utf8(buf).map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))
}

// ---------------------------------------------------------------------------
// フォーマット判別 & 高レベル API
// ---------------------------------------------------------------------------

/// ファイルパスからメタデータを抽出する。
///
/// PNG は tEXt/iTXt/zTXt、JPEG/JFIF は EXIF UserComment を見る。
pub fn extract_metadata(path: &Path) -> Option<AiMetadata> {
    extract_metadata_with_origin(path).map(|(meta, _)| meta)
}

pub fn extract_metadata_with_origin(path: &Path) -> Option<(AiMetadata, MetadataOrigin)> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    match ext.as_str() {
        "png" => {
            let chunks = read_png_text_chunks(path).ok()?;
            detect_and_parse_outcome(&chunks).map(|outcome| (outcome.meta, MetadataOrigin::PngText))
        }
        "jpg" | "jpeg" | "jfif" => extract_exif_user_comment_metadata_from_path(path)
            .map(|meta| (meta, MetadataOrigin::ExifUserComment)),
        _ => None,
    }
}

/// バイト列からメタデータを抽出する（ZIP 内画像用）。
pub fn extract_metadata_from_bytes(bytes: &[u8]) -> Option<AiMetadata> {
    extract_metadata_from_bytes_with_origin(bytes).map(|(meta, _)| meta)
}

pub fn extract_metadata_from_bytes_with_origin(
    bytes: &[u8],
) -> Option<(AiMetadata, MetadataOrigin)> {
    if is_png_bytes(bytes) {
        let chunks = read_png_text_chunks_from_bytes(bytes).ok()?;
        return detect_and_parse_outcome(&chunks)
            .map(|outcome| (outcome.meta, MetadataOrigin::PngText));
    }
    extract_exif_user_comment_metadata_from_bytes(bytes)
        .map(|meta| (meta, MetadataOrigin::ExifUserComment))
}

fn is_png_bytes(bytes: &[u8]) -> bool {
    bytes.len() >= 8 && &bytes[..8] == b"\x89PNG\r\n\x1a\n"
}

#[cfg(test)]
fn detect_and_parse(chunks: &[(String, String)]) -> Option<AiMetadata> {
    detect_and_parse_outcome(chunks).map(|outcome| outcome.meta)
}

fn detect_and_parse_outcome(chunks: &[(String, String)]) -> Option<ParseOutcome> {
    if chunks.is_empty() {
        return None;
    }

    // ComfyUI: "prompt" キーの JSON object
    if let Some(json_str) = chunk_value(chunks, "prompt") {
        // ComfyUI の prompt は JSON 形式
        if let Ok(val) = serde_json::from_str::<serde_json::Value>(json_str) {
            if val.is_object() {
                let workflow = chunk_value(chunks, "workflow")
                    .and_then(|s| serde_json::from_str::<serde_json::Value>(s).ok());
                return Some(ParseOutcome {
                    meta: AiMetadata::ComfyUI(parse_comfyui(val, workflow)),
                    consumed_keys: vec!["prompt", "workflow"],
                });
            }
        }
    }

    // JSON を持つ parameters は A1111 平文として読まない。まず JSON 系へ分岐する。
    if let Some(raw) = chunk_value(chunks, "parameters") {
        if let Some(json) = parse_json_object(raw) {
            let mut consumed = vec!["parameters"];
            if chunk_value(chunks, "fooocus_scheme").is_some() {
                consumed.push("fooocus_scheme");
            }
            if let Some(meta) = parse_parameters_json(raw, &json, chunks) {
                return Some(ParseOutcome {
                    meta: AiMetadata::A1111(meta),
                    consumed_keys: consumed,
                });
            }
            return Some(ParseOutcome {
                meta: AiMetadata::Unknown(vec![("parameters".to_string(), raw.to_string())]),
                consumed_keys: consumed,
            });
        }
    }

    // InvokeAI: 現行 JSON / 旧 JSON / 旧 Dream テキスト
    if let Some(raw) = chunk_value(chunks, "invokeai_metadata")
        && let Some(json) = parse_json_object(raw)
        && let Some(meta) = parse_invokeai_json(raw, &json)
    {
        return Some(ParseOutcome {
            meta: AiMetadata::A1111(meta),
            consumed_keys: vec!["invokeai_metadata", "invokeai_workflow", "invokeai_graph"],
        });
    }
    if let Some(raw) = chunk_value(chunks, "sd-metadata")
        && let Some(json) = parse_json_object(raw)
        && let Some(meta) = parse_invokeai_legacy_json(raw, &json)
    {
        return Some(ParseOutcome {
            meta: AiMetadata::A1111(meta),
            consumed_keys: vec!["sd-metadata", "Dream"],
        });
    }
    if let Some(raw) = chunk_value(chunks, "Dream")
        && let Some(meta) = parse_invokeai_dream(raw)
    {
        return Some(ParseOutcome {
            meta: AiMetadata::A1111(meta),
            consumed_keys: vec!["Dream", "sd-metadata"],
        });
    }

    // NovelAI は Description と Comment を併用するため、Description 汎用分岐より前に置く。
    if let Some(meta) = parse_novelai(chunks) {
        return Some(ParseOutcome {
            meta: AiMetadata::A1111(meta),
            consumed_keys: vec!["Description", "Comment", "Software", "Source", "Title"],
        });
    }

    // Fooocus 系の Comment JSON: 新しめの Fooocus / Fooocus-MRE は parameters では
    // なく Comment チャンクに JSON を書く (receyuki / DiffusionToolkit と同じ判別)。
    // NovelAI も Comment を使うため、NovelAI 判定より後に置くこと。
    if let Some(raw) = chunk_value(chunks, "Comment")
        && let Some(json) = parse_json_object(raw)
        && let Some(meta) =
            parse_ruinedfooocus_json(raw, &json).or_else(|| parse_fooocus_json(raw, &json, chunks))
    {
        return Some(ParseOutcome {
            meta: AiMetadata::A1111(meta),
            consumed_keys: vec!["Comment"],
        });
    }

    // EasyDiffusion: フィールドごとに独立した tEXt チャンクで保存される
    // (sdkit の save_dicts(output_format="embed"))。リーダー実装
    // (receyuki / DiffusionToolkit) と同じく negative_prompt チャンクの存在で判別。
    if let Some(meta) = parse_easydiffusion_chunks(chunks) {
        return Some(ParseOutcome {
            meta: AiMetadata::A1111(meta),
            consumed_keys: EASYDIFFUSION_KEYS.to_vec(),
        });
    }

    // A1111 / Forge: "parameters" キー (平文)
    if let Some(raw) = chunk_value(chunks, "parameters") {
        if let Some(meta) = parse_a1111_as(raw, AiToolKind::A1111) {
            return Some(ParseOutcome {
                meta: AiMetadata::A1111(meta),
                consumed_keys: vec!["parameters"],
            });
        }
    }

    // Midjourney: "Description" キー
    if let Some(raw) = chunk_value(chunks, "Description") {
        if let Some(meta) = parse_a1111_as(raw, AiToolKind::Midjourney) {
            return Some(ParseOutcome {
                meta: AiMetadata::A1111(meta),
                consumed_keys: vec!["Description"],
            });
        }
        // Description があるが A1111 形式でない場合は Unknown として表示
        return Some(ParseOutcome {
            meta: AiMetadata::Unknown(vec![("Description".to_string(), raw.to_string())]),
            consumed_keys: Vec::new(),
        });
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
        Some(ParseOutcome {
            meta: AiMetadata::Unknown(interesting),
            consumed_keys: Vec::new(),
        })
    }
}

fn chunk_value<'a>(chunks: &'a [(String, String)], key: &str) -> Option<&'a str> {
    chunks
        .iter()
        .find(|(k, _)| k == key)
        .map(|(_, v)| v.as_str())
}

fn parse_json_object(raw: &str) -> Option<serde_json::Value> {
    let val = serde_json::from_str::<serde_json::Value>(raw).ok()?;
    val.is_object().then_some(val)
}

fn parse_parameters_json(
    raw: &str,
    json: &serde_json::Value,
    chunks: &[(String, String)],
) -> Option<A1111Metadata> {
    if let Some(meta) = parse_swarmui_json(raw, json) {
        return Some(meta);
    }
    if let Some(meta) = parse_ruinedfooocus_json(raw, json) {
        return Some(meta);
    }
    if let Some(meta) = parse_fooocus_json(raw, json, chunks) {
        return Some(meta);
    }
    None
}

fn parse_swarmui_json(raw: &str, json: &serde_json::Value) -> Option<A1111Metadata> {
    let params_obj = json.get("sui_image_params")?.as_object()?;
    let prompt = json_value_to_text(params_obj.get("prompt")?).unwrap_or_default();
    let negative_prompt = params_obj
        .get("negativeprompt")
        .and_then(json_value_to_text)
        .unwrap_or_default();
    if prompt.trim().is_empty() && negative_prompt.trim().is_empty() {
        return None;
    }

    let mut params = scalar_params_from_object(
        params_obj,
        &[
            "prompt",
            "negativeprompt",
            "originalprompt",
            "originalnegativeprompt",
        ],
    );
    if let Some(extra) = json.get("sui_extra_data").and_then(|v| v.as_object()) {
        params.extend(prefix_scalar_params("extra.", extra, &[]));
    }
    if let Some(models) = json.get("sui_models").and_then(|v| v.as_array()) {
        for model in models {
            if let Some(obj) = model.as_object() {
                let name = obj.get("name").and_then(json_value_to_text);
                let hash = obj.get("hash").and_then(json_value_to_text);
                match (name, hash) {
                    (Some(name), Some(hash)) if !hash.is_empty() => {
                        params.push(("model".to_string(), format!("{name} ({hash})")));
                    }
                    (Some(name), _) => params.push(("model".to_string(), name)),
                    _ => {}
                }
            }
        }
    }

    Some(A1111Metadata {
        tool: AiToolKind::SwarmUI,
        prompt,
        negative_prompt,
        params,
        raw: raw.to_string(),
    })
}

fn parse_fooocus_json(
    raw: &str,
    json: &serde_json::Value,
    chunks: &[(String, String)],
) -> Option<A1111Metadata> {
    let obj = json.as_object()?;
    let has_scheme = chunk_value(chunks, "fooocus_scheme").is_some();
    let has_fingerprint = [
        "base_model",
        "guidance_scale",
        "full_prompt",
        "full_negative_prompt",
        "performance",
        "styles",
        "refiner_model",
        // prompt + negative_prompt の組は生成メタデータとして十分特異。
        // 未知亜種でも negative を検索へ漏らさないために拾う。
        "negative_prompt",
    ]
    .iter()
    .any(|key| obj.contains_key(*key));
    if !has_scheme && !(obj.contains_key("prompt") && has_fingerprint) {
        return None;
    }

    let prompt = first_json_text(
        json,
        &[
            &["prompt"],
            &["full_prompt"],
            &["raw_prompt"],
            // Fooocus-MRE: prompt 展開後の配列
            &["real_prompt"],
            &["Prompt"],
            &["Full prompt"],
        ],
    )
    .unwrap_or_default();
    let negative_prompt = first_json_text(
        json,
        &[
            &["negative_prompt"],
            &["full_negative_prompt"],
            &["raw_negative_prompt"],
            &["real_negative_prompt"],
            &["Negative"],
            &["Negative Prompt"],
        ],
    )
    .unwrap_or_default();
    if prompt.trim().is_empty() && negative_prompt.trim().is_empty() {
        return None;
    }

    let params = scalar_params_from_object(
        obj,
        &[
            "prompt",
            "full_prompt",
            "raw_prompt",
            "real_prompt",
            "Prompt",
            "Full prompt",
            "negative_prompt",
            "full_negative_prompt",
            "raw_negative_prompt",
            "real_negative_prompt",
            "Negative",
            "Negative Prompt",
        ],
    );
    Some(A1111Metadata {
        tool: AiToolKind::Fooocus,
        prompt,
        negative_prompt,
        params,
        raw: raw.to_string(),
    })
}

fn parse_ruinedfooocus_json(raw: &str, json: &serde_json::Value) -> Option<A1111Metadata> {
    let obj = json.as_object()?;
    let software = obj
        .get("software")
        .or_else(|| obj.get("Software"))
        .and_then(json_value_to_text)
        .unwrap_or_default();
    if !software.eq_ignore_ascii_case("RuinedFooocus") {
        return None;
    }
    let prompt = first_json_text(json, &[&["Prompt"], &["prompt"]]).unwrap_or_default();
    let negative_prompt =
        first_json_text(json, &[&["Negative"], &["negative"], &["negative_prompt"]])
            .unwrap_or_default();
    let params = scalar_params_from_object(
        obj,
        &[
            "Prompt",
            "prompt",
            "Negative",
            "negative",
            "negative_prompt",
        ],
    );
    Some(A1111Metadata {
        tool: AiToolKind::Fooocus,
        prompt,
        negative_prompt,
        params,
        raw: raw.to_string(),
    })
}

/// EasyDiffusion が PNG に書く tEXt キー (sdkit `save_dicts(output_format="embed")` が
/// メタデータ dict をフィールドごとに独立チャンクとして保存する)。キー名は
/// バージョン / 設定により内部名 (negative_prompt 等) と表示名 (Negative Prompt 等)
/// の 2 系統がある (easydiffusion `save_utils.py` の TASK_TEXT_MAPPING)。
const EASYDIFFUSION_KEYS: &[&str] = &[
    "prompt",
    "Prompt",
    "negative_prompt",
    "Negative Prompt",
    "seed",
    "Seed",
    "use_stable_diffusion_model",
    "Stable Diffusion model",
    "clip_skip",
    "Clip Skip",
    "use_controlnet_model",
    "ControlNet model",
    "control_filter_to_apply",
    "ControlNet Filter",
    "control_alpha",
    "ControlNet Strength",
    "use_vae_model",
    "VAE model",
    "sampler_name",
    "Sampler",
    "width",
    "Width",
    "height",
    "Height",
    "num_inference_steps",
    "Steps",
    "guidance_scale",
    "Guidance Scale",
    "use_lora_model",
    "LoRA model",
    "lora_alpha",
    "LoRA Strength",
    "use_hypernetwork_model",
    "Hypernetwork model",
    "hypernetwork_strength",
    "Hypernetwork Strength",
    "use_embeddings_model",
    "Embedding models",
    "tiling",
    "Seamless Tiling",
    "use_face_correction",
    "Use Face Correction",
    "use_upscale",
    "Use Upscaling",
    "upscale_amount",
    "Upscale By",
    "latent_upscaler_steps",
    "Latent Upscaler Steps",
];

fn parse_easydiffusion_chunks(chunks: &[(String, String)]) -> Option<A1111Metadata> {
    // negative_prompt / Negative Prompt の独立チャンクは EasyDiffusion 固有
    // (他ツールは parameters / Comment 等の単一チャンクにまとめる)。
    let negative_prompt = chunk_value(chunks, "negative_prompt")
        .or_else(|| chunk_value(chunks, "Negative Prompt"))?
        .to_string();
    let prompt = chunk_value(chunks, "prompt")
        .or_else(|| chunk_value(chunks, "Prompt"))
        .unwrap_or("")
        .to_string();
    if prompt.trim().is_empty() && negative_prompt.trim().is_empty() {
        return None;
    }

    let mut params = Vec::new();
    let mut raw = String::new();
    for (k, v) in chunks {
        if !EASYDIFFUSION_KEYS.contains(&k.as_str()) {
            continue;
        }
        if !raw.is_empty() {
            raw.push('\n');
        }
        raw.push_str(k);
        raw.push_str(": ");
        raw.push_str(v);
        if matches!(
            k.as_str(),
            "prompt" | "Prompt" | "negative_prompt" | "Negative Prompt"
        ) {
            continue;
        }
        if !v.trim().is_empty() {
            params.push((k.clone(), v.clone()));
        }
    }

    Some(A1111Metadata {
        tool: AiToolKind::EasyDiffusion,
        prompt,
        negative_prompt,
        params,
        raw,
    })
}

fn parse_invokeai_json(raw: &str, json: &serde_json::Value) -> Option<A1111Metadata> {
    let prompt = first_json_text(
        json,
        &[
            &["positive_prompt"],
            &["positive"],
            &["prompt"],
            &["metadata", "positive_prompt"],
        ],
    )
    .unwrap_or_default();
    let style_prompt = first_json_text(json, &[&["positive_style_prompt"]]).unwrap_or_default();
    let prompt = join_non_empty(&[prompt.as_str(), style_prompt.as_str()], "\n");

    let negative_prompt = first_json_text(
        json,
        &[
            &["negative_prompt"],
            &["negative"],
            &["metadata", "negative_prompt"],
        ],
    )
    .unwrap_or_default();
    let negative_style = first_json_text(json, &[&["negative_style_prompt"]]).unwrap_or_default();
    let negative_prompt =
        join_non_empty(&[negative_prompt.as_str(), negative_style.as_str()], "\n");

    if prompt.trim().is_empty() && negative_prompt.trim().is_empty() {
        return None;
    }

    let mut params = Vec::new();
    for key in [
        "generation_mode",
        "width",
        "height",
        "seed",
        "rand_device",
        "cfg_scale",
        "cfg_rescale_multiplier",
        "steps",
        "scheduler",
        "clip_skip",
        "model",
        "strength",
        "vae",
        "app_version",
    ] {
        if let Some(v) = json.get(key).and_then(json_value_to_text) {
            params.push((key.to_string(), v));
        }
    }

    Some(A1111Metadata {
        tool: AiToolKind::InvokeAI,
        prompt,
        negative_prompt,
        params,
        raw: raw.to_string(),
    })
}

fn parse_invokeai_legacy_json(raw: &str, json: &serde_json::Value) -> Option<A1111Metadata> {
    // 旧 sd-metadata の image.prompt は [{"prompt": "...", "weight": 1.0}] という
    // 配列のことがある。要素の prompt フィールドだけを取り出す (生 JSON を
    // prompt として扱わない)。
    let array_prompt = json_path(json, &["image", "prompt"])
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|e| e.get("prompt").and_then(json_value_to_text))
                .collect::<Vec<_>>()
                .join("\n")
        })
        .filter(|s| !s.trim().is_empty());
    let mut prompt = array_prompt
        .or_else(|| {
            first_json_text(
                json,
                &[
                    &["image", "prompt"],
                    &["prompt"],
                    &["image", "positive_prompt"],
                ],
            )
        })
        .unwrap_or_default();
    let mut negative_prompt = first_json_text(
        json,
        &[
            &["image", "negative_prompt"],
            &["negative_prompt"],
            &["image", "negative"],
        ],
    )
    .unwrap_or_default();
    // 旧 InvokeAI は Dream 形式と同様、prompt 中の [..] が negative。
    if negative_prompt.trim().is_empty()
        && let Some((before, inside, after)) = extract_square_bracket_segment(&prompt)
    {
        prompt = join_non_empty(&[before.trim(), after.trim()], " ");
        negative_prompt = inside.trim().to_string();
    }
    if prompt.trim().is_empty() && negative_prompt.trim().is_empty() {
        return None;
    }

    let mut params = Vec::new();
    if let Some(obj) = json.as_object() {
        params.extend(prefix_scalar_params(
            "",
            obj,
            &["image", "prompt", "negative_prompt"],
        ));
    }
    Some(A1111Metadata {
        tool: AiToolKind::InvokeAI,
        prompt,
        negative_prompt,
        params,
        raw: raw.to_string(),
    })
}

fn parse_invokeai_dream(raw: &str) -> Option<A1111Metadata> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    let (prompt_part, flags_part) = split_dream_prompt_and_flags(trimmed);
    let mut prompt = prompt_part.trim().trim_matches('"').to_string();
    let mut negative_prompt = String::new();
    if let Some((before, inside, after)) = extract_square_bracket_segment(&prompt) {
        prompt = join_non_empty(&[before.trim(), after.trim()], " ");
        negative_prompt = inside.trim().to_string();
    }

    let params = parse_dream_flags(flags_part);
    Some(A1111Metadata {
        tool: AiToolKind::InvokeAI,
        prompt,
        negative_prompt,
        params,
        raw: raw.to_string(),
    })
}

fn parse_novelai(chunks: &[(String, String)]) -> Option<A1111Metadata> {
    let software_is_novelai =
        chunk_value(chunks, "Software").is_some_and(|s| s.trim().eq_ignore_ascii_case("NovelAI"));
    let comment_raw = chunk_value(chunks, "Comment")?;
    let comment = parse_json_object(comment_raw)?;
    let looks_novelai =
        software_is_novelai || comment.get("uc").is_some() || comment.get("v4_prompt").is_some();
    if !looks_novelai {
        return None;
    }

    let prompt = comment
        .get("v4_prompt")
        .and_then(novelai_v4_caption_text)
        .or_else(|| chunk_value(chunks, "Description").map(str::to_string))
        .or_else(|| comment.get("prompt").and_then(json_value_to_text))
        .unwrap_or_default();
    let negative_prompt = comment
        .get("v4_negative_prompt")
        .and_then(novelai_v4_caption_text)
        .or_else(|| comment.get("uc").and_then(json_value_to_text))
        .unwrap_or_default();
    if prompt.trim().is_empty() && negative_prompt.trim().is_empty() {
        return None;
    }

    let mut params = Vec::new();
    if let Some(obj) = comment.as_object() {
        params.extend(scalar_params_from_object(
            obj,
            &["prompt", "uc", "v4_prompt", "v4_negative_prompt"],
        ));
    }
    for key in ["Title", "Source", "Software"] {
        if let Some(v) = chunk_value(chunks, key).filter(|v| !v.trim().is_empty()) {
            params.push((key.to_string(), v.to_string()));
        }
    }

    Some(A1111Metadata {
        tool: AiToolKind::NovelAI,
        prompt,
        negative_prompt,
        params,
        raw: comment_raw.to_string(),
    })
}

fn extract_exif_user_comment_metadata_from_path(path: &Path) -> Option<AiMetadata> {
    let exif = rexif::parse_file(path.to_str()?).ok()?;
    extract_exif_user_comment_metadata_from_entries(&exif.entries)
}

fn extract_exif_user_comment_metadata_from_bytes(bytes: &[u8]) -> Option<AiMetadata> {
    let exif = rexif::parse_buffer(bytes).ok()?;
    extract_exif_user_comment_metadata_from_entries(&exif.entries)
}

fn extract_exif_user_comment_metadata_from_entries(
    entries: &[rexif::ExifEntry],
) -> Option<AiMetadata> {
    let raw = entries
        .iter()
        .find(|entry| matches!(entry.tag, rexif::ExifTag::UserComment))
        .and_then(decode_user_comment_entry)?;
    parse_user_comment_metadata(&raw).map(AiMetadata::A1111)
}

/// EXIF UserComment を **検索向け** に分類して (検索テキスト, origin) を返す。
/// 表示用 (`extract_exif_user_comment_metadata_*`) と違い、未知 JSON を
/// `ExifUserCommentSuppressed` として返し、生 UserComment を索引から外させる。
/// - `Ai`         → Negative 除外テキスト + `ExifUserComment`
/// - `UnknownJson`→ 空テキスト + `ExifUserCommentSuppressed` (negative リーク防止)
/// - `Plain`      → `None` (AI ではないので通常 EXIF コメントとして索引させる)
fn exif_user_comment_searchable_from_entries(
    entries: &[rexif::ExifEntry],
) -> Option<(String, MetadataOrigin)> {
    let raw = entries
        .iter()
        .find(|entry| matches!(entry.tag, rexif::ExifTag::UserComment))
        .and_then(decode_user_comment_entry)?;
    match classify_user_comment(&raw) {
        UserCommentClass::Ai(meta) => Some((
            build_searchable_text(&AiMetadata::A1111(meta)),
            MetadataOrigin::ExifUserComment,
        )),
        UserCommentClass::UnknownJson => {
            Some((String::new(), MetadataOrigin::ExifUserCommentSuppressed))
        }
        UserCommentClass::Plain => None,
    }
}

fn exif_user_comment_searchable_from_bytes(bytes: &[u8]) -> Option<(String, MetadataOrigin)> {
    let exif = rexif::parse_buffer(bytes).ok()?;
    exif_user_comment_searchable_from_entries(&exif.entries)
}

fn exif_user_comment_searchable_from_path(path: &Path) -> Option<(String, MetadataOrigin)> {
    let exif = rexif::parse_file(path.to_str()?).ok()?;
    exif_user_comment_searchable_from_entries(&exif.entries)
}

fn decode_user_comment_entry(entry: &rexif::ExifEntry) -> Option<String> {
    match &entry.value {
        rexif::TagValue::Undefined(bytes, le) => decode_user_comment_bytes(bytes, *le),
        rexif::TagValue::Ascii(s) => Some(clean_user_comment_text(s)),
        _ => {
            let s = entry.value_more_readable.trim();
            (!s.is_empty()).then(|| clean_user_comment_text(s))
        }
    }
}

fn decode_user_comment_bytes(bytes: &[u8], le: bool) -> Option<String> {
    const ASCII: &[u8; 8] = b"ASCII\0\0\0";
    const UNICODE: &[u8; 8] = b"UNICODE\0";
    const JIS: &[u8; 8] = b"JIS\0\0\0\0\0";
    let text = if let Some(rest) = bytes.strip_prefix(ASCII) {
        String::from_utf8_lossy(rest).into_owned()
    } else if let Some(rest) = bytes.strip_prefix(UNICODE) {
        let mut u16s = Vec::with_capacity(rest.len() / 2);
        for pair in rest.chunks_exact(2) {
            let code = if le {
                u16::from_le_bytes([pair[0], pair[1]])
            } else {
                u16::from_be_bytes([pair[0], pair[1]])
            };
            u16s.push(code);
        }
        String::from_utf16_lossy(&u16s)
    } else if let Some(rest) = bytes.strip_prefix(JIS) {
        String::from_utf8_lossy(rest).into_owned()
    } else if bytes.len() > 8 && bytes[..8].iter().all(|&b| b == 0) {
        String::from_utf8_lossy(&bytes[8..]).into_owned()
    } else {
        // Pillow/Fooocus は EXIF UserComment に charset preamble 無しの UTF-8 文字列を
        // 書くことがある。rexif の表示文字列に頼ると "undefined encoding [..]" になるため、
        // raw bytes を UTF-8 lossy で直接扱う。
        String::from_utf8_lossy(bytes).into_owned()
    };
    let cleaned = clean_user_comment_text(&text);
    (!cleaned.trim().is_empty()).then_some(cleaned)
}

fn clean_user_comment_text(s: &str) -> String {
    s.trim_matches('\0').trim().to_string()
}

/// EXIF UserComment を A1111 形式とみなしてよいシグネチャがあるか。
///
/// `parse_a1111_as` は空でない任意のテキストに `Some` を返すため、これで前置判定しないと
/// **普通の写真キャプション** (例: 「旅行の写真」) まで AI メタデータと誤分類し、
/// メタデータパネルに偽の AI ツールバッジが出て、検索索引でも EXIF コメントとして
/// 扱われなくなる。A1111 / Forge の JPEG は必ず Negative prompt 行か Steps パラメータ行を
/// 持つので、それを必須にする。
fn looks_like_a1111(raw: &str) -> bool {
    raw.contains("\nNegative prompt:") || find_params_line(raw).is_some()
}

/// EXIF UserComment の検索/表示向け分類。
enum UserCommentClass {
    /// 認識した AI メタデータ。
    Ai(A1111Metadata),
    /// 未知の JSON object。Positive/Negative を分離できないので AI として index/display
    /// しないが、生 JSON を EXIF 索引へ流すと negative がリークするので suppress する。
    UnknownJson,
    /// AI ではない通常テキストコメント。EXIF として通常 index する。
    Plain,
}

fn classify_user_comment(raw: &str) -> UserCommentClass {
    if let Some(json) = parse_json_object(raw) {
        if let Some(meta) = parse_swarmui_json(raw, &json) {
            return UserCommentClass::Ai(meta);
        }
        if let Some(meta) = parse_ruinedfooocus_json(raw, &json) {
            return UserCommentClass::Ai(meta);
        }
        if let Some(meta) = parse_fooocus_json(raw, &json, &[]) {
            return UserCommentClass::Ai(meta);
        }
        if let Some(meta) = parse_invokeai_json(raw, &json) {
            return UserCommentClass::Ai(meta);
        }
        if let Some(meta) = parse_invokeai_legacy_json(raw, &json) {
            return UserCommentClass::Ai(meta);
        }
        return UserCommentClass::UnknownJson;
    }
    if looks_like_a1111(raw)
        && let Some(meta) = parse_a1111_as(raw, AiToolKind::JpegExif)
    {
        return UserCommentClass::Ai(meta);
    }
    UserCommentClass::Plain
}

fn parse_user_comment_metadata(raw: &str) -> Option<A1111Metadata> {
    match classify_user_comment(raw) {
        UserCommentClass::Ai(meta) => Some(meta),
        UserCommentClass::UnknownJson | UserCommentClass::Plain => None,
    }
}

fn first_json_text(json: &serde_json::Value, paths: &[&[&str]]) -> Option<String> {
    for path in paths {
        if let Some(v) = json_path(json, path).and_then(json_value_to_text) {
            if !v.trim().is_empty() {
                return Some(v);
            }
        }
    }
    None
}

fn json_path<'a>(json: &'a serde_json::Value, path: &[&str]) -> Option<&'a serde_json::Value> {
    let mut cur = json;
    for key in path {
        cur = cur.get(*key)?;
    }
    Some(cur)
}

fn json_value_to_text(value: &serde_json::Value) -> Option<String> {
    match value {
        serde_json::Value::Null => None,
        serde_json::Value::Bool(v) => Some(v.to_string()),
        serde_json::Value::Number(v) => Some(v.to_string()),
        serde_json::Value::String(v) => Some(v.clone()),
        serde_json::Value::Array(values) => {
            let parts: Vec<String> = values.iter().filter_map(json_value_to_text).collect();
            (!parts.is_empty()).then(|| parts.join(", "))
        }
        serde_json::Value::Object(_) => serde_json::to_string(value).ok(),
    }
}

fn scalar_params_from_object(
    obj: &serde_json::Map<String, serde_json::Value>,
    skip: &[&str],
) -> Vec<(String, String)> {
    prefix_scalar_params("", obj, skip)
}

fn prefix_scalar_params(
    prefix: &str,
    obj: &serde_json::Map<String, serde_json::Value>,
    skip: &[&str],
) -> Vec<(String, String)> {
    let mut params = Vec::new();
    for (key, value) in obj {
        if skip.iter().any(|candidate| key == candidate) {
            continue;
        }
        if let Some(text) = json_value_to_text(value).filter(|s| !s.trim().is_empty()) {
            params.push((format!("{prefix}{key}"), text));
        }
    }
    params
}

fn join_non_empty(parts: &[&str], sep: &str) -> String {
    parts
        .iter()
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(sep)
}

fn novelai_v4_caption_text(value: &serde_json::Value) -> Option<String> {
    let mut parts = Vec::new();
    collect_novelai_caption_parts(value, &mut parts);
    (!parts.is_empty()).then(|| parts.join("\n"))
}

fn collect_novelai_caption_parts(value: &serde_json::Value, out: &mut Vec<String>) {
    match value {
        serde_json::Value::String(s) if !s.trim().is_empty() => out.push(s.trim().to_string()),
        serde_json::Value::Object(obj) => {
            if let Some(caption) = obj.get("caption") {
                collect_novelai_caption_parts(caption, out);
            }
            for key in ["base_caption", "char_caption"] {
                if let Some(s) = obj.get(key).and_then(|v| v.as_str()) {
                    if !s.trim().is_empty() {
                        out.push(s.trim().to_string());
                    }
                }
            }
            for key in ["char_captions", "characters"] {
                if let Some(arr) = obj.get(key).and_then(|v| v.as_array()) {
                    for child in arr {
                        collect_novelai_caption_parts(child, out);
                    }
                }
            }
        }
        serde_json::Value::Array(arr) => {
            for child in arr {
                collect_novelai_caption_parts(child, out);
            }
        }
        _ => {}
    }
}

fn split_dream_prompt_and_flags(raw: &str) -> (&str, &str) {
    if let Some(stripped) = raw.strip_prefix('"') {
        let mut escaped = false;
        for (idx, ch) in stripped.char_indices() {
            if escaped {
                escaped = false;
                continue;
            }
            if ch == '\\' {
                escaped = true;
                continue;
            }
            if ch == '"' {
                let end = idx + 2; // opening quote + closing quote byte position
                return (&raw[..end], raw[end..].trim());
            }
        }
    }
    if let Some(pos) = raw.find(" -") {
        (&raw[..pos], raw[pos..].trim())
    } else {
        (raw, "")
    }
}

fn extract_square_bracket_segment(s: &str) -> Option<(String, String, String)> {
    let end = s.rfind(']')?;
    let start = s[..end].rfind('[')?;
    Some((
        s[..start].to_string(),
        s[start + 1..end].to_string(),
        s[end + 1..].to_string(),
    ))
}

fn parse_dream_flags(flags: &str) -> Vec<(String, String)> {
    let mut params = Vec::new();
    let mut iter = flags.split_whitespace().peekable();
    while let Some(flag) = iter.next() {
        if !flag.starts_with('-') {
            continue;
        }
        let key = match flag {
            "-s" | "--steps" => "Steps",
            "-S" | "--seed" => "Seed",
            "-W" | "--width" => "Width",
            "-H" | "--height" => "Height",
            "-C" | "--cfg_scale" => "CFG scale",
            "-A" | "--sampler" => "Sampler",
            "-m" | "--model" => "Model",
            _ => flag.trim_start_matches('-'),
        };
        let Some(value) = iter.peek().copied().filter(|v| !v.starts_with('-')) else {
            continue;
        };
        iter.next();
        params.push((key.to_string(), value.trim_matches('"').to_string()));
    }
    params
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
    let outcome = detect_and_parse_outcome(chunks);
    let mut out = String::new();

    if let Some(ref parsed) = outcome {
        // 未知の parameters JSON は表示用には Unknown として保持するが、検索には
        // raw JSON を入れない。JSON 内に negative 系キーがある場合の混入を避ける。
        let suppress_unknown_raw =
            matches!(parsed.meta, AiMetadata::Unknown(_)) && !parsed.consumed_keys.is_empty();
        if !suppress_unknown_raw {
            append_line(&mut out, &build_searchable_text(&parsed.meta));
        }
    }

    // 認識済み AI キーの生値には Negative が残っているので再掲しない。
    // Unknown でも consumed_keys がある場合 (未知 parameters JSON など) は、
    // consumed 以外の非 AI チャンクだけを足す。
    let include_non_ai_chunks = match outcome {
        Some(ParseOutcome {
            meta: AiMetadata::A1111(_) | AiMetadata::ComfyUI(_),
            ..
        }) => true,
        Some(ParseOutcome {
            meta: AiMetadata::Unknown(_),
            ref consumed_keys,
        }) if !consumed_keys.is_empty() => true,
        Some(ParseOutcome {
            meta: AiMetadata::Unknown(_),
            ..
        }) => false,
        None => true,
    };

    if include_non_ai_chunks {
        for (k, v) in chunks {
            if outcome
                .as_ref()
                .is_some_and(|parsed| parsed.consumed_keys.contains(&k.as_str()))
            {
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
    build_searchable_from_path_with_origin(path).0
}

pub fn build_searchable_from_path_with_origin(path: &Path) -> (String, Option<MetadataOrigin>) {
    let Some(ext) = path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
    else {
        return (String::new(), None);
    };
    match ext.as_str() {
        "png" => {
            let chunks = read_png_text_chunks(path).unwrap_or_default();
            if chunks.is_empty() {
                (String::new(), None)
            } else {
                (
                    build_searchable_from_chunks(&chunks),
                    Some(MetadataOrigin::PngText),
                )
            }
        }
        "jpg" | "jpeg" | "jfif" => {
            let Some((text, origin)) = exif_user_comment_searchable_from_path(path) else {
                return (String::new(), None);
            };
            (text, Some(origin))
        }
        _ => (String::new(), None),
    }
}

/// PNG バイト列から Negative Prompt を除外した検索対象文字列を取得する。
pub fn build_searchable_from_bytes(bytes: &[u8]) -> String {
    build_searchable_from_bytes_with_origin(bytes).0
}

pub fn build_searchable_from_bytes_with_origin(bytes: &[u8]) -> (String, Option<MetadataOrigin>) {
    if is_png_bytes(bytes) {
        let chunks = read_png_text_chunks_from_bytes(bytes).unwrap_or_default();
        if chunks.is_empty() {
            return (String::new(), None);
        }
        return (
            build_searchable_from_chunks(&chunks),
            Some(MetadataOrigin::PngText),
        );
    }
    let Some((text, origin)) = exif_user_comment_searchable_from_bytes(bytes) else {
        return (String::new(), None);
    };
    (text, Some(origin))
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
    parse_a1111_as(raw, AiToolKind::A1111)
}

fn parse_a1111_as(raw: &str, tool: AiToolKind) -> Option<A1111Metadata> {
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
        tool,
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
            let class = node
                .get("class_type")
                .and_then(|c| c.as_str())
                .unwrap_or("");

            match class {
                "KSampler" | "KSamplerAdvanced" | "SamplerCustom" => {
                    // 生成パラメータを抽出
                    if let Some(inputs) = node.get("inputs").and_then(|i| i.as_object()) {
                        for &key in &[
                            "steps",
                            "cfg",
                            "sampler_name",
                            "scheduler",
                            "seed",
                            "denoise",
                        ] {
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
                let class = node
                    .get("class_type")
                    .and_then(|c| c.as_str())
                    .unwrap_or("");
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
    let class = node
        .get("class_type")
        .and_then(|c| c.as_str())
        .unwrap_or("");

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

    /// zlib bomb 防御 (post-v1.3.0 backlog): 上限 (16 MiB) を超える解凍出力は拒否し、
    /// 正規サイズはそのまま往復できる。
    #[test]
    fn decompress_zlib_rejects_bomb_and_roundtrips_small() {
        use flate2::Compression;
        use flate2::write::ZlibEncoder;
        use std::io::Write;

        // 正規: 小さいテキストは往復できる。
        let mut enc = ZlibEncoder::new(Vec::new(), Compression::default());
        enc.write_all("プロンプト metadata".as_bytes()).unwrap();
        let small = enc.finish().unwrap();
        assert_eq!(decompress_zlib(&small).unwrap(), "プロンプト metadata");

        // 爆弾: 17 MiB のゼロは高圧縮率で小さく圧縮されるが、解凍は上限超で拒否される。
        let mut enc = ZlibEncoder::new(Vec::new(), Compression::best());
        enc.write_all(&vec![0u8; 17 * 1024 * 1024]).unwrap();
        let bomb = enc.finish().unwrap();
        assert!(
            bomb.len() < 1024 * 1024,
            "bomb should compress small (got {} bytes)",
            bomb.len()
        );
        assert!(decompress_zlib(&bomb).is_err());
    }

    fn chunks(items: &[(&str, &str)]) -> Vec<(String, String)> {
        items
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect()
    }

    fn expect_prompt_meta(meta: Option<AiMetadata>) -> A1111Metadata {
        match meta {
            Some(AiMetadata::A1111(meta)) => meta,
            other => panic!("expected normalized prompt metadata, got {other:?}"),
        }
    }

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
        assert!(
            meta.params
                .iter()
                .any(|(k, v)| k == "Model" && v == "sd_xl_base")
        );
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
        assert!(
            meta.extracted_prompts
                .contains(&"a beautiful sunset".to_string())
        );
        assert!(meta.extracted_negatives.contains(&"ugly".to_string()));
        assert!(
            meta.sampler_params
                .iter()
                .any(|(k, v)| k == "steps" && v == "20")
        );
        assert!(
            meta.sampler_params
                .iter()
                .any(|(k, v)| k == "model" && v == "sd_xl_base_1.0.safetensors")
        );
    }

    #[test]
    fn test_detect_comfyui() {
        let chunks = vec![(
            "prompt".to_string(),
            r#"{"1": {"class_type": "KSampler", "inputs": {"steps": 10}}}"#.to_string(),
        )];
        let result = detect_and_parse(&chunks);
        assert!(matches!(result, Some(AiMetadata::ComfyUI(_))));
    }

    #[test]
    fn test_detect_a1111() {
        let chunks = vec![(
            "parameters".to_string(),
            "hello world\nSteps: 20, Sampler: Euler".to_string(),
        )];
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
    fn test_detect_novelai_before_description_and_excludes_uc() {
        let chunks = chunks(&[
            ("Title", "AI generated image"),
            ("Description", "masterpiece cat in space"),
            ("Software", "NovelAI"),
            ("Source", "Stable Diffusion abcdef"),
            (
                "Comment",
                r#"{"steps":28,"sampler":"k_euler","scale":5.5,"seed":123,"uc":"novelai-negative-leak"}"#,
            ),
        ]);
        let meta = expect_prompt_meta(detect_and_parse(&chunks));
        assert_eq!(meta.tool, AiToolKind::NovelAI);
        assert_eq!(meta.prompt, "masterpiece cat in space");
        assert_eq!(meta.negative_prompt, "novelai-negative-leak");
        assert!(
            meta.params
                .iter()
                .any(|(k, v)| k == "sampler" && v == "k_euler")
        );

        let text = build_searchable_from_chunks(&chunks);
        assert!(text.contains("masterpiece cat"));
        assert!(text.contains("k_euler"));
        assert!(!text.contains("novelai-negative-leak"), "text={text}");
    }

    #[test]
    fn test_detect_novelai_v4_caption_shape() {
        let chunks = chunks(&[
            ("Software", "NovelAI"),
            (
                "Comment",
                r#"{
                    "v4_prompt": {"caption": {
                        "base_caption": "base scene",
                        "char_captions": [{"char_caption": "hero character"}]
                    }},
                    "v4_negative_prompt": {"caption": {"base_caption": "v4-negative-leak"}},
                    "seed": 45
                }"#,
            ),
        ]);
        let meta = expect_prompt_meta(detect_and_parse(&chunks));
        assert_eq!(meta.tool, AiToolKind::NovelAI);
        assert!(meta.prompt.contains("base scene"));
        assert!(meta.prompt.contains("hero character"));
        let text = build_searchable_from_chunks(&chunks);
        assert!(!text.contains("v4-negative-leak"), "text={text}");
    }

    #[test]
    fn test_parameters_json_prompt_negative_pair_is_recognized() {
        // prompt + negative_prompt の組は未知亜種でも生成メタデータとして解釈する
        // (Fooocus 系の汎用指紋)。positive は検索でき、negative は混入しない。
        let chunks = chunks(&[(
            "parameters",
            r#"{"prompt":"unknown positive","negative_prompt":"json-negative-leak"}"#,
        )]);
        let meta = expect_prompt_meta(detect_and_parse(&chunks));
        assert_eq!(meta.prompt, "unknown positive");
        assert_eq!(meta.negative_prompt, "json-negative-leak");
        let text = build_searchable_from_chunks(&chunks);
        assert!(text.contains("unknown positive"));
        assert!(!text.contains("json-negative-leak"), "text={text}");
    }

    #[test]
    fn test_parameters_json_unknown_does_not_leak_raw_negative() {
        // どの抽出器にも一致しない未知 JSON は表示専用 Unknown のまま、
        // 検索には raw を入れない (negative 系キーの混入防止)。
        let chunks = chunks(&[(
            "parameters",
            r#"{"caption":"unknown positive","custom_negative":"json-negative-leak"}"#,
        )]);
        assert!(matches!(
            detect_and_parse(&chunks),
            Some(AiMetadata::Unknown(_))
        ));
        let text = build_searchable_from_chunks(&chunks);
        assert!(
            !text.contains("json-negative-leak"),
            "unknown parameters JSON must not be searched raw: {text}"
        );
        assert!(
            !text.contains("unknown positive"),
            "unknown parameters JSON is display-only until a concrete extractor recognizes it: {text}"
        );
    }

    #[test]
    fn test_detect_swarmui_parameters_json() {
        let chunks = chunks(&[(
            "parameters",
            r#"{
                "sui_image_params": {
                    "prompt": "a photo of a cat",
                    "negativeprompt": "swarm-negative-leak",
                    "model": "OfficialStableDiffusion/sd_xl_base_1.0",
                    "seed": 1,
                    "steps": 20,
                    "cfgscale": 7.0
                }
            }"#,
        )]);
        let meta = expect_prompt_meta(detect_and_parse(&chunks));
        assert_eq!(meta.tool, AiToolKind::SwarmUI);
        assert_eq!(meta.prompt, "a photo of a cat");
        assert_eq!(meta.negative_prompt, "swarm-negative-leak");
        assert!(
            meta.params
                .iter()
                .any(|(k, v)| k == "model" && v.contains("sd_xl"))
        );
        let text = build_searchable_from_chunks(&chunks);
        assert!(text.contains("a photo of a cat"));
        assert!(!text.contains("swarm-negative-leak"), "text={text}");
    }

    #[test]
    fn test_detect_fooocus_parameters_json() {
        let chunks = chunks(&[
            (
                "parameters",
                r#"{
                    "prompt": "fooocus positive",
                    "negative_prompt": "fooocus-negative-leak",
                    "base_model": "juggernautXL",
                    "guidance_scale": 4,
                    "sampler": "dpmpp_2m_sde_gpu",
                    "seed": 222
                }"#,
            ),
            ("fooocus_scheme", "fooocus"),
        ]);
        let meta = expect_prompt_meta(detect_and_parse(&chunks));
        assert_eq!(meta.tool, AiToolKind::Fooocus);
        assert_eq!(meta.prompt, "fooocus positive");
        assert!(
            meta.params
                .iter()
                .any(|(k, v)| k == "base_model" && v == "juggernautXL")
        );
        let text = build_searchable_from_chunks(&chunks);
        assert!(text.contains("fooocus positive"));
        assert!(text.contains("juggernautXL"));
        assert!(!text.contains("fooocus-negative-leak"), "text={text}");
    }

    #[test]
    fn test_detect_ruinedfooocus_parameters_json() {
        let chunks = chunks(&[(
            "parameters",
            r#"{
                "software": "RuinedFooocus",
                "Prompt": "ruined positive",
                "Negative": "ruined-negative-leak",
                "Seed": 333
            }"#,
        )]);
        let meta = expect_prompt_meta(detect_and_parse(&chunks));
        assert_eq!(meta.tool, AiToolKind::Fooocus);
        assert_eq!(meta.prompt, "ruined positive");
        let text = build_searchable_from_chunks(&chunks);
        assert!(!text.contains("ruined-negative-leak"), "text={text}");
    }

    #[test]
    fn test_detect_invokeai_metadata_json() {
        let chunks = chunks(&[(
            "invokeai_metadata",
            r#"{
                "positive_prompt": "invoke positive",
                "negative_prompt": "invoke-negative-leak",
                "seed": 444,
                "steps": 30,
                "cfg_scale": 6.5,
                "model": {"name": "invoke-model"}
            }"#,
        )]);
        let meta = expect_prompt_meta(detect_and_parse(&chunks));
        assert_eq!(meta.tool, AiToolKind::InvokeAI);
        assert_eq!(meta.prompt, "invoke positive");
        let text = build_searchable_from_chunks(&chunks);
        assert!(text.contains("invoke positive"));
        assert!(text.contains("invoke-model"));
        assert!(!text.contains("invoke-negative-leak"), "text={text}");
    }

    #[test]
    fn test_detect_invokeai_legacy_sd_metadata_json() {
        let chunks = chunks(&[(
            "sd-metadata",
            r#"{"image":{"prompt":"legacy positive","negative_prompt":"legacy-negative-leak"},"seed":555}"#,
        )]);
        let meta = expect_prompt_meta(detect_and_parse(&chunks));
        assert_eq!(meta.tool, AiToolKind::InvokeAI);
        assert_eq!(meta.prompt, "legacy positive");
        let text = build_searchable_from_chunks(&chunks);
        assert!(!text.contains("legacy-negative-leak"), "text={text}");
    }

    #[test]
    fn test_detect_invokeai_dream_text() {
        let chunks = chunks(&[(
            "Dream",
            r#""dream positive [dream-negative-leak]" -s 24 -S 999 -W 512 -H 768 -C 7.5 -A k_lms"#,
        )]);
        let meta = expect_prompt_meta(detect_and_parse(&chunks));
        assert_eq!(meta.tool, AiToolKind::InvokeAI);
        assert_eq!(meta.prompt, "dream positive");
        assert_eq!(meta.negative_prompt, "dream-negative-leak");
        assert!(meta.params.iter().any(|(k, v)| k == "Steps" && v == "24"));
        let text = build_searchable_from_chunks(&chunks);
        assert!(!text.contains("dream-negative-leak"), "text={text}");
    }

    #[test]
    fn test_user_comment_ascii_a1111_is_ai_metadata() {
        let raw = "jpeg positive\nNegative prompt: jpeg-negative-leak\nSteps: 18, Seed: 123";
        let mut bytes = b"ASCII\0\0\0".to_vec();
        bytes.extend_from_slice(raw.as_bytes());
        let decoded = decode_user_comment_bytes(&bytes, true).unwrap();
        let meta = parse_user_comment_metadata(&decoded).unwrap();
        assert_eq!(meta.tool, AiToolKind::JpegExif);
        let text = build_searchable_text(&AiMetadata::A1111(meta));
        assert!(text.contains("jpeg positive"));
        assert!(!text.contains("jpeg-negative-leak"), "text={text}");
    }

    #[test]
    fn test_user_comment_json_prefers_ai_reader_over_generic_json() {
        let raw = r#"{
            "sui_image_params": {
                "prompt": "json usercomment positive",
                "negativeprompt": "json-usercomment-negative-leak",
                "model": "model-name"
            }
        }"#;
        let meta = parse_user_comment_metadata(raw).unwrap();
        assert_eq!(meta.tool, AiToolKind::SwarmUI);
        let text = build_searchable_text(&AiMetadata::A1111(meta));
        assert!(text.contains("json usercomment positive"));
        assert!(text.contains("model-name"));
        assert!(
            !text.contains("json-usercomment-negative-leak"),
            "text={text}"
        );
    }

    #[test]
    fn user_comment_plain_text_is_not_ai_metadata() {
        // 普通の写真キャプション (A1111 シグネチャ無し) は AI メタとして拾わない。
        // これがないとパネルに偽の AI バッジが出て、検索でも EXIF コメント扱いされない。
        assert!(parse_user_comment_metadata("旅行の写真 2026 夏").is_none());
        assert!(parse_user_comment_metadata("Shot on my camera at the beach").is_none());
        assert!(matches!(
            classify_user_comment("just a normal note"),
            UserCommentClass::Plain
        ));
    }

    #[test]
    fn user_comment_a1111_signature_is_still_ai_metadata() {
        // Negative prompt 行があれば AI。
        let meta = parse_user_comment_metadata("pos\nNegative prompt: neg\nSteps: 20").unwrap();
        assert_eq!(meta.tool, AiToolKind::JpegExif);
        // Steps パラメータ行だけでも AI。
        assert!(parse_user_comment_metadata("a cat\nSteps: 20, Seed: 1").is_some());
    }

    #[test]
    fn user_comment_unknown_json_is_suppressed() {
        // 認識できない JSON は AI として index しない (positive/negative を分離できず、
        // 生 JSON を索引へ流すと negative がリークするため suppress する)。
        let raw = r#"{"some_tool":{"prompt":"p","secret_negative":"should-not-leak"}}"#;
        assert!(parse_user_comment_metadata(raw).is_none());
        assert!(matches!(
            classify_user_comment(raw),
            UserCommentClass::UnknownJson
        ));
    }

    #[test]
    fn suppressed_origin_skips_raw_user_comment() {
        assert!(MetadataOrigin::ExifUserComment.suppresses_exif_user_comment());
        assert!(MetadataOrigin::ExifUserCommentSuppressed.suppresses_exif_user_comment());
        assert!(!MetadataOrigin::PngText.suppresses_exif_user_comment());
    }

    #[test]
    fn optional_external_novelai_sample_smoke() {
        let Some(dir) = std::env::var_os("MIV_AI_METADATA_SAMPLE_DIR") else {
            return;
        };
        let path = std::path::PathBuf::from(dir).join("novelai-sample-cat.png");
        if !path.exists() {
            return;
        }
        let meta = expect_prompt_meta(extract_metadata(&path));
        assert_eq!(meta.tool, AiToolKind::NovelAI);
        let text = build_searchable_from_path(&path);
        assert!(text.contains("cat"));
        assert!(
            !text.contains("lowres"),
            "NovelAI uc leaked into search text"
        );
    }

    #[test]
    fn test_easydiffusion_chunks_internal_keys() {
        // sdkit の embed はメタデータ dict をフィールドごとに独立チャンクで書く。
        let chunks = vec![
            ("prompt".to_string(), "ed positive prompt".to_string()),
            (
                "negative_prompt".to_string(),
                "ed-negative-leak".to_string(),
            ),
            ("seed".to_string(), "12345".to_string()),
            (
                "use_stable_diffusion_model".to_string(),
                "sd-v1-5".to_string(),
            ),
            ("num_inference_steps".to_string(), "25".to_string()),
        ];
        let meta = expect_prompt_meta(detect_and_parse(&chunks));
        assert_eq!(meta.tool, AiToolKind::EasyDiffusion);
        assert_eq!(meta.prompt, "ed positive prompt");
        assert_eq!(meta.negative_prompt, "ed-negative-leak");
        assert!(meta.params.iter().any(|(k, v)| k == "seed" && v == "12345"));
        let text = build_searchable_from_chunks(&chunks);
        assert!(text.contains("ed positive prompt"));
        assert!(text.contains("sd-v1-5"));
        assert!(!text.contains("ed-negative-leak"), "text={text}");
    }

    #[test]
    fn test_easydiffusion_chunks_label_keys() {
        // 表示名キー (TASK_TEXT_MAPPING の値側) を書くバージョンもある。
        let chunks = vec![
            ("Prompt".to_string(), "label positive".to_string()),
            (
                "Negative Prompt".to_string(),
                "label-negative-leak".to_string(),
            ),
            ("Stable Diffusion model".to_string(), "sd-v1-4".to_string()),
        ];
        let meta = expect_prompt_meta(detect_and_parse(&chunks));
        assert_eq!(meta.tool, AiToolKind::EasyDiffusion);
        assert_eq!(meta.prompt, "label positive");
        let text = build_searchable_from_chunks(&chunks);
        assert!(text.contains("sd-v1-4"));
        assert!(!text.contains("label-negative-leak"), "text={text}");
    }

    #[test]
    fn test_fooocus_mre_comment_json() {
        // Fooocus-MRE / 新しめの Fooocus は Comment チャンクに JSON を書く
        // (DiffusionToolkit の ReadFooocusMREParameters と同じキー構成)。
        let comment = r#"{
            "prompt": "mre positive",
            "real_prompt": ["mre expanded positive"],
            "negative_prompt": "mre-negative-leak",
            "real_negative_prompt": ["mre-real-negative-leak"],
            "steps": 30, "cfg": 4.0, "width": 1152, "height": 896,
            "seed": 42, "sampler": "dpmpp_2m_sde_gpu",
            "base_model": "sd_xl_base_1.0", "performance": "Speed"
        }"#;
        let chunks = vec![("Comment".to_string(), comment.to_string())];
        let meta = expect_prompt_meta(detect_and_parse(&chunks));
        assert_eq!(meta.tool, AiToolKind::Fooocus);
        assert_eq!(meta.prompt, "mre positive");
        assert_eq!(meta.negative_prompt, "mre-negative-leak");
        let text = build_searchable_from_chunks(&chunks);
        assert!(text.contains("mre positive"));
        assert!(text.contains("sd_xl_base_1.0"));
        assert!(!text.contains("mre-negative-leak"), "text={text}");
        assert!(!text.contains("mre-real-negative-leak"), "text={text}");
    }

    #[test]
    fn optional_external_sdparsers_samples_smoke() {
        let Some(dir) = std::env::var_os("MIV_AI_METADATA_SAMPLE_DIR") else {
            return;
        };
        let dir = std::path::PathBuf::from(dir);
        // (ファイル名, 期待ツール, 検索テキストに必要な正プロンプト断片,
        //  混入してはならない negative 断片)
        let cases: &[(&str, AiToolKind, &str, &str)] = &[
            (
                "sdparsers-automatic1111_cropped.png",
                AiToolKind::A1111,
                "photo of a duck",
                "monochrome",
            ),
            // zTXt parameters が IDAT より後ろにあるエッジケース
            (
                "sdparsers-text_after_idat.png",
                AiToolKind::A1111,
                "photo of a duck",
                "monochrome",
            ),
            (
                "sdparsers-fooocus1_cropped.png",
                AiToolKind::Fooocus,
                "a smiling goldfish",
                "worst quality",
            ),
            (
                "sdparsers-invokeai_imeta1.png",
                AiToolKind::InvokeAI,
                "digital artwork, oil painting",
                "oversaturated",
            ),
            (
                "sdparsers-invokeai_sdmeta1.png",
                AiToolKind::InvokeAI,
                "professional full body photo",
                "glowing eyes",
            ),
            (
                "sdparsers-invokeai_dream1.png",
                AiToolKind::InvokeAI,
                "professional full body photo",
                "glowing eyes",
            ),
            (
                "sdparsers-novelai1_cropped.png",
                AiToolKind::NovelAI,
                "cat, space, icon",
                "lowres",
            ),
        ];
        for (name, tool, positive, negative) in cases {
            let path = dir.join(name);
            if !path.exists() {
                continue;
            }
            let meta = expect_prompt_meta(extract_metadata(&path));
            assert_eq!(meta.tool, *tool, "{name}");
            let text = build_searchable_from_path(&path);
            assert!(
                text.contains(positive),
                "{name}: positive prompt missing from search text: {text}"
            );
            assert!(
                !text.contains(negative),
                "{name}: negative prompt leaked into search text"
            );
        }

        // JPEG (EXIF UserComment) 経由の A1111 互換テキスト
        let jpg = dir.join("sdparsers-automatic1111_cropped.jpg");
        if jpg.exists() {
            let meta = expect_prompt_meta(extract_metadata(&jpg));
            assert_eq!(meta.tool, AiToolKind::JpegExif);
            assert!(meta.prompt.contains("photo of a duck"));
            let text = build_searchable_text(&AiMetadata::A1111(meta));
            assert!(!text.contains("monochrome"));
        }

        // stealth pnginfo (アルファ LSB のみ、テキストチャンクなし) はスコープ外。
        // 誤検出して Unknown 等を返さないこと。
        let stealth = dir.join("sdparsers-automatic1111_stealth.png");
        if stealth.exists() {
            assert!(extract_metadata(&stealth).is_none());
        }
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
