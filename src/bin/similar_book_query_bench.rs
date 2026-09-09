//! Read-only adoption measurement harness for the product similar-book query engine.

use std::ffi::c_void;
use std::io::{BufWriter, Read, Write};
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::{Duration, Instant};

use mimageviewer::similar_book_query_bench::{
    BenchBaseInfo, BenchCacheObservation, BenchEngine, BenchInputFingerprint, BenchInputSpec,
    BenchQuerySummary, GuardedBenchInputs, expected_product_book_query_worker_priority,
    lower_product_book_query_worker_priority, product_book_query_worker_priority,
};
use serde::Serialize;
use sha2::{Digest, Sha256};

const SCHEMA_VERSION: u32 = 1;

fn main() {
    if let Err(error) = run() {
        eprintln!("similar-book query benchmark failed: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    if cfg!(debug_assertions) || !cfg!(feature = "portable") {
        return Err(
            "adoption measurements require --release --features dev-tools,portable".to_owned(),
        );
    }
    let options = Options::parse()?;
    let run_started = Instant::now();
    let guard_started = Instant::now();
    let mut guarded = GuardedBenchInputs::acquire(
        BenchInputSpec {
            db_path: options.db_path.clone(),
            active_base_paths: options.active_base_paths.clone(),
            retained_base_paths: options.retained_base_paths.clone(),
            immutable_roots: options.roots.clone(),
        },
        &options.output_path,
    )?;
    let input_guard_wall_ns = duration_ns(guard_started.elapsed());
    let initial_inputs = guarded.initial_fingerprints();
    let output_path = guarded.output_path().display().to_string();
    let engine_config = guarded.engine_config();
    let binary = fingerprint_path(&std::env::current_exe().map_err(|error| error.to_string())?)?;

    let (sampler, sampler_thread) = MemorySampler::start(options.sample_interval_ms)?;
    let origin = options.origin.clone();
    let warmup_count = options.warmup_count;
    let sample_count = options.sample_count;
    let worker_spawn_started = Instant::now();
    let worker_sampler = sampler.clone();
    let worker_result = thread::Builder::new()
        .name("similar-book-query-bench".to_owned())
        .spawn(move || {
            run_worker(
                engine_config,
                origin,
                warmup_count,
                sample_count,
                worker_sampler,
                worker_spawn_started,
            )
        })
        .map_err(|error| format!("benchmark worker spawn failed: {error}"))
        .and_then(|worker| {
            worker
                .join()
                .map_err(|_| "benchmark worker panicked".to_owned())?
        });

    // The worker owns and drops the engine/SQLite connection before join returns. Cleanup remains
    // mandatory after spawn, setup, query, sampler, or panic failures: stop and join the sampler,
    // then verify the guarded bytes before reporting any error.
    let sampler_stop_result = sampler.stop();
    drop(sampler);
    let sampler_join_result = sampler_thread
        .join()
        .map_err(|_| "memory sampler panicked".to_owned());

    let final_verify_started = Instant::now();
    let final_inputs_result = guarded.verify_unchanged();
    let final_verify_wall_ns = duration_ns(final_verify_started.elapsed());
    let mut failures = Vec::new();
    let worker_report = match worker_result {
        Ok(report) => Some(report),
        Err(error) => {
            failures.push(error);
            None
        }
    };
    if let Err(error) = sampler_stop_result {
        failures.push(error);
    }
    if let Err(error) = sampler_join_result {
        failures.push(error);
    }
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
    let worker_report = worker_report.expect("worker success was checked");
    let final_inputs = final_inputs.expect("input verification success was checked");
    let measurement_overall_wall_ns = duration_ns(run_started.elapsed());
    let final_memory = process_memory()?;

    let config_record = ConfigRecord {
        record: "config",
        schema_version: SCHEMA_VERSION,
        case_id: &options.case_id,
        output_path: &output_path,
        origin: &options.origin,
        roots: &options.roots,
        active_bases: &options.active_base_paths,
        retained_bases: &options.retained_base_paths,
        warmup_count: options.warmup_count,
        sample_count: options.sample_count,
        sample_interval_ms: options.sample_interval_ms,
        release: !cfg!(debug_assertions),
        portable: cfg!(feature = "portable"),
        dev_tools: cfg!(feature = "dev-tools"),
        binary,
        initial_inputs,
        input_guard_wall_ns,
    };
    let final_record = FinalRecord {
        record: "final",
        schema_version: SCHEMA_VERSION,
        measurement_overall_wall_ns,
        final_verify_wall_ns,
        final_inputs,
        lifetime_peak_working_set_bytes: final_memory.peak_working_set_bytes,
        lifetime_peak_commit_bytes: final_memory.peak_commit_bytes,
    };

    let output_started = Instant::now();
    {
        let mut output = BufWriter::new(guarded.output_file());
        write_json_line(&mut output, &config_record)?;
        write_json_line(&mut output, &worker_report.setup)?;
        for record in &worker_report.warmups {
            write_json_line(&mut output, record)?;
        }
        for record in &worker_report.samples {
            write_json_line(&mut output, record)?;
        }
        write_json_line(&mut output, &final_record)?;
        output.flush().map_err(|error| error.to_string())?;
    }
    let output_write_wall_ns = duration_ns(output_started.elapsed());
    let trailer = OutputRecord {
        record: "output",
        schema_version: SCHEMA_VERSION,
        output_write_wall_ns,
    };
    {
        let mut output = BufWriter::new(guarded.output_file());
        write_json_line(&mut output, &trailer)?;
        output.flush().map_err(|error| error.to_string())?;
    }
    Ok(())
}

fn run_worker(
    config: mimageviewer::similar_book_query_bench::BenchEngineConfig,
    origin: String,
    warmup_count: u64,
    sample_count: u64,
    sampler: MemorySampler,
    worker_spawn_started: Instant,
) -> Result<WorkerReport, String> {
    let (setup_result, setup_metrics) = measure_phase(&sampler, || {
        lower_product_book_query_worker_priority();
        let priority = product_book_query_worker_priority()?;
        let expected = expected_product_book_query_worker_priority();
        if priority != expected {
            return Err(format!(
                "worker priority {priority} did not match product BelowNormal {expected}"
            ));
        }
        let engine = BenchEngine::open(config)?;
        engine.validate_origin(&origin)?;
        let bases = engine.base_infos();
        Ok((engine, priority, bases))
    })?;
    let (mut engine, priority, bases) = setup_result?;
    let setup = SetupRecord {
        record: "setup",
        schema_version: SCHEMA_VERSION,
        worker_priority: priority,
        expected_worker_priority: expected_product_book_query_worker_priority(),
        bases,
        metrics: setup_metrics,
    };

    let total_count = warmup_count
        .checked_add(sample_count)
        .ok_or_else(|| "warmup and sample counts overflow u64".to_owned())?;
    let mut warmups = Vec::new();
    let mut samples = Vec::new();
    let mut first_query_from_worker_spawn_ns = None;
    for index in 0..total_count {
        let phase = if index < warmup_count {
            "warmup"
        } else {
            "sample"
        };
        let phase_index = if phase == "warmup" {
            index
        } else {
            index - warmup_count
        };
        let (query_result, metrics) = measure_phase(&sampler, || engine.query(&origin))?;
        if first_query_from_worker_spawn_ns.is_none() {
            first_query_from_worker_spawn_ns = Some(duration_ns(worker_spawn_started.elapsed()));
        }
        let query = query_result?;
        let cache = engine.cache_observation();
        let summary_started = Instant::now();
        let summary = query.into_summary();
        let summary_wall_ns = duration_ns(summary_started.elapsed());
        let record = QueryRecord {
            record: "query",
            schema_version: SCHEMA_VERSION,
            phase,
            index: phase_index,
            query_core_metrics: metrics,
            first_query_from_worker_spawn_ns: if index == 0 {
                first_query_from_worker_spawn_ns
            } else {
                None
            },
            summary_wall_ns,
            cache,
            summary,
        };
        if phase == "warmup" {
            warmups.push(record);
        } else {
            samples.push(record);
        }
    }
    // Return only owned summaries. `engine` drops here on the worker, before the main thread can
    // join and perform the final guarded input hash.
    drop(engine);
    Ok(WorkerReport {
        setup,
        warmups,
        samples,
    })
}

fn measure_phase<T>(
    sampler: &MemorySampler,
    operation: impl FnOnce() -> T,
) -> Result<(T, PhaseMetrics), String> {
    sampler.begin()?;
    let process_before = process_cpu()?;
    let worker_before = current_thread_cpu()?;
    let started = Instant::now();
    let value = operation();
    let wall_ns = duration_ns(started.elapsed());
    let worker_after = current_thread_cpu()?;
    let process_after = process_cpu()?;
    let memory = sampler.end()?;
    Ok((
        value,
        PhaseMetrics {
            wall_ns,
            process_kernel_100ns: process_after
                .kernel_100ns
                .saturating_sub(process_before.kernel_100ns),
            process_user_100ns: process_after
                .user_100ns
                .saturating_sub(process_before.user_100ns),
            worker_kernel_100ns: worker_after
                .kernel_100ns
                .saturating_sub(worker_before.kernel_100ns),
            worker_user_100ns: worker_after
                .user_100ns
                .saturating_sub(worker_before.user_100ns),
            sampled_peak_working_set_bytes: memory.sampled_peak_working_set_bytes,
            sampled_peak_private_bytes: memory.sampled_peak_private_bytes,
            sampled_count: memory.sampled_count,
            lifetime_peak_working_set_bytes: memory.lifetime_peak_working_set_bytes,
            lifetime_peak_commit_bytes: memory.lifetime_peak_commit_bytes,
        },
    ))
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
    warmup_count: u64,
    sample_count: u64,
    sample_interval_ms: u64,
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
            warmup_count: 0,
            sample_count: 1,
            sample_interval_ms: 5,
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
                "--warmup" => options.warmup_count = required_u64(&mut args, "--warmup")?,
                "--samples" => options.sample_count = required_u64(&mut args, "--samples")?,
                "--sample-interval-ms" => {
                    options.sample_interval_ms = required_u64(&mut args, "--sample-interval-ms")?
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
        {
            return Err(format!("missing required argument\n{}", usage()));
        }
        if options.sample_count == 0 {
            return Err("--samples must be greater than zero".to_owned());
        }
        if options.sample_interval_ms == 0 {
            return Err("--sample-interval-ms must be greater than zero".to_owned());
        }
        Ok(options)
    }
}

fn required_string(
    args: &mut impl Iterator<Item = std::ffi::OsString>,
    flag: &str,
) -> Result<String, String> {
    let value = args
        .next()
        .ok_or_else(|| format!("{flag} requires a value"))?;
    value
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
    "Usage: similar_book_query_bench --case-id ID --db PATH --base PATH [--base PATH] \
     [--retained-base PATH] --origin LOGICAL_KEY --root LOGICAL_ROOT [--root ROOT] \
     --output NEW_JSONL [--warmup N] [--samples N] [--sample-interval-ms N]"
        .to_owned()
}

#[derive(Serialize)]
struct ConfigRecord<'a> {
    record: &'static str,
    schema_version: u32,
    case_id: &'a str,
    output_path: &'a str,
    origin: &'a str,
    roots: &'a [String],
    active_bases: &'a [PathBuf],
    retained_bases: &'a [PathBuf],
    warmup_count: u64,
    sample_count: u64,
    sample_interval_ms: u64,
    release: bool,
    portable: bool,
    dev_tools: bool,
    binary: PathFingerprint,
    initial_inputs: Vec<BenchInputFingerprint>,
    input_guard_wall_ns: u64,
}

