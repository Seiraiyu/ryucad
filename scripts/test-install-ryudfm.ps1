# Tests install-ryudfm.ps1 against a fake release on disk. Windows only.
#
#   scripts\test-install-ryudfm.ps1 [-Binary target\release\ryudfm.exe] [-Shell powershell|pwsh]
#
# The fake release holds the given binary under ryudfm-v0.0.9,
# ryudfm-v0.1.0 and a pre-release, ryudfm-v0.2.0-pre.1, listed as GitHub's
# API lists them, newest first, after a newer ryucad release (v0.9.0) and a
# newer ryueda one (ryueda-v0.3.0).
# Each case runs the installer in a fresh child shell, so Windows
# PowerShell 5.1 and PowerShell 7 can both be tested.

[Diagnostics.CodeAnalysis.SuppressMessageAttribute('PSAvoidUsingWriteHost', '')]
# $Shell is read inside Invoke-Installer.
[Diagnostics.CodeAnalysis.SuppressMessageAttribute('PSReviewUnusedParameter', 'Shell')]
param(
    [string]$Binary = 'target\release\ryudfm.exe',
    [string]$Shell = 'powershell'
)
$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
$installer = Join-Path $root 'install-ryudfm.ps1'
$Binary = (Resolve-Path $Binary).Path
$work = Join-Path ([IO.Path]::GetTempPath()) ("ryudfm-test-" + [Guid]::NewGuid())
New-Item -ItemType Directory $work | Out-Null
$failures = 0
$target = 'x86_64-pc-windows-msvc'

function Write-ReleaseList($dir, $tags) {
    $list = foreach ($t in $tags) {
        [pscustomobject]@{ tag_name = $t; draft = $false; prerelease = $t.Contains('-pre') }
    }
    ConvertTo-Json -InputObject @($list) | Set-Content -Path (Join-Path $dir 'releases')
}

function Build-FakeRelease($dir) {
    foreach ($v in @('0.0.9', '0.1.0', '0.2.0-pre.1')) {
        $d = Join-Path $dir "download\ryudfm-v$v"
        $stage = Join-Path $work "stage\ryudfm-$v-$target"
        New-Item -ItemType Directory -Force $d, $stage | Out-Null
        Copy-Item $Binary (Join-Path $stage 'ryudfm.exe')
        $zip = Join-Path $d "ryudfm-$v-$target.zip"
        Compress-Archive -Path $stage -DestinationPath $zip
        $hash = (Get-FileHash -Algorithm SHA256 $zip).Hash.ToLowerInvariant()
        Set-Content -Path (Join-Path $d 'SHA256SUMS') -Value "$hash  ryudfm-$v-$target.zip"
    }
    Write-ReleaseList $dir @('v0.9.0', 'ryueda-v0.3.0', 'ryudfm-v0.2.0-pre.1', 'ryudfm-v0.1.0', 'ryudfm-v0.0.9')
}

