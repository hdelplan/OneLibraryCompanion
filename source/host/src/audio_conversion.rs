//! Streaming, in-process PCM conversion shared by every host, including iPadOS.
use rubato::{FftFixedInOut, Resampler};
use serde::Serialize;
use std::{
    fs::File,
    io::{BufWriter, Seek, SeekFrom, Write},
    path::Path,
    time::{Duration, Instant},
};
use symphonia::core::{
    codecs::audio::{AudioDecoder, AudioDecoderOptions, well_known::*},
    formats::probe::Hint,
    formats::{FormatOptions, FormatReader, TrackType},
    io::{MediaSourceStream, MediaSourceStreamOptions},
    meta::MetadataOptions,
};

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AudioInfo {
    pub format: String,
    pub sample_rate: u32,
    pub sample_depth: u32,
    pub channels: usize,
    pub frames: Option<u64>,
}
struct Input {
    reader: Box<dyn FormatReader>,
    decoder: Box<dyn AudioDecoder>,
    id: u32,
    info: AudioInfo,
}
fn open(path: &Path) -> Result<Input, String> {
    let stream = MediaSourceStream::new(
        Box::new(File::open(path).map_err(|e| e.to_string())?),
        MediaSourceStreamOptions::default(),
    );
    let mut hint = Hint::new();
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        hint.with_extension(ext);
    }
    let reader = symphonia::default::get_probe()
        .probe(
            &hint,
            stream,
            FormatOptions::default(),
            MetadataOptions::default(),
        )
        .map_err(|e| format!("Cannot inspect audio: {e}"))?;
    let track = reader
        .default_track(TrackType::Audio)
        .ok_or("No audio stream")?;
    let initial = track
        .codec_params
        .as_ref()
        .and_then(|p| p.audio())
        .ok_or("Missing audio codec")?;
    let decoder = symphonia::default::get_codecs()
        .make_audio_decoder(initial, &AudioDecoderOptions::default().gapless(true))
        .map_err(|e| format!("Decoder unavailable: {e}"))?;
    let p = decoder.codec_params();
    let format = match p.codec {
        CODEC_ID_FLAC => "flac",
        CODEC_ID_ALAC => "alac",
        CODEC_ID_MP3 => "mp3",
        CODEC_ID_AAC => "aac",
        CODEC_ID_PCM_S16LE | CODEC_ID_PCM_S24LE | CODEC_ID_PCM_S32LE | CODEC_ID_PCM_F32LE
        | CODEC_ID_PCM_F64LE | CODEC_ID_PCM_U8 => "wav",
        CODEC_ID_PCM_S16BE | CODEC_ID_PCM_S24BE | CODEC_ID_PCM_S32BE | CODEC_ID_PCM_F32BE
        | CODEC_ID_PCM_F64BE => "aiff",
        _ => "other",
    }
    .to_owned();
    let sample_rate = p.sample_rate.ok_or("Missing sample rate")?;
    let channels = p.channels.as_ref().ok_or("Missing channel layout")?.count();
    if !(8000..=384000).contains(&sample_rate) || !(1..=2).contains(&channels) {
        return Err("Only mono/stereo audio at 8–384 kHz can be converted".into());
    }
    let info = AudioInfo {
        format,
        sample_rate,
        sample_depth: p.bits_per_sample.unwrap_or(0),
        channels,
        frames: track.num_frames,
    };
    Ok(Input {
        id: track.id,
        reader,
        decoder,
        info,
    })
}
pub fn inspect(path: &Path) -> Result<AudioInfo, String> {
    Ok(open(path)?.info)
}
#[derive(Clone, Copy, Debug)]
pub struct Target {
    pub rate: u32,
    pub depth: u32,
    pub aiff: bool,
}
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResultInfo {
    pub input: AudioInfo,
    pub sample_rate: u32,
    pub sample_depth: u32,
    pub frames: u64,
    pub bytes: u64,
    pub clipped_samples: u64,
}

