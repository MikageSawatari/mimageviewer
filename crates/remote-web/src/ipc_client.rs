use std::collections::{HashMap, HashSet};
use std::fmt;
use std::io::{Read, Write};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, mpsc};
use std::time::{Duration, Instant};

use mimageviewer_ipc::{
    ClientHello, ClientMessage, CollectionError, CollectionErrorCode, CollectionKind,
    CollectionPayload, CollectionRequest, CollectionResponse, ContainerPayload, ContainerRequest,
    ContainerResponse, FolderListPayload, FolderListRequest, FolderListResponse, FrameError,
    HomePayload, HomeRequest, HomeResponse, MAX_CONTROL_FRAME_BYTES, MAX_RESPONSE_FRAME_BYTES,
    MediaError, MediaErrorCode, PIPE_NAME, PROTOCOL_VERSION, PagePayload, PagePriority,
    PageRequest, PageResponse, RemoteAddress, RemoteWebConnectionInfo, RemoteWriteError,
    RemoteWriteErrorCode, RemoteWriteRequest, RemoteWriteResponse, RemoteWriteResult, RequestId,
    ServerMessage, SessionAcquireRequest, SessionPeerInfo, SessionPingRequest, SessionResponse,
    SessionStatus, ThumbnailError, ThumbnailErrorCode, ThumbnailRequest, ThumbnailResponse,
    VideoStreamControlAction, VideoStreamError, VideoStreamErrorCode, VideoStreamPlaylistKind,
    VideoStreamPlaylistPayload, VideoStreamQuality, VideoStreamResult, VideoStreamSeekPayload,
    VideoStreamSegmentIndex, VideoStreamSegmentPayload, VideoStreamStartPayload,
    VideoStreamStatePayload, read_frame, write_frame,
};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(1);
/// 実測 1.7 秒の RAW decode に約 6 倍の余裕を持たせつつ、HTTP worker を
/// 無期限に保持しない。HTTP 側の入場制限とブラウザ再試行を前提にする。
const RESPONSE_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_RETRIES: u32 = 2;
/// 本体を後から起動しても QR 用接続情報を自動通知できるよう、接続を常時監視する。
/// 500 ms polling は named pipe の blocking reader とは別で、CPU / I/O を発生させない。
const CONNECTION_HEALTH_POLL: Duration = Duration::from_millis(500);
/// 初回は素早く追従し、常時停止中の本体へ過剰に接続しないよう 5 秒で頭打ちにする。
const RECONNECT_INITIAL_DELAY: Duration = Duration::from_millis(250);
const RECONNECT_MAX_DELAY: Duration = Duration::from_secs(5);

pub struct ThumbnailClient {
    pipe_name: String,
    connection: Mutex<Option<Arc<Connection>>>,
    next_request_id: AtomicU64,
    next_connection_id: AtomicU64,
    remote_web_info: Mutex<Option<RemoteWebConnectionInfo>>,
}

pub struct ConnectionMaintainer {
    stop: Arc<AtomicBool>,
    wake: Arc<(Mutex<()>, Condvar)>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Drop for ConnectionMaintainer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        self.wake.1.notify_all();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

pub struct ThumbnailSuccess {
    pub bytes: Vec<u8>,
    pub retry_count: u32,
    pub retry_statuses: Vec<String>,
    pub connection_id: u64,
}

pub struct IpcSuccess<T> {
    pub value: T,
    pub retry_count: u32,
    pub retry_statuses: Vec<String>,
    pub connection_id: u64,
}

pub struct ClientFailure {
    pub error: ClientError,
    pub retry_count: u32,
    pub retry_statuses: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct ProtocolFailure {
    pub stage: &'static str,
    pub kind: &'static str,
    pub os_error: Option<i32>,
    detail: String,
}

impl ProtocolFailure {
    #[cfg(test)]
    pub(crate) fn new(
        stage: &'static str,
        kind: &'static str,
        os_error: Option<i32>,
        detail: impl Into<String>,
    ) -> Self {
        protocol_failure(stage, kind, os_error, detail)
    }
}

#[derive(Clone, Debug)]
pub enum ClientError {
    Unavailable(ProtocolFailure),
    VersionMismatch { client: u32, server: u32 },
    Protocol(ProtocolFailure),
    Remote(ThumbnailError),
    CollectionRemote(CollectionError),
    MediaRemote(MediaError),
    WriteRemote(RemoteWriteError),
    SessionRemote(SessionResponse),
    VideoStreamRemote(VideoStreamError),
}

impl ClientError {
    fn should_retry_internally(&self) -> bool {
        matches!(self, Self::Unavailable(_))
            || matches!(
                self,
                Self::Protocol(failure)
                    if !(failure.stage == "response_read" && failure.kind == "timeout")
            )
            || matches!(self, Self::Remote(error) if error.code == ThumbnailErrorCode::Busy)
            || matches!(self, Self::CollectionRemote(error) if error.code == CollectionErrorCode::Busy)
            || matches!(self, Self::MediaRemote(error) if error.code == MediaErrorCode::Busy)
            || matches!(self, Self::WriteRemote(error) if error.code == RemoteWriteErrorCode::Busy)
            || matches!(self, Self::VideoStreamRemote(error) if error.code == VideoStreamErrorCode::Busy)
    }

    pub fn ipc_status(&self) -> String {
        match self {
            Self::Unavailable(failure) | Self::Protocol(failure) => {
                format!("{}_{}", failure.stage, failure.kind)
            }
            Self::VersionMismatch { .. } => "protocol_version_mismatch".to_owned(),
            Self::SessionRemote(response) => {
                format!("session_{:?}", response.status).to_lowercase()
            }
            Self::Remote(error) => match error.code {
                ThumbnailErrorCode::BadRequest => "miv_bad_request",
                ThumbnailErrorCode::FavoriteNotFound => "miv_favorite_not_found",
                ThumbnailErrorCode::PathRejected => "miv_path_rejected",
                ThumbnailErrorCode::NotFound => "miv_not_found",
                ThumbnailErrorCode::Unsupported => "miv_unsupported",
                ThumbnailErrorCode::GenerationFailed => "miv_generation_failed",
                ThumbnailErrorCode::Busy => "miv_busy",
                ThumbnailErrorCode::PasswordRequired => "miv_password_required",
                ThumbnailErrorCode::PageOutOfRange => "miv_page_out_of_range",
                ThumbnailErrorCode::Internal => "miv_internal",
            }
            .to_owned(),
            Self::MediaRemote(error) => match error.code {
                MediaErrorCode::BadRequest => "miv_media_bad_request",
                MediaErrorCode::FavoriteNotFound => "miv_media_favorite_not_found",
                MediaErrorCode::PathRejected => "miv_media_path_rejected",
                MediaErrorCode::NotFound => "miv_media_not_found",
                MediaErrorCode::Unsupported => "miv_media_unsupported",
                MediaErrorCode::PasswordRequired => "miv_media_password_required",
                MediaErrorCode::PageOutOfRange => "miv_media_page_out_of_range",
                MediaErrorCode::Busy => "miv_media_busy",
                MediaErrorCode::RenderFailed => "miv_media_render_failed",
                MediaErrorCode::Internal => "miv_media_internal",
            }
            .to_owned(),
            Self::WriteRemote(error) => format!("miv_write_{:?}", error.code).to_lowercase(),
            Self::CollectionRemote(error) => match error.code {
                CollectionErrorCode::BadRequest => "miv_collection_bad_request",
                CollectionErrorCode::NotFound => "miv_collection_not_found",
                CollectionErrorCode::Busy => "miv_collection_busy",
                CollectionErrorCode::Internal => "miv_collection_internal",
            }
            .to_owned(),
            Self::VideoStreamRemote(error) => format!("miv_video_{:?}", error.code).to_lowercase(),
        }
    }