#[derive(Serialize)]
struct SetupRecord {
    record: &'static str,
    schema_version: u32,
    worker_priority: i32,
    expected_worker_priority: i32,
    bases: Vec<BenchBaseInfo>,
    metrics: PhaseMetrics,
}

#[derive(Serialize)]
struct QueryRecord {
    record: &'static str,
    schema_version: u32,
    phase: &'static str,
    index: u64,
    query_core_metrics: PhaseMetrics,
    first_query_from_worker_spawn_ns: Option<u64>,
    summary_wall_ns: u64,
    cache: Option<BenchCacheObservation>,
    summary: BenchQuerySummary,
}

#[derive(Serialize)]
struct FinalRecord {
    record: &'static str,
    schema_version: u32,
    measurement_overall_wall_ns: u64,
    final_verify_wall_ns: u64,
    final_inputs: Vec<BenchInputFingerprint>,
    lifetime_peak_working_set_bytes: u64,
    lifetime_peak_commit_bytes: u64,
}

#[derive(Serialize)]
struct OutputRecord {
    record: &'static str,
    schema_version: u32,
    output_write_wall_ns: u64,
}

#[derive(Clone, Copy, Serialize)]
struct PhaseMetrics {
    wall_ns: u64,
    process_kernel_100ns: u64,
    process_user_100ns: u64,
    worker_kernel_100ns: u64,
    worker_user_100ns: u64,
    sampled_peak_working_set_bytes: u64,
    sampled_peak_private_bytes: u64,
    sampled_count: u64,
    lifetime_peak_working_set_bytes: u64,
    lifetime_peak_commit_bytes: u64,
}

