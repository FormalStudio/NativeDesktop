# Runs cargo in a Windows guest checkout (scripts/windows/vm cargo). MSVC host toolchain at the
# repo's pinned channel; two jobs, since three run the 8 GB guest out of memory with the pagefile
# off; no debug info and no rustc wrapper, which keep the overlay disk small.
# Usage: cargo.ps1 <dir> <cargo args..>. No param block: an advanced script would bind `-p` to
# -PipelineVariable.
$ErrorActionPreference = 'Stop'
$Dir, $Rest = $args
$Rest = @($Rest)
Set-Location $Dir
$channel = 'stable'
if (Test-Path rust-toolchain.toml) {
    $m = Select-String -Path rust-toolchain.toml -Pattern '^channel\s*=\s*"([^"]+)"'
    if ($m) { $channel = $m.Matches[0].Groups[1].Value }
}
$env:RUSTUP_TOOLCHAIN = "$channel-x86_64-pc-windows-msvc"
$env:CARGO_BUILD_JOBS = '2'
$env:CARGO_PROFILE_DEV_DEBUG = '0'
# PowerShell 5.1 turns a native command's stderr into error records, which Stop would throw on.
$ErrorActionPreference = 'Continue'
& cargo --config "build.rustc-wrapper=''" @Rest 2>&1 | ForEach-Object { "$_" }
exit $LASTEXITCODE
