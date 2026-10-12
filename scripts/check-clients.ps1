param([switch]$WasmIpc)
$ErrorActionPreference = 'Stop'
$root = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
$created = [Collections.Generic.List[string]]::new()
$oldTmp = $env:TMP
$oldTemp = $env:TEMP
Push-Location $root
try {
    $env:TMP = Join-Path $root 'target'
    $env:TEMP = $env:TMP
    [IO.Directory]::CreateDirectory($env:TMP) | Out-Null
    # Build-only fixtures. Never overwrite an existing frontend or icon.
    $dist = Join-Path $root 'crates/rulibre-gui/dist'
    if (!(Test-Path -LiteralPath $dist)) {
        [IO.Directory]::CreateDirectory($dist) | Out-Null
        $created.Add($dist)
        $index = Join-Path $dist 'index.html'
        [IO.File]::WriteAllText($index, '<html><body>Host check fixture</body></html>')
        $created.Add($index)
    }
    $icons = Join-Path $root 'crates/rulibre-gui/icons'
    if (!(Test-Path -LiteralPath $icons)) {
        [IO.Directory]::CreateDirectory($icons) | Out-Null
        $created.Add($icons)
    }
    $icon = Join-Path $icons 'icon.ico'
    if (!(Test-Path -LiteralPath $icon)) {
        [IO.File]::WriteAllBytes($icon, [Convert]::FromBase64String('AAABAAEAAQEAAAEAIAAwAAAAFgAAACgAAAABAAAAAgAAAAEAIAAAAAAABAAAAAAAAAAAAAAAAAAAAAAAAABkRh7/AAAAAA=='))
        $created.Add($icon)
    }
    & cargo test --locked
    if ($LASTEXITCODE -ne 0) { throw 'core/TUI tests failed' }
    & cargo check --locked
    if ($LASTEXITCODE -ne 0) { throw 'core/TUI check failed' }
    & cargo check --locked -p rulibre-gui
    if ($LASTEXITCODE -ne 0) { throw 'Tauri host check failed' }
    & cargo test --locked -p rulibre-gui
    if ($LASTEXITCODE -ne 0) { throw 'Tauri host tests failed' }
    & cargo check --locked -p rulibre-gui --target wasm32-unknown-unknown
    if ($LASTEXITCODE -ne 0) { throw 'WASM check failed' }
    & cargo fmt --all -- --check
    if ($LASTEXITCODE -ne 0) { throw 'workspace formatting failed' }
    if ($WasmIpc) {
        $bindgenName = if ($env:OS -eq 'Windows_NT') { 'wasm-bindgen.exe' } else { 'wasm-bindgen' }
        $bindgen = Join-Path $root "target/tools/bin/$bindgenName"
        if (!(Test-Path -LiteralPath $bindgen)) { $bindgen = 'wasm-bindgen' }
        & cargo build --locked -p rulibre-gui --target wasm32-unknown-unknown --features ipc-smoke
        if ($LASTEXITCODE -ne 0) { throw 'WASM IPC build failed' }
        & $bindgen --target nodejs --out-name rulibre_gui --out-dir target/wasm-ipc target/wasm32-unknown-unknown/debug/rulibre-gui.wasm
        if ($LASTEXITCODE -ne 0) { throw 'wasm-bindgen failed' }
        & node scripts/check-wasm-ipc.cjs
        if ($LASTEXITCODE -ne 0) { throw 'WASM IPC smoke failed' }
    }
} finally {
    # Remove only the exact files/empty directories created by this run.
    for ($i = $created.Count - 1; $i -ge 0; $i--) {
        $item = [IO.Path]::GetFullPath($created[$i])
        if (!$item.StartsWith($root + [IO.Path]::DirectorySeparatorChar, [StringComparison]::OrdinalIgnoreCase)) {
            throw 'Fixture cleanup escaped workspace'
        }
        Remove-Item -LiteralPath $item
    }
    $env:TMP = $oldTmp
    $env:TEMP = $oldTemp
    Pop-Location
}
