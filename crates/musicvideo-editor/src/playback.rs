use std::collections::VecDeque;
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use musicvideo_ui::ResourceRevision;

use crate::decode::AudioData;

// The UI never owns a stream, callback state, or JoinHandle. WASAPI stream drop
// can join internally, so even destruction belongs to musicvideo-audio.
pub struct AudioPlayer {
    tx: mpsc::Sender<Message>,
    published: Arc<Mutex<PlaybackSnapshot>>,
    repaint_ctx: Arc<Mutex<egui::Context>>,
    last_operation: u64,
}

enum Command {
    Replace {
        audio: Option<(Arc<AudioData>, ResourceRevision)>,
        position_secs: f64,
    },
    Toggle,
    Pause,
    Seek(f64),
    Shutdown,
}

enum Message {
    Command { operation: u64, command: Command },
    DeviceError { stream_id: u64, error: String },
}

struct PlayerState {
    audio: Option<(Arc<AudioData>, ResourceRevision)>,
    output_sample_rate: u32,
    source_frames_per_output_frame: f64,
    next_source_frame: f64,
    playing: bool,
    playback_anchors: VecDeque<PlaybackAnchor>,
}

impl Default for PlayerState {
    fn default() -> Self {
        Self {
            audio: None,
            output_sample_rate: 48_000,
            source_frames_per_output_frame: 1.0,
            next_source_frame: 0.0,
            playing: false,
            playback_anchors: VecDeque::new(),
        }
    }
}

#[derive(Clone, Copy)]
struct PlaybackAnchor {
    wall_instant: Instant,
    source_frame: f64,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct PlaybackSnapshot {
    pub applied_operation: u64,
    pub resource_revision: Option<ResourceRevision>,
    pub position_secs: f64,
    pub playing: bool,
    pub error: Option<String>,
}

impl AudioPlayer {
    pub fn new(
        audio: Arc<AudioData>,
        revision: ResourceRevision,
        position_secs: f64,
    ) -> Result<Self, String> {
        Self::spawn(audio, revision, position_secs, open_stream)
    }

    #[cfg(test)]
    pub(crate) fn new_headless(
        audio: Arc<AudioData>,
        revision: ResourceRevision,
        position_secs: f64,
    ) -> Result<Self, String> {
        Self::spawn(audio, revision, position_secs, |_, _, _| Ok(()))
    }

    #[cfg(test)]
    pub(crate) fn new_failing_headless(
        audio: Arc<AudioData>,
        revision: ResourceRevision,
        position_secs: f64,
    ) -> Result<Self, String> {
        Self::spawn::<()>(audio, revision, position_secs, |_, _, _| {
            Err("headless: no device".to_owned())
        })
    }

    fn spawn<S: 'static>(
        audio: Arc<AudioData>,
        revision: ResourceRevision,
        position_secs: f64,
        open: impl FnMut(&Arc<Mutex<PlayerState>>, &mpsc::Sender<Message>, u64) -> Result<S, String>
        + Send
        + 'static,
    ) -> Result<Self, String> {
        let (mut player, rx) = Self::channel();
        player.replace_audio(Some((audio, revision)), position_secs);
        let published = Arc::clone(&player.published);
        let repaint_ctx = Arc::clone(&player.repaint_ctx);
        let tx = player.tx.clone();
        std::thread::Builder::new()
            .name("musicvideo-audio".to_owned())
            .spawn(move || {
                // S may be !Send: it is created, used and dropped inside this closure.
                let mut worker = AudioWorker::new(published, tx, repaint_ctx, open);
                loop {
                    let playing = worker.shared.lock().unwrap().playing;
                    let message = if playing {
                        rx.recv_timeout(Duration::from_millis(10))
                    } else {
                        rx.recv().map_err(|_| mpsc::RecvTimeoutError::Disconnected)
                    };
                    match message {
                        Ok(message) => {
                            if !worker.handle(message) {
                                break;
                            }
                            worker.repaint_ctx.lock().unwrap().request_repaint();
                        }
                        Err(mpsc::RecvTimeoutError::Timeout) => worker.publish(),
                        Err(mpsc::RecvTimeoutError::Disconnected) => break,
                    }
                }
                // Drop stream (including its callback) before the worker's PCM owner.
                worker.stream = None;
            })
            .map_err(|error| format!("音声スレッドを開始できません: {error}"))?;
        Ok(player)
    }

