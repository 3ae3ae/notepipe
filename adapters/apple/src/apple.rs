//! FFI and cache preparation recovered from the former core Apple backend.

use anyhow::{Result, bail};

fn cache_manifest(model: &str) -> Result<String> {
    let parts: &[(&str, &[&str])] = match model {
        "qwen3-1.7b" => &[(
            "aufklarer/Qwen3-ASR-1.7B-MLX-8bit",
            &[
                "config.json",
                "vocab.json",
                "merges.txt",
                "tokenizer_config.json",
            ],
        )],
        "qwen3-0.6b" => &[
            (
                "aufklarer/Qwen3-ASR-CoreML",
                &[
                    "config.json",
                    "encoder.mlmodelc/**",
                    "embedding.mlmodelc/**",
                    "decoder_part1.mlmodelc/**",
                    "decoder_part2.mlmodelc/**",
                ],
            ),
            (
                "aufklarer/Qwen3-ASR-0.6B-MLX-4bit",
                &["vocab.json", "merges.txt", "tokenizer_config.json"],
            ),
        ],
        "whisper" => &[(
            "aufklarer/Whisper-Large-v3-Turbo-CoreML",
            &[
                "generation_config.json",
                "manifest.json",
                "tokenizer.json",
                "tokenizer_config.json",
                "vocab.json",
                "merges.txt",
                "MelSpectrogram.mlmodelc/**",
                "AudioEncoder.mlmodelc/**",
                "TextDecoder.mlmodelc/**",
                "TextDecoderContextPrefill.mlmodelc/**",
            ],
        )],
        _ => bail!("Unsupported Apple model: {model}"),
    };
    Ok(serde_json::to_string(
        &parts
            .iter()
            .map(|(repo, files)| serde_json::json!({"repo":repo,"files":files}))
            .collect::<Vec<_>>(),
    )?)
}

#[cfg(apple_native)]
mod native {
    use super::*;
    use std::{
        ffi::{CStr, CString, c_char, c_void},
        path::{Path, PathBuf},
    };

    unsafe extern "C" {
        fn notepipe_asr_create(model: *const c_char) -> *mut c_void;
        fn notepipe_asr_prepare_cache(model: *const c_char, manifest: *const c_char) -> i32;
        fn notepipe_asr_transcribe(
            handle: *mut c_void,
            wav: *const c_char,
            language: *const c_char,
            out: *mut c_char,
            len: usize,
        ) -> i32;
        fn notepipe_asr_destroy(handle: *mut c_void);
        fn notepipe_last_error() -> *const c_char;
    }

    #[unsafe(no_mangle)]
    extern "C" fn notepipe_model_progress(
        fraction: f64,
        _phase: *const c_char,
        message: *const c_char,
    ) {
        if message.is_null() {
            return;
        }
        static LAST: std::sync::atomic::AtomicI64 = std::sync::atomic::AtomicI64::new(-1);
        let percent = (fraction.clamp(0.0, 1.0) * 100.0) as i64;
        if LAST.swap(percent, std::sync::atomic::Ordering::Relaxed) != percent {
            eprintln!(
                "Model {percent}%: {}",
                unsafe { CStr::from_ptr(message) }.to_string_lossy()
            );
        }
    }

    fn last_error() -> String {
        let error = unsafe { notepipe_last_error() };
        if error.is_null() {
            "Unknown Apple transcription error".into()
        } else {
            unsafe { CStr::from_ptr(error) }
                .to_string_lossy()
                .into_owned()
        }
    }

    fn find_metallib(executable: &Path) -> Option<PathBuf> {
        let dir = executable.parent()?;
        let resources = dir.join("Resources");
        let bundle_resources = dir.join("../Resources");
        let cwd = std::env::current_dir().ok()?;
        [dir, &resources, &bundle_resources, &cwd]
            .into_iter()
            .flat_map(|base| ["mlx.metallib", "default.metallib"].map(|name| base.join(name)))
            .find(|path| path.is_file())
    }

    pub struct Asr {
        handle: *mut c_void,
    }

    impl Asr {
        pub fn load(model: &str) -> Result<Self> {
            let manifest = CString::new(cache_manifest(model)?)?;
            anyhow::ensure!(
                find_metallib(&std::env::current_exe()?).is_some(),
                "Missing mlx.metallib beside the Apple adapter; rebuild with the Metal toolchain"
            );
            let model = CString::new(model)?;
            // The Swift downloader honors HF_ENDPOINT and HF_HUB_OFFLINE directly.
            if unsafe { notepipe_asr_prepare_cache(model.as_ptr(), manifest.as_ptr()) } != 0 {
                bail!("{}", last_error());
            }
            let handle = unsafe { notepipe_asr_create(model.as_ptr()) };
            anyhow::ensure!(!handle.is_null(), "{}", last_error());
            Ok(Self { handle })
        }

        pub fn transcribe(&self, wav: &Path, language: &str) -> Result<String> {
            let wav = CString::new(wav.to_string_lossy().as_bytes())?;
            let language = CString::new(language)?;
            let mut out = vec![0u8; 16 * 1024];
            let status = unsafe {
                notepipe_asr_transcribe(
                    self.handle,
                    wav.as_ptr(),
                    language.as_ptr(),
                    out.as_mut_ptr().cast(),
                    out.len(),
                )
            };
            match status {
                0 => Ok(CStr::from_bytes_until_nul(&out)?
                    .to_string_lossy()
                    .into_owned()),
                1 => Ok(String::new()),
                2 => bail!("Apple transcript exceeded the output buffer"),
                _ => bail!("{}", last_error()),
            }
        }
    }

    impl Drop for Asr {
        fn drop(&mut self) {
            unsafe {
                notepipe_asr_destroy(self.handle);
            }
        }
    }
}

#[cfg(apple_native)]
pub use native::Asr;

#[cfg(not(apple_native))]
pub struct Asr;

#[cfg(not(apple_native))]
impl Asr {
    pub fn load(model: &str) -> Result<Self> {
        cache_manifest(model)?;
        bail!(
            "Apple transcription is unavailable: this adapter requires macOS arm64 and a successful Swift/Metal build. See the Cargo build warnings; use another ASR service until the native build succeeds."
        )
    }
    pub fn transcribe(&self, _wav: &std::path::Path, _language: &str) -> Result<String> {
        unreachable!("load must succeed before serving requests")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn manifests_match_the_three_native_loaders_without_vad_downloads() {
        for (model, count) in [("qwen3-1.7b", 1), ("qwen3-0.6b", 2), ("whisper", 1)] {
            let manifest: serde_json::Value =
                serde_json::from_str(&cache_manifest(model).unwrap()).unwrap();
            assert_eq!(manifest.as_array().unwrap().len(), count);
            assert!(!manifest.to_string().contains("Silero"));
            assert!(
                manifest[0]["files"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .all(serde_json::Value::is_string)
            );
        }
        assert!(cache_manifest("other").is_err());
    }
}