    pub fn protocol_failure(&self) -> Option<&ProtocolFailure> {
        match self {
            Self::Unavailable(failure) | Self::Protocol(failure) => Some(failure),
            _ => None,
        }
    }
}

impl fmt::Display for ClientError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unavailable(error) => write!(
                f,
                "mIV 本体の IPC に接続できません ({}:{}: {})",
                error.stage, error.kind, error.detail
            ),
            Self::VersionMismatch { client, server } => write!(
                f,
                "IPC プロトコル版が一致しません (remote-web={client}, mIV={server})"
            ),
            Self::Protocol(error) => write!(
                f,
                "IPC 通信に失敗しました ({}:{}: {})",
                error.stage, error.kind, error.detail
            ),
            Self::Remote(error) => write!(f, "mIV 本体が要求を拒否しました: {}", error.message),
            Self::CollectionRemote(error) => {
                write!(
                    f,
                    "mIV 本体が集約ビュー要求を拒否しました: {}",
                    error.message
                )
            }
            Self::MediaRemote(error) => {
                write!(f, "mIV 本体がコンテナ要求を拒否しました: {}", error.message)
            }
            Self::WriteRemote(error) => {
                write!(f, "mIV 本体が書き込み要求を拒否しました: {}", error.message)
            }
            Self::SessionRemote(response) => write!(f, "{}", response.message),
            Self::VideoStreamRemote(error) => {
                write!(f, "mIV 本体が動画要求を拒否しました: {}", error.message)
            }
        }
    }
}

impl ThumbnailClient {
    pub fn new() -> Self {
        Self {
            pipe_name: PIPE_NAME.to_owned(),
            connection: Mutex::new(None),
            next_request_id: AtomicU64::new(1),
            next_connection_id: AtomicU64::new(1),
            remote_web_info: Mutex::new(None),
        }
    }

    pub fn set_remote_web_connection_info(&self, info: RemoteWebConnectionInfo) {
        *self
            .remote_web_info
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = Some(info);
    }

    pub fn start_connection_maintainer(self: &Arc<Self>) -> Result<ConnectionMaintainer, String> {
        let stop = Arc::new(AtomicBool::new(false));
        let wake = Arc::new((Mutex::new(()), Condvar::new()));
        let worker_client = Arc::clone(self);
        let worker_stop = Arc::clone(&stop);
        let worker_wake = Arc::clone(&wake);
        let thread = std::thread::Builder::new()
            .name("remote-ipc-maintainer".to_owned())
            .spawn(move || {
                run_connection_maintainer(
                    &worker_stop,
                    || {
                        worker_client
                            .get_connection()
                            .map_err(|error| error.to_string())
                    },
                    |connection| connection.is_broken(),
                    |delay| wait_for_maintainer(&worker_stop, &worker_wake, delay),
                    |event| match event {
                        ConnectionMaintenanceEvent::ConnectFailed { error, retry_after } => {
                            eprintln!(
                                "remote-web: mIV IPC へ接続できません。{} ms 後に再試行します ({error})",
                                retry_after.as_millis()
                            );
                        }
                        ConnectionMaintenanceEvent::Connected { recovered: true } => {
                            println!(
                                "remote-web: mIV IPC に再接続し、接続 URL を通知しました"
                            );
                        }
                        ConnectionMaintenanceEvent::Connected { recovered: false } => {}
                        ConnectionMaintenanceEvent::Disconnected => {
                            eprintln!(
                                "remote-web: mIV IPC 接続が切断されました。自動再接続します"
                            );
                        }
                    },
                );
            })
            .map_err(|error| format!("IPC 常時接続 worker を開始できません: {error}"))?;
        Ok(ConnectionMaintainer {
            stop,
            wake,
            thread: Some(thread),
        })
    }

    pub fn session_acquire(
        &self,
        client_id: &str,
        peer: SessionPeerInfo,
    ) -> Result<SessionResponse, ClientFailure> {
        self.session_request(|id| ClientMessage::SessionAcquire {
            id,
            request: SessionAcquireRequest {
                client_id: client_id.to_owned(),
                peer: peer.clone(),
            },
        })
    }

    pub fn session_ping(
        &self,
        client_id: &str,
        user_active: bool,
        media_playing: bool,
    ) -> Result<SessionResponse, ClientFailure> {
        self.session_request(|id| ClientMessage::SessionPing {
            id,
            request: SessionPingRequest {
                client_id: client_id.to_owned(),
                user_active,
                media_playing,
            },
        })
    }

    pub fn session_activity(&self, client_id: &str) -> Result<SessionResponse, ClientFailure> {
        self.session_request(|id| ClientMessage::SessionActivity {
            id,
            client_id: client_id.to_owned(),
        })
    }

    fn session_request(
        &self,
        request: impl Fn(RequestId) -> ClientMessage,
    ) -> Result<SessionResponse, ClientFailure> {
        self.collection_request(request)
            .and_then(|success| match success.value {
                ServerMessage::Session { response, .. } => Ok(response),
                _ => Err(ClientFailure {
                    error: ClientError::Protocol(protocol_failure(
                        "response_route",
                        "response_type_mismatch",
                        None,
                        "session request received another response type",
                    )),
                    retry_count: success.retry_count,
                    retry_statuses: success.retry_statuses,
                }),
            })
    }

    pub fn probe(&self) -> Result<(), ClientError> {
        self.get_connection().map(|_| ())
    }

    pub fn thumbnail_address(
        &self,
        client_id: &str,
        address: RemoteAddress,
        target_px: u32,
    ) -> Result<ThumbnailSuccess, ClientFailure> {
        let request = ThumbnailRequest { address, target_px };
        run_with_retry(|| {
            let connection = self.get_connection()?;
            let connection_id = connection.id;
            let id = self.next_request_id.fetch_add(1, Ordering::Relaxed);
            match connection.request(
                id,
                ClientMessage::Thumbnail {
                    id,
                    client_id: client_id.to_owned(),
                    request: request.clone(),
                },
            ) {
                Ok(ServerMessage::Thumbnail {
                    response: ThumbnailResponse::Success { webp_bytes },
                    ..
                }) => Ok((webp_bytes, connection_id)),
                Ok(ServerMessage::Thumbnail {
                    response: ThumbnailResponse::Error(error),
                    ..
                }) => Err(ClientError::Remote(error)),
                Ok(ServerMessage::Session { response, .. }) => {
                    Err(ClientError::SessionRemote(response))
                }
                Ok(_) => Err(ClientError::Protocol(protocol_failure(
                    "response_route",
                    "response_type_mismatch",
                    None,
                    "thumbnail request received another response type",
                ))),
                Err(error) => {
                    self.invalidate_connection(connection_id);
                    Err(ClientError::Protocol(error))
                }
            }
        })
        .map(
            |RetrySuccess {
                 value: (bytes, connection_id),
                 retry_count,
                 retry_statuses,
             }| ThumbnailSuccess {
                bytes,
                retry_count,
                retry_statuses,
                connection_id,
            },
        )
        .map_err(|failure| ClientFailure {
            error: failure.error,
            retry_count: failure.retry_count,
            retry_statuses: failure.retry_statuses,
        })
    }

