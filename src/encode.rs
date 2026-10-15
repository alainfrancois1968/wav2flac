//! Block-parallel FLAC encode. The encoder crate's own thread pool stays off;
//! the only pool is the Rayon one sized by [`worker_count`].

use flacenc::bitsink::ByteSink;
use flacenc::component::{BitRepr, Stream, StreamInfo};
use flacenc::config::Encoder;
use flacenc::encode_fixed_size_frame;
use flacenc::error::Verify;
use flacenc::source::{Context, Fill, FrameBuf};
use rayon::prelude::*;

use crate::wav::Pcm;

const DEFAULT_BLOCK: usize = 4096;
const MIN_BLOCK: usize = 16;
const MIN_ALLOC: usize = 32;
const MAX_BLOCK: usize = 32_767;

/// How many workers to use. `0` means one per logical CPU. The result is `1..=256`.
pub fn worker_count(threads: usize) -> usize {
    let count = if threads == 0 {
        std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(1)
    } else {
        threads
    };
    count.clamp(1, 256)
}

#[derive(Debug)]
pub enum Error {
    Invalid(String),
    Unsupported(String),
    Encode(String),
    Io(std::io::Error),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Invalid(message) | Error::Unsupported(message) | Error::Encode(message) => {
                write!(f, "{message}")
            }
            Error::Io(err) => write!(f, "{err}"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Error::Io(err) => Some(err),
            _ => None,
        }
    }
}

impl From<std::io::Error> for Error {
    fn from(err: std::io::Error) -> Self {
        Error::Io(err)
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Options {
    /// `0` picks one worker per logical CPU. Otherwise clamped to `1..=256`.
    pub threads: usize,
    /// Samples per channel in one FLAC block. `0` uses 4096.
    pub block_size: usize,
}

/// Encoded stream plus the counts the command line prints.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Encoded {
    pub bytes: Vec<u8>,
    pub frames: usize,
}

/// Encode interleaved PCM. Blocks run on a Rayon pool and are stitched in order.
pub fn encode_samples(pcm: &Pcm, options: &Options) -> Result<Encoded, Error> {
    let channels = pcm.channels as usize;
    if channels == 0 || channels > 8 {
        return Err(Error::Unsupported(format!(
            "{} channels are not supported (FLAC allows 1 to 8)",
            pcm.channels
        )));
    }
    if !pcm.interleaved.len().is_multiple_of(channels) {
        return Err(Error::Invalid(
            "sample count is not divisible by the channel count".into(),
        ));
    }
    let per_channel = pcm.interleaved.len() / channels;
    if per_channel < MIN_BLOCK {
        return Err(Error::Invalid(format!(
            "audio is only {per_channel} samples per channel; FLAC needs at least {MIN_BLOCK}"
        )));
    }
    let block = resolve_block(options.block_size)?;
    let ranges = block_ranges(per_channel, block);
    let threads = worker_count(options.threads);

    let mut encoder = Encoder::default();
    encoder.multithread = false;
    encoder.block_size = block;
    let config = encoder
        .into_verified()
        .map_err(|(_, err)| Error::Encode(err.to_string()))?;
    let info = StreamInfo::new(
        pcm.sample_rate as usize,
        channels,
        pcm.bits_per_sample as usize,
    )
    .map_err(|err| Error::Encode(err.to_string()))?;

    let frames = if ranges.len() == 1 || threads == 1 {
        ranges
            .iter()
            .enumerate()
            .map(|(index, &(start, end))| encode_block(pcm, start, end, index, &config, &info))
            .collect::<Result<Vec<_>, _>>()?
    } else {
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(threads.min(ranges.len()))
            .build()
            .map_err(|err| Error::Encode(format!("could not start worker threads: {err}")))?;
        pool.install(|| {
            ranges
                .par_iter()
                .enumerate()
                .map(|(index, &(start, end))| encode_block(pcm, start, end, index, &config, &info))
                .collect::<Result<Vec<_>, _>>()
        })?
    };

    let mut stream = Stream::new(
        pcm.sample_rate as usize,
        channels,
        pcm.bits_per_sample as usize,
    )
    .map_err(|err| Error::Encode(err.to_string()))?;
    let frame_count = frames.len();
    for frame in frames {
        stream.add_frame(frame);
    }
    let mut ctx = Context::new(pcm.bits_per_sample as usize, channels);
    ctx.fill_interleaved(&pcm.interleaved)
        .map_err(|err| Error::Encode(err.to_string()))?;
    let digest = ctx.md5_digest();
    stream.stream_info_mut().set_md5_digest(&digest);

    let mut sink = ByteSink::new();
    stream
        .write(&mut sink)
        .map_err(|err| Error::Encode(err.to_string()))?;
    Ok(Encoded {
        bytes: sink.as_slice().to_vec(),
        frames: frame_count,
    })
}

fn resolve_block(requested: usize) -> Result<usize, Error> {
    let block = if requested == 0 {
        DEFAULT_BLOCK
    } else {
        requested
    };
    if !(MIN_ALLOC..=MAX_BLOCK).contains(&block) {
        return Err(Error::Invalid(format!(
            "block size {block} is outside {MIN_ALLOC}..={MAX_BLOCK}"
        )));
    }
    Ok(block)
}

fn block_ranges(per_channel: usize, block: usize) -> Vec<(usize, usize)> {
    let mut ranges = Vec::new();
    let mut start = 0usize;
    while start < per_channel {
        let mut end = (start + block).min(per_channel);
        let tail = per_channel - end;
        if tail > 0 && tail < MIN_BLOCK {
            end = per_channel;
        }
        ranges.push((start, end));
        start = end;
    }
    ranges
}

fn encode_block(
    pcm: &Pcm,
    start: usize,
    end: usize,
    frame_number: usize,
    config: &flacenc::error::Verified<Encoder>,
    info: &StreamInfo,
) -> Result<flacenc::component::Frame, Error> {
    let channels = pcm.channels as usize;
    let slice = &pcm.interleaved[start * channels..end * channels];
    let per = end - start;
    let alloc = per.max(MIN_ALLOC);
    let mut buffer =
        FrameBuf::with_size(channels, alloc).map_err(|err| Error::Encode(err.to_string()))?;
    buffer
        .fill_interleaved(slice)
        .map_err(|err| Error::Encode(err.to_string()))?;
    encode_fixed_size_frame(config, &buffer, frame_number, info)
        .map_err(|err| Error::Encode(err.to_string()))
}
