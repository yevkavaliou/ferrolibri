param([switch]$RealConverters, [switch]$Ui)
$ErrorActionPreference = 'Stop'
$root = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
$oldTmp = $env:TMP
$oldTemp = $env:TEMP
$oldTmpDir = $env:TMPDIR
Push-Location $root
try {
    $env:TMP = Join-Path $root 'target'
    $env:TEMP = $env:TMP
    $env:TMPDIR = $env:TMP
    [IO.Directory]::CreateDirectory($env:TMP) | Out-Null
    & (Join-Path $PSScriptRoot 'check-clients.ps1')
    & cargo clippy --locked -p rulibre-core -p rulibre --all-targets -- -D warnings
    if ($LASTEXITCODE -ne 0) { throw 'core/TUI Clippy failed' }
    & cargo test --locked -p rulibre-core --test device_detection native_volume_enumeration_smoke -- --ignored --exact --nocapture
    if ($LASTEXITCODE -ne 0) { throw 'native volume enumeration smoke failed' }
    if ($Ui) {
        # The iced client has its own workflow (.github/workflows/ui.yml) with the same script.
        & (Join-Path $PSScriptRoot 'check-ui.ps1')
    }
    if ($RealConverters) {
        & cargo test --locked -p rulibre-core --lib converter::tests::real_ebook_convert_keeps_library_read_only -- --ignored --exact --nocapture
        if ($LASTEXITCODE -ne 0) { throw 'real ebook-convert smoke failed' }
    }
} finally {
    $env:TMP = $oldTmp
    $env:TEMP = $oldTemp
    $env:TMPDIR = $oldTmpDir
    Pop-Location
}
