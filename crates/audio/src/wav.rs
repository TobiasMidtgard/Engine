//! Minimal WAV writing and reading (16-bit PCM and 32-bit float), for recordings and tests.

use std::io::{self, Read, Write};
use std::path::Path;

/// Write interleaved `samples` as a 16-bit PCM WAV file (plays everywhere), creating parent
/// directories as needed.
pub fn write_wav(path: &Path, samples: &[f32], channels: u16, sample_rate: u32) -> io::Result<()> {
    if let Some(dir) = path.parent()
        && !dir.as_os_str().is_empty()
    {
        std::fs::create_dir_all(dir)?;
    }
    let mut f = io::BufWriter::new(std::fs::File::create(path)?);
    write_wav_to(&mut f, samples, channels, sample_rate)?;
    f.flush()
}

/// Write a 16-bit PCM WAV to any writer.
pub fn write_wav_to(
    w: &mut impl Write,
    samples: &[f32],
    channels: u16,
    sample_rate: u32,
) -> io::Result<()> {
    let data_len = (samples.len() * 2) as u32;
    let block_align = channels * 2;
    w.write_all(b"RIFF")?;
    w.write_all(&(36 + data_len).to_le_bytes())?;
    w.write_all(b"WAVEfmt ")?;
    w.write_all(&16u32.to_le_bytes())?;
    w.write_all(&1u16.to_le_bytes())?;
    w.write_all(&channels.to_le_bytes())?;
    w.write_all(&sample_rate.to_le_bytes())?;
    w.write_all(&(sample_rate * block_align as u32).to_le_bytes())?;
    w.write_all(&block_align.to_le_bytes())?;
    w.write_all(&16u16.to_le_bytes())?;
    w.write_all(b"data")?;
    w.write_all(&data_len.to_le_bytes())?;
    let mut buf = Vec::with_capacity(samples.len() * 2);
    for s in samples {
        let v = (s.clamp(-1.0, 1.0) * 32_767.0).round() as i16;
        buf.extend_from_slice(&v.to_le_bytes());
    }
    w.write_all(&buf)
}

/// A decoded WAV file: interleaved samples in −1..1.
#[derive(Debug, Clone, PartialEq)]
pub struct WavData {
    pub samples: Vec<f32>,
    pub channels: u16,
    pub sample_rate: u32,
}

/// Read a PCM (8/16/24/32-bit) or float (32-bit) WAV file.
pub fn read_wav(path: &Path) -> io::Result<WavData> {
    let mut bytes = Vec::new();
    std::fs::File::open(path)?.read_to_end(&mut bytes)?;
    parse_wav(&bytes).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

/// Parse WAV bytes.
pub fn parse_wav(bytes: &[u8]) -> Result<WavData, String> {
    if bytes.len() < 12 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return Err("not a WAV file (no RIFF/WAVE header)".into());
    }
    let u16_at = |i: usize| u16::from_le_bytes([bytes[i], bytes[i + 1]]);
    let u32_at =
        |i: usize| u32::from_le_bytes([bytes[i], bytes[i + 1], bytes[i + 2], bytes[i + 3]]);
    let mut i = 12;
    let mut fmt = None;
    while i + 8 <= bytes.len() {
        let id = &bytes[i..i + 4];
        let len = u32_at(i + 4) as usize;
        let body = i + 8;
        let end = (body + len).min(bytes.len());
        if id == b"fmt " && len >= 16 {
            fmt = Some((
                u16_at(body),
                u16_at(body + 2),
                u32_at(body + 4),
                u16_at(body + 14),
            ));
        } else if id == b"data" {
            let (format, channels, sample_rate, bits) =
                fmt.ok_or("WAV data chunk before its fmt chunk")?;
            let data = &bytes[body..end];
            let samples: Vec<f32> = match (format, bits) {
                (1, 8) => data.iter().map(|&b| (b as f32 - 128.0) / 128.0).collect(),
                (1, 16) | (0xFFFE, 16) => data
                    .as_chunks::<2>()
                    .0
                    .iter()
                    .map(|c| i16::from_le_bytes([c[0], c[1]]) as f32 / 32_768.0)
                    .collect(),
                (1, 24) | (0xFFFE, 24) => data
                    .as_chunks::<3>()
                    .0
                    .iter()
                    .map(|c| (i32::from_le_bytes([0, c[0], c[1], c[2]]) >> 8) as f32 / 8_388_608.0)
                    .collect(),
                (1, 32) | (0xFFFE, 32) if format == 1 => data
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .map(|c| i32::from_le_bytes([c[0], c[1], c[2], c[3]]) as f32 / 2_147_483_648.0)
                    .collect(),
                (3, 32) | (0xFFFE, 32) => data
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
                    .collect(),
                _ => {
                    return Err(format!(
                        "unsupported WAV format {format} with {bits} bits; use 16-bit PCM or 32-bit float"
                    ));
                }
            };
            return Ok(WavData {
                samples,
                channels: channels.max(1),
                sample_rate,
            });
        }
        i = body + len + (len & 1);
    }
    Err("WAV file has no data chunk".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_16_bit() {
        let samples: Vec<f32> = (0..1000).map(|i| ((i as f32) * 0.05).sin() * 0.8).collect();
        let mut bytes = Vec::new();
        write_wav_to(&mut bytes, &samples, 2, 48_000).unwrap();
        let back = parse_wav(&bytes).unwrap();
        assert_eq!(back.channels, 2);
        assert_eq!(back.sample_rate, 48_000);
        assert_eq!(back.samples.len(), samples.len());
        for (a, b) in samples.iter().zip(&back.samples) {
            assert!((a - b).abs() < 1e-4);
        }
    }

    #[test]
    fn rejects_garbage_with_a_clear_message() {
        assert!(parse_wav(b"hello").unwrap_err().contains("not a WAV"));
    }
}