# Run the installer with a fresh USERPROFILE named $name and extra variables.
function Invoke-Installer($name, $base, $vars) {
    $profileDir = Join-Path $work "home-$name"
    New-Item -ItemType Directory -Force $profileDir | Out-Null
    $saved = @{}
    $all = @{ USERPROFILE = $profileDir; HOME = $profileDir; RYUDFM_DOWNLOAD_BASE = $base; RYUDFM_NO_MODIFY_PATH = '1' }
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
    Add-Content -Path (Join-Path $bad "download\ryudfm-v0.1.0\ryudfm-0.1.0-$target.zip") -Value 'x'
    $early = Join-Path $work 'early'
    Build-FakeRelease $early
    Write-ReleaseList $early @('v0.9.0', 'ryueda-v0.3.0', 'ryudfm-v0.2.0-pre.1')

    $r = Invoke-Installer 'fresh' $good @{ RYUDFM_NO_MODIFY_PATH = '' }
    $exe = Join-Path $r.Home '.ryudfm\bin\ryudfm.exe'
    Test-Case 'fresh install succeeds' $r.Ok $r
    Test-Case 'binary installed' (Test-Path $exe) $r
    Test-Case 'the newest ryudfm release, not ryucad''s latest, ryueda''s or a pre-release' ($r.Out -match 'installing ryudfm 0\.1\.0') $r
    Test-Case 'reports the installed version' ($r.Out -match '"ryudfm":') $r
    $userPath = [Environment]::GetEnvironmentVariable('Path', 'User')
    Test-Case 'user PATH gets the bin directory' (($userPath -split ';') -contains (Split-Path $exe)) $r
    Test-Case 'nothing of ryucad''s or ryueda''s is touched' (-not (Test-Path (Join-Path $r.Home '.ryucad')) -and -not (Test-Path (Join-Path $r.Home '.ryueda'))) $r
    $r = Invoke-Installer 'fresh' $good @{ RYUDFM_NO_MODIFY_PATH = '' }
    $count = @([Environment]::GetEnvironmentVariable('Path', 'User') -split ';' | Where-Object { $_ -eq (Split-Path $exe) }).Count
    Test-Case 'reinstall adds no second PATH entry' ($r.Ok -and $count -eq 1) $r

    # Replacing a running binary: a long-running stand-in holds ryudfm.exe.
    Copy-Item (Join-Path $env:SystemRoot 'System32\PING.EXE') $exe -Force
    $running = Start-Process -FilePath $exe -ArgumentList '-n', '60', '127.0.0.1' -WindowStyle Hidden -PassThru
    Start-Sleep -Seconds 1
    $r = Invoke-Installer 'fresh' $good $null
    Test-Case 'replacing a running binary succeeds' ($r.Ok -and $r.Out -match '"ryudfm":') $r
    Stop-Process -Id $running.Id -Force -ErrorAction SilentlyContinue
    $running.WaitForExit()
    $r = Invoke-Installer 'fresh' $good $null
    Test-Case 'the next install removes ryudfm.exe.old' ($r.Ok -and -not (Test-Path "$exe.old")) $r

    foreach ($v in @('0.0.9', 'v0.0.9', 'ryudfm-v0.0.9')) {
        $r = Invoke-Installer "pinned-$v" $good @{ RYUDFM_VERSION = $v }
        Test-Case "RYUDFM_VERSION=$v installs 0.0.9" ($r.Ok -and $r.Out -match 'installing ryudfm 0\.0\.9') $r
    }
    $r = Invoke-Installer 'prerelease' $good @{ RYUDFM_VERSION = 'ryudfm-v0.2.0-pre.1' }
    Test-Case 'a pre-release installs when named' ($r.Ok -and $r.Out -match 'installing ryudfm 0\.2\.0-pre\.1') $r
    $r = Invoke-Installer 'missing' $good @{ RYUDFM_VERSION = '9.9.9' }
    Test-Case 'a missing version fails' ((-not $r.Ok) -and $r.Out -match 'no ryudfm-9\.9\.9') $r
    $r = Invoke-Installer 'early' $early $null
    Test-Case 'no release yet says to name a pre-release' ((-not $r.Ok) -and $r.Out -match 'no ryudfm release yet') $r

    $r = Invoke-Installer 'corrupt' $bad $null
    Test-Case 'checksum mismatch fails' ((-not $r.Ok) -and $r.Out -match 'checksum mismatch') $r
    Test-Case 'checksum mismatch installs nothing' (-not (Test-Path (Join-Path $r.Home '.ryudfm\bin\ryudfm.exe'))) $r

    $elsewhere = Join-Path $work 'elsewhere'
    $before = [Environment]::GetEnvironmentVariable('Path', 'User')
    $r = Invoke-Installer 'custom' $good @{ RYUDFM_HOME = $elsewhere }
    Test-Case 'RYUDFM_HOME is honoured' (Test-Path (Join-Path $elsewhere 'bin\ryudfm.exe')) $r
    Test-Case 'RYUDFM_NO_MODIFY_PATH leaves the user PATH alone' ([Environment]::GetEnvironmentVariable('Path', 'User') -eq $before) $r

    $r = Invoke-Installer 'arm' $good @{ PROCESSOR_ARCHITECTURE = 'ARM64' }
    Test-Case 'unsupported platform fails' ((-not $r.Ok) -and $r.Out -match 'no ryudfm build for Windows ARM64') $r
} finally {
    [Environment]::SetEnvironmentVariable('Path', $savedUserPath, 'User')
    Remove-Item -Recurse -Force $work -ErrorAction SilentlyContinue
}

if ($failures -ne 0) { throw "$failures installer test(s) failed" }
Write-Host 'all ryudfm installer tests passed'
# The last case's installer failed on purpose; don't let its exit code
# become the script's.
exit 0
