//! The microphone's voice: a clean-up chain (noise suppression, automatic
//! level, compressor) for the mix track and streams, and a health check
//! that says while recording when the mic is too quiet, too loud, cutting
//! out or silent, instead of afterwards.
//!
//! The chain runs at 48 kHz on the mono voice. The mic stem is never
//! processed: it stays the untouched original, so a recording can always
//! be cleaned up again in an editor.

use std::collections::VecDeque;

use nnnoiseless::DenoiseState;

const RATE: f32 = 48000.0;

/// Samples the denoiser works on at a time: 10 ms.
const FRAME: usize = DenoiseState::FRAME_SIZE;

/// How much later the chain's output is than its input, in samples: one
/// frame of buffering plus the denoiser's own delay of one frame
/// (`chain_latency_is_two_frames` measures it). The mixer places the output
/// this much earlier, so the voice stays on the picture.
pub const LATENCY: usize = 2 * FRAME;

/// Speech level the leveller aims for (RMS of 10 ms speech frames).
const TARGET_DB: f32 = -20.0;
/// The leveller never boosts more than this (a very quiet mic would
/// otherwise bring its hiss up with it) or cuts more than `MIN_GAIN_DB`.
const MAX_GAIN_DB: f32 = 30.0;
const MIN_GAIN_DB: f32 = -6.0;

fn db_to_lin(db: f32) -> f32 {
    10f32.powf(db / 20.0)
}

fn lin_to_db(x: f32) -> f32 {
    if x <= 1e-7 { -140.0 } else { 20.0 * x.log10() }
}

/// Slow automatic level: measures the speech level (only while the
/// denoiser hears speech, so pauses and noise never pump it up) and moves
/// the gain smoothly toward the one that brings speech to `TARGET_DB`.
struct Leveller {
    speech_db: Option<f32>,
    speech_frames: u32,
    gain: f32,
    target: f32,
}

impl Leveller {
    fn new() -> Self {
        Leveller { speech_db: None, speech_frames: 0, gain: 1.0, target: 1.0 }
    }

    /// Learn from one 10 ms frame (`speech`: the denoiser's verdict).
    fn observe(&mut self, frame: &[f32], speech: bool) {
        let rms = (frame.iter().map(|s| s * s).sum::<f32>() / frame.len().max(1) as f32).sqrt();
        let db = lin_to_db(rms);
        if !speech || db < -70.0 {
            return;
        }
        // Settle within about a third of a second of the first speech,
        // then follow slowly (about a second).
        let a = if self.speech_frames < 30 { 0.25 } else { 0.02 };
        self.speech_frames += 1;
        let level = self.speech_db.map_or(db, |s| s + (db - s) * a);
        self.speech_db = Some(level);
        self.target = db_to_lin((TARGET_DB - level).clamp(MIN_GAIN_DB, MAX_GAIN_DB));
    }

    fn process(&mut self, s: f32) -> f32 {
        // About 300 ms to reach a new target: no audible pumping.
        self.gain += (self.target - self.gain) / (0.3 * RATE);
        s * self.gain
    }
}

/// Gentle feed-forward compressor: 3:1 above -18 dBFS with a 6 dB soft
/// knee, 5 ms attack and 150 ms release. It evens out loud syllables; the
/// mix bus limiter catches whatever is left near full scale.
struct Compressor {
    env: f32,
}

const COMP_THRESHOLD_DB: f32 = -18.0;
const COMP_RATIO: f32 = 3.0;
const COMP_KNEE_DB: f32 = 6.0;

impl Compressor {
    fn reduction_db(level_db: f32) -> f32 {
        let over = level_db - COMP_THRESHOLD_DB;
        let slope = 1.0 - 1.0 / COMP_RATIO;
        if over <= -COMP_KNEE_DB / 2.0 {
            0.0
        } else if over >= COMP_KNEE_DB / 2.0 {
            over * slope
        } else {
            slope * (over + COMP_KNEE_DB / 2.0).powi(2) / (2.0 * COMP_KNEE_DB)
        }
    }

    fn process(&mut self, s: f32) -> f32 {
        let attack = 1.0 - (-1.0 / (0.005 * RATE)).exp();
        let release = 1.0 - (-1.0 / (0.150 * RATE)).exp();
        let level = s.abs();
        self.env += (level - self.env) * if level > self.env { attack } else { release };
        s * db_to_lin(-Self::reduction_db(lin_to_db(self.env)))
    }
}

