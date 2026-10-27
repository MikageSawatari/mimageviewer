//! HarmNet L pitch probabilities, ported from EffeTune DSP 0.13.0 (MIT).
//! Window and inference state belong to one spectrum worker; no GUI or audio I/O.
//! See third_party/effetune-harmnet for copyright and weight provenance.

pub const HARMNET_HOP_SECS: f64 = 0.02;
pub const HARMNET_HOP_SAMPLES: usize = 960;
pub const HARMNET_LONG_WINDOW_SAMPLES: usize = 16_384;
pub const HARMNET_PITCH_COUNT: usize = 88;
pub const HARMNET_HEAD_AGES: [usize; 4] = [0, 2, 4, 8];
const FLOOR_POWER: f64 = 2.511886431509582e-10;
const WEIGHT_BYTES: &[u8; 188_048] = include_bytes!("../assets/harmnet_l_effetune_dsp_0.13.0.bin");

fn weights() -> &'static [f32] {
    static WEIGHTS: OnceLock<Box<[f32]>> = OnceLock::new();
    WEIGHTS.get_or_init(|| {
        WEIGHT_BYTES
            .chunks_exact(4)
            .map(|c| f32::from_le_bytes(c.try_into().unwrap()))
            .collect()
    })
}

/// A causal sequence of 20ms frames. Reset before jumping to a different sequence.
/// Each frame borrows mono history ending at the frame's absolute hop boundary.
/// Short history is padded with zeros on the left. The four heads estimate ages
/// 0, 2, 4, and 8; silence is checked at each head's target frame, not the newest.
pub struct HarmNetTracker {
    supported: bool,
    frontends: [Frontend; 2],
    network: Net,
    features: [[f32; BINS]; 2],
    below_floor: [bool; 9],
    history_head: usize,
}

impl HarmNetTracker {
    pub fn new(sample_rate: u32) -> Self {
        Self {
            supported: sample_rate == 48_000,
            // Only the supported rate needs a frontend. Unsupported inputs produce
            // valid zeros without planning a potentially huge or zero-length FFT.
            frontends: [
                Frontend::new(48_000.0, false),
                Frontend::new(48_000.0, true),
            ],
            network: Net::new(),
            features: [[0.0; BINS]; 2],
            below_floor: [true; 9],
            history_head: 8,
        }
    }

    /// Clears recurrent/convolution and target-frame silence history, without allocating.
    pub fn reset(&mut self) {
        self.network.reset();
        self.features.fill([0.0; BINS]);
        self.below_floor.fill(true);
        self.history_head = 8;
    }

    /// Performs one full frontend/inference frame without heap allocation.
    pub fn push_frame(&mut self, history: &[f32]) {
        if !self.supported {
            return;
        }
        self.history_head = (self.history_head + 1) % self.below_floor.len();
        let n = self.frontends[0].window.len();
        let short = &history[history.len().saturating_sub(n)..];
        let sum: f64 = short.iter().map(|v| f64::from(*v)).sum();
        let squares: f64 = short.iter().map(|v| f64::from(*v).powi(2)).sum();
        let mean = sum / n as f64;
        let variance = squares / n as f64 - mean * mean;
        self.below_floor[self.history_head] = variance < FLOOR_POWER || variance.is_nan();
        for (frontend, features) in self.frontends.iter_mut().zip(self.features.iter_mut()) {
            frontend.features(history, features);
        }
        self.network.step([&self.features[0], &self.features[1]]);
    }

    /// MIDI 21..=108 probabilities. Unsupported rates and unavailable past targets
    /// are valid all-zero results. Head indexes outside 0..4 also return zeros.
    pub fn probabilities(&self, head: usize) -> [f32; HARMNET_PITCH_COUNT] {
        let mut result = [0.0; HARMNET_PITCH_COUNT];
        let Some(&age) = HARMNET_HEAD_AGES.get(head) else {
            return result;
        };
        let target = (self.history_head + self.below_floor.len() - age) % self.below_floor.len();
        if self.supported && !self.below_floor[target] {
            for (pitch, value) in result.iter_mut().enumerate() {
                *value = self.network.probability(pitch, head);
            }
        }
        result
    }
}

use rustfft::{Fft, FftPlanner, num_complex::Complex32};
use std::sync::{Arc, OnceLock};

