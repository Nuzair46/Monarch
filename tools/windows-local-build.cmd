@REM This script is for me (@Nuzair46) to build locally, might not work for you without tweaks.
@echo off
setlocal EnableExtensions

set "REPO_DIR=%~dp0.."
pushd "%REPO_DIR%" || exit /b 1

call :find_vsdevcmd
if errorlevel 1 goto :fail

call "%VSDEVCMD%" -arch=amd64 -host_arch=amd64 >nul
if errorlevel 1 (
  echo Failed to initialize Visual Studio developer environment.
  goto :fail
)

set "PATH=%ProgramFiles%\nodejs;%USERPROFILE%\.cargo\bin;%PATH%"

echo [check] Toolchain
where rc || goto :fail
where node || goto :fail
where cargo || goto :fail
where rustc || goto :fail
node -v || goto :fail
call "%ProgramFiles%\nodejs\corepack.cmd" yarn -v || goto :fail
cargo -V || goto :fail
rustc -V || goto :fail

echo [step] Rust target
rustup target add x86_64-pc-windows-msvc || goto :fail

echo [step] Install JS dependencies
call "%ProgramFiles%\nodejs\corepack.cmd" yarn install --frozen-lockfile || goto :fail

echo [step] Build frontend
call "%ProgramFiles%\nodejs\corepack.cmd" yarn build || goto :fail

echo [step] Run core tests
cargo test --manifest-path Cargo.toml || goto :fail

echo [step] Build Windows MSI
call "%ProgramFiles%\nodejs\corepack.cmd" yarn tauri build --bundles msi || goto :fail

echo [done] Build completed successfully.
echo MSI output: src-tauri\target\release\bundle\msi\
popd
exit /b 0

:find_vsdevcmd
set "VSDEVCMD="
for %%P in (
  "C:\Program Files\Microsoft Visual Studio\2022\BuildTools\Common7\Tools\VsDevCmd.bat"
  "C:\Program Files\Microsoft Visual Studio\2022\Community\Common7\Tools\VsDevCmd.bat"
  "C:\Program Files\Microsoft Visual Studio\2022\Professional\Common7\Tools\VsDevCmd.bat"
  "C:\Program Files\Microsoft Visual Studio\2022\Enterprise\Common7\Tools\VsDevCmd.bat"
) do (
  if exist %%~P (
    set "VSDEVCMD=%%~P"
    goto :eof
  )
)
echo Could not find VsDevCmd.bat (VS 2022 Build Tools/Community/Professional/Enterprise).
exit /b 1

:fail
set "ERR=%ERRORLEVEL%"
if "%ERR%"=="" set "ERR=1"
echo [error] Build failed with exit code %ERR%.
popd
exit /b %ERR%
