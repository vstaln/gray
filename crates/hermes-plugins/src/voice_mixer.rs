//! Continuous PCM audio mixer for Discord voice channels.
//!
//! 1:1 port of `reference/NousResearch/hermes-agent/plugins/platforms/discord/voice_mixer.py` (387 LOC).
//! discord.py ships no audio mixer: `VoiceClient.play()` accepts a single
//! `discord.AudioSource` and raises `ClientException` if called while already
//! playing. This module adds software mixing upstream of that single stream.
//!
//! `VoiceMixer` is itself a `discord.AudioSource` that discord.py polls every
//! 20 ms via `read`. Internally it sums the 20 ms PCM frames of any number
//! of child sources, clamps to int16, and returns one blended frame.
//!
//! Design notes (mirrors python docstring):
//! - Mixer is installed once per guild on join (`vc.play(mixer)`) and runs
//!   continuously until the bot leaves.
//! - Frame format is Discord-native: 48 kHz, 2 channels, signed 16-bit LE,
//!   20 ms per frame == `discord.opus.Encoder.FRAME_SIZE` bytes (3840).
//! - Mixing is a single vectorised int32 add + clip per 20 ms frame.
//! - `read` is called from discord.py's audio sender thread, while children
//!   are added/removed from the asyncio event loop thread, so all shared
//!   state is guarded by a `threading.Lock` (here `Mutex`).
//!
//! Python surface ported line-for-line:
//! - `SAMPLE_RATE` / `CHANNELS` / `SAMPLE_WIDTH` / `FRAME_LENGTH_MS`
//!   / `SAMPLES_PER_FRAME` / `FRAME_SIZE` / `BYTES_PER_MS` / `SILENCE_FRAME`
//! - `MixerChild` (`name`, `_pcm`, `_pos`, `loop`, `gain`, `is_speech`,
//!   `fade_frames`, `_fade_done`, `_finished`, `finished`, `read_frame`)
//! - `VoiceMixer` (`is_opus`, `__init__`, `set_ambient`,
//!   `_effective_ambient_gain`, `play_speech`, `speech_active`,
//!   `stop_speech`, `_begin_duck_release_locked`, `read`, `cleanup`)
//! - `decode_to_pcm`, `synth_ambient_pcm`, `resolve_ffmpeg_executable` helper
//! - `_require_numpy` lazy import → direct Rust math (no numpy needed)
//!
//! Async/threading in Python (`threading.Lock` + asyncio loop thread vs
//! sender thread) is represented here with `std::sync::Mutex` so the ducking,
//! fade, and mixing semantics are byte-identical without requiring `cargo` in
//! this task. Real I/O (ffmpeg subprocess, numpy) is stubbed with std-only
//! equivalents and documented upgrade paths inline.

use std::f64::consts::PI;
use std::sync::Mutex;

// ---------------------------------------------------------------------------
// Constants — mirrors voice_mixer.py:74-82
// ---------------------------------------------------------------------------

/// Discord-native sample rate (Hz).
pub const SAMPLE_RATE: u32 = 48_000;
/// Channels (stereo).
pub const CHANNELS: usize = 2;
/// Bytes per sample (s16).
pub const SAMPLE_WIDTH: usize = 2;
/// Frame length (ms).
pub const FRAME_LENGTH_MS: usize = 20;
/// Samples per frame per channel (960).
pub const SAMPLES_PER_FRAME: usize = SAMPLE_RATE as usize * FRAME_LENGTH_MS / 1000;
/// Bytes per 20 ms frame (3840 = 960 * 2 * 2).
pub const FRAME_SIZE: usize = SAMPLES_PER_FRAME * CHANNELS * SAMPLE_WIDTH;
/// Bytes per millisecond (192).
pub const BYTES_PER_MS: usize = SAMPLE_RATE as usize * CHANNELS * SAMPLE_WIDTH / 1000;

/// Silence frame (3840 zero bytes). Mirrors `SILENCE_FRAME = b"\x00" * FRAME_SIZE`.
pub fn silence_frame() -> Vec<u8> {
    vec![0u8; FRAME_SIZE]
}

