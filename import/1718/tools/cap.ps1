# Capture every visible top-level window of the game processes running from -Root (path-checked), with PrintWindow:
# the windows' own content, never the screen (other apps never end up in a capture).
param([Parameter(Mandatory=$true)][string]$Root, [Parameter(Mandatory=$true)][string]$OutDir)
Add-Type -AssemblyName System.Drawing
Add-Type @"
using System;
using System.Runtime.InteropServices;
public class W {
  [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
  [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr h);
  [DllImport("user32.dll")] public static extern bool ShowWindow(IntPtr h, int cmd);
  [DllImport("user32.dll")] public static extern bool IsIconic(IntPtr h);
  [DllImport("user32.dll")] public static extern bool PrintWindow(IntPtr h, IntPtr dc, uint flags);
  [StructLayout(LayoutKind.Sequential)] public struct RECT { public int L, T, R, B; }
}
"@
New-Item -ItemType Directory -Force $OutDir | Out-Null
Get-Process client_Release, control_Release -ErrorAction SilentlyContinue | Where-Object { $_.Path -like ($Root.TrimEnd('\') + '\*') -and $_.MainWindowHandle -ne 0 } | ForEach-Object {
  $h = $_.MainWindowHandle
  if ([W]::IsIconic($h)) { [void][W]::ShowWindow($h, 9) }
  $r = New-Object W+RECT; [void][W]::GetWindowRect($h, [ref]$r)
  $w = $r.R - $r.L; $ht = $r.B - $r.T
  if ($w -gt 0 -and $ht -gt 0) {
    $b = New-Object Drawing.Bitmap $w, $ht; $g = [Drawing.Graphics]::FromImage($b)
    $dc = $g.GetHdc(); [void][W]::PrintWindow($h, $dc, 2); $g.ReleaseHdc($dc)   # PW_RENDERFULLCONTENT: the window only, never the screen
    $seat = if ($_.Path -like '*\seat1\*') { 'seat1' } elseif ($_.ProcessName -eq 'control_Release') { 'control' } else { 'projector' }
    $f = Join-Path $OutDir ("{0}-{1}.png" -f $seat, $_.Id)
    $b.Save($f, [Drawing.Imaging.ImageFormat]::Png); $g.Dispose(); $b.Dispose()
    "{0} pid {1} '{2}' {3}x{4} -> {5}" -f $seat, $_.Id, $_.MainWindowTitle, $w, $ht, $f
  }
}
