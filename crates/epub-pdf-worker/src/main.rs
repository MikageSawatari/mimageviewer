use epub_pdf_worker::{
    Report,
    package::{self, EpubErrorKind},
    paths::{self, OutputTemp},
    protocol::{Event, Phase},
    render::{self, Segment},
    report::{SourceImage, UserDataCleanup},
    webview::Host,
};
use std::{
    fs,
    panic::{AssertUnwindSafe, catch_unwind},
    path::{Path, PathBuf},
    thread,
    time::{Duration, Instant},
};

const USAGE: &str = "usage:\n  mimageviewer-epub-pdf inspect <in.epub>\n  mimageviewer-epub-pdf convert <in.epub> <out> --work-dir <dir> [--user-data-dir <dir>] [--progress-json] [--report <file.json>] [--timeout-secs N] [--force-iframe]\n  mimageviewer-epub-pdf batch <dir-with-epubs> <out-dir> [--timeout-secs N]";
struct Options {
    timeout_secs: u64,
    force_iframe: bool,
    work_dir: Option<PathBuf>,
    report: Option<PathBuf>,
    user_data_dir: Option<PathBuf>,
    progress_json: bool,
}
fn options(args: &[String]) -> Result<Options, String> {
    let mut o = Options {
        timeout_secs: 600,
        force_iframe: false,
        work_dir: None,
        report: None,
        user_data_dir: None,
        progress_json: false,
    };
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--timeout-secs" => {
                i += 1;
                o.timeout_secs = args
                    .get(i)
                    .ok_or("missing timeout value")?
                    .parse()
                    .map_err(|_| "invalid timeout")?;
                if o.timeout_secs == 0 {
                    return Err("timeout must be positive".into());
                }
            }
            "--work-dir" => {
                i += 1;
                o.work_dir = Some(PathBuf::from(args.get(i).ok_or("missing work directory")?));
            }
            "--report" => {
                i += 1;
                o.report = Some(PathBuf::from(args.get(i).ok_or("missing report path")?));
            }
            "--user-data-dir" => {
                i += 1;
                o.user_data_dir = Some(PathBuf::from(
                    args.get(i).ok_or("missing user data directory")?,
                ));
            }
            "--progress-json" => o.progress_json = true,
            "--force-iframe" => o.force_iframe = true,
            x => return Err(format!("unknown option: {x}")),
        }
        i += 1;
    }
    Ok(o)
}
fn absolute(path: &Path) -> Result<PathBuf, String> {
    if path.is_absolute() {
        Ok(path.to_path_buf())
    } else {
        Ok(std::env::current_dir()
            .map_err(|e| e.to_string())?
            .join(path))
    }
}
fn write_json(path: &Path, report: &Report) -> Result<(), String> {
    if let Some(p) = path.parent() {
        fs::create_dir_all(p).map_err(|e| e.to_string())?;
    }
    fs::write(
        path,
        serde_json::to_vec_pretty(report).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())
}
fn cleanup_user_data(dir: &Path) -> UserDataCleanup {
    let start = Instant::now();
    let mut last = None;
    let path = dir.display().to_string();
    for _ in 0..300 {
        if !dir.exists() {
            return UserDataCleanup {
                path,
                deleted: true,
                held_ms: start.elapsed().as_millis(),
                error: None,
            };
        }
        match fs::remove_dir_all(dir) {
            Ok(()) => {
                return UserDataCleanup {
                    path,
                    deleted: true,
                    held_ms: start.elapsed().as_millis(),
                    error: None,
                };
            }
            Err(e) => last = Some(e.to_string()),
        }
        thread::sleep(Duration::from_millis(100));
    }
    UserDataCleanup {
        path,
        deleted: false,
        held_ms: start.elapsed().as_millis(),
        error: last,
    }
}
struct Engine {
    work_dir: PathBuf,
    user_data: PathBuf,
    owns_user_data: bool,
    host: Option<Host>,
    unavailable: Option<(i32, String)>,
    run_id: String,
    sequence: usize,
    progress_json: bool,
}
impl Engine {
    fn new(
        dir: &Path,
        user_data_dir: Option<&Path>,
        progress_json: bool,
        convert: Option<(&Path, &Path)>,
    ) -> Result<Self, String> {
        let work_dir = absolute(dir)?;
        fs::create_dir_all(&work_dir).map_err(|e| e.to_string())?;
        let run_id = format!(
            "{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_err(|e| e.to_string())?
                .as_millis()
        );
        let user_data = if let Some(dir) = user_data_dir {
            absolute(dir)?
        } else {
            work_dir.join(format!("webview2-user-data-{run_id}"))
        };
        if let Some((input, out)) = convert {
            paths::validate_convert_paths(input, out, &work_dir, &user_data)?;
        }
        if user_data.exists() {
            return Err(format!(
                "user data directory already exists: {}",
                user_data.display()
            ));
        }
        if let Some(parent) = user_data.parent() {
            fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        fs::create_dir(&user_data).map_err(|e| format!("create user data directory: {e}"))?;
        let engine = Self {
            work_dir,
            user_data,
            owns_user_data: true,
            host: None,
            unavailable: None,
            run_id,
            sequence: 0,
            progress_json,
        };
        #[cfg(debug_assertions)]
        if std::env::var_os("MIV_EPUB_PDF_TEST_PANIC").as_deref()
            == Some(std::ffi::OsStr::new("after_user_data"))
        {
            panic!("test panic after user data folder creation");
        }
        Ok(engine)
    }
    fn progress(&self, phase: Phase, done: usize, total: usize) {
        if self.progress_json {
            println!(
                "{}",
                serde_json::to_string(&Event::Progress { phase, done, total })
                    .expect("protocol serialisation")
            );
        }
    }
    fn process(
        &mut self,
        input: &Path,
        out: &Path,
        timeout_secs: u64,
        force_iframe: bool,
    ) -> Report {
        let start = Instant::now();
        let deadline = start + Duration::from_secs(timeout_secs);
        let mut report = Report::new(input);
        if let Some(host) = &self.host {
            host.reset_blocked();
        }
        self.sequence += 1;
        let same_file = fs::canonicalize(input)
            .ok()
            .zip(fs::canonicalize(out).ok())
            .is_some_and(|(a, b)| a == b);
        let result = if same_file {
            Err((3, "input and output refer to the same file".into()))
        } else {
            OutputTemp::new(out, std::process::id(), self.sequence)
                .map_err(|e| (5, e))
                .and_then(|temp| {
                    self.process_inner(input, temp.path(), force_iframe, deadline, &mut report)?;
                    if Instant::now() >= deadline {
                        return Err((6, "WebView2 timeout before output publication".into()));
                    }
                    temp.publish(out)
                        .map_err(|e| (5, format!("publish PDF: {e}")))
                })
        };
        if let Some(host) = &self.host {
            (report.blocked_requests, report.blocked_request_count) = host.blocked_requests();
        }
        if let Err((code, error)) = result {
            report.fail(code, error)
        } else {
            report.status = "success".into();
        }
        if report.image_fidelity.is_none() {
            report.image_fidelity = Some("not_measured".into());
        }
        report
            .timings
            .insert("total_ms".into(), start.elapsed().as_millis());
        report
    }
    fn process_inner(
        &mut self,
        input: &Path,
        out: &Path,
        force_iframe: bool,
        deadline: Instant,
        r: &mut Report,
    ) -> Result<(), (i32, String)> {
        let t = Instant::now();
        self.progress(Phase::Parse, 0, 1);
        let parsed = package::inspect_file(input);
        r.timings.insert("parse_ms".into(), t.elapsed().as_millis());
        let package = parsed.map_err(|e| {
            r.drm = if e.kind == EpubErrorKind::Drm {
                "detected".into()
            } else {
                "unknown".into()
            };
            (
                if e.kind == EpubErrorKind::Drm { 2 } else { 3 },
                e.to_string(),
            )
        })?;
        self.progress(Phase::Parse, 1, 1);
        r.title = package.title.clone();
        r.direction = Some(package.direction.clone());
        r.spine_count = Some(package.spine.len());
        r.layout = Some(
            if package
                .spine
                .iter()
                .any(|x| x.rendition.layout.as_deref() == Some("pre-paginated"))
            {
                if package
                    .spine
                    .iter()
                    .any(|x| x.rendition.layout.as_deref() != Some("pre-paginated"))
                {
                    "mixed"
                } else {
                    "pre-paginated"
                }
            } else {
                "reflowable"
            }
            .into(),
        );
        r.drm = "none".into();
        let root = self
            .work_dir
            .join(format!("book-{}-{}", self.run_id, self.sequence));
        fs::create_dir_all(&root).map_err(|e| (5, e.to_string()))?;
        let t = Instant::now();
        self.progress(Phase::Extract, 0, 1);
        package::extract_file(input, &root).map_err(|e| (3, e.to_string()))?;
        self.progress(Phase::Extract, 1, 1);
        r.timings
            .insert("extract_ms".into(), t.elapsed().as_millis());
        let sources = render::source_images(&package, &root);
        r.source_images = sources
            .iter()
            .map(|(format, width, height, bytes, sha256)| SourceImage {
                format: format.clone(),
                width: *width,
                height: *height,
                bytes: *bytes,
                sha256: sha256.clone(),
            })
            .collect();
        if let Some(error) = &self.unavailable {
            return Err(error.clone());
        }
        if self.host.is_none() {
            let t = Instant::now();
            self.progress(Phase::Init, 0, 1);
            match Host::new(&self.user_data, deadline) {
                Ok(host) => self.host = Some(host),
                Err(error) => {
                    let failure = classify_webview_error(error);
                    self.unavailable = Some(failure.clone());
                    return Err(failure);
                }
            }
            r.timings
                .insert("webview_init_ms".into(), t.elapsed().as_millis());
        }
        self.progress(Phase::Init, 1, 1);
        let host = self.host.as_ref().unwrap();
        r.webview_runtime = Some(host.version.clone());
        r.web_resource_filter = Some(host.request_filter.clone());
        r.user_data_folder_redirected = host
            .user_data_folder_redirected
            .as_ref()
            .map(|path| path.display().to_string());
        r.user_data_folder_check_error = host.user_data_folder_check_error.clone();
        host.map_folder(&root).map_err(|e| (5, e))?;
        let mut part_files = Vec::<PathBuf>::new();
        let mut size_sources = Vec::<Option<String>>::new();
        let mut expected = Vec::<Option<(u32, u32)>>::new();
        let mut print_index = 0;
        let mut printed_pages = 0;
        self.progress(Phase::Print, 0, 0);
        for (first, items) in render::segments(&package) {
            let fixed = items[0].rendition.layout.as_deref() == Some("pre-paginated");
            let mut segment = Segment {
                kind: if fixed { "fixed" } else { "reflow" }.into(),
                first_spine: first,
                spine_count: items.len(),
                output_pages: 0,
                print_ms: 0,
                chunks: 0,
                method: "Page.printToPDF".into(),
                error: None,
            };
            let chunks: Vec<&[&package::SpineItem]> = if fixed {
                items.chunks(50).collect()
            } else {
                vec![&items]
            };
            for chunk in chunks {
                let t = Instant::now();
                print_index += 1;
                let relative = if fixed {
                    render::write_fixed_html(&root, chunk, force_iframe, print_index)
                        .map_err(|e| (5, e))?
                } else {
                    render::reflow_print_copy(&root, chunk[0], print_index).map_err(|e| (5, e))?
                };
                let url = render::virtual_url(&relative);
                host.navigate(&url).map_err(|e| (5, e))?;
                host.wait_ready(&url, deadline)
                    .map_err(classify_webview_error)?;
                r.book_script_ran |= host
                    .book_script_ran(deadline)
                    .map_err(classify_webview_error)?;
                let pdf = host
                    .devtools_pdf(deadline)
                    .map_err(classify_webview_error)?;
                let part = self.work_dir.join(format!(
                    "part-{}-{}-{print_index}.pdf",
                    self.run_id, self.sequence
                ));
                fs::write(&part, &pdf).map_err(|e| (5, e.to_string()))?;
                let actual = render::pdf_sizes(&part).map_err(|e| (5, e))?;
                if fixed && actual.len() != chunk.len() {
                    return Err((
                        5,
                        format!(
                            "fixed segment page count mismatch: {} spine pages -> {} PDF pages",
                            chunk.len(),
                            actual.len()
                        ),
                    ));
                }
                for (n, _) in actual.iter().enumerate() {
                    if fixed {
                        let item = chunk[n];
                        size_sources.push(
                            item.size_source
                                .clone()
                                .or(Some("fallback_1200x1700".into())),
                        );
                        expected.push(Some((
                            item.width.unwrap_or(1200),
                            item.height.unwrap_or(1700),
                        )));
                    } else {
                        size_sources.push(Some(render::REFLOW_PROFILE.into()));
                        expected.push(None);
                    }
                }
                segment.output_pages += actual.len();
                printed_pages += actual.len();
                self.progress(Phase::Print, printed_pages, 0);
                segment.chunks += 1;
                segment.print_ms += t.elapsed().as_millis();
                part_files.push(part);
            }
            r.timings.insert(
                format!("segment_{}_print_ms", r.segments.len() + 1),
                segment.print_ms,
            );
            r.segments.push(segment);
        }
        let t = Instant::now();
        self.progress(Phase::Merge, 0, 1);
        render::merge_pdf(&part_files, out, package.direction == "rtl").map_err(|e| (5, e))?;
        self.progress(Phase::Merge, 1, 1);
        r.timings.insert("merge_ms".into(), t.elapsed().as_millis());
        self.progress(Phase::Verify, 0, 1);
        let (pages, images) = render::inspect_pdf(out, &size_sources).map_err(|e| (5, e))?;
        for (page, expect) in pages.iter().zip(expected.iter()) {
            if let Some((w, h)) = expect
                && ((page.width_pt - *w as f64 * 0.75).abs() > 1.0
                    || (page.height_pt - *h as f64 * 0.75).abs() > 1.0)
            {
                r.errors.push(format!(
                    "page {} size mismatch: expected {}x{} pt, got {:.1}x{:.1}",
                    page.number,
                    *w as f64 * 0.75,
                    *h as f64 * 0.75,
                    page.width_pt,
                    page.height_pt
                ));
            }
        }
        r.output_page_count = Some(pages.len());
        r.pdf_pages = pages;
        r.image_fidelity = Some(render::image_verdict(&sources, &images));
        r.pdf_images = images;
        r.output_bytes = fs::metadata(out).ok().map(|x| x.len());
        if !r.errors.is_empty() {
            return Err((5, "PDF page size verification failed".into()));
        }
        self.progress(Phase::Verify, 1, 1);
        Ok(())
    }
    fn finish(mut self) -> UserDataCleanup {
        self.host.take();
        // Only the fresh requested path created by Engine::new belongs to us.
        let cleanup = cleanup_user_data(&self.user_data);
        self.owns_user_data = false;
        cleanup
    }
}
impl Drop for Engine {
    fn drop(&mut self) {
        self.host.take();
        if self.owns_user_data {
            let _ = cleanup_user_data(&self.user_data);
            self.owns_user_data = false;
        }
    }
}
fn classify_webview_error(e: String) -> (i32, String) {
    let code = if e.contains("WebView2 unsupported") {
        8
    } else if e.contains("runtime missing") {
        4
    } else if e.contains("timeout") {
        6
    } else {
        5
    };
    (code, e)
}
fn run() -> Result<(i32, Option<Event>), String> {
    epub_pdf_worker::webview::clear_webview_environment();
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.as_slice() {
        [cmd, input] if cmd == "inspect" => match package::inspect_file(Path::new(input)) {
            Ok(p) => {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&p).map_err(|e| e.to_string())?
                );
                Ok((0, None))
            }
            Err(e) => {
                eprintln!("{e}");
                Ok((if e.kind == EpubErrorKind::Drm { 2 } else { 3 }, None))
            }
        },
        [cmd, input, out, rest @ ..] if cmd == "convert" => {
            let o = options(rest)?;
            let work = o.work_dir.ok_or("convert requires --work-dir")?;
            let mut engine = Engine::new(
                &work,
                o.user_data_dir.as_deref(),
                o.progress_json,
                Some((Path::new(input), Path::new(out))),
            )?;
            let mut r = engine.process(
                Path::new(input),
                Path::new(out),
                o.timeout_secs,
                o.force_iframe,
            );
            r.user_data_cleanup = Some(engine.finish());
            let report = o
                .report
                .unwrap_or_else(|| PathBuf::from(out).with_extension("json"));
            if let Err(error) = write_json(&report, &r) {
                eprintln!("report {}: {error}", report.display());
            }
            if !o.progress_json {
                println!("{}: {} (report {})", r.file, r.status, report.display());
            }
            Ok((r.exit_code, Some(Event::from_report(&r))))
        }
        [cmd, input, out, rest @ ..] if cmd == "batch" => {
            let o = options(rest)?;
            if o.force_iframe
                || o.work_dir.is_some()
                || o.report.is_some()
                || o.user_data_dir.is_some()
                || o.progress_json
            {
                return Err("batch accepts only --timeout-secs".into());
            }
            let in_dir = Path::new(input);
            let out_dir = Path::new(out);
            fs::create_dir_all(out_dir).map_err(|e| e.to_string())?;
            let mut files = walkdir::WalkDir::new(in_dir)
                .into_iter()
                .filter_map(Result::ok)
                .filter(|x| {
                    x.file_type().is_file()
                        && x.path()
                            .extension()
                            .is_some_and(|e| e.eq_ignore_ascii_case("epub"))
                })
                .map(|x| x.path().to_path_buf())
                .collect::<Vec<_>>();
            files.sort();
            let mut engine = Engine::new(&out_dir.join("_work"), None, false, None)?;
            let mut reports = Vec::new();
            for (n, file) in files.iter().enumerate() {
                let stem = file.file_stem().unwrap_or_default().to_string_lossy();
                let name = format!("{n:03}_{stem}");
                let pdf = out_dir.join(format!("{name}.pdf"));
                let r = engine.process(file, &pdf, o.timeout_secs, false);
                println!("{}: {}", file.display(), r.status);
                reports.push((name, r));
            }
            let cleanup = engine.finish();
            for (name, r) in &mut reports {
                r.user_data_cleanup = Some(cleanup.clone());
                write_json(&out_dir.join(format!("{name}.json")), r)?;
            }
            let all: Vec<_> = reports.into_iter().map(|(_, r)| r).collect();
            fs::write(
                out_dir.join("summary.json"),
                serde_json::to_vec_pretty(&all).map_err(|e| e.to_string())?,
            )
            .map_err(|e| e.to_string())?;
            fs::write(out_dir.join("summary.md"), render::summarize(&all))
                .map_err(|e| e.to_string())?;
            Ok((
                all.iter()
                    .find(|r| r.exit_code != 0)
                    .map_or(0, |r| r.exit_code),
                None,
            ))
        }
        _ => Err(USAGE.into()),
    }
}
fn main() {
    let progress_json = std::env::args_os().any(|arg| arg == "--progress-json");
    let (code, result) = match catch_unwind(AssertUnwindSafe(run)) {
        Ok(Ok((code, result))) => (code, result),
        Ok(Err(error)) => {
            eprintln!("{error}");
            (3, Some(Event::failure(3, error)))
        }
        Err(_) => {
            let message = "converter panicked".to_string();
            eprintln!("{message}");
            (5, Some(Event::failure(5, message)))
        }
    };
    if progress_json {
        let result = result.unwrap_or_else(|| Event::failure(code, String::new()));
        println!(
            "{}",
            serde_json::to_string(&result).expect("result protocol serialisation")
        );
    }
    std::process::exit(code)
}