    fn channel() -> (Self, mpsc::Receiver<Message>) {
        let (tx, rx) = mpsc::channel();
        (
            Self {
                tx,
                published: Arc::new(Mutex::new(PlaybackSnapshot::default())),
                repaint_ctx: Arc::new(Mutex::new(egui::Context::default())),
                last_operation: 0,
            },
            rx,
        )
    }

    fn send(&mut self, command: Command) {
        self.last_operation += 1;
        if self
            .tx
            .send(Message::Command {
                operation: self.last_operation,
                command,
            })
            .is_err()
        {
            let error = "音声スレッドが終了しました".to_owned();
            eprintln!("音楽ビデオの音声出力エラー: {error}");
            self.published.lock().unwrap().error = Some(error);
        }
    }

    pub fn toggle(&mut self) {
        self.send(Command::Toggle);
    }

    pub fn pause(&mut self) {
        self.send(Command::Pause);
    }

    pub fn replace_audio(
        &mut self,
        audio: Option<(Arc<AudioData>, ResourceRevision)>,
        position_secs: f64,
    ) {
        self.send(Command::Replace {
            audio,
            position_secs,
        });
    }

    pub fn seek(&mut self, position_secs: f64) {
        self.send(Command::Seek(position_secs));
    }

    pub fn last_operation(&self) -> u64 {
        self.last_operation
    }

    pub fn snapshot(&self) -> PlaybackSnapshot {
        self.published.lock().unwrap().clone()
    }

    pub fn set_repaint_context(&self, ctx: &egui::Context) {
        *self.repaint_ctx.lock().unwrap() = ctx.clone();
    }

    #[cfg(test)]
    pub(crate) fn wait_applied(&self) -> PlaybackSnapshot {
        let (tx, rx) = mpsc::channel();
        let ctx = egui::Context::default();
        ctx.set_request_repaint_callback(move |_| {
            let _ = tx.send(());
        });
        self.set_repaint_context(&ctx);
        loop {
            let snapshot = self.snapshot();
            if snapshot.applied_operation == self.last_operation {
                return snapshot;
            }
            rx.recv_timeout(Duration::from_secs(10))
                .expect("audio command acknowledgement");
        }
    }
}

impl Drop for AudioPlayer {
    fn drop(&mut self) {
        self.send(Command::Shutdown);
    }
}

struct AudioWorker<S, F> {
    shared: Arc<Mutex<PlayerState>>,
    published: Arc<Mutex<PlaybackSnapshot>>,
    tx: mpsc::Sender<Message>,
    repaint_ctx: Arc<Mutex<egui::Context>>,
    open: F,
    stream: Option<S>,
    stream_id: u64,
    applied_operation: u64,
    error: Option<String>,
}

