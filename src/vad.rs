//! Voice activity detection: finds speech with earshot and cuts the audio into segments for transcription.

use anyhow::{Context, Result};
use std::path::Path;
use std::process::Command;
use std::time::Duration;

/// Limit for cutting one segment with ffmpeg; real cuts take a fraction of this.
const CUT_TIMEOUT: Duration = Duration::from_secs(120);

/// Stream 16 kHz mono s16le PCM in 256-sample frames; pad only the final frame.
pub(crate) fn earshot_vad(
    wav: &Path,
    asr: &crate::settings::Asr,
    check_control: &dyn Fn() -> Result<()>,
) -> Result<Vec<Seg>> {
    let mut detector = earshot::Detector::default();
    let mut speech: Vec<(u64, u64)> = Vec::new();
    let mut detect = |frame: &[i16], start: u64, end: u64| {
        if detector.predict_i16(frame) >= asr.vad_threshold {
            if let Some(last) = speech.last_mut()
                && (start == last.1 || start - last.1 < (asr.min_silence as f64 * 16_000.0) as u64)
            {
                last.1 = end;
            } else {
                speech.push((start, end));
            }
        }
    };
    let mut frame = [0i16; 256];
    let mut filled = 0;
    let mut offset = 0u64;
    let samples = read_pcm_samples(wav, check_control, |sample| {
        frame[filled] = sample;
        filled += 1;
        if filled == frame.len() {
            if offset.is_multiple_of(256 * 32) {
                check_control()?;
            }
            detect(&frame, offset, offset + 256);
            offset += 256;
            filled = 0;
        }
        Ok(())
    })?;
    if filled > 0 {
        frame[filled..].fill(0);
        detect(&frame, offset, samples);
    }
    let speech = speech
        .into_iter()
        .map(|(start, end)| (start as f64 / 16_000.0, end as f64 / 16_000.0))
        .collect();
    normalize_segments(
        speech,
        asr.max_speech as f64,
        wav,
        samples as f64 / 16_000.0,
        check_control,
    )
}

/// A segment sent for transcription: `start`/`end` are its timeline times, `cut_*` the audio range including padding.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Seg {
    pub start: f64,
    pub end: f64,
    pub cut_start: f64,
    pub cut_end: f64,
}

/// Stream the pipeline's 16 kHz mono s16le WAV, returning its actual sample count.
fn read_pcm_samples(
    wav: &Path,
    mut check_control: impl FnMut() -> Result<()>,
    mut on_sample: impl FnMut(i16) -> Result<()>,
) -> Result<u64> {
    use std::io::{BufReader, Read, Seek};
    let file = std::fs::File::open(wav)
        .with_context(|| format!("Could not read audio: {}", wav.display()))?;
    let mut reader = BufReader::new(file);
    // Read the RIFF header and scan chunk headers to find the PCM data chunk.
    let mut header = [0u8; 12];
    reader
        .read_exact(&mut header)
        .ok()
        .filter(|_| &header[0..4] == b"RIFF" && &header[8..12] == b"WAVE")
        .ok_or_else(|| anyhow::anyhow!("Cannot parse wav PCM data"))?;
    let data_size = loop {
        let mut ch = [0u8; 8];
        if reader.read_exact(&mut ch).is_err() {
            anyhow::bail!("Cannot parse wav PCM data");
        }
        let size = u32::from_le_bytes([ch[4], ch[5], ch[6], ch[7]]) as u64;
        if &ch[0..4] == b"data" {
            break size;
        }
        // Skip other chunks, including the padding byte after odd sizes.
        reader.seek(std::io::SeekFrom::Current((size + (size & 1)) as i64))?;
    };
    let mut buf = vec![0u8; 128 * 1024];
    let mut half: Option<u8> = None; // low byte of a sample split across reads
    let mut samples = 0;
    let mut emit = |sample| {
        on_sample(sample)?;
        samples += 1;
        Ok::<(), anyhow::Error>(())
    };
    let mut remaining = data_size;
    while remaining > 0 {
        check_control()?;
        let want = remaining.min(buf.len() as u64) as usize;
        let n = reader.read(&mut buf[..want])?;
        if n == 0 {
            break; // truncated file: the data chunk claims more than exists; use what was read
        }
        remaining -= n as u64;
        let mut i = 0;
        if let Some(lo) = half.take() {
            emit(i16::from_le_bytes([lo, buf[0]]))?;
            i = 1;
        }
        while i + 1 < n {
            emit(i16::from_le_bytes([buf[i], buf[i + 1]]))?;
            i += 2;
        }
        if i < n {
            half = Some(buf[i]);
        }
    }
    Ok(samples)
}

