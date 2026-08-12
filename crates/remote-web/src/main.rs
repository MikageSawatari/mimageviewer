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
mod web_assets;

use std::io::Write as _;
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::AtomicU64;

use auth::{AuthService, AuthToken};
use config::{Config, default_data_dir};
use connection_url::choose_connection_url;
use diagnostics::{DiagnosticsLogger, resolve_file_path};
use http::{
    AppState, HTTP_WORKER_COUNT, IpcAdmission, MAX_CONCURRENT_HEAVY_IPC, MAX_CONCURRENT_IPC,
    MAX_CONCURRENT_STREAM_IPC, SessionActivityNotifier, TelemetryLimiter,
};
use ipc_client::ThumbnailClient;
use mimageviewer_ipc::{RemoteWebConnectionInfo, load_pin_file};
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
    let auth_path = resolve_file_path(&config.auth_path, "認証ファイル")?;
    let auth_record = load_pin_file(&auth_path)?;
    let bearer =
        AuthToken::generate().map_err(|error| format!("認証トークンを生成できません: {error}"))?;
    let auth = AuthService::new(auth_record, bearer)?;
    let log_secrets = auth.permanent_log_secrets();
    let logger = DiagnosticsLogger::open(&config.log_path, &protected_roots, &log_secrets)?;
    if logger.path() == auth_path {
        return Err("--log と --auth-file には別のファイルを指定してください".to_owned());
    }
    let library = Library::load(&config.data_dir)
        .map_err(|error| format!("settings.db を read-only で読み込めません: {error:?}"))?;
    let address = SocketAddr::new(config.bind, config.port);
    let connection = choose_connection_url(config.public_url.as_deref(), address)?;
    let thumbnail_client = Arc::new(ThumbnailClient::new());
    thumbnail_client.set_remote_web_connection_info(RemoteWebConnectionInfo {
        public_url: connection.base.clone(),
        tailscale_serve: connection.tailscale_serve,
        tailscale_serve_conflict: connection.tailscale_serve_conflict,
        tailscale_https_certificate: connection.tailscale_https_certificate,
        tailscale_key_expiry_unix_seconds: connection.tailscale_key_expiry_unix_seconds,
    });
    let ipc_status = match thumbnail_client.probe() {
        Ok(()) => "接続済み".to_owned(),
        Err(error) => format!("未接続 ({error})"),
    };
    let disconnect_grace = config
        .managed_by_core
        .then_some(ipc_client::MANAGED_CORE_RECONNECT_GRACE);
    let _ipc_maintainer = thumbnail_client.start_connection_maintainer(disconnect_grace)?;
    let server = Arc::new(
        Server::http(address).map_err(|error| format!("HTTP bind に失敗しました: {error}"))?,
    );

    println!("mIV remote PoC bind: http://{address}");
    println!("計測ログ: {}", logger.path().display());
    println!("認証ファイル: {}", auth_path.display());
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

    let session_activity = SessionActivityNotifier::start(Arc::clone(&thumbnail_client))?;
    let state = Arc::new(AppState {
        auth,
        library,
        thumbnail_client,
        session_activity,
        ipc_admission: IpcAdmission::new(),
        logger,
        telemetry_limiter: TelemetryLimiter::new(),
        request_sequence: AtomicU64::new(1),
        web_root: config.web_root,
        session_peers: Mutex::new(std::collections::HashMap::new()),
        remote_client_identities: http::RemoteClientIdentities::default(),
    });
    let workers = HTTP_WORKER_COUNT;
    println!(
        "HTTP workers: {workers} (IPC max: {MAX_CONCURRENT_IPC}, heavy IPC max: {MAX_CONCURRENT_HEAVY_IPC}, stream IPC max: {MAX_CONCURRENT_STREAM_IPC})"
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
