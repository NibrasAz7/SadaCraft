@echo off
REM CUDA check build for SadaCraft (whisper-rs-sys -> whisper.cpp ggml-cuda).
REM REQUIRES: CUDA Toolkit >= 13.2 (first release supporting VS2026 / MSVC 195x).
REM           CUDA 13.0 cannot parse MSVC 14.5x headers (cudafe++.exe crashes).
REM After upgrading the toolkit (winget upgrade Nvidia.CUDA -> 13.4.x),
REM update CUDA_PATH below if the version dir differs.
call "C:\Program Files\Microsoft Visual Studio\18\Community\VC\Auxiliary\Build\vcvars64.bat" >nul 2>&1
set CUDA_PATH=C:\Program Files\NVIDIA GPU Computing Toolkit\CUDA\v13.4
set PATH=%CUDA_PATH%\bin;%PATH%
set CMAKE_GENERATOR=Ninja
set LIBCLANG_PATH=C:\Program Files\LLVM\bin
REM Short target dir REQUIRED: ggml's nested cmake exceeds MAX_PATH
REM under the default target dir (LongPathsEnabled=0).
set CARGO_TARGET_DIR=C:\sada-target-cuda
cd /d C:\Users\utente\Desktop\Projects\playground\PlaygroundCraft\04-SadaCraft-MeetingCompanion\app\frontend\src-tauri
cargo check --features cuda 2>&1