    pub fn home(&self, client_id: &str) -> Result<IpcSuccess<HomePayload>, ClientFailure> {
        self.collection_request(|id| ClientMessage::Home {
            id,
            client_id: client_id.to_owned(),
            request: HomeRequest,
        })
        .and_then(|success| match success.value {
            ServerMessage::Home {
                response: HomeResponse::Success(payload),
                ..
            } => Ok(IpcSuccess {
                value: payload,
                retry_count: success.retry_count,
                retry_statuses: success.retry_statuses,
                connection_id: success.connection_id,
            }),
            ServerMessage::Home {
                response: HomeResponse::Error(error),
                ..
            } => Err(ClientFailure {
                error: ClientError::CollectionRemote(error),
                retry_count: success.retry_count,
                retry_statuses: success.retry_statuses,
            }),
            _ => Err(ClientFailure {
                error: ClientError::Protocol(protocol_failure(
                    "response_route",
                    "response_type_mismatch",
                    None,
                    "home request received another response type",
                )),
                retry_count: success.retry_count,
                retry_statuses: success.retry_statuses,
            }),
        })
    }

    pub fn collection(
        &self,
        client_id: &str,
        kind: CollectionKind,
    ) -> Result<IpcSuccess<CollectionPayload>, ClientFailure> {
        self.collection_request(|id| ClientMessage::Collection {
            id,
            client_id: client_id.to_owned(),
            request: CollectionRequest { kind: kind.clone() },
        })
        .and_then(|success| match success.value {
            ServerMessage::Collection {
                response: CollectionResponse::Success(payload),
                ..
            } => Ok(IpcSuccess {
                value: payload,
                retry_count: success.retry_count,
                retry_statuses: success.retry_statuses,
                connection_id: success.connection_id,
            }),
            ServerMessage::Collection {
                response: CollectionResponse::Error(error),
                ..
            } => Err(ClientFailure {
                error: ClientError::CollectionRemote(error),
                retry_count: success.retry_count,
                retry_statuses: success.retry_statuses,
            }),
            _ => Err(ClientFailure {
                error: ClientError::Protocol(protocol_failure(
                    "response_route",
                    "response_type_mismatch",
                    None,
                    "collection request received another response type",
                )),
                retry_count: success.retry_count,
                retry_statuses: success.retry_statuses,
            }),
        })
    }
    pub fn folder_list(
        &self,
        client_id: &str,
        address: RemoteAddress,
    ) -> Result<IpcSuccess<FolderListPayload>, ClientFailure> {
        self.collection_request(|id| ClientMessage::FolderList {
            id,
            client_id: client_id.to_owned(),
            request: FolderListRequest {
                address: address.clone(),
            },
        })
        .and_then(|success| match success.value {
            ServerMessage::FolderList {
                response: FolderListResponse::Success(payload),
                ..
            } => Ok(IpcSuccess {
                value: payload,
                retry_count: success.retry_count,
                retry_statuses: success.retry_statuses,
                connection_id: success.connection_id,
            }),
            ServerMessage::FolderList {
                response: FolderListResponse::Error(error),
                ..
            } => Err(ClientFailure {
                error: ClientError::MediaRemote(error),
                retry_count: success.retry_count,
                retry_statuses: success.retry_statuses,
            }),
            _ => Err(ClientFailure {
                error: ClientError::Protocol(protocol_failure(
                    "response_route",
                    "response_type_mismatch",
                    None,
                    "folder list request received another response type",
                )),
                retry_count: success.retry_count,
                retry_statuses: success.retry_statuses,
            }),
        })
    }

    pub fn container(
        &self,
        client_id: &str,
        address: RemoteAddress,
        spread_mode: Option<mimageviewer_ipc::RemoteSpreadMode>,
        reading_direction: Option<mimageviewer_ipc::RemoteReadingDirection>,
        force_single_page: bool,
    ) -> Result<IpcSuccess<ContainerPayload>, ClientFailure> {
        self.collection_request(|id| ClientMessage::Container {
            id,
            client_id: client_id.to_owned(),
            request: ContainerRequest {
                address: address.clone(),
                spread_mode,
                reading_direction,
                force_single_page,
            },
        })
        .and_then(|success| match success.value {
            ServerMessage::Container {
                response: ContainerResponse::Success(payload),
                ..
            } => Ok(IpcSuccess {
                value: payload,
                retry_count: success.retry_count,
                retry_statuses: success.retry_statuses,
                connection_id: success.connection_id,
            }),
            ServerMessage::Container {
                response: ContainerResponse::Error(error),
                ..
            } => Err(ClientFailure {
                error: ClientError::MediaRemote(error),
                retry_count: success.retry_count,
                retry_statuses: success.retry_statuses,
            }),
            _ => Err(ClientFailure {
                error: ClientError::Protocol(protocol_failure(
                    "response_route",
                    "response_type_mismatch",
                    None,
                    "container request received another response type",
                )),
                retry_count: success.retry_count,
                retry_statuses: success.retry_statuses,
            }),
        })
    }

    pub fn page(
        &self,
        client_id: &str,
        address: RemoteAddress,
        target_px: u32,
        priority: PagePriority,
    ) -> Result<IpcSuccess<PagePayload>, ClientFailure> {
        self.collection_request(|id| ClientMessage::Page {
            id,
            client_id: client_id.to_owned(),
            request: PageRequest {
                address: address.clone(),
                target_px,
                priority,
            },
        })
        .and_then(|success| match success.value {
            ServerMessage::Page {
                response: PageResponse::Success(payload),
                ..
            } => Ok(IpcSuccess {
                value: payload,
                retry_count: success.retry_count,
                retry_statuses: success.retry_statuses,
                connection_id: success.connection_id,
            }),
            ServerMessage::Page {
                response: PageResponse::Error(error),
                ..
            } => Err(ClientFailure {
                error: ClientError::MediaRemote(error),
                retry_count: success.retry_count,
                retry_statuses: success.retry_statuses,
            }),
            _ => Err(ClientFailure {
                error: ClientError::Protocol(protocol_failure(
                    "response_route",
                    "response_type_mismatch",
                    None,
                    "page request received another response type",
                )),
                retry_count: success.retry_count,
                retry_statuses: success.retry_statuses,
            }),
        })
    }

    pub fn video_stream_start(
        &self,
        client_id: &str,
        address: RemoteAddress,
        quality: VideoStreamQuality,
    ) -> Result<IpcSuccess<VideoStreamStartPayload>, ClientFailure> {
        self.video_request(
            |id| ClientMessage::VideoStreamStart {
                id,
                client_id: client_id.to_owned(),
                address: address.clone(),
                quality,
            },
            |message| match message {
                ServerMessage::VideoStreamStart { response, .. } => Some(response),
                _ => return None,
            },
        )
    }

    pub fn video_stream_control(
        &self,
        client_id: &str,
        session: u64,
        action: VideoStreamControlAction,
    ) -> Result<IpcSuccess<SessionResponse>, ClientFailure> {
        self.video_request(
            |id| ClientMessage::VideoStreamControl {
                id,
                client_id: client_id.to_owned(),
                session,
                action: action.clone(),
            },
            |message| match message {
                ServerMessage::VideoStreamControl { response, .. } => Some(response),
                _ => return None,
            },
        )
    }

