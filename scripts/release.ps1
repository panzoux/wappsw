<#
.SYNOPSIS
    Builds, packages and (optionally) publishes a wappsw release.

.DESCRIPTION
    Writes down the recipe v0.1.0 was assembled with by hand in /dist:

      dist\wappsw-v<version>-x86.zip      (i686-pc-windows-msvc)
      dist\wappsw-v<version>-x86_64.zip   (x86_64-pc-windows-msvc)

    Each zip is flat (no top-level folder) and holds wappsw.exe, README.md,
    LICENSE, setup\install-task.ps1 and the assets\LICENSE* files. Those last
    ones are a redistribution requirement of the Migemo dictionary embedded
    in the exe, and were missing from v0.1.0. The version comes from
    Cargo.toml.

    Without -Publish this only runs `cargo test`, builds each target and
    writes into dist\ -- nothing leaves the machine.

    With -Publish it first checks that master is clean, contains origin/master
    and that the tag doesn't exist yet; then after packaging it creates the
    annotated tag v<version> ("wappsw v<version>", same as v0.1.0), pushes
    master and the tag, and runs `gh release create` with the zips.

.PARAMETER Targets
    Rust target triples to build. Every one must have its standard library
    installed; a missing one is an error rather than a silently smaller
    release. Pass a subset to ship fewer.

.PARAMETER Publish
    Tag, push and create the GitHub Release. Requires -NotesFile.

.PARAMETER NotesFile
    Markdown file used as the release body.

.EXAMPLE
    powershell -ExecutionPolicy Bypass -File scripts\release.ps1

.EXAMPLE
    powershell -ExecutionPolicy Bypass -File scripts\release.ps1 -Targets i686-pc-windows-msvc

.EXAMPLE
    powershell -ExecutionPolicy Bypass -File scripts\release.ps1 -Publish -NotesFile dist\notes.md
#>

[CmdletBinding()]
param(
    [string[]]$Targets = @('i686-pc-windows-msvc', 'x86_64-pc-windows-msvc'),
    [switch]$Publish,
    [string]$NotesFile
)

$ErrorActionPreference = 'Stop'

$Repo = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
$Manifest = Join-Path $Repo 'Cargo.toml'
$Dist = Join-Path $Repo 'dist'

# Arch suffix used in the zip name, and the PE header machine type the exe
# must carry -- so a 32-bit exe can never end up in the x86_64 zip.
$TargetInfo = @{
    'i686-pc-windows-msvc'   = @{ Arch = 'x86';    Machine = 0x014c }
    'x86_64-pc-windows-msvc' = @{ Arch = 'x86_64'; Machine = 0x8664 }
}

# Copied into every zip at the same relative path, after wappsw.exe.
$Payload = @(
    'README.md',
    'LICENSE',
    'setup/install-task.ps1',
    'assets/LICENSE',
    'assets/LICENSE_Mozc',
    'assets/LICENSE_UniDic'
)

function Invoke-Native([string]$What, [scriptblock]$Command) {
    & $Command
    if ($LASTEXITCODE -ne 0) { throw "$What failed (exit code $LASTEXITCODE)" }
}

function Get-PeMachine([string]$Path) {
    $bytes = [System.IO.File]::ReadAllBytes($Path)
    $peOffset = [BitConverter]::ToInt32($bytes, 0x3C)
    return [BitConverter]::ToUInt16($bytes, $peOffset + 4)
}

# --- what are we releasing --------------------------------------------------

$versionLine = Select-String -Path $Manifest -Pattern '^version\s*=\s*"([^"]+)"' | Select-Object -First 1
if (-not $versionLine) { throw "no version = `"...`" line in $Manifest" }
$Version = $versionLine.Matches[0].Groups[1].Value
$Tag = "v$Version"

$sysroot = (& rustc --print sysroot).Trim()
if ($LASTEXITCODE -ne 0) { throw 'rustc --print sysroot failed -- is Rust installed?' }
$installed = @(Get-ChildItem (Join-Path $sysroot 'lib\rustlib') -Directory |
    Where-Object { Test-Path (Join-Path $_.FullName 'lib') } | ForEach-Object Name)
foreach ($t in $Targets) {
    if (-not $TargetInfo.ContainsKey($t)) {
        throw "unsupported target '$t' (known: $($TargetInfo.Keys -join ', '))"
    }
    if ($installed -notcontains $t) {
        throw ("target '$t' is not installed (installed: $($installed -join ', ')). " +
            "Install it (rustup target add $t) or pass -Targets with only the installed ones.")
    }
}

Write-Host "wappsw $Tag -- targets: $($Targets -join ', ')" -ForegroundColor Cyan

# --- publish preconditions, checked before spending time on builds ----------

if ($Publish) {
    if (-not $NotesFile) { throw '-Publish needs -NotesFile <release notes .md>' }
    if (-not (Test-Path $NotesFile)) { throw "notes file not found: $NotesFile" }
    $NotesFile = (Resolve-Path $NotesFile).Path

    $dirty = & git -C $Repo status --porcelain
    if ($dirty) { throw "working tree is not clean:`n$($dirty -join "`n")" }

    $branch = (& git -C $Repo rev-parse --abbrev-ref HEAD).Trim()
    if ($branch -ne 'master') { throw "publish from master (currently on '$branch')" }

    Invoke-Native 'git fetch' { git -C $Repo fetch -q origin }
    & git -C $Repo merge-base --is-ancestor origin/master HEAD
    if ($LASTEXITCODE -ne 0) { throw 'origin/master has commits this master lacks -- pull first' }

    & git -C $Repo rev-parse -q --verify "refs/tags/$Tag" | Out-Null
    if ($LASTEXITCODE -eq 0) { throw "tag $Tag already exists locally -- bump the version in Cargo.toml" }
    $remoteTag = & git -C $Repo ls-remote --tags origin "refs/tags/$Tag"
    if ($remoteTag) { throw "tag $Tag already exists on origin" }
}

