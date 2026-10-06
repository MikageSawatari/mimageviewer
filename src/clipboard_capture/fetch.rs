//! Dialog-scoped network ownership and a process-wide four-worker budget.
//! UI calls only submit work and poll the bounded result channel.
use super::data::{
    CaptureOrigin, CaptureTimestamp, capture_filename, validate_dimensions, write_motw,
};
use crossbeam_channel::{Receiver, Sender, bounded};
use sha2::{Digest, Sha256};
use std::collections::VecDeque;
use std::fs::File;
use std::io::{self, BufReader, Read, Write};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, ToSocketAddrs};
use std::path::{Path, PathBuf};
use std::sync::{
    Arc, Condvar, Mutex, OnceLock, Weak,
    atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
};
use std::time::{Duration, Instant};
use url::Url;

const WORKERS: usize = 4;
const BACKLOG: usize = 8;
const IMAGE_BYTES: u64 = 64 * 1024 * 1024;
const SESSION_BYTES: u64 = 2 * 1024 * 1024 * 1024;
const DEADLINE: Duration = Duration::from_secs(60);
static NEXT_SESSION: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Debug)]
pub(crate) enum CaptureDestination {
    Monthly(Option<PathBuf>),
    Direct(PathBuf),
}

#[derive(Clone, Debug)]
pub(crate) struct FetchedImage {
    pub index: usize,
    pub width: u32,
    pub height: u32,
    pub thumbnail: egui::ColorImage,
    pub content_hash: [u8; 32],
    pub image_url: String,
    pub extension: String,
    path: PathBuf,
    session_id: u64,
}

impl FetchedImage {
    /// Display-only fixture: it owns no session/file and cannot be saved.
    pub(crate) fn snapshot_preview() -> Self {
        Self {
            index: 0,
            width: 1200,
            height: 900,
            thumbnail: egui::ColorImage::filled([160, 120], egui::Color32::from_rgb(92, 142, 168)),
            content_hash: [0; 32],
            image_url: "https://example.com/image.png".into(),
            extension: "png".into(),
            path: PathBuf::new(),
            session_id: 0,
        }
    }
}

#[derive(Debug, Default)]
pub(crate) struct SaveSummary {
    pub saved: usize,
    pub failed: usize,
    pub metadata_failed: usize,
    pub errors: Vec<String>,
    pub paths: Vec<PathBuf>,
}

#[derive(Debug)]
pub(crate) enum CaptureFetchEvent {
    Fetched {
        index: usize,
        result: Result<FetchedImage, String>,
    },
    FetchComplete,
    SaveProgress {
        completed: usize,
        total: usize,
    },
    SaveComplete(SaveSummary),
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase {
    Fetching,
    Saving,
    Finished,
}

struct Session {
    id: u64,
    directory: PathBuf,
    page_url: String,
    destination: CaptureDestination,
    timestamp: CaptureTimestamp,
    agent: OnceLock<ureq::Agent>,
    phase: Mutex<Phase>,
    cancelled: AtomicBool,
    cancel_sender: Mutex<Option<Sender<()>>>,
    cancel_receiver: Receiver<()>,
    results: Sender<CaptureFetchEvent>,
    ctx: egui::Context,
    pending: AtomicUsize,
    bytes: AtomicU64,
    queue: Weak<Queue>,
}

impl Session {
    fn fetching(&self) -> bool {
        !self.cancelled.load(Ordering::Acquire)
            && *self.phase.lock().unwrap_or_else(|p| p.into_inner()) == Phase::Fetching
    }
    fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
        self.cancel_sender
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .take();
    }
    fn publish(&self, event: CaptureFetchEvent) {
        if self.cancelled.load(Ordering::Acquire) {
            return;
        }
        // Wake before a potentially backpressured send, so a full backlog is
        // drained even if no other UI input arrives.
        self.ctx.request_repaint();
        crossbeam_channel::select! {
            send(self.results, event) -> _ => { self.ctx.request_repaint(); },
            recv(self.cancel_receiver) -> _ => {},
        }
    }
    fn fetched(&self) {
        if self.pending.fetch_sub(1, Ordering::AcqRel) == 1 && self.fetching() {
            self.publish(CaptureFetchEvent::FetchComplete);
        }
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        // This is the last session owner: no fetch/save job can still touch the
        // directory. The last owner may be the UI, so cleanup is always deferred.
        let directory = self.directory.clone();
        if let Err(error) = std::thread::Builder::new()
            .name("clipboard-fetch-cleanup".into())
            .spawn(move || {
                if let Err(error) = remove_owned_tree(&directory) {
                    log_error(&error);
                }
            })
        {
            log_error(&format!("cleanup worker: {error}"));
        }
    }
}

