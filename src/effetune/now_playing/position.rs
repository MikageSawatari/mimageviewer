//! Trial position sender. Only the host-control worker polls the owning player's clock.
use super::{Core, PositionSource};
use crate::video::dsp::{DspBridge, bridge::Cmd};
use std::{
    sync::Weak,
    time::{Duration, Instant},
};

pub(super) const POLL: Duration = Duration::from_millis(200);

pub(crate) fn host_time_ns() -> u64 {
    #[cfg(windows)]
    {
        use windows::Win32::System::Performance::{
            QueryPerformanceCounter, QueryPerformanceFrequency,
        };
        static FREQUENCY: std::sync::OnceLock<i64> = std::sync::OnceLock::new();
        let frequency = *FREQUENCY.get_or_init(|| {
            let mut value = 0;
            unsafe {
                QueryPerformanceFrequency(&mut value).expect("QPC frequency");
            }
            value
        });
        let mut counter = 0;
        unsafe {
            QueryPerformanceCounter(&mut counter).expect("QPC counter");
        }
        (counter as u128 * 1_000_000_000 / frequency as u128) as u64
    }
    #[cfg(not(windows))]
    {
        0
    }
}

#[derive(Clone, Copy, Debug)]
struct Sample {
    position: f64,
    playing: bool,
    rate: f64,
    time: u64,
}
impl Sample {
    fn needs_send(self, old: Self) -> bool {
        let elapsed = self.time.saturating_sub(old.time) as f64 / 1e9;
        let predicted = old.position + if old.playing { elapsed * old.rate } else { 0.0 };
        self.playing != old.playing
            || self.rate != old.rate
            || elapsed >= 1.0
            || (self.position - predicted).abs() > 0.020
    }
}

