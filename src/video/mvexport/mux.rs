use std::cmp::Ordering;
use std::path::Path;

use ffmpeg_the_third as ffmpeg;

use crate::video::stream::audio_encoder::OpenedAacEncoder;
use crate::video::stream::encoder::OpenedH264Encoder;

use super::ExportError;

pub(super) struct Mp4Muxer {
    output: ffmpeg::format::context::Output,
    video_stream_index: usize,
    audio: Option<AudioStream>,
    video_source_time_base: ffmpeg::Rational,
    video_stream_time_base: ffmpeg::Rational,
}

struct AudioStream {
    index: usize,
    source_time_base: ffmpeg::Rational,
    stream_time_base: ffmpeg::Rational,
}

impl Mp4Muxer {
    pub(super) fn new(
        path: &Path,
        video: &OpenedH264Encoder,
        audio: &OpenedAacEncoder,
        frame_rate: (u32, u32),
    ) -> Result<Self, ExportError> {
        Self::new_tracks(path, video, Some(audio), frame_rate)
    }

    pub(super) fn new_video_only(
        path: &Path,
        video: &OpenedH264Encoder,
        frame_rate: (u32, u32),
    ) -> Result<Self, ExportError> {
        Self::new_tracks(path, video, None, frame_rate)
    }

    fn new_tracks(
        path: &Path,
        video: &OpenedH264Encoder,
        audio: Option<&OpenedAacEncoder>,
        frame_rate: (u32, u32),
    ) -> Result<Self, ExportError> {
        let mut output = ffmpeg::format::output(path).map_err(|error| {
            ExportError::Mux(format!(
                "MP4 出力を作成できません ({}): {error}",
                path.display()
            ))
        })?;
        let video_codec = video
            .encoder
            .codec()
            .ok_or_else(|| ExportError::Mux("H.264 エンコーダ情報を取得できません".to_owned()))?;
        let video_source_time_base = ffmpeg::Rational(frame_rate.1 as i32, frame_rate.0 as i32);

        let video_stream_index;
        {
            let mut stream = output.add_stream(video_codec).map_err(|error| {
                ExportError::Mux(format!("映像ストリームを作成できません: {error}"))
            })?;
            video_stream_index = stream.index();
            stream.set_time_base(video_source_time_base);
            stream.set_rate((frame_rate.0 as i32, frame_rate.1 as i32));
            stream.set_avg_frame_rate((frame_rate.0 as i32, frame_rate.1 as i32));
            stream.copy_parameters_from_context(&video.encoder);
            unsafe {
                (*stream.parameters_mut().as_mut_ptr()).codec_tag = 0;
            }
        }
        let audio_stream = if let Some(audio) = audio {
            let audio_codec = audio
                .encoder
                .codec()
                .ok_or_else(|| ExportError::Mux("AAC エンコーダ情報を取得できません".to_owned()))?;
            let audio_source_time_base = ffmpeg::Rational(1, audio.output_sample_rate() as i32);
            let mut stream = output.add_stream(audio_codec).map_err(|error| {
                ExportError::Mux(format!("音声ストリームを作成できません: {error}"))
            })?;
            let audio_stream_index = stream.index();
            stream.set_time_base(audio_source_time_base);
            stream.copy_parameters_from_context(&audio.encoder);
            unsafe {
                (*stream.parameters_mut().as_mut_ptr()).codec_tag = 0;
            }
            Some((audio_stream_index, audio_source_time_base))
        } else {
            None
        };

        let mut options = ffmpeg::Dictionary::new();
        options.set("movflags", "+faststart");
        output
            .write_header_with(options)
            .map_err(|error| ExportError::Mux(format!("MP4 ヘッダーを書き込めません: {error}")))?;
        let video_stream_time_base = output
            .stream(video_stream_index)
            .ok_or_else(|| ExportError::Mux("映像ストリームを再取得できません".to_owned()))?
            .time_base();
        let audio = audio_stream
            .map(|(index, source_time_base)| {
                let stream_time_base = output
                    .stream(index)
                    .ok_or_else(|| ExportError::Mux("音声ストリームを再取得できません".to_owned()))?
                    .time_base();
                Ok::<_, ExportError>(AudioStream {
                    index,
                    source_time_base,
                    stream_time_base,
                })
            })
            .transpose()?;

        Ok(Self {
            output,
            video_stream_index,
            audio,
            video_source_time_base,
            video_stream_time_base,
        })
    }

    pub(super) fn audio_precedes_video(
        &self,
        audio: &ffmpeg::Packet,
        video: &ffmpeg::Packet,
    ) -> Result<bool, ExportError> {
        let audio_stream = self.audio_stream()?;
        let audio_dts = audio
            .dts()
            .ok_or_else(|| ExportError::Mux("AAC パケットに DTS がありません".to_owned()))?;
        let video_dts = video
            .dts()
            .ok_or_else(|| ExportError::Mux("H.264 パケットに DTS がありません".to_owned()))?;
        Ok(compare_timestamps(
            audio_dts,
            audio_stream.source_time_base,
            video_dts,
            self.video_source_time_base,
        ) != Ordering::Greater)
    }

    pub(super) fn write_video(&mut self, mut packet: ffmpeg::Packet) -> Result<(), ExportError> {
        packet.set_stream(self.video_stream_index);
        packet.set_duration(packet.duration().max(1));
        packet.rescale_ts(self.video_source_time_base, self.video_stream_time_base);
        packet
            .write_interleaved(&mut self.output)
            .map_err(|error| ExportError::Mux(format!("映像パケットを書き込めません: {error}")))
    }

    pub(super) fn write_audio(&mut self, mut packet: ffmpeg::Packet) -> Result<(), ExportError> {
        let audio = self.audio_stream()?;
        packet.set_stream(audio.index);
        packet.set_duration(packet.duration().max(1));
        packet.rescale_ts(audio.source_time_base, audio.stream_time_base);
        packet
            .write_interleaved(&mut self.output)
            .map_err(|error| ExportError::Mux(format!("音声パケットを書き込めません: {error}")))
    }

    fn audio_stream(&self) -> Result<&AudioStream, ExportError> {
        self.audio
            .as_ref()
            .ok_or_else(|| ExportError::Mux("映像のみの MP4 に音声を書き込めません".to_owned()))
    }

    pub(super) fn finish(mut self) -> Result<(), ExportError> {
        self.output
            .write_trailer()
            .map_err(|error| ExportError::Mux(format!("MP4 を確定できません: {error}")))?;
        Ok(())
    }
}

fn compare_timestamps(
    left: i64,
    left_time_base: ffmpeg::Rational,
    right: i64,
    right_time_base: ffmpeg::Rational,
) -> Ordering {
    let left_scaled = i128::from(left)
        * i128::from(left_time_base.numerator())
        * i128::from(right_time_base.denominator());
    let right_scaled = i128::from(right)
        * i128::from(right_time_base.numerator())
        * i128::from(left_time_base.denominator());
    left_scaled.cmp(&right_scaled)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timestamp_comparison_uses_each_stream_time_base() {
        assert_eq!(
            compare_timestamps(
                48_000,
                ffmpeg::Rational(1, 48_000),
                60,
                ffmpeg::Rational(1, 60)
            ),
            Ordering::Equal
        );
        assert_eq!(
            compare_timestamps(
                -1_024,
                ffmpeg::Rational(1, 48_000),
                0,
                ffmpeg::Rational(1, 60)
            ),
            Ordering::Less
        );
    }
}
