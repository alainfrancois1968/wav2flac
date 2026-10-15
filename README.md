# wav2flac

Convert WAV to FLAC on all of your cores. The audio stays at the same sample rate, channel count, and bit depth, and the samples come back unchanged. LIST/INFO tags in the WAV are not copied.

Parallelism is a [Rayon](https://docs.rs/rayon) pool. The file is cut into blocks of 4096 samples per channel, each block is encoded on its own worker, and the blocks are written in order. The encoder library's own thread pool is turned off, so `-j` is the only source of threads. Several files share one pool instead, one file per worker.

## Build on a Mac

Install Rust from [rustup](https://rustup.rs) if you do not already have it, then:

```sh
cargo test
cargo build --release
```

The binary is `target/release/wav2flac`. Intel and Apple Silicon both work. You do not need FFmpeg or a C compiler. `cargo test` builds its own short WAVs and does not need network access after the crates are downloaded.

## Usage

```sh
# One file. -j 0, the default, uses one worker per logical CPU.
wav2flac -j 8 -o track.flac track.wav

# Many files. -j is how many files run at once.
wav2flac -j 8 -o ~/Music/flac ~/Music/wav/*.wav

# Write song.flac next to song.wav.
wav2flac song.wav
```

| Flag | Meaning |
| --- | --- |
| `-j`, `--jobs` | Threads, 1 through 256. `0` (the default) uses one per logical CPU. |
| `-o`, `--output` | A `.flac` path for one input, or a directory for several. |
| `-f`, `--force` | Replace a FLAC that is already there. |
| `-q`, `--quiet` | Only print errors. |

The default is to refuse to overwrite. A failed file does not delete the others in a batch; the command exits non-zero if any file failed.

A file shorter than one block (about a tenth of a second at 44.1 kHz) has nothing to split, so it uses a single worker. Anything longer scales with `-j`. The thread count follows the machine; the speedup is not a straight line, because the WAV read and the final stream write stay on one thread.

## Library

```rust
use wav2flac::wav_to_flac;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let wav = std::fs::read("song.wav")?;
    let flac = wav_to_flac(&wav, 0)?; // 0 = one thread per CPU
    std::fs::write("song.flac", flac)?;
    Ok(())
}
```

## Limits

- Integer PCM only: 8, 12, 16, 20, and 24-bit. 32-bit WAV is rejected.
- 1 to 8 channels.
- At least 16 samples per channel.
- A WAV larger than 4 GiB (RF64) is rejected.
- Extensible WAV is accepted only when it is integer PCM and the container width matches the bit depth.

## Layout

```text
src/main.rs     command line
src/lib.rs      public API
src/wav.rs      WAV reader
src/encode.rs   Rayon block encode and FLAC stitch
tests/          lossless round-trip tests
```

`cargo test` encodes with one thread and with several, checks the bytes match, and decodes with a separate FLAC decoder.