impl<S, F> AudioWorker<S, F>
where
    F: FnMut(&Arc<Mutex<PlayerState>>, &mpsc::Sender<Message>, u64) -> Result<S, String>,
{
    fn new(
        published: Arc<Mutex<PlaybackSnapshot>>,
        tx: mpsc::Sender<Message>,
        repaint_ctx: Arc<Mutex<egui::Context>>,
        open: F,
    ) -> Self {
        Self {
            shared: Arc::new(Mutex::new(PlayerState::default())),
            published,
            tx,
            repaint_ctx,
            open,
            stream: None,
            stream_id: 0,
            applied_operation: 0,
            error: None,
        }
    }

    fn open_device(&mut self) {
        self.stream_id += 1;
        match (self.open)(&self.shared, &self.tx, self.stream_id) {
            Ok(stream) => {
                self.stream = Some(stream);
                self.error = None;
            }
            Err(error) => self.note_error(error),
        }
    }

    fn note_error(&mut self, error: String) {
        pause_state(&mut self.shared.lock().unwrap(), Instant::now());
        eprintln!("音楽ビデオの音声出力エラー: {error}");
        self.error = Some(error);
    }

    fn handle(&mut self, message: Message) -> bool {
        match message {
            Message::Command { operation, command } => {
                match command {
                    Command::Replace {
                        audio,
                        position_secs,
                    } => {
                        replace_audio_state(&mut self.shared.lock().unwrap(), audio, position_secs);
                        // Keep the former initial device-open attempt, but on this thread.
                        if self.applied_operation == 0 {
                            self.open_device();
                        }
                    }
                    Command::Toggle => {
                        if self.stream.is_none() {
                            self.open_device();
                        }
                        let mut state = self.shared.lock().unwrap();
                        if state.playing {
                            pause_state(&mut state, Instant::now());
                        } else if self.stream.is_some() && state.audio.is_some() {
                            if state.next_source_frame >= audio_frames(&state) {
                                state.next_source_frame = 0.0;
                            }
                            state.playback_anchors.clear();
                            state.playing = true;
                        }
                    }
                    Command::Pause => pause_state(&mut self.shared.lock().unwrap(), Instant::now()),
                    Command::Seek(position) => {
                        seek_state(&mut self.shared.lock().unwrap(), position)
                    }
                    Command::Shutdown => {
                        self.stream = None;
                        self.applied_operation = operation;
                        pause_state(&mut self.shared.lock().unwrap(), Instant::now());
                        self.publish();
                        return false;
                    }
                }
                self.applied_operation = operation;
            }
            Message::DeviceError { stream_id, error } => {
                // A dropped stream's queued error cannot stop its replacement.
                if stream_id != self.stream_id || self.stream.is_none() {
                    return true;
                }
                self.note_error(error);
                self.stream = None;
            }
        }
        self.publish();
        true
    }

    fn publish(&self) {
        // Never hold both locks. Neither side of the callback can encounter the
        // UI snapshot lock, even if the UI reader is descheduled while holding it.
        let mut snapshot = snapshot_at(&mut self.shared.lock().unwrap(), Instant::now());
        snapshot.applied_operation = self.applied_operation;
        snapshot.error = self.error.clone();
        *self.published.lock().unwrap() = snapshot;
    }
}

fn open_stream(
    shared: &Arc<Mutex<PlayerState>>,
    tx: &mpsc::Sender<Message>,
    stream_id: u64,
) -> Result<cpal::Stream, String> {
    let host = cpal::default_host();
    let device = host
        .default_output_device()
        .ok_or_else(|| "既定の音声出力デバイスがありません".to_owned())?;
    let supported = device
        .default_output_config()
        .map_err(|error| format!("音声出力形式を取得できません: {error}"))?;
    let format = supported.sample_format();
    let config: cpal::StreamConfig = supported.into();
    {
        let mut state = shared.lock().unwrap();
        state.output_sample_rate = config.sample_rate.0;
        update_step(&mut state);
    }
    macro_rules! build {
        ($sample:ty) => {
            build_stream::<$sample>(&device, &config, shared, tx, stream_id)
        };
    }
    let stream = match format {
        cpal::SampleFormat::I8 => build!(i8),
        cpal::SampleFormat::I16 => build!(i16),
        cpal::SampleFormat::I24 => build!(cpal::I24),
        cpal::SampleFormat::I32 => build!(i32),
        cpal::SampleFormat::I64 => build!(i64),
        cpal::SampleFormat::U8 => build!(u8),
        cpal::SampleFormat::U16 => build!(u16),
        cpal::SampleFormat::U32 => build!(u32),
        cpal::SampleFormat::U64 => build!(u64),
        cpal::SampleFormat::F32 => build!(f32),
        cpal::SampleFormat::F64 => build!(f64),
        _ => return Err(format!("未対応の音声出力形式です: {format:?}")),
    }
    .map_err(|error| format!("音声出力を開始できません: {error}"))?;
    stream
        .play()
        .map_err(|error| format!("音声出力を再生状態にできません: {error}"))?;
    Ok(stream)
}

