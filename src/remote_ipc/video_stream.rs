use std::path::PathBuf;
use std::time::{Duration, Instant};

use mimageviewer_ipc::{
    RemoteAddress, RemoteSubresource, VIDEO_STREAM_START_BUDGET, VideoStreamAudioProcessing,
    VideoStreamError, VideoStreamErrorCode, VideoStreamPlaylistKind, VideoStreamPlaylistPayload,
    VideoStreamResult, VideoStreamSegmentIndex, VideoStreamSegmentPayload, VideoStreamSize,
    VideoStreamStartPayload, VideoStreamStatePayload,
};

use super::path_guard::{ResolveError, resolve_existing};
use super::session::{PublishedVideoStream, SessionHandle};
use crate::video::stream::session::{
    StreamGenerationMetrics, StreamGenerationStatus, StreamResource, StreamResourceError,
    StreamResourceKind, StreamSegmentBytes, StreamingGeneration, StreamingGenerationAccess,
};

#[derive(Clone, Copy, Debug)]
pub(crate) struct VideoStreamStartBudget {
    deadline: Instant,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum VideoStreamStartStage {
    Queue,
    Ui,
    Player,
    Seek,
    Encoder,
    Playlist,
}

impl VideoStreamStartBudget {
    pub(super) fn from_enqueued_at(enqueued_at: Instant) -> Self {
        Self {
            deadline: enqueued_at + VIDEO_STREAM_START_BUDGET,
        }
    }

    pub(super) fn remaining(self) -> Duration {
        self.remaining_at(Instant::now())
    }

    pub(super) fn remaining_at(self, now: Instant) -> Duration {
        self.deadline.saturating_duration_since(now)
    }

    pub(super) fn expired_error(self, stage: VideoStreamStartStage) -> Option<VideoStreamError> {
        self.expired_error_at(Instant::now(), stage)
    }

    pub(super) fn expired_error_at(
        self,
        now: Instant,
        stage: VideoStreamStartStage,
    ) -> Option<VideoStreamError> {
        self.remaining_at(now)
            .is_zero()
            .then(|| self.timeout_error(stage))
    }

    pub(super) fn timeout_error(self, stage: VideoStreamStartStage) -> VideoStreamError {
        let (code, waiting_for) = match stage {
            VideoStreamStartStage::Queue => (
                VideoStreamErrorCode::StartQueueTimeout,
                "本体の動画 stream queue で実行開始",
            ),
            VideoStreamStartStage::Ui => (
                VideoStreamErrorCode::StartUiTimeout,
                "本体 UI による動画開始要求の受理",
            ),
            VideoStreamStartStage::Player => (
                VideoStreamErrorCode::StartPlayerTimeout,
                "player の metadata・映像/音声 tap の準備",
            ),
            VideoStreamStartStage::Seek => (
                VideoStreamErrorCode::StartSeekTimeout,
                "再開位置への seek と generation の同期",
            ),
            VideoStreamStartStage::Encoder => (
                VideoStreamErrorCode::StartEncoderTimeout,
                "動画 encoder の初期化",
            ),
            VideoStreamStartStage::Playlist => (
                VideoStreamErrorCode::StartPlaylistTimeout,
                "最初の master playlist の生成",
            ),
        };
        video_error(
            code,
            format!(
                "動画 start の {} 秒予算を、{waiting_for}の待機中に使い切りました",
                VIDEO_STREAM_START_BUDGET.as_secs()
            ),
        )
    }
}
pub(super) struct VideoStreamEngine;

impl VideoStreamEngine {
    pub(super) fn new() -> Self {
        Self
    }

