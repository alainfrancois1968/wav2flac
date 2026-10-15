//! Lossless round trip: synthesized WAV -> parallel FLAC -> claxon.

use std::io::Cursor;
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

use claxon::FlacReader;
use wav2flac::{encode_samples, read_wav, wav_to_flac, Options, Pcm};

fn tone(rate: u32, channels: u32, bits: u32, samples: usize) -> Pcm {
    let mut interleaved = Vec::with_capacity(samples * channels as usize);
    for n in 0..samples {
        for channel in 0..channels {
            let amp = match bits {
                8 => 100,
                24 => 4_000_000,
                _ => 20_000,
            };
            let step = 40 + channel as i32 * 17;
            let mut sample = ((n as i32 * step) % (amp * 2)) - amp;
            if bits < 32 {
                let shift = 32 - bits;
                sample = (sample << shift) >> shift;
            }
            interleaved.push(sample);
        }
    }
    Pcm {
        sample_rate: rate,
        channels,
        bits_per_sample: bits,
        interleaved,
    }
}

fn write_wav(pcm: &Pcm) -> Vec<u8> {
    let width = pcm.bits_per_sample.div_ceil(8) as usize;
    let block_align = pcm.channels as usize * width;
    let data_len = pcm.interleaved.len() * width;
    let mut out = Vec::with_capacity(44 + data_len);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&((36 + data_len) as u32).to_le_bytes());
    out.extend_from_slice(b"WAVE");
    out.extend_from_slice(b"fmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&(pcm.channels as u16).to_le_bytes());
    out.extend_from_slice(&pcm.sample_rate.to_le_bytes());
    out.extend_from_slice(&(pcm.sample_rate * block_align as u32).to_le_bytes());
    out.extend_from_slice(&(block_align as u16).to_le_bytes());
    out.extend_from_slice(&(pcm.bits_per_sample as u16).to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&(data_len as u32).to_le_bytes());
    for &sample in &pcm.interleaved {
        match width {
            1 => out.push((sample + 128) as u8),
            2 => out.extend_from_slice(&(sample as i16).to_le_bytes()),
            3 => {
                out.push(sample as u8);
                out.push((sample >> 8) as u8);
                out.push((sample >> 16) as u8);
            }
            _ => out.extend_from_slice(&sample.to_le_bytes()),
        }
    }
    out
}

fn decoded(flac: &[u8]) -> Vec<i32> {
    let mut reader = FlacReader::new(Cursor::new(flac)).unwrap();
    reader.samples().map(|sample| sample.unwrap()).collect()
}

#[test]
fn stereo_is_lossless_and_thread_count_does_not_change_bytes() {
    let pcm = tone(44_100, 2, 16, 44_100);
    let wav = write_wav(&pcm);
    assert_eq!(read_wav(&wav).unwrap(), pcm);
    let one = wav_to_flac(&wav, 1).unwrap();
    let many = wav_to_flac(&wav, 4).unwrap();
    assert_eq!(one, many);
    assert_eq!(decoded(&many), pcm.interleaved);
    let encoded = encode_samples(
        &pcm,
        &Options {
            threads: 4,
            block_size: 0,
        },
    )
    .unwrap();
    assert!(encoded.frames > 1);
}

#[test]
fn mono_24_bit_and_8_bit_round_trip() {
    let wide = tone(48_000, 1, 24, 12_000);
    assert_eq!(
        decoded(&wav_to_flac(&write_wav(&wide), 2).unwrap()),
        wide.interleaved
    );
    let narrow = tone(22_050, 1, 8, 4_000);
    assert_eq!(
        decoded(&wav_to_flac(&write_wav(&narrow), 2).unwrap()),
        narrow.interleaved
    );
}

#[test]
fn junk_chunk_before_data_is_skipped() {
    let pcm = tone(16_000, 1, 16, 2_000);
    let mut wav = write_wav(&pcm);
    let mut with_junk = wav[..12].to_vec();
    with_junk.extend_from_slice(b"JUNK");
    with_junk.extend_from_slice(&4u32.to_le_bytes());
    with_junk.extend_from_slice(b"skip");
    with_junk.extend_from_slice(&wav[12..]);
    wav = with_junk;
    assert_eq!(decoded(&wav_to_flac(&wav, 1).unwrap()), pcm.interleaved);
}

#[test]
fn cli_writes_flac_and_refuses_to_clobber() {
    let id = NEXT.fetch_add(1, Ordering::Relaxed);
    let root = std::env::temp_dir().join(format!("wav2flac-{id}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let wav_path = root.join("tone.wav");
    let other_path = root.join("other.wav");
    let pcm = tone(44_100, 2, 16, 8_000);
    std::fs::write(&wav_path, write_wav(&pcm)).unwrap();
    std::fs::write(&other_path, write_wav(&tone(22_050, 1, 16, 4_000))).unwrap();

    let bin = env!("CARGO_BIN_EXE_wav2flac");
    let output = root.join("tone.flac");
    let wrote = Command::new(bin)
        .args(["-j", "2", "-o"])
        .arg(&output)
        .arg(&wav_path)
        .status()
        .unwrap();
    assert!(wrote.success());
    assert!(output.is_file());
    assert_eq!(decoded(&std::fs::read(&output).unwrap()), pcm.interleaved);

    let clobber = Command::new(bin)
        .arg("-o")
        .arg(&output)
        .arg(&wav_path)
        .status()
        .unwrap();
    assert!(!clobber.success());
    let forced = Command::new(bin)
        .args(["-f", "-q", "-o"])
        .arg(&output)
        .arg(&wav_path)
        .status()
        .unwrap();
    assert!(forced.success());

    let batch = root.join("batch");
    let batched = Command::new(bin)
        .args(["-j", "2", "-o"])
        .arg(&batch)
        .arg(&wav_path)
        .arg(&other_path)
        .status()
        .unwrap();
    assert!(batched.success());
    assert!(batch.join("tone.flac").is_file());
    assert!(batch.join("other.flac").is_file());
    let help = Command::new(bin).arg("--help").status().unwrap();
    assert!(help.success());
    let _ = std::fs::remove_dir_all(&root);
}

static NEXT: AtomicU64 = AtomicU64::new(1);