/// RMS energy per 100 ms, used to find the quietest point near a planned cut.
pub struct Energy {
    hop: f64,
    rms: Vec<f32>,
}

impl Energy {
    /// Streams the 16 kHz mono 16-bit WAV produced by extract_audio and keeps only the RMS values,
    /// accumulating squares per 100 ms window, so long recordings are not loaded into memory
    /// (two hours of audio would be about 1 GB).
    pub fn load(wav: &Path, check_control: impl FnMut() -> Result<()>) -> Result<Self> {
        let mut rms = Vec::new();
        let mut sum = 0.0f64;
        let mut n_in_hop = 0usize;
        read_pcm_samples(wav, check_control, |sample| {
            rms_accumulate(sample, &mut sum, &mut n_in_hop, &mut rms);
            Ok(())
        })?;
        if n_in_hop > 0 {
            rms.push((sum / n_in_hop as f64).sqrt() as f32);
        }
        Ok(Self { hop: 0.1, rms })
    }

    /// The quietest moment within [a, b] seconds, or None without data.
    fn quietest(&self, a: f64, b: f64) -> Option<f64> {
        let i0 = (a / self.hop).ceil() as usize;
        let i1 = (b / self.hop).floor() as usize;
        if i1 <= i0 || i0 >= self.rms.len() {
            return None;
        }
        let i1 = i1.min(self.rms.len() - 1);
        // total_cmp: RMS values cannot be NaN in theory, but this never panics.
        let (bi, _) = self.rms[i0..=i1]
            .iter()
            .enumerate()
            .min_by(|x, y| x.1.total_cmp(y.1))?;
        Some((i0 + bi) as f64 * self.hop + self.hop / 2.0)
    }
}

/// Adds one sample to the current 100 ms window and closes the window when it is full.
fn rms_accumulate(sample: i16, sum: &mut f64, n_in_hop: &mut usize, rms: &mut Vec<f32>) {
    const HOP: usize = 1600; // 100ms @16k
    *sum += (sample as f64 / 32768.0).powi(2);
    *n_in_hop += 1;
    if *n_in_hop == HOP {
        rms.push((*sum / HOP as f64).sqrt() as f32);
        *sum = 0.0;
        *n_in_hop = 0;
    }
}

const PAD: f64 = 0.25; // seconds of silence added on each outer side of a cut
const SPLIT_WINDOW: f64 = 3.0; // seconds before a planned cut searched for the quietest point
const MIN_PIECE: f64 = 1.0; // shortest piece a forced cut may produce
/// Minimum distance of a cut from the segment start, so odd energy curves cannot produce endless tiny pieces.
const MIN_HARD_CUT: f64 = 0.5;

/// After speech detection: split long segments at quiet points and pad with silence.
/// - A segment longer than max_speech is cut at the quietest point in [target - 3 s, target], avoiding mid-word cuts.
/// - Outer edges get 0.25 s of silence, never neighbouring speech, so no text is duplicated.
///
/// `dur` is probed once by the caller.
pub fn normalize_segments(
    speech: Vec<(f64, f64)>,
    max_speech: f64,
    wav: &Path,
    dur: f64,
    check_control: &dyn Fn() -> Result<()>,
) -> Result<Vec<Seg>> {
    // No speech, or only fragments too short to keep: no segments.
    // Never fall back to sending the whole file: models hallucinate on silence.
    let mut raw = speech;
    raw.retain(|(a, b)| b - a >= 0.2);
    if raw.is_empty() {
        return Ok(vec![]);
    }
    // Read the energy curve only when a segment must be split; it scans the whole file.
    let energy = if raw.iter().any(|(a, b)| b - a > max_speech) {
        let mut interrupted = false;
        let result = Energy::load(wav, || {
            let result = check_control();
            interrupted |= result.is_err();
            result
        });
        match result {
            Ok(energy) => Some(energy),
            Err(error) if interrupted => return Err(error),
            Err(e) => {
                tracing::warn!(
                    "Could not read audio energy; long segments are split at fixed lengths: {e:#}"
                );
                None
            }
        }
    } else {
        None
    };

    let mut pieces: Vec<(f64, f64)> = vec![];
    for &(s, e) in &raw {
        split_smart(s, e, max_speech, energy.as_ref(), &mut pieces);
    }

    // Padding: outer edges extend 0.25 s into silence, limited by the neighbouring speech segments;
    // internal cuts already sit at quiet points and get no padding, so neighbouring chunks do not repeat text.
    let segs = pieces
        .iter()
        .map(|&(s, e)| {
            let host_idx = raw
                .iter()
                .position(|&(rs, re)| s >= rs - 1e-6 && e <= re + 1e-6);
            // Only the first and last piece of a detected segment have outer edges to pad.
            let (is_first_of_host, is_last_of_host) = match host_idx {
                Some(h) => {
                    let (rs, re) = raw[h];
                    let first = (s - rs).abs() < 1e-6;
                    let last = (re - e).abs() < 1e-6;
                    (first, last)
                }
                None => (true, true),
            };
            // End of the previous detected segment: the limit for padding into the gap.
            let speech_lo = match host_idx {
                Some(h) if h > 0 => raw[h - 1].1,
                _ => 0.0,
            };
            let speech_hi = match host_idx {
                Some(h) if h + 1 < raw.len() => raw[h + 1].0,
                _ => dur,
            };
            let cut_start = if is_first_of_host {
                (s - PAD).max(speech_lo).max(0.0)
            } else {
                s
            };
            let cut_end = if is_last_of_host {
                (e + PAD).min(speech_hi)
            } else {
                e
            };
            Seg {
                start: s,
                end: e,
                cut_start,
                cut_end: cut_end.max(e),
            }
        })
        .collect();
    Ok(segs)
}