    pub fn video_stream_seek(
        &self,
        client_id: &str,
        session: u64,
        position_secs: f64,
    ) -> Result<IpcSuccess<VideoStreamSeekPayload>, ClientFailure> {
        self.video_request(
            |id| ClientMessage::VideoStreamSeek {
                id,
                client_id: client_id.to_owned(),
                session,
                position_secs,
            },
            |message| match message {
                ServerMessage::VideoStreamSeek { response, .. } => Some(response),
                _ => return None,
            },
        )
    }

    pub fn video_stream_playlist(
        &self,
        client_id: &str,
        session: u64,
        generation: u64,
        kind: VideoStreamPlaylistKind,
    ) -> Result<IpcSuccess<VideoStreamPlaylistPayload>, ClientFailure> {
        self.video_request(
            |id| ClientMessage::VideoStreamPlaylist {
                id,
                client_id: client_id.to_owned(),
                session,
                generation,
                kind,
            },
            |message| match message {
                ServerMessage::VideoStreamPlaylist { response, .. } => Some(response),
                _ => return None,
            },
        )
    }

    pub fn video_stream_segment(
        &self,
        client_id: &str,
        session: u64,
        generation: u64,
        index: VideoStreamSegmentIndex,
    ) -> Result<IpcSuccess<VideoStreamSegmentPayload>, ClientFailure> {
        self.video_request(
            |id| ClientMessage::VideoStreamSegment {
                id,
                client_id: client_id.to_owned(),
                session,
                generation,
                index,
            },
            |message| match message {
                ServerMessage::VideoStreamSegment { response, .. } => Some(response),
                _ => return None,
            },
        )
    }

    pub fn video_stream_state(
        &self,
        client_id: &str,
        session: u64,
    ) -> Result<IpcSuccess<VideoStreamStatePayload>, ClientFailure> {
        self.video_request(
            |id| ClientMessage::VideoStreamState {
                id,
                client_id: client_id.to_owned(),
                session,
            },
            |message| match message {
                ServerMessage::VideoStreamState { response, .. } => Some(response),
                _ => return None,
            },
        )
    }

    pub fn video_stream_stop(
        &self,
        client_id: &str,
        session: u64,
    ) -> Result<IpcSuccess<()>, ClientFailure> {
        self.video_request(
            |id| ClientMessage::VideoStreamStop {
                id,
                client_id: client_id.to_owned(),
                session,
            },
            |message| match message {
                ServerMessage::VideoStreamStop { response, .. } => Some(response),
                _ => return None,
            },
        )
    }

    fn video_request<T>(
        &self,
        request: impl Fn(RequestId) -> ClientMessage,
        response: impl Fn(ServerMessage) -> Option<VideoStreamResult<T>>,
    ) -> Result<IpcSuccess<T>, ClientFailure> {
        self.collection_request(request).and_then(|success| {
            let Some(routed) = response(success.value) else {
                return Err(ClientFailure {
                    error: ClientError::Protocol(protocol_failure(
                        "response_route",
                        "response_type_mismatch",
                        None,
                        "video stream request received another response type",
                    )),
                    retry_count: success.retry_count,
                    retry_statuses: success.retry_statuses,
                });
            };
            match routed {
                VideoStreamResult::Success(value) => Ok(IpcSuccess {
                    value,
                    retry_count: success.retry_count,
                    retry_statuses: success.retry_statuses,
                    connection_id: success.connection_id,
                }),
                VideoStreamResult::Error(error) => Err(ClientFailure {
                    error: ClientError::VideoStreamRemote(error),
                    retry_count: success.retry_count,
                    retry_statuses: success.retry_statuses,
                }),
            }
        })
    }

    /// 書き込みは適用済み応答を失った場合の重複実行を避けるため自動 retry しない。
    pub fn write(
        &self,
        client_id: &str,
        request: RemoteWriteRequest,
    ) -> Result<IpcSuccess<RemoteWriteResult>, ClientFailure> {
        let result = (|| {
            let connection = self.get_connection()?;
            let connection_id = connection.id;
            let id = self.next_request_id.fetch_add(1, Ordering::Relaxed);
            let response = connection.request(
                id,
                ClientMessage::Write {
                    id,
                    client_id: client_id.to_owned(),
                    request,
                },
            );
            match response {
                Ok(ServerMessage::Write {
                    response: RemoteWriteResponse::Success(result),
                    ..
                }) => Ok((result, connection_id)),
                Ok(ServerMessage::Write {
                    response: RemoteWriteResponse::Error(error),
                    ..
                }) => Err(ClientError::WriteRemote(error)),
                Ok(ServerMessage::Session { response, .. }) => {
                    Err(ClientError::SessionRemote(response))
                }
                Ok(_) => Err(ClientError::Protocol(protocol_failure(
                    "response_route",
                    "response_type_mismatch",
                    None,
                    "write request received another response type",
                ))),
                Err(error) => {
                    self.invalidate_connection(connection_id);
                    Err(ClientError::Protocol(error))
                }
            }
        })();
        result
            .map(|(value, connection_id)| IpcSuccess {
                value,
                retry_count: 0,
                retry_statuses: Vec::new(),
                connection_id,
            })
            .map_err(|error| ClientFailure {
                error,
                retry_count: 0,
                retry_statuses: Vec::new(),
            })
    }

    fn collection_request(
        &self,
        request: impl Fn(RequestId) -> ClientMessage,
    ) -> Result<IpcSuccess<ServerMessage>, ClientFailure> {
        run_with_retry(|| {
            let connection = self.get_connection()?;
            let connection_id = connection.id;
            let id = self.next_request_id.fetch_add(1, Ordering::Relaxed);
            match connection.request(id, request(id)) {
                Ok(ServerMessage::Session { response, .. })
                    if response.status != SessionStatus::Active =>
                {
                    Err(ClientError::SessionRemote(response))
                }
                Ok(response) => Ok((response, connection_id)),
                Err(error) => {
                    self.invalidate_connection(connection_id);
                    Err(ClientError::Protocol(error))
                }
            }
        })
        .map(
            |RetrySuccess {
                 value: (value, connection_id),
                 retry_count,
                 retry_statuses,
             }| IpcSuccess {
                value,
                retry_count,
                retry_statuses,
                connection_id,
            },
        )
        .map_err(|failure| ClientFailure {
            error: failure.error,
            retry_count: failure.retry_count,
            retry_statuses: failure.retry_statuses,
        })
    }

    fn get_connection(&self) -> Result<Arc<Connection>, ClientError> {
        let mut slot = self
            .connection
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if let Some(connection) = slot.as_ref()
            && !connection.is_broken()
        {
            return Ok(Arc::clone(connection));
        }

        let mut pipe = PipeStream::connect(&self.pipe_name, CONNECT_TIMEOUT)
            .map_err(|error| ClientError::Unavailable(io_failure("connect", error)))?;
        handshake(&mut pipe)?;
        let connection_id = self.next_connection_id.fetch_add(1, Ordering::Relaxed);
        let connection = Connection::start(connection_id, pipe).map_err(ClientError::Protocol)?;
        if let Some(info) = self
            .remote_web_info
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clone()
        {
            let id = self.next_request_id.fetch_add(1, Ordering::Relaxed);
            let announcement =
                match connection.request(id, ClientMessage::RemoteWebConnectionInfo { id, info }) {
                    Ok(ServerMessage::RemoteWebConnectionInfo { accepted: true, .. }) => Ok(()),
                    Ok(ServerMessage::RemoteWebConnectionInfo { message, .. }) => Err(
                        protocol_failure("connection_info", "rejected", None, message),
                    ),
                    Ok(_) => Err(protocol_failure(
                        "connection_info",
                        "response_type_mismatch",
                        None,
                        "connection information received another response type",
                    )),
                    Err(error) => Err(error),
                };
            if let Err(error) = announcement {
                // URL を持たない half-open 接続を本体 UI に残さず、次の retry を新規接続にする。
                connection.fail(error.clone(), true);
                return Err(ClientError::Protocol(error));
            }
        }
        *slot = Some(Arc::clone(&connection));
        Ok(connection)
    }

