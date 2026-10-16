$ErrorActionPreference = 'Stop'
$root = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
Push-Location $root
try {
    & cargo fmt --all -- --check
    if ($LASTEXITCODE -ne 0) { throw 'Workspace formatting failed' }
    & cargo build -p ferrolibri-ui --locked
    if ($LASTEXITCODE -ne 0) { throw 'UI build failed' }
    & cargo clippy -p ferrolibri-ui --locked --all-targets -- -D warnings
    if ($LASTEXITCODE -ne 0) { throw 'UI Clippy failed' }
    & cargo test -p ferrolibri-ui --locked --all-targets
    if ($LASTEXITCODE -ne 0) { throw 'UI tests failed' }
} finally {
    Pop-Location
}