    pub(super) fn resolve_start_address(
        &self,
        address: &RemoteAddress,
    ) -> Result<PathBuf, VideoStreamError> {
        address.validate_syntax().map_err(|error| {
            video_error(
                VideoStreamErrorCode::BadRequest,
                if error == mimageviewer_ipc::AddressError::NetworkPath {
                    mimageviewer_ipc::REMOTE_NETWORK_PATH_MESSAGE
                } else {
                    "メディアアドレスの形式が正しくありません"
                },
            )
        })?;
        if !matches!(address.subresource, RemoteSubresource::File) {
            return Err(video_error(
                VideoStreamErrorCode::BadRequest,
                "メディアストリーミングは実ファイルだけを受け付けます",
            ));
        }
        let resolved = resolve_existing(&address.path).map_err(resolve_error)?;
        if !resolved
            .canonical
            .extension()
            .and_then(|extension| extension.to_str())
            .map(str::to_ascii_lowercase)
            .is_some_and(|extension| {
                crate::folder_tree::SUPPORTED_VIDEO_EXTENSIONS.contains(&extension.as_str())
                    || crate::folder_tree::SUPPORTED_AUDIO_EXTENSIONS.contains(&extension.as_str())
            })
        {
            return Err(video_error(
                VideoStreamErrorCode::Unsupported,
                "対応していないメディア形式です",
            ));
        }
        let metadata = std::fs::metadata(&resolved.canonical).map_err(|_| {
            video_error(
                VideoStreamErrorCode::NotFound,
                "メディアファイルが見つかりません",
            )
        })?;
        if !metadata.is_file() {
            return Err(video_error(
                VideoStreamErrorCode::NotFound,
                "メディアファイルが見つかりません",
            ));
        }
        // canonical は worker 上の containment / 種別検証だけに使う。UI へは既存 player.path()
        // と I/O なしで比較できる favorite 配下の論理 path を渡す。
        Ok(resolved.logical)
    }

    pub(super) fn complete_start(
        &self,
        stream: PublishedVideoStream,
        budget: VideoStreamStartBudget,
    ) -> VideoStreamResult<VideoStreamStartPayload> {
        match stream.generation.wait_ready(budget.remaining()) {
            StreamGenerationStatus::Ready(ready) | StreamGenerationStatus::Ended(ready) => {
                if let Err(error) = wait_for_start_playlist(&stream.generation, budget) {
                    return VideoStreamResult::Error(error);
                }
                let metrics = match stream
                    .generation
                    .resource(stream.generation_id(), StreamResourceKind::State)
                {
                    Ok(StreamResource::State(metrics)) => metrics,
                    Ok(_) => {
                        return VideoStreamResult::Error(video_error(
                            VideoStreamErrorCode::Internal,
                            "動画状態応答の型が一致しません",
                        ));
                    }
                    Err(error) => {
                        return VideoStreamResult::Error(resource_error(
                            error,
                            StreamResourceKind::State,
                        ));
                    }
                };
                let playback = stream.playback.snapshot();
                let (has_video, encoder, video_size) = ready_video_payload(&ready);
                VideoStreamResult::Success(VideoStreamStartPayload {
                    session: stream.session.0,
                    generation: stream.generation_id().0,
                    duration_secs: playback.duration_secs,
                    source_origin_secs: metrics.source_origin_secs,
                    buffer_target_secs: stream.buffer_target_secs,
                    has_video,
                    encoder,
                    video_size,
                    codecs: ready.codecs,
                    audio_processing: audio_processing_payload(stream.generation.audio_status()),
                    end_behavior: stream.end_behavior,
                })
            }
            StreamGenerationStatus::Opening => {
                VideoStreamResult::Error(budget.timeout_error(VideoStreamStartStage::Encoder))
            }
            StreamGenerationStatus::Failed(error) => {
                VideoStreamResult::Error(video_error(VideoStreamErrorCode::Failed, error))
            }
            StreamGenerationStatus::Stopped => VideoStreamResult::Error(video_error(
                VideoStreamErrorCode::Failed,
                "動画ストリーミングが停止しました",
            )),
        }
    }

