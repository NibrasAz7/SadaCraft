@echo off
REM SadaCraft dev build — MSVC toolchain via vcvars64 + NMake generator
REM (system cmake 3.29 lacks the "Visual Studio 18 2026" generator;
REM  NMake works with any cmake and the same cl.exe toolchain)
call "C:\Program Files\Microsoft Visual Studio\18\Community\VC\Auxiliary\Build\vcvars64.bat" >nul
set CMAKE_GENERATOR=NMake Makefiles
set LIBCLANG_PATH=C:\Program Files\LLVM\bin
npx -y pnpm tauri:dev:cpu
