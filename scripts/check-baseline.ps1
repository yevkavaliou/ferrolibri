# Run from any directory: pwsh -File scripts/check-baseline.ps1
# GUI and whole-workspace formatting are separate checks; see docs/BASELINE.md.
$ErrorActionPreference = 'Stop'
Push-Location (Join-Path $PSScriptRoot '..')
try {
    & cargo test --locked
    if ($LASTEXITCODE -ne 0) { throw 'cargo test --locked failed' }
    & cargo check --locked
    if ($LASTEXITCODE -ne 0) { throw 'cargo check --locked failed' }
    & cargo fmt -p rulibre-core -p rulibre -- --check
    if ($LASTEXITCODE -ne 0) { throw 'core/TUI formatting failed' }
} finally {
    Pop-Location
}