    pub(super) fn playlist(
        &self,
        session_handle: &SessionHandle,
        session: u64,
        generation: u64,
        kind: VideoStreamPlaylistKind,
    ) -> VideoStreamResult<VideoStreamPlaylistPayload> {
        let stream = match session_handle.video_stream(session) {
            Ok(stream) => stream,
            Err(error) => return VideoStreamResult::Error(error),
        };
        let kind = match kind {
            VideoStreamPlaylistKind::Master => StreamResourceKind::MasterPlaylist,
            VideoStreamPlaylistKind::Media => StreamResourceKind::MediaPlaylist,
        };
        match stream
            .generation
            .resource(StreamingGeneration(generation), kind)
        {
            Ok(StreamResource::Playlist(Some(body))) if !body.is_empty() => {
                VideoStreamResult::Success(VideoStreamPlaylistPayload { body })
            }
            Ok(StreamResource::Playlist(None)) => VideoStreamResult::Error(video_error(
                VideoStreamErrorCode::NotReady,
                "プレイリストはまだ生成されていません",
            )),
            Ok(StreamResource::Playlist(Some(_))) => VideoStreamResult::Error(video_error(
                VideoStreamErrorCode::Internal,
                "空のプレイリストを配信できません",
            )),
            Ok(_) => VideoStreamResult::Error(video_error(
                VideoStreamErrorCode::Internal,
                "プレイリスト応答の型が一致しません",
            )),
            Err(error) => VideoStreamResult::Error(resource_error(error, kind)),
        }
    }

    pub(super) fn segment(
        &self,
        session_handle: &SessionHandle,
        session: u64,
        generation: u64,
        index: VideoStreamSegmentIndex,
    ) -> VideoStreamResult<VideoStreamSegmentPayload> {
        let stream = match session_handle.video_stream(session) {
            Ok(stream) => stream,
            Err(error) => return VideoStreamResult::Error(error),
        };
        let kind = match index {
            VideoStreamSegmentIndex::Init => StreamResourceKind::InitSegment,
            VideoStreamSegmentIndex::Media { sequence } => {
                StreamResourceKind::MediaSegment(sequence)
            }
        };
        match stream
            .generation
            .resource(StreamingGeneration(generation), kind)
        {
            Ok(StreamResource::InitSegment(Some(bytes))) if !bytes.is_empty() => {
                VideoStreamResult::Success(VideoStreamSegmentPayload::Found(bytes))
            }
            Ok(StreamResource::InitSegment(None)) => VideoStreamResult::Error(video_error(
                VideoStreamErrorCode::NotReady,
                "初期化セグメントはまだ生成されていません",
            )),
            Ok(StreamResource::InitSegment(Some(_))) => VideoStreamResult::Error(video_error(
                VideoStreamErrorCode::Internal,
                "空の初期化セグメントを配信できません",
            )),
            Ok(StreamResource::MediaSegment(StreamSegmentBytes::Found(bytes)))
                if !bytes.is_empty() =>
            {
                VideoStreamResult::Success(VideoStreamSegmentPayload::Found(bytes))
            }
            Ok(StreamResource::MediaSegment(StreamSegmentBytes::Found(_))) => {
                VideoStreamResult::Error(video_error(
                    VideoStreamErrorCode::Internal,
                    "空のメディアセグメントを配信できません",
                ))
            }
            Ok(StreamResource::MediaSegment(StreamSegmentBytes::NotFound)) => {
                VideoStreamResult::Success(VideoStreamSegmentPayload::NotFound)
            }
            Ok(StreamResource::MediaSegment(StreamSegmentBytes::Gone)) => {
                VideoStreamResult::Success(VideoStreamSegmentPayload::Gone)
            }
            Ok(_) => VideoStreamResult::Error(video_error(
                VideoStreamErrorCode::Internal,
                "セグメント応答の型が一致しません",
            )),
            Err(error) => VideoStreamResult::Error(resource_error(error, kind)),
        }
    }