pub(crate) struct CaptureFetchSession {
    owner: Arc<Session>,
    pub results: Receiver<CaptureFetchEvent>,
}

impl CaptureFetchSession {
    pub fn cancel(&self) {
        self.owner.cancel();
    }
    pub fn save(&self, selected: Vec<FetchedImage>) -> Result<(), String> {
        if selected.iter().any(|item| {
            item.session_id != self.owner.id
                || item.path.parent() != Some(self.owner.directory.as_path())
        }) {
            return Err("別の取り込みの画像は保存できません".into());
        }
        let mut phase = self.owner.phase.lock().unwrap_or_else(|p| p.into_inner());
        if *phase != Phase::Fetching || self.owner.cancelled.load(Ordering::Acquire) {
            return Err("この取り込みは終了しています".into());
        }
        let queue = self
            .owner
            .queue
            .upgrade()
            .ok_or("取得サービスは終了しています")?;
        *phase = Phase::Saving;
        if !queue.push(Job::Save(self.owner.clone(), selected)) {
            *phase = Phase::Finished;
            return Err("取得サービスは終了しています".into());
        }
        Ok(())
    }
}
impl Drop for CaptureFetchSession {
    fn drop(&mut self) {
        self.cancel();
    }
}

enum Job {
    Startup,
    Start(Arc<Session>, Vec<String>),
    Fetch(Arc<Session>, usize, String),
    Save(Arc<Session>, Vec<FetchedImage>),
}
struct QueueState {
    jobs: VecDeque<Job>,
    shutdown: bool,
    sessions: Vec<Weak<Session>>,
}
struct Queue {
    state: Mutex<QueueState>,
    ready: Condvar,
    directory: PathBuf,
    initialized: OnceLock<Result<(), String>>,
}
impl Queue {
    fn push(&self, job: Job) -> bool {
        let mut state = self.state.lock().unwrap_or_else(|p| p.into_inner());
        if state.shutdown {
            return false;
        }
        state.jobs.push_back(job);
        self.ready.notify_one();
        true
    }
    fn pop(&self) -> Option<Job> {
        let mut state = self.state.lock().unwrap_or_else(|p| p.into_inner());
        loop {
            if state.shutdown {
                return None;
            }
            if let Some(job) = state.jobs.pop_front() {
                return Some(job);
            }
            state = self.ready.wait(state).unwrap_or_else(|p| p.into_inner());
        }
    }
    fn initialize(&self) -> &Result<(), String> {
        self.initialized.get_or_init(|| {
            remove_owned_tree(&self.directory)?;
            std::fs::create_dir_all(&self.directory).map_err(|e| e.to_string())
        })
    }
}

pub(crate) struct CaptureFetchService {
    queue: Arc<Queue>,
}
impl CaptureFetchService {
    pub fn new(data_dir: PathBuf) -> Result<Self, String> {
        let service = Self {
            queue: Arc::new(Queue {
                state: Mutex::new(QueueState {
                    jobs: VecDeque::from([Job::Startup]),
                    shutdown: false,
                    sessions: Vec::new(),
                }),
                ready: Condvar::new(),
                directory: data_dir.join("tmp/clipboard-capture"),
                initialized: OnceLock::new(),
            }),
        };
        for index in 0..WORKERS {
            let queue = service.queue.clone();
            std::thread::Builder::new()
                .name(format!("clipboard-fetch-{index}"))
                .spawn(move || {
                    while let Some(job) = queue.pop() {
                        run_job(&queue, job);
                    }
                })
                .map_err(|e| format!("画像取得 worker を開始できませんでした: {e}"))?;
        }
        Ok(service)
    }
    pub fn start(
        &self,
        page_url: String,
        candidates: Vec<String>,
        destination: CaptureDestination,
        timestamp: CaptureTimestamp,
        ctx: egui::Context,
    ) -> CaptureFetchSession {
        let id = NEXT_SESSION.fetch_add(1, Ordering::Relaxed);
        let (results_tx, results) = bounded(BACKLOG);
        let (cancel_sender, cancel_receiver) = bounded(0);
        let owner = Arc::new(Session {
            id,
            directory: self
                .queue
                .directory
                .join(format!("{}-{id}", std::process::id())),
            page_url,
            destination,
            timestamp,
            agent: OnceLock::new(),
            phase: Mutex::new(Phase::Fetching),
            cancelled: AtomicBool::new(false),
            cancel_sender: Mutex::new(Some(cancel_sender)),
            cancel_receiver,
            results: results_tx,
            ctx,
            pending: AtomicUsize::new(candidates.len()),
            bytes: AtomicU64::new(0),
            queue: Arc::downgrade(&self.queue),
        });
        {
            let mut state = self.queue.state.lock().unwrap_or_else(|p| p.into_inner());
            state.sessions.retain(|s| s.strong_count() != 0);
            state.sessions.push(Arc::downgrade(&owner));
        }
        self.queue.push(Job::Start(owner.clone(), candidates));
        CaptureFetchSession { owner, results }
    }
}
impl Drop for CaptureFetchService {
    fn drop(&mut self) {
        let mut state = self.queue.state.lock().unwrap_or_else(|p| p.into_inner());
        state.shutdown = true;
        for session in &state.sessions {
            if let Some(session) = session.upgrade() {
                session.cancel();
            }
        }
        state.jobs.clear();
        self.queue.ready.notify_all();
    }
}