# --- test and build ---------------------------------------------------------

Write-Host "`n-- cargo test --" -ForegroundColor Cyan
Invoke-Native 'cargo test' { cargo test --locked --manifest-path $Manifest }

$targetDir = $env:CARGO_TARGET_DIR
if (-not $targetDir) { $targetDir = Join-Path $Repo 'target' }

$zips = @()
foreach ($t in $Targets) {
    $info = $TargetInfo[$t]
    Write-Host "`n-- build $t --" -ForegroundColor Cyan
    # --target puts the exe under target\<triple>\release, so a running
    # target\release\wappsw.exe never locks the release build.
    Invoke-Native "cargo build ($t)" { cargo build --release --locked --target $t --manifest-path $Manifest }

    $exe = Join-Path $targetDir "$t\release\wappsw.exe"
    $machine = Get-PeMachine $exe
    if ($machine -ne $info.Machine) {
        throw ("{0} has PE machine 0x{1:X4}, expected 0x{2:X4} for {3}" -f $exe, $machine, $info.Machine, $t)
    }

    # --- package ------------------------------------------------------------

    $name = "wappsw-$Tag-$($info.Arch)"
    $stage = Join-Path $Dist $name
    $zip = Join-Path $Dist "$name.zip"
    if (Test-Path $stage) { Remove-Item $stage -Recurse -Force }
    if (Test-Path $zip) { Remove-Item $zip -Force }
    New-Item -ItemType Directory -Force $stage | Out-Null

    $entries = @('wappsw.exe') + $Payload
    Copy-Item $exe (Join-Path $stage 'wappsw.exe')
    foreach ($rel in $Payload) {
        $dest = Join-Path $stage $rel
        New-Item -ItemType Directory -Force (Split-Path $dest) | Out-Null
        Copy-Item (Join-Path $Repo $rel) $dest
    }

    # Built entry by entry rather than with Compress-Archive: Windows
    # PowerShell 5.1's Compress-Archive writes '\' into entry names, which
    # other unzip tools treat as part of the file name.
    Add-Type -AssemblyName System.IO.Compression, System.IO.Compression.FileSystem
    $archive = [System.IO.Compression.ZipFile]::Open($zip, [System.IO.Compression.ZipArchiveMode]::Create)
    try {
        foreach ($rel in $entries) {
            [System.IO.Compression.ZipFileExtensions]::CreateEntryFromFile(
                $archive, (Join-Path $stage $rel), $rel,
                [System.IO.Compression.CompressionLevel]::Optimal) | Out-Null
        }
    } finally {
        $archive.Dispose()
    }

    $hash = (Get-FileHash $zip -Algorithm SHA256).Hash.ToLowerInvariant()
    Write-Host ("  {0}  {1:N0} bytes  sha256:{2}" -f $zip, (Get-Item $zip).Length, $hash) -ForegroundColor Green
    $zips += $zip
}

if (-not $Publish) {
    Write-Host "`nPackaged $($zips.Count) zip(s). Not published (no -Publish)." -ForegroundColor Cyan
    exit 0
}

# --- publish ----------------------------------------------------------------

Write-Host "`n-- publish $Tag --" -ForegroundColor Cyan
Invoke-Native 'git tag' { git -C $Repo tag -a $Tag -m "wappsw $Tag" }
Invoke-Native 'git push master' { git -C $Repo push origin master }
Invoke-Native "git push $Tag" { git -C $Repo push origin $Tag }
Invoke-Native 'gh release create' {
    gh release create $Tag @zips --repo panzoux/wappsw --title "wappsw $Tag" --notes-file $NotesFile --verify-tag
}
Write-Host "`nPublished $Tag." -ForegroundColor Green
