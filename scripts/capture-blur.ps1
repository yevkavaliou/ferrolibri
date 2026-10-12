# Windows only: shows the real window-blur probe over a striped backdrop and saves an
# OS-level desktop capture (iced's own screenshots cannot include the acrylic layer).
param(
    [Parameter(Mandatory)][string]$Output,
    [ValidateRange(10, 100)][int]$Opacity = 30,
    # Hand the focus to the backdrop window before capturing (inactive-window look).
    [switch]$Inactive,
    # Seconds to wait before the capture, and when (ms after start) to fake scroll activity.
    [double]$Delay = 5,
    [int]$ScrollAtMs = -1
)
$ErrorActionPreference = 'Stop'
$root = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
& cargo build -p rulibre-ui --example blur_probe --locked
if ($LASTEXITCODE -ne 0) { throw 'blur_probe build failed' }
Add-Type -AssemblyName System.Windows.Forms, System.Drawing
$flag = Join-Path ([IO.Path]::GetTempPath()) "rulibre-blur-inactive.flag"
Remove-Item $flag -ErrorAction SilentlyContinue
$backdrop = Start-Job -ArgumentList $flag -ScriptBlock {
    param($flag)
    Add-Type -Namespace Native -Name Win -MemberDefinition '[DllImport("user32.dll")] public static extern bool SetWindowPos(System.IntPtr h, System.IntPtr after, int x, int y, int cx, int cy, uint flags);'
    Add-Type -AssemblyName System.Windows.Forms, System.Drawing
    $form = New-Object Windows.Forms.Form
    $form.FormBorderStyle = 'None'; $form.StartPosition = 'Manual'; $form.Location = '0,0'
    $form.Size = [Windows.Forms.Screen]::PrimaryScreen.Bounds.Size
    $bitmap = New-Object Drawing.Bitmap $form.Width, $form.Height
    $g = [Drawing.Graphics]::FromImage($bitmap)
    $colors = 'Red', 'Yellow', 'Lime', 'Cyan', 'Blue', 'Magenta', 'White', 'Black'
    for ($i = 0; $i -lt 40; $i++) {
        $brush = New-Object Drawing.SolidBrush ([Drawing.Color]::FromName($colors[$i % 8]))
        $g.FillRectangle($brush, $i * ($form.Width / 40), 0, ($form.Width / 40) + 1, $form.Height)
    }
    $form.BackgroundImage = $bitmap
    $form.Show()
    $end = (Get-Date).AddSeconds(16)
    $sent = $false
    while ((Get-Date) -lt $end) {
        if (-not $sent -and (Test-Path $flag)) {
            $form.Activate()
            $sent = $true
        }
        [Windows.Forms.Application]::DoEvents(); Start-Sleep -Milliseconds 50
    }
}
try {
    Start-Sleep 3
    $env:PROBE_OPACITY = "$Opacity"
    if ($ScrollAtMs -ge 0) { $env:PROBE_SCROLL_AT_MS = "$ScrollAtMs" }
    $probe = Start-Process (Join-Path $root 'target/debug/examples/blur_probe.exe') -PassThru
    Start-Sleep $Delay
    if ($Inactive) {
        # The backdrop takes the focus; the probe stays visible above it, unfocused.
        Add-Type -Namespace Native -Name Top -MemberDefinition '[DllImport("user32.dll")] public static extern bool SetWindowPos(System.IntPtr h, System.IntPtr after, int x, int y, int cx, int cy, uint flags);'
        New-Item $flag -ItemType File | Out-Null
        Start-Sleep 1
        [Native.Top]::SetWindowPos($probe.MainWindowHandle, [IntPtr]-1, 0, 0, 0, 0, 0x13) | Out-Null
        Start-Sleep 1
    }
    $bounds = [Windows.Forms.Screen]::PrimaryScreen.Bounds
    $shot = New-Object Drawing.Bitmap $bounds.Width, $bounds.Height
    [Drawing.Graphics]::FromImage($shot).CopyFromScreen($bounds.Location, [Drawing.Point]::Empty, $bounds.Size)
    $shot.Save([IO.Path]::GetFullPath($Output))
    if (-not $probe.HasExited) { $probe.Kill() }
} finally {
    Stop-Job $backdrop -ErrorAction SilentlyContinue
    Remove-Job $backdrop -Force -ErrorAction SilentlyContinue
    Remove-Item Env:PROBE_OPACITY -ErrorAction SilentlyContinue
    Remove-Item Env:PROBE_SCROLL_AT_MS -ErrorAction SilentlyContinue
    Remove-Item $flag -ErrorAction SilentlyContinue
}