fn run_job(queue: &Arc<Queue>, job: Job) {
    match job {
        Job::Startup => {
            if let Err(error) = queue.initialize() {
                log_error(error);
            }
        }
        Job::Start(session, candidates) => {
            if !session.fetching() {
                return;
            }
            let initialized = queue
                .initialize()
                .clone()
                .and_then(|()| std::fs::create_dir(&session.directory).map_err(|e| e.to_string()));
            if let Err(error) = initialized {
                for index in 0..candidates.len() {
                    session.publish(CaptureFetchEvent::Fetched {
                        index,
                        result: Err(error.clone()),
                    });
                }
                session.publish(CaptureFetchEvent::FetchComplete);
                return;
            }
            let class = page_class(&session.page_url, &bare_resolve);
            let agent = ureq::AgentBuilder::new()
                .redirects(0)
                .try_proxy_from_env(false)
                .timeout_connect(Duration::from_secs(10))
                .user_agent(&format!("mImageViewer/{}", env!("CARGO_PKG_VERSION")))
                .resolver(move |netloc: &str| filtered_resolve(class, netloc, &bare_resolve))
                .build();
            let _ = session.agent.set(agent);
            if candidates.is_empty() {
                session.publish(CaptureFetchEvent::FetchComplete);
            }
            for (index, url) in candidates.into_iter().enumerate() {
                queue.push(Job::Fetch(session.clone(), index, url));
            }
        }
        Job::Fetch(session, index, url) => {
            if session.fetching() {
                let result = fetch_image(&session, index, &url, DEADLINE, IMAGE_BYTES);
                if session.fetching() {
                    session.publish(CaptureFetchEvent::Fetched { index, result });
                }
            }
            session.fetched();
        }
        Job::Save(session, selected) => {
            let summary = save_selected(&session, &selected);
            *session.phase.lock().unwrap_or_else(|p| p.into_inner()) = Phase::Finished;
            session.publish(CaptureFetchEvent::SaveComplete(summary));
        }
    }
}

fn log_error(error: &str) {
    crate::logger::log(format!("clipboard_capture: fetch: {error}"));
}

