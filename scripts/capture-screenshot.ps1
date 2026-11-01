# Windows only: a picture of the FerroLibri window on a made-up library. Three ways to use it:
#
#   1. All at once: starts the program on the made-up library, sends -Keys, saves the picture, closes it.
#        pwsh -File scripts/capture-screenshot.ps1 -Keys RIGHT,RIGHT,RIGHT,RIGHT,RIGHT
#   2. By hand: -Launch starts the program on the library and leaves it open. Size the window, select a
#      book, open whatever you like; then -Attach saves a picture of that window and leaves it open.
#        pwsh -File scripts/capture-screenshot.ps1 -Launch
#        pwsh -File scripts/capture-screenshot.ps1 -Attach
#      -Attach takes the newest running ferrolibri.exe, or the one given with -ProcessId. It needs no
#      library and also works on a program you started yourself.
#
# The library comes from scripts/make-screenshot-library.py (needs Python with Pillow) and lives in
# target/screenshot-data. FERROLIBRI_UI_DATA_DIR points the program there, so the real profile is not
# read or changed. Build the program first (cargo build --release) or pass -Exe.
# -Regenerate makes the library again; -RealCovers fetches the covers from Open Library (see the
# library script) instead of drawing them. -Keys is a list of keys sent to the window before the
# picture, for example to move the cursor to a book: RIGHT,RIGHT,DOWN. The window is captured from the
# screen, so keep it uncovered while the script runs.
param(
    [string]$Output = 'assets/screenshot.png',
    [string]$Exe = 'target/release/ferrolibri.exe',
    [string[]]$Keys = @(),
    [int]$SettleSeconds = 6,
    [switch]$Launch,
    [switch]$Attach,
    [int]$ProcessId = 0,
    [switch]$KeepOpen,
    [switch]$Regenerate,
    [switch]$RealCovers
)
$ErrorActionPreference = 'Stop'
# With -File a list arrives as one string: -Keys RIGHT,RIGHT
$Keys = @($Keys | ForEach-Object { $_ -split ',' } | Where-Object { $_ })
$root = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
Set-Location $root
$data = Join-Path $root 'target/screenshot-data'

Add-Type -AssemblyName System.Drawing
Add-Type @'
using System;
using System.Runtime.InteropServices;
public static class Win {
    [StructLayout(LayoutKind.Sequential)] public struct RECT { public int Left, Top, Right, Bottom; }
    [DllImport("user32.dll")] public static extern bool SetProcessDPIAware();
    [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr h);
    [DllImport("user32.dll")] public static extern bool ShowWindow(IntPtr h, int cmd);
    [DllImport("user32.dll")] public static extern void keybd_event(byte vk, byte scan, uint flags, UIntPtr extra);
    [DllImport("dwmapi.dll")] public static extern int DwmGetWindowAttribute(IntPtr h, int attr, out RECT r, int size);
    delegate bool EnumProc(IntPtr h, IntPtr p);
    [DllImport("user32.dll")] static extern bool EnumWindows(EnumProc f, IntPtr p);
    [DllImport("user32.dll")] static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
    [DllImport("user32.dll")] static extern bool IsWindowVisible(IntPtr h);
    [DllImport("user32.dll")] static extern bool GetWindowRect(IntPtr h, out RECT r);
    // The largest visible top-level window of a process: the program may own small helper windows.
    public static IntPtr MainWindow(uint pid) {
        IntPtr best = IntPtr.Zero; long area = 0;
        EnumWindows((h, p) => {
            uint owner; GetWindowThreadProcessId(h, out owner);
            RECT r;
            if (owner == pid && IsWindowVisible(h) && GetWindowRect(h, out r)) {
                long a = (long)(r.Right - r.Left) * (r.Bottom - r.Top);
                if (a > area) { area = a; best = h; }
            }
            return true;
        }, IntPtr.Zero);
        return best;
    }
}
'@
[Win]::SetProcessDPIAware() | Out-Null

function Get-Frame($handle) {
    $rect = New-Object Win+RECT
    # 9 = DWMWA_EXTENDED_FRAME_BOUNDS: the visible window without the invisible resize border.
    [Win]::DwmGetWindowAttribute($handle, 9, [ref]$rect, [Runtime.InteropServices.Marshal]::SizeOf($rect)) | Out-Null
    $rect
}