    pub(super) fn state(
        &self,
        session_handle: &SessionHandle,
        session: u64,
    ) -> VideoStreamResult<VideoStreamStatePayload> {
        let stream = match session_handle.video_stream(session) {
            Ok(stream) => stream,
            Err(error) => return VideoStreamResult::Error(error),
        };
        let ready = match stream.generation.status() {
            StreamGenerationStatus::Ready(ready) | StreamGenerationStatus::Ended(ready) => ready,
            StreamGenerationStatus::Opening => {
                return VideoStreamResult::Error(video_error(
                    VideoStreamErrorCode::NotReady,
                    "動画エンコーダの準備が完了していません",
                ));
            }
            StreamGenerationStatus::Failed(error) => {
                return VideoStreamResult::Error(video_error(VideoStreamErrorCode::Failed, error));
            }
            StreamGenerationStatus::Stopped => {
                return VideoStreamResult::Error(video_error(
                    VideoStreamErrorCode::Failed,
                    "動画ストリーミングが停止しました",
                ));
            }
        };
        let metrics = match stream
            .generation
            .resource(stream.generation_id(), StreamResourceKind::State)
        {
            Ok(StreamResource::State(metrics)) => metrics,
            Ok(_) => {
                return VideoStreamResult::Error(video_error(
                    VideoStreamErrorCode::Internal,
                    "動画状態応答の型が一致しません",
                ));
            }
            Err(error) => {
                return VideoStreamResult::Error(resource_error(error, StreamResourceKind::State));
            }
        };
        VideoStreamResult::Success(state_payload(stream, ready, metrics))
    }
}

fn wait_for_start_playlist(
    generation: &StreamingGenerationAccess,
    budget: VideoStreamStartBudget,
) -> Result<(), VideoStreamError> {
    wait_for_start_playlist_with(budget, |remaining| {
        generation.resource_with_timeout(
            generation.generation(),
            StreamResourceKind::MasterPlaylist,
            remaining,
        )
    })
}

fn wait_for_start_playlist_with(
    budget: VideoStreamStartBudget,
    mut request: impl FnMut(Duration) -> Result<StreamResource, StreamResourceError>,
) -> Result<(), VideoStreamError> {
    loop {
        let remaining = budget.remaining();
        if remaining.is_zero() {
            return Err(budget.timeout_error(VideoStreamStartStage::Playlist));
        }
        match request(remaining) {
            Ok(StreamResource::Playlist(Some(body))) if !body.is_empty() => return Ok(()),
            Ok(StreamResource::Playlist(None)) | Err(StreamResourceError::NotReady) => {}
            Ok(StreamResource::Playlist(Some(_))) => {
                return Err(video_error(
                    VideoStreamErrorCode::Internal,
                    "空のプレイリストを配信できません",
                ));
            }
            Ok(_) => {
                return Err(video_error(
                    VideoStreamErrorCode::Internal,
                    "プレイリスト応答の型が一致しません",
                ));
            }
            Err(StreamResourceError::Timeout) => {
                return Err(budget.timeout_error(VideoStreamStartStage::Playlist));
            }
            Err(error) => {
                return Err(resource_error(error, StreamResourceKind::MasterPlaylist));
            }
        }
        std::thread::sleep(budget.remaining().min(Duration::from_millis(50)));
    }
}

fn state_payload(
    stream: PublishedVideoStream,
    ready: crate::video::stream::session::StreamReadyInfo,
    metrics: StreamGenerationMetrics,
) -> VideoStreamStatePayload {
    let playback = stream.playback.snapshot();
    let (has_video, encoder, video_size) = ready_video_payload(&ready);
    VideoStreamStatePayload {
        session: stream.session.0,
        generation: stream.generation_id().0,
        duration_secs: playback.duration_secs,
        source_origin_secs: metrics.source_origin_secs,
        generated_start_secs: metrics.generated_start_secs,
        generated_end_secs: metrics.generated_end_secs,
        ring_start_secs: metrics.ring_start_secs,
        ring_end_secs: metrics.ring_end_secs,
        ring_earliest_sequence: metrics.earliest_sequence,
        ring_latest_sequence: metrics.latest_sequence,
        buffer_target_secs: stream.buffer_target_secs,
        buffered_secs: metrics.buffered_secs,
        effective_bitrate_bps: metrics.effective_bitrate_bps,
        ended: metrics.ended,
        has_video,
        encoder,
        video_size,
        codecs: ready.codecs,
        audio_processing: audio_processing_payload(stream.generation.audio_status()),
        play_intent: playback.play_intent,
        volume: playback.volume,
    }
}

fn ready_video_payload(
    ready: &crate::video::stream::session::StreamReadyInfo,
) -> (bool, String, VideoStreamSize) {
    ready.video.as_ref().map_or_else(
        || {
            (
                false,
                "audio-only".to_owned(),
                VideoStreamSize {
                    width: 0,
                    height: 0,
                },
            )
        },
        |video| {
            (
                true,
                video.encoder.as_str().to_owned(),
                VideoStreamSize {
                    width: video.output_dimensions.width,
                    height: video.output_dimensions.height,
                },
            )
        },
    )
}

fn audio_processing_payload(
    status: crate::video::clockless_transcode::ClocklessVstStatusSnapshot,
) -> VideoStreamAudioProcessing {
    VideoStreamAudioProcessing {
        vst3_requested: status.requested,
        vst3_active: status.active,
        vst3_active_slots: status.active_slots,
        vst3_warning: status.warning,
    }
}

fn resolve_error(error: ResolveError) -> VideoStreamError {
    let (code, message) = match error {
        ResolveError::InvalidPath => (
            VideoStreamErrorCode::BadRequest,
            "メディアアドレスの形式が正しくありません",
        ),
        ResolveError::NetworkPath => (
            VideoStreamErrorCode::BadRequest,
            mimageviewer_ipc::REMOTE_NETWORK_PATH_MESSAGE,
        ),
        ResolveError::Unavailable => (
            VideoStreamErrorCode::NotFound,
            "メディアファイルが見つかりません",
        ),
    };
    video_error(code, message)
}

fn resource_error(error: StreamResourceError, kind: StreamResourceKind) -> VideoStreamError {
    match error {
        StreamResourceError::GenerationMismatch => video_error(
            VideoStreamErrorCode::GenerationMismatch,
            "動画ストリーミング generation が一致しません",
        ),
        StreamResourceError::NotReady => video_error(
            VideoStreamErrorCode::NotReady,
            "動画ストリーミングの出力はまだ準備できていません",
        ),
        StreamResourceError::Failed(error) => video_error(VideoStreamErrorCode::Failed, error),
        StreamResourceError::Stopped => video_error(
            VideoStreamErrorCode::Failed,
            "動画ストリーミングが停止しました",
        ),
        StreamResourceError::Timeout => {
            let waiting_for = match kind {
                StreamResourceKind::MasterPlaylist => "master playlist",
                StreamResourceKind::MediaPlaylist => "media playlist",
                StreamResourceKind::InitSegment => "初期化セグメント",
                StreamResourceKind::MediaSegment(_) => "media segment",
                StreamResourceKind::State => "stream state",
            };
            video_error(
                VideoStreamErrorCode::ResourceTimeout,
                format!("generation worker から {waiting_for} の応答を待つ内部期限を超えました"),
            )
        }
    }
}

fn video_error(code: VideoStreamErrorCode, message: impl Into<String>) -> VideoStreamError {
    VideoStreamError::new(code, message)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resource_generation_mismatch_stays_typed_for_http_409() {
        assert_eq!(
            resource_error(
                StreamResourceError::GenerationMismatch,
                StreamResourceKind::MasterPlaylist
            )
            .code,
            VideoStreamErrorCode::GenerationMismatch
        );
    }

    #[test]
    fn readiness_and_timeout_never_become_empty_successes() {
        assert_eq!(
            resource_error(
                StreamResourceError::NotReady,
                StreamResourceKind::MediaPlaylist
            )
            .code,
            VideoStreamErrorCode::NotReady
        );
        let timeout = resource_error(
            StreamResourceError::Timeout,
            StreamResourceKind::MediaSegment(42),
        );
        assert_eq!(timeout.code, VideoStreamErrorCode::ResourceTimeout);
        assert!(timeout.message.contains("media segment"));
    }

    #[test]
    fn start_readiness_waits_until_the_playlist_is_obtainable() {
        let mut requests = 0;
        let budget = VideoStreamStartBudget::from_enqueued_at(Instant::now());
        let result = wait_for_start_playlist_with(budget, |_| {
            requests += 1;
            Ok(StreamResource::Playlist(
                (requests >= 2).then(|| "#EXTM3U".to_owned()),
            ))
        });
        assert!(result.is_ok());
        assert_eq!(requests, 2);
    }

    #[test]
    fn player_wait_uses_the_outer_start_deadline_instead_of_the_old_two_second_cutoff() {
        let started_at = Instant::now();
        let budget = VideoStreamStartBudget::from_enqueued_at(started_at);

        assert!(
            budget
                .expired_error_at(
                    started_at + Duration::from_secs(2),
                    VideoStreamStartStage::Player,
                )
                .is_none()
        );
        let error = budget
            .expired_error_at(
                started_at + VIDEO_STREAM_START_BUDGET,
                VideoStreamStartStage::Player,
            )
            .expect("the single start deadline must eventually expire");
        assert_eq!(error.code, VideoStreamErrorCode::StartPlayerTimeout);
        assert!(error.message.contains("player"));
    }

    #[test]
    fn exhausted_start_budget_identifies_the_wait_stage() {
        let budget = VideoStreamStartBudget::from_enqueued_at(Instant::now());
        for (stage, code, marker) in [
            (
                VideoStreamStartStage::Player,
                VideoStreamErrorCode::StartPlayerTimeout,
                "player",
            ),
            (
                VideoStreamStartStage::Encoder,
                VideoStreamErrorCode::StartEncoderTimeout,
                "encoder",
            ),
        ] {
            let error = budget.timeout_error(stage);
            assert_eq!(error.code, code);
            assert!(error.message.contains(marker));
        }
    }

    #[test]
    fn start_address_accepts_existing_absolute_video_and_audio_paths() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("favorite");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("movie.mp4"), b"fixture").unwrap();
        std::fs::write(root.join("song.mp3"), b"fixture").unwrap();
        std::fs::write(root.join("notes.txt"), b"fixture").unwrap();
        let outside = temp.path().join("outside.mp4");
        std::fs::write(&outside, b"fixture").unwrap();
        let engine = VideoStreamEngine::new();