/// Noise suppression (RNNoise), then the leveller and the compressor, on
/// the mono voice at 48 kHz. Output has the same length as input and lags
/// it by `LATENCY` samples.
pub struct VoiceChain {
    denoise: Box<DenoiseState<'static>>,
    /// Input waiting for a whole frame.
    input: Vec<f32>,
    /// Processed samples not yet returned (starts with one frame of
    /// silence, so there is always enough to return).
    output: VecDeque<f32>,
    leveller: Leveller,
    compressor: Compressor,
}

impl VoiceChain {
    pub fn new() -> Self {
        VoiceChain {
            denoise: DenoiseState::new(),
            input: Vec::with_capacity(FRAME),
            output: std::iter::repeat_n(0.0, FRAME).collect(),
            leveller: Leveller::new(),
            compressor: Compressor { env: 0.0 },
        }
    }

    /// Start over after a gap in the input: drops the few milliseconds in
    /// flight so they are not played at the wrong time. The level learned
    /// so far is kept.
    pub fn reset(&mut self) {
        self.input.clear();
        self.output = std::iter::repeat_n(0.0, FRAME).collect();
    }

    /// Process mono samples in -1..1.
    pub fn process(&mut self, samples: &[f32]) -> Vec<f32> {
        let mut out_frame = [0f32; FRAME];
        for &s in samples {
            self.input.push(s * 32768.0);
            if self.input.len() == FRAME {
                let vad = self.denoise.process_frame(&mut out_frame, &self.input);
                self.input.clear();
                for v in &mut out_frame {
                    *v /= 32768.0;
                }
                self.leveller.observe(&out_frame, vad > 0.5);
                for &v in &out_frame {
                    let v = self.compressor.process(self.leveller.process(v));
                    self.output.push_back(v);
                }
            }
        }
        // Never runs dry: `output` holds FRAME minus the input still
        // waiting, which is at least as much as was just pushed.
        self.output.drain(..samples.len().min(self.output.len())).collect()
    }
}

impl Default for VoiceChain {
    fn default() -> Self {
        Self::new()
    }
}

/// What the mic sounds like right now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verdict {
    /// Not enough speech heard yet to judge the level.
    Listening,
    Good,
    /// Speech peaks below -24 dBFS.
    Quiet,
    /// Speech peaks below -36 dBFS: far too low to fix well afterwards.
    TooQuiet,
    /// Speech peaks above -3 dBFS: one loud word from clipping.
    Loud,
    /// Peaks at full scale: distorted.
    Clipping,
    /// No signal at all for 3 s: muted, unplugged or the wrong device.
    Silent,
    /// Speech that drops to digital silence between words: noise reduction
    /// or a gate before the recorder (on the mic or in macOS).
    CuttingOut,
    /// Background noise within 20 dB of the voice.
    Noisy,
}

impl Verdict {
    pub const ALL: [Verdict; 9] = [
        Verdict::Listening,
        Verdict::Good,
        Verdict::Quiet,
        Verdict::TooQuiet,
        Verdict::Loud,
        Verdict::Clipping,
        Verdict::Silent,
        Verdict::CuttingOut,
        Verdict::Noisy,
    ];

    fn index(self) -> usize {
        Self::ALL.iter().position(|v| *v == self).unwrap_or(0)
    }

    /// Worth stopping for: the recording would be hard or impossible to fix.
    pub fn is_bad(self) -> bool {
        matches!(self, Verdict::TooQuiet | Verdict::Clipping | Verdict::Silent | Verdict::CuttingOut)
    }

    /// Worth a look, though it may be fine (or fixed by voice clean-up).
    pub fn is_warning(self) -> bool {
        matches!(self, Verdict::Quiet | Verdict::Loud | Verdict::Noisy)
    }

