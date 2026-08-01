use std::path::PathBuf;
use std::time::Duration;

use mimageviewer_ipc::{
    RemoteAddress, RemoteSubresource, VideoStreamError, VideoStreamErrorCode,
    VideoStreamPlaylistKind, VideoStreamPlaylistPayload, VideoStreamResult,
    VideoStreamSegmentIndex, VideoStreamSegmentPayload, VideoStreamSize, VideoStreamStartPayload,
    VideoStreamStatePayload,
};

use super::path_guard::{ResolveError, resolve_existing};
use super::session::{PublishedVideoStream, SessionHandle};
use crate::video::stream::session::{
    StreamGenerationMetrics, StreamGenerationStatus, StreamResource, StreamResourceError,
    StreamResourceKind, StreamSegmentBytes, StreamingGeneration,
};

const START_READY_TIMEOUT: Duration = Duration::from_secs(7);

pub(super) struct VideoStreamEngine {
    settings: crate::settings::Settings,
}

impl VideoStreamEngine {
    pub(super) fn new(settings: crate::settings::Settings) -> Self {
        Self { settings }
    }

    pub(super) fn resolve_start_address(
        &self,
        address: &RemoteAddress,
    ) -> Result<PathBuf, VideoStreamError> {
        address.validate_syntax().map_err(|_| {
            video_error(
                VideoStreamErrorCode::BadRequest,
                "動画アドレスの形式が正しくありません",
            )
        })?;
        if !matches!(address.subresource, RemoteSubresource::File) {
            return Err(video_error(
                VideoStreamErrorCode::BadRequest,
                "動画ストリーミングは実ファイルだけを受け付けます",
            ));
        }
        let resolved = resolve_existing(
            &self.settings.favorites,
            &address.favorite_id,
            &address.relative_path,
        )
        .map_err(resolve_error)?;
        if !resolved
            .canonical
            .extension()
            .and_then(|extension| extension.to_str())
            .map(str::to_ascii_lowercase)
            .is_some_and(|extension| {
                crate::folder_tree::SUPPORTED_VIDEO_EXTENSIONS.contains(&extension.as_str())
            })
        {
            return Err(video_error(
                VideoStreamErrorCode::Unsupported,
                "対応していない動画形式です",
            ));
        }
        let metadata = std::fs::metadata(&resolved.canonical).map_err(|_| {
            video_error(
                VideoStreamErrorCode::NotFound,
                "動画ファイルが見つかりません",
            )
        })?;
        if !metadata.is_file() {
            return Err(video_error(
                VideoStreamErrorCode::NotFound,
                "動画ファイルが見つかりません",
            ));
        }
        // canonical は worker 上の containment / 種別検証だけに使う。UI へは既存 player.path()
        // と I/O なしで比較できる favorite 配下の論理 path を渡す。
        Ok(resolved.logical)
    }

    pub(super) fn complete_start(
        &self,
        stream: PublishedVideoStream,
    ) -> VideoStreamResult<VideoStreamStartPayload> {
        match stream.generation.wait_ready(START_READY_TIMEOUT) {
            StreamGenerationStatus::Ready(ready) => {
                let playback = stream.playback.snapshot();
                VideoStreamResult::Success(VideoStreamStartPayload {
                    session: stream.session.0,
                    generation: stream.generation_id().0,
                    duration_secs: playback.duration_secs,
                    encoder: ready.encoder.as_str().to_owned(),
                    video_size: VideoStreamSize {
                        width: ready.output_dimensions.width,
                        height: ready.output_dimensions.height,
                    },
                    codecs: ready.codecs,
                })
            }
            StreamGenerationStatus::Opening => VideoStreamResult::Error(video_error(
                VideoStreamErrorCode::NotReady,
                "動画エンコーダの準備が完了していません",
            )),
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
            Err(error) => VideoStreamResult::Error(resource_error(error)),
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
            Err(error) => VideoStreamResult::Error(resource_error(error)),
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
            StreamGenerationStatus::Ready(ready) => ready,
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
            Err(error) => return VideoStreamResult::Error(resource_error(error)),
        };
        VideoStreamResult::Success(state_payload(stream, ready, metrics))
    }
}