        let resolved = engine
            .resolve_start_address(&RemoteAddress::file(
                root.join("movie.mp4").to_string_lossy().into_owned(),
            ))
            .unwrap();
        assert_eq!(
            resolved,
            super::super::path_guard::resolve_existing(
                root.join("movie.mp4").to_string_lossy().as_ref(),
            )
            .unwrap()
            .logical
        );
        assert_eq!(
            engine
                .resolve_start_address(&RemoteAddress::file(
                    root.join("song.mp3").to_string_lossy().into_owned(),
                ))
                .unwrap(),
            super::super::path_guard::resolve_existing(
                root.join("song.mp3").to_string_lossy().as_ref(),
            )
            .unwrap()
            .logical
        );
        assert_eq!(
            engine
                .resolve_start_address(
                    &RemoteAddress::file(outside.to_string_lossy().into_owned(),)
                )
                .unwrap(),
            super::super::path_guard::resolve_existing(outside.to_string_lossy().as_ref())
                .unwrap()
                .logical
        );
        assert_eq!(
            engine
                .resolve_start_address(&RemoteAddress::file("../movie.mp4"))
                .unwrap_err()
                .code,
            VideoStreamErrorCode::BadRequest
        );
        assert_eq!(
            engine
                .resolve_start_address(&RemoteAddress::file(
                    root.join("notes.txt").to_string_lossy().into_owned(),
                ))
                .unwrap_err()
                .code,
            VideoStreamErrorCode::Unsupported
        );
    }
}
