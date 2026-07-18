param(
  [Parameter(Mandatory = $true)]
  [int]$ProcessId
)

$ErrorActionPreference = "Stop"

Add-Type @'
using System;
using System.Collections.Generic;
using System.Runtime.InteropServices;
using System.Text;

public static class OpenWhisperWindowProbe {
  [StructLayout(LayoutKind.Sequential)]
  public struct RECT { public int Left, Top, Right, Bottom; }

  [StructLayout(LayoutKind.Sequential)]
  public struct MONITORINFO {
    public int Size;
    public RECT Monitor;
    public RECT Work;
    public uint Flags;
  }

  public delegate bool MonitorCallback(IntPtr monitor, IntPtr hdc, ref RECT rect, IntPtr data);
  public delegate bool WindowCallback(IntPtr window, IntPtr data);

  [DllImport("user32.dll")]
  public static extern bool SetProcessDpiAwarenessContext(IntPtr context);
  [DllImport("user32.dll")]
  public static extern bool EnumDisplayMonitors(IntPtr hdc, IntPtr clip, MonitorCallback callback, IntPtr data);
  [DllImport("user32.dll")]
  public static extern bool GetMonitorInfo(IntPtr monitor, ref MONITORINFO info);
  [DllImport("user32.dll")]
  public static extern bool GetWindowRect(IntPtr window, out RECT rect);
  [DllImport("user32.dll")]
  public static extern bool SetCursorPos(int x, int y);
  [DllImport("user32.dll")]
  public static extern bool SetForegroundWindow(IntPtr window);
  [DllImport("user32.dll")]
  public static extern bool EnumWindows(WindowCallback callback, IntPtr data);
  [DllImport("user32.dll")]
  public static extern uint GetWindowThreadProcessId(IntPtr window, out uint processId);
  [DllImport("user32.dll", CharSet = CharSet.Unicode)]
  public static extern int GetWindowText(IntPtr window, StringBuilder text, int count);

  public static List<MONITORINFO> Monitors() {
    var monitors = new List<MONITORINFO>();
    EnumDisplayMonitors(IntPtr.Zero, IntPtr.Zero, delegate(IntPtr monitor, IntPtr hdc, ref RECT rect, IntPtr data) {
      var info = new MONITORINFO { Size = Marshal.SizeOf<MONITORINFO>() };
      if (GetMonitorInfo(monitor, ref info)) monitors.Add(info);
      return true;
    }, IntPtr.Zero);
    return monitors;
  }

  public static IntPtr FindMainWindow(uint processId) {
    IntPtr result = IntPtr.Zero;
    EnumWindows(delegate(IntPtr window, IntPtr data) {
      uint owner;
      GetWindowThreadProcessId(window, out owner);
      if (owner != processId) return true;
      var title = new StringBuilder(128);
      GetWindowText(window, title, title.Capacity);
      if (title.ToString() == "OpenWhisper") {
        result = window;
        return false;
      }
      return true;
    }, IntPtr.Zero);
    return result;
  }
}
'@

# PER_MONITOR_AWARE_V2. Call before UI Automation or window-coordinate APIs.
[void][OpenWhisperWindowProbe]::SetProcessDpiAwarenessContext([IntPtr](-4))

$process = Get-Process -Id $ProcessId
$window = [OpenWhisperWindowProbe]::FindMainWindow([uint32]$ProcessId)
if ($window -eq [IntPtr]::Zero) {
  throw "Process $ProcessId has no main window"
}

$monitors = [OpenWhisperWindowProbe]::Monitors()
if ($monitors.Count -lt 2) {
  throw "This regression requires at least two monitors"
}

Write-Output "Physical monitor work areas:"
for ($i = 0; $i -lt $monitors.Count; $i++) {
  $work = $monitors[$i].Work
  Write-Output "  [$i] L=$($work.Left) T=$($work.Top) R=$($work.Right) B=$($work.Bottom)"
}

$pair = $null
for ($aboveIndex = 0; $aboveIndex -lt $monitors.Count; $aboveIndex++) {
  for ($belowIndex = 0; $belowIndex -lt $monitors.Count; $belowIndex++) {
    if ($aboveIndex -eq $belowIndex) { continue }
    $aboveInfo = $monitors[$aboveIndex]
    $belowInfo = $monitors[$belowIndex]
    $above = $aboveInfo.Monitor
    $below = $belowInfo.Monitor
    $overlapLeft = [Math]::Max($above.Left, $below.Left)
    $overlapRight = [Math]::Min($above.Right, $below.Right)
    if ($overlapLeft -lt $overlapRight -and $above.Bottom -le $below.Top) {
      $pair = [pscustomobject]@{
        Above = $aboveInfo
        Below = $belowInfo
        Boundary = $below.Top
        X = [int](($overlapLeft + $overlapRight) / 2)
      }
      break
    }
  }
  if ($pair) { break }
}
if (-not $pair) {
  throw "No vertically stacked monitor pair with a shared horizontal span was found"
}

function Get-WindowRectangle {
  $rect = New-Object OpenWhisperWindowProbe+RECT
  if (-not [OpenWhisperWindowProbe]::GetWindowRect($window, [ref]$rect)) {
    throw "GetWindowRect failed"
  }
  return $rect
}