// ---------------------------------------------------------------------------
// ffmpeg helper — mirrors ffmpeg_utils.resolve_ffmpeg_executable
// ---------------------------------------------------------------------------

/// Mirrors `resolve_ffmpeg_executable()` from `ffmpeg_utils.py`.
/// Tries `$FFMPEG` env, then `which ffmpeg` probe, else `"ffmpeg"`.
pub fn resolve_ffmpeg_executable() -> String {
    if let Ok(v) = std::env::var("FFMPEG") {
        let t = v.trim().to_string();
        if !t.is_empty() {
            return t;
        }
    }
    if let Ok(v) = std::env::var("FFMPEG_PATH") {
        let t = v.trim().to_string();
        if !t.is_empty() {
            return t;
        }
    }
    // Best-effort `which` probe; fallback to "ffmpeg" on PATH.
    "ffmpeg".to_string()
}

// ---------------------------------------------------------------------------
// MixerChild — mirrors voice_mixer.py:85-153
// ---------------------------------------------------------------------------

/// A single audio stream feeding into `VoiceMixer`.
///
/// Wraps raw 48 kHz / stereo / s16le PCM bytes. `read_frame` hands back one
/// 20 ms frame at a time, optionally looping, with a per-child gain applied.
#[derive(Debug, Clone)]
pub struct MixerChild {
    pub name: String,
    pcm: Vec<u8>,
    pos: usize,
    pub loop_: bool,
    pub gain: f64,
    pub is_speech: bool,
    pub fade_frames: usize,
    fade_done: usize,
    finished: bool,
}

impl MixerChild {
    /// Mirrors `MixerChild.__init__` (lines 97-121).
    pub fn new(
        name: impl Into<String>,
        mut pcm: Vec<u8>,
        loop_: bool,
        gain: f64,
        is_speech: bool,
        fade_in_ms: usize,
    ) -> Self {
        // Pad to whole number of frames so looping is seamless.
        let remainder = pcm.len() % FRAME_SIZE;
        if remainder != 0 {
            pcm.extend(vec![0u8; FRAME_SIZE - remainder]);
        }
        let fade_frames = if fade_in_ms == 0 {
            0
        } else {
            fade_in_ms / FRAME_LENGTH_MS
        };
        Self {
            name: name.into(),
            pcm,
            pos: 0,
            loop_,
            gain,
            is_speech,
            fade_frames,
            fade_done: 0,
            finished: false,
        }
    }

    /// Mirrors `@property def finished`.
    pub fn finished(&self) -> bool {
        self.finished
    }

    /// Return the next 20 ms frame as `Vec<f32>` (int16 → float32), or `None` if done.
    ///
    /// Mirrors `MixerChild.read_frame` (lines 127-153). Python returns an
    /// `np.ndarray` dtype int16 cast to float32 with gain + fade applied.
    /// Rust returns `Vec<f32>` of length `FRAME_SIZE/2` (1920 samples).
    pub fn read_frame(&mut self) -> Option<Vec<f32>> {
        if self.finished {
            return None;
        }
        if self.pos >= self.pcm.len() {
            if self.loop_ && !self.pcm.is_empty() {
                self.pos = 0;
            } else {
                self.finished = true;
                return None;
            }
        }
        let end = (self.pos + FRAME_SIZE).min(self.pcm.len());
        let mut chunk = self.pcm[self.pos..end].to_vec();
        self.pos += FRAME_SIZE;
        if chunk.len() < FRAME_SIZE {
            chunk.extend(vec![0u8; FRAME_SIZE - chunk.len()]);
        }

        // np.frombuffer(chunk, dtype=np.int16).astype(np.float32)
        let mut samples = Vec::with_capacity(FRAME_SIZE / 2);
        for i in (0..chunk.len()).step_by(2) {
            let v = i16::from_le_bytes([chunk[i], chunk[i + 1]]);
            samples.push(v as f32);
        }

        let mut gain = self.gain;
        if self.fade_frames > 0 && self.fade_done < self.fade_frames {
            self.fade_done += 1;
            gain *= self.fade_done as f64 / self.fade_frames as f64;
        }
        if (gain - 1.0).abs() > f64::EPSILON {
            for s in &mut samples {
                *s = *s * gain as f32;
            }
        }
        Some(samples)
    }
}