    pub fn label(self) -> &'static str {
        match self {
            Verdict::Listening => "Listening",
            Verdict::Good => "Good",
            Verdict::Quiet => "Quiet",
            Verdict::TooQuiet => "Too quiet",
            Verdict::Loud => "Loud",
            Verdict::Clipping => "Clipping",
            Verdict::Silent => "No sound",
            Verdict::CuttingOut => "Cutting out",
            Verdict::Noisy => "Noisy",
        }
    }

    /// What to do about it.
    pub fn advice(self) -> &'static str {
        match self {
            Verdict::Listening => "Say a few words to check your mic level.",
            Verdict::Good => "Your voice is at a good level.",
            Verdict::Quiet => "Speech is quiet. Raise the gain on the mic, or move closer. Clean up voice levels it automatically.",
            Verdict::TooQuiet => "Speech is far too quiet to fix well later. Raise the gain on the mic itself and move closer.",
            Verdict::Loud => "Speech is close to the top. Lower the gain on the mic a little.",
            Verdict::Clipping => "Your voice is distorting. Lower the gain on the mic, or move back.",
            Verdict::Silent => "No sound from the mic. Check it is plugged in, unmuted and selected.",
            Verdict::CuttingOut => {
                "Audio drops to silence between words. Turn off noise reduction on the mic, and set Mic Mode to Standard in Control Centre."
            }
            Verdict::Noisy => "Background noise is high. Move closer to the mic or reduce the noise. Clean up voice suppresses it.",
        }
    }
}

/// The mic's state for the UI.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MicReport {
    pub verdict: Verdict,
    /// Typical speech peak (dBFS) in the last 10 s, once speech is heard.
    pub speech_peak_db: Option<f32>,
}

const BLOCK_SECS: f64 = 0.05;
/// Blocks judged: the last 10 s.
const WINDOW: usize = 200;
/// Judge every this many blocks (4 times a second): plenty for the badge,
/// and a fifth of the work of judging every block.
const JUDGE_EVERY: u32 = 5;

#[derive(Clone, Copy)]
struct Block {
    rms_db: f32,
    peak_db: f32,
}

/// Watches the raw mic and judges it every 50 ms over the last 10 s.
pub struct MicHealth {
    sum_sq: f64,
    peak: f32,
    n: usize,
    blocks: VecDeque<Block>,
    /// Blocks since the last judgement.
    unjudged: u32,
    /// Blocks spent in each verdict (`Verdict::ALL` order) since `reset_tally`.
    tally: [u64; 9],
    report: MicReport,
}

impl Default for MicHealth {
    fn default() -> Self {
        Self::new()
    }
}

impl MicHealth {
    pub fn new() -> Self {
        MicHealth {
            sum_sq: 0.0,
            peak: 0.0,
            n: 0,
            blocks: VecDeque::with_capacity(WINDOW + 1),
            unjudged: 0,
            tally: [0; 9],
            report: MicReport { verdict: Verdict::Listening, speech_peak_db: None },
        }
    }

    /// Raw mic frames at `rate`.
    pub fn push(&mut self, rate: f64, frames: &[[f32; 2]]) {
        let block_len = (rate * BLOCK_SECS).round().max(1.0) as usize;
        for f in frames {
            let mono = (f[0] + f[1]) * 0.5;
            self.sum_sq += (mono * mono) as f64;
            self.peak = self.peak.max(f[0].abs()).max(f[1].abs());
            self.n += 1;
            if self.n >= block_len {
                let rms = (self.sum_sq / self.n as f64).sqrt() as f32;
                self.blocks.push_back(Block { rms_db: lin_to_db(rms), peak_db: lin_to_db(self.peak) });
                if self.blocks.len() > WINDOW {
                    self.blocks.pop_front();
                }
                (self.sum_sq, self.peak, self.n) = (0.0, 0.0, 0);
                self.unjudged += 1;
                if self.unjudged >= JUDGE_EVERY {
                    self.unjudged = 0;
                    self.report = judge(&self.blocks);
                }
                self.tally[self.report.verdict.index()] += 1;
            }
        }
    }

    pub fn report(&self) -> MicReport {
        self.report
    }

    pub fn reset_tally(&mut self) {
        self.tally = [0; 9];
    }

    /// One line on how the mic did since `reset_tally`, e.g. "good 92%,
    /// quiet 8%"; None before anything was judged.
    pub fn summary(&self) -> Option<String> {
        let total: u64 = self.tally.iter().sum();
        if total == 0 {
            return None;
        }
        let mut parts: Vec<(u64, Verdict)> =
            Verdict::ALL.iter().map(|v| (self.tally[v.index()], *v)).filter(|(n, _)| *n * 100 >= total).collect();
        parts.sort_by(|a, b| b.0.cmp(&a.0));
        Some(
            parts
                .iter()
                .map(|(n, v)| format!("{} {}%", v.label().to_lowercase(), n * 100 / total))
                .collect::<Vec<_>>()
                .join(", "),
        )
    }
}