fn state_payload(
    stream: PublishedVideoStream,
    ready: crate::video::stream::session::StreamReadyInfo,
    metrics: StreamGenerationMetrics,
) -> VideoStreamStatePayload {
    let playback = stream.playback.snapshot();
    VideoStreamStatePayload {
        session: stream.session.0,
        generation: stream.generation_id().0,
        position_secs: playback.position_secs,
        duration_secs: playback.duration_secs,
        buffered_secs: metrics.buffered_secs,
        effective_bitrate_bps: metrics.effective_bitrate_bps,
        encoder: ready.encoder.as_str().to_owned(),
        video_size: VideoStreamSize {
            width: ready.output_dimensions.width,
            height: ready.output_dimensions.height,
        },
        codecs: ready.codecs,
        playing: playback.playing,
        volume: playback.volume,
    }
}

fn resolve_error(error: ResolveError) -> VideoStreamError {
    let (code, message) = match error {
        ResolveError::InvalidFavoriteId | ResolveError::InvalidRelativePath => (
            VideoStreamErrorCode::BadRequest,
            "動画アドレスの形式が正しくありません",
        ),
        ResolveError::FavoriteNotFound => (
            VideoStreamErrorCode::FavoriteNotFound,
            "お気に入りが見つかりません",
        ),
        ResolveError::Unavailable => (
            VideoStreamErrorCode::NotFound,
            "動画ファイルが見つかりません",
        ),
        ResolveError::EscapesFavorite => (
            VideoStreamErrorCode::PathRejected,
            "お気に入りの外側は開けません",
        ),
    };
    video_error(code, message)
}

fn resource_error(error: StreamResourceError) -> VideoStreamError {
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
        StreamResourceError::Timeout => video_error(
            VideoStreamErrorCode::NotReady,
            "動画セグメントの取得が 2 秒以内に完了しませんでした",
        ),
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
            resource_error(StreamResourceError::GenerationMismatch).code,
            VideoStreamErrorCode::GenerationMismatch
        );
    }

    #[test]
    fn readiness_and_timeout_never_become_empty_successes() {
        for error in [StreamResourceError::NotReady, StreamResourceError::Timeout] {
            assert_eq!(resource_error(error).code, VideoStreamErrorCode::NotReady);
        }
    }

    #[test]
    fn start_address_is_revalidated_against_the_core_favorite_allowlist() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("favorite");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("movie.mp4"), b"fixture").unwrap();
        std::fs::write(root.join("notes.txt"), b"fixture").unwrap();
        let favorite = crate::settings::FavoriteEntry::new("fixture".to_owned(), root.clone());
        let favorite_id = favorite.id.to_string();
        let mut settings = crate::settings::Settings::default();
        settings.favorites = vec![favorite];
        let engine = VideoStreamEngine::new(settings);

        let resolved = engine
            .resolve_start_address(&RemoteAddress::file(&favorite_id, "movie.mp4"))
            .unwrap();
        assert_eq!(resolved, root.join("movie.mp4"));
        assert_eq!(
            engine
                .resolve_start_address(&RemoteAddress::file(&favorite_id, "../movie.mp4"))
                .unwrap_err()
                .code,
            VideoStreamErrorCode::BadRequest
        );
        assert_eq!(
            engine
                .resolve_start_address(&RemoteAddress::file(
                    "00000000-0000-0000-0000-000000000000",
                    "movie.mp4",
                ))
                .unwrap_err()
                .code,
            VideoStreamErrorCode::FavoriteNotFound
        );
        assert_eq!(
            engine
                .resolve_start_address(&RemoteAddress::file(&favorite_id, "notes.txt"))
                .unwrap_err()
                .code,
            VideoStreamErrorCode::Unsupported
        );
    }
}
