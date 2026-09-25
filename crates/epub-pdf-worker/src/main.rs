use epub_pdf_worker::{
    Report,
    package::{self, EpubErrorKind},
    render::{self, Segment},
    report::{SourceImage, UserDataCleanup},
    webview::Host,
};
use std::{
    fs,
    path::{Path, PathBuf},
    thread,
    time::{Duration, Instant},
};

const USAGE: &str = "usage:\n  mimageviewer-epub-pdf inspect <in.epub>\n  mimageviewer-epub-pdf convert <in.epub> <out.pdf> --work-dir <dir> [--report <file.json>] [--timeout-secs N] [--force-iframe]\n  mimageviewer-epub-pdf batch <dir-with-epubs> <out-dir> [--timeout-secs N]";
struct Options {
    timeout_secs: u64,
    force_iframe: bool,
    work_dir: Option<PathBuf>,
    report: Option<PathBuf>,
}
fn options(args: &[String]) -> Result<Options, String> {
    let mut o = Options {
        timeout_secs: 600,
        force_iframe: false,
        work_dir: None,
        report: None,
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
    host: Option<Host>,
    unavailable: Option<(i32, String)>,
    run_id: String,
    sequence: usize,
}
impl Engine {
    fn new(dir: &Path) -> Result<Self, String> {
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
        let user_data = work_dir.join(format!("webview2-user-data-{run_id}"));
        Ok(Self {
            work_dir,
            user_data,
            host: None,
            unavailable: None,
            run_id,
            sequence: 0,
        })
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
        let result = self.process_inner(input, out, force_iframe, deadline, &mut report);
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
        self.sequence += 1;
        let root = self
            .work_dir
            .join(format!("book-{}-{}", self.run_id, self.sequence));
        fs::create_dir_all(&root).map_err(|e| (5, e.to_string()))?;
        let t = Instant::now();
        package::extract_file(input, &root).map_err(|e| (3, e.to_string()))?;
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
        let host = self.host.as_ref().unwrap();
        r.webview_runtime = Some(host.version.clone());
        host.map_folder(&root).map_err(|e| (5, e))?;
        let mut part_files = Vec::<PathBuf>::new();
        let mut size_sources = Vec::<Option<String>>::new();
        let mut expected = Vec::<Option<(u32, u32)>>::new();
        let mut print_index = 0;
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
                        size_sources.push(Some("reflow_default_1200x1700".into()));
                        expected.push(None);
                    }
                }
                segment.output_pages += actual.len();
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
        if let Some(p) = out.parent() {
            fs::create_dir_all(p).map_err(|e| (5, e.to_string()))?;
        }
        let t = Instant::now();
        render::merge_pdf(&part_files, out, package.direction == "rtl").map_err(|e| (5, e))?;
        r.timings.insert("merge_ms".into(), t.elapsed().as_millis());
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
        Ok(())
    }
    fn finish(mut self) -> UserDataCleanup {
        self.host.take();
        cleanup_user_data(&self.user_data)
    }
}
fn classify_webview_error(e: String) -> (i32, String) {
    let code = if e.contains("runtime missing") {
        4
    } else if e.contains("timeout") {
        6
    } else {
        5
    };
    (code, e)
}
fn run() -> Result<i32, String> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.as_slice() {
        [cmd, input] if cmd == "inspect" => match package::inspect_file(Path::new(input)) {
            Ok(p) => {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&p).map_err(|e| e.to_string())?
                );
                Ok(0)
            }
            Err(e) => {
                eprintln!("{e}");
                Ok(if e.kind == EpubErrorKind::Drm { 2 } else { 3 })
            }
        },
        [cmd, input, out, rest @ ..] if cmd == "convert" => {
            let o = options(rest)?;
            let work = o.work_dir.ok_or("convert requires --work-dir")?;
            let mut engine = Engine::new(&work)?;
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
            write_json(&report, &r)?;
            println!("{}: {} (report {})", r.file, r.status, report.display());
            Ok(r.exit_code)
        }
        [cmd, input, out, rest @ ..] if cmd == "batch" => {
            let o = options(rest)?;
            if o.force_iframe || o.work_dir.is_some() || o.report.is_some() {
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
            let mut engine = Engine::new(&out_dir.join("_work"))?;
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
            Ok(all
                .iter()
                .find(|r| r.exit_code != 0)
                .map_or(0, |r| r.exit_code))
        }
        _ => Err(USAGE.into()),
    }
}
fn main() {
    match run() {
        Ok(code) => std::process::exit(code),
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(3)
        }
    }
}