    fn invalidate_connection(&self, connection_id: u64) {
        let mut slot = self
            .connection
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if slot
            .as_ref()
            .is_some_and(|connection| connection.id == connection_id)
        {
            *slot = None;
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
enum ConnectionMaintenanceEvent {
    ConnectFailed {
        error: String,
        retry_after: Duration,
    },
    Connected {
        recovered: bool,
    },
    Disconnected,
}

/// `connect_and_announce` の成功は、handshake と接続情報の応答確認まで完了したことを表す。
/// transport を差し替えたテストでも「本体の後起動」と「切断後の再通知」を同じ loop で固定する。
fn run_connection_maintainer<C>(
    stop: &AtomicBool,
    mut connect_and_announce: impl FnMut() -> Result<C, String>,
    mut is_broken: impl FnMut(&C) -> bool,
    mut wait: impl FnMut(Duration),
    mut report: impl FnMut(ConnectionMaintenanceEvent),
) {
    let mut consecutive_failures = 0_u32;
    let mut reported_max_delay = false;
    let mut recovering = false;
    while !stop.load(Ordering::Acquire) {
        match connect_and_announce() {
            Ok(connection) => {
                report(ConnectionMaintenanceEvent::Connected {
                    recovered: recovering,
                });
                consecutive_failures = 0;
                reported_max_delay = false;
                while !stop.load(Ordering::Acquire) && !is_broken(&connection) {
                    wait(CONNECTION_HEALTH_POLL);
                }
                if stop.load(Ordering::Acquire) {
                    break;
                }
                report(ConnectionMaintenanceEvent::Disconnected);
                recovering = true;
            }
            Err(error) => {
                recovering = true;
                let retry_after = reconnect_delay(consecutive_failures);
                // 最初の失敗と上限到達時だけ出す。5 秒上限で同じログを繰り返さない。
                if consecutive_failures == 0
                    || (retry_after == RECONNECT_MAX_DELAY && !reported_max_delay)
                {
                    report(ConnectionMaintenanceEvent::ConnectFailed { error, retry_after });
                    reported_max_delay |= retry_after == RECONNECT_MAX_DELAY;
                }
                consecutive_failures = consecutive_failures.saturating_add(1);
                wait(retry_after);
            }
        }
    }
}

fn reconnect_delay(consecutive_failures: u32) -> Duration {
    let multiplier = 1_u32 << consecutive_failures.min(8);
    RECONNECT_INITIAL_DELAY
        .saturating_mul(multiplier)
        .min(RECONNECT_MAX_DELAY)
}

fn wait_for_maintainer(stop: &AtomicBool, wake: &(Mutex<()>, Condvar), duration: Duration) {
    if stop.load(Ordering::Acquire) {
        return;
    }
    let guard = wake.0.lock().unwrap_or_else(|error| error.into_inner());
    if stop.load(Ordering::Acquire) {
        return;
    }
    let _ = wake
        .1
        .wait_timeout(guard, duration)
        .unwrap_or_else(|error| error.into_inner());
}

fn handshake(pipe: &mut PipeStream) -> Result<(), ClientError> {
    write_frame(pipe, &ClientHello::current())
        .map_err(|error| ClientError::Protocol(frame_failure("handshake_write", error)))?;
    let hello: mimageviewer_ipc::ServerHello = read_frame(pipe, MAX_CONTROL_FRAME_BYTES)
        .map_err(|error| ClientError::Protocol(frame_failure("handshake_read", error)))?;
    if !hello.accepted || hello.protocol_version != PROTOCOL_VERSION {
        return Err(ClientError::VersionMismatch {
            client: PROTOCOL_VERSION,
            server: hello.protocol_version,
        });
    }
    Ok(())
}

type PendingReply = mpsc::SyncSender<Result<ServerMessage, ProtocolFailure>>;

#[derive(Default)]
struct PendingRequests {
    state: Mutex<PendingState>,
}

#[derive(Default)]
struct PendingState {
    entries: HashMap<RequestId, PendingReply>,
    expired: HashSet<RequestId>,
}

impl PendingRequests {
    fn register(
        &self,
        id: RequestId,
        reply: PendingReply,
        broken: &AtomicBool,
    ) -> Result<(), ProtocolFailure> {
        let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        if broken.load(Ordering::Acquire) {
            return Err(protocol_failure(
                "request_write",
                "connection_closed",
                None,
                "connection already closed",
            ));
        }
        state.entries.insert(id, reply);
        Ok(())
    }

    fn remove(&self, id: RequestId) -> Option<PendingReply> {
        self.state
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .entries
            .remove(&id)
    }

    fn expire(&self, id: RequestId) {
        let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        if state.entries.remove(&id).is_some() {
            state.expired.insert(id);
        }
    }

    fn resolve(&self, id: RequestId, response: ServerMessage) -> bool {
        let reply = {
            let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
            let reply = state.entries.remove(&id);
            if reply.is_none() && state.expired.remove(&id) {
                return true;
            }
            reply
        };
        if let Some(reply) = reply {
            return reply.send(Ok(response)).is_ok();
        }
        false
    }

    fn fail_all(&self, failure: ProtocolFailure) {
        let pending = {
            let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
            state.expired.clear();
            state
                .entries
                .drain()
                .map(|(_, reply)| reply)
                .collect::<Vec<_>>()
        };
        for reply in pending {
            let _ = reply.send(Err(failure.clone()));
        }
    }
}

struct Connection {
    id: u64,
    writer: Mutex<PipeStream>,
    cancel_pipe: PipeStream,
    pending: PendingRequests,
    broken: AtomicBool,
}

impl Connection {
    fn start(id: u64, pipe: PipeStream) -> Result<Arc<Self>, ProtocolFailure> {
        let connection = Arc::new(Self {
            id,
            writer: Mutex::new(pipe.clone()),
            cancel_pipe: pipe.clone(),
            pending: PendingRequests::default(),
            broken: AtomicBool::new(false),
        });
        let reader_connection = Arc::clone(&connection);
        std::thread::Builder::new()
            .name("remote-ipc-reader".to_owned())
            .spawn(move || reader_connection.reader_loop(pipe))
            .map_err(|error| {
                protocol_failure(
                    "reader_spawn",
                    "thread",
                    None,
                    format!("response reader thread: {error}"),
                )
            })?;
        Ok(connection)
    }

    fn is_broken(&self) -> bool {
        self.broken.load(Ordering::Acquire)
    }

    fn request(
        &self,
        id: RequestId,
        request: ClientMessage,
    ) -> Result<ServerMessage, ProtocolFailure> {
        let (reply_tx, reply_rx) = mpsc::sync_channel(1);
        self.pending.register(id, reply_tx, &self.broken)?;
        let write_result = {
            let mut writer = self
                .writer
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            write_frame(&mut *writer, &request)
        };
        if let Err(error) = write_result {
            self.pending.remove(id);
            let failure = frame_failure("request_write", error);
            self.fail(failure.clone(), true);
            return Err(failure);
        }
        match reply_rx.recv_timeout(RESPONSE_TIMEOUT) {
            Ok(result) => result,
            Err(mpsc::RecvTimeoutError::Timeout) => {
                self.pending.expire(id);
                let failure =
                    protocol_failure("response_read", "timeout", None, "IPC response timed out");
                Err(failure)
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => Err(protocol_failure(
                "response_route",
                "disconnected",
                None,
                "response dispatcher stopped",
            )),
        }
    }

    fn reader_loop(&self, mut pipe: PipeStream) {
        loop {
            let message: ServerMessage = match read_frame(&mut pipe, MAX_RESPONSE_FRAME_BYTES) {
                Ok(message) => message,
                Err(error) => {
                    self.fail(frame_failure("response_read", error), false);
                    return;
                }
            };
            let id = message.id();
            if !self.pending.resolve(id, message) {
                self.fail(
                    protocol_failure(
                        "response_route",
                        "unknown_request_id",
                        None,
                        format!("response id {id} has no pending request"),
                    ),
                    true,
                );
                return;
            }
        }
    }

    fn fail(&self, failure: ProtocolFailure, cancel_io: bool) {
        if self.broken.swap(true, Ordering::AcqRel) {
            return;
        }
        self.pending.fail_all(failure);
        if cancel_io {
            self.cancel_pipe.cancel_io();
        }
    }
}

struct RetrySuccess<T> {
    value: T,
    retry_count: u32,
    retry_statuses: Vec<String>,
}

#[derive(Debug)]
struct RetryFailure {
    error: ClientError,
    retry_count: u32,
    retry_statuses: Vec<String>,
}

fn run_with_retry<T>(
    mut operation: impl FnMut() -> Result<T, ClientError>,
) -> Result<RetrySuccess<T>, RetryFailure> {
    let mut retry_count = 0;
    let mut retry_statuses = Vec::new();
    loop {
        match operation() {
            Ok(value) => {
                return Ok(RetrySuccess {
                    value,
                    retry_count,
                    retry_statuses,
                });
            }
            Err(error) if error.should_retry_internally() && retry_count < MAX_RETRIES => {
                retry_statuses.push(error.ipc_status());
                std::thread::sleep(retry_delay(retry_count));
                retry_count += 1;
            }
            Err(error) => {
                return Err(RetryFailure {
                    error,
                    retry_count,
                    retry_statuses,
                });
            }
        }
    }
}

fn retry_delay(retry_count: u32) -> Duration {
    Duration::from_millis(25_u64.saturating_mul(1_u64 << retry_count.min(6)))
}

fn frame_failure(stage: &'static str, error: FrameError) -> ProtocolFailure {
    match error {
        FrameError::Io(error) => io_failure(stage, error),
        FrameError::TooLarge { length, maximum } => protocol_failure(
            stage,
            "frame_too_large",
            None,
            format!("{length} > {maximum}"),
        ),
        FrameError::Encode(error) => protocol_failure(stage, "encode", None, error),
        FrameError::Decode(error) => protocol_failure(stage, "decode", None, error),
    }
}

fn io_failure(stage: &'static str, error: std::io::Error) -> ProtocolFailure {
    let kind = match error.kind() {
        std::io::ErrorKind::NotFound => "not_found",
        std::io::ErrorKind::PermissionDenied => "permission_denied",
        std::io::ErrorKind::TimedOut => "timeout",
        std::io::ErrorKind::UnexpectedEof => "unexpected_eof",
        std::io::ErrorKind::BrokenPipe => "broken_pipe",
        std::io::ErrorKind::ConnectionReset => "connection_reset",
        std::io::ErrorKind::WouldBlock => "busy",
        _ => "io",
    };
    protocol_failure(stage, kind, error.raw_os_error(), error.to_string())
}

fn protocol_failure(
    stage: &'static str,
    kind: &'static str,
    os_error: Option<i32>,
    detail: impl Into<String>,
) -> ProtocolFailure {
    ProtocolFailure {
        stage,
        kind,
        os_error,
        detail: detail.into(),
    }
}

#[cfg(windows)]
struct PipeHandle {
    handle: windows::Win32::Foundation::HANDLE,
}

#[cfg(windows)]
unsafe impl Send for PipeHandle {}
#[cfg(windows)]
unsafe impl Sync for PipeHandle {}

#[cfg(windows)]
impl Drop for PipeHandle {
    fn drop(&mut self) {
        unsafe {
            let _ = windows::Win32::Foundation::CloseHandle(self.handle);
        }
    }
}

#[cfg(windows)]
#[derive(Clone)]
struct PipeStream {
    inner: Arc<PipeHandle>,
}

#[cfg(windows)]
impl PipeStream {
    fn connect(pipe_name: &str, timeout: Duration) -> Result<Self, std::io::Error> {
        use windows::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_PIPE_BUSY, GetLastError};
        use windows::Win32::Storage::FileSystem::{
            CreateFileW, FILE_GENERIC_READ, FILE_GENERIC_WRITE, FILE_SHARE_READ, FILE_SHARE_WRITE,
            OPEN_EXISTING,
        };
        use windows::Win32::System::Pipes::WaitNamedPipeW;
        use windows::core::PCWSTR;

        let name: Vec<u16> = pipe_name.encode_utf16().chain([0]).collect();
        let deadline = Instant::now() + timeout;
        loop {
            let handle = unsafe {
                CreateFileW(
                    PCWSTR(name.as_ptr()),
                    FILE_GENERIC_READ.0 | FILE_GENERIC_WRITE.0,
                    FILE_SHARE_READ | FILE_SHARE_WRITE,
                    None,
                    OPEN_EXISTING,
                    client_pipe_attributes(),
                    None,
                )
            };
            match handle {
                Ok(handle) => {
                    return Ok(Self {
                        inner: Arc::new(PipeHandle { handle }),
                    });
                }
                Err(_) => {
                    let error = unsafe { GetLastError() };
                    if error == ERROR_PIPE_BUSY && Instant::now() < deadline {
                        unsafe {
                            let _ = WaitNamedPipeW(PCWSTR(name.as_ptr()), 50);
                        }
                        continue;
                    }
                    if error == ERROR_FILE_NOT_FOUND && Instant::now() < deadline {
                        std::thread::sleep(Duration::from_millis(20));
                        continue;
                    }
                    return Err(std::io::Error::from_raw_os_error(error.0 as i32));
                }
            }
        }
    }

    fn cancel_io(&self) {
        unsafe {
            let _ = windows::Win32::System::IO::CancelIoEx(self.inner.handle, None);
        }
    }
}

#[cfg(windows)]
fn client_pipe_attributes() -> windows::Win32::Storage::FileSystem::FILE_FLAGS_AND_ATTRIBUTES {
    use windows::Win32::Storage::FileSystem::{FILE_ATTRIBUTE_NORMAL, FILE_FLAG_OVERLAPPED};

    windows::Win32::Storage::FileSystem::FILE_FLAGS_AND_ATTRIBUTES(
        FILE_ATTRIBUTE_NORMAL.0 | FILE_FLAG_OVERLAPPED.0,
    )
}

#[cfg(windows)]
struct OverlappedEvent(windows::Win32::Foundation::HANDLE);

#[cfg(windows)]
impl OverlappedEvent {
    fn new() -> std::io::Result<Self> {
        use windows::Win32::System::Threading::CreateEventW;
        use windows::core::PCWSTR;

        unsafe { CreateEventW(None, true, false, PCWSTR::null()) }
            .map(Self)
            .map_err(|_| std::io::Error::last_os_error())
    }
}

#[cfg(windows)]
impl Drop for OverlappedEvent {
    fn drop(&mut self) {
        unsafe {
            let _ = windows::Win32::Foundation::CloseHandle(self.0);
        }
    }
}

#[cfg(windows)]
fn complete_overlapped(
    handle: windows::Win32::Foundation::HANDLE,
    overlapped: &windows::Win32::System::IO::OVERLAPPED,
    started: windows::core::Result<()>,
) -> std::io::Result<u32> {
    use windows::Win32::Foundation::{ERROR_IO_PENDING, GetLastError};
    use windows::Win32::System::IO::GetOverlappedResult;

    if started.is_err() {
        let error = unsafe { GetLastError() };
        if error != ERROR_IO_PENDING {
            return Err(std::io::Error::from_raw_os_error(error.0 as i32));
        }
    }
    let mut transferred = 0_u32;
    unsafe { GetOverlappedResult(handle, overlapped, &mut transferred, true) }
        .map_err(|_| std::io::Error::last_os_error())?;
    Ok(transferred)
}

#[cfg(not(windows))]
#[derive(Clone)]
struct PipeStream;

#[cfg(not(windows))]
impl PipeStream {
    fn connect(_pipe_name: &str, _timeout: Duration) -> Result<Self, std::io::Error> {
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "Windows named pipes are unavailable",
        ))
    }

    fn cancel_io(&self) {}
}

#[cfg(windows)]
impl Read for PipeStream {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        use windows::Win32::Storage::FileSystem::ReadFile;
        use windows::Win32::System::IO::OVERLAPPED;

