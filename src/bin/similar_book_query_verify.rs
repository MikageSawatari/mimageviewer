//! Guarded independent correctness certificate for one real similar-book query.

use std::io::{BufReader, BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::thread;

use mimageviewer::similar_book_query_bench::{
    BenchInputFingerprint, BenchInputSpec, GuardedBenchInputs,
    lower_product_book_query_worker_priority,
};
use mimageviewer::similar_book_query_verify::{
    VerificationBudgets, VerificationReport, VerificationSpec, verify_real_book, write_json_bounded,
};
use serde::Serialize;
use sha2::{Digest, Sha256};

fn main() {
    if let Err(error) = run() {
        eprintln!("similar-book query verification failed: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    if cfg!(debug_assertions) || !cfg!(feature = "portable") {
        return Err(
            "real-book verification requires --release --features dev-tools,portable".to_owned(),
        );
    }
    let options = Options::parse()?;
    let mut guarded = GuardedBenchInputs::acquire(
        BenchInputSpec {
            db_path: options.db_path.clone(),
            active_base_paths: options.active_base_paths.clone(),
            retained_base_paths: options.retained_base_paths.clone(),
            immutable_roots: options.roots.clone(),
        },
        &options.output_path,
    )?;
    let initial_inputs = guarded.initial_fingerprints();
    let database_path = guarded.database_path().to_path_buf();
    let engine_config = guarded.engine_config();
    let spec = VerificationSpec {
        origin: options.origin.clone(),
        immutable_roots: options.roots.clone(),
        expected_result_sha256: options.expected_result_sha256.clone(),
        budgets: options.budgets,
    };
    let worker_result = thread::Builder::new()
        .name("similar-book-query-verify".to_owned())
        .spawn(move || {
            lower_product_book_query_worker_priority();
            verify_real_book(engine_config, &database_path, spec)
        })
        .map_err(|error| format!("verification worker spawn failed: {error}"))
        .and_then(|worker| {
            worker
                .join()
                .map_err(|_| "verification worker panicked".to_owned())?
        });

    // The worker owns and drops both product and oracle SQLite connections before returning.
    // Final input verification is mandatory even when spawn, query, oracle, or panic fails.
    let final_inputs_result = guarded.verify_unchanged();
    let mut failures = Vec::new();
    let report = match worker_result {
        Ok(report) => Some(report),
        Err(error) => {
            failures.push(error);
            None
        }
    };
    let final_inputs = match final_inputs_result {
        Ok(inputs) => Some(inputs),
        Err(error) => {
            failures.push(error);
            None
        }
    };
    if !failures.is_empty() {
        return Err(failures.join("; cleanup: "));
    }
    let report = report.expect("verification worker success was checked");
    let final_inputs = final_inputs.expect("final input verification was checked");
    let envelope = VerificationEnvelope {
        schema_version: 1,
        case_id: options.case_id,
        release: !cfg!(debug_assertions),
        portable: cfg!(feature = "portable"),
        dev_tools: cfg!(feature = "dev-tools"),
        binary: fingerprint_path(&std::env::current_exe().map_err(|error| error.to_string())?)?,
        initial_inputs,
        final_inputs,
        report,
    };
    let max_output_bytes = envelope.report.budgets.max_output_bytes;
    {
        let mut output = BufWriter::new(guarded.output_file());
        write_json_bounded(&mut output, &envelope, max_output_bytes)?;
        output.flush().map_err(|error| error.to_string())?;
    }
    guarded
        .output_file()
        .sync_all()
        .map_err(|error| format!("verification output sync failed: {error}"))?;
    Ok(())
}

#[derive(Debug)]
struct Options {
    case_id: String,
    db_path: PathBuf,
    active_base_paths: Vec<PathBuf>,
    retained_base_paths: Vec<PathBuf>,
    origin: String,
    roots: Vec<String>,
    output_path: PathBuf,
    expected_result_sha256: String,
    budgets: VerificationBudgets,
}

impl Options {
    fn parse() -> Result<Self, String> {
        let mut args = std::env::args_os().skip(1);
        let mut options = Self {
            case_id: String::new(),
            db_path: PathBuf::new(),
            active_base_paths: Vec::new(),
            retained_base_paths: Vec::new(),
            origin: String::new(),
            roots: Vec::new(),
            output_path: PathBuf::new(),
            expected_result_sha256: String::new(),
            budgets: VerificationBudgets::default(),
        };
        while let Some(argument) = args.next() {
            let flag = argument.to_string_lossy();
            match flag.as_ref() {
                "--case-id" => options.case_id = required_string(&mut args, "--case-id")?,
                "--db" => options.db_path = required_path(&mut args, "--db")?,
                "--base" => options
                    .active_base_paths
                    .push(required_path(&mut args, "--base")?),
                "--retained-base" => options
                    .retained_base_paths
                    .push(required_path(&mut args, "--retained-base")?),
                "--origin" => options.origin = required_string(&mut args, "--origin")?,
                "--root" => options.roots.push(required_string(&mut args, "--root")?),
                "--output" => options.output_path = required_path(&mut args, "--output")?,
                "--expect-digest" => {
                    options.expected_result_sha256 = required_string(&mut args, "--expect-digest")?
                }
                "--max-candidates" => {
                    options.budgets.max_candidates = required_u64(&mut args, "--max-candidates")?
                }
                "--max-comparisons" => {
                    options.budgets.max_hamming_comparisons =
                        required_u64(&mut args, "--max-comparisons")?
                }
                "--max-certificate-pages" => {
                    options.budgets.max_certificate_page_units =
                        required_u64(&mut args, "--max-certificate-pages")?
                }
                "--max-legacy-pairs" => {
                    options.budgets.max_legacy_possible_pairs =
                        required_u64(&mut args, "--max-legacy-pairs")?
                }
                "--max-stale-zip-pages" => {
                    options.budgets.max_stale_zip_pages =
                        required_u64(&mut args, "--max-stale-zip-pages")?
                }
                "--max-output-bytes" => {
                    options.budgets.max_output_bytes =
                        required_u64(&mut args, "--max-output-bytes")?
                }
                "--help" | "-h" => return Err(usage()),
                other => return Err(format!("unknown argument: {other}\n{}", usage())),
            }
        }
        if options.case_id.is_empty()
            || options.db_path.as_os_str().is_empty()
            || options.active_base_paths.is_empty()
            || options.origin.is_empty()
            || options.roots.is_empty()
            || options.output_path.as_os_str().is_empty()
            || options.expected_result_sha256.is_empty()
        {
            return Err(format!("missing required argument\n{}", usage()));
        }
        if options.expected_result_sha256.len() != 64
            || !options
                .expected_result_sha256
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit())
        {
            return Err("--expect-digest must be exactly 64 hexadecimal characters".to_owned());
        }
        if options.budgets.max_candidates == 0
            || options.budgets.max_hamming_comparisons == 0
            || options.budgets.max_certificate_page_units == 0
            || options.budgets.max_legacy_possible_pairs == 0
            || options.budgets.max_stale_zip_pages == 0
            || options.budgets.max_output_bytes == 0
        {
            return Err("every verification budget must be greater than zero".to_owned());
        }
        options.expected_result_sha256.make_ascii_lowercase();
        Ok(options)
    }
}

fn required_string(
    args: &mut impl Iterator<Item = std::ffi::OsString>,
    flag: &str,
) -> Result<String, String> {
    args.next()
        .ok_or_else(|| format!("{flag} requires a value"))?
        .into_string()
        .map_err(|_| format!("{flag} must be valid Unicode"))
}

fn required_path(
    args: &mut impl Iterator<Item = std::ffi::OsString>,
    flag: &str,
) -> Result<PathBuf, String> {
    args.next()
        .map(PathBuf::from)
        .ok_or_else(|| format!("{flag} requires a path"))
}

fn required_u64(
    args: &mut impl Iterator<Item = std::ffi::OsString>,
    flag: &str,
) -> Result<u64, String> {
    required_string(args, flag)?
        .parse()
        .map_err(|error| format!("{flag}: {error}"))
}

fn usage() -> String {
    "Usage: similar_book_query_verify --case-id ID --db PATH --base PATH [--base PATH] \
     [--retained-base PATH] --origin LOGICAL_KEY --root LOGICAL_ROOT [--root ROOT] \
     --expect-digest SHA256 --output NEW_JSON [--max-candidates N] [--max-comparisons N] \
     [--max-certificate-pages N] [--max-legacy-pairs N] [--max-stale-zip-pages N] \
     [--max-output-bytes N]"
        .to_owned()
}

#[derive(Serialize)]
struct VerificationEnvelope {
    schema_version: u32,
    case_id: String,
    release: bool,
    portable: bool,
    dev_tools: bool,
    binary: PathFingerprint,
    initial_inputs: Vec<BenchInputFingerprint>,
    final_inputs: Vec<BenchInputFingerprint>,
    report: VerificationReport,
}

#[derive(Serialize)]
struct PathFingerprint {
    path: String,
    bytes: u64,
    sha256: String,
}

fn fingerprint_path(path: &Path) -> Result<PathFingerprint, String> {
    let canonical = std::fs::canonicalize(path).map_err(|error| error.to_string())?;
    let file = std::fs::File::open(&canonical).map_err(|error| error.to_string())?;
    let bytes = file.metadata().map_err(|error| error.to_string())?.len();
    let mut reader = BufReader::with_capacity(1024 * 1024, file);
    let mut buffer = vec![0u8; 1024 * 1024];
    let mut digest = Sha256::new();
    loop {
        let read = reader
            .read(&mut buffer)
            .map_err(|error| error.to_string())?;
        if read == 0 {
            break;
        }
        digest.update(&buffer[..read]);
    }
    Ok(PathFingerprint {
        path: canonical.display().to_string(),
        bytes,
        sha256: hex_bytes(&digest.finalize()),
    })
}

fn hex_bytes(bytes: &[u8]) -> String {
    let mut value = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        use std::fmt::Write as _;
        let _ = write!(value, "{byte:02x}");
    }
    value
}
