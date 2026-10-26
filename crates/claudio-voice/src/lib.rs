//! Voix de Claudio, entièrement locale : reconnaissance (whisper.cpp), mot
//! d'éveil et synthèse (voix Windows ou Piper).

pub mod components;
pub mod text;
pub mod tts;
pub mod whisper;

pub use components::{Component, Kind};
pub use whisper::Whisper;

use anyhow::{Result, bail};

/// Durée en secondes d'un WAV PCM 16 bits, après vérification de son format.
pub fn wav_duration(wav: &[u8]) -> Result<f32> {
    if wav.len() < 44 || &wav[0..4] != b"RIFF" || &wav[8..12] != b"WAVE" {
        bail!("enregistrement audio invalide (WAV attendu)");
    }
    let u16_at = |i: usize| u16::from_le_bytes([wav[i], wav[i + 1]]);
    let u32_at = |i: usize| u32::from_le_bytes([wav[i], wav[i + 1], wav[i + 2], wav[i + 3]]);
    let (channels, rate, bits) = (u16_at(22), u32_at(24), u16_at(34));
    if u16_at(20) != 1 || bits != 16 || channels == 0 || rate == 0 {
        bail!("enregistrement audio : PCM 16 bits attendu");
    }
    let bytes_per_sec = rate as f32 * channels as f32 * 2.0;
    Ok((wav.len() - 44) as f32 / bytes_per_sec)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wav(seconds: f32) -> Vec<u8> {
        let samples = (16_000.0 * seconds) as u32;
        let mut v = Vec::new();
        v.extend(b"RIFF");
        v.extend((36 + samples * 2).to_le_bytes());
        v.extend(b"WAVEfmt ");
        v.extend(16u32.to_le_bytes());
        v.extend(1u16.to_le_bytes());
        v.extend(1u16.to_le_bytes());
        v.extend(16_000u32.to_le_bytes());
        v.extend(32_000u32.to_le_bytes());
        v.extend(2u16.to_le_bytes());
        v.extend(16u16.to_le_bytes());
        v.extend(b"data");
        v.extend((samples * 2).to_le_bytes());
        v.resize(v.len() + samples as usize * 2, 0);
        v
    }

    #[test]
    fn validates_wav() {
        assert!((wav_duration(&wav(1.5)).unwrap() - 1.5).abs() < 0.01);
        assert!(wav_duration(b"pas un wav").is_err());
    }
}