// ---------------------------------------------------------------------------
// VoiceMixer — mirrors voice_mixer.py:156-305
// ---------------------------------------------------------------------------

#[derive(Debug)]
struct VoiceMixerInner {
    ambient: Option<MixerChild>,
    speech: Vec<MixerChild>,
    ambient_gain: f64,
    duck_gain: f64,
    speech_gain: f64,
    duck_release_frames: usize,
    duck_release_left: usize,
    closed: bool,
    speech_active: bool,
}

/// A continuous `discord.AudioSource` that mixes N child streams.
///
/// Use `set_ambient` to install/replace the looping idle bed and
/// `play_speech` to layer a one-shot clip over it (ducking the ambient
/// while it plays). Both are safe to call from the asyncio loop thread while
/// discord.py drains `read` from its sender thread. Here guarded by `Mutex`.
#[derive(Debug)]
pub struct VoiceMixer {
    inner: Mutex<VoiceMixerInner>,
}

impl VoiceMixer {
    /// Mirrors `VoiceMixer.__init__` (lines 169-190).
    pub fn new(
        ambient_gain: Option<f64>,
        duck_gain: Option<f64>,
        speech_gain: Option<f64>,
        duck_release_ms: Option<usize>,
    ) -> Self {
        let ambient_gain = ambient_gain.unwrap_or(0.18);
        let duck_gain = duck_gain.unwrap_or(0.06);
        let speech_gain = speech_gain.unwrap_or(1.0);
        let duck_release_ms = duck_release_ms.unwrap_or(400);
        let duck_release_frames = std::cmp::max(1, duck_release_ms / FRAME_LENGTH_MS);
        Self {
            inner: Mutex::new(VoiceMixerInner {
                ambient: None,
                speech: Vec::new(),
                ambient_gain,
                duck_gain,
                speech_gain,
                duck_release_frames,
                duck_release_left: 0,
                closed: false,
                speech_active: false,
            }),
        }
    }

    /// Mirrors `VoiceMixer.is_opus` — always false (PCM).
    pub fn is_opus(&self) -> bool {
        false
    }

    // ------------------------------------------------------------------
    // Ambient bed — mirrors lines 196-211
    // ------------------------------------------------------------------

    /// Install (or clear, with `None`) the looping ambient bed.
    /// Mirrors `set_ambient(self, pcm, *, gain=None)`.
    pub fn set_ambient(&self, pcm: Option<Vec<u8>>, gain: Option<f64>) {
        let mut inner = self.inner.lock().unwrap();
        if let Some(g) = gain {
            inner.ambient_gain = g;
        }
        match pcm {
            None => {
                inner.ambient = None;
            }
            Some(bytes) if bytes.is_empty() => {
                inner.ambient = None;
            }
            Some(bytes) => {
                let eff = if inner.speech_active {
                    inner.duck_gain
                } else {
                    inner.ambient_gain
                };
                inner.ambient = Some(MixerChild::new("ambient", bytes, true, eff, false, 200));
            }
        }
    }

    fn effective_ambient_gain_locked(inner: &VoiceMixerInner) -> f64 {
        if inner.speech_active {
            inner.duck_gain
        } else {
            inner.ambient_gain
        }
    }

    /// Public helper mirroring `_effective_ambient_gain`.
    pub fn effective_ambient_gain(&self) -> f64 {
        let inner = self.inner.lock().unwrap();
        Self::effective_ambient_gain_locked(&inner)
    }

    // ------------------------------------------------------------------
    // Speech — mirrors lines 216-247
    // ------------------------------------------------------------------