/// Splits recursively at the quietest point, or at a fixed length when there is none.
fn split_smart(s: f64, e: f64, max: f64, energy: Option<&Energy>, out: &mut Vec<(f64, f64)>) {
    if e - s <= max {
        out.push((s, e));
        return;
    }
    let target = s + max;
    // Search only [target - 3 s, target], so no piece exceeds max_speech; speech services often
    // limit input length.
    let w0 = (target - SPLIT_WINDOW).max(s + MIN_PIECE.min(max / 2.0));
    let w1 = target;
    let cut = energy
        .and_then(|en| en.quietest(w0.max(s), w1))
        .unwrap_or(target);
    let cut = cut.clamp(s + MIN_HARD_CUT, target);
    out.push((s, cut));
    split_smart(cut, e, max, energy, out);
}

pub fn cut_wav(src: &Path, start: f64, end: f64, dest: &Path) -> Result<()> {
    let dur = (end - start).max(0.05);
    let mut cmd = Command::new("ffmpeg");
    cmd.args([
        "-hide_banner",
        "-loglevel",
        "error",
        "-nostdin",
        "-y",
        "-ss",
    ])
    .arg(format!("{start:.3}"))
    .arg("-t")
    .arg(format!("{dur:.3}"))
    .arg("-i")
    .arg(src)
    .args(["-ac", "1", "-ar", "16000", "-c:a", "pcm_s16le"])
    .arg(dest);
    let out = crate::runtime::run_bounded("ffmpeg", &mut cmd, CUT_TIMEOUT, None)?;
    if !out.status.success() {
        anyhow::bail!("ffmpeg could not split audio");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn speech_fixture_samples() -> Vec<i16> {
        let path = Path::new(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/speech.wav"
        ));
        let mut samples = Vec::new();
        let count = read_pcm_samples(
            path,
            || Ok(()),
            |sample| {
                samples.push(sample);
                Ok(())
            },
        )
        .unwrap();
        assert_eq!(count, samples.len() as u64);
        samples
    }

    fn test_wav(samples: &[i16]) -> tempfile::NamedTempFile {
        use std::io::Write as _;
        let pcm: Vec<u8> = samples
            .iter()
            .flat_map(|sample| sample.to_le_bytes())
            .collect();
        let size = u32::try_from(pcm.len()).unwrap();
        let header = [
            b"RIFF".as_slice(),
            &(size + 36).to_le_bytes(),
            b"WAVEfmt ",
            &16u32.to_le_bytes(),
            &1u16.to_le_bytes(),
            &1u16.to_le_bytes(),
            &16_000u32.to_le_bytes(),
            &32_000u32.to_le_bytes(),
            &2u16.to_le_bytes(),
            &16u16.to_le_bytes(),
            b"data",
            &size.to_le_bytes(),
        ]
        .concat();
        let mut wav = tempfile::NamedTempFile::new().unwrap();
        wav.write_all(&header).unwrap();
        wav.write_all(&pcm).unwrap();
        wav
    }

    #[test]
    fn earshot_detects_the_speech_fixture_between_silent_seconds() {
        let mut samples = vec![0; 16_000];
        samples.extend(speech_fixture_samples());
        samples.extend([0; 16_000]);
        let wav = test_wav(&samples);
        let segments = earshot_vad(wav.path(), &Default::default(), &|| Ok(())).unwrap();
        assert_eq!(segments.len(), 1, "{segments:?}");
        assert!((segments[0].start - 1.0).abs() <= 0.35, "{segments:?}");
        assert!((segments[0].end - 3.4).abs() <= 0.35, "{segments:?}");
        assert!(segments[0].cut_start >= 0.0);
        assert!(segments[0].cut_end <= samples.len() as f64 / 16_000.0);
    }

    #[test]
    fn vad_checks_control_during_pcm_scanning() {
        let wav = test_wav(&vec![0; 16_000]);
        let polls = std::cell::Cell::new(0);
        let error = earshot_vad(wav.path(), &Default::default(), &|| {
            polls.set(polls.get() + 1);
            anyhow::ensure!(polls.get() < 2, "test cancellation");
            Ok(())
        })
        .unwrap_err();
        assert_eq!(error.to_string(), "test cancellation");
        assert_eq!(polls.get(), 2);
    }

    #[test]
    fn energy_scan_propagates_control_errors_but_keeps_io_fallback() {
        let wav = test_wav(&vec![0; 160_000]);
        let polls = std::cell::Cell::new(0);
        let error = normalize_segments(vec![(0.0, 10.0)], 2.0, wav.path(), 10.0, &|| {
            polls.set(polls.get() + 1);
            anyhow::ensure!(polls.get() != 2, "test energy cancellation");
            Ok(())
        })
        .unwrap_err();
        assert_eq!(error.to_string(), "test energy cancellation");
        assert_eq!(polls.get(), 2);
        let missing = wav.path().with_extension("missing");
        assert_eq!(
            normalize_segments(vec![(0.0, 10.0)], 2.0, &missing, 10.0, &|| Ok(()))
                .unwrap()
                .len(),
            5
        );
    }

    #[test]
    fn earshot_silence_has_no_segments_including_a_partial_final_frame() {
        let wav = test_wav(&vec![0; 16_000]);
        assert!(
            earshot_vad(wav.path(), &Default::default(), &|| Ok(()))
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn earshot_merges_short_pauses_and_keeps_long_pauses_separate() {
        let speech = speech_fixture_samples();
        for (silence_samples, expected) in [(1_600, 1), (16_000, 2)] {
            let mut samples = vec![0; 16_000];
            samples.extend_from_slice(&speech);
            samples.resize(samples.len() + silence_samples, 0);
            samples.extend_from_slice(&speech);
            samples.extend([0; 16_000]);
            let wav = test_wav(&samples);
            let segments = earshot_vad(wav.path(), &Default::default(), &|| Ok(())).unwrap();
            assert_eq!(
                segments.len(),
                expected,
                "pause={silence_samples}: {segments:?}"
            );
        }
    }

    #[test]
    fn shared_pcm_reader_preserves_energy_and_actual_sample_count() {
        let samples = vec![16_384; 16_017];
        let wav = test_wav(&samples);
        let mut decoded = Vec::new();
        assert_eq!(
            read_pcm_samples(
                wav.path(),
                || Ok(()),
                |sample| {
                    decoded.push(sample);
                    Ok(())
                }
            )
            .unwrap(),
            16_017
        );
        assert_eq!(decoded, samples);
        assert_eq!(
            Energy::load(wav.path(), || Ok(())).unwrap().rms,
            vec![0.5; 11]
        );
    }

    #[test]
    fn vad_settings_control_scores_merging_and_maximum_length() {
        let wav = test_wav(&vec![0; 32_000]);
        let asr = crate::settings::Asr {
            vad_threshold: 0.0,
            min_silence: 0.0,
            max_speech: 1.0,
            ..Default::default()
        };
        let uncut = crate::settings::Asr {
            max_speech: 20.0,
            ..asr.clone()
        };
        assert_eq!(
            earshot_vad(wav.path(), &uncut, &|| Ok(())).unwrap().len(),
            1
        );
        let segments = earshot_vad(wav.path(), &asr, &|| Ok(())).unwrap();
        assert!(segments.len() >= 2);
        assert!(segments.iter().all(|seg| seg.end - seg.start <= 1.0));
        let mut samples = speech_fixture_samples();
        samples.extend([0; 16_000]);
        samples.extend(speech_fixture_samples());
        let wav = test_wav(&samples);
        let asr = crate::settings::Asr {
            min_silence: 2.0,
            ..Default::default()
        };
        assert_eq!(earshot_vad(wav.path(), &asr, &|| Ok(())).unwrap().len(), 1);
    }
}
