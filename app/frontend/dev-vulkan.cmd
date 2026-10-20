@echo off
REM SadaCraft dev build — Whisper Vulkan GPU acceleration.
REM Requirements: Vulkan SDK installed (VULKAN_SDK dir below), Ninja on PATH.
REM CUDA path is not viable on this box: nvcc 13.0 rejects MSVC 14.51 (VS 18);
REM Vulkan needs no nvcc and runs on the NVIDIA driver directly.
REM
REM CARGO_TARGET_DIR is kept short on purpose: ggml-vulkan's nested
REM ExternalProject build exceeds MAX_PATH (260) under the default
REM app\target\ path (LongPathsEnabled=0 on this machine).
call "C:\Program Files\Microsoft Visual Studio\18\Community\VC\Auxiliary\Build\vcvars64.bat" >nul
set VULKAN_SDK=C:\VulkanSDK\1.4.363.0
set PATH=%VULKAN_SDK%\Bin;%PATH%
set CMAKE_GENERATOR=Ninja
set CARGO_TARGET_DIR=C:\sada-target
set LIBCLANG_PATH=C:\Program Files\LLVM\bin
npx -y pnpm tauri:dev:vulkan
