mod auth;
mod config;
mod connection_url;
mod console_qr;
mod diagnostics;
mod http;
mod image_support;
mod ipc_client;
mod path_guard;
mod store;

use std::io::Write as _;
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::AtomicU64;

use auth::{AuthService, AuthToken, load_pin_file, set_pin_file};
use config::{Config, default_data_dir};
use connection_url::choose_connection_url;
use diagnostics::DiagnosticsLogger;
use http::{
    AppState, HTTP_WORKER_COUNT, IpcAdmission, MAX_CONCURRENT_HEAVY_IPC, MAX_CONCURRENT_IPC,
    TelemetryLimiter,
};
use ipc_client::ThumbnailClient;
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
    let protected_roots = [default_data_dir(), config.data_dir.clone()];
    if let Some(pin) = config.set_pin.as_deref() {
        let path = set_pin_file(&config.auth_path, &protected_roots, pin)?;
        println!("PIN を設定しました: {}", path.display());
        println!("PIN の平文は保存していません。次回は --set-pin なしで起動してください。");
        return Ok(());
    }

    let loaded_auth = load_pin_file(&config.auth_path, &protected_roots)?;
    let bearer =
        AuthToken::generate().map_err(|error| format!("認証トークンを生成できません: {error}"))?;
    let auth = AuthService::new(loaded_auth.record, bearer)?;
    let log_secrets = auth.permanent_log_secrets();
    let logger = DiagnosticsLogger::open(&config.log_path, &protected_roots, &log_secrets)?;
    if logger.path() == loaded_auth.path {
        return Err("--log と --auth-file には別のファイルを指定してください".to_owned());
    }
    let library = Library::load(&config.data_dir)
        .map_err(|error| format!("settings.db を read-only で読み込めません: {error:?}"))?;
    let thumbnail_client = ThumbnailClient::new();
    let ipc_status = match thumbnail_client.probe() {
        Ok(()) => "接続済み".to_owned(),
        Err(error) => format!("未接続 ({error})"),
    };
    let address = SocketAddr::new(config.bind, config.port);
    let connection = choose_connection_url(config.public_url.as_deref(), address)?;
    let server = Arc::new(
        Server::http(address).map_err(|error| format!("HTTP bind に失敗しました: {error}"))?,
    );

    println!("mIV remote PoC bind: http://{address}");
    println!("計測ログ: {}", logger.path().display());
    println!("認証ファイル: {}", loaded_auth.path.display());
    println!("mIV サムネイル IPC: {ipc_status}");
    println!("デバッグ用 Bearer トークン: {}", auth.bearer_printable());
    println!("接続 URL の決定元: {}", connection.source.label());
    println!(
        "Cookie Secure 判定: リクエストごと (direct TLS または X-Forwarded-Proto=https の場合のみ ON)"
    );
    if let Err(error) = console_qr::print_url_qr(&connection.base) {
        eprintln!("{error}");
        println!("接続 URL: {}", connection.base);
    }
    std::io::stdout()
        .flush()
        .map_err(|error| format!("起動情報をコンソールへ出力できません: {error}"))?;

    let state = Arc::new(AppState {
        auth,
        library,
        thumbnail_client,
        ipc_admission: IpcAdmission::new(),
        logger,
        telemetry_limiter: TelemetryLimiter::new(),
        request_sequence: AtomicU64::new(1),
        web_root: config.web_root,
    });
    let workers = HTTP_WORKER_COUNT;
    println!(
        "HTTP workers: {workers} (IPC max: {MAX_CONCURRENT_IPC}, heavy IPC max: {MAX_CONCURRENT_HEAVY_IPC})"
    );

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
