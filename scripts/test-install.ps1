# Tests install.ps1 against a fake release on disk. Windows only.
#
#   scripts\test-install.ps1 [-Binary target\release\ryucad.exe] [-Shell powershell|pwsh]
#
# Each case runs the installer in a fresh child shell, so Windows
# PowerShell 5.1 and PowerShell 7 can both be tested.

[Diagnostics.CodeAnalysis.SuppressMessageAttribute('PSAvoidUsingWriteHost', '')]
# $Shell is read inside Invoke-Installer.
[Diagnostics.CodeAnalysis.SuppressMessageAttribute('PSReviewUnusedParameter', 'Shell')]
param(
    [string]$Binary = 'target\release\ryucad.exe',
    [string]$Shell = 'powershell'
)
$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
$installer = Join-Path $root 'install.ps1'
$Binary = (Resolve-Path $Binary).Path
$work = Join-Path ([IO.Path]::GetTempPath()) ("ryucad-test-" + [Guid]::NewGuid())
New-Item -ItemType Directory $work | Out-Null
$failures = 0
$target = 'x86_64-pc-windows-msvc'

function Build-FakeRelease($dir) {
    foreach ($v in @('0.0.9', '0.1.0')) {
        $d = Join-Path $dir "download\v$v"
        $stage = Join-Path $work "stage\ryucad-$v-$target"
        New-Item -ItemType Directory -Force $d, $stage | Out-Null
        Copy-Item $Binary (Join-Path $stage 'ryucad.exe')
        $zip = Join-Path $d "ryucad-$v-$target.zip"
        Compress-Archive -Path $stage -DestinationPath $zip
        $hash = (Get-FileHash -Algorithm SHA256 $zip).Hash.ToLowerInvariant()
        Set-Content -Path (Join-Path $d 'SHA256SUMS') -Value "$hash  ryucad-$v-$target.zip"
    }
    Set-Content -Path (Join-Path $dir 'latest') -Value 'v0.1.0'
}

# Run the installer with a fresh USERPROFILE named $name and extra variables.
function Invoke-Installer($name, $base, $vars) {
    $profileDir = Join-Path $work "home-$name"
    New-Item -ItemType Directory -Force $profileDir | Out-Null
    $saved = @{}
    $all = @{ USERPROFILE = $profileDir; RYUCAD_DOWNLOAD_BASE = $base; RYUCAD_NO_MODIFY_PATH = '1' }
    if ($vars) { foreach ($k in $vars.Keys) { $all[$k] = $vars[$k] } }
    foreach ($k in $all.Keys) {
        $saved[$k] = [Environment]::GetEnvironmentVariable($k)
        [Environment]::SetEnvironmentVariable($k, $all[$k])
    }
    # Windows PowerShell 5.1 turns redirected stderr into terminating errors
    # under 'Stop'; the exit code is what decides.
    $ErrorActionPreference = 'Continue'
    try {
        $out = & $Shell -NoProfile -Command "Get-Content -Raw '$installer' | Invoke-Expression" 2>&1 | Out-String
        $status = $LASTEXITCODE
    } finally {
        foreach ($k in $saved.Keys) { [Environment]::SetEnvironmentVariable($k, $saved[$k]) }
    }
    return @{ Out = $out; Ok = ($status -eq 0); Home = $profileDir }
}

function Test-Case($label, $cond, $run) {
    if ($cond) {
        Write-Host "ok   $label"
    } else {
        Write-Host "FAIL $label"
        Write-Host ($run.Out -split "`n" | ForEach-Object { "     | $_" } | Out-String)
        $script:failures++
    }
}