#[derive(Serialize)]
struct PathFingerprint {
    path: String,
    bytes: u64,
    sha256: String,
}

struct WorkerReport {
    setup: SetupRecord,
    warmups: Vec<QueryRecord>,
    samples: Vec<QueryRecord>,
}

fn write_json_line(output: &mut impl Write, record: &impl Serialize) -> Result<(), String> {
    serde_json::to_writer(&mut *output, record).map_err(|error| error.to_string())?;
    output.write_all(b"\n").map_err(|error| error.to_string())
}

fn fingerprint_path(path: &std::path::Path) -> Result<PathFingerprint, String> {
    let canonical = std::fs::canonicalize(path).map_err(|error| error.to_string())?;
    let file = std::fs::File::open(&canonical).map_err(|error| error.to_string())?;
    let bytes = file.metadata().map_err(|error| error.to_string())?.len();
    let mut reader = std::io::BufReader::with_capacity(1024 * 1024, file);
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

fn duration_ns(duration: Duration) -> u64 {
    u64::try_from(duration.as_nanos()).unwrap_or(u64::MAX)
}

#[derive(Clone, Copy)]
struct CpuTimes {
    kernel_100ns: u64,
    user_100ns: u64,
}

#[cfg(windows)]
fn process_cpu() -> Result<CpuTimes, String> {
    use windows::Win32::Foundation::FILETIME;
    use windows::Win32::System::Threading::{GetCurrentProcess, GetProcessTimes};
    let mut creation = FILETIME::default();
    let mut exit = FILETIME::default();
    let mut kernel = FILETIME::default();
    let mut user = FILETIME::default();
    unsafe {
        GetProcessTimes(
            GetCurrentProcess(),
            &mut creation,
            &mut exit,
            &mut kernel,
            &mut user,
        )
        .map_err(|error| error.to_string())?;
    }
    Ok(CpuTimes {
        kernel_100ns: filetime_ticks(kernel),
        user_100ns: filetime_ticks(user),
    })
}

#[cfg(not(windows))]
fn process_cpu() -> Result<CpuTimes, String> {
    Err("process CPU measurement requires Windows".to_owned())
}

#[cfg(windows)]
fn current_thread_cpu() -> Result<CpuTimes, String> {
    use windows::Win32::Foundation::FILETIME;
    use windows::Win32::System::Threading::{GetCurrentThread, GetThreadTimes};
    let mut creation = FILETIME::default();
    let mut exit = FILETIME::default();
    let mut kernel = FILETIME::default();
    let mut user = FILETIME::default();
    unsafe {
        GetThreadTimes(
            GetCurrentThread(),
            &mut creation,
            &mut exit,
            &mut kernel,
            &mut user,
        )
        .map_err(|error| error.to_string())?;
    }
    Ok(CpuTimes {
        kernel_100ns: filetime_ticks(kernel),
        user_100ns: filetime_ticks(user),
    })
}

#[cfg(not(windows))]
fn current_thread_cpu() -> Result<CpuTimes, String> {
    Err("thread CPU measurement requires Windows".to_owned())
}

#[cfg(windows)]
fn filetime_ticks(value: windows::Win32::Foundation::FILETIME) -> u64 {
    (u64::from(value.dwHighDateTime) << 32) | u64::from(value.dwLowDateTime)
}

#[derive(Clone, Copy)]
struct ProcessMemory {
    working_set_bytes: u64,
    private_bytes: u64,
    peak_working_set_bytes: u64,
    peak_commit_bytes: u64,
}

#[cfg(windows)]
fn process_memory() -> Result<ProcessMemory, String> {
    #[repr(C)]
    struct ProcessMemoryCountersEx {
        cb: u32,
        page_fault_count: u32,
        peak_working_set_size: usize,
        working_set_size: usize,
        quota_peak_paged_pool_usage: usize,
        quota_paged_pool_usage: usize,
        quota_peak_non_paged_pool_usage: usize,
        quota_non_paged_pool_usage: usize,
        pagefile_usage: usize,
        peak_pagefile_usage: usize,
        private_usage: usize,
    }
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetCurrentProcess() -> *mut c_void;
    }
    #[link(name = "psapi")]
    unsafe extern "system" {
        fn GetProcessMemoryInfo(
            process: *mut c_void,
            counters: *mut ProcessMemoryCountersEx,
            size: u32,
        ) -> i32;
    }
    let mut counters = ProcessMemoryCountersEx {
        cb: std::mem::size_of::<ProcessMemoryCountersEx>() as u32,
        page_fault_count: 0,
        peak_working_set_size: 0,
        working_set_size: 0,
        quota_peak_paged_pool_usage: 0,
        quota_paged_pool_usage: 0,
        quota_peak_non_paged_pool_usage: 0,
        quota_non_paged_pool_usage: 0,
        pagefile_usage: 0,
        peak_pagefile_usage: 0,
        private_usage: 0,
    };
    let ok = unsafe {
        GetProcessMemoryInfo(
            GetCurrentProcess(),
            &mut counters,
            std::mem::size_of::<ProcessMemoryCountersEx>() as u32,
        )
    };
    if ok == 0 {
        return Err(format!(
            "GetProcessMemoryInfo failed: {}",
            std::io::Error::last_os_error()
        ));
    }
    Ok(ProcessMemory {
        working_set_bytes: counters.working_set_size as u64,
        private_bytes: counters.private_usage as u64,
        peak_working_set_bytes: counters.peak_working_set_size as u64,
        peak_commit_bytes: counters.peak_pagefile_usage as u64,
    })
}

