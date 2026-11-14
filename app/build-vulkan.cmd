@echo off
REM SadaCraft Vulkan binary build — cargo build only, no dev server.
REM Produces C:\sv\debug\sadacraft.exe; run it against the already-running
REM Next.js dev server (localhost:3118) or after `pnpm dev`.
call "C:\Program Files\Microsoft Visual Studio\18\Community\VC\Auxiliary\Build\vcvars64.bat" >nul
set VULKAN_SDK=C:\VulkanSDK\1.4.363.0
set PATH=%VULKAN_SDK%\Bin;%PATH%
set CMAKE_GENERATOR=Ninja
set CARGO_TARGET_DIR=C:\sv
set LIBCLANG_PATH=C:\Program Files\LLVM\bin
cargo build -p sadacraft --no-default-features --features "platform-default vulkan"
