mod auth;
mod config;
mod diagnostics;
mod http;
mod image_support;
mod path_guard;
mod store;

use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::AtomicU64;

use auth::AuthToken;
use config::{Config, default_data_dir};
use diagnostics::DiagnosticsLogger;
use http::{AppState, TelemetryLimiter};
use store::Library;
use tiny_http::Server;

fn main() {
    if let Err(error) = run() {
        eprintln!("{error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let config = Config::parse()?;
    let token =
        AuthToken::generate().map_err(|error| format!("認証トークンを生成できません: {error}"))?;
    let logger = DiagnosticsLogger::open(
        &config.log_path,
        &[default_data_dir(), config.data_dir.clone()],
        token.printable(),
    )?;
    let library = Library::load(&config.data_dir)
        .map_err(|error| format!("settings.db を read-only で読み込めません: {error:?}"))?;
    let address = SocketAddr::new(config.bind, config.port);
    let server = Arc::new(
        Server::http(address).map_err(|error| format!("HTTP bind に失敗しました: {error}"))?,
    );

    println!("mIV remote PoC bind: http://{address}");
    println!("計測ログ: {}", logger.path().display());
    println!("認証トークン: {}", token.printable());
    if config.bind.is_unspecified() {
        println!(
            "初回 URL: http://<このPCのIP>:{}/?t={}",
            config.port,
            token.printable()
        );
    } else {
        println!("初回 URL: http://{address}/?t={}", token.printable());
    }

    let state = Arc::new(AppState {
        token,
        library,
        logger,
        telemetry_limiter: TelemetryLimiter::new(),
        request_sequence: AtomicU64::new(1),
        web_root: config.web_root,
    });
    let workers = std::thread::available_parallelism()
        .map(usize::from)
        .unwrap_or(4)
        .clamp(2, 8);
    println!("HTTP workers: {workers}");

    let mut threads = Vec::with_capacity(workers);
    for index in 0..workers {
        let server = Arc::clone(&server);
        let state = Arc::clone(&state);
        threads.push(
            std::thread::Builder::new()
                .name(format!("remote-http-{index}"))
                .spawn(move || {
                    loop {
                        match server.recv() {
                            Ok(request) => http::handle(request, &state),
                            Err(error) => {
                                eprintln!("remote-web: HTTP receive stopped: {error}");
                                break;
                            }
                        }
                    }
                })
                .map_err(|error| format!("HTTP worker を開始できません: {error}"))?,
        );
    }

    for thread in threads {
        let _ = thread.join();
    }
    Ok(())
}
