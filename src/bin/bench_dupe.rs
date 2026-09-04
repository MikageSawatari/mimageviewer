//! Duplicate-signature measurement helper.
//!
//! This binary deliberately reports distributions and caller-selected bins. It
//! does not contain an accept/reject threshold for duplicate detection.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::File;
use std::io::{BufRead, BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::time::Instant;

use image::{DynamicImage, GenericImageView, Rgb, RgbImage, Rgba, RgbaImage};
use mimageviewer::dupe::{self, Algo, LumaMetrics, Proxy, Sig, Signature};
use mimageviewer::folder_tree::{SUPPORTED_EXTENSIONS, is_apple_double};
use mimageviewer::pdf_loader::{self, CancelWaitPolicy, JobPriority};
use mimageviewer::thumb_loader::{
    DctDecodeError, apply_exif_orientation, apply_exif_orientation_from_bytes,
    decode_jpeg_turbo_scaled_from_bytes,
};
use rayon::prelude::*;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

const SCHEMA_VERSION: u32 = 1;
const JPEG_DCT_TARGET_EDGE: u32 = 64;
const DEFAULT_LARGE_DIFF_THRESHOLD_BIN: u8 = 0;

type Result<T> = std::result::Result<T, String>;

#[derive(Debug, Serialize, Deserialize)]
struct ScanRecord {
    schema_version: u32,
    proxy_version: u32,
    path: PathBuf,
    width: u32,
    height: u32,
    file_size: u64,
    extension: String,
    decode: DecodeInfo,
    decode_ms: f64,
    signature_ms: f64,
    total_ms: f64,
    signatures: Vec<StoredSignature>,
}

#[derive(Debug, Serialize, Deserialize)]
struct DecodeInfo {
    method: String,
    scale_num: u32,
    scale_den: u32,
    decoded_width: u32,
    decoded_height: u32,
    note: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct StoredSignature {
    algo: String,
    kind: String,
    hex: String,
    bit_width: Option<u32>,
    value_count: Option<u32>,
    quality: u8,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
struct LumaDistance {
    l1: f32,
    l1_gain_offset: f32,
    grad_l1: f32,
    large_diff_area: f32,
    aspect_ratio_delta: f32,
}

impl From<LumaMetrics> for LumaDistance {
    fn from(value: LumaMetrics) -> Self {
        Self {
            l1: value.l1,
            l1_gain_offset: value.l1_gain_offset,
            grad_l1: value.grad_l1,
            large_diff_area: value.large_diff_area,
            aspect_ratio_delta: value.aspect_ratio_delta,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct PairDistances {
    pdq256: u32,
    pdq64: u32,
    phash63: u32,
    blockhash256: u32,
    luma32: LumaDistance,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct CandidateBins {
    exhaustive: bool,
    pdq256: Option<u32>,
    pdq64: Option<u32>,
    phash63: Option<u32>,
    blockhash256: Option<u32>,
    luma_l1: Option<f32>,
    luma_gain_offset: Option<f32>,
    luma_grad: Option<f32>,
    luma_large_diff_area: Option<f32>,
    aspect_ratio_delta: Option<f32>,
    large_diff_threshold_bin: u8,
}

impl CandidateBins {
    fn exhaustive(large_diff_threshold_bin: u8) -> Self {
        Self {
            exhaustive: true,
            pdq256: None,
            pdq64: None,
            phash63: None,
            blockhash256: None,
            luma_l1: None,
            luma_gain_offset: None,
            luma_grad: None,
            luma_large_diff_area: None,
            aspect_ratio_delta: None,
            large_diff_threshold_bin,
        }
    }

    fn has_selected_bin(&self) -> bool {
        self.pdq256.is_some()
            || self.pdq64.is_some()
            || self.phash63.is_some()
            || self.blockhash256.is_some()
            || self.luma_l1.is_some()
            || self.luma_gain_offset.is_some()
            || self.luma_grad.is_some()
            || self.luma_large_diff_area.is_some()
            || self.aspect_ratio_delta.is_some()
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct PairRecord {
    schema_version: u32,
    path_a: PathBuf,
    path_b: PathBuf,
    dims_a: (u32, u32),
    dims_b: (u32, u32),
    distances: PairDistances,
    candidate_by: Vec<String>,
    bins: CandidateBins,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum SynthDistance {
    Hamming { value: u32 },
    Luma { metrics: LumaDistance },
}

#[derive(Debug, Serialize, Deserialize)]
struct SynthRecord {
    schema_version: u32,
    relation: String,
    transformation: String,
    source_path: PathBuf,
    other_path: Option<PathBuf>,
    algo: String,
    dims_a: (u32, u32),
    dims_b: (u32, u32),
    quality_a: u8,
    quality_b: u8,
    large_diff_threshold_bin: u8,
    distance: SynthDistance,
}

struct DecodedImage {
    rgba: RgbaImage,
    source_dims: (u32, u32),
    method: String,
    scale_num: u32,
    scale_den: u32,
    note: Option<String>,
}

#[derive(Clone)]
struct PreparedSignatures {
    pdq256: Signature,
    pdq64: Signature,
    phash63: Signature,
    blockhash256: Signature,
    luma32: Signature,
}

impl PreparedSignatures {
    fn compute(proxy: &Proxy) -> Self {
        Self {
            pdq256: dupe::compute(Algo::Pdq256, proxy),
            pdq64: dupe::compute(Algo::Pdq64, proxy),
            phash63: dupe::compute(Algo::Phash63, proxy),
            blockhash256: dupe::compute(Algo::Blockhash256, proxy),
            luma32: dupe::compute(Algo::Luma32, proxy),
        }
    }

    fn get(&self, algo: Algo) -> &Signature {
        match algo {
            Algo::Pdq256 => &self.pdq256,
            Algo::Pdq64 => &self.pdq64,
            Algo::Phash63 => &self.phash63,
            Algo::Blockhash256 => &self.blockhash256,
            Algo::Luma32 => &self.luma32,
        }
    }
}

struct PreparedScan {
    path: PathBuf,
    dims: (u32, u32),
    signatures: PreparedSignatures,
}

fn main() {
    if std::env::args().any(|arg| arg == pdf_loader::PDF_WORKER_ARG) {
        mimageviewer::data_dir::init();
        pdf_loader::run_worker_process();
        return;
    }

    if let Err(error) = run() {
        eprintln!("bench_dupe: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let Some(command) = args.next() else {
        return Err(usage());
    };
    let rest: Vec<String> = args.collect();
    match command.as_str() {
        "scan" => run_scan(&rest),
        "pdf-selfcheck" => run_pdf_selfcheck(&rest),
        "pairs" => run_pairs(&rest),
        "synth" => run_synth(&rest),
        "report" => run_report(&rest),
        "-h" | "--help" | "help" => {
            println!("{}", usage());
            Ok(())
        }
        _ => Err(format!("unknown command {command:?}\n{}", usage())),
    }
}

fn usage() -> String {
    format!(
        "Usage:\n  bench_dupe scan --dir DIR [--recursive] --out FILE\n  \
         bench_dupe pdf-selfcheck --dir DIR [--recursive] --out FILE \
         [--limit-books N] [--max-pages-per-book N]\n  \
         bench_dupe pairs --in FILE --out FILE [--max-pairs N] [--loose | BIN OPTIONS]\n  \
         bench_dupe synth --dir DIR --out FILE [--recursive] [--limit N] \
         [--large-diff-threshold-bin N]\n  \
         bench_dupe report --synth FILE [--pairs FILE] --out FILE\n\n\
         Pair BIN OPTIONS (the union of every supplied bin is emitted):\n  \
         --pdq256-bin N --pdq64-bin N --phash63-bin N --blockhash256-bin N\n  \
         --luma-l1-bin X --luma-gain-offset-bin X --luma-grad-bin X\n  \
         --luma-large-diff-area-bin X --aspect-ratio-delta-bin X\n  \
         --large-diff-threshold-bin N\n\n\
         With no pair bins, or with --loose, every pair is emitted. No option is\n  \
         an accept/reject rule; all are measurement bins. The compiled default\n  \
         large-difference pixel bin is {DEFAULT_LARGE_DIFF_THRESHOLD_BIN}. Filtered mode requires\n  \
         one bin for every bit algorithm and at least one Luma32 metric."
    )
}

fn take_value(args: &[String], index: &mut usize, flag: &str) -> Result<String> {
    *index += 1;
    args.get(*index)
        .cloned()
        .ok_or_else(|| format!("{flag} requires a value"))
}

fn parse_usize(value: &str, flag: &str) -> Result<usize> {
    value
        .parse()
        .map_err(|_| format!("invalid {flag} value {value:?}"))
}

fn parse_u32(value: &str, flag: &str) -> Result<u32> {
    value
        .parse()
        .map_err(|_| format!("invalid {flag} value {value:?}"))
}

fn parse_u8(value: &str, flag: &str) -> Result<u8> {
    value
        .parse()
        .map_err(|_| format!("invalid {flag} value {value:?}"))
}

fn parse_nonnegative_f32(value: &str, flag: &str) -> Result<f32> {
    let parsed: f32 = value
        .parse()
        .map_err(|_| format!("invalid {flag} value {value:?}"))?;
    if parsed.is_finite() && parsed >= 0.0 {
        Ok(parsed)
    } else {
        Err(format!("{flag} must be finite and non-negative"))
    }
}

fn run_scan(args: &[String]) -> Result<()> {
    let mut dir = None;
    let mut output = None;
    let mut recursive = false;
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--dir" => dir = Some(PathBuf::from(take_value(args, &mut index, "--dir")?)),
            "--out" => output = Some(PathBuf::from(take_value(args, &mut index, "--out")?)),
            "--recursive" => recursive = true,
            flag => return Err(format!("unknown scan option {flag:?}")),
        }
        index += 1;
    }
    let dir = dir.ok_or_else(|| "scan requires --dir".to_owned())?;
    let output = output.ok_or_else(|| "scan requires --out".to_owned())?;
    let paths = collect_image_paths(&dir, recursive)?;
    let results: Vec<Result<ScanRecord>> = paths.par_iter().map(|path| scan_one(path)).collect();
    let mut records = Vec::with_capacity(results.len());
    for result in results {
        records.push(result?);
    }
    write_jsonl(&output, &records)?;
    eprintln!(
        "wrote {} image records to {}",
        records.len(),
        output.display()
    );
    Ok(())
}

/// Picks `limit` paths spread evenly across `paths` instead of the first `limit`.
///
/// Collection order is directory order, so a prefix of a recursive walk is every
/// page of the first few works rather than a sample of the library, and a distance
/// distribution measured from it would describe those works instead of the corpus.
/// Selection is by integer stride, so one input always yields the same sample.
fn stride_sample(paths: Vec<PathBuf>, limit: usize) -> Vec<PathBuf> {
    if limit == 0 || paths.len() <= limit {
        return paths;
    }
    let total = paths.len();
    (0..limit)
        .map(|slot| paths[slot * total / limit].clone())
        .collect()
}

fn stride_sample_page_numbers(page_numbers: Vec<u32>, limit: usize) -> Vec<u32> {
    if limit == 0 || page_numbers.len() <= limit {
        return page_numbers;
    }
    let total = page_numbers.len();
    (0..limit)
        .map(|slot| page_numbers[slot * total / limit])
        .collect()
}

#[derive(Default)]
struct PdfSelfcheckStats {
    selected_books: usize,
    measured_books: usize,
    measured_pages: usize,
    password_required_books: usize,
    zero_page_books: usize,
    failed_books: usize,
    failed_pages: usize,
    records: usize,
}

struct RenderedPageSignatures {
    rgba: RgbaImage,
    dims: (u32, u32),
    signatures: PreparedSignatures,
}

fn run_pdf_selfcheck(args: &[String]) -> Result<()> {
    let mut dir = None;
    let mut output = None;
    let mut recursive = false;
    let mut limit_books = None;
    let mut max_pages_per_book = None;
    let mut index = 0;
    while index < args.len() {
        let flag = args[index].as_str();
        match flag {
            "--dir" => dir = Some(PathBuf::from(take_value(args, &mut index, flag)?)),
            "--out" => output = Some(PathBuf::from(take_value(args, &mut index, flag)?)),
            "--recursive" => recursive = true,
            "--limit-books" => {
                limit_books = Some(parse_usize(&take_value(args, &mut index, flag)?, flag)?)
            }
            "--max-pages-per-book" => {
                max_pages_per_book = Some(parse_usize(&take_value(args, &mut index, flag)?, flag)?)
            }
            _ => return Err(format!("unknown pdf-selfcheck option {flag:?}")),
        }
        index += 1;
    }

    let dir = dir.ok_or_else(|| "pdf-selfcheck requires --dir".to_owned())?;
    let output = output.ok_or_else(|| "pdf-selfcheck requires --out".to_owned())?;
    let mut paths = collect_pdf_paths(&dir, recursive)?;
    if let Some(limit) = limit_books {
        paths = stride_sample(paths, limit);
    }

    let file =
        File::create(&output).map_err(|error| format!("create {}: {error}", output.display()))?;
    let mut writer = BufWriter::new(file);
    let mut stats = PdfSelfcheckStats {
        selected_books: paths.len(),
        ..PdfSelfcheckStats::default()
    };

    for path in paths {
        let entries = match pdf_loader::enumerate_pages(&path, None) {
            Ok(entries) => entries,
            Err(error) if pdf_password_required(&error) => {
                stats.password_required_books += 1;
                eprintln!(
                    "pdf-selfcheck: skip password-required PDF {}: {error}",
                    path.display()
                );
                continue;
            }
            Err(error) => {
                stats.failed_books += 1;
                eprintln!(
                    "pdf-selfcheck: skip unreadable PDF {}: {error}",
                    path.display()
                );
                continue;
            }
        };
        if entries.is_empty() {
            stats.zero_page_books += 1;
            eprintln!("pdf-selfcheck: skip zero-page PDF {}", path.display());
            continue;
        }

        let page_numbers = entries
            .into_iter()
            .map(|entry| entry.page_num)
            .collect::<Vec<_>>();
        let page_numbers = max_pages_per_book
            .map(|limit| stride_sample_page_numbers(page_numbers.clone(), limit))
            .unwrap_or(page_numbers);
        let mut measured_this_book = false;
        for page_num in page_numbers {
            match selfcheck_pdf_page(&mut writer, &path, page_num) {
                Ok(record_count) => {
                    measured_this_book = true;
                    stats.measured_pages += 1;
                    stats.records += record_count;
                }
                Err(error) => {
                    stats.failed_pages += 1;
                    eprintln!(
                        "pdf-selfcheck: skip page {} of {}: {error}",
                        page_num + 1,
                        path.display()
                    );
                }
            }
        }
        if measured_this_book {
            stats.measured_books += 1;
        }
    }

    writer
        .flush()
        .map_err(|error| format!("flush {}: {error}", output.display()))?;
    eprintln!(
        "pdf-selfcheck: selected_books={} measured_books={} measured_pages={} \
         password_required_books={} zero_page_books={} failed_books={} failed_pages={} records={} out={}",
        stats.selected_books,
        stats.measured_books,
        stats.measured_pages,
        stats.password_required_books,
        stats.zero_page_books,
        stats.failed_books,
        stats.failed_pages,
        stats.records,
        output.display()
    );
    Ok(())
}

fn collect_image_paths(dir: &Path, recursive: bool) -> Result<Vec<PathBuf>> {
    if !dir.is_dir() {
        return Err(format!("not a directory: {}", dir.display()));
    }
    let mut paths = Vec::new();
    collect_image_paths_inner(dir, recursive, &mut paths)?;
    paths.sort();
    Ok(paths)
}

fn collect_image_paths_inner(dir: &Path, recursive: bool, paths: &mut Vec<PathBuf>) -> Result<()> {
    let entries = std::fs::read_dir(dir)
        .map_err(|error| format!("read directory {}: {error}", dir.display()))?;
    let mut entries = entries
        .collect::<std::io::Result<Vec<_>>>()
        .map_err(|error| format!("read directory entry in {}: {error}", dir.display()))?;
    entries.sort_by_key(|entry| entry.path());
    for entry in entries {
        let file_type = entry
            .file_type()
            .map_err(|error| format!("read file type {}: {error}", entry.path().display()))?;
        let path = entry.path();
        if file_type.is_dir() {
            if recursive {
                collect_image_paths_inner(&path, true, paths)?;
            }
        } else if file_type.is_file() && is_supported_image(&path) && !is_apple_double(&path) {
            paths.push(path);
        }
    }
    Ok(())
}

fn collect_pdf_paths(dir: &Path, recursive: bool) -> Result<Vec<PathBuf>> {
    if !dir.is_dir() {
        return Err(format!("not a directory: {}", dir.display()));
    }
    let mut paths = Vec::new();
    collect_pdf_paths_inner(dir, recursive, &mut paths)?;
    paths.sort();
    Ok(paths)
}

fn collect_pdf_paths_inner(dir: &Path, recursive: bool, paths: &mut Vec<PathBuf>) -> Result<()> {
    let entries = std::fs::read_dir(dir)
        .map_err(|error| format!("read directory {}: {error}", dir.display()))?;
    let mut entries = entries
        .collect::<std::io::Result<Vec<_>>>()
        .map_err(|error| format!("read directory entry in {}: {error}", dir.display()))?;
    entries.sort_by_key(|entry| entry.path());
    for entry in entries {
        let file_type = entry
            .file_type()
            .map_err(|error| format!("read file type {}: {error}", entry.path().display()))?;
        let path = entry.path();
        if file_type.is_dir() {
            if recursive {
                collect_pdf_paths_inner(&path, true, paths)?;
            }
        } else if file_type.is_file() && extension_lower(&path) == "pdf" {
            paths.push(path);
        }
    }
    Ok(())
}

fn is_supported_image(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .map(|extension| extension.to_ascii_lowercase())
        .is_some_and(|extension| SUPPORTED_EXTENSIONS.contains(&extension.as_str()))
}

fn scan_one(path: &Path) -> Result<ScanRecord> {
    let total_start = Instant::now();
    let file_size = std::fs::metadata(path)
        .map_err(|error| format!("metadata {}: {error}", path.display()))?
        .len();
    let bytes =
        std::fs::read(path).map_err(|error| format!("read image {}: {error}", path.display()))?;

    let decode_start = Instant::now();
    let decoded = decode_for_scan(path, &bytes)?;
    let decode_ms = elapsed_ms(decode_start);
    let decoded_dims = decoded.rgba.dimensions();

    let signature_start = Instant::now();
    let mut proxy = dupe::proxy::build(
        decoded.rgba.as_raw(),
        decoded.rgba.width(),
        decoded.rgba.height(),
    );
    // A JPEG DCT-scaled decode contains fewer pixels than the source. The
    // canonical samples come from that explicit decode, while the Proxy
    // metadata continues to describe the EXIF-oriented source dimensions.
    proxy.src_width = decoded.source_dims.0;
    proxy.src_height = decoded.source_dims.1;
    let signatures = dupe::all_algos()
        .iter()
        .map(|&algo| store_signature(dupe::compute(algo, &proxy)))
        .collect();
    let signature_ms = elapsed_ms(signature_start);

    Ok(ScanRecord {
        schema_version: SCHEMA_VERSION,
        proxy_version: dupe::PROXY_VERSION,
        path: path.to_path_buf(),
        width: decoded.source_dims.0,
        height: decoded.source_dims.1,
        file_size,
        extension: extension_lower(path),
        decode: DecodeInfo {
            method: decoded.method,
            scale_num: decoded.scale_num,
            scale_den: decoded.scale_den,
            decoded_width: decoded_dims.0,
            decoded_height: decoded_dims.1,
            note: decoded.note,
        },
        decode_ms,
        signature_ms,
        total_ms: elapsed_ms(total_start),
        signatures,
    })
}

fn decode_for_scan(path: &Path, bytes: &[u8]) -> Result<DecodedImage> {
    if matches!(extension_lower(path).as_str(), "jpg" | "jpeg") {
        match decode_jpeg_turbo_scaled_from_bytes(bytes, JPEG_DCT_TARGET_EDGE) {
            Ok((image, stats)) => {
                let orientation = read_exif_orientation_for_source_dims(bytes);
                let image = apply_exif_orientation_from_bytes(image, bytes);
                let source_dims = stats.source_dims_after_exif(orientation);
                return Ok(DecodedImage {
                    rgba: image.to_rgba8(),
                    source_dims,
                    method: "turbojpeg_dct".to_owned(),
                    scale_num: stats.scale_num,
                    scale_den: 8,
                    note: None,
                });
            }
            Err(DctDecodeError::TerminalRejection(error)) => {
                return Err(format!(
                    "terminal JPEG rejection for {}: {error}",
                    path.display()
                ));
            }
            Err(DctDecodeError::Fallback(turbo_error)) => {
                let (image, method, fallback_note) = decode_full(path, bytes)?;
                let image = apply_exif_orientation(image, path);
                let source_dims = image.dimensions();
                return Ok(DecodedImage {
                    rgba: image.to_rgba8(),
                    source_dims,
                    method,
                    scale_num: 8,
                    scale_den: 8,
                    note: Some(format!(
                        "TurboJPEG fallback: {turbo_error}; {fallback_note}"
                    )),
                });
            }
        }
    }

    let (image, method, note) = decode_full(path, bytes)?;
    let image = apply_exif_orientation(image, path);
    let source_dims = image.dimensions();
    Ok(DecodedImage {
        rgba: image.to_rgba8(),
        source_dims,
        method,
        scale_num: 1,
        scale_den: 1,
        note: (note != "direct decode").then_some(note),
    })
}

fn selfcheck_pdf_page<W: Write>(writer: &mut W, path: &Path, page_num: u32) -> Result<usize> {
    let render_512 = render_pdf_page_signatures(path, page_num, 512)?;
    let render_1024 = render_pdf_page_signatures(path, page_num, 1024)?;
    let render_2048 = render_pdf_page_signatures(path, page_num, 2048)?;
    let jpeg_bytes = encode_jpeg(&render_1024.rgba, 95)?;
    let jpeg_decoded = decode_for_scan(Path::new("pdf-selfcheck.jpg"), &jpeg_bytes)?;
    let jpeg_dims = jpeg_decoded.source_dims;
    let jpeg_signatures = signatures_from_rgba(&jpeg_decoded.rgba, jpeg_dims);
    let source_path = PathBuf::from(format!("{}::page_{}", path.display(), page_num));
    let mut records = 0;

    for (transformation, left, right) in [
        ("pdf_render_512_vs_1024", &render_512, &render_1024),
        ("pdf_render_1024_vs_2048", &render_1024, &render_2048),
        ("pdf_render_512_vs_2048", &render_512, &render_2048),
    ] {
        records += write_synth_comparison(
            writer,
            "related",
            transformation,
            &source_path,
            None,
            left.dims,
            right.dims,
            &left.signatures,
            &right.signatures,
            DEFAULT_LARGE_DIFF_THRESHOLD_BIN,
        )?;
    }

    records += write_synth_comparison(
        writer,
        "related",
        "pdf_render_1024_vs_jpeg_q95_scan_path",
        &source_path,
        None,
        render_1024.dims,
        jpeg_dims,
        &render_1024.signatures,
        &jpeg_signatures,
        DEFAULT_LARGE_DIFF_THRESHOLD_BIN,
    )?;
    Ok(records)
}

fn render_pdf_page_signatures(
    path: &Path,
    page_num: u32,
    long_edge: u32,
) -> Result<RenderedPageSignatures> {
    let rendered = pdf_loader::render_page(
        path,
        page_num,
        long_edge,
        None,
        None,
        JobPriority::Normal,
        0,
        CancelWaitPolicy::AbortOnCancel,
    )
    .map_err(|error| {
        format!(
            "render {} page {} at long edge {long_edge}: {error}",
            path.display(),
            page_num + 1
        )
    })?;
    let rgba = rendered.image.to_rgba8();
    let dims = rgba.dimensions();
    let signatures = signatures_from_rgba(&rgba, dims);
    Ok(RenderedPageSignatures {
        rgba,
        dims,
        signatures,
    })
}

fn pdf_password_required(error: &std::io::Error) -> bool {
    error.kind() == std::io::ErrorKind::PermissionDenied
        || error.to_string().to_ascii_lowercase().contains("password")
}

fn decode_full(path: &Path, bytes: &[u8]) -> Result<(DynamicImage, String, String)> {
    match image::load_from_memory(bytes) {
        Ok(image) => Ok((image, "image_full".to_owned(), "direct decode".to_owned())),
        Err(image_error) => match mimageviewer::wic_decoder::decode_to_dynamic_image(path) {
            Some(image) => Ok((
                image,
                "wic_full".to_owned(),
                format!("image crate failed: {image_error}"),
            )),
            None => Err(format!(
                "decode {}: image crate failed ({image_error}); WIC failed",
                path.display()
            )),
        },
    }
}

fn read_exif_orientation_for_source_dims(bytes: &[u8]) -> u16 {
    rexif::parse_buffer(bytes)
        .ok()
        .and_then(|exif| {
            exif.entries
                .iter()
                .find(|entry| entry.ifd.tag == 274)
                .and_then(|entry| {
                    entry
                        .value
                        .to_i64(0)
                        .and_then(|value| u16::try_from(value).ok())
                        .filter(|value| (1..=8).contains(value))
                        .or_else(|| {
                            entry
                                .value_more_readable
                                .trim()
                                .parse::<u16>()
                                .ok()
                                .filter(|value| (1..=8).contains(value))
                        })
                        .or_else(|| orientation_from_text(&entry.value_more_readable))
                })
        })
        .unwrap_or(1)
}

fn orientation_from_text(text: &str) -> Option<u16> {
    let text = text.to_ascii_lowercase();
    if text.contains("straight") || text.contains("normal") {
        Some(1)
    } else if text.contains("rotated to left") || text.contains("90 cw") {
        Some(6)
    } else if text.contains("upside down") || text.contains("180") {
        Some(3)
    } else if text.contains("rotated to right")
        || text.contains("270 cw")
        || text.contains("90 ccw")
    {
        Some(8)
    } else if text.contains("mirrored horizontally") {
        Some(2)
    } else if text.contains("mirrored vertically") {
        Some(4)
    } else {
        None
    }
}

fn store_signature(signature: Signature) -> StoredSignature {
    let (kind, bytes, bit_width, value_count) = match signature.sig {
        Sig::Bits(bytes) => (
            "bits".to_owned(),
            bytes,
            Some(match signature.algo {
                Algo::Pdq256 | Algo::Blockhash256 => 256,
                Algo::Pdq64 => 64,
                Algo::Phash63 => 63,
                Algo::Luma32 => unreachable!("Luma32 cannot contain Bits"),
            }),
            None,
        ),
        Sig::Luma(bytes) => ("luma".to_owned(), bytes, None, Some(32 * 32)),
    };
    StoredSignature {
        algo: algo_name(signature.algo).to_owned(),
        kind,
        hex: encode_hex(&bytes),
        bit_width,
        value_count,
        quality: signature.quality,
    }
}

fn elapsed_ms(start: Instant) -> f64 {
    start.elapsed().as_secs_f64() * 1_000.0
}

fn extension_lower(path: &Path) -> String {
    path.extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase()
}

fn run_pairs(args: &[String]) -> Result<()> {
    let mut input = None;
    let mut output = None;
    let mut max_pairs = None;
    let mut loose = false;
    let mut bins = CandidateBins::exhaustive(DEFAULT_LARGE_DIFF_THRESHOLD_BIN);
    let mut index = 0;
    while index < args.len() {
        let flag = args[index].as_str();
        match flag {
            "--in" => input = Some(PathBuf::from(take_value(args, &mut index, flag)?)),
            "--out" => output = Some(PathBuf::from(take_value(args, &mut index, flag)?)),
            "--max-pairs" => {
                max_pairs = Some(parse_usize(&take_value(args, &mut index, flag)?, flag)?)
            }
            "--loose" => loose = true,
            "--pdq256-bin" => {
                bins.pdq256 = Some(parse_bounded_u32(
                    &take_value(args, &mut index, flag)?,
                    flag,
                    256,
                )?);
                bins.exhaustive = false;
            }
            "--pdq64-bin" => {
                bins.pdq64 = Some(parse_bounded_u32(
                    &take_value(args, &mut index, flag)?,
                    flag,
                    64,
                )?);
                bins.exhaustive = false;
            }
            "--phash63-bin" => {
                bins.phash63 = Some(parse_bounded_u32(
                    &take_value(args, &mut index, flag)?,
                    flag,
                    63,
                )?);
                bins.exhaustive = false;
            }
            "--blockhash256-bin" => {
                bins.blockhash256 = Some(parse_bounded_u32(
                    &take_value(args, &mut index, flag)?,
                    flag,
                    256,
                )?);
                bins.exhaustive = false;
            }
            "--luma-l1-bin" => {
                bins.luma_l1 = Some(parse_nonnegative_f32(
                    &take_value(args, &mut index, flag)?,
                    flag,
                )?);
                bins.exhaustive = false;
            }
            "--luma-gain-offset-bin" => {
                bins.luma_gain_offset = Some(parse_nonnegative_f32(
                    &take_value(args, &mut index, flag)?,
                    flag,
                )?);
                bins.exhaustive = false;
            }
            "--luma-grad-bin" => {
                bins.luma_grad = Some(parse_nonnegative_f32(
                    &take_value(args, &mut index, flag)?,
                    flag,
                )?);
                bins.exhaustive = false;
            }
            "--luma-large-diff-area-bin" => {
                let value = parse_nonnegative_f32(&take_value(args, &mut index, flag)?, flag)?;
                if value > 1.0 {
                    return Err(format!("{flag} must be in the metric domain 0..=1"));
                }
                bins.luma_large_diff_area = Some(value);
                bins.exhaustive = false;
            }
            "--aspect-ratio-delta-bin" => {
                bins.aspect_ratio_delta = Some(parse_nonnegative_f32(
                    &take_value(args, &mut index, flag)?,
                    flag,
                )?);
                bins.exhaustive = false;
            }
            "--large-diff-threshold-bin" => {
                bins.large_diff_threshold_bin =
                    parse_u8(&take_value(args, &mut index, flag)?, flag)?;
            }
            _ => return Err(format!("unknown pairs option {flag:?}")),
        }
        index += 1;
    }
    if loose && bins.has_selected_bin() {
        return Err("--loose cannot be combined with a candidate bin".to_owned());
    }
    if loose {
        bins = CandidateBins::exhaustive(bins.large_diff_threshold_bin);
    }
    if !bins.exhaustive {
        validate_all_algorithms_have_candidate_bins(&bins)?;
    }

    let input = input.ok_or_else(|| "pairs requires --in".to_owned())?;
    let output = output.ok_or_else(|| "pairs requires --out".to_owned())?;
    let scan_records: Vec<ScanRecord> = read_jsonl(&input)?;
    let prepared: Vec<PreparedScan> = scan_records
        .iter()
        .map(prepare_scan_record)
        .collect::<Result<_>>()?;
    let file =
        File::create(&output).map_err(|error| format!("create {}: {error}", output.display()))?;
    let mut writer = BufWriter::new(file);
    let mut emitted = 0usize;

    'outer: for left in 0..prepared.len() {
        for right in left + 1..prepared.len() {
            if max_pairs.is_some_and(|maximum| emitted >= maximum) {
                break 'outer;
            }
            let a = &prepared[left];
            let b = &prepared[right];
            let distances = pair_distances(
                &a.signatures,
                &b.signatures,
                a.dims,
                b.dims,
                bins.large_diff_threshold_bin,
            )?;
            let candidate_by = candidate_reasons(&distances, &bins);
            if candidate_by.is_empty() {
                continue;
            }
            let record = PairRecord {
                schema_version: SCHEMA_VERSION,
                path_a: a.path.clone(),
                path_b: b.path.clone(),
                dims_a: a.dims,
                dims_b: b.dims,
                distances,
                candidate_by,
                bins: bins.clone(),
            };
            write_json_line(&mut writer, &record)?;
            emitted += 1;
        }
    }
    writer
        .flush()
        .map_err(|error| format!("flush {}: {error}", output.display()))?;
    eprintln!("wrote {emitted} candidate pairs to {}", output.display());
    Ok(())
}

fn validate_all_algorithms_have_candidate_bins(bins: &CandidateBins) -> Result<()> {
    let mut missing = Vec::new();
    if bins.pdq256.is_none() {
        missing.push("Pdq256");
    }
    if bins.pdq64.is_none() {
        missing.push("Pdq64");
    }
    if bins.phash63.is_none() {
        missing.push("Phash63");
    }
    if bins.blockhash256.is_none() {
        missing.push("Blockhash256");
    }
    if bins.luma_l1.is_none()
        && bins.luma_gain_offset.is_none()
        && bins.luma_grad.is_none()
        && bins.luma_large_diff_area.is_none()
        && bins.aspect_ratio_delta.is_none()
    {
        missing.push("Luma32");
    }
    if missing.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "a filtered candidate pool must include every algorithm; missing bins for {}",
            missing.join(", ")
        ))
    }
}

fn parse_bounded_u32(value: &str, flag: &str, maximum: u32) -> Result<u32> {
    let parsed = parse_u32(value, flag)?;
    if parsed <= maximum {
        Ok(parsed)
    } else {
        Err(format!("{flag} must be in the metric domain 0..={maximum}"))
    }
}

fn prepare_scan_record(record: &ScanRecord) -> Result<PreparedScan> {
    if record.schema_version != SCHEMA_VERSION {
        return Err(format!(
            "{} has schema version {}, expected {}",
            record.path.display(),
            record.schema_version,
            SCHEMA_VERSION
        ));
    }
    if record.proxy_version != dupe::PROXY_VERSION {
        return Err(format!(
            "{} has proxy version {}, expected {}",
            record.path.display(),
            record.proxy_version,
            dupe::PROXY_VERSION
        ));
    }
    Ok(PreparedScan {
        path: record.path.clone(),
        dims: (record.width, record.height),
        signatures: PreparedSignatures {
            pdq256: restore_signature(record, Algo::Pdq256)?,
            pdq64: restore_signature(record, Algo::Pdq64)?,
            phash63: restore_signature(record, Algo::Phash63)?,
            blockhash256: restore_signature(record, Algo::Blockhash256)?,
            luma32: restore_signature(record, Algo::Luma32)?,
        },
    })
}

fn restore_signature(record: &ScanRecord, algo: Algo) -> Result<Signature> {
    let name = algo_name(algo);
    let matches: Vec<_> = record
        .signatures
        .iter()
        .filter(|signature| signature.algo == name)
        .collect();
    if matches.len() != 1 {
        return Err(format!(
            "{} contains {} {name} signatures, expected exactly one",
            record.path.display(),
            matches.len()
        ));
    }
    let stored = matches[0];
    let bytes = decode_hex(&stored.hex)?;
    let sig = match algo {
        Algo::Pdq256 | Algo::Blockhash256 => {
            validate_stored_shape(stored, "bits", Some(256), None, 32)?;
            Sig::Bits(bytes.into_boxed_slice())
        }
        Algo::Pdq64 => {
            validate_stored_shape(stored, "bits", Some(64), None, 8)?;
            Sig::Bits(bytes.into_boxed_slice())
        }
        Algo::Phash63 => {
            validate_stored_shape(stored, "bits", Some(63), None, 8)?;
            if bytes[0] & 0x80 != 0 {
                return Err(format!("{} has a 64th Phash63 bit", record.path.display()));
            }
            Sig::Bits(bytes.into_boxed_slice())
        }
        Algo::Luma32 => {
            validate_stored_shape(stored, "luma", None, Some(32 * 32), 32 * 32)?;
            Sig::Luma(bytes.into_boxed_slice())
        }
    };
    Ok(Signature {
        algo,
        sig,
        quality: stored.quality,
    })
}

fn validate_stored_shape(
    stored: &StoredSignature,
    kind: &str,
    bit_width: Option<u32>,
    value_count: Option<u32>,
    byte_len: usize,
) -> Result<()> {
    if stored.kind != kind
        || stored.bit_width != bit_width
        || stored.value_count != value_count
        || stored.hex.len() != byte_len * 2
    {
        return Err(format!("invalid stored shape for {}", stored.algo));
    }
    Ok(())
}

fn pair_distances(
    a: &PreparedSignatures,
    b: &PreparedSignatures,
    dims_a: (u32, u32),
    dims_b: (u32, u32),
    large_diff_threshold_bin: u8,
) -> Result<PairDistances> {
    Ok(PairDistances {
        pdq256: required_hamming(&a.pdq256.sig, &b.pdq256.sig, "Pdq256")?,
        pdq64: required_hamming(&a.pdq64.sig, &b.pdq64.sig, "Pdq64")?,
        phash63: required_hamming(&a.phash63.sig, &b.phash63.sig, "Phash63")?,
        blockhash256: required_hamming(&a.blockhash256.sig, &b.blockhash256.sig, "Blockhash256")?,
        luma32: dupe::luma_metrics(
            &a.luma32.sig,
            &b.luma32.sig,
            dims_a,
            dims_b,
            large_diff_threshold_bin,
        )
        .ok_or_else(|| "Luma32 signature types or dimensions do not match".to_owned())?
        .into(),
    })
}

fn required_hamming(a: &Sig, b: &Sig, name: &str) -> Result<u32> {
    dupe::hamming(a, b).ok_or_else(|| format!("{name} signature types or widths do not match"))
}

fn candidate_reasons(distances: &PairDistances, bins: &CandidateBins) -> Vec<String> {
    if bins.exhaustive {
        return vec!["exhaustive".to_owned()];
    }
    let mut reasons = Vec::new();
    push_if_within(&mut reasons, "Pdq256", distances.pdq256, bins.pdq256);
    push_if_within(&mut reasons, "Pdq64", distances.pdq64, bins.pdq64);
    push_if_within(&mut reasons, "Phash63", distances.phash63, bins.phash63);
    push_if_within(
        &mut reasons,
        "Blockhash256",
        distances.blockhash256,
        bins.blockhash256,
    );
    push_if_within_f32(&mut reasons, "Luma32.l1", distances.luma32.l1, bins.luma_l1);
    push_if_within_f32(
        &mut reasons,
        "Luma32.l1_gain_offset",
        distances.luma32.l1_gain_offset,
        bins.luma_gain_offset,
    );
    push_if_within_f32(
        &mut reasons,
        "Luma32.grad_l1",
        distances.luma32.grad_l1,
        bins.luma_grad,
    );
    push_if_within_f32(
        &mut reasons,
        "Luma32.large_diff_area",
        distances.luma32.large_diff_area,
        bins.luma_large_diff_area,
    );
    push_if_within_f32(
        &mut reasons,
        "Luma32.aspect_ratio_delta",
        distances.luma32.aspect_ratio_delta,
        bins.aspect_ratio_delta,
    );
    reasons
}

fn push_if_within(reasons: &mut Vec<String>, name: &str, value: u32, bin: Option<u32>) {
    if bin.is_some_and(|maximum| value <= maximum) {
        reasons.push(name.to_owned());
    }
}

fn push_if_within_f32(reasons: &mut Vec<String>, name: &str, value: f32, bin: Option<f32>) {
    if bin.is_some_and(|maximum| value <= maximum) {
        reasons.push(name.to_owned());
    }
}

fn run_synth(args: &[String]) -> Result<()> {
    let mut dir = None;
    let mut output = None;
    let mut limit = None;
    let mut recursive = false;
    let mut large_diff_threshold_bin = DEFAULT_LARGE_DIFF_THRESHOLD_BIN;
    let mut index = 0;
    while index < args.len() {
        let flag = args[index].as_str();
        match flag {
            "--dir" => dir = Some(PathBuf::from(take_value(args, &mut index, flag)?)),
            "--out" => output = Some(PathBuf::from(take_value(args, &mut index, flag)?)),
            "--limit" => limit = Some(parse_usize(&take_value(args, &mut index, flag)?, flag)?),
            "--recursive" => recursive = true,
            "--large-diff-threshold-bin" => {
                large_diff_threshold_bin = parse_u8(&take_value(args, &mut index, flag)?, flag)?
            }
            _ => return Err(format!("unknown synth option {flag:?}")),
        }
        index += 1;
    }
    let dir = dir.ok_or_else(|| "synth requires --dir".to_owned())?;
    let output = output.ok_or_else(|| "synth requires --out".to_owned())?;
    let mut paths = collect_image_paths(&dir, recursive)?;
    if let Some(limit) = limit {
        paths = stride_sample(paths, limit);
    }

    let file =
        File::create(&output).map_err(|error| format!("create {}: {error}", output.display()))?;
    let mut writer = BufWriter::new(file);
    let mut originals = Vec::with_capacity(paths.len());
    let mut record_count = 0usize;

    for path in &paths {
        let bytes = std::fs::read(path)
            .map_err(|error| format!("read image {}: {error}", path.display()))?;
        let rgba = decode_full_oriented(path, &bytes)?;
        let dims = rgba.dimensions();
        let original_signatures = signatures_from_rgba(&rgba, dims);

        for (label, numerator) in [
            ("scale_50pct", 50),
            ("scale_75pct", 75),
            ("scale_25pct", 25),
        ] {
            let transformed = scale_percent(&rgba, numerator);
            record_count += emit_related_transform(
                &mut writer,
                path,
                dims,
                &original_signatures,
                label,
                &transformed,
                large_diff_threshold_bin,
            )?;
        }

        for quality in [95u8, 85, 70] {
            let transformed = jpeg_round_trip(&rgba, quality)?;
            record_count += emit_related_transform(
                &mut writer,
                path,
                dims,
                &original_signatures,
                &format!("jpeg_q{quality}"),
                &transformed,
                large_diff_threshold_bin,
            )?;
        }

        if extension_lower(path) == "png" {
            // The conversion reuses q=95 from the explicitly specified JPEG
            // recompression levels; it does not introduce another quality value.
            let transformed = jpeg_round_trip(&rgba, 95)?;
            record_count += emit_related_transform(
                &mut writer,
                path,
                dims,
                &original_signatures,
                "png_to_jpeg_q95",
                &transformed,
                large_diff_threshold_bin,
            )?;
        }

        for (label, area_fraction) in [("logo_area_1pct", 0.01), ("logo_area_4pct", 0.04)] {
            let transformed = add_bottom_right_logo(&rgba, area_fraction);
            record_count += emit_related_transform(
                &mut writer,
                path,
                dims,
                &original_signatures,
                label,
                &transformed,
                large_diff_threshold_bin,
            )?;
        }

        let transformed = gain_and_offset(&rgba, 1.05, 8.0);
        record_count += emit_related_transform(
            &mut writer,
            path,
            dims,
            &original_signatures,
            "gain_1.05_offset_plus_8",
            &transformed,
            large_diff_threshold_bin,
        )?;

        originals.push(PreparedScan {
            path: path.clone(),
            dims,
            signatures: original_signatures,
        });
    }

    for left in 0..originals.len() {
        for right in left + 1..originals.len() {
            let a = &originals[left];
            let b = &originals[right];
            record_count += write_synth_comparison(
                &mut writer,
                "unrelated",
                "unrelated_pair",
                &a.path,
                Some(&b.path),
                a.dims,
                b.dims,
                &a.signatures,
                &b.signatures,
                large_diff_threshold_bin,
            )?;
        }
    }

    writer
        .flush()
        .map_err(|error| format!("flush {}: {error}", output.display()))?;
    eprintln!(
        "wrote {record_count} synthetic measurements to {}",
        output.display()
    );
    Ok(())
}

fn decode_full_oriented(path: &Path, bytes: &[u8]) -> Result<RgbaImage> {
    let image = if matches!(extension_lower(path).as_str(), "jpg" | "jpeg") {
        match decode_jpeg_turbo_scaled_from_bytes(bytes, u32::MAX) {
            Ok((image, _)) => apply_exif_orientation_from_bytes(image, bytes),
            Err(DctDecodeError::TerminalRejection(error)) => {
                return Err(format!(
                    "terminal JPEG rejection for {}: {error}",
                    path.display()
                ));
            }
            Err(DctDecodeError::Fallback(_)) => {
                let (image, _, _) = decode_full(path, bytes)?;
                apply_exif_orientation(image, path)
            }
        }
    } else {
        let (image, _, _) = decode_full(path, bytes)?;
        apply_exif_orientation(image, path)
    };
    Ok(image.to_rgba8())
}

fn signatures_from_rgba(rgba: &RgbaImage, source_dims: (u32, u32)) -> PreparedSignatures {
    let mut proxy = dupe::proxy::build(rgba.as_raw(), rgba.width(), rgba.height());
    proxy.src_width = source_dims.0;
    proxy.src_height = source_dims.1;
    PreparedSignatures::compute(&proxy)
}

fn emit_related_transform<W: Write>(
    writer: &mut W,
    source_path: &Path,
    original_dims: (u32, u32),
    original_signatures: &PreparedSignatures,
    transformation: &str,
    transformed: &RgbaImage,
    large_diff_threshold_bin: u8,
) -> Result<usize> {
    let transformed_dims = transformed.dimensions();
    let transformed_signatures = signatures_from_rgba(transformed, transformed_dims);
    write_synth_comparison(
        writer,
        "related",
        transformation,
        source_path,
        None,
        original_dims,
        transformed_dims,
        original_signatures,
        &transformed_signatures,
        large_diff_threshold_bin,
    )
}

#[allow(clippy::too_many_arguments)]
fn write_synth_comparison<W: Write>(
    writer: &mut W,
    relation: &str,
    transformation: &str,
    source_path: &Path,
    other_path: Option<&Path>,
    dims_a: (u32, u32),
    dims_b: (u32, u32),
    signatures_a: &PreparedSignatures,
    signatures_b: &PreparedSignatures,
    large_diff_threshold_bin: u8,
) -> Result<usize> {
    for &algo in dupe::all_algos() {
        let a = signatures_a.get(algo);
        let b = signatures_b.get(algo);
        let distance = if algo == Algo::Luma32 {
            let metrics =
                dupe::luma_metrics(&a.sig, &b.sig, dims_a, dims_b, large_diff_threshold_bin)
                    .ok_or_else(|| {
                        "Luma32 signature types or dimensions do not match".to_owned()
                    })?;
            SynthDistance::Luma {
                metrics: metrics.into(),
            }
        } else {
            SynthDistance::Hamming {
                value: required_hamming(&a.sig, &b.sig, algo_name(algo))?,
            }
        };
        let record = SynthRecord {
            schema_version: SCHEMA_VERSION,
            relation: relation.to_owned(),
            transformation: transformation.to_owned(),
            source_path: source_path.to_path_buf(),
            other_path: other_path.map(Path::to_path_buf),
            algo: algo_name(algo).to_owned(),
            dims_a,
            dims_b,
            quality_a: a.quality,
            quality_b: b.quality,
            large_diff_threshold_bin,
            distance,
        };
        write_json_line(writer, &record)?;
    }
    Ok(dupe::all_algos().len())
}

fn scale_percent(source: &RgbaImage, percent: u32) -> RgbaImage {
    let width = ((source.width() as u64 * percent as u64 + 50) / 100).max(1) as u32;
    let height = ((source.height() as u64 * percent as u64 + 50) / 100).max(1) as u32;
    image::imageops::resize(source, width, height, image::imageops::FilterType::Lanczos3)
}

fn encode_jpeg(source: &RgbaImage, quality: u8) -> Result<Vec<u8>> {
    let mut rgb = RgbImage::new(source.width(), source.height());
    for (target, pixel) in rgb.pixels_mut().zip(source.pixels()) {
        let alpha = pixel[3] as u16;
        let inverse = 255 - alpha;
        *target = Rgb([
            ((pixel[0] as u16 * alpha + 255 * inverse + 127) / 255) as u8,
            ((pixel[1] as u16 * alpha + 255 * inverse + 127) / 255) as u8,
            ((pixel[2] as u16 * alpha + 255 * inverse + 127) / 255) as u8,
        ]);
    }
    let mut encoded = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut encoded, quality)
        .encode_image(&DynamicImage::ImageRgb8(rgb))
        .map_err(|error| format!("encode synthetic JPEG q={quality}: {error}"))?;
    Ok(encoded)
}

fn jpeg_round_trip(source: &RgbaImage, quality: u8) -> Result<RgbaImage> {
    let encoded = encode_jpeg(source, quality)?;
    image::load_from_memory(&encoded)
        .map(|image| image.to_rgba8())
        .map_err(|error| format!("decode synthetic JPEG q={quality}: {error}"))
}

fn add_bottom_right_logo(source: &RgbaImage, area_fraction: f64) -> RgbaImage {
    let mut output = source.clone();
    let image_area = source.width() as f64 * source.height() as f64;
    let target_area = (image_area * area_fraction).round().max(1.0);
    let logo_width = (target_area.sqrt().round() as u32)
        .max(1)
        .min(source.width());
    let logo_height = ((target_area / logo_width as f64).ceil() as u32)
        .max(1)
        .min(source.height());
    let start_x = source.width() - logo_width;
    let start_y = source.height() - logo_height;
    for local_y in 0..logo_height {
        for local_x in 0..logo_width {
            let border = local_x == 0
                || local_y == 0
                || local_x + 1 == logo_width
                || local_y + 1 == logo_height;
            let diagonal =
                local_x as u64 * logo_height as u64 / logo_width.max(1) as u64 == local_y as u64;
            let value = if border || diagonal { 0 } else { 255 };
            output.put_pixel(
                start_x + local_x,
                start_y + local_y,
                Rgba([value, value, value, 255]),
            );
        }
    }
    output
}

fn gain_and_offset(source: &RgbaImage, gain: f32, offset: f32) -> RgbaImage {
    let mut output = source.clone();
    for pixel in output.pixels_mut() {
        for channel in &mut pixel.0[..3] {
            *channel = (*channel as f32 * gain + offset).round().clamp(0.0, 255.0) as u8;
        }
    }
    output
}

fn run_report(args: &[String]) -> Result<()> {
    let mut synth_path = None;
    let mut pairs_path = None;
    let mut output = None;
    let mut index = 0;
    while index < args.len() {
        let flag = args[index].as_str();
        match flag {
            "--synth" => synth_path = Some(PathBuf::from(take_value(args, &mut index, flag)?)),
            "--pairs" => pairs_path = Some(PathBuf::from(take_value(args, &mut index, flag)?)),
            "--out" => output = Some(PathBuf::from(take_value(args, &mut index, flag)?)),
            _ => return Err(format!("unknown report option {flag:?}")),
        }
        index += 1;
    }
    let synth_path = synth_path.ok_or_else(|| "report requires --synth".to_owned())?;
    let output = output.ok_or_else(|| "report requires --out".to_owned())?;
    let synth_records: Vec<SynthRecord> = read_jsonl(&synth_path)?;

    let mut related: BTreeMap<(String, String, String), Vec<f64>> = BTreeMap::new();
    let mut unrelated: BTreeMap<(String, String), Vec<f64>> = BTreeMap::new();
    let mut large_diff_bins = BTreeSet::new();
    for record in &synth_records {
        if record.schema_version != SCHEMA_VERSION {
            return Err(format!(
                "{} contains synth schema version {}, expected {}",
                synth_path.display(),
                record.schema_version,
                SCHEMA_VERSION
            ));
        }
        large_diff_bins.insert(record.large_diff_threshold_bin);
        for (metric, value) in synth_metric_values(record)? {
            if !value.is_finite() {
                return Err(format!(
                    "non-finite {}.{} measurement in {}",
                    record.algo,
                    metric,
                    synth_path.display()
                ));
            }
            match record.relation.as_str() {
                "related" => related
                    .entry((record.algo.clone(), metric, record.transformation.clone()))
                    .or_default()
                    .push(value),
                "unrelated" => unrelated
                    .entry((record.algo.clone(), metric))
                    .or_default()
                    .push(value),
                other => return Err(format!("unknown synth relation {other:?}")),
            }
        }
    }
    if large_diff_bins.len() > 1 {
        return Err(format!(
            "{} mixes large-difference pixel bins ({}); report each bin separately",
            synth_path.display(),
            format_u8_set(&large_diff_bins)
        ));
    }

    let pair_records = if let Some(path) = &pairs_path {
        let records: Vec<PairRecord> = read_jsonl(path)?;
        for record in &records {
            if record.schema_version != SCHEMA_VERSION {
                return Err(format!(
                    "{} contains pair schema version {}, expected {}",
                    path.display(),
                    record.schema_version,
                    SCHEMA_VERSION
                ));
            }
        }
        let distinct_bins: BTreeSet<_> = records
            .iter()
            .map(|record| {
                serde_json::to_string(&record.bins)
                    .map_err(|error| format!("serialize candidate bins: {error}"))
            })
            .collect::<Result<_>>()?;
        if distinct_bins.len() > 1 {
            return Err(format!(
                "{} mixes candidate-bin configurations; report each configuration separately",
                path.display()
            ));
        }
        Some(records)
    } else {
        None
    };

    let file =
        File::create(&output).map_err(|error| format!("create {}: {error}", output.display()))?;
    let mut writer = BufWriter::new(file);
    writeln!(writer, "# Duplicate signature measurement report").map_err(io_error)?;
    writeln!(writer).map_err(io_error)?;
    writeln!(
        writer,
        "> This report is measurement-only. It does not choose or recommend an accept/reject threshold."
    )
    .map_err(io_error)?;
    writeln!(writer).map_err(io_error)?;
    writeln!(writer, "## Measurement configuration").map_err(io_error)?;
    writeln!(writer).map_err(io_error)?;
    writeln!(
        writer,
        "- Compiled default large-difference pixel bin: `{DEFAULT_LARGE_DIFF_THRESHOLD_BIN}` (not a recommendation)"
    )
    .map_err(io_error)?;
    writeln!(
        writer,
        "- Large-difference pixel bins present in synth input: `{}`",
        format_u8_set(&large_diff_bins)
    )
    .map_err(io_error)?;
    writeln!(
        writer,
        "- Quantiles use nearest-rank order statistics; `p0.01` means the 0.01st percentile."
    )
    .map_err(io_error)?;

    writeln!(writer).map_err(io_error)?;
    writeln!(writer, "## Known-related transformations").map_err(io_error)?;
    writeln!(writer).map_err(io_error)?;
    writeln!(
        writer,
        "| Algorithm | Metric | Transformation | N | p50 | p90 | p99 | max | unrelated min | gap (unrelated min - related max) |"
    )
    .map_err(io_error)?;
    writeln!(writer, "|---|---|---|---:|---:|---:|---:|---:|---:|---:|").map_err(io_error)?;
    for ((algo, metric, transformation), values) in &related {
        let related_max = maximum(values);
        let unrelated_min = unrelated
            .get(&(algo.clone(), metric.clone()))
            .and_then(|values| minimum(values));
        let gap = unrelated_min.zip(related_max).map(|(u, r)| u - r);
        writeln!(
            writer,
            "| {algo} | {metric} | {transformation} | {} | {} | {} | {} | {} | {} | {} |",
            values.len(),
            format_value(metric, percentile(values, 50.0)),
            format_value(metric, percentile(values, 90.0)),
            format_value(metric, percentile(values, 99.0)),
            format_value(metric, related_max),
            format_value(metric, unrelated_min),
            format_signed_value(metric, gap),
        )
        .map_err(io_error)?;
    }

    writeln!(writer).map_err(io_error)?;
    writeln!(writer, "## Unrelated image pairs").map_err(io_error)?;
    writeln!(writer).map_err(io_error)?;
    writeln!(
        writer,
        "| Algorithm | Metric | N | p0.01 | p0.1 | p1 | min |"
    )
    .map_err(io_error)?;
    writeln!(writer, "|---|---|---:|---:|---:|---:|---:|").map_err(io_error)?;
    for ((algo, metric), values) in &unrelated {
        writeln!(
            writer,
            "| {algo} | {metric} | {} | {} | {} | {} | {} |",
            values.len(),
            format_value(metric, percentile(values, 0.01)),
            format_value(metric, percentile(values, 0.1)),
            format_value(metric, percentile(values, 1.0)),
            format_value(metric, minimum(values)),
        )
        .map_err(io_error)?;
    }

    if let Some(records) = pair_records.as_deref() {
        write_pair_report(&mut writer, records)?;
    }
    writer
        .flush()
        .map_err(|error| format!("flush {}: {error}", output.display()))?;
    eprintln!("wrote report to {}", output.display());
    Ok(())
}

fn synth_metric_values(record: &SynthRecord) -> Result<Vec<(String, f64)>> {
    match (&record.distance, record.algo.as_str()) {
        (SynthDistance::Hamming { value }, "Pdq256" | "Pdq64" | "Phash63" | "Blockhash256") => {
            Ok(vec![("hamming".to_owned(), *value as f64)])
        }
        (SynthDistance::Luma { metrics }, "Luma32") => Ok(vec![
            ("l1".to_owned(), metrics.l1 as f64),
            ("l1_gain_offset".to_owned(), metrics.l1_gain_offset as f64),
            ("grad_l1".to_owned(), metrics.grad_l1 as f64),
            ("large_diff_area".to_owned(), metrics.large_diff_area as f64),
            (
                "aspect_ratio_delta".to_owned(),
                metrics.aspect_ratio_delta as f64,
            ),
        ]),
        _ => Err(format!(
            "distance kind does not match algorithm {}",
            record.algo
        )),
    }
}

fn write_pair_report<W: Write>(writer: &mut W, records: &[PairRecord]) -> Result<()> {
    writeln!(writer).map_err(io_error)?;
    writeln!(writer, "## Candidate-pool measurements").map_err(io_error)?;
    writeln!(writer).map_err(io_error)?;
    writeln!(writer, "- Emitted pair count: `{}`", records.len()).map_err(io_error)?;

    let mut configurations = BTreeSet::new();
    let mut reasons: BTreeMap<String, usize> = BTreeMap::new();
    let mut values: BTreeMap<(String, String), Vec<f64>> = BTreeMap::new();
    for record in records {
        let configuration = serde_json::to_string(&record.bins)
            .map_err(|error| format!("serialize candidate bins: {error}"))?;
        configurations.insert(configuration);
        for reason in &record.candidate_by {
            *reasons.entry(reason.clone()).or_default() += 1;
        }
        append_pair_values(&mut values, &record.distances);
    }
    writeln!(
        writer,
        "- Distinct exact candidate-bin configurations present: `{}`",
        configurations.len()
    )
    .map_err(io_error)?;
    for configuration in configurations {
        writeln!(writer, "  - `{configuration}`").map_err(io_error)?;
    }
    if records.is_empty() {
        writeln!(
            writer,
            "- No pair record was available, so the JSONL contains no repeated bin configuration to report."
        )
        .map_err(io_error)?;
    }
    for (reason, count) in reasons {
        writeln!(writer, "- Candidate reason `{reason}`: `{count}` pairs").map_err(io_error)?;
    }

    writeln!(writer).map_err(io_error)?;
    writeln!(
        writer,
        "| Algorithm | Metric | N | min | p50 | p90 | p99 | max |"
    )
    .map_err(io_error)?;
    writeln!(writer, "|---|---|---:|---:|---:|---:|---:|---:|").map_err(io_error)?;
    for ((algo, metric), metric_values) in values {
        writeln!(
            writer,
            "| {algo} | {metric} | {} | {} | {} | {} | {} | {} |",
            metric_values.len(),
            format_value(&metric, minimum(&metric_values)),
            format_value(&metric, percentile(&metric_values, 50.0)),
            format_value(&metric, percentile(&metric_values, 90.0)),
            format_value(&metric, percentile(&metric_values, 99.0)),
            format_value(&metric, maximum(&metric_values)),
        )
        .map_err(io_error)?;
    }
    Ok(())
}

fn append_pair_values(
    values: &mut BTreeMap<(String, String), Vec<f64>>,
    distances: &PairDistances,
) {
    let mut push = |algo: &str, metric: &str, value: f64| {
        values
            .entry((algo.to_owned(), metric.to_owned()))
            .or_default()
            .push(value);
    };
    push("Pdq256", "hamming", distances.pdq256 as f64);
    push("Pdq64", "hamming", distances.pdq64 as f64);
    push("Phash63", "hamming", distances.phash63 as f64);
    push("Blockhash256", "hamming", distances.blockhash256 as f64);
    push("Luma32", "l1", distances.luma32.l1 as f64);
    push(
        "Luma32",
        "l1_gain_offset",
        distances.luma32.l1_gain_offset as f64,
    );
    push("Luma32", "grad_l1", distances.luma32.grad_l1 as f64);
    push(
        "Luma32",
        "large_diff_area",
        distances.luma32.large_diff_area as f64,
    );
    push(
        "Luma32",
        "aspect_ratio_delta",
        distances.luma32.aspect_ratio_delta as f64,
    );
}

fn percentile(values: &[f64], percentile: f64) -> Option<f64> {
    if values.is_empty() {
        return None;
    }
    let mut sorted = values.to_vec();
    sorted.sort_by(f64::total_cmp);
    let rank = ((percentile / 100.0) * sorted.len() as f64).ceil();
    let index = (rank.max(1.0) as usize - 1).min(sorted.len() - 1);
    Some(sorted[index])
}

fn minimum(values: &[f64]) -> Option<f64> {
    values.iter().copied().min_by(f64::total_cmp)
}

fn maximum(values: &[f64]) -> Option<f64> {
    values.iter().copied().max_by(f64::total_cmp)
}

fn format_value(metric: &str, value: Option<f64>) -> String {
    value.map_or_else(
        || "n/a".to_owned(),
        |value| {
            if metric == "hamming" {
                format!("{value:.0}")
            } else {
                format!("{value:.6}")
            }
        },
    )
}

fn format_signed_value(metric: &str, value: Option<f64>) -> String {
    value.map_or_else(
        || "n/a".to_owned(),
        |value| {
            if metric == "hamming" {
                format!("{value:+.0}")
            } else {
                format!("{value:+.6}")
            }
        },
    )
}

fn format_u8_set(values: &BTreeSet<u8>) -> String {
    if values.is_empty() {
        return "none".to_owned();
    }
    values
        .iter()
        .map(u8::to_string)
        .collect::<Vec<_>>()
        .join(", ")
}

fn algo_name(algo: Algo) -> &'static str {
    match algo {
        Algo::Pdq256 => "Pdq256",
        Algo::Pdq64 => "Pdq64",
        Algo::Phash63 => "Phash63",
        Algo::Blockhash256 => "Blockhash256",
        Algo::Luma32 => "Luma32",
    }
}

fn encode_hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut encoded = String::with_capacity(bytes.len() * 2);
    for &byte in bytes {
        encoded.push(DIGITS[(byte >> 4) as usize] as char);
        encoded.push(DIGITS[(byte & 0x0f) as usize] as char);
    }
    encoded
}

fn decode_hex(encoded: &str) -> Result<Vec<u8>> {
    if !encoded.len().is_multiple_of(2) {
        return Err("hex signature has odd length".to_owned());
    }
    let bytes = encoded.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len() / 2);
    for index in (0..bytes.len()).step_by(2) {
        let high = decode_hex_digit(bytes[index])?;
        let low = decode_hex_digit(bytes[index + 1])?;
        decoded.push((high << 4) | low);
    }
    Ok(decoded)
}

fn decode_hex_digit(digit: u8) -> Result<u8> {
    match digit {
        b'0'..=b'9' => Ok(digit - b'0'),
        b'a'..=b'f' => Ok(digit - b'a' + 10),
        b'A'..=b'F' => Ok(digit - b'A' + 10),
        _ => Err(format!("invalid hex digit {:?}", digit as char)),
    }
}

fn read_jsonl<T: DeserializeOwned>(path: &Path) -> Result<Vec<T>> {
    let file = File::open(path).map_err(|error| format!("open {}: {error}", path.display()))?;
    let reader = BufReader::new(file);
    let mut records = Vec::new();
    for (line_index, line) in reader.lines().enumerate() {
        let line = line
            .map_err(|error| format!("read {} line {}: {error}", path.display(), line_index + 1))?;
        if line.trim().is_empty() {
            continue;
        }
        let record = serde_json::from_str(&line).map_err(|error| {
            format!("parse {} line {}: {error}", path.display(), line_index + 1)
        })?;
        records.push(record);
    }
    Ok(records)
}

fn write_jsonl<T: Serialize>(path: &Path, records: &[T]) -> Result<()> {
    let file = File::create(path).map_err(|error| format!("create {}: {error}", path.display()))?;
    let mut writer = BufWriter::new(file);
    for record in records {
        write_json_line(&mut writer, record)?;
    }
    writer
        .flush()
        .map_err(|error| format!("flush {}: {error}", path.display()))
}

fn write_json_line<W: Write, T: Serialize>(writer: &mut W, record: &T) -> Result<()> {
    serde_json::to_writer(&mut *writer, record)
        .map_err(|error| format!("serialize JSONL record: {error}"))?;
    writer
        .write_all(b"\n")
        .map_err(|error| format!("write JSONL record: {error}"))
}

fn io_error(error: std::io::Error) -> String {
    error.to_string()
}