const K_INPUT_SCALE: usize = 0;
const K_INPUT_SHIFT: usize = 600;
const K_CONV0_W: usize = 1200;
const K_CONV0_B: usize = 1872;
const K_CONV1_W: usize = 1888;
const K_CONV1_B: usize = 5728;
const K_CONV2_W: usize = 5744;
const K_CONV2_B: usize = 9584;
const K_PROJ_W: usize = 9600;
const K_PROJ_B: usize = 16512;
const K_PITCH_EMB: usize = 16560;
const K_PCONV_W: usize = 20784;
const K_PCONV_B: usize = 32304;
const K_GRU_IW: usize = 32352;
const K_GRU_HW: usize = 39264;
const K_GRU_IB: usize = 46176;
const K_GRU_HB: usize = 46320;
const K_HEAD_W: usize = 46464;
const K_HEAD_B: usize = 46656;
const K_PITCH_BIAS: usize = 46660;
const CALIBRATION: [f32; 4] = [0.25, 0.20, 0.20, 0.20];

const BINS: usize = 300;
const CH: usize = 16;
const HID: usize = 48;
const HARM: usize = 9;
const PITCHES: usize = 88;
const HEADS: usize = 4;
const PAD: usize = 8;
const STRIDE: usize = 316;
const GATES: usize = 3 * HID;

fn midi_hz(m: f64) -> f64 {
    440.0 * ((m - 69.0) / 12.0).exp2()
}

struct Frontend {
    window: Vec<f32>,
    fft: Arc<dyn Fft<f32>>,
    fft_size: usize,
    buf: Vec<Complex32>,
    scratch: Vec<Complex32>,
    mag: Vec<f32>,
    rows: Vec<(usize, Vec<f32>)>,
}

impl Frontend {
    fn new(rate: f64, long: bool) -> Self {
        let window_length = (rate * if long { 16384.0 } else { 4096.0 } / 48000.0).round() as usize;
        let mut fft_size = 32;
        while fft_size < window_length {
            fft_size *= 2;
        }
        let fall = (rate * if long { 0.02 } else { 0.01 }).round() as usize;
        let rise = window_length - fall;
        let pi = std::f64::consts::PI;
        let raw: Vec<f64> = (0..window_length)
            .map(|i| {
                if i < rise {
                    0.5 * (1.0 - (pi * (i as f64 + 0.5) / rise as f64).cos())
                } else {
                    0.5 * (1.0 + (pi * ((i - rise) as f64 + 0.5) / fall as f64).cos())
                }
            })
            .collect();
        let sum: f64 = raw.iter().sum();
        let window = raw.iter().map(|v| (v / sum) as f32).collect();
        let bins = fft_size / 2 + 1;
        let df = rate / fft_size as f64;
        let mut rows = Vec::with_capacity(BINS);
        for k in 0..BINS {
            let midi = 21.0 + (k as f64 - 1.0) / 3.0;
            let (center, low, high) = (
                midi_hz(midi),
                midi_hz(midi - 1.0 / 3.0),
                midi_hz(midi + 1.0 / 3.0),
            );
            if center > rate * 0.5 {
                rows.push((0, Vec::new()));
                continue;
            }
            let mut column = vec![0.0f64; bins];
            let mut total = 0.0;
            for (j, cell) in column.iter_mut().enumerate() {
                let hz = j as f64 * df;
                let a = (hz - low) / (center - low);
                let b = (high - hz) / (high - center);
                let v = a.min(b).max(0.0);
                *cell = v;
                total += v;
            }
            if (high - low) / 2.0 > df && total > 0.0 {
                for v in &mut column {
                    *v /= total;
                }
            } else {
                column.iter_mut().for_each(|v| *v = 0.0);
                let position = center / df;
                let first = position.floor() as usize;
                if first < bins {
                    column[first] = 1.0 - (position - first as f64);
                }
                if first + 1 < bins {
                    column[first + 1] = position - first as f64;
                }
            }
            let first = column.iter().position(|v| *v != 0.0).unwrap_or(bins);
            let last = column
                .iter()
                .rposition(|v| *v != 0.0)
                .map_or(first, |l| l + 1);
            rows.push((
                first,
                column[first..last].iter().map(|v| *v as f32).collect(),
            ));
        }
        let fft = FftPlanner::<f32>::new().plan_fft_forward(fft_size);
        Frontend {
            window,
            scratch: vec![Complex32::default(); fft.get_inplace_scratch_len()],
            fft,
            fft_size,
            buf: vec![Complex32::new(0.0, 0.0); fft_size],
            mag: vec![0.0; bins],
            rows,
        }
    }

