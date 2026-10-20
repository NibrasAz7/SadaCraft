@echo off
call "C:\Program Files\Microsoft Visual Studio\18\Community\VC\Auxiliary\Build\vcvars64.bat" >nul 2>&1
set CUDA_PATH=C:\Program Files\NVIDIA GPU Computing Toolkit\CUDA\v13.0
set PATH=%CUDA_PATH%\bin;%PATH%
set NVCC_APPEND_FLAGS=-allow-unsupported-compiler
set CMAKE_GENERATOR=NMake Makefiles
set LIBCLANG_PATH=C:\Program Files\LLVM\bin
cd /d C:\Users\utente\Desktop\Projects\playground\PlaygroundCraft\04-SadaCraft-MeetingCompanion\app\frontend\src-tauri
cargo check --features cuda 2>&1
