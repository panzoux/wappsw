<#
.SYNOPSIS
    End-to-end keyboard test for the wappsw popup.

.DESCRIPTION
    Drives a real wappsw.exe with injected keystrokes (keybd_event) and reads
    the search box back with a cross-process WM_GETTEXT, so every assertion is
    about what the edit control actually contains -- not merely about whether
    the process is still alive.

    That distinction is why this file exists. Ctrl+Q (roadmap item 1) widened
    the WM_CHAR swallow to the whole C0 range, which silently ate backspace
    (0x08). Everything checked at the time -- it compiles, the popup appears,
    Ctrl+Q exits with code 0 -- stayed green throughout. Only reading the edit
    control's text catches that class of bug.

    NOT wired into `cargo test`, deliberately. This needs an interactive
    desktop, installs a global low-level keyboard hook, injects system-wide
    input, and stops any running wappsw.exe. None of that belongs in a plain
    `cargo test` run.

.PARAMETER Exe
    The wappsw.exe to test. Defaults to the repo's release build.

.PARAMETER Build
    Run `cargo build --release` first.

.EXAMPLE
    powershell -ExecutionPolicy Bypass -File tests\popup-keyboard.ps1 -Build

.NOTES
    While this runs it owns the keyboard for a few seconds -- do not type.
    It stops any running wappsw.exe, and reads the real
    %APPDATA%\wappsw\config.ini to learn which hotkey to press, so it works
    whatever `hotkey=` is set to. Lock-key state (Caps/Scroll) is restored
    before it returns.

    Exit code 0 = all checks passed, 1 = at least one failed.
#>

[CmdletBinding()]
param(
    [string]$Exe = (Join-Path $PSScriptRoot '..\target\release\wappsw.exe'),
    [switch]$Build
)

$ErrorActionPreference = 'Stop'