fn build_stream<T: cpal::SizedSample + cpal::FromSample<f32>>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    shared: &Arc<Mutex<PlayerState>>,
    tx: &mpsc::Sender<Message>,
    stream_id: u64,
) -> Result<cpal::Stream, cpal::BuildStreamError> {
    let shared = Arc::clone(shared);
    let tx = tx.clone();
    let channels = usize::from(config.channels.max(1));
    device.build_output_stream(
        config,
        move |output: &mut [T], info| {
            let timestamp = info.timestamp();
            let now = Instant::now();
            let delay = timestamp
                .playback
                .duration_since(&timestamp.callback)
                .unwrap_or_default();
            fill_output_at(
                output,
                channels,
                &shared,
                now.checked_add(delay).unwrap_or(now),
            );
        },
        move |error| {
            let _ = tx.send(Message::DeviceError {
                stream_id,
                error: error.to_string(),
            });
        },
        None,
    )
}

fn update_step(state: &mut PlayerState) {
    state.source_frames_per_output_frame = state.audio.as_ref().map_or(1.0, |(audio, _)| {
        f64::from(audio.sample_rate) / f64::from(state.output_sample_rate.max(1))
    });
}

fn audio_frames(state: &PlayerState) -> f64 {
    state
        .audio
        .as_ref()
        .map_or(0.0, |(audio, _)| (audio.stereo_pcm.len() / 2) as f64)
}

fn replace_audio_state(
    state: &mut PlayerState,
    audio: Option<(Arc<AudioData>, ResourceRevision)>,
    position_secs: f64,
) {
    state.audio = audio;
    update_step(state);
    seek_state(state, position_secs);
    state.playing = false;
}

fn seek_state(state: &mut PlayerState, position_secs: f64) {
    state.next_source_frame = state.audio.as_ref().map_or(0.0, |(audio, _)| {
        (position_secs.max(0.0) * f64::from(audio.sample_rate))
            .round()
            .clamp(0.0, audio_frames(state))
    });
    state.playback_anchors.clear();
}

fn pause_state(state: &mut PlayerState, now: Instant) {
    state.next_source_frame = playback_position_at(state, now);
    state.playback_anchors.clear();
    state.playing = false;
}

fn snapshot_at(state: &mut PlayerState, now: Instant) -> PlaybackSnapshot {
    let frames = playback_position_at(state, now);
    if frames >= audio_frames(state) {
        state.playing = false;
    }
    PlaybackSnapshot {
        position_secs: state.audio.as_ref().map_or(0.0, |(audio, _)| {
            frames / f64::from(audio.sample_rate.max(1))
        }),
        resource_revision: state.audio.as_ref().map(|(_, revision)| *revision),
        playing: state.playing,
        ..Default::default()
    }
}

fn playback_position_at(state: &PlayerState, now: Instant) -> f64 {
    if !state.playing {
        return state.next_source_frame;
    }
    let Some(anchor) = state
        .playback_anchors
        .iter()
        .rev()
        .find(|anchor| anchor.wall_instant <= now)
    else {
        return state
            .playback_anchors
            .front()
            .map(|anchor| anchor.source_frame)
            .unwrap_or(state.next_source_frame);
    };
    let rate = state
        .audio
        .as_ref()
        .map_or(1, |(audio, _)| audio.sample_rate);
    let elapsed_frames = now.duration_since(anchor.wall_instant).as_secs_f64() * f64::from(rate);
    (anchor.source_frame + elapsed_frames)
        .min(state.next_source_frame)
        .min(audio_frames(state))
}

