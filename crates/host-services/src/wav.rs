//! Bounded RIFF PCM/IEEE-float decoding; no device calls or game metadata.
use std::{fs, io::Read, path::Path};
const MAX_WAV_BYTES: usize = 128 * 1024 * 1024;
const MAX_DECODED_SAMPLES: usize = 64 * 1024 * 1024;
const PCM_GUID: [u8; 16] = [
    1, 0, 0, 0, 0, 0, 0x10, 0, 0x80, 0, 0, 0xaa, 0, 0x38, 0x9b, 0x71,
];
const FLOAT_GUID: [u8; 16] = [
    3, 0, 0, 0, 0, 0, 0x10, 0, 0x80, 0, 0, 0xaa, 0, 0x38, 0x9b, 0x71,
];

#[derive(Debug)]
pub(crate) struct DecodedWav {
    pub samples: Vec<f32>,
    pub sample_rate: u32,
}
struct Format {
    channels: usize,
    sample_rate: u32,
    container_bits: u16,
    valid_bits: u16,
    float: bool,
    channel_mask: u32,
}
fn invalid() -> String {
    "Invalid WAV: expected nonempty mono/stereo PCM8/16/24/32 or finite IEEE float32, 8000..=192000 Hz, with consistent RIFF/format/data sizes".into()
}
impl Format {
    fn parse(bytes: &[u8]) -> Result<Self, String> {
        if bytes.len() < 16 || bytes.len() == 17 {
            return Err(invalid());
        }
        let u16_at = |at: usize| u16::from_le_bytes(bytes[at..at + 2].try_into().unwrap());
        let u32_at = |at: usize| u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap());
        let extra = if bytes.len() >= 18 {
            u16_at(16) as usize
        } else {
            0
        };
        if bytes.len() >= 18 && extra > bytes.len() - 18 {
            return Err(invalid());
        }
        let tag = u16_at(0);
        let container_bits = u16_at(14);
        let (float, valid_bits, channel_mask) = match tag {
            1 => (false, container_bits, 0),
            3 => (true, container_bits, 0),
            0xfffe => {
                if extra < 22 || bytes.len() < 40 {
                    return Err(invalid());
                }
                let float = if bytes[24..40] == PCM_GUID {
                    false
                } else if bytes[24..40] == FLOAT_GUID {
                    true
                } else {
                    return Err("Unsupported WAV extensible subformat: only PCM and IEEE float GUIDs are supported".into());
                };
                (float, u16_at(18), u32_at(20))
            }
            _ => {
                return Err(format!(
                    "Unsupported WAV encoding tag {tag}: compressed codecs are not implemented; use PCM8/16/24/32 or IEEE float32"
                ));
            }
        };
        let channels = u16_at(2) as usize;
        let sample_rate = u32_at(4);
        if !(1..=2).contains(&channels)
            || !(8_000..=192_000).contains(&sample_rate)
            || !matches!(container_bits, 8 | 16 | 24 | 32)
            || valid_bits == 0
            || valid_bits > container_bits
            || (float && (container_bits != 32 || valid_bits != 32))
        {
            return Err(invalid());
        }
        // Preserve standard mono/stereo placement. Other layouts need an
        // explicit downmix contract instead of silently reinterpreting channels.
        if !matches!((channels, channel_mask), (1, 0 | 1 | 2 | 4) | (2, 0 | 3)) {
            return Err("Unsupported WAV channel mask: use mono unspecified/front-left/front-right/front-center, or stereo unspecified/front-left+front-right".into());
        }
        let block = channels * (container_bits as usize / 8);
        if u16_at(12) as usize != block || u32_at(8) != sample_rate * block as u32 {
            return Err(invalid());
        }
        Ok(Self {
            channels,
            sample_rate,
            container_bits,
            valid_bits,
            float,
            channel_mask,
        })
    }
    fn sample(&self, bytes: &[u8]) -> Result<f32, String> {
        if self.float {
            let value = f32::from_le_bytes(bytes.try_into().unwrap());
            return if value.is_finite() {
                Ok(value)
            } else {
                Err("WAV IEEE float samples must be finite".into())
            };
        }
        let raw = bytes.iter().enumerate().fold(0_u32, |bits, (index, byte)| {
            bits | (*byte as u32) << (index * 8)
        });
        let padding = self.container_bits - self.valid_bits;
        if raw & ((1_u32 << padding) - 1) != 0 {
            return Err(
                "WAV extensible PCM unused low bits must be zero; valid bits are left-aligned"
                    .into(),
            );
        }
        let scale = (1_u64 << (self.valid_bits - 1)) as f64;
        // PCM8 is offset binary; wider PCM is signed two's complement.
        let value = if self.container_bits == 8 {
            (raw >> padding) as i64 - (1_i64 << (self.valid_bits - 1))
        } else {
            let signed = ((raw as i32) << (32 - self.container_bits)) >> (32 - self.container_bits);
            (signed >> padding) as i64
        };
        Ok((value as f64 / scale) as f32)
    }
}
impl DecodedWav {
    pub(crate) fn read(path: &Path) -> Result<Self, String> {
        let file = fs::File::open(path).map_err(|error| error.to_string())?;
        let mut bytes = Vec::new();
        file.take(MAX_WAV_BYTES as u64 + 1)
            .read_to_end(&mut bytes)
            .map_err(|error| error.to_string())?;
        Self::decode(&bytes)
    }
    fn decode(bytes: &[u8]) -> Result<Self, String> {
        if bytes.len() > MAX_WAV_BYTES {
            return Err("WAV exceeds the 128 MiB encoded budget".into());
        }
        if bytes.len() < 12 || &bytes[..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
            return Err(invalid());
        }
        let end = (u32::from_le_bytes(bytes[4..8].try_into().unwrap()) as usize)
            .checked_add(8)
            .ok_or_else(invalid)?;
        if end > bytes.len() || end < 12 {
            return Err(invalid());
        }
        let mut format = None;
        let mut data = None;
        let mut position = 12_usize;
        while position < end {
            if end - position < 8 {
                return Err(invalid());
            }
            let size =
                u32::from_le_bytes(bytes[position + 4..position + 8].try_into().unwrap()) as usize;
            let begin = position + 8;
            let next = begin.checked_add(size).ok_or_else(invalid)?;
            if next > end {
                return Err(invalid());
            }
            match &bytes[position..position + 4] {
                b"fmt " if format.is_none() => format = Some(&bytes[begin..next]),
                b"data" if data.is_none() => data = Some(&bytes[begin..next]),
                b"fmt " | b"data" => return Err("WAV has duplicate format/data chunks".into()),
                _ => {}
            }
            position = next.checked_add(size & 1).ok_or_else(invalid)?;
            if position > end {
                return Err(invalid());
            }
        }
        let format = Format::parse(format.ok_or_else(invalid)?)?;
        let data = data.ok_or_else(invalid)?;
        let sample_bytes = format.container_bits as usize / 8;
        let block = sample_bytes * format.channels;
        if data.is_empty() || !data.len().is_multiple_of(block) {
            return Err(invalid());
        }
        let decoded_samples = (data.len() / block).checked_mul(2).ok_or_else(invalid)?;
        if decoded_samples > MAX_DECODED_SAMPLES {
            return Err("WAV exceeds the decoded sample budget".into());
        }
        let mut samples = Vec::with_capacity(decoded_samples);
        for frame in data.chunks_exact(block) {
            let first = format.sample(&frame[..sample_bytes])?;
            let (left, right) = if format.channels == 2 {
                (first, format.sample(&frame[sample_bytes..])?)
            } else {
                match format.channel_mask {
                    1 => (first, 0.),
                    2 => (0., first),
                    _ => (first, first),
                }
            };
            samples.extend_from_slice(&[left, right]);
        }
        Ok(Self {
            samples,
            sample_rate: format.sample_rate,
        })
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    fn format(tag: u16, bits: u16, channels: u16, rate: u32) -> Vec<u8> {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&tag.to_le_bytes());
        bytes.extend_from_slice(&channels.to_le_bytes());
        bytes.extend_from_slice(&rate.to_le_bytes());
        bytes.extend_from_slice(&(rate * channels as u32 * (bits as u32 / 8)).to_le_bytes());
        bytes.extend_from_slice(&(channels * (bits / 8)).to_le_bytes());
        bytes.extend_from_slice(&bits.to_le_bytes());
        bytes
    }
    fn extensible(bits: u16, valid: u16, channels: u16, mask: u32, guid: &[u8; 16]) -> Vec<u8> {
        let mut bytes = format(0xfffe, bits, channels, 48_000);
        bytes.extend_from_slice(&22_u16.to_le_bytes());
        bytes.extend_from_slice(&valid.to_le_bytes());
        bytes.extend_from_slice(&mask.to_le_bytes());
        bytes.extend_from_slice(guid);
        bytes
    }
    fn wave(format: &[u8], data: &[u8]) -> Vec<u8> {
        let mut bytes = b"RIFF\0\0\0\0WAVE".to_vec();
        for (id, content) in [(b"fmt ", format), (b"data", data)] {
            bytes.extend_from_slice(id);
            bytes.extend_from_slice(&(content.len() as u32).to_le_bytes());
            bytes.extend_from_slice(content);
            if content.len() % 2 != 0 {
                bytes.push(0);
            }
        }
        let len = bytes.len() as u32 - 8;
        bytes[4..8].copy_from_slice(&len.to_le_bytes());
        bytes
    }
    pub(crate) fn pcm16(samples: &[i16], channels: u16, rate: u32) -> Vec<u8> {
        let data = samples
            .iter()
            .flat_map(|value| value.to_le_bytes())
            .collect::<Vec<_>>();
        wave(&format(1, 16, channels, rate), &data)
    }
    pub(crate) fn pcm8(samples: &[u8], channels: u16, rate: u32) -> Vec<u8> {
        wave(&format(1, 8, channels, rate), samples)
    }
    fn assert_mono(bytes: &[u8], expected: &[f32]) {
        let decoded = DecodedWav::decode(bytes).unwrap();
        let stereo = expected
            .iter()
            .flat_map(|value| [*value, *value])
            .collect::<Vec<_>>();
        assert_eq!(decoded.samples, stereo);
    }
    #[test]
    fn legacy_pcm8_and_common_integer_widths_decode_golden_signed_and_unsigned_samples() {
        assert_mono(
            &pcm8(&[0, 64, 128, 192, 255], 1, 8_000),
            &[-1., -0.5, 0., 0.5, 127. / 128.],
        );
        assert_mono(
            &pcm16(&[i16::MIN, -16384, 0, 16384, i16::MAX], 1, 24_000),
            &[-1., -0.5, 0., 0.5, 32767. / 32768.],
        );
        let pcm24 = [
            0, 0, 0x80, 0, 0, 0xc0, 0, 0, 0, 0, 0, 0x40, 0xff, 0xff, 0x7f,
        ];
        assert_mono(
            &wave(&format(1, 24, 1, 44_100), &pcm24),
            &[-1., -0.5, 0., 0.5, 8388607. / 8388608.],
        );
        let pcm32 = [i32::MIN, -1073741824, 0, 1073741824, i32::MAX]
            .into_iter()
            .flat_map(i32::to_le_bytes)
            .collect::<Vec<_>>();
        assert_mono(
            &wave(&format(1, 32, 1, 192_000), &pcm32),
            &[-1., -0.5, 0., 0.5, 1.],
        );
        let stereo =
            DecodedWav::decode(&wave(&format(1, 8, 2, 48_000), &[0, 255, 128, 64])).unwrap();
        assert_eq!(stereo.samples, [-1., 127. / 128., 0., -0.5]);
    }
    #[test]
    fn finite_float32_samples_are_preserved_including_headroom_and_nonfinite_is_rejected() {
        let expected = [-f32::MAX, -1.5_f32, -0.5, 0., 0.5, 1.5, f32::MAX];
        let bytes = expected
            .into_iter()
            .flat_map(f32::to_le_bytes)
            .collect::<Vec<_>>();
        assert_mono(&wave(&format(3, 32, 1, 48_000), &bytes), &expected);
        assert_mono(
            &wave(&extensible(32, 32, 1, 4, &FLOAT_GUID), &bytes),
            &expected,
        );
        for value in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            assert!(
                DecodedWav::decode(&wave(&format(3, 32, 1, 48_000), &value.to_le_bytes()))
                    .unwrap_err()
                    .contains("must be finite")
            );
        }
    }
    #[test]
    fn extensible_valid_bits_are_left_aligned_and_standard_channel_placement_is_preserved() {
        assert_mono(
            &wave(
                &extensible(24, 20, 1, 4, &PCM_GUID),
                &[0, 0, 0x80, 0, 0, 0x40, 0xf0, 0xff, 0x7f],
            ),
            &[-1., 0.5, 524287. / 524288.],
        );
        assert_mono(
            &wave(
                &extensible(32, 24, 1, 0, &PCM_GUID),
                &[0, 0, 0, 0x80, 0, 0, 0, 0x40, 0, 0xff, 0xff, 0x7f],
            ),
            &[-1., 0.5, 8388607. / 8388608.],
        );
        assert_mono(
            &wave(&extensible(8, 4, 1, 0, &PCM_GUID), &[0, 128, 240]),
            &[-1., 0., 7. / 8.],
        );
        let left = DecodedWav::decode(&wave(
            &extensible(16, 16, 1, 1, &PCM_GUID),
            &16384_i16.to_le_bytes(),
        ))
        .unwrap();
        let right = DecodedWav::decode(&wave(
            &extensible(16, 16, 1, 2, &PCM_GUID),
            &16384_i16.to_le_bytes(),
        ))
        .unwrap();
        assert_eq!(left.samples, [0.5, 0.]);
        assert_eq!(right.samples, [0., 0.5]);
        let stereo = DecodedWav::decode(&wave(
            &extensible(16, 16, 2, 3, &PCM_GUID),
            &[0, 0x40, 0, 0xc0],
        ))
        .unwrap();
        assert_eq!(stereo.samples, [0.5, -0.5]);
    }
    #[test]
    fn malformed_format_rate_alignment_extension_guid_valid_bits_and_padding_are_rejected() {
        let good = format(1, 16, 1, 48_000);
        for (at, bytes) in [
            (0, 2_u16.to_le_bytes().to_vec()),
            (2, 0_u16.to_le_bytes().to_vec()),
            (2, 3_u16.to_le_bytes().to_vec()),
            (4, 7999_u32.to_le_bytes().to_vec()),
            (4, 192001_u32.to_le_bytes().to_vec()),
            (8, 1_u32.to_le_bytes().to_vec()),
            (12, 3_u16.to_le_bytes().to_vec()),
            (14, 12_u16.to_le_bytes().to_vec()),
        ] {
            let mut invalid = good.clone();
            invalid[at..at + bytes.len()].copy_from_slice(&bytes);
            assert!(
                DecodedWav::decode(&wave(&invalid, &[0, 0])).is_err(),
                "accepted malformed format at {at}"
            );
        }
        for length in [0, 15, 17] {
            assert!(DecodedWav::decode(&wave(&vec![0; length], &[0, 0])).is_err());
        }
        for (bits, valid, mask) in [(24, 0, 4), (24, 25, 4), (24, 20, 8), (32, 32, 5)] {
            assert!(
                DecodedWav::decode(&wave(
                    &extensible(bits, valid, 1, mask, &PCM_GUID),
                    &[0, 0, 0, 0, 0, 0]
                ))
                .is_err()
            );
        }
        let ext = extensible(24, 20, 1, 4, &PCM_GUID);
        assert!(
            DecodedWav::decode(&wave(&ext, &[1, 0, 0x40]))
                .unwrap_err()
                .contains("unused low bits")
        );
        for (at, bytes) in [
            (16, 21_u16.to_le_bytes().to_vec()),
            (16, 23_u16.to_le_bytes().to_vec()),
            (24, 2_u32.to_le_bytes().to_vec()),
            (39, vec![0]),
        ] {
            let mut invalid = ext.clone();
            invalid[at..at + bytes.len()].copy_from_slice(&bytes);
            assert!(
                DecodedWav::decode(&wave(&invalid, &[0, 0, 0])).is_err(),
                "accepted malformed extensible field at {at}"
            );
        }
        assert!(DecodedWav::decode(&wave(&ext[..39], &[0, 0, 0])).is_err());
        assert!(
            DecodedWav::decode(&wave(&extensible(32, 24, 1, 4, &FLOAT_GUID), &[0, 0, 0, 0]))
                .is_err()
        );
        assert!(DecodedWav::decode(&wave(&format(3, 16, 1, 48_000), &[0, 0])).is_err());
    }
    #[test]
    fn pcm_decode_preserves_channels_and_rejects_truncation_and_empty_or_partial_frames() {
        let bytes = pcm16(&[i16::MIN, 0, i16::MAX], 1, 24_000);
        let decoded = DecodedWav::decode(&bytes).unwrap();
        assert_eq!(decoded.sample_rate, 24_000);
        assert_eq!(&decoded.samples[..4], &[-1., -1., 0., 0.]);
        for end in 0..bytes.len() {
            assert!(
                DecodedWav::decode(&bytes[..end]).is_err(),
                "accepted truncation at {end}"
            );
        }
        assert!(DecodedWav::decode(&pcm16(&[], 1, 48_000)).is_err());
        assert!(DecodedWav::decode(&pcm16(&[1], 2, 48_000)).is_err());
    }
    #[test]
    fn padded_unknown_chunks_are_accepted_but_overflow_and_duplicate_data_are_rejected() {
        let bytes = pcm16(&[12], 1, 48_000);
        let mut padded = bytes[..12].to_vec();
        padded.extend_from_slice(b"JUNK");
        padded.extend_from_slice(&1_u32.to_le_bytes());
        padded.extend_from_slice(&[9, 0]);
        padded.extend_from_slice(&bytes[12..]);
        let length = padded.len() as u32 - 8;
        padded[4..8].copy_from_slice(&length.to_le_bytes());
        assert!(DecodedWav::decode(&padded).is_ok());
        padded[16..20].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(DecodedWav::decode(&padded).is_err());
        let mut duplicate = bytes.clone();
        duplicate.extend_from_slice(&bytes[36..]);
        let length = duplicate.len() as u32 - 8;
        duplicate[4..8].copy_from_slice(&length.to_le_bytes());
        assert!(DecodedWav::decode(&duplicate).is_err());
    }
}