# Waits for the real window of a process: tiny helper windows come first.
function Wait-Window($processId) {
    $deadline = (Get-Date).AddSeconds(30)
    while ((Get-Date) -lt $deadline) {
        Start-Sleep -Milliseconds 300
        $handle = [Win]::MainWindow([uint32]$processId)
        if ($handle -ne [IntPtr]::Zero) {
            $frame = Get-Frame $handle
            if (($frame.Right - $frame.Left) -ge 600 -and ($frame.Bottom - $frame.Top) -ge 400) { return $handle }
        }
    }
    throw 'The window did not appear'
}

function Ensure-Library {
    if ($Regenerate -or -not (Test-Path (Join-Path $data 'config.toml'))) {
        if (Test-Path $data) { Remove-Item -Recurse -Force $data }
        $extra = @(); if ($RealCovers) { $extra += '--real-covers' }
        & python (Join-Path $PSScriptRoot 'make-screenshot-library.py') $data @extra
        if ($LASTEXITCODE -ne 0) { throw 'Could not make the library' }
    }
}

function Start-OnLibrary {
    $previous = $env:FERROLIBRI_UI_DATA_DIR
    $env:FERROLIBRI_UI_DATA_DIR = $data
    try { Start-Process -FilePath (Join-Path $root $Exe) -PassThru } finally { $env:FERROLIBRI_UI_DATA_DIR = $previous }
}

function Save-Window($handle) {
    $frame = Get-Frame $handle
    # One pixel is cut off all round: the window's own border blends with what is behind it.
    $width = $frame.Right - $frame.Left - 2
    $height = $frame.Bottom - $frame.Top - 2
    $bitmap = New-Object Drawing.Bitmap $width, $height
    $graphics = [Drawing.Graphics]::FromImage($bitmap)
    $graphics.CopyFromScreen($frame.Left + 1, $frame.Top + 1, 0, 0, $bitmap.Size)
    $target = if ([IO.Path]::IsPathRooted($Output)) { $Output } else { Join-Path $root $Output }
    New-Item -ItemType Directory -Force (Split-Path $target) | Out-Null
    $bitmap.Save($target, [Drawing.Imaging.ImageFormat]::Png)
    $graphics.Dispose(); $bitmap.Dispose()
    "Saved ${width}x${height} to $target"
}

function Send-Keys($handle) {
    $names = @{ RIGHT = 0x27; LEFT = 0x25; DOWN = 0x28; UP = 0x26; ENTER = 0x0D; ESC = 0x1B; TAB = 0x09 }
    [Win]::SetForegroundWindow($handle) | Out-Null
    Start-Sleep -Seconds 1
    foreach ($key in $Keys) {
        $code = $names[$key.ToUpper()]
        if ($null -eq $code) { $code = [byte][char]$key.ToUpper() }
        [Win]::keybd_event([byte]$code, 0, 0, [UIntPtr]::Zero)
        [Win]::keybd_event([byte]$code, 0, 2, [UIntPtr]::Zero)
        Start-Sleep -Milliseconds 400
    }
}

if ($Launch) {
    Ensure-Library
    $process = Start-OnLibrary
    "Started ferrolibri (process $($process.Id)) on $data. Set it up, then run this script with -Attach."
    return
}

if ($Attach) {
    if ($ProcessId -gt 0) {
        $process = Get-Process -Id $ProcessId
    } else {
        $process = Get-Process -Name ([IO.Path]::GetFileNameWithoutExtension($Exe)) -ErrorAction SilentlyContinue |
            Sort-Object StartTime -Descending | Select-Object -First 1
    }
    if ($null -eq $process) { throw 'No running ferrolibri found; start it with -Launch' }
    $handle = Wait-Window $process.Id
    [Win]::ShowWindow($handle, 9) | Out-Null
    [Win]::SetForegroundWindow($handle) | Out-Null
    Start-Sleep -Seconds 1
    if ($Keys.Count -gt 0) { Send-Keys $handle }
    Start-Sleep -Seconds 2
    "Window of process $($process.Id)"
    Save-Window $handle
    return
}

Ensure-Library
$process = Start-OnLibrary
try {
    $handle = Wait-Window $process.Id
    [Win]::ShowWindow($handle, 9) | Out-Null
    Start-Sleep -Seconds $SettleSeconds   # the library is scanned and the covers are decoded
    Send-Keys $handle
    Start-Sleep -Seconds 2
    Save-Window $handle
} finally {
    if (-not $KeepOpen) {
        $null = $process.CloseMainWindow()
        if (-not $process.WaitForExit(5000)) { $process.Kill() }
    }
}