fn fill_output_at<T: cpal::SizedSample + cpal::FromSample<f32>>(
    output: &mut [T],
    channels: usize,
    shared: &Arc<Mutex<PlayerState>>,
    playback_wall: Instant,
) {
    let Ok(mut state) = shared.lock() else {
        output.fill(T::EQUILIBRIUM);
        return;
    };
    let available = audio_frames(&state) as usize;
    if state.playing && state.next_source_frame < available as f64 {
        let source_frame = state.next_source_frame;
        state.playback_anchors.push_back(PlaybackAnchor {
            wall_instant: playback_wall,
            source_frame,
        });
        while state.playback_anchors.len() > 4 {
            state.playback_anchors.pop_front();
        }
    }
    for output_frame in output.chunks_mut(channels) {
        let (left, right) = if state.playing && state.next_source_frame < available as f64 {
            let frame = state.next_source_frame.floor() as usize;
            let next = frame.saturating_add(1).min(available.saturating_sub(1));
            let amount = (state.next_source_frame - frame as f64) as f32;
            let audio = &state.audio.as_ref().unwrap().0;
            let left = lerp(
                audio.stereo_pcm[frame * 2],
                audio.stereo_pcm[next * 2],
                amount,
            );
            let right = lerp(
                audio.stereo_pcm[frame * 2 + 1],
                audio.stereo_pcm[next * 2 + 1],
                amount,
            );
            state.next_source_frame = (state.next_source_frame
                + state.source_frames_per_output_frame)
                .min(available as f64);
            (left, right)
        } else {
            (0.0, 0.0)
        };
        for (channel, sample) in output_frame.iter_mut().enumerate() {
            let value = match channel {
                0 => left,
                1 => right,
                _ => (left + right) * 0.5,
            };
            // Integer conversion needs the normalized range; floats retain the
            // former f32 behavior (including values above unity).
            *sample = T::from_sample(if T::FORMAT.is_float() {
                value
            } else {
                value.clamp(-1.0, 1.0 - f32::EPSILON)
            });
        }
    }
}

fn lerp(from: f32, to: f32, amount: f32) -> f32 {
    from + (to - from) * amount
}

#[cfg(test)]
mod tests {
    use super::*;
    use musicvideo_ui::ResourceLease;
    use std::time::Duration;

    fn revision() -> ResourceRevision {
        ResourceLease::new(Arc::new(())).revision()
    }

    fn audio() -> Arc<AudioData> {
        Arc::new(AudioData {
            stereo_pcm: vec![0.25; 48_000 * 2],
            sample_rate: 48_000,
            duration_secs: 1.0,
        })
    }

    fn harness<F, S>(open: F) -> (AudioPlayer, mpsc::Receiver<Message>, AudioWorker<S, F>)
    where
        F: FnMut(&Arc<Mutex<PlayerState>>, &mpsc::Sender<Message>, u64) -> Result<S, String>,
    {
        let (player, rx) = AudioPlayer::channel();
        let worker = AudioWorker::new(
            Arc::clone(&player.published),
            player.tx.clone(),
            Arc::clone(&player.repaint_ctx),
            open,
        );
        (player, rx, worker)
    }

    #[test]
    fn commands_apply_fifo_for_replace_then_seek_and_seek_then_replace() {
        let (mut player, rx, mut worker) = harness(|_, _, _| Ok(()));
        let pcm = audio();
        let first = revision();
        player.replace_audio(Some((pcm.clone(), first)), 0.1);
        player.seek(0.6);
        worker.handle(rx.recv().unwrap());
        assert_eq!(player.snapshot().position_secs, 0.1);
        assert_eq!(player.snapshot().applied_operation, 1);
        worker.handle(rx.recv().unwrap());
        assert_eq!(player.snapshot().position_secs, 0.6);
        assert_eq!(player.snapshot().applied_operation, 2);
        player.seek(0.8);
        let second = revision();
        player.replace_audio(Some((pcm, second)), 0.2);
        worker.handle(rx.recv().unwrap());
        assert_eq!(player.snapshot().position_secs, 0.8);
        worker.handle(rx.recv().unwrap());
        assert_eq!(player.snapshot().position_secs, 0.2);
        assert_eq!(player.snapshot().resource_revision, Some(second));
        assert_eq!(player.last_operation(), 4);
        assert_eq!(player.snapshot().applied_operation, 4);
    }

    #[test]
    fn replacement_publishes_revision_stops_playback_and_clears_anchors() {
        let (mut player, rx, mut worker) = harness(|_, _, _| Ok(()));
        player.replace_audio(Some((audio(), revision())), 0.0);
        worker.handle(rx.recv().unwrap());
        player.toggle();
        worker.handle(rx.recv().unwrap());
        fill_output_at(&mut [0.0_f32; 128], 2, &worker.shared, Instant::now());
        assert!(!worker.shared.lock().unwrap().playback_anchors.is_empty());
        let new_revision = revision();
        player.replace_audio(Some((audio(), new_revision)), 0.25);
        worker.handle(rx.recv().unwrap());
        let snapshot = player.snapshot();
        assert!(!snapshot.playing);
        assert_eq!(snapshot.resource_revision, Some(new_revision));
        assert_eq!(snapshot.applied_operation, 3);
        assert_eq!(snapshot.position_secs, 0.25);
        assert!(worker.shared.lock().unwrap().playback_anchors.is_empty());
    }