#[derive(Default)]
pub(in crate::effetune) struct Sender {
    generation: Option<u64>,
    next_poll: Option<Instant>,
    sent: Option<Sample>,
}
impl Sender {
    pub(in crate::effetune) fn reset(&mut self) {
        *self = Self::default();
    }
    pub(in crate::effetune) fn wait(&self) -> Option<Duration> {
        self.next_poll
            .map(|deadline| deadline.saturating_duration_since(Instant::now()))
    }
    pub(in crate::effetune) fn update(&mut self, core: &Core, destination: &Weak<DspBridge>) {
        self.update_at(core, destination, Instant::now(), |clock| {
            let position = clock.now_secs();
            let time = host_time_ns();
            Sample {
                position,
                time,
                playing: clock.is_playing(),
                rate: clock.playback_speed(),
            }
        });
    }
    fn update_at(
        &mut self,
        core: &Core,
        destination: &Weak<DspBridge>,
        now: Instant,
        read_clock: impl FnOnce(&crate::video::clock::AvClock) -> Sample,
    ) {
        let Some(bridge) = destination.upgrade().filter(|_| core.active()) else {
            self.reset();
            return;
        };
        let (generation, source) = core.position();
        if self.generation != Some(generation) {
            self.generation = Some(generation);
            self.sent = None;
            self.next_poll = matches!(source, PositionSource::Local { .. }).then_some(now + POLL);
            let command = Cmd::SetPlaybackPosition {
                slot_id: 0,
                media_generation: generation,
                available: 0,
                playing: 0,
                position_seconds: 0.0,
                rate: 1.0,
                host_time_ns: host_time_ns(),
            };
            self.send(&bridge, command);
            return;
        }
        let PositionSource::Local { clock, diagnostics } = source else {
            self.next_poll = None;
            return;
        };
        if self.next_poll.is_some_and(|deadline| now < deadline) {
            return;
        }
        self.next_poll = Some(now + POLL);
        let sample = read_clock(&clock);
        let Sample { position, time, .. } = sample;
        if !self.sent.is_none_or(|old| sample.needs_send(old)) {
            return;
        }
        self.sent = Some(sample);
        let command = Cmd::SetPlaybackPosition {
            slot_id: 0,
            media_generation: generation,
            available: 1,
            playing: u32::from(sample.playing),
            position_seconds: position,
            rate: sample.rate,
            host_time_ns: time,
        };
        // 試作 D 記録: never subtract a stale callback PTS from this later clock read.
        if crate::perf::is_enabled() {
            let report = diagnostics.lyrics_trial_lead.read_report();
            let lead_status = match &report {
                Ok(Some(_)) => "coherent",
                Ok(None) => "not_sampled",
                Err(()) => "changed",
            };
            let lead = report.ok().flatten();
            crate::perf::event(
                "effetune",
                "lyrics_position",
                None,
                0,
                &[
                    ("media_generation", generation.into()),
                    ("available", true.into()),
                    ("position_seconds", position.into()),
                    ("host_time_ns", time.into()),
                    ("playing", sample.playing.into()),
                    ("rate", sample.rate.into()),
                    ("seek_serial", clock.current_seek_serial().into()),
                    ("lead_coherent", lead.is_some().into()),
                    ("lead_status", lead_status.into()),
                    ("lead_seq", lead.as_ref().map(|s| s.sequence).into()),
                    ("lead_ms", lead.as_ref().map(|s| s.lead_ms).into()),
                    (
                        "lead_pcm_serial",
                        lead.as_ref().map(|s| s.pump_serial).into(),
                    ),
                    (
                        "lead_clock_serial_before",
                        lead.as_ref().map(|s| s.clock_serial_before).into(),
                    ),
                    (
                        "lead_clock_serial_after",
                        lead.as_ref().map(|s| s.clock_serial_after).into(),
                    ),
                    ("lead_wall_ns", lead.as_ref().map(|s| s.wall_ns).into()),
                ],
            );
        }
        self.send(&bridge, command);
    }
    fn send(&self, bridge: &DspBridge, command: Cmd) {
        if let Cmd::SetPlaybackPosition {
            available: 0,
            media_generation,
            host_time_ns,
            ..
        } = &command
            && crate::perf::is_enabled()
        {
            crate::perf::event(
                "effetune",
                "lyrics_position",
                None,
                0,
                &[
                    ("media_generation", (*media_generation).into()),
                    ("available", false.into()),
                    ("host_time_ns", (*host_time_ns).into()),
                ],
            );
        }
        if let Err(error) = bridge.send_playback_position(command) {
            crate::logger::log(format!("[EffeTune lyrics] position send failed: {error}"));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex, atomic::AtomicU64, mpsc};

    #[test]
    fn lyrics_sender_owner_lifecycle_and_fake_clock() {
        use super::super::{NowPlaying, NowPlayingArtworkSource, NowPlayingInput, NowPlayingOwner};
        let (tx, _rx) = mpsc::channel();
        let now = NowPlaying::new(true, tx, Arc::new(Mutex::new(None)));
        now.set_active(true);
        let clock = Arc::new(crate::video::clock::AvClock::new(
            1.0,
            Arc::new(AtomicU64::new(0)),
        ));
        let diagnostics = Arc::new(crate::video::audio_diagnostics::AudioDiagnostics::new(
            Instant::now(),
        ));
        let input = |path: &str| NowPlayingInput {
            path: path.into(),
            title: None,
            album: None,
            artist: None,
            artwork: NowPlayingArtworkSource::None,
            lyrics_embedded: Some("lyrics".into()),
            duration_seconds: Some(60.0),
            position_source: PositionSource::Local {
                clock: clock.clone(),
                diagnostics: diagnostics.clone(),
            },
        };
        let bridge = crate::effetune::new_bridge();
        let (host, commands, _) = crate::video::dsp::bridge::Bridge::fake_host_for_test();
        bridge.install_fake_host_for_test(host);
        let destination = Arc::downgrade(&bridge);
        let base = Instant::now();
        let mut sender = Sender::default();
        let a = NowPlayingOwner::Local(1);
        let b = NowPlayingOwner::Local(2);
        now.publish(a, input("a.flac"));
        let generation_a = now.core.snapshot().media_generation;
        let sample = Sample {
            position: 2.0,
            playing: true,
            rate: 1.0,
            time: 1_000_000_000,
        };
        sender.update_at(&now.core, &destination, base, |_| {
            panic!("generation invalidation must precede clock read")
        });
        assert_eq!(commands.recv().unwrap()["available"], 0);
        assert!(sender.wait().is_some());
        sender.update_at(&now.core, &destination, base + POLL, |_| sample);
        let command = commands.recv().unwrap();
        assert_eq!(command["mediaGeneration"], generation_a);
        assert_eq!(command["positionSeconds"], 2.0);
        for step in 2..=5 {
            sender.update_at(&now.core, &destination, base + POLL * step, |_| Sample {
                position: 2.0 + (step - 1) as f64 * 0.2,
                time: 1_000_000_000 + (step - 1) as u64 * 200_000_000,
                ..sample
            });
            assert!(commands.try_recv().is_err());
        }
        sender.update_at(&now.core, &destination, base + POLL * 6, |_| Sample {
            position: 3.0,
            time: 2_000_000_000,
            ..sample
        });
        commands.recv().unwrap(); // One-second resend.
        sender.update_at(&now.core, &destination, base + POLL * 7, |_| Sample {
            playing: false,
            position: 3.0,
            time: 2_200_000_000,
            ..sample
        });
        assert_eq!(commands.recv().unwrap()["playing"], 0);
        sender.update_at(&now.core, &destination, base + POLL * 8, |_| Sample {
            playing: false,
            position: 40.0,
            rate: 2.0,
            time: 2_400_000_000,
            ..sample
        });
        assert_eq!(commands.recv().unwrap()["positionSeconds"], 40.0);
        now.publish(b, input("b.flac"));
        sender.update_at(&now.core, &destination, base + POLL * 8, |_| {
            panic!("new generation first invalidates")
        });
        let generation_b = commands.recv().unwrap()["mediaGeneration"]
            .as_u64()
            .unwrap();
        assert!(generation_b > generation_a);
        now.clear(a); // Old owner cannot invalidate B.
        assert_eq!(now.core.snapshot().media_generation, generation_b);
        now.publish(a, input("a.flac"));
        assert!(now.core.snapshot().media_generation > generation_b);
        let remote = NowPlayingOwner::Remote {
            session: 3,
            registration: 1,
        };
        now.publish(remote, input("remote.flac"));
        sender.update_at(&now.core, &destination, base + POLL * 9, |_| {
            panic!("Remote never reads local clock")
        });
        assert_eq!(commands.recv().unwrap()["available"], 0);
        assert!(sender.wait().is_none());
        assert!(now.core.snapshot().lyrics_embedded.is_none());
        assert!(now.core.snapshot().lyrics_sidecar.is_none());
        sender.reset(); // RegisterBridge replays even the same generation.
        sender.update_at(&now.core, &destination, base + POLL * 10, |_| {
            panic!("Remote")
        });
        assert_eq!(commands.recv().unwrap()["available"], 0);
        now.clear(remote);
        sender.update_at(&now.core, &destination, base + POLL * 11, |_| {
            panic!("Empty")
        });
        assert_eq!(commands.recv().unwrap()["available"], 0);
        assert!(sender.wait().is_none());
        now.clear(a);
        assert!(now.core.snapshot().lyrics_embedded.is_none());
    }
    #[test]
    fn lyrics_sender_prediction_pause_seek_rate_and_resend() {
        let first = Sample {
            position: 2.0,
            playing: true,
            rate: 1.0,
            time: 1_000_000_000,
        };
        let next = Sample {
            position: 2.2,
            time: 1_200_000_000,
            ..first
        };
        assert!(!next.needs_send(first));
        assert!(
            Sample {
                playing: false,
                ..next
            }
            .needs_send(first)
        );
        assert!(Sample { rate: 2.0, ..next }.needs_send(first));
        assert!(
            Sample {
                position: 20.0,
                ..next
            }
            .needs_send(first)
        );
        assert!(
            Sample {
                position: 3.0,
                time: 2_000_000_000,
                ..first
            }
            .needs_send(first)
        );
        let paused = Sample {
            playing: false,
            ..first
        };
        assert!(
            !Sample {
                time: 1_200_000_000,
                ..paused
            }
            .needs_send(paused)
        );
        assert!(Sender::default().wait().is_none());
    }
    #[test]
    fn lyrics_qpc_is_monotonic() {
        let a = host_time_ns();
        let b = host_time_ns();
        assert!(b >= a);
    }
}