fn header(target: Target, frames: u64) -> Vec<u8> {
    let bytes = (frames * 2 * u64::from(target.depth / 8)) as u32;
    if !target.aiff {
        let mut h = b"RIFF".to_vec();
        h.extend((bytes + 36).to_le_bytes());
        h.extend(b"WAVEfmt ");
        h.extend(16u32.to_le_bytes());
        h.extend(1u16.to_le_bytes());
        h.extend(2u16.to_le_bytes());
        h.extend(target.rate.to_le_bytes());
        h.extend((target.rate * 2 * (target.depth / 8)).to_le_bytes());
        h.extend((2 * (target.depth / 8) as u16).to_le_bytes());
        h.extend((target.depth as u16).to_le_bytes());
        h.extend(b"data");
        h.extend(bytes.to_le_bytes());
        h
    } else {
        let mut h = b"FORM".to_vec();
        h.extend((bytes + 46).to_be_bytes());
        h.extend(b"AIFFCOMM");
        h.extend(18u32.to_be_bytes());
        h.extend(2u16.to_be_bytes());
        h.extend((frames as u32).to_be_bytes());
        h.extend((target.depth as u16).to_be_bytes());
        let exponent = 31 - target.rate.leading_zeros();
        h.extend(((16383 + exponent) as u16).to_be_bytes());
        h.extend((u64::from(target.rate) << (63 - exponent)).to_be_bytes());
        h.extend(b"SSND");
        h.extend((bytes + 8).to_be_bytes());
        h.extend([0; 8]);
        h
    }
}
struct Output {
    file: BufWriter<File>,
    target: Target,
    frames: u64,
    skip: usize,
    dither: bool,
    rng: u64,
    clipped: u64,
}
impl Output {
    fn random(&mut self) -> f64 {
        self.rng ^= self.rng << 13;
        self.rng ^= self.rng >> 7;
        self.rng ^= self.rng << 17;
        (self.rng >> 11) as f64 / ((1u64 << 53) as f64)
    }
    fn write(&mut self, channels: &[Vec<f64>], count: usize) -> Result<(), String> {
        let start = self.skip.min(count);
        self.skip -= start;
        if (self.frames + (count - start) as u64) * 2 * u64::from(self.target.depth / 8)
            > 512 * 1024 * 1024
        {
            return Err("Converted audio exceeds the 512 MiB per-track limit".into());
        }
        let mut bytes = Vec::with_capacity((count - start) * 6);
        let scale = (1u32 << (self.target.depth - 1)) as f64;
        for (&left, &right) in channels[0][start..count]
            .iter()
            .zip(&channels[channels.len() - 1][start..count])
        {
            for value in [left, right] {
                if !value.is_finite() {
                    return Err("Audio contains non-finite samples".into());
                }
                if !(-1.0..=1.0).contains(&value) {
                    self.clipped += 1;
                }
                let noise = if self.dither {
                    self.random() - self.random()
                } else {
                    0.0
                };
                let sample = (value * scale + noise).round().clamp(-scale, scale - 1.0) as i32;
                let raw = if self.target.aiff {
                    sample.to_be_bytes()
                } else {
                    sample.to_le_bytes()
                };
                let width = (self.target.depth / 8) as usize;
                if self.target.aiff {
                    bytes.extend_from_slice(&raw[4 - width..]);
                } else {
                    bytes.extend_from_slice(&raw[..width]);
                }
            }
        }
        self.file
            .write_all(&bytes)
            .map_err(|e| format!("Cannot write local conversion: {e}"))?;
        self.frames += (count - start) as u64;
        Ok(())
    }
}
/// Destination must be a new local temporary file. Caller removes it on failure.
pub fn convert(
    source: &Path,
    destination: &Path,
    target: Target,
    cancelled: &dyn Fn() -> bool,
    progress: &mut dyn FnMut(f64),
) -> Result<ResultInfo, String> {
    if ![44100, 48000].contains(&target.rate) || ![16, 24].contains(&target.depth) {
        return Err("Invalid PCM target".into());
    }
    let mut input = open(source)?;
    let started = Instant::now();
    let check = || {
        if cancelled() || started.elapsed() > Duration::from_secs(120) {
            Err("Conversion cancelled or exceeded 120 seconds; no load sent".to_owned())
        } else {
            Ok(())
        }
    };
    let channels = input.info.channels;
    let mut resampler = if input.info.sample_rate != target.rate {
        Some(
            FftFixedInOut::<f64>::new(
                input.info.sample_rate as usize,
                target.rate as usize,
                2048,
                channels,
            )
            .map_err(|e| e.to_string())?,
        )
    } else {
        None
    };
    let mut output = Output {
        file: BufWriter::new(File::create_new(destination).map_err(|e| e.to_string())?),
        target,
        frames: 0,
        skip: resampler.as_ref().map_or(0, Resampler::output_delay),
        dither: target.depth < input.info.sample_depth || resampler.is_some(),
        rng: 0x123456789abcdef,
        clipped: 0,
    };
    output
        .file
        .write_all(&header(target, 0))
        .map_err(|e| e.to_string())?;
    let mut pending = vec![Vec::new(); channels];
    let mut total = 0u64;
    loop {
        check()?;
        let packet = match input.reader.next_packet() {
            Ok(Some(p)) => p,
            Ok(None) => break,
            Err(e) => return Err(format!("Audio read failed: {e}")),
        };
        if packet.track_id != input.id {
            continue;
        }
        let audio = input
            .decoder
            .decode(&packet)
            .map_err(|e| format!("Audio decode failed: {e}"))?;
        if audio.spec().rate() != input.info.sample_rate
            || audio.spec().channels().count() != channels
        {
            return Err("Changing audio format within a file is unsupported".into());
        }
        let mut samples = vec![0.0f64; audio.samples_interleaved()];
        audio.copy_to_slice_interleaved(&mut samples);
        let count = samples.len() / channels;
        total += count as u64;
        for frame in samples.chunks_exact(channels) {
            for (ch, sample) in frame.iter().enumerate() {
                pending[ch].push(*sample);
            }
        }
        if let Some(r) = resampler.as_mut() {
            let needed = r.input_frames_next();
            while pending[0].len() >= needed {
                check()?;
                let chunk: Vec<Vec<f64>> = pending
                    .iter_mut()
                    .map(|c| c.drain(..needed).collect())
                    .collect();
                let converted = r.process(&chunk, None).map_err(|e| e.to_string())?;
                output.write(&converted, converted[0].len())?;
            }
        } else {
            output.write(&pending, count)?;
            for c in &mut pending {
                c.clear();
            }
        }
        progress(
            input
                .info
                .frames
                .map_or(0.0, |n| (total as f64 / n.max(1) as f64).min(0.99)),
        );
    }
    if total == 0 {
        return Err("Audio file is empty".into());
    }
    // Lossless files expose exact frame counts: reject silent truncation.
    if matches!(input.info.format.as_str(), "flac" | "alac" | "wav" | "aiff")
        && input.info.frames.is_some_and(|n| n != total)
    {
        return Err("Audio ended before its declared frame count".into());
    }
    let wanted = (total * u64::from(target.rate) + u64::from(input.info.sample_rate) / 2)
        / u64::from(input.info.sample_rate);
    if let Some(r) = resampler.as_mut() {
        while output.frames < wanted {
            check()?;
            for c in &mut pending {
                c.resize(r.input_frames_next(), 0.0);
            }
            let converted = r.process(&pending, None).map_err(|e| e.to_string())?;
            let remaining = (wanted - output.frames) as usize + output.skip;
            output.write(&converted, converted[0].len().min(remaining))?;
            for c in &mut pending {
                c.clear();
            }
        }
    }
    let h = header(target, wanted);
    let bytes = h.len() as u64 + wanted * 2 * u64::from(target.depth / 8);
    output.file.flush().map_err(|e| e.to_string())?;
    output
        .file
        .get_ref()
        .set_len(bytes)
        .map_err(|e| e.to_string())?;
    output
        .file
        .seek(SeekFrom::Start(0))
        .map_err(|e| e.to_string())?;
    output.file.write_all(&h).map_err(|e| e.to_string())?;
    output.file.flush().map_err(|e| e.to_string())?;
    check()?;
    progress(1.0);
    Ok(ResultInfo {
        input: input.info,
        sample_rate: target.rate,
        sample_depth: target.depth,
        frames: wanted,
        bytes,
        clipped_samples: output.clipped,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    struct Fixture(std::path::PathBuf);
    impl Fixture {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "olc-audio-test-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            std::fs::create_dir_all(&path).unwrap();
            Self(path)
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    #[test]
    fn flac_and_alac_to_classic_pcm_preserve_every_sample_and_source() {
        let f = Fixture::new();
        let reference = include_bytes!("../tests/fixtures/transcoding/reference.wav");
        for (name, bytes) in [
            (
                "input.flac",
                include_bytes!("../tests/fixtures/transcoding/reference.flac").as_slice(),
            ),
            (
                "input.m4a",
                include_bytes!("../tests/fixtures/transcoding/reference.m4a").as_slice(),
            ),
        ] {
            let input = f.0.join(name);
            std::fs::write(&input, bytes).unwrap();
            let output = f.0.join(format!("{name}.wav"));
            let result = convert(
                &input,
                &output,
                Target {
                    rate: 44100,
                    depth: 24,
                    aiff: false,
                },
                &|| false,
                &mut |_| {},
            )
            .unwrap();
            assert_eq!(result.frames, 4410);
            assert_eq!(result.clipped_samples, 0);
            assert_eq!(std::fs::read(output).unwrap(), reference);
            assert_eq!(std::fs::read(input).unwrap(), bytes);
        }
    }
    #[test]
    fn aiff_has_correct_header_and_roundtrips_without_timing_or_sample_changes() {
        let f = Fixture::new();
        let input = f.0.join("input.wav");
        std::fs::write(
            &input,
            include_bytes!("../tests/fixtures/transcoding/reference.wav"),
        )
        .unwrap();
        let aiff = f.0.join("out.aiff");
        let wave = f.0.join("roundtrip.wav");
        convert(
            &input,
            &aiff,
            Target {
                rate: 44100,
                depth: 24,
                aiff: true,
            },
            &|| false,
            &mut |_| {},
        )
        .unwrap();
        let info = inspect(&aiff).unwrap();
        assert_eq!(info.sample_rate, 44100);
        assert_eq!(info.sample_depth, 24);
        convert(
            &aiff,
            &wave,
            Target {
                rate: 44100,
                depth: 24,
                aiff: false,
            },
            &|| false,
            &mut |_| {},
        )
        .unwrap();
        assert_eq!(std::fs::read(input).unwrap(), std::fs::read(wave).unwrap());
    }
    #[test]
    fn resampling_compensates_latency_preserves_duration_and_rejects_aliases() {
        let f = Fixture::new();
        let input = f.0.join("96k.wav");
        let output = f.0.join("48k.wav");
        let mut bytes = header(
            Target {
                rate: 96000,
                depth: 24,
                aiff: false,
            },
            9600,
        );
        for i in 0..9600 {
            let value = if i == 4000 { 4000000i32 } else { 0 };
            for _ in 0..2 {
                bytes.extend(&value.to_le_bytes()[..3]);
            }
        }
        std::fs::write(&input, &bytes).unwrap();
        let info = convert(
            &input,
            &output,
            Target {
                rate: 48000,
                depth: 24,
                aiff: false,
            },
            &|| false,
            &mut |_| {},
        )
        .unwrap();
        assert_eq!(info.frames, 4800);
        assert_eq!(info.bytes, 44 + 4800 * 6);
        let data = std::fs::read(&output).unwrap();
        let samples: Vec<i32> = data[44..]
            .as_chunks::<6>()
            .0
            .iter()
            .map(|b| i32::from_le_bytes([0, b[0], b[1], b[2]]) >> 8)
            .collect();
        let peak = samples
            .iter()
            .enumerate()
            .max_by_key(|(_, v)| v.abs())
            .unwrap()
            .0;
        assert_eq!(peak, 2000, "Resampler must not shift cues");
        // A tone above the new Nyquist frequency must be filtered, not aliased.
        let mut bytes = header(
            Target {
                rate: 96000,
                depth: 24,
                aiff: false,
            },
            9600,
        );
        for i in 0..9600 {
            let value = ((2.0 * std::f64::consts::PI * 30000.0 * i as f64 / 96000.0).sin()
                * 1000000.0) as i32;
            for _ in 0..2 {
                bytes.extend(&value.to_le_bytes()[..3]);
            }
        }
        std::fs::write(&input, bytes).unwrap();
        let filtered = f.0.join("filtered.wav");
        convert(
            &input,
            &filtered,
            Target {
                rate: 48000,
                depth: 24,
                aiff: false,
            },
            &|| false,
            &mut |_| {},
        )
        .unwrap();
        let data = std::fs::read(filtered).unwrap();
        let peak = data[44 + 1000 * 6..44 + 4000 * 6]
            .as_chunks::<6>()
            .0
            .iter()
            .map(|b| (i32::from_le_bytes([0, b[0], b[1], b[2]]) >> 8).abs())
            .max()
            .unwrap();
        assert!(peak < 10000, "Aliasing rejection: {peak}");
    }
    #[test]
    fn cancellation_and_truncated_audio_fail_instead_of_publishing_success() {
        let f = Fixture::new();
        let input = f.0.join("input.flac");
        let bytes = include_bytes!("../tests/fixtures/transcoding/reference.flac");
        std::fs::write(&input, bytes).unwrap();
        assert!(
            convert(
                &input,
                &f.0.join("cancel.wav"),
                Target {
                    rate: 44100,
                    depth: 16,
                    aiff: false
                },
                &|| true,
                &mut |_| {}
            )
            .unwrap_err()
            .contains("cancelled")
        );
        std::fs::write(&input, &bytes[..bytes.len() / 2]).unwrap();
        assert!(
            convert(
                &input,
                &f.0.join("truncated.wav"),
                Target {
                    rate: 44100,
                    depth: 16,
                    aiff: false
                },
                &|| false,
                &mut |_| {}
            )
            .is_err()
        );
    }
}