// No reparse point is traversed, including under an otherwise owned directory.
fn remove_owned_tree(path: &Path) -> Result<(), String> {
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e.to_string()),
    };
    if is_reparse(&metadata) {
        return Err(format!(
            "一時フォルダのリンクは削除しません: {}",
            path.display()
        ));
    }
    if !metadata.is_dir() {
        return Err("一時フォルダがディレクトリではありません".into());
    }
    for entry in std::fs::read_dir(path).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        let metadata = std::fs::symlink_metadata(entry.path()).map_err(|e| e.to_string())?;
        if is_reparse(&metadata) {
            return Err("一時フォルダ内のリンクは削除しません".into());
        }
        if metadata.is_dir() {
            remove_owned_tree(&entry.path())?;
        } else {
            std::fs::remove_file(entry.path()).map_err(|e| e.to_string())?;
        }
    }
    std::fs::remove_dir(path).map_err(|e| e.to_string())
}
fn is_reparse(metadata: &std::fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        metadata.file_attributes() & 0x400 != 0
    }
    #[cfg(not(windows))]
    {
        metadata.file_type().is_symlink()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum AddressClass {
    Public,
    Private,
    Local,
    Denied,
}
fn address_class(address: IpAddr) -> AddressClass {
    match address {
        IpAddr::V4(ip) => {
            let [a, b, _, _] = ip.octets();
            if ip.is_unspecified() || ip.is_multicast() || ip == Ipv4Addr::BROADCAST {
                AddressClass::Denied
            } else if a == 127 || (a == 198 && (b == 18 || b == 19)) {
                AddressClass::Local
            } else if ip.is_private() || ip.is_link_local() || (a == 100 && (64..=127).contains(&b))
            {
                AddressClass::Private
            } else {
                AddressClass::Public
            }
        }
        IpAddr::V6(ip) => {
            if let Some(ip) = ip.to_ipv4_mapped() {
                return address_class(IpAddr::V4(ip));
            }
            let first = ip.segments()[0];
            if ip.is_unspecified() || ip.is_multicast() {
                AddressClass::Denied
            } else if ip.is_loopback() {
                AddressClass::Local
            } else if first & 0xfe00 == 0xfc00 || first & 0xffc0 == 0xfe80 {
                AddressClass::Private
            } else {
                AddressClass::Public
            }
        }
    }
}
fn localhost(host: &str) -> bool {
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    host == "localhost" || host.ends_with(".localhost")
}
fn bare_resolve(netloc: &str) -> io::Result<Vec<SocketAddr>> {
    // ureq passes [IPv6]:port, and hostnames always include the port.
    let (host, port) = split_netloc(netloc)?;
    if localhost(&host) {
        return Ok(vec![
            SocketAddr::new(Ipv4Addr::LOCALHOST.into(), port),
            SocketAddr::new(Ipv6Addr::LOCALHOST.into(), port),
        ]);
    }
    netloc
        .to_socket_addrs()
        .map(|addresses| addresses.collect())
}
fn split_netloc(netloc: &str) -> io::Result<(String, u16)> {
    let (host, port) = netloc
        .rsplit_once(':')
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "missing port"))?;
    let port = port
        .parse()
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "invalid port"))?;
    Ok((
        host.trim_start_matches('[')
            .trim_end_matches(']')
            .to_owned(),
        port,
    ))
}
fn page_class(page: &str, resolve: &dyn Fn(&str) -> io::Result<Vec<SocketAddr>>) -> AddressClass {
    let Ok(url) = Url::parse(page) else {
        return AddressClass::Public;
    };
    let Some(host) = url.host_str() else {
        return AddressClass::Public;
    };
    if localhost(host) {
        return AddressClass::Local;
    }
    let Some(port) = url.port_or_known_default() else {
        return AddressClass::Public;
    };
    resolve(&format!("{host}:{port}"))
        .ok()
        .and_then(|addresses| {
            addresses
                .into_iter()
                .map(|a| address_class(a.ip()))
                .filter(|c| *c != AddressClass::Denied)
                .min()
        })
        .unwrap_or(AddressClass::Public)
}
fn filtered_resolve(
    class: AddressClass,
    netloc: &str,
    resolve: &dyn Fn(&str) -> io::Result<Vec<SocketAddr>>,
) -> io::Result<Vec<SocketAddr>> {
    let (host, port) = split_netloc(netloc)?;
    let addresses = if localhost(&host) {
        vec![
            SocketAddr::new(Ipv4Addr::LOCALHOST.into(), port),
            SocketAddr::new(Ipv6Addr::LOCALHOST.into(), port),
        ]
    } else {
        resolve(netloc)?
    };
    let addresses: Vec<_> = addresses
        .into_iter()
        .filter(|a| {
            let target = address_class(a.ip());
            target != AddressClass::Denied && target <= class
        })
        .collect();
    if addresses.is_empty() {
        Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "ページより内側のネットワークには接続できません",
        ))
    } else {
        Ok(addresses)
    }
}

