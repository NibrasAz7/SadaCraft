@echo off
REM SadaCraft release build — Vulkan GPU acceleration, NSIS installer.
REM Same env constraints as build-vulkan.cmd: short CARGO_TARGET_DIR for
REM ggml-vulkan MAX_PATH, Ninja generator, LLVM libclang for bindgen.
call "C:\Program Files\Microsoft Visual Studio\18\Community\VC\Auxiliary\Build\vcvars64.bat" >nul
set VULKAN_SDK=C:\VulkanSDK\1.4.363.0
set PATH=C:\shim-bin;%VULKAN_SDK%\Bin;%PATH%
set CMAKE_GENERATOR=Ninja
set CARGO_TARGET_DIR=C:\sv
set LIBCLANG_PATH=C:\Program Files\LLVM\bin
cd /d "%~dp0frontend"
corepack pnpm tauri:build:vulkan