#[cfg(not(windows))]
fn process_memory() -> Result<ProcessMemory, String> {
    Err("process memory measurement requires Windows".to_owned())
}

enum SamplerCommand {
    Begin(Sender<Result<(), String>>),
    End(Sender<Result<SampledMemory, String>>),
    Stop(Sender<()>),
    #[cfg(test)]
    SampleNow(Sender<()>),
}

#[derive(Clone)]
struct MemorySampler {
    commands: Sender<SamplerCommand>,
}

#[derive(Clone, Copy)]
struct SampledMemory {
    sampled_peak_working_set_bytes: u64,
    sampled_peak_private_bytes: u64,
    sampled_count: u64,
    lifetime_peak_working_set_bytes: u64,
    lifetime_peak_commit_bytes: u64,
}

impl MemorySampler {
    fn start(interval_ms: u64) -> Result<(Self, thread::JoinHandle<()>), String> {
        if interval_ms == 0 {
            return Err("memory sample interval must be greater than zero".to_owned());
        }
        let (commands, receiver) = mpsc::channel();
        let interval = Duration::from_millis(interval_ms);
        let handle = thread::Builder::new()
            .name("similar-book-memory-sampler".to_owned())
            .spawn(move || sampler_main(receiver, interval))
            .map_err(|error| format!("memory sampler spawn failed: {error}"))?;
        Ok((Self { commands }, handle))
    }

