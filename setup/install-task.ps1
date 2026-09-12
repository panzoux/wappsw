<#
One-time setup: registers a Task Scheduler task that launches wappsw.exe
at logon with the highest available privileges (elevated).

Why elevated: the app's global keyboard hook (WH_KEYBOARD_LL) can't see
keystrokes sent to a higher-integrity (elevated) window unless wappsw
itself is also elevated -- otherwise the hotkey silently stops working
whenever an elevated window (e.g. an elevated terminal) has focus.

Why Task Scheduler instead of the Startup folder: a scheduled task created
this way by an admin user launches elevated at logon WITHOUT a UAC prompt
every time, which a Startup-folder shortcut cannot do.

Run this script once from an elevated PowerShell prompt:
    powershell -ExecutionPolicy Bypass -File setup\install-task.ps1
#>

$ErrorActionPreference = "Stop"

$exePath = Join-Path $PSScriptRoot "..\target\release\wappsw.exe"
if (-not (Test-Path $exePath)) {
    Write-Error "Release build not found at $exePath -- run 'cargo build --release' first."
    exit 1
}
$exePath = (Resolve-Path $exePath).Path

$taskName = "wappsw"
$action = New-ScheduledTaskAction -Execute $exePath
$trigger = New-ScheduledTaskTrigger -AtLogOn
$principal = New-ScheduledTaskPrincipal -UserId $env:USERNAME -RunLevel Highest -LogonType Interactive
$settings = New-ScheduledTaskSettingsSet -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries -StartWhenAvailable

Register-ScheduledTask -TaskName $taskName -Action $action -Trigger $trigger `
    -Principal $principal -Settings $settings -Force | Out-Null

Write-Host "Task '$taskName' registered: $exePath will launch elevated at logon."
Write-Host "To start it right now without logging off/on: Start-ScheduledTask -TaskName $taskName"
Write-Host "To remove it later: Unregister-ScheduledTask -TaskName $taskName -Confirm:`$false"
