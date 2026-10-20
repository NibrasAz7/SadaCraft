@echo off
call "C:\Program Files\Microsoft Visual Studio\18\Community\VC\Auxiliary\Build\vcvars64.bat" >nul 2>&1
set VULKAN_SDK=C:\VulkanSDK\1.4.363.0
set PATH=%VULKAN_SDK%\Bin;%PATH%
set CMAKE_GENERATOR=Ninja
set CARGO_TARGET_DIR=C:\sada-target
set LIBCLANG_PATH=C:\Program Files\LLVM\bin
cd /d C:\Users\utente\Desktop\Projects\playground\PlaygroundCraft\04-SadaCraft-MeetingCompanion\app\frontend\src-tauri
if "%~1"=="build" (cargo build --features vulkan --bin transcribe_test 2>&1) else (cargo check --features vulkan 2>&1)
