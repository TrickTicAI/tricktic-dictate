//! Fixed, packaged runtime families. Browser settings never supply executable paths.
use super::*;
pub const PARAKEET: &str = "parakeet-tdt-0-6b-v3";
pub fn nemo(model: &str) -> bool {
    model == PARAKEET
}
pub fn executable(model: &str) -> &'static str {
    if nemo(model) {
        if cfg!(windows) {
            "nemo-speech.exe"
        } else {
            "nemo-speech"
        }
    } else {
        WORKER_EXECUTABLE
    }
}
pub fn supported(model: &str) -> bool {
    // Parakeet runs only where Writer builds NeMo itself. The Windows package
    // ships no NeMo: the only Windows build is the upstream prebuilt without
    // private-worker authentication and with unattributed vendor DLLs (MW-SEC-26).
    !nemo(model) || cfg!(all(target_os = "macos", target_arch = "aarch64"))
}
pub fn compatibility_issue(model: &str) -> Option<&'static str> {
    if supported(model) {
        None
    } else if cfg!(windows) {
        Some(
            "Parakeet is not included in the Windows app. Choose a Whisper model on this computer.",
        )
    } else {
        Some("This runtime is not available on this computer. Choose a Whisper model.")
    }
}
pub fn directories(root: &Path, model: &str, cpu: bool) -> Vec<PathBuf> {
    if nemo(model) {
        root.parent()
            .map(|p| {
                vec![p
                    .join("nemo")
                    .join(if cpu { "cpu" } else { GPU_BACKEND })
                    .join("bin")]
            })
            .unwrap_or_default()
    } else if cpu {
        vec![root.join("cpu"), root.to_owned()]
    } else {
        vec![root.to_owned()]
    }
}
pub fn gpu_library(dir: &Path, model: &str) -> PathBuf {
    if cfg!(target_os = "macos") && nemo(model) {
        dir.join("../lib").join(VULKAN_BACKEND_LIBRARY)
    } else {
        dir.join(VULKAN_BACKEND_LIBRARY)
    }
}
impl Markers {
    pub(super) fn observe_nemo(&mut self, line: &str) {
        if line.starts_with("[asr] model=parakeet-tdt-0.6b-v3.oss-align.q8_0 ") {
            self.model_path_matched = true;
            self.cpu |= line.contains("backend=CPU");
            self.gpu |= line.contains("backend=Vulkan")
                || (cfg!(target_os = "macos") && line.contains("backend=MTL"));
        }
    }
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;
    #[test]
    fn metal_device_log_confirms_parakeet_backend() {
        let mut markers = Markers::default();
        markers
            .observe_nemo("[asr] model=parakeet-tdt-0.6b-v3.oss-align.q8_0 head=tdt backend=MTL0");
        assert!(markers.gpu && markers.model_path_matched && !markers.cpu);
        let mut cpu = Markers::default();
        cpu.observe_nemo("[asr] model=parakeet-tdt-0.6b-v3.oss-align.q8_0 head=tdt backend=CPU");
        assert!(cpu.cpu && !cpu.gpu);
    }
}