    fn begin(&self) -> Result<(), String> {
        let (reply, response) = mpsc::channel();
        self.commands
            .send(SamplerCommand::Begin(reply))
            .map_err(|_| "memory sampler stopped before Begin".to_owned())?;
        response
            .recv()
            .map_err(|_| "memory sampler dropped Begin response".to_owned())??;
        Ok(())
    }

    fn end(&self) -> Result<SampledMemory, String> {
        let (reply, response) = mpsc::channel();
        self.commands
            .send(SamplerCommand::End(reply))
            .map_err(|_| "memory sampler stopped before End".to_owned())?;
        response
            .recv()
            .map_err(|_| "memory sampler dropped End response".to_owned())?
    }

    fn stop(&self) -> Result<(), String> {
        let (reply, response) = mpsc::channel();
        self.commands
            .send(SamplerCommand::Stop(reply))
            .map_err(|_| "memory sampler stopped before Stop".to_owned())?;
        response
            .recv()
            .map_err(|_| "memory sampler dropped Stop response".to_owned())
    }

    #[cfg(test)]
    fn sample_now_for_test(&self) {
        let (reply, response) = mpsc::channel();
        self.commands
            .send(SamplerCommand::SampleNow(reply))
            .unwrap();
        response.recv().unwrap();
    }
}