    #[test]
    fn pause_and_seek_preserve_the_audible_clock_and_operation_numbers() {
        let (mut player, rx, mut worker) = harness(|_, _, _| Ok(()));
        player.replace_audio(Some((audio(), revision())), 0.0);
        worker.handle(rx.recv().unwrap());
        player.toggle();
        worker.handle(rx.recv().unwrap());
        let future = Instant::now() + Duration::from_secs(10);
        fill_output_at(&mut [0.0_f32; 24_000], 2, &worker.shared, future);
        player.pause();
        worker.handle(rx.recv().unwrap());
        let paused = player.snapshot();
        assert_eq!(paused.applied_operation, 3);
        assert_eq!(paused.position_secs, 0.0, "queued PCM is not yet audible");
        assert!(!paused.playing);
        player.seek(0.45);
        worker.handle(rx.recv().unwrap());
        assert_eq!(player.snapshot().applied_operation, 4);
        assert_eq!(player.snapshot().position_secs, 0.45);
        assert!(!player.snapshot().playing);
    }

    fn converted<T: cpal::SizedSample + cpal::FromSample<f32>>(initial: T) -> Vec<T> {
        let pcm = Arc::new(AudioData {
            stereo_pcm: vec![-1.0, -0.5, 0.0, 0.5, 0.25, -0.25],
            sample_rate: 48_000,
            duration_secs: 3.0 / 48_000.0,
        });
        let mut state = PlayerState::default();
        replace_audio_state(&mut state, Some((pcm, revision())), 0.0);
        state.playing = true;
        let mut output = vec![initial; 8];
        fill_output_at(&mut output, 2, &Arc::new(Mutex::new(state)), Instant::now());
        output
    }

    #[test]
    fn callback_converts_f32_pcm_and_writes_float_silence() {
        assert_eq!(
            converted(99.0_f32),
            [-1.0, -0.5, 0.0, 0.5, 0.25, -0.25, 0.0, 0.0]
        );
    }

    #[test]
    fn callback_converts_i16_pcm_and_writes_signed_silence() {
        assert_eq!(
            converted(99_i16),
            [-32768, -16384, 0, 16384, 8192, -8192, 0, 0]
        );
    }

    #[test]
    fn callback_converts_u16_pcm_and_writes_unsigned_silence() {
        assert_eq!(
            converted(99_u16),
            [0, 16384, 32768, 49152, 40960, 24576, 32768, 32768]
        );
    }

    #[test]
    fn snapshot_and_pcm_locks_are_independent_in_both_directions() {
        let (mut player, rx, mut worker) = harness(|_, _, _| Ok(()));
        player.replace_audio(Some((audio(), revision())), 0.25);
        worker.handle(rx.recv().unwrap());
        std::thread::scope(|scope| {
            let guard = worker.shared.lock().unwrap();
            let (tx, rx) = mpsc::channel();
            let player = &player;
            scope.spawn(move || {
                tx.send(player.snapshot()).unwrap();
            });
            let result = rx.recv_timeout(Duration::from_secs(2));
            drop(guard); // Release before asserting, including on a broken implementation.
            assert_eq!(
                result.expect("UI must not wait for PCM lock").position_secs,
                0.25
            );
        });
        std::thread::scope(|scope| {
            let guard = player.published.lock().unwrap();
            let (tx, rx) = mpsc::channel();
            let shared = &worker.shared;
            scope.spawn(move || {
                let mut output = [1.0_f32; 8];
                fill_output_at(&mut output, 2, shared, Instant::now());
                tx.send(output).unwrap();
            });
            let result = rx.recv_timeout(Duration::from_secs(2));
            drop(guard);
            assert_eq!(
                result.expect("callback must not wait for UI lock"),
                [0.0; 8]
            );
        });
    }

