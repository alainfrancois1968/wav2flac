//! Parallel WAV to FLAC encoding.
//!
//! A file is split into fixed-size blocks. Each block is encoded on a Rayon
//! worker, then the blocks are written in order as one FLAC stream. Sample
//! rate, channel count, and bit depth are preserved. LIST/INFO tags are not
//! copied.

#![forbid(unsafe_code)]

mod encode;
mod wav;

pub use encode::{encode_samples, worker_count, Encoded, Error, Options};
pub use wav::{read_wav, Pcm};

/// Read a WAV file and encode it.
///
/// `threads == 0` uses one worker per logical CPU, capped at 256.
pub fn wav_to_flac(bytes: &[u8], threads: usize) -> Result<Vec<u8>, Error> {
    Ok(encode_samples(
        &read_wav(bytes)?,
        &Options {
            threads,
            block_size: 0,
        },
    )?
    .bytes)
}
