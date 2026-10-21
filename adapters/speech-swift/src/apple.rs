//! FFI to the Swift transcription engine.

use anyhow::{Result, bail};
use std::{
    ffi::{CStr, CString, c_char, c_void},
    path::Path,
};

unsafe extern "C" {
    fn notepipe_asr_create(model: *const c_char) -> *mut c_void;
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
extern "C" fn notepipe_model_progress(fraction: f64, message: *const c_char) {
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

pub struct Asr {
    handle: *mut c_void,
}

impl Asr {
    pub fn load(model: &str) -> Result<Self> {
        let model = CString::new(model)?;
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
