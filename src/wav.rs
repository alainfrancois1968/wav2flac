//! PCM WAV reader. 8-bit samples are converted from unsigned WAV to signed PCM.
//test
use crate::encode::Error;

/// Interleaved signed samples. Every sample is an `i32`, already sign-extended.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pcm {
    pub sample_rate: u32,
    pub channels: u32,
    pub bits_per_sample: u32,
    pub interleaved: Vec<i32>,
}

/// Read a PCM WAV. Format 1 is accepted, plus extensible PCM when the
/// container width matches the valid bit depth.
pub fn read_wav(bytes: &[u8]) -> Result<Pcm, Error> {
    if bytes.len() < 12 || &bytes[..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return Err(Error::Invalid("not a WAV file".into()));
    }
    let mut offset = 12usize;
    let mut fmt: Option<&[u8]> = None;
    let mut data: Option<&[u8]> = None;
    while offset + 8 <= bytes.len() {
        let id = &bytes[offset..offset + 4];
        let size = u32::from_le_bytes(bytes[offset + 4..offset + 8].try_into().unwrap()) as usize;
        let body = offset + 8;
        let next = body
            .checked_add(size)
            .ok_or_else(|| Error::Invalid("WAV chunk size overflowed".into()))?;
        if next > bytes.len() {
            return Err(Error::Invalid(
                "WAV chunk runs past the end of the file".into(),
            ));
        }
        if id == b"fmt " {
            fmt = Some(&bytes[body..next]);
        } else if id == b"data" && data.is_none() {
            data = Some(&bytes[body..next]);
        }
        offset = next + (size & 1);
    }
    let fmt = fmt.ok_or_else(|| Error::Invalid("WAV file has no fmt chunk".into()))?;
    let data = data.ok_or_else(|| Error::Invalid("WAV file has no data chunk".into()))?;
    let layout = parse_fmt(fmt)?;
    if layout.channels == 0 || layout.sample_rate == 0 {
        return Err(Error::Invalid(
            "WAV header has no channels or sample rate".into(),
        ));
    }
    if !data.len().is_multiple_of(layout.block_align) {
        return Err(Error::Invalid(
            "WAV data length is not a whole number of samples".into(),
        ));
    }
    let mut interleaved = Vec::with_capacity(data.len() / layout.width);
    for frame in data.chunks(layout.block_align) {
        for channel in 0..layout.channels as usize {
            let start = channel * layout.width;
            interleaved.push(read_sample(
                &frame[start..start + layout.width],
                layout.bits_per_sample,
            ));
        }
    }
    Ok(Pcm {
        sample_rate: layout.sample_rate,
        channels: layout.channels,
        bits_per_sample: layout.bits_per_sample,
        interleaved,
    })
}

struct Layout {
    sample_rate: u32,
    channels: u32,
    bits_per_sample: u32,
    width: usize,
    block_align: usize,
}

fn parse_fmt(fmt: &[u8]) -> Result<Layout, Error> {
    if fmt.len() < 16 {
        return Err(Error::Invalid("WAV fmt chunk is too short".into()));
    }
    let format = u16::from_le_bytes(fmt[0..2].try_into().unwrap());
    let channels = u16::from_le_bytes(fmt[2..4].try_into().unwrap()) as u32;
    let sample_rate = u32::from_le_bytes(fmt[4..8].try_into().unwrap());
    let block_align = u16::from_le_bytes(fmt[12..14].try_into().unwrap()) as usize;
    let mut bits = u16::from_le_bytes(fmt[14..16].try_into().unwrap()) as u32;
    if format == 0xFFFE {
        if fmt.len() < 40 {
            return Err(Error::Invalid(
                "extensible WAV fmt chunk is too short".into(),
            ));
        }
        let valid = u16::from_le_bytes(fmt[18..20].try_into().unwrap()) as u32;
        if valid != 0 {
            bits = valid;
        }
        if fmt[24] != 1 || fmt[25] != 0 {
            return Err(Error::Invalid("extensible WAV is not integer PCM".into()));
        }
    } else if format != 1 {
        return Err(Error::Invalid(format!(
            "unsupported WAV format {format} (only integer PCM)"
        )));
    }
    if channels == 0 || block_align == 0 || !block_align.is_multiple_of(channels as usize) {
        return Err(Error::Invalid(
            "WAV block align does not match the channel count".into(),
        ));
    }
    let width = block_align / channels as usize;
    if width == 0 || width > 4 {
        return Err(Error::Invalid(format!(
            "{width}-byte WAV samples are not supported"
        )));
    }
    if !(8..=24).contains(&bits) || !bits.is_multiple_of(4) {
        return Err(Error::Unsupported(format!(
            "{bits}-bit WAV is not supported (use 8, 12, 16, 20, or 24-bit PCM)"
        )));
    }
    if width != bits.div_ceil(8) as usize {
        return Err(Error::Unsupported(format!(
            "{bits}-bit samples stored in {width}-byte containers are not supported"
        )));
    }
    Ok(Layout {
        sample_rate,
        channels,
        bits_per_sample: bits,
        width,
        block_align,
    })
}

fn read_sample(bytes: &[u8], bits: u32) -> i32 {
    if bits == 8 {
        return i32::from(bytes[0]) - 128;
    }
    let mut value = 0i32;
    for (index, byte) in bytes.iter().enumerate() {
        value |= i32::from(*byte) << (8 * index);
    }
    let shift = 32 - bits;
    (value << shift) >> shift
}