fn validate_http_url(url: &Url) -> Result<(), String> {
    if !matches!(url.scheme(), "http" | "https") {
        return Err("対応していない URL です".into());
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err("認証情報を含む URL は取得できません".into());
    }
    const BAD_PORTS: &[u16] = &[
        0, 1, 7, 9, 11, 13, 15, 17, 19, 20, 21, 22, 23, 25, 37, 42, 43, 53, 69, 77, 79, 87, 95,
        101, 102, 103, 104, 109, 110, 111, 113, 115, 117, 119, 123, 135, 137, 139, 143, 161, 179,
        389, 427, 465, 512, 513, 514, 515, 526, 530, 531, 532, 540, 548, 554, 556, 563, 587, 601,
        636, 989, 990, 993, 995, 1719, 1720, 1723, 2049, 3659, 4045, 4190, 5060, 5061, 6000, 6566,
        6665, 6666, 6667, 6668, 6669, 6679, 6697, 10080,
    ];
    let port = url
        .port_or_known_default()
        .ok_or("URL のポートが不正です")?;
    if BAD_PORTS.contains(&port) {
        return Err("ブラウザが禁止しているポートです".into());
    }
    Ok(())
}
fn referer(page: &str, target: &Url) -> Option<String> {
    let mut page = Url::parse(page).ok()?;
    if !matches!(page.scheme(), "http" | "https")
        || (page.scheme() == "https" && target.scheme() == "http")
    {
        return None;
    }
    page.set_username("").ok()?;
    page.set_password(None).ok()?;
    page.set_fragment(None);
    if page.origin() == target.origin() {
        Some(page.into())
    } else {
        Some(format!("{}/", page.origin().ascii_serialization()))
    }
}

fn fetch_image(
    session: &Session,
    index: usize,
    source: &str,
    timeout: Duration,
    byte_limit: u64,
) -> Result<FetchedImage, String> {
    let deadline = Instant::now() + timeout;
    let path = session.directory.join(format!("{index}.image"));
    let mut output = File::create(&path).map_err(|e| e.to_string())?;
    let mut budget = DownloadBudget {
        session,
        reserved: 0,
        retained: false,
    };
    let result = (|| {
        let final_url = if source.starts_with("data:") {
            stream_data(source, &mut output, &mut budget, byte_limit, deadline)?;
            source.to_owned()
        } else {
            let mut url = Url::parse(source).map_err(|_| "画像 URL が不正です")?;
            let agent = session.agent.get().ok_or("取得の準備ができませんでした")?;
            let mut redirects = 0;
            loop {
                if !session.fetching() {
                    return Err("取り込みを取り消しました".into());
                }
                validate_http_url(&url)?;
                let remaining = deadline
                    .checked_duration_since(Instant::now())
                    .ok_or("取得の期限を超えました")?;
                let mut request = agent.get(url.as_str()).timeout(remaining);
                if let Some(value) = referer(&session.page_url, &url) {
                    request = request.set("Referer", &value);
                }
                let response = match request.call() {
                    Ok(response) => response,
                    Err(ureq::Error::Status(status, _)) => {
                        return Err(format!("取得できませんでした ({status})"));
                    }
                    Err(error) => return Err(format!("取得できませんでした: {error}")),
                };
                if matches!(response.status(), 301 | 302 | 303 | 307 | 308) {
                    if redirects >= 5 {
                        return Err("リダイレクトが多すぎます".into());
                    }
                    let location = response
                        .header("Location")
                        .ok_or("リダイレクト先がありません")?;
                    url = url.join(location).map_err(|_| "リダイレクト先が不正です")?;
                    redirects += 1;
                    continue;
                }
                if response.status() < 200 || response.status() >= 300 {
                    return Err(format!("取得できませんでした ({})", response.status()));
                }
                if response
                    .header("Content-Length")
                    .and_then(|v| v.parse::<u64>().ok())
                    .is_some_and(|n| n > byte_limit)
                {
                    return Err("画像が 64 MiB を超えています".into());
                }
                stream_bytes(
                    &mut response.into_reader(),
                    &mut output,
                    &mut budget,
                    byte_limit,
                    deadline,
                )?;
                break url.to_string();
            }
        };
        output.flush().map_err(|e| e.to_string())?;
        drop(output);
        if !session.fetching() {
            return Err("取り込みを取り消しました".into());
        }
        let (decoded, extension) = decode_fetched(&path)?;
        let width = decoded.width();
        let height = decoded.height();
        let rgba = decoded.into_rgba8();
        let mut hash = Sha256::new();
        hash.update(width.to_le_bytes());
        hash.update(height.to_le_bytes());
        hash.update(rgba.as_raw());
        let thumbnail = image::DynamicImage::ImageRgba8(rgba)
            .thumbnail(192, 192)
            .to_rgba8();
        Ok(FetchedImage {
            index,
            width,
            height,
            thumbnail: egui::ColorImage::from_rgba_unmultiplied(
                [thumbnail.width() as usize, thumbnail.height() as usize],
                thumbnail.as_raw(),
            ),
            content_hash: hash.finalize().into(),
            image_url: final_url,
            extension,
            path: path.clone(),
            session_id: session.id,
        })
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&path);
    } else {
        budget.retained = true;
    }
    result
}