        let event = OverlappedEvent::new()?;
        let mut overlapped = OVERLAPPED {
            hEvent: event.0,
            ..Default::default()
        };
        let started =
            unsafe { ReadFile(self.inner.handle, Some(buffer), None, Some(&mut overlapped)) };
        complete_overlapped(self.inner.handle, &overlapped, started).map(|read| read as usize)
    }
}

#[cfg(not(windows))]
impl Read for PipeStream {
    fn read(&mut self, _buffer: &mut [u8]) -> std::io::Result<usize> {
        Ok(0)
    }
}

#[cfg(windows)]
impl Write for PipeStream {
    fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
        use windows::Win32::Storage::FileSystem::WriteFile;
        use windows::Win32::System::IO::OVERLAPPED;

        let event = OverlappedEvent::new()?;
        let mut overlapped = OVERLAPPED {
            hEvent: event.0,
            ..Default::default()
        };
        let started =
            unsafe { WriteFile(self.inner.handle, Some(buffer), None, Some(&mut overlapped)) };
        complete_overlapped(self.inner.handle, &overlapped, started).map(|written| written as usize)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        // Persistent pipe では overlapped WriteFile の完了が framing の境界になる。
        // FlushFileBuffers は同じ handle の pending read と直列化するため使わない。
        Ok(())
    }
}