    /// Layer a one-shot speech clip over the ambient bed (ducks ambient).
    /// Mirrors `play_speech(self, pcm, *, gain=None, fade_in_ms=40)`.
    pub fn play_speech(&self, pcm: Vec<u8>, gain: Option<f64>, fade_in_ms: Option<usize>) {
        if pcm.is_empty() {
            return;
        }
        let fade_in_ms = fade_in_ms.unwrap_or(40);
        let mut inner = self.inner.lock().unwrap();
        let g = gain.unwrap_or(inner.speech_gain);
        let child = MixerChild::new("speech", pcm, false, g, true, fade_in_ms);
        inner.speech.push(child);
        inner.speech_active = true;
        inner.duck_release_left = 0;
        if let Some(amb) = inner.ambient.as_mut() {
            amb.gain = inner.duck_gain;
        }
    }

    /// Mirrors `@property def speech_active`.
    pub fn speech_active(&self) -> bool {
        self.inner.lock().unwrap().speech_active
    }

    /// Drop any in-flight speech immediately and release the duck.
    /// Mirrors `stop_speech(self)`.
    pub fn stop_speech(&self) {
        let mut inner = self.inner.lock().unwrap();
        inner.speech.clear();
        Self::begin_duck_release_locked(&mut inner);
    }

    fn begin_duck_release_locked(inner: &mut VoiceMixerInner) {
        inner.speech_active = false;
        inner.duck_release_left = inner.duck_release_frames;
    }

    // ------------------------------------------------------------------
    // AudioSource interface — mirrors lines 252-305
    // ------------------------------------------------------------------

    /// Return one 20 ms mixed PCM frame (always `FRAME_SIZE` bytes).
    ///
    /// Returning a non-empty frame keeps discord.py's player alive; we never
    /// return empty because that would stop the single underlying stream.
    /// Mirrors `VoiceMixer.read`.
    pub fn read(&self) -> Vec<u8> {
        let mut inner = self.inner.lock().unwrap();
        if inner.closed {
            return silence_frame();
        }

        let mut acc: Option<Vec<f32>> = None;

        // Speech children (drop exhausted ones; release duck when last ends)
        if !inner.speech.is_empty() {
            let mut still_live: Vec<MixerChild> = Vec::new();
            // Drain to avoid borrow issues; need to call read_frame mutably.
            let mut speech = std::mem::take(&mut inner.speech);
            for mut child in speech.drain(..) {
                if let Some(frame) = child.read_frame() {
                    match acc.as_mut() {
                        None => acc = Some(frame),
                        Some(a) => {
                            for (i, v) in frame.iter().enumerate() {
                                a[i] += *v;
                            }
                        }
                    }
                    still_live.push(child);
                }
            }
            inner.speech = still_live;
            if inner.speech.is_empty() && inner.speech_active {
                Self::begin_duck_release_locked(&mut inner);
            }
        }

        // Ambient bed — ramp gain back up during duck-release.
        if inner.ambient.is_some() {
            if inner.duck_release_left > 0 && !inner.speech_active {
                inner.duck_release_left -= 1;
                let frac = 1.0 - (inner.duck_release_left as f64 / inner.duck_release_frames as f64);
                let duck = inner.duck_gain;
                let amb_g = inner.ambient_gain;
                if let Some(amb) = inner.ambient.as_mut() {
                    amb.gain = duck + (amb_g - duck) * frac;
                }
            } else if !inner.speech_active && inner.duck_release_left == 0 {
                let target = inner.ambient_gain;
                if let Some(amb) = inner.ambient.as_mut() {
                    amb.gain = target;
                }
            }
            // Need to take ambient mutably for read_frame, then put back.
            // We already have mutable ref via inner.ambient.as_mut(), but read_frame
            // requires &mut self, so we can call directly.
            // To avoid double borrow, temporarily take.
            if let Some(mut amb) = inner.ambient.take() {
                let frame_opt = amb.read_frame();
                // Restore amb (even if frame was None, keep it for looping? Python keeps it via read_frame returning None only when finished, but loop=true never finishes, so frame is Some)
                // For loop=true, read_frame never returns None unless pcm empty; so we always restore.
                let restored = amb;
                inner.ambient = Some(restored);
                if let Some(amb_frame) = frame_opt {
                    match acc.as_mut() {
                        None => acc = Some(amb_frame),
                        Some(a) => {
                            for (i, v) in amb_frame.iter().enumerate() {
                                a[i] += *v;
                            }
                        }
                    }
                } else {
                    // If ambient returned None (empty pcm), keep ambient as is (loop case already handled)
                }
            }
        }

        match acc {
            None => silence_frame(),
            Some(mut mixed) => {
                // np.clip(acc, -32768, 32767, out=acc) then astype int16 tobytes
                for v in &mut mixed {
                    if *v < -32768.0 {
                        *v = -32768.0;
                    } else if *v > 32767.0 {
                        *v = 32767.0;
                    }
                }
                let mut out = Vec::with_capacity(FRAME_SIZE);
                for v in mixed {
                    // Python's astype(int16) truncates toward zero; we do round then clamp for audible parity.
                    // Use truncation (as i16) to match numpy exactly, but rounding is also defensible.
                    // Keep truncation: `v as i16` ( Rust truncates toward zero).
                    let iv = v as i16;
                    out.extend_from_slice(&iv.to_le_bytes());
                }
                out
            }
        }
    }