fn reserve_bytes(session: &Session, count: u64) -> Result<(), String> {
    session
        .bytes
        .fetch_update(Ordering::AcqRel, Ordering::Acquire, |old| {
            old.checked_add(count).filter(|n| *n <= SESSION_BYTES)
        })
        .map(|_| ())
        .map_err(|_| "上限のため省略しました (一時保存 2 GiB)".into())
}
struct DownloadBudget<'a> {
    session: &'a Session,
    reserved: u64,
    retained: bool,
}
impl Drop for DownloadBudget<'_> {
    fn drop(&mut self) {
        if !self.retained {
            self.session
                .bytes
                .fetch_sub(self.reserved, Ordering::AcqRel);
        }
    }
}
fn stream_bytes(
    input: &mut dyn Read,
    output: &mut dyn Write,
    budget: &mut DownloadBudget<'_>,
    limit: u64,
    deadline: Instant,
) -> Result<(), String> {
    let mut bytes = 0u64;
    let mut buffer = [0; 64 * 1024];
    loop {
        if !budget.session.fetching() {
            return Err("取り込みを取り消しました".into());
        }
        if Instant::now() >= deadline {
            return Err("取得の期限を超えました".into());
        }
        let count = input
            .read(&mut buffer)
            .map_err(|e| format!("画像を読み取れませんでした: {e}"))?;
        if Instant::now() >= deadline {
            return Err("取得の期限を超えました".into());
        }
        if count == 0 {
            break;
        }
        bytes = bytes
            .checked_add(count as u64)
            .ok_or("画像が大きすぎます")?;
        if bytes > limit {
            return Err("画像が 64 MiB を超えています".into());
        }
        reserve_bytes(budget.session, count as u64)?;
        budget.reserved += count as u64;
        output
            .write_all(&buffer[..count])
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}
fn stream_data(
    source: &str,
    output: &mut dyn Write,
    budget: &mut DownloadBudget<'_>,
    limit: u64,
    deadline: Instant,
) -> Result<(), String> {
    let (header, encoded) = source
        .strip_prefix("data:")
        .and_then(|s| s.split_once(','))
        .ok_or("data URL が不正です")?;
    if header
        .split(';')
        .next()
        .is_none_or(|mime| !mime.to_ascii_lowercase().starts_with("image/"))
    {
        return Err("data URL が画像ではありません".into());
    }
    if header
        .split(';')
        .any(|part| part.eq_ignore_ascii_case("base64"))
    {
        // Decode incrementally; the encoded candidate already belongs to the
        // HTML snapshot, but no downloaded byte vector is retained in memory.
        let input = PercentReader {
            bytes: encoded.as_bytes(),
            position: 0,
        };
        let mut decoder =
            base64::read::DecoderReader::new(input, &base64::engine::general_purpose::STANDARD);
        stream_bytes(&mut decoder, output, budget, limit, deadline)
    } else {
        stream_bytes(
            &mut PercentReader {
                bytes: encoded.as_bytes(),
                position: 0,
            },
            output,
            budget,
            limit,
            deadline,
        )
    }
}
struct PercentReader<'a> {
    bytes: &'a [u8],
    position: usize,
}
impl Read for PercentReader<'_> {
    fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
        let mut count = 0;
        while count < output.len() && self.position < self.bytes.len() {
            let byte = self.bytes[self.position];
            self.position += 1;
            output[count] = if byte == b'%' {
                let pair = self
                    .bytes
                    .get(self.position..self.position + 2)
                    .ok_or_else(|| {
                        io::Error::new(io::ErrorKind::InvalidData, "invalid percent encoding")
                    })?;
                self.position += 2;
                let hex = std::str::from_utf8(pair)
                    .ok()
                    .and_then(|s| u8::from_str_radix(s, 16).ok())
                    .ok_or_else(|| {
                        io::Error::new(io::ErrorKind::InvalidData, "invalid percent encoding")
                    })?;
                hex
            } else {
                byte
            };
            count += 1;
        }
        Ok(count)
    }
}

