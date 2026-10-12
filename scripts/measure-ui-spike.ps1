param(
    [ValidateSet('default', 'dx12', 'software')]
    [string]$Backend = 'default',
    [string]$OutputDirectory = 'target/ui-spike'
)
$ErrorActionPreference = 'Stop'
$root = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
$output = [IO.Path]::GetFullPath((Join-Path $root $OutputDirectory))
$previousLog = $env:RUST_LOG
$previousBackend = $env:ICED_BACKEND
$previousWgpuBackend = $env:WGPU_BACKEND
Push-Location $root
try {
    & cargo build -p rulibre-ui --example spike_capabilities --locked
    if ($LASTEXITCODE -ne 0) { throw 'Spike build failed' }
    New-Item -ItemType Directory -Force $output | Out-Null
    $env:RUST_LOG = 'iced_winit=info,iced_wgpu=info,fontdb=warn'
    $env:ICED_BACKEND = if ($Backend -eq 'software') { 'tiny-skia' } else { 'wgpu' }
    $env:WGPU_BACKEND = if ($Backend -eq 'dx12') { 'dx12' } else { $null }
    $process = Start-Process -FilePath (Join-Path $root 'target/debug/examples/spike_capabilities.exe') `
        -ArgumentList '--measure', ('"{0}"' -f $output) -WindowStyle Hidden -PassThru `
        -RedirectStandardOutput (Join-Path $output 'run.log') `
        -RedirectStandardError (Join-Path $output 'renderer.log')
    $samples = [Collections.Generic.List[object]]::new()
    while (-not $process.HasExited) {
        $process.Refresh()
        if ($process.HasExited) { break }
        $samples.Add([PSCustomObject]@{
            elapsed_ms = [math]::Round(((Get-Date) - $process.StartTime).TotalMilliseconds)
            working_set_bytes = $process.WorkingSet64
            private_bytes = $process.PrivateMemorySize64
        })
        Start-Sleep -Milliseconds 250
    }
    $process.WaitForExit()
    $samples | Export-Csv (Join-Path $output 'memory.csv') -NoTypeInformation
    if ($process.ExitCode -ne 0) { throw "Spike exited with $($process.ExitCode); see renderer.log" }
    Get-Content (Join-Path $output 'run.log')
    $samples | Measure-Object -Property working_set_bytes, private_bytes -Maximum
} finally {
    $env:RUST_LOG = $previousLog
    $env:ICED_BACKEND = $previousBackend
    $env:WGPU_BACKEND = $previousWgpuBackend
    Pop-Location
}