$savedUserPath = [Environment]::GetEnvironmentVariable('Path', 'User')
try {
    $good = Join-Path $work 'good'
    Build-FakeRelease $good
    $bad = Join-Path $work 'bad'
    Build-FakeRelease $bad
    # A tampered download: the archive no longer matches its checksum.
    Add-Content -Path (Join-Path $bad "download\v0.1.0\ryucad-0.1.0-$target.zip") -Value 'x'

    $r = Invoke-Installer 'fresh' $good @{ RYUCAD_NO_MODIFY_PATH = '' }
    $exe = Join-Path $r.Home '.ryucad\bin\ryucad.exe'
    Test-Case 'fresh install succeeds' $r.Ok $r
    Test-Case 'binary installed' (Test-Path $exe) $r
    Test-Case 'reports the installed version' ($r.Out -match '"ryucad":') $r
    $userPath = [Environment]::GetEnvironmentVariable('Path', 'User')
    Test-Case 'user PATH gets the bin directory' (($userPath -split ';') -contains (Split-Path $exe)) $r
    $r = Invoke-Installer 'fresh' $good @{ RYUCAD_NO_MODIFY_PATH = '' }
    $count = @([Environment]::GetEnvironmentVariable('Path', 'User') -split ';' | Where-Object { $_ -eq (Split-Path $exe) }).Count
    Test-Case 'reinstall adds no second PATH entry' ($r.Ok -and $count -eq 1) $r

    # Replacing a running binary: a long-running stand-in holds ryucad.exe.
    Copy-Item (Join-Path $env:SystemRoot 'System32\PING.EXE') $exe -Force
    $running = Start-Process -FilePath $exe -ArgumentList '-n', '60', '127.0.0.1' -WindowStyle Hidden -PassThru
    Start-Sleep -Seconds 1
    $r = Invoke-Installer 'fresh' $good $null
    Test-Case 'replacing a running binary succeeds' ($r.Ok -and $r.Out -match '"ryucad":') $r
    Stop-Process -Id $running.Id -Force -ErrorAction SilentlyContinue
    $running.WaitForExit()
    $r = Invoke-Installer 'fresh' $good $null
    Test-Case 'the next install removes ryucad.exe.old' ($r.Ok -and -not (Test-Path "$exe.old")) $r

    $r = Invoke-Installer 'pinned' $good @{ RYUCAD_VERSION = '0.0.9' }
    Test-Case 'a pinned version installs' ($r.Ok -and $r.Out -match 'installing ryucad 0\.0\.9') $r
    $r = Invoke-Installer 'missing' $good @{ RYUCAD_VERSION = 'v9.9.9' }
    Test-Case 'a missing version fails' ((-not $r.Ok) -and $r.Out -match 'no ryucad-9\.9\.9') $r

    $r = Invoke-Installer 'corrupt' $bad $null
    Test-Case 'checksum mismatch fails' ((-not $r.Ok) -and $r.Out -match 'checksum mismatch') $r
    Test-Case 'checksum mismatch installs nothing' (-not (Test-Path (Join-Path $r.Home '.ryucad\bin\ryucad.exe'))) $r

    $elsewhere = Join-Path $work 'elsewhere'
    $before = [Environment]::GetEnvironmentVariable('Path', 'User')
    $r = Invoke-Installer 'custom' $good @{ RYUCAD_HOME = $elsewhere }
    Test-Case 'RYUCAD_HOME is honoured' (Test-Path (Join-Path $elsewhere 'bin\ryucad.exe')) $r
    Test-Case 'RYUCAD_NO_MODIFY_PATH leaves the user PATH alone' ([Environment]::GetEnvironmentVariable('Path', 'User') -eq $before) $r

    $r = Invoke-Installer 'arm' $good @{ PROCESSOR_ARCHITECTURE = 'ARM64' }
    Test-Case 'unsupported platform fails' ((-not $r.Ok) -and $r.Out -match 'no ryucad build for Windows ARM64') $r
} finally {
    [Environment]::SetEnvironmentVariable('Path', $savedUserPath, 'User')
    Remove-Item -Recurse -Force $work -ErrorAction SilentlyContinue
}

if ($failures -ne 0) { throw "$failures installer test(s) failed" }
Write-Host 'all installer tests passed'
# The last case's installer failed on purpose; don't let its exit code
# become the script's.
exit 0