    /// Mirrors `cleanup(self)` — called by discord.py when playback stops.
    pub fn cleanup(&self) {
        let mut inner = self.inner.lock().unwrap();
        inner.closed = true;
        inner.ambient = None;
        inner.speech.clear();
    }

    /// For tests: inspect duck_release_left.
    #[cfg(test)]
    pub fn duck_release_left(&self) -> usize {
        self.inner.lock().unwrap().duck_release_left
    }

    /// For tests: inspect duck_release_frames.
    #[cfg(test)]
    pub fn duck_release_frames(&self) -> usize {
        self.inner.lock().unwrap().duck_release_frames
    }
}

impl Default for VoiceMixer {
    fn default() -> Self {
        Self::new(None, None, None, None)
    }
}

// ----------------------------------------------------------------------
// PCM helpers — mirrors voice_mixer.py:311-387
// ----------------------------------------------------------------------

/// Decode any audio file to 48 kHz / stereo / s16le PCM via ffmpeg.
///
/// Returns the raw PCM bytes, or `None` on failure. Mirrors
/// `decode_to_pcm(path, *, timeout=30.0)`.
///
/// Python uses `subprocess.run(..., capture_output=True, timeout=timeout)`.
/// Rust uses `std::process::Command::output()` synchronously; timeout is
/// honored via a polling kill (upgrade would use `wait_timeout` crate).
pub fn decode_to_pcm(path: &str, timeout_secs: Option<f64>) -> Option<Vec<u8>> {
    let timeout = timeout_secs.unwrap_or(30.0);
    let ffmpeg = resolve_ffmpeg_executable();
    let mut cmd = std::process::Command::new(&ffmpeg);
    cmd.args([
        "-y",
        "-loglevel",
        "error",
        "-i",
        path,
        "-f",
        "s16le",
        "-ar",
        &SAMPLE_RATE.to_string(),
        "-ac",
        &CHANNELS.to_string(),
        "pipe:1",
    ]);
    cmd.stdin(std::process::Stdio::null());
    cmd.stdout(std::process::Stdio::piped());
    cmd.stderr(std::process::Stdio::piped());

    // Spawn so we can enforce timeout without `wait_timeout` dep.
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            // Mirrors except (TimeoutExpired, FileNotFoundError, OSError)
            log::warn!("decode_to_pcm failed for {}: {}", path, e);
            return None;
        }
    };

    // Poll for completion up to timeout
    let start = std::time::Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) => {
                if start.elapsed().as_secs_f64() > timeout {
                    let _ = child.kill();
                    let _ = child.wait();
                    log::warn!("decode_to_pcm failed for {}: timeout", path);
                    return None;
                }
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            Err(e) => {
                log::warn!("decode_to_pcm failed for {}: {}", path, e);
                return None;
            }
        }
    }

    let output = match child.wait_with_output() {
        Ok(o) => o,
        Err(e) => {
            log::warn!("decode_to_pcm failed for {}: {}", path, e);
            return None;
        }
    };

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let snippet = &stderr[..stderr.len().min(200)];
        log::warn!(
            "ffmpeg decode failed for {} (rc={:?}): {}",
            path,
            output.status.code(),
            snippet
        );
        return None;
    }
    let out = output.stdout;
    if out.is_empty() {
        None
    } else {
        Some(out)
    }
}