#[cfg(not(windows))]
impl Write for PipeStream {
    fn write(&mut self, _buffer: &[u8]) -> std::io::Result<usize> {
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "Windows named pipes are unavailable",
        ))
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::cell::{Cell, RefCell};

    use super::*;

    #[test]
    fn maintainer_connects_after_server_start_and_reannounces_after_disconnect() {
        #[derive(Clone, Copy)]
        struct FakeConnection {
            generation: u32,
        }

        let stop = AtomicBool::new(false);
        let server_running = Cell::new(false);
        let attempts = Cell::new(0_u32);
        let announcements = Cell::new(0_u32);
        let events = RefCell::new(Vec::new());

        run_connection_maintainer(
            &stop,
            || {
                attempts.set(attempts.get() + 1);
                if !server_running.get() {
                    return Err("pipe not found".to_owned());
                }
                let generation = announcements.get() + 1;
                announcements.set(generation);
                if generation == 2 {
                    stop.store(true, Ordering::Release);
                }
                Ok(FakeConnection { generation })
            },
            |connection| connection.generation == 1,
            |_| server_running.set(true),
            |event| events.borrow_mut().push(event),
        );

        assert_eq!(attempts.get(), 3, "停止中1回 + 初回接続 + 再接続");
        assert_eq!(announcements.get(), 2, "接続ごとに URL を再通知する");
        assert_eq!(
            events.into_inner(),
            [
                ConnectionMaintenanceEvent::ConnectFailed {
                    error: "pipe not found".to_owned(),
                    retry_after: RECONNECT_INITIAL_DELAY,
                },
                ConnectionMaintenanceEvent::Connected { recovered: true },
                ConnectionMaintenanceEvent::Disconnected,
                ConnectionMaintenanceEvent::Connected { recovered: true },
            ]
        );
    }

    #[test]
    fn reconnect_backoff_is_bounded() {
        assert_eq!(reconnect_delay(0), Duration::from_millis(250));
        assert_eq!(reconnect_delay(1), Duration::from_millis(500));
        assert_eq!(reconnect_delay(4), Duration::from_secs(4));
        assert_eq!(reconnect_delay(5), RECONNECT_MAX_DELAY);
        assert_eq!(reconnect_delay(u32::MAX), RECONNECT_MAX_DELAY);
    }

    #[test]
    fn responses_are_routed_by_id_even_when_they_arrive_out_of_order() {
        let pending = PendingRequests::default();
        let broken = AtomicBool::new(false);
        let (first_tx, first_rx) = mpsc::sync_channel(1);
        let (second_tx, second_rx) = mpsc::sync_channel(1);
        pending.register(10, first_tx, &broken).unwrap();
        pending.register(20, second_tx, &broken).unwrap();

        assert!(pending.resolve(
            20,
            ServerMessage::Thumbnail {
                id: 20,
                response: ThumbnailResponse::Success {
                    webp_bytes: vec![2]
                }
            }
        ));
        assert!(pending.resolve(
            10,
            ServerMessage::Thumbnail {
                id: 10,
                response: ThumbnailResponse::Success {
                    webp_bytes: vec![1]
                }
            }
        ));
        assert!(matches!(
            first_rx.recv().unwrap(),
            Ok(ServerMessage::Thumbnail {
                response: ThumbnailResponse::Success { webp_bytes }, ..
            }) if webp_bytes == vec![1]
        ));
        assert!(matches!(
            second_rx.recv().unwrap(),
            Ok(ServerMessage::Thumbnail {
                response: ThumbnailResponse::Success { webp_bytes }, ..
            }) if webp_bytes == vec![2]
        ));
    }

    #[cfg(windows)]
    #[test]
    fn persistent_client_pipe_enables_overlapped_io() {
        use windows::Win32::Storage::FileSystem::FILE_FLAG_OVERLAPPED;

        assert_ne!(
            client_pipe_attributes().0 & FILE_FLAG_OVERLAPPED.0,
            0,
            "reader thread と request writer が同じ pipe handle を同時使用するため必須"
        );
    }

    #[test]
    fn connection_loss_is_retried_and_the_next_connection_can_succeed() {
        let mut calls = 0;
        let result = run_with_retry(|| {
            calls += 1;
            if calls == 1 {
                Err(ClientError::Protocol(protocol_failure(
                    "response_read",
                    "broken_pipe",
                    Some(109),
                    "pipe ended",
                )))
            } else {
                Ok(7)
            }
        })
        .unwrap();
        assert_eq!(result.value, 7);
        assert_eq!(result.retry_count, 1);
        assert_eq!(result.retry_statuses, ["response_read_broken_pipe"]);
        assert_eq!(calls, 2);
    }

    #[test]
    fn permanent_remote_errors_are_not_retried() {
        let mut calls = 0;
        let result = run_with_retry(|| {
            calls += 1;
            Err::<(), _>(ClientError::Remote(ThumbnailError::new(
                ThumbnailErrorCode::NotFound,
                "missing",
            )))
        });
        assert!(result.is_err());
        assert_eq!(calls, 1);
    }

    #[test]
    fn response_timeout_is_returned_to_http_without_internal_retry() {
        let mut calls = 0;
        let result = run_with_retry(|| {
            calls += 1;
            Err::<(), _>(ClientError::Protocol(protocol_failure(
                "response_read",
                "timeout",
                None,
                "deadline",
            )))
        });
        assert!(matches!(
            result,
            Err(RetryFailure {
                error: ClientError::Protocol(ProtocolFailure {
                    stage: "response_read",
                    kind: "timeout",
                    ..
                }),
                retry_count: 0,
                ..
            })
        ));
        assert_eq!(calls, 1);
    }

    #[test]
    fn late_response_for_an_expired_request_is_ignored_without_breaking_routing() {
        let pending = PendingRequests::default();
        let broken = AtomicBool::new(false);
        let (expired_tx, _expired_rx) = mpsc::sync_channel(1);
        pending.register(10, expired_tx, &broken).unwrap();
        pending.expire(10);
        assert!(pending.resolve(
            10,
            ServerMessage::Thumbnail {
                id: 10,
                response: ThumbnailResponse::Success {
                    webp_bytes: vec![1],
                },
            }
        ));

        let (current_tx, current_rx) = mpsc::sync_channel(1);
        pending.register(20, current_tx, &broken).unwrap();
        assert!(pending.resolve(
            20,
            ServerMessage::Thumbnail {
                id: 20,
                response: ThumbnailResponse::Success {
                    webp_bytes: vec![2],
                },
            }
        ));
        assert!(matches!(
            current_rx.recv().unwrap(),
            Ok(ServerMessage::Thumbnail {
                response: ThumbnailResponse::Success { webp_bytes },
                ..
            }) if webp_bytes == vec![2]
        ));
    }

    #[cfg(windows)]
    #[test]
    #[ignore = "managed test sandbox denies local named-pipe connections (os error 5)"]
    fn one_pipe_routes_two_concurrent_requests_with_reversed_responses() {
        use windows::Win32::Foundation::{CloseHandle, ERROR_PIPE_CONNECTED, GetLastError, HANDLE};
        use windows::Win32::Storage::FileSystem::{
            FlushFileBuffers, PIPE_ACCESS_DUPLEX, ReadFile, WriteFile,
        };
        use windows::Win32::System::Pipes::{
            ConnectNamedPipe, CreateNamedPipeW, DisconnectNamedPipe, NAMED_PIPE_MODE,
            PIPE_READMODE_BYTE, PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_BYTE, PIPE_WAIT,
        };
        use windows::core::PCWSTR;

        struct TestServerStream(HANDLE);
        unsafe impl Send for TestServerStream {}
        impl Read for TestServerStream {
            fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
                let mut read = 0;
                unsafe { ReadFile(self.0, Some(buffer), Some(&mut read), None) }
                    .map_err(|_| std::io::Error::last_os_error())?;
                Ok(read as usize)
            }
        }
        impl Write for TestServerStream {
            fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
                let mut written = 0;
                unsafe { WriteFile(self.0, Some(buffer), Some(&mut written), None) }
                    .map_err(|_| std::io::Error::last_os_error())?;
                Ok(written as usize)
            }
            fn flush(&mut self) -> std::io::Result<()> {
                unsafe { FlushFileBuffers(self.0) }.map_err(|_| std::io::Error::last_os_error())
            }
        }
        impl Drop for TestServerStream {
            fn drop(&mut self) {
                unsafe {
                    let _ = DisconnectNamedPipe(self.0);
                    let _ = CloseHandle(self.0);
                }
            }
        }

        let pipe_name = format!(
            r"\\.\pipe\mimageviewer-remote-test-{}-{}",
            std::process::id(),
            Instant::now().elapsed().as_nanos()
        );
        let server_name = pipe_name.clone();
        let (ready_tx, ready_rx) = mpsc::sync_channel(1);
        let server = std::thread::spawn(move || {
            let wide: Vec<u16> = server_name.encode_utf16().chain([0]).collect();
            let mode = NAMED_PIPE_MODE(
                PIPE_TYPE_BYTE.0
                    | PIPE_READMODE_BYTE.0
                    | PIPE_WAIT.0
                    | PIPE_REJECT_REMOTE_CLIENTS.0,
            );
            let handle = unsafe {
                CreateNamedPipeW(
                    PCWSTR(wide.as_ptr()),
                    PIPE_ACCESS_DUPLEX,
                    mode,
                    1,
                    64 * 1024,
                    64 * 1024,
                    0,
                    None,
                )
            };
            if handle.is_invalid() {
                let _ = ready_tx.send(Err(std::io::Error::last_os_error()));
                return;
            }
            let _ = ready_tx.send(Ok(()));
            let connected = unsafe {
                ConnectNamedPipe(handle, None).is_ok() || GetLastError() == ERROR_PIPE_CONNECTED
            };
            assert!(connected);
            let mut pipe = TestServerStream(handle);
            let hello: ClientHello = read_frame(&mut pipe, MAX_CONTROL_FRAME_BYTES).unwrap();
            write_frame(
                &mut pipe,
                &mimageviewer_ipc::negotiate(hello.protocol_version),
            )
            .unwrap();
            let first: ClientMessage = read_frame(&mut pipe, MAX_CONTROL_FRAME_BYTES).unwrap();
            let second: ClientMessage = read_frame(&mut pipe, MAX_CONTROL_FRAME_BYTES).unwrap();
            for message in [second, first] {
                let ClientMessage::Thumbnail { id, request, .. } = message else {
                    panic!("unexpected request type")
                };
                let marker = request.address.relative_path.as_bytes()[0];
                write_frame(
                    &mut pipe,
                    &ServerMessage::Thumbnail {
                        id,
                        response: ThumbnailResponse::Success {
                            webp_bytes: vec![marker],
                        },
                    },
                )
                .unwrap();
            }
        });
        ready_rx
            .recv_timeout(Duration::from_secs(2))
            .unwrap()
            .unwrap();

        let client = Arc::new(ThumbnailClient {
            pipe_name,
            connection: Mutex::new(None),
            next_request_id: AtomicU64::new(1),
            next_connection_id: AtomicU64::new(1),
            remote_web_info: Mutex::new(None),
        });
        let first_client = Arc::clone(&client);
        let first = std::thread::spawn(move || {
            first_client
                .thumbnail_address(
                    "test-client",
                    RemoteAddress::file("00000000-0000-0000-0000-000000000000", "a.jpg"),
                    128,
                )
                .map(|result| result.bytes)
                .map_err(|failure| failure.error.to_string())
        });
        let second_client = Arc::clone(&client);
        let second = std::thread::spawn(move || {
            second_client
                .thumbnail_address(
                    "test-client",
                    RemoteAddress::file("00000000-0000-0000-0000-000000000000", "b.jpg"),
                    128,
                )
                .map(|result| result.bytes)
                .map_err(|failure| failure.error.to_string())
        });
        assert_eq!(first.join().unwrap().unwrap(), vec![b'a']);
        assert_eq!(second.join().unwrap().unwrap(), vec![b'b']);
        server.join().unwrap();
    }
}