    /// `history` ends at the newest sample.
    fn features(&mut self, history: &[f32], out: &mut [f32]) {
        let n = self.window.len();
        let missing = n.saturating_sub(history.len());
        let start = history.len().saturating_sub(n);
        for i in 0..self.fft_size {
            self.buf[i] = if i < n {
                Complex32::new(
                    if i < missing {
                        0.0
                    } else {
                        history[start + i - missing] * self.window[i]
                    },
                    0.0,
                )
            } else {
                Complex32::new(0.0, 0.0)
            };
        }
        self.fft
            .process_with_scratch(&mut self.buf, &mut self.scratch);
        for i in 0..self.mag.len() {
            self.mag[i] = self.buf[i].norm();
        }
        for (k, (first, w)) in self.rows.iter().enumerate() {
            let v: f32 = w
                .iter()
                .enumerate()
                .map(|(j, x)| x * self.mag[first + j])
                .sum();
            out[k] = (1.0e4 * v).ln_1p();
        }
    }
}

struct Conv {
    inputs: usize,
    kernel: usize,
    dilation: usize,
    w: usize,
    b: usize,
    position: usize,
    history: Vec<f32>,
}

impl Conv {
    fn slot(&self, age: usize) -> usize {
        ((self.position + 3 - age) % 3) * self.inputs * STRIDE
    }
}

struct Net {
    w: &'static [f32],
    conv: [Conv; 3],
    conv_output: Vec<f32>,
    indices: Vec<usize>,
    convolved: Vec<f32>,
    projected: Vec<f32>,
    pitch_features: Vec<f32>,
    hidden: Vec<f32>,
    logits: Vec<f32>,
}

fn sigmoid(x: f32) -> f32 {
    1.0 / (1.0 + (-x).exp())
}

impl Net {
    fn new() -> Self {
        let w = weights();
        let mk = |inputs, kernel, dilation, wo, bo| Conv {
            inputs,
            kernel,
            dilation,
            w: wo,
            b: bo,
            position: 0,
            history: vec![0.0; 3 * inputs * STRIDE],
        };
        let conv = [
            mk(2, 7, 1, K_CONV0_W, K_CONV0_B),
            mk(CH, 5, 1, K_CONV1_W, K_CONV1_B),
            mk(CH, 5, 3, K_CONV2_W, K_CONV2_B),
        ];
        let harmonics = [0.5f64, 1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0];
        let mut indices = vec![BINS; PITCHES * HARM];
        for p in 0..PITCHES {
            for (h, hv) in harmonics.iter().enumerate() {
                let idx = (p * 3 + 1) as i64 + (36.0 * hv.log2()).round() as i64;
                if idx >= 0 && (idx as usize) < BINS {
                    indices[p * HARM + h] = idx as usize;
                }
            }
        }
        Net {
            w,
            conv,
            conv_output: vec![0.0; CH * STRIDE],
            indices,
            convolved: vec![0.0; CH * STRIDE],
            projected: vec![0.0; PITCHES * HID],
            pitch_features: vec![0.0; PITCHES * HID],
            hidden: vec![0.0; PITCHES * HID],
            logits: vec![0.0; PITCHES * HEADS],
        }
    }

    fn reset(&mut self) {
        for conv in &mut self.conv {
            conv.position = 0;
            conv.history.fill(0.0);
        }
        self.conv_output.fill(0.0);
        self.convolved.fill(0.0);
        self.projected.fill(0.0);
        self.pitch_features.fill(0.0);
        self.hidden.fill(0.0);
        self.logits.fill(0.0);
    }