function Test-MonitorAnchor([string]$Label, $MonitorInfo, [int]$CursorX, [int]$CursorY, [bool]$AllowExpandedOffset = $false) {
  [void][OpenWhisperWindowProbe]::SetCursorPos($CursorX, $CursorY)
  Start-Sleep -Milliseconds 250
  $samples = @()
  for ($sample = 0; $sample -lt 6; $sample++) {
    Start-Sleep -Milliseconds 30
    $rect = Get-WindowRectangle
    $samples += [pscustomobject]@{
      Mode = $Label
      Left = $rect.Left
      Top = $rect.Top
      Right = $rect.Right
      Bottom = $rect.Bottom
      Width = $rect.Right - $rect.Left
      Height = $rect.Bottom - $rect.Top
    }
  }
  Write-Host ($samples | Format-Table -AutoSize | Out-String)

  $positions = $samples | ForEach-Object { "$($_.Left),$($_.Top),$($_.Right),$($_.Bottom)" } | Select-Object -Unique
  if ($positions.Count -ne 1) {
    throw "$Label window jittered after settling: $($positions -join '; ')"
  }

  $settled = $samples[-1]
  $expectedCenter = ($MonitorInfo.Work.Left + $MonitorInfo.Work.Right) / 2
  $expectedBottom = $MonitorInfo.Work.Bottom - 10
  $actualCenter = ($settled.Left + $settled.Right) / 2
  $isPrimary = ($MonitorInfo.Flags -band 1) -ne 0
  $baseMinimum = if ($isPrimary) { $expectedBottom } else { $expectedBottom + 8 }
  $baseMaximum = if ($isPrimary) { $expectedBottom } else { $MonitorInfo.Monitor.Bottom - 10 }
  $bottomMatches = if ($AllowExpandedOffset) {
    $settled.Bottom -ge ($baseMinimum + 1) -and $settled.Bottom -le ($baseMaximum + 20)
  } else {
    $settled.Bottom -ge ($baseMinimum - 2) -and $settled.Bottom -le ($baseMaximum + 2)
  }
  if ([Math]::Abs($actualCenter - $expectedCenter) -gt 2 -or -not $bottomMatches) {
    throw "$Label anchor mismatch: expected center/bottom=$expectedCenter/$expectedBottom, actual=$actualCenter/$($settled.Bottom)"
  }

  return $settled
}

$belowRect = $pair.Below.Monitor
$aboveRect = $pair.Above.Monitor
$belowY = [int](($belowRect.Top + $belowRect.Bottom) / 2)
$aboveY = [int](($aboveRect.Top + $aboveRect.Bottom) / 2)

$collapsedBelow = Test-MonitorAnchor "collapsed-below" $pair.Below $pair.X $belowY
$collapsedAbove = Test-MonitorAnchor "collapsed-above" $pair.Above $pair.X $aboveY

# Bounce across the edge faster than the debounce interval. The overlay must
# remain locked instead of following every noisy boundary sample.
for ($bounce = 0; $bounce -lt 4; $bounce++) {
  [void][OpenWhisperWindowProbe]::SetCursorPos($pair.X, $pair.Boundary + 1)
  Start-Sleep -Milliseconds 35
  [void][OpenWhisperWindowProbe]::SetCursorPos($pair.X, $pair.Boundary - 1)
  Start-Sleep -Milliseconds 35
}
$afterBounce = Get-WindowRectangle
if ($afterBounce.Left -ne $collapsedAbove.Left -or
    $afterBounce.Top -ne $collapsedAbove.Top -or
    $afterBounce.Right -ne $collapsedAbove.Right -or
    $afterBounce.Bottom -ne $collapsedAbove.Bottom) {
  throw "Overlay moved during rapid monitor-boundary noise"
}

[void][OpenWhisperWindowProbe]::SetForegroundWindow($window)
$panelOpener = Join-Path $PSScriptRoot "open-settings-panel.mjs"
& node $panelOpener
if ($LASTEXITCODE -ne 0) {
  throw "Failed to open the settings panel through WebView2"
}
Start-Sleep -Milliseconds 750
$expanded = Get-WindowRectangle
if (($expanded.Bottom - $expanded.Top) -lt 150) {
  throw "Settings panel did not expand; measured height was $($expanded.Bottom - $expanded.Top)"
}
$collapsedCenter = ($collapsedAbove.Left + $collapsedAbove.Right) / 2
$expandedCenter = ($expanded.Left + $expanded.Right) / 2
if ([Math]::Abs($collapsedCenter - $expandedCenter) -gt 2 -or
    ($expanded.Bottom - $collapsedAbove.Bottom) -lt 1 -or
    ($expanded.Bottom - $collapsedAbove.Bottom) -gt 20) {
  throw "Expansion moved the anchor: collapsed center/bottom=$collapsedCenter/$($collapsedAbove.Bottom), settings center/bottom=$expandedCenter/$($expanded.Bottom)"
}

Test-MonitorAnchor "settings-below" $pair.Below $pair.X $belowY $true | Out-Null
Test-MonitorAnchor "settings-above" $pair.Above $pair.X $aboveY $true | Out-Null
Write-Output "PASS: overlay followed the cursor monitor, preserved its anchor, and did not jitter at the boundary."
