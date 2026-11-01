fn main() {
    if let Err(error) = run() {
        eprintln!("mimageviewer-mcp: {error}");
        std::process::exit(1);
    }
}
fn run() -> std::io::Result<()> {
    let mut args = std::env::args_os().skip(1);
    let data_dir = match args.next() {
        None => mimageviewer_mcp::pipe::default_data_dir()?,
        Some(arg) if arg == "--data-dir" => {
            args.next().map(std::path::PathBuf::from).ok_or_else(|| {
                std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "--data-dir requires a path",
                )
            })?
        }
        _ => {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "expected --data-dir <path>",
            ));
        }
    };
    if args.next().is_some() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "unexpected arguments",
        ));
    }
    let backend = std::sync::Arc::new(mimageviewer_mcp::pipe::PipeBackend::new(&data_dir)?);
    mimageviewer_mcp::run_stdio_with_eof(std::io::stdin(), std::io::stdout(), backend, || {
        // stdin EOF owns this stdio child process's lifetime. Do not wait for a
        // blocked stdout writer; process teardown closes the core pipe as well.
        // Rust process::exit runs stdout cleanup and can wait for its lock.
        // Windows owns this executable; bypass that cleanup at the EOF boundary.
        #[cfg(windows)]
        unsafe {
            windows::Win32::System::Threading::ExitProcess(0)
        }
        #[cfg(not(windows))]
        std::process::exit(0)
    })
}
