//! Sound effects via rodio (cpal; oboe backend on Android). The same code
//! path plays on desktop, so audio behaves identically in dev and on
//! device. The chime is synthesised into a WAV in memory — no assets.

use std::io::Cursor;
use std::sync::{Mutex, OnceLock};

/// Play a short pleasant chime (C6–E6–G6 arpeggio).
pub fn ding() -> Result<(), String> {
    // Keep the output sink alive for the process lifetime so later dings
    // are instant; recreating it per sound can hit audio-focus churn.
    static OUT: OnceLock<Mutex<Option<rodio::MixerDeviceSink>>> = OnceLock::new();
    let mut slot = OUT
        .get_or_init(|| Mutex::new(None))
        .lock()
        .map_err(|e| e.to_string())?;
    if slot.is_none() {
        let mut sink = rodio::DeviceSinkBuilder::open_default_sink()
            .map_err(|e| format!("audio output: {e}"))?;
        sink.log_on_drop(false);
        *slot = Some(sink);
    }
    let sink = slot.as_ref().unwrap();

    let source =
        rodio::Decoder::new(Cursor::new(chime_wav())).map_err(|e| format!("decode: {e}"))?;
    let player = rodio::Player::connect_new(sink.mixer());
    player.append(source);
    Ok(())
}

/// Synthesise a ~0.8 s chime into a 16-bit mono WAV buffer.
fn chime_wav() -> Vec<u8> {
    const SR: u32 = 24000;
    let note_len = (SR as f32 * 0.55) as usize;
    let gap = (SR as f32 * 0.13) as usize;
    let total = note_len + 2 * gap;
    let freqs = [1046.5f32, 1318.5, 1568.0]; // C6 E6 G6
    let mut samples = vec![0f32; total];
    for (i, f) in freqs.iter().enumerate() {
        let start = i * gap;
        for (n, s) in samples[start..total].iter_mut().take(note_len).enumerate() {
            let t = n as f32 / SR as f32;
            let env = (-t * 6.0).exp();
            *s += (2.0 * std::f32::consts::PI * f * t).sin() * 0.28 * env;
        }
    }

    let data_len = (samples.len() * 2) as u32;
    let mut out = Vec::with_capacity(44 + samples.len() * 2);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data_len).to_le_bytes());
    out.extend_from_slice(b"WAVE");
    out.extend_from_slice(b"fmt ");
    out.extend_from_slice(&16u32.to_le_bytes()); // fmt chunk size
    out.extend_from_slice(&1u16.to_le_bytes()); // PCM
    out.extend_from_slice(&1u16.to_le_bytes()); // mono
    out.extend_from_slice(&SR.to_le_bytes());
    out.extend_from_slice(&(SR * 2).to_le_bytes()); // byte rate
    out.extend_from_slice(&2u16.to_le_bytes()); // block align
    out.extend_from_slice(&16u16.to_le_bytes()); // bits per sample
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_len.to_le_bytes());
    for s in &samples {
        let v = (s.clamp(-1.0, 1.0) * 32767.0) as i16;
        out.extend_from_slice(&v.to_le_bytes());
    }
    out
}
