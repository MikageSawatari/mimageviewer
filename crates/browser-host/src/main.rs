fn main() {
    // Rust's redirected Windows stdio uses raw pipe I/O, not CRT text translation.
    // Do not print to stdout: it contains exactly one native-messaging response.
    let origin = std::env::args_os().nth(1);
    let executable = std::env::current_exe().unwrap_or_default();
    let result = mimageviewer_browser_host::run_one(
        origin.as_deref().and_then(|value| value.to_str()),
        &executable,
        &mut std::io::stdin().lock(),
        &mut std::io::stdout().lock(),
        &mut mimageviewer_browser_host::NativeEffects,
    );
    if result.is_err() {
        eprintln!("browser_host: response write failed");
        std::process::exit(1);
    }
}