Add-Type @"
using System;
using System.Text;
using System.Runtime.InteropServices;
public static class Win {
  [DllImport("user32.dll")] public static extern void keybd_event(byte bVk, byte bScan, uint dwFlags, UIntPtr dwExtraInfo);
  [DllImport("user32.dll")] public static extern short GetKeyState(int vk);
  [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
  [DllImport("user32.dll")] public static extern bool EnumWindows(EnumProc cb, IntPtr p);
  [DllImport("user32.dll")] public static extern bool EnumChildWindows(IntPtr parent, EnumProc cb, IntPtr p);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetClassNameW(IntPtr h, StringBuilder s, int n);
  [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern IntPtr SendMessageW(IntPtr h, uint msg, IntPtr wp, StringBuilder lp);
  public delegate bool EnumProc(IntPtr h, IntPtr p);

  // The popup is WS_POPUP|WS_EX_TOOLWINDOW with no taskbar entry, so it has
  // to be found by class name within the target process.
  public static IntPtr FindPopup(uint targetPid) {
    IntPtr found = IntPtr.Zero;
    EnumWindows(delegate(IntPtr h, IntPtr p) {
      uint pid; GetWindowThreadProcessId(h, out pid);
      if (pid != targetPid) return true;
      var sb = new StringBuilder(256); GetClassNameW(h, sb, 256);
      if (sb.ToString() == "wappsw_popup") { found = h; return false; }
      return true;
    }, IntPtr.Zero);
    return found;
  }

  public static IntPtr FindEdit(IntPtr popup) {
    IntPtr found = IntPtr.Zero;
    EnumChildWindows(popup, delegate(IntPtr h, IntPtr p) {
      var sb = new StringBuilder(256); GetClassNameW(h, sb, 256);
      if (String.Equals(sb.ToString(), "edit", StringComparison.OrdinalIgnoreCase)) { found = h; return false; }
      return true;
    }, IntPtr.Zero);
    return found;
  }

  // WM_GETTEXT is one of the messages USER32 marshals across process
  // boundaries, so this reads the other process's edit control directly.
  public static string GetText(IntPtr h) {
    var sb = new StringBuilder(512);
    SendMessageW(h, 0x000D, (IntPtr)512, sb);
    return sb.ToString();
  }
}
"@

# --- key plumbing -----------------------------------------------------------

$KEYEVENTF_KEYUP = 2

function Send-Key([byte]$Vk, [byte]$Scan, [switch]$Up) {
    $flags = 0
    if ($Up) { $flags = $KEYEVENTF_KEYUP }
    [Win]::keybd_event($Vk, $Scan, $flags, [UIntPtr]::Zero)
    Start-Sleep -Milliseconds 45
}

function Send-Tap([byte]$Vk, [byte]$Scan) {
    Send-Key $Vk $Scan
    Send-Key $Vk $Scan -Up
}

# Hold Ctrl across a single tap.
function Send-Ctrl([byte]$Vk, [byte]$Scan) {
    Send-Key 0x11 0x1D
    Send-Tap $Vk $Scan
    Send-Key 0x11 0x1D -Up
}

function Test-Toggled([int]$Vk) { ([Win]::GetKeyState($Vk) -band 1) -ne 0 }

# --- assertions -------------------------------------------------------------

$script:Failures = 0

function Assert-Equal([string]$Label, $Actual, $Expected) {
    if ($Actual -eq $Expected) {
        Write-Host ("  PASS  {0} -> '{1}'" -f $Label, $Actual) -ForegroundColor Green
    } else {
        $script:Failures++
        Write-Host ("  FAIL  {0} -> got '{1}', expected '{2}'" -f $Label, $Actual, $Expected) -ForegroundColor Red
    }
}

function Assert-True([string]$Label, [bool]$Condition, [string]$Detail = '') {
    if ($Condition) {
        Write-Host ("  PASS  {0} {1}" -f $Label, $Detail) -ForegroundColor Green
    } else {
        $script:Failures++
        Write-Host ("  FAIL  {0} {1}" -f $Label, $Detail) -ForegroundColor Red
    }
}

# --- which key opens the popup ----------------------------------------------
# Mirrors config::key_name_to_vk: same three names, same aliases, same
# normalization (case, '_', '-', space). The scan codes must match the
# constants in config.rs, because the hook matches on scan code, not VK.

function Get-ConfiguredHotkey {
    $name = 'CapsLock'
    $ini = Join-Path $env:APPDATA 'wappsw\config.ini'
    if (Test-Path $ini) {
        foreach ($line in Get-Content $ini) {
            $line = $line.Trim()
            if ($line -eq '' -or $line.StartsWith(';') -or $line.StartsWith('#')) { continue }
            $pair = $line -split '=', 2
            if ($pair.Count -eq 2 -and $pair[0].Trim() -ieq 'hotkey') { $name = $pair[1].Trim() }
        }
    }
    $norm = ($name -replace '[_\- ]', '').ToLowerInvariant()
    if ($norm -eq 'capslock' -or $norm -eq 'caps') {
        return @{ Name = $name; Vk = 0x14; Scan = 0x3A; Toggle = 0x14 }
    }
    if ($norm -eq 'scrolllock' -or $norm -eq 'scroll') {
        return @{ Name = $name; Vk = 0x91; Scan = 0x46; Toggle = 0x91 }
    }
    if ($norm -eq 'insert' -or $norm -eq 'ins') {
        # Insert is not a lock key, so the hook-uninstall check is skipped.
        return @{ Name = $name; Vk = 0x2D; Scan = 0x52; Toggle = $null }
    }
    Write-Warning "config.ini hotkey '$name' is not recognized; wappsw falls back to CapsLock, so this test will too."
    return @{ Name = 'CapsLock (fallback)'; Vk = 0x14; Scan = 0x3A; Toggle = 0x14 }
}

# --- run --------------------------------------------------------------------

if ($Build) {
    Write-Host 'Building...' -ForegroundColor Cyan
    & cargo build --release --manifest-path (Join-Path $PSScriptRoot '..\Cargo.toml')
    if ($LASTEXITCODE -ne 0) { throw 'cargo build failed' }
}

$Exe = (Resolve-Path $Exe).Path
$hotkey = Get-ConfiguredHotkey
Write-Host "exe:    $Exe"
Write-Host ("hotkey: {0} (vk=0x{1:X2} scan=0x{2:X2})" -f $hotkey.Name, $hotkey.Vk, $hotkey.Scan)
Write-Host 'Do not type while this runs.' -ForegroundColor Yellow

$proc = $null
try {
    Get-Process wappsw -ErrorAction SilentlyContinue | Stop-Process -Force
    Start-Sleep -Milliseconds 400

    Write-Host "`n-- startup --"
    $proc = Start-Process -FilePath $Exe -PassThru
    Start-Sleep -Milliseconds 1500
    $proc.Refresh()
    Assert-True 'app stays running after launch' (-not $proc.HasExited) "(pid $($proc.Id))"
    if ($proc.HasExited) { throw "wappsw exited immediately (code $($proc.ExitCode)) -- is another instance running?" }

    $popup = [Win]::FindPopup([uint32]$proc.Id)
    Assert-True 'popup window exists at startup' ($popup -ne [IntPtr]::Zero) "(hwnd $popup)"
    if ($popup -eq [IntPtr]::Zero) { throw 'no popup window -- cannot continue' }
    Assert-True 'popup is hidden until the hotkey' (-not [Win]::IsWindowVisible($popup))

    Write-Host "`n-- the hotkey opens the popup --"
    $lockBefore = $null
    if ($hotkey.Toggle) { $lockBefore = Test-Toggled $hotkey.Toggle }
    Send-Tap $hotkey.Vk $hotkey.Scan
    Start-Sleep -Milliseconds 900
    Assert-True 'popup is visible after the hotkey' ([Win]::IsWindowVisible($popup))
    if ($null -ne $lockBefore) {
        # The hook swallows both down and up, so the real lock state must not
        # have moved -- that is the whole point of never passing the key on.
        Assert-True 'hotkey did not toggle the real lock state' ((Test-Toggled $hotkey.Toggle) -eq $lockBefore)
    }

    $edit = [Win]::FindEdit($popup)
    Assert-True 'search box found' ($edit -ne [IntPtr]::Zero) "(hwnd $edit)"
    if ($edit -eq [IntPtr]::Zero) { throw 'no edit control -- cannot continue' }

    Write-Host "`n-- typing --"
    Send-Tap 0x41 0x1E; Send-Tap 0x42 0x30; Send-Tap 0x43 0x2E   # a b c
    Assert-Equal "typing 'abc'" ([Win]::GetText($edit)) 'abc'

    Write-Host "`n-- backspace (0x08 must NOT be swallowed) --"
    Send-Tap 0x08 0x0E
    Assert-Equal 'backspace deletes one character' ([Win]::GetText($edit)) 'ab'
    Send-Tap 0x08 0x0E; Send-Tap 0x08 0x0E
    Assert-Equal 'backspace clears the box' ([Win]::GetText($edit)) ''
    Send-Tap 0x08 0x0E
    Assert-Equal 'backspace on an empty box is a no-op' ([Win]::GetText($edit)) ''

    Write-Host "`n-- control characters are swallowed, not inserted --"
    Send-Tap 0x41 0x1E                                            # a
    Send-Ctrl 0x5A 0x2C                                           # Ctrl+Z
    Assert-Equal 'Ctrl+Z inserts nothing' ([Win]::GetText($edit)) 'a'
    Send-Ctrl 0x4B 0x25                                           # Ctrl+K
    Assert-Equal 'Ctrl+K inserts nothing' ([Win]::GetText($edit)) 'a'
    Send-Ctrl 0x55 0x16                                           # Ctrl+U
    Assert-Equal 'Ctrl+U inserts nothing' ([Win]::GetText($edit)) 'a'
    Send-Tap 0x09 0x0F                                            # Tab
    Assert-Equal 'Tab inserts nothing' ([Win]::GetText($edit)) 'a'
    Send-Tap 0x08 0x0E
    Assert-Equal 'backspace still works after Ctrl chords' ([Win]::GetText($edit)) ''

    Write-Host "`n-- Ctrl+H is the same 0x08, so it is backspace too --"
    Send-Tap 0x41 0x1E; Send-Tap 0x42 0x30
    Send-Ctrl 0x48 0x23
    Assert-Equal 'Ctrl+H deletes one character' ([Win]::GetText($edit)) 'a'

    Write-Host "`n-- bare Q types, Esc only hides --"
    Send-Tap 0x51 0x10
    Assert-Equal 'bare Q types a character' ([Win]::GetText($edit)) 'aq'
    $proc.Refresh()
    Assert-True 'bare Q does not quit' (-not $proc.HasExited)
    Send-Tap 0x1B 0x01
    Start-Sleep -Milliseconds 600
    Assert-True 'Esc hides the popup' (-not [Win]::IsWindowVisible($popup))
    $proc.Refresh()
    Assert-True 'Esc does not quit' (-not $proc.HasExited)

    Write-Host "`n-- Ctrl+Q quits cleanly --"
    Send-Tap $hotkey.Vk $hotkey.Scan
    Start-Sleep -Milliseconds 800
    Assert-True 'popup reopens' ([Win]::IsWindowVisible($popup))
    Send-Ctrl 0x51 0x10
    Start-Sleep -Milliseconds 1500
    $proc.Refresh()
    Assert-True 'Ctrl+Q exits the process' $proc.HasExited
    if ($proc.HasExited) {
        # Exit code 0 means it left through main()'s message loop rather than
        # being killed, which is what guarantees the uninstall path ran.
        Assert-Equal 'exit code (0 = clean message-loop exit)' $proc.ExitCode 0
    }

    if ($hotkey.Toggle) {
        # The strongest proof available that UnhookWindowsHookEx actually ran:
        # the hotkey reaches the system again and moves the lock state.
        $before = Test-Toggled $hotkey.Toggle
        Send-Tap $hotkey.Vk $hotkey.Scan
        Start-Sleep -Milliseconds 300
        $after = Test-Toggled $hotkey.Toggle
        Assert-True 'keyboard hook was uninstalled (hotkey toggles again)' ($after -ne $before)
        if ($after -ne $before) { Send-Tap $hotkey.Vk $hotkey.Scan }   # restore
    }
}
finally {
    Get-Process wappsw -ErrorAction SilentlyContinue | Stop-Process -Force
}

Write-Host ''
if ($script:Failures -eq 0) {
    Write-Host 'RESULT: all checks passed' -ForegroundColor Green
    exit 0
} else {
    Write-Host "RESULT: $($script:Failures) check(s) failed" -ForegroundColor Red
    exit 1
}
