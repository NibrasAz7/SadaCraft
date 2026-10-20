use crate::audio::{GpuType, PerformanceTier};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WhisperCompiledBackend {
    Metal,
    Cuda,
    Vulkan,
    HipBlas,
    Cpu,
}

impl WhisperCompiledBackend {
    pub fn current() -> Self {
        if cfg!(feature = "cuda") {
            Self::Cuda
        } else if cfg!(feature = "vulkan") {
            Self::Vulkan
        } else if cfg!(feature = "hipblas") {
            Self::HipBlas
        } else if cfg!(target_os = "macos") || cfg!(feature = "metal") {
            Self::Metal
        } else {
            Self::Cpu
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Metal => "Metal",
            Self::Cuda => "Cuda",
            Self::Vulkan => "Vulkan",
            Self::HipBlas => "HipBlas",
            Self::Cpu => "Cpu",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WhisperContextAcceleration {
    pub compiled_backend: WhisperCompiledBackend,
    pub runtime_detected_gpu: GpuType,
    pub use_gpu: bool,
    pub flash_attn: bool,
    pub gpu_device: i32,
}

impl WhisperContextAcceleration {
    pub fn status_label(self) -> &'static str {
        match (self.compiled_backend, self.flash_attn) {
            (WhisperCompiledBackend::Metal, true) => "Metal GPU with Flash Attention (Ultra-Fast)",
            (WhisperCompiledBackend::Metal, false) => "Metal GPU acceleration",
            (WhisperCompiledBackend::Cuda, true) => "CUDA GPU with Flash Attention (Ultra-Fast)",
            (WhisperCompiledBackend::Cuda, false) => "CUDA GPU acceleration",
            (WhisperCompiledBackend::Vulkan, _) => "Vulkan GPU acceleration",
            (WhisperCompiledBackend::HipBlas, _) => "HIP BLAS GPU acceleration",
            (WhisperCompiledBackend::Cpu, _) => "CPU processing only",
        }
    }
}

pub fn whisper_context_acceleration_for(
    compiled_backend: WhisperCompiledBackend,
    runtime_detected_gpu: GpuType,
    performance_tier: PerformanceTier,
) -> WhisperContextAcceleration {
    let use_gpu = !matches!(compiled_backend, WhisperCompiledBackend::Cpu);
    let fast_tier = matches!(performance_tier, PerformanceTier::High | PerformanceTier::Ultra);
    let flash_attn = match compiled_backend {
        WhisperCompiledBackend::Metal | WhisperCompiledBackend::Cuda => fast_tier,
        WhisperCompiledBackend::Vulkan | WhisperCompiledBackend::HipBlas | WhisperCompiledBackend::Cpu => false,
    };

    WhisperContextAcceleration {
        compiled_backend,
        runtime_detected_gpu,
        use_gpu,
        flash_attn: use_gpu && flash_attn,
        gpu_device: preferred_gpu_device(compiled_backend),
    }
}

/// Resolve which GPU index whisper should use. Order of precedence:
/// `SADACRAFT_WHISPER_GPU_DEVICE` env override -> discrete-GPU autodetect -> 0.
fn preferred_gpu_device(compiled_backend: WhisperCompiledBackend) -> i32 {
    if let Ok(v) = std::env::var("SADACRAFT_WHISPER_GPU_DEVICE") {
        if let Ok(idx) = v.parse::<i32>() {
            log::info!("Whisper GPU device forced to {} via SADACRAFT_WHISPER_GPU_DEVICE", idx);
            return idx;
        }
    }
    #[cfg(all(target_os = "windows", feature = "vulkan"))]
    if compiled_backend == WhisperCompiledBackend::Vulkan {
        if let Some(idx) = detect_discrete_gpu_index() {
            log::info!("Whisper Vulkan device autodetected discrete GPU at index {}", idx);
            return idx;
        }
    }
    let _ = compiled_backend;
    0
}

/// Index of the discrete GPU in adapter enumeration order, which matches
/// ggml-vulkan's vkEnumeratePhysicalDevices order on Windows.
#[cfg(all(target_os = "windows", feature = "vulkan"))]
fn detect_discrete_gpu_index() -> Option<i32> {
    use windows::Win32::Graphics::Dxgi::{CreateDXGIFactory1, IDXGIFactory1, DXGI_ADAPTER_FLAG_SOFTWARE};

    unsafe {
        let factory: IDXGIFactory1 = CreateDXGIFactory1().ok()?;
        let mut best: Option<(i32, usize, String)> = None;
        for i in 0u32.. {
            let Ok(adapter) = factory.EnumAdapters1(i) else { break };
            let mut desc = windows::Win32::Graphics::Dxgi::DXGI_ADAPTER_DESC1::default();
            if adapter.GetDesc1(&mut desc).is_err() {
                continue;
            }
            if desc.Flags & (DXGI_ADAPTER_FLAG_SOFTWARE.0 as u32) != 0 {
                continue;
            }
            let name = String::from_utf16_lossy(&desc.Description)
                .trim_end_matches(char::from(0))
                .to_string();
            let vram = desc.DedicatedVideoMemory;
            if best.as_ref().is_none_or(|(_, v, _)| vram > *v) {
                log::debug!("DXGI adapter {}: {} ({} MB dedicated)", i, name, vram / 1_048_576);
                best = Some((i as i32, vram, name));
            }
        }
        if let Some((idx, _, name)) = best {
            log::info!("Discrete GPU selected for whisper: {} (adapter index {})", name, idx);
            return Some(idx);
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn acceleration_vulkan_backend_ignores_runtime_cuda_flash_attention() {
        let params = whisper_context_acceleration_for(
            WhisperCompiledBackend::Vulkan,
            GpuType::Cuda,
            PerformanceTier::High,
        );

        assert_eq!(params.compiled_backend, WhisperCompiledBackend::Vulkan);
        assert_eq!(params.runtime_detected_gpu, GpuType::Cuda);
        assert!(params.use_gpu);
        assert!(!params.flash_attn);
    }

    #[test]
    fn acceleration_vulkan_backend_keeps_gpu_without_runtime_gpu_detection() {
        let params = whisper_context_acceleration_for(
            WhisperCompiledBackend::Vulkan,
            GpuType::None,
            PerformanceTier::Low,
        );

        assert!(params.use_gpu);
        assert!(!params.flash_attn);
    }

    #[test]
    fn acceleration_cuda_backend_enables_flash_attention_for_fast_tiers() {
        let high = whisper_context_acceleration_for(
            WhisperCompiledBackend::Cuda,
            GpuType::Cuda,
            PerformanceTier::High,
        );
        let ultra = whisper_context_acceleration_for(
            WhisperCompiledBackend::Cuda,
            GpuType::Cuda,
            PerformanceTier::Ultra,
        );

        assert!(high.use_gpu);
        assert!(high.flash_attn);
        assert!(ultra.use_gpu);
        assert!(ultra.flash_attn);
    }

    #[test]
    fn acceleration_cpu_backend_disables_gpu_and_flash_attention() {
        for runtime_gpu in [GpuType::None, GpuType::Cuda, GpuType::Vulkan] {
            let params = whisper_context_acceleration_for(
                WhisperCompiledBackend::Cpu,
                runtime_gpu,
                PerformanceTier::Ultra,
            );

            assert!(!params.use_gpu);
            assert!(!params.flash_attn);
        }
    }
}