fn decode_fetched(path: &Path) -> Result<(image::DynamicImage, String), String> {
    let mut header = [0; 64];
    let count = File::open(path)
        .and_then(|mut f| f.read(&mut header))
        .map_err(|e| e.to_string())?;
    let header = &header[..count];
    let special =
        if header.starts_with(b"\xff\x0a") || header.starts_with(b"\0\0\0\x0cJXL \r\n\x87\n") {
            Some("jxl")
        } else if header.len() >= 12 && &header[4..8] == b"ftyp" {
            match &header[8..12] {
                b"avif" | b"avis" => Some("avif"),
                b"heic" | b"heix" | b"hevc" | b"hevx" | b"mif1" | b"msf1" => Some("heic"),
                _ => None,
            }
        } else {
            None
        };
    if let Some(extension) = special {
        return decode_wic_bounded(path).map(|image| (image, extension.into()));
    }
    let format =
        image::guess_format(header).map_err(|_| "取得した内容は対応する画像ではありません")?;
    // image enables more codecs than mIV recognizes in its normal listing.
    // Keep this boundary to its built-in image formats (folder_tree §extensions);
    // the WIC-only modern formats were handled above.
    if !matches!(
        format,
        image::ImageFormat::Jpeg
            | image::ImageFormat::Png
            | image::ImageFormat::Gif
            | image::ImageFormat::WebP
            | image::ImageFormat::Bmp
            | image::ImageFormat::Tiff
    ) {
        return Err("取得した内容は対応する画像ではありません".into());
    }
    if format == image::ImageFormat::Png && header.len() >= 24 && &header[12..16] == b"IHDR" {
        validate_dimensions(
            u32::from_be_bytes(header[16..20].try_into().unwrap()),
            u32::from_be_bytes(header[20..24].try_into().unwrap()),
        )?;
    }
    // Header probing has no pixel allocation. Never fallback after a failed
    // dimension/budget check or decoder error.
    let dimensions = image::ImageReader::with_format(
        BufReader::new(File::open(path).map_err(|e| e.to_string())?),
        format,
    )
    .into_dimensions()
    .map_err(|e| format!("画像の寸法を読み取れませんでした: {e}"))?;
    validate_dimensions(dimensions.0, dimensions.1)?;
    let mut reader = image::ImageReader::with_format(
        BufReader::new(File::open(path).map_err(|e| e.to_string())?),
        format,
    );
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(32768);
    limits.max_image_height = Some(32768);
    limits.max_alloc = Some(1024 * 1024 * 1024);
    reader.limits(limits);
    let image = reader
        .decode()
        .map_err(|e| format!("画像を読み取れませんでした: {e}"))?;
    let extension = format
        .extensions_str()
        .first()
        .ok_or("画像形式を判定できませんでした")?
        .to_string();
    Ok((image, extension))
}

#[cfg(not(windows))]
fn decode_wic_bounded(_: &Path) -> Result<image::DynamicImage, String> {
    Err("画像のコーデックを利用できません".into())
}
#[cfg(windows)]
fn decode_wic_bounded(path: &Path) -> Result<image::DynamicImage, String> {
    use windows::Win32::{
        Foundation::GENERIC_READ,
        Graphics::Imaging::{
            CLSID_WICImagingFactory, GUID_WICPixelFormat32bppRGBA, IWICBitmapSource,
            IWICImagingFactory, WICBitmapDitherTypeNone, WICBitmapPaletteTypeCustom,
            WICDecodeMetadataCacheOnDemand,
        },
        System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance},
    };
    use windows::core::{GUID, Interface, PCWSTR};
    let _com = crate::wic_decoder::ComScope::init();
    let operation = || -> Result<image::DynamicImage, String> {
        unsafe {
            let factory: IWICImagingFactory =
                CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER)
                    .map_err(|e| e.to_string())?;
            let wide: Vec<_> = path
                .as_os_str()
                .to_string_lossy()
                .encode_utf16()
                .chain(Some(0))
                .collect();
            let decoder = factory
                .CreateDecoderFromFilename(
                    PCWSTR(wide.as_ptr()),
                    Some(&GUID::zeroed()),
                    GENERIC_READ,
                    WICDecodeMetadataCacheOnDemand,
                )
                .map_err(|e| e.to_string())?;
            let frame = decoder.GetFrame(0).map_err(|e| e.to_string())?;
            let (mut width, mut height) = (0, 0);
            frame
                .GetSize(&mut width, &mut height)
                .map_err(|e| e.to_string())?;
            validate_dimensions(width, height)?;
            let converter = factory.CreateFormatConverter().map_err(|e| e.to_string())?;
            let source: IWICBitmapSource = frame.cast().map_err(|e| e.to_string())?;
            converter
                .Initialize(
                    &source,
                    &GUID_WICPixelFormat32bppRGBA,
                    WICBitmapDitherTypeNone,
                    None,
                    0.0,
                    WICBitmapPaletteTypeCustom,
                )
                .map_err(|e| e.to_string())?;
            let stride = width.checked_mul(4).ok_or("画像が大きすぎます")?;
            let mut pixels = vec![0; stride as usize * height as usize];
            converter
                .CopyPixels(std::ptr::null(), stride, &mut pixels)
                .map_err(|e| e.to_string())?;
            let rgba = image::RgbaImage::from_raw(width, height, pixels)
                .ok_or("画像を読み取れませんでした")?;
            Ok(image::DynamicImage::ImageRgba8(rgba))
        }
    };
    operation()
}