    fn step(&mut self, features: [&[f32]; 2]) {
        for c in &mut self.conv {
            c.position = (c.position + 1) % 3;
        }
        // input
        {
            let base = self.conv[0].slot(0);
            for (ch, channel_features) in features.iter().enumerate() {
                for (i, feature) in channel_features.iter().enumerate().take(BINS) {
                    let scale = self.w[K_INPUT_SCALE + ch * BINS + i];
                    let shift = self.w[K_INPUT_SHIFT + ch * BINS + i];
                    self.conv[0].history[base + ch * STRIDE + PAD + i] = feature * scale + shift;
                }
            }
        }
        for layer in 0..3 {
            let out = &mut self.conv_output;
            out.fill(0.0);
            {
                let c = &self.conv[layer];
                for co in 0..CH {
                    let dest = &mut out[co * STRIDE + PAD..co * STRIDE + PAD + BINS];
                    dest.iter_mut().for_each(|d| *d = self.w[c.b + co]);
                    for tap in 0..3 {
                        let src = c.slot(2 - tap);
                        for ci in 0..c.inputs {
                            let x = &c.history[src + ci * STRIDE..src + (ci + 1) * STRIDE];
                            let wo = c.w + ((co * c.inputs + ci) * 3 + tap) * c.kernel;
                            // Each row is padded for the largest dilation (3 * 2).
                            // A contiguous frequency sweep exposes independent bins to
                            // auto-vectorization while preserving upstream tap order.
                            for k in 0..c.kernel {
                                let start = (PAD as isize
                                    + c.dilation as isize * (k as isize - (c.kernel / 2) as isize))
                                    as usize;
                                let weight = self.w[wo + k];
                                for (output, value) in dest.iter_mut().zip(&x[start..start + BINS])
                                {
                                    *output += weight * value;
                                }
                            }
                        }
                    }
                    dest.iter_mut().for_each(|d| *d = d.max(0.0));
                }
            }
            if layer < 2 {
                let next = &mut self.conv[layer + 1];
                let base = next.slot(0);
                next.history[base..base + CH * STRIDE].copy_from_slice(out);
            } else {
                self.convolved.copy_from_slice(out);
            }
        }
        // gather
        for p in 0..PITCHES {
            let a = &mut self.projected[p * HID..(p + 1) * HID];
            for (j, output) in a.iter_mut().enumerate() {
                *output = self.w[K_PROJ_B + j] + self.w[K_PITCH_EMB + p * HID + j];
            }
            for c in 0..CH {
                for h in 0..HARM {
                    let idx = self.indices[p * HARM + h];
                    let value = if idx < BINS {
                        self.convolved[c * STRIDE + PAD + idx]
                    } else {
                        0.0
                    };
                    let wo = K_PROJ_W + (c * HARM + h) * HID;
                    for (j, output) in a.iter_mut().enumerate() {
                        *output += self.w[wo + j] * value;
                    }
                }
            }
            a.iter_mut().for_each(|v| *v = v.max(0.0));
        }
        // pitch convolution
        for p in 0..PITCHES {
            let mut a = [0.0f32; HID];
            a.copy_from_slice(&self.w[K_PCONV_B..K_PCONV_B + HID]);
            for k in 0..5 {
                let pp = p as i64 + k as i64 - 2;
                if pp < 0 || pp >= PITCHES as i64 {
                    continue;
                }
                let input = &self.projected[pp as usize * HID..(pp as usize + 1) * HID];
                for (i, input_value) in input.iter().enumerate() {
                    let wo = K_PCONV_W + (k * HID + i) * HID;
                    for (o, output) in a.iter_mut().enumerate() {
                        *output += self.w[wo + o] * input_value;
                    }
                }
            }
            for (j, value) in a.iter().enumerate() {
                self.pitch_features[p * HID + j] = self.projected[p * HID + j] + value.max(0.0);
            }
        }
        // GRU + heads
        for p in 0..PITCHES {
            let mut gi = [0.0f32; GATES];
            let mut gh = [0.0f32; GATES];
            gi.copy_from_slice(&self.w[K_GRU_IB..K_GRU_IB + GATES]);
            gh.copy_from_slice(&self.w[K_GRU_HB..K_GRU_HB + GATES]);
            let x = &self.pitch_features[p * HID..(p + 1) * HID];
            let hp = &mut self.hidden[p * HID..(p + 1) * HID];
            for i in 0..HID {
                for j in 0..GATES {
                    gi[j] += self.w[K_GRU_IW + i * GATES + j] * x[i];
                    gh[j] += self.w[K_GRU_HW + i * GATES + j] * hp[i];
                }
            }
            for j in 0..HID {
                let r = sigmoid(gi[j] + gh[j]);
                let z = sigmoid(gi[HID + j] + gh[HID + j]);
                let n = (gi[2 * HID + j] + r * gh[2 * HID + j]).tanh();
                hp[j] = (1.0 - z) * n + z * hp[j];
            }
            for d in 0..HEADS {
                let mut v = self.w[K_HEAD_B + d] + self.w[K_PITCH_BIAS + p * HEADS + d];
                for (j, hidden) in hp.iter().enumerate() {
                    v += self.w[K_HEAD_W + d * HID + j] * hidden;
                }
                self.logits[p * HEADS + d] = v;
            }
        }
    }

    fn probability(&self, p: usize, head: usize) -> f32 {
        sigmoid(self.logits[p * HEADS + head] + CALIBRATION[head])
    }
}
