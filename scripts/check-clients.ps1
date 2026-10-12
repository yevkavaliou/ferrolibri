# Core library and TUI checks. The iced UI has its own script (check-ui.ps1)
# because it needs graphics libraries that the core does not.
$ErrorActionPreference = 'Stop'
$root = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
$oldTmp = $env:TMP
$oldTemp = $env:TEMP
Push-Location $root
try {
    $env:TMP = Join-Path $root 'target'
    $env:TEMP = $env:TMP
    [IO.Directory]::CreateDirectory($env:TMP) | Out-Null
    & cargo test --locked -p rulibre-core -p rulibre
    if ($LASTEXITCODE -ne 0) { throw 'core/TUI tests failed' }
    & cargo check --locked -p rulibre-core -p rulibre
    if ($LASTEXITCODE -ne 0) { throw 'core/TUI check failed' }
    & cargo fmt --all -- --check
    if ($LASTEXITCODE -ne 0) { throw 'workspace formatting failed' }
} finally {
    $env:TMP = $oldTmp
    $env:TEMP = $oldTemp
    Pop-Location
}