fn save_selected(session: &Session, selected: &[FetchedImage]) -> SaveSummary {
    let mut summary = SaveSummary::default();
    let directory = match &session.destination {
        CaptureDestination::Monthly(path) => {
            // Resolving the default shell folder is already shared by S1 and
            // runs on its dedicated worker; only this save worker may wait.
            super::start_default_destination_resolution(&session.ctx);
            match super::resolve_save_destination(path, || {
                !session.cancelled.load(Ordering::Acquire)
            }) {
                Some(Ok(path)) => path.join(&session.timestamp.month),
                error => {
                    summary.failed = selected.len();
                    summary.errors.push(
                        error
                            .and_then(Result::err)
                            .unwrap_or_else(|| "保存を取り消しました".into()),
                    );
                    return summary;
                }
            }
        }
        CaptureDestination::Direct(path) => path.clone(),
    };
    if let Err(error) = std::fs::create_dir_all(&directory) {
        summary.failed = selected.len();
        summary
            .errors
            .push(format!("保存先を作成できませんでした: {error}"));
        return summary;
    }
    let domain = Url::parse(&session.page_url)
        .ok()
        .and_then(|u| u.host_str().map(str::to_owned))
        .or_else(|| {
            selected
                .first()
                .and_then(|image| Url::parse(&image.image_url).ok())
                .and_then(|u| u.host_str().map(str::to_owned))
        });
    // Pick one collision prefix for the whole copy before publishing any item.
    let mut collision = 1;
    while selected.iter().enumerate().any(|(i, item)| {
        directory
            .join(capture_filename(
                &session.timestamp,
                domain.as_deref(),
                i + 1,
                selected.len(),
                collision,
                &item.extension,
            ))
            .exists()
    }) {
        collision += 1;
    }
    for (index, item) in selected.iter().enumerate() {
        let result = (|| -> Result<PathBuf, String> {
            let path = directory.join(capture_filename(
                &session.timestamp,
                domain.as_deref(),
                index + 1,
                selected.len(),
                collision,
                &item.extension,
            ));
            let mut part = tempfile::Builder::new()
                .prefix(".miv-clipboard-")
                .suffix(".part")
                .tempfile_in(&directory)
                .map_err(|e| e.to_string())?;
            io::copy(
                &mut File::open(&item.path).map_err(|e| e.to_string())?,
                &mut part,
            )
            .map_err(|e| e.to_string())?;
            part.flush().map_err(|e| e.to_string())?;
            part.persist_noclobber(&path)
                .map_err(|e| format!("画像を確定できませんでした: {}", e.error))?;
            if let Err(error) = write_motw(
                &path,
                &CaptureOrigin {
                    page_url: Some(session.page_url.clone()),
                    image_url: Some(item.image_url.clone()),
                },
            ) {
                log_error(&error);
                summary.metadata_failed += 1;
                summary.errors.push(error);
            }
            Ok(path)
        })();
        match result {
            Ok(path) => {
                summary.saved += 1;
                summary.paths.push(path);
            }
            Err(error) => {
                log_error(&error);
                summary.failed += 1;
                summary.errors.push(error);
            }
        }
        session.publish(CaptureFetchEvent::SaveProgress {
            completed: index + 1,
            total: selected.len(),
        });
    }
    summary
}

#[cfg(test)]
#[path = "fetch_tests.rs"]
mod tests;
