@echo off
setlocal
set "LIGHTCRAFT_BUILD_SCRIPT=%~f0"
powershell.exe -NoLogo -NoProfile -ExecutionPolicy Bypass -Command "& ([scriptblock]::Create(([IO.File]::ReadAllText($env:LIGHTCRAFT_BUILD_SCRIPT) -split '(?m)^# POWERSHELL_START\r?$', 2)[1]))"
set "BUILD_EXIT_CODE=%ERRORLEVEL%"
if not "%~1"=="--no-pause" pause
exit /b %BUILD_EXIT_CODE%
# POWERSHELL_START
$ErrorActionPreference = 'Stop'
try {
    $root = Split-Path -Parent $env:LIGHTCRAFT_BUILD_SCRIPT
    Set-Location -LiteralPath $root
    $env:PATH = (Join-Path $env:USERPROFILE '.cargo\bin') + ';' + $env:PATH
    if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) {
        throw 'Rust is missing. Install the MSVC toolchain from https://rustup.rs, then click this script again.'
    }
    if (-not (Get-Command git -ErrorAction SilentlyContinue)) {
        throw 'Git is missing. Install Git for Windows from https://git-scm.com/downloads/win, then try again.'
    }
    $vswhere = Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio\Installer\vswhere.exe'
    if (-not (Test-Path -LiteralPath $vswhere)) {
        throw 'Install Visual Studio Build Tools with Desktop development with C++ and the Windows SDK, then try again.'
    }
    $vs = & $vswhere -latest -products '*' -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath
    if ($LASTEXITCODE -ne 0 -or -not $vs) {
        throw 'No MSVC x64 build tools found. Add Desktop development with C++ and the Windows SDK in Visual Studio Installer.'
    }
    $vcvars = Join-Path $vs 'VC\Auxiliary\Build\vcvars64.bat'
    if (-not (Test-Path -LiteralPath $vcvars)) {
        throw "MSVC environment script is missing: $vcvars"
    }
    . (Join-Path $root 'packaging\windows\fonts.ps1')
    Initialize-CraftFonts $root
    $env:LIGHTCRAFT_REQUIRE_WINRES = '1'
    Write-Host ''
    Write-Host 'Building LightCraft for Windows x64 with Korean fonts (first build can take a long time)...'
    & $env:ComSpec /d /c "call `"$vcvars`" >nul && cargo build --release --locked --target x86_64-pc-windows-msvc -p lightcraft -p lightcraft-cli --features lightcraft/heif,lightcraft-cli/heif"
    if ($LASTEXITCODE -ne 0) {
        throw "Build failed with exit code $LASTEXITCODE. See the error above."
    }
    $targetDir = if ($env:CARGO_TARGET_DIR) { $env:CARGO_TARGET_DIR } else { Join-Path $root 'target' }
    $output = [IO.Path]::GetFullPath((Join-Path $targetDir 'x86_64-pc-windows-msvc\release'))
    foreach ($name in @('lightcraft.exe', 'lightcraft-cli.exe')) {
        if (-not (Test-Path -LiteralPath (Join-Path $output $name) -PathType Leaf)) {
            throw "Build returned success but $name was not found in $output"
        }
    }
    Write-Host ''
    Write-Host 'Build succeeded. Start the app by double-clicking:'
    Write-Host (Join-Path $output 'lightcraft.exe')
    exit 0
} catch {
    [Console]::Error.WriteLine("BUILD FAILED: $($_.Exception.Message)")
    exit 1
}