    #[test]
    fn device_failure_retries_only_on_next_play_and_loss_reports_an_error() {
        let mut attempts = 0;
        let (mut player, rx, mut worker) = harness(move |_, _, _| {
            attempts += 1;
            if attempts == 1 {
                Err("no device".to_owned())
            } else {
                Ok(())
            }
        });
        player.replace_audio(Some((audio(), revision())), 0.25);
        worker.handle(rx.recv().unwrap());
        assert_eq!(player.snapshot().error.as_deref(), Some("no device"));
        assert!(!player.snapshot().playing);
        let initial_id = worker.stream_id;
        worker.publish();
        player.seek(0.5);
        worker.handle(rx.recv().unwrap());
        assert_eq!(
            worker.stream_id, initial_id,
            "seek and publish must not retry"
        );
        player.toggle();
        worker.handle(rx.recv().unwrap());
        assert!(player.snapshot().playing);
        assert!(player.snapshot().error.is_none());
        worker.handle(Message::DeviceError {
            stream_id: worker.stream_id,
            error: "lost".to_owned(),
        });
        assert_eq!(player.snapshot().error.as_deref(), Some("lost"));
        assert!(!player.snapshot().playing);
        assert!(worker.stream.is_none());
        player.toggle();
        worker.handle(rx.recv().unwrap());
        assert!(player.snapshot().playing);
        assert!(player.snapshot().error.is_none());
        worker.handle(Message::DeviceError {
            stream_id: initial_id,
            error: "stale".to_owned(),
        });
        assert!(player.snapshot().playing);
        assert!(player.snapshot().error.is_none());
    }