fn percentile(mut v: Vec<f32>, p: f32) -> f32 {
    v.sort_by(f32::total_cmp);
    v[((v.len() - 1) as f32 * p).round() as usize]
}

fn judge(blocks: &VecDeque<Block>) -> MicReport {
    let listening = MicReport { verdict: Verdict::Listening, speech_peak_db: None };
    let n = blocks.len();
    if n < 20 {
        return listening;
    }
    let recent = |k: usize| blocks.iter().skip(n.saturating_sub(k));
    if n >= 60 && recent(60).all(|b| b.rms_db < -75.0) {
        return MicReport { verdict: Verdict::Silent, speech_peak_db: None };
    }
    let floor = percentile(blocks.iter().map(|b| b.rms_db).collect(), 0.1);
    let speech_th = (floor + 12.0).max(-60.0);
    let speech: Vec<Block> = blocks.iter().copied().filter(|b| b.rms_db > speech_th).collect();
    let peak = (speech.len() >= 10).then(|| percentile(speech.iter().map(|b| b.peak_db).collect(), 0.95));
    let report = |verdict| MicReport { verdict, speech_peak_db: peak };
    if recent(40).any(|b| b.peak_db >= -0.3) {
        return report(Verdict::Clipping);
    }
    // Speech that falls to digital silence (below what any live mic's own
    // noise reaches) again and again: a gate or noise reduction upstream.
    let (mut cuts, mut after_speech) = (0, false);
    for b in blocks {
        if b.rms_db > speech_th {
            after_speech = true;
        } else if b.rms_db < -100.0 && after_speech {
            cuts += 1;
            after_speech = false;
        }
    }
    if cuts >= 3 {
        return report(Verdict::CuttingOut);
    }
    let Some(peak_db) = peak else { return listening };
    let speech_rms = percentile(speech.iter().map(|b| b.rms_db).collect(), 0.5);
    report(if peak_db < -36.0 {
        Verdict::TooQuiet
    } else if peak_db < -24.0 {
        Verdict::Quiet
    } else if peak_db > -3.0 {
        Verdict::Loud
    } else if speech_rms - floor < 20.0 && floor > -100.0 {
        Verdict::Noisy
    } else {
        Verdict::Good
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Deterministic noise in -1..1.
    fn noise(seed: &mut u32) -> f32 {
        *seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        (*seed >> 8) as f32 / (1u32 << 23) as f32 - 1.0
    }

    /// `secs` of "speech": half a second of a 220 Hz tone with harmonics
    /// peaking at `peak_db`, then half a second of the floor, repeated;
    /// the floor is noise at `floor_db` RMS (or digital silence).
    fn talk(secs: f64, peak_db: f32, floor_db: Option<f32>) -> Vec<[f32; 2]> {
        let mut seed = 7;
        let shape = |t: f32| {
            (t * 220.0 * std::f32::consts::TAU).sin()
                + 0.4 * (t * 440.0 * std::f32::consts::TAU).sin()
                + 0.2 * (t * 660.0 * std::f32::consts::TAU).sin()
        };
        let shape_peak = (0..4800).map(|i| shape(i as f32 / 48000.0).abs()).fold(0.0, f32::max);
        let a = db_to_lin(peak_db) / shape_peak;
        // Uniform noise: RMS is 1/sqrt(3) of its amplitude.
        let fl = floor_db.map_or(0.0, |d| db_to_lin(d) * 3f32.sqrt());
        (0..(secs * 48000.0) as usize)
            .map(|i| {
                let t = i as f32 / 48000.0;
                let on = (t * 2.0) as u32 % 2 == 0;
                let tone = if on { a * shape(t) } else { 0.0 };
                let s = tone + fl * noise(&mut seed);
                [s, s]
            })
            .collect()
    }

    fn verdict(frames: &[[f32; 2]]) -> MicReport {
        let mut h = MicHealth::new();
        h.push(48000.0, frames);
        h.report()
    }

    #[test]
    fn judges_the_level_of_speech() {
        assert_eq!(verdict(&talk(6.0, -10.0, Some(-70.0))).verdict, Verdict::Good);
        assert_eq!(verdict(&talk(6.0, -30.0, Some(-80.0))).verdict, Verdict::Quiet);
        assert_eq!(verdict(&talk(6.0, -45.0, Some(-85.0))).verdict, Verdict::TooQuiet);
        assert_eq!(verdict(&talk(6.0, -1.5, Some(-70.0))).verdict, Verdict::Loud);
        assert_eq!(verdict(&talk(6.0, 0.0, Some(-70.0))).verdict, Verdict::Clipping);
        let p = verdict(&talk(6.0, -10.0, Some(-70.0))).speech_peak_db.unwrap();
        assert!((-11.0..-9.0).contains(&p), "{p}");
    }

    #[test]
    fn catches_dead_mics_gates_and_noise() {
        assert_eq!(verdict(&vec![[0.0, 0.0]; 48000 * 4]).verdict, Verdict::Silent);
        // Speech with digital silence between words (the Maono/macOS noise
        // reduction fault).
        assert_eq!(verdict(&talk(6.0, -12.0, None)).verdict, Verdict::CuttingOut);
        assert_eq!(verdict(&talk(6.0, -10.0, Some(-30.0))).verdict, Verdict::Noisy);
        // Room noise and no one talking: nothing to judge yet.
        assert_eq!(verdict(&talk(0.0, -10.0, None)).verdict, Verdict::Listening);
        let room: Vec<[f32; 2]> = {
            let mut seed = 3;
            (0..48000 * 5).map(|_| [0.001 * noise(&mut seed); 2]).collect()
        };
        assert_eq!(verdict(&room).verdict, Verdict::Listening);
    }

    #[test]
    fn summarises_the_recording() {
        let mut h = MicHealth::new();
        h.push(48000.0, &talk(6.0, -10.0, Some(-70.0)));
        h.reset_tally();
        h.push(48000.0, &talk(4.0, -10.0, Some(-70.0)));
        assert_eq!(h.summary().as_deref(), Some("good 100%"));
    }

    #[test]
    fn leveller_brings_quiet_speech_up_and_leaves_pauses_alone() {
        let mut l = Leveller::new();
        let quiet: Vec<f32> = (0..480).map(|i| db_to_lin(-40.0) * 1.414 * (i as f32 * 0.05).sin()).collect();
        for _ in 0..200 {
            l.observe(&quiet, true);
        }
        let target = lin_to_db(l.target);
        assert!((19.0..21.0).contains(&target), "{target}");
        // Silence or noise between words never raises it further.
        for _ in 0..200 {
            l.observe(&vec![0.0001; 480], false);
        }
        assert!((lin_to_db(l.target) - target).abs() < 0.01);
        // Capped for a near-silent mic.
        let mut l = Leveller::new();
        let whisper: Vec<f32> = vec![db_to_lin(-65.0); 480];
        for _ in 0..200 {
            l.observe(&whisper, true);
        }
        assert!((lin_to_db(l.target) - MAX_GAIN_DB).abs() < 0.01);
    }

    #[test]
    fn compressor_curve() {
        assert_eq!(Compressor::reduction_db(-30.0), 0.0);
        // 3:1 well above the knee: 12 dB over keeps 4.
        assert!((Compressor::reduction_db(-6.0) - 8.0).abs() < 1e-4);
        // Continuous through the knee.
        let a = Compressor::reduction_db(-18.0 - 3.0);
        let b = Compressor::reduction_db(-18.0 + 3.0);
        assert!(a.abs() < 1e-4 && (b - 2.0).abs() < 1e-4, "{a} {b}");
    }

    #[test]
    fn chain_keeps_length_and_settles() {
        let mut c = VoiceChain::new();
        let speech: Vec<f32> = talk(3.0, -30.0, Some(-70.0)).iter().map(|f| f[0]).collect();
        let mut out = vec![];
        for chunk in speech.chunks(1013) {
            let o = c.process(chunk);
            assert_eq!(o.len(), chunk.len());
            out.extend(o);
        }
        assert_eq!(out.len(), speech.len());
        assert!(out.iter().all(|s| s.is_finite() && s.abs() < 1.5));
    }

    /// Speech from a 48 kHz mono 16-bit WAV named by VOICE_WAV (e.g.
    /// `say -o /tmp/v.wav --data-format=LEI16@48000 ...`), for the tests
    /// below: `VOICE_WAV=/tmp/v.wav cargo test voice -- --ignored`.
    fn voice_wav() -> Vec<f32> {
        let path = std::env::var("VOICE_WAV").expect("VOICE_WAV");
        let bytes = std::fs::read(path).unwrap();
        let data = bytes.windows(4).position(|w| w == b"data").unwrap() + 8;
        bytes[data..].chunks_exact(2).map(|b| i16::from_le_bytes([b[0], b[1]]) as f32 / 32768.0).collect()
    }

    fn rms_db(s: &[f32]) -> f32 {
        lin_to_db((s.iter().map(|x| x * x).sum::<f32>() / s.len().max(1) as f32).sqrt())
    }

    /// Speech level: the 90th percentile of 50 ms block RMS.
    fn speech_db(s: &[f32]) -> f32 {
        percentile(s.chunks(2400).map(rms_db).collect(), 0.9)
    }

    #[test]
    #[ignore]
    fn chain_suppresses_noise_and_levels_quiet_speech() {
        // Two seconds of room noise (a fan and mains hum, about 25 dB
        // under the voice), then quiet speech in the same noise.
        let mut seed = 11;
        let mut fan = 0f32;
        let speech = voice_wav();
        let lead = 2 * 48000;
        let input: Vec<f32> = (0..lead + speech.len())
            .map(|i| {
                fan = fan * 0.97 + noise(&mut seed) * 0.03;
                let hum = 0.15 * (i as f32 / 48000.0 * 50.0 * std::f32::consts::TAU).sin();
                let voice = if i < lead { 0.0 } else { speech[i - lead] * db_to_lin(-12.0) };
                voice + (fan * 8.0 + hum) * 0.012
            })
            .collect();
        let out = VoiceChain::new().process(&input);
        let out = &out[LATENCY..];
        // Noise from 0.5 s on (the denoiser learns it first), speech after.
        let (n0, n1) = (rms_db(&input[24000..lead]), rms_db(&out[24000..lead]));
        let (s0, s1) = (speech_db(&input[lead..]), speech_db(&out[lead..]));
        println!("in: speech {s0:.1} dB, noise {n0:.1} dB (SNR {:.1}); out: speech {s1:.1} dB, noise {n1:.1} dB (SNR {:.1})", s0 - n0, s1 - n1);
        assert!((s1 - n1) - (s0 - n0) >= 10.0, "noise not reduced enough");
        assert!(s1 > s0 + 4.0, "quiet speech not brought up");
        // Listen: VOICE_OUT_DIR gets before.wav and after.wav.
        if let Ok(dir) = std::env::var("VOICE_OUT_DIR") {
            let wav = |name: &str, s: &[f32]| {
                let pcm: Vec<u8> = s.iter().flat_map(|x| ((x.clamp(-1.0, 1.0) * 32767.0) as i16).to_le_bytes()).collect();
                let mut f = Vec::with_capacity(44 + pcm.len());
                f.extend(b"RIFF");
                f.extend((36 + pcm.len() as u32).to_le_bytes());
                f.extend(b"WAVEfmt ");
                f.extend(16u32.to_le_bytes());
                f.extend(1u16.to_le_bytes());
                f.extend(1u16.to_le_bytes());
                f.extend(48000u32.to_le_bytes());
                f.extend(96000u32.to_le_bytes());
                f.extend(2u16.to_le_bytes());
                f.extend(16u16.to_le_bytes());
                f.extend(b"data");
                f.extend((pcm.len() as u32).to_le_bytes());
                f.extend(pcm);
                std::fs::write(std::path::Path::new(&dir).join(name), f).unwrap();
            };
            wav("before.wav", &input);
            wav("after.wav", out);
        }
    }

    #[test]
    #[ignore]
    fn chain_latency_is_two_frames() {
        let input = voice_wav();
        let mut c = VoiceChain::new();
        // Unit gain so only the timing differs.
        c.leveller.target = 1.0;
        c.leveller.speech_frames = u32::MAX / 2;
        c.leveller.speech_db = Some(TARGET_DB);
        let out = c.process(&input);
        let corr = |lag: usize| -> f64 { input.iter().zip(&out[lag..]).map(|(a, b)| (*a as f64) * (*b as f64)).sum() };
        let best = (0..3 * FRAME).max_by(|a, b| corr(*a).total_cmp(&corr(*b))).unwrap();
        println!("best lag {best} samples");
        // The denoiser's filtering moves the correlation peak by a sample.
        assert!(best.abs_diff(LATENCY) <= 2, "{best}");
    }
}


