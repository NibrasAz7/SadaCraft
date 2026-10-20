@echo off
REM Run any command inside the MSVC build environment for this workspace.
REM System cmake 3.29 lacks the "Visual Studio 18 2026" generator, so all
REM cmake-driven sys crates use the NMake generator with cl.exe instead.
call "C:\Program Files\Microsoft Visual Studio\18\Community\VC\Auxiliary\Build\vcvars64.bat" >nul
set CMAKE_GENERATOR=NMake Makefiles
set LIBCLANG_PATH=C:\Program Files\LLVM\bin
%*