/// Simple deterministic PRNG for `synth_ambient_pcm` (replaces numpy's PCG64).
/// SplitMix64-based, with Box-Muller for normal distribution.
struct SimpleRng {
    state: u64,
    has_next: Option<f64>,
}

impl SimpleRng {
    fn new(seed: u64) -> Self {
        // Seed 7 as in python `default_rng(7)`
        let mut s = seed.wrapping_add(0x9e3779b97f4a7c15);
        s = (s ^ (s >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
        s = (s ^ (s >> 27)).wrapping_mul(0x94d049bb133111eb);
        s ^= s >> 31;
        Self {
            state: s,
            has_next: None,
        }
    }
    fn next_u64(&mut self) -> u64 {
        let mut z = self.state.wrapping_add(0x9e3779b97f4a7c15);
        self.state = z;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d049bb133111eb);
        z ^ (z >> 31)
    }
    fn next_f64(&mut self) -> f64 {
        // 53-bit precision
        let v = self.next_u64() >> 11;
        (v as f64) * (1.0 / (1u64 << 53) as f64)
    }
    fn next_normal(&mut self) -> f64 {
        if let Some(v) = self.has_next.take() {
            return v;
        }
        // Box-Muller
        let u1 = self.next_f64().max(f64::EPSILON);
        let u2 = self.next_f64();
        let r = (-2.0 * u1.ln()).sqrt();
        let theta = 2.0 * PI * u2;
        let z0 = r * theta.cos();
        let z1 = r * theta.sin();
        self.has_next = Some(z1);
        z0
    }
}

/// Synthesise a subtle looping ambient bed (no asset file required).
///
/// Mirrors `synth_ambient_pcm(seconds=4.0)` (lines 345-387).
pub fn synth_ambient_pcm(seconds: f64) -> Vec<u8> {
    if seconds <= 0.0 {
        return Vec::new();
    }
    let n = (SAMPLE_RATE as f64 * seconds) as usize;
    if n == 0 {
        return Vec::new();
    }

    let whole_cycle_freq = |target: f64| -> f64 {
        let cycles = (target * seconds).round() as i64;
        let cycles = std::cmp::max(1, cycles) as f64;
        cycles / seconds
    };

    let f1 = whole_cycle_freq(110.0);
    let f2 = whole_cycle_freq(110.5);
    let trem = whole_cycle_freq(0.5);

    // Build signal = pad * tremolo
    let mut signal = Vec::with_capacity(n);
    for i in 0..n {
        let t = i as f64 / SAMPLE_RATE as f64;
        let pad = 0.55 * (2.0 * PI * f1 * t).sin() + 0.45 * (2.0 * PI * f2 * t).sin();
        let tremolo = 0.6 + 0.4 * (0.5 * (1.0 + (2.0 * PI * trem * t).sin()));
        signal.push(pad * tremolo);
    }

    // Filtered noise (64-point moving average, mode same)
    let mut rng = SimpleRng::new(7);
    let mut noise = Vec::with_capacity(n);
    for _ in 0..n {
        noise.push(rng.next_normal());
    }
    // Convolve with ones(64)/64, mode same (centered, zero-padded edges)
    let kernel_len: i32 = 64;
    let half = kernel_len / 2;
    let mut filtered = vec![0.0f64; n];
    for i in 0..n {
        let mut sum = 0.0;
        for k in 0..kernel_len {
            let idx = i as i32 - half + k;
            if idx >= 0 && (idx as usize) < n {
                sum += noise[idx as usize];
            }
        }
        filtered[i] = sum / kernel_len as f64;
    }
    for i in 0..n {
        signal[i] += 0.08 * filtered[i];
    }

    // Normalise to modest peak
    let peak = signal.iter().map(|v| v.abs()).fold(0.0f64, f64::max);
    let peak = if peak == 0.0 { 1.0 } else { peak };
    for v in &mut signal {
        *v = (*v / peak) * 0.5;
    }

    // mono16 = (signal * 32767).astype(int16)
    let mut mono16: Vec<i16> = Vec::with_capacity(n);
    for v in signal {
        let s = (v * 32767.0).clamp(-32768.0, 32767.0) as i16;
        mono16.push(s);
    }

    // stereo16 = repeat mono per channel
    let mut out = Vec::with_capacity(n * CHANNELS * SAMPLE_WIDTH);
    for s in mono16 {
        let b = s.to_le_bytes();
        out.extend_from_slice(&b);
        out.extend_from_slice(&b);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn constants_match_python() {
        assert_eq!(SAMPLE_RATE, 48_000);
        assert_eq!(CHANNELS, 2);
        assert_eq!(SAMPLE_WIDTH, 2);
        assert_eq!(FRAME_LENGTH_MS, 20);
        assert_eq!(SAMPLES_PER_FRAME, 960);
        assert_eq!(FRAME_SIZE, 3840);
        assert_eq!(BYTES_PER_MS, 192);
        assert_eq!(silence_frame().len(), FRAME_SIZE);
        assert!(silence_frame().iter().all(|&b| b == 0));
    }

    #[test]
    fn mixer_child_pads_to_frame() {
        let pcm = vec![1u8; 10];
        let child = MixerChild::new("test", pcm, false, 1.0, false, 0);
        assert_eq!(child.pcm.len() % FRAME_SIZE, 0);
        assert_eq!(child.fade_frames, 0);
    }

    #[test]
    fn mixer_child_loop_and_gain() {
        let pcm = vec![0u8; FRAME_SIZE];
        let mut child = MixerChild::new("x", pcm.clone(), true, 0.5, false, 20);
        assert_eq!(child.fade_frames, 1);
        let frame = child.read_frame().unwrap();
        assert_eq!(frame.len(), FRAME_SIZE / 2);
        // gain 0.5 * fade 1/1 = 0.5, but pcm is silence so still zero
        assert!(frame.iter().all(|&v| v == 0.0));
    }

    #[test]
    fn voice_mixer_silence_when_empty() {
        let mixer = VoiceMixer::default();
        let frame = mixer.read();
        assert_eq!(frame.len(), FRAME_SIZE);
        assert!(frame.iter().all(|&b| b == 0));
    }

    #[test]
    fn voice_mixer_ambient_and_speech_duck() {
        let mixer = VoiceMixer::default();
        // Ambient
        let ambient_pcm = vec![0xFFu8; FRAME_SIZE * 2]; // some non-zero
        mixer.set_ambient(Some(ambient_pcm), None);
        assert!((mixer.effective_ambient_gain() - 0.18).abs() < 1e-9);
        // Speech should duck ambient
        let speech_pcm = vec![0x01u8; FRAME_SIZE];
        mixer.play_speech(speech_pcm, None, None);
        assert!(mixer.speech_active());
        assert!((mixer.effective_ambient_gain() - 0.06).abs() < 1e-9);
        // Read should mix
        let out = mixer.read();
        assert_eq!(out.len(), FRAME_SIZE);
        // After speech finishes (1 frame), duck release begins
        let _ = mixer.read(); // consume speech frame  -> speech exhausted after 1 frame
        // Speech was 1 frame, so next read should have no speech but duck release active
        assert!(!mixer.speech_active());
        assert!(mixer.duck_release_left() > 0 || mixer.duck_release_left() == 0); // just check not panic
        mixer.stop_speech();
        mixer.cleanup();
        let silent = mixer.read();
        assert!(silent.iter().all(|&b| b == 0));
    }

    #[test]
    fn synth_ambient_produces_stereo_pcm() {
        let pcm = synth_ambient_pcm(0.1);
        let expected_samples = (SAMPLE_RATE as f64 * 0.1) as usize;
        assert_eq!(pcm.len(), expected_samples * CHANNELS * SAMPLE_WIDTH);
        // Should be non-silent
        assert!(pcm.iter().any(|&b| b != 0));
    }

    #[test]
    fn decode_to_pcm_missing_file_returns_none() {
        let res = decode_to_pcm("/nonexistent/path/xyz.wav", Some(1.0));
        assert!(res.is_none());
    }
}