fn sampler_main(receiver: Receiver<SamplerCommand>, interval: Duration) {
    sampler_main_with(receiver, interval, process_memory);
}

enum ActiveSample {
    Measuring(SampledMemory),
    Failed(String),
}

fn sampler_main_with(
    receiver: Receiver<SamplerCommand>,
    interval: Duration,
    mut read_memory: impl FnMut() -> Result<ProcessMemory, String>,
) {
    let mut active = None::<ActiveSample>;
    loop {
        let command = if active.is_some() {
            match receiver.recv_timeout(interval) {
                Ok(command) => Some(command),
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    update_active_sample(&mut active, &mut read_memory);
                    None
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => return,
            }
        } else {
            match receiver.recv() {
                Ok(command) => Some(command),
                Err(_) => return,
            }
        };
        let Some(command) = command else {
            continue;
        };
        match command {
            SamplerCommand::Begin(reply) => {
                let response = read_memory().map(|memory| SampledMemory {
                    sampled_peak_working_set_bytes: memory.working_set_bytes,
                    sampled_peak_private_bytes: memory.private_bytes,
                    sampled_count: 1,
                    lifetime_peak_working_set_bytes: memory.peak_working_set_bytes,
                    lifetime_peak_commit_bytes: memory.peak_commit_bytes,
                });
                match response {
                    Ok(sample) => {
                        active = Some(ActiveSample::Measuring(sample));
                        let _ = reply.send(Ok(()));
                    }
                    Err(error) => {
                        active = None;
                        let _ = reply.send(Err(error));
                    }
                }
            }
            SamplerCommand::End(reply) => {
                let response = match active.take() {
                    Some(ActiveSample::Measuring(mut sample)) => read_memory().map(|memory| {
                        update_sample(&mut sample, memory);
                        sample
                    }),
                    Some(ActiveSample::Failed(error)) => Err(error),
                    None => Err("memory sampler received End without Begin".to_owned()),
                };
                let _ = reply.send(response);
            }
            SamplerCommand::Stop(reply) => {
                let _ = reply.send(());
                return;
            }
            #[cfg(test)]
            SamplerCommand::SampleNow(reply) => {
                update_active_sample(&mut active, &mut read_memory);
                let _ = reply.send(());
            }
        }
    }
}

fn update_active_sample(
    active: &mut Option<ActiveSample>,
    read_memory: &mut impl FnMut() -> Result<ProcessMemory, String>,
) {
    let Some(ActiveSample::Measuring(sample)) = active else {
        return;
    };
    match read_memory() {
        Ok(memory) => update_sample(sample, memory),
        Err(error) => *active = Some(ActiveSample::Failed(error)),
    }
}

fn update_sample(sample: &mut SampledMemory, memory: ProcessMemory) {
    sample.sampled_peak_working_set_bytes = sample
        .sampled_peak_working_set_bytes
        .max(memory.working_set_bytes);
    sample.sampled_peak_private_bytes = sample.sampled_peak_private_bytes.max(memory.private_bytes);
    sample.sampled_count = sample.sampled_count.saturating_add(1);
    sample.lifetime_peak_working_set_bytes = sample
        .lifetime_peak_working_set_bytes
        .max(memory.peak_working_set_bytes);
    sample.lifetime_peak_commit_bytes = sample
        .lifetime_peak_commit_bytes
        .max(memory.peak_commit_bytes);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interval_sampling_failure_is_reported_at_end() {
        let (commands, receiver) = mpsc::channel();
        let mut call_count = 0u64;
        let sampler_thread = thread::spawn(move || {
            sampler_main_with(receiver, Duration::from_secs(60), move || {
                call_count += 1;
                if call_count == 1 {
                    Ok(ProcessMemory {
                        working_set_bytes: 10,
                        private_bytes: 20,
                        peak_working_set_bytes: 30,
                        peak_commit_bytes: 40,
                    })
                } else {
                    Err("injected interval sample failure".to_owned())
                }
            })
        });
        let sampler = MemorySampler { commands };

        sampler.begin().unwrap();
        sampler.sample_now_for_test();
        let error = match sampler.end() {
            Ok(_) => panic!("an interval failure must reject the measurement"),
            Err(error) => error,
        };
        assert!(error.contains("injected interval sample failure"));
        sampler.stop().unwrap();
        drop(sampler);
        sampler_thread.join().unwrap();
    }
}