    #[test]
    fn shutdown_drops_non_send_stream_and_pcm_on_audio_thread_without_ui_wait() {
        struct StreamProbe {
            _non_send: std::rc::Rc<()>,
            drop_started: mpsc::Sender<String>,
            release_drop: mpsc::Receiver<()>,
        }
        impl Drop for StreamProbe {
            fn drop(&mut self) {
                self.drop_started
                    .send(std::thread::current().name().unwrap().to_owned())
                    .unwrap();
                self.release_drop
                    .recv_timeout(Duration::from_secs(10))
                    .unwrap();
            }
        }
        let (opened_tx, opened_rx) = mpsc::channel();
        let (drop_tx, drop_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let (exited_tx, exited_rx) = mpsc::channel();
        struct ExitProbe(mpsc::Sender<String>);
        impl Drop for ExitProbe {
            fn drop(&mut self) {
                self.0
                    .send(std::thread::current().name().unwrap().to_owned())
                    .unwrap();
            }
        }
        let mut release_rx = Some(release_rx);
        let exit_probe = ExitProbe(exited_tx);
        let pcm = audio();
        let weak = Arc::downgrade(&pcm);
        let player = AudioPlayer::spawn(pcm, revision(), 0.0, move |_, _, _| {
            let _keep_alive = &exit_probe;
            opened_tx
                .send(std::thread::current().name().unwrap().to_owned())
                .unwrap();
            Ok(StreamProbe {
                _non_send: std::rc::Rc::new(()),
                drop_started: drop_tx.clone(),
                release_drop: release_rx.take().unwrap(),
            })
        })
        .unwrap();
        assert_eq!(
            opened_rx.recv_timeout(Duration::from_secs(10)).unwrap(),
            "musicvideo-audio"
        );
        drop(player); // Must return before the deliberately blocked stream drop.
        assert_eq!(
            drop_rx.recv_timeout(Duration::from_secs(10)).unwrap(),
            "musicvideo-audio"
        );
        assert!(
            weak.upgrade().is_some(),
            "PCM remains owned until stream drop finishes"
        );
        release_tx.send(()).unwrap();
        assert_eq!(
            exited_rx.recv_timeout(Duration::from_secs(10)).unwrap(),
            "musicvideo-audio"
        );
        assert!(weak.upgrade().is_none(), "worker must release PCM on exit");
    }

    #[test]
    fn snapshot_uses_the_audio_cursor_as_time() {
        let mut state = PlayerState {
            audio: Some((
                Arc::new(AudioData {
                    stereo_pcm: vec![0.0; 96_000],
                    sample_rate: 48_000,
                    duration_secs: 1.0,
                }),
                revision(),
            )),
            output_sample_rate: 48_000,
            source_frames_per_output_frame: 1.0,
            next_source_frame: 12_000.0,
            playing: true,
            playback_anchors: VecDeque::from([PlaybackAnchor {
                wall_instant: Instant::now(),
                source_frame: 12_000.0,
            }]),
        };
        let now = state.playback_anchors[0].wall_instant;
        assert_eq!(
            snapshot_at(&mut state, now),
            PlaybackSnapshot {
                position_secs: 0.25,
                playing: true,
                resource_revision: state.audio.as_ref().map(|(_, revision)| *revision),
                ..Default::default()
            }
        );
    }

    #[test]
    fn replacing_audio_reuses_the_player_state_and_pauses_at_the_requested_time() {
        let old = Arc::new(AudioData {
            stereo_pcm: vec![0.0; 96_000],
            sample_rate: 48_000,
            duration_secs: 1.0,
        });
        let mut state = PlayerState {
            audio: Some((old, revision())),
            output_sample_rate: 44_100,
            source_frames_per_output_frame: 48_000.0 / 44_100.0,
            next_source_frame: 1_000.0,
            playing: true,
            playback_anchors: VecDeque::new(),
        };
        let replacement = Arc::new(AudioData {
            stereo_pcm: vec![0.5; 192_000],
            sample_rate: 96_000,
            duration_secs: 1.0,
        });

        replace_audio_state(
            &mut state,
            Some((Arc::clone(&replacement), revision())),
            0.25,
        );

        assert!(Arc::ptr_eq(&state.audio.as_ref().unwrap().0, &replacement));
        assert_eq!(state.source_frames_per_output_frame, 96_000.0 / 44_100.0);
        assert_eq!(state.next_source_frame, 24_000.0);
        assert!(!state.playing);
        assert!(state.playback_anchors.is_empty());
    }

    #[test]
    fn output_rate_conversion_advances_the_source_clock_without_a_second_buffer() {
        let shared = Arc::new(Mutex::new(PlayerState {
            audio: Some((
                Arc::new(AudioData {
                    stereo_pcm: vec![0.25; 48_000 * 2],
                    sample_rate: 48_000,
                    duration_secs: 1.0,
                }),
                revision(),
            )),
            output_sample_rate: 44_100,
            source_frames_per_output_frame: 48_000.0 / 44_100.0,
            next_source_frame: 0.0,
            playing: true,
            playback_anchors: VecDeque::new(),
        }));
        let mut output = vec![0.0; 44_100 * 2];
        let playback_wall = Instant::now();
        fill_output_at(&mut output, 2, &shared, playback_wall);

        assert!(output.iter().all(|sample| *sample == 0.25));
        let mut state = shared.lock().unwrap();
        assert!(
            (snapshot_at(&mut state, playback_wall + Duration::from_secs(1)).position_secs - 1.0)
                .abs()
                < 0.000_001
        );
        assert_eq!(Arc::strong_count(&state.audio.as_ref().unwrap().0), 1);
    }

    #[test]
    fn large_callback_buffer_does_not_make_the_audio_clock_jump() {
        let shared = Arc::new(Mutex::new(PlayerState {
            audio: Some((
                Arc::new(AudioData {
                    stereo_pcm: vec![0.25; 48_000 * 2],
                    sample_rate: 48_000,
                    duration_secs: 1.0,
                }),
                revision(),
            )),
            output_sample_rate: 48_000,
            source_frames_per_output_frame: 1.0,
            next_source_frame: 0.0,
            playing: true,
            playback_anchors: VecDeque::new(),
        }));
        let mut output = vec![0.0; 24_000 * 2];
        let playback_wall = Instant::now() + Duration::from_millis(40);
        fill_output_at(&mut output, 2, &shared, playback_wall);

        let mut state = shared.lock().unwrap();
        assert_eq!(
            snapshot_at(&mut state, playback_wall - Duration::from_millis(20)).position_secs,
            0.0
        );
        let quarter = snapshot_at(&mut state, playback_wall + Duration::from_millis(250));
        assert!((quarter.position_secs - 0.25).abs() < 0.000_001);
    }
}
