@echo off
rem Run any cargo command inside the MSVC 2022 BuildTools environment.
rem Usage: scripts\msvc.cmd cargo build --release --bin rustseis-desktop
call "C:\Program Files (x86)\Microsoft Visual Studio\2022\BuildTools\VC\Auxiliary\Build\vcvars64.bat" >nul
cd /d "%~dp0.."
%*
