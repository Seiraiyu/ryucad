# Install ryueda from a GitHub release: native Windows x64.
#
#   gh release download ryueda-v0.1.0 -R Seiraiyu/ryucad -p install-ryueda.ps1 -O install-ryueda.ps1
#   Get-Content -Raw install-ryueda.ps1 | Invoke-Expression
#
# ryueda's releases are tagged ryueda-vX.Y.Z, apart from ryucad's vX.Y.Z,
# so GitHub's "latest" release is never ryueda's: with no version given,
# this installs the newest ryueda release that isn't a pre-release.
#
# Environment:
#   RYUEDA_VERSION          version or tag to install: 0.1.0, v0.1.0 or
#                           ryueda-v0.1.0 (default: the newest release)
#   RYUEDA_HOME             install root (default: ~\.ryueda); the binary goes in bin\
#   RYUEDA_NO_MODIFY_PATH   set to 1 to leave the user PATH alone
#   GH_TOKEN, GITHUB_TOKEN  a token for a private repository (else `gh auth token`)
#   RYUEDA_DOWNLOAD_BASE    a directory laid out as <base>\releases (the API's
#                           list of releases) and <base>\download\<tag>\<asset>;
#                           used by tests
#
# Runs in the caller's session under `iex`, so it never calls `exit`.

function Install-Ryueda {
    # An installer talks to the person running it.
    [Diagnostics.CodeAnalysis.SuppressMessageAttribute('PSAvoidUsingWriteHost', '')]
    param()
    $ErrorActionPreference = 'Stop'
    $ProgressPreference = 'SilentlyContinue'
    $repo = 'Seiraiyu/ryucad'
    $api = "https://api.github.com/repos/$repo"

    if ($env:OS -ne 'Windows_NT') {
        throw 'install-ryueda.ps1 is for Windows; on Linux, macOS and WSL use install-ryueda.sh'
    }
    if ($env:PROCESSOR_ARCHITECTURE -ne 'AMD64') {
        throw "no ryueda build for Windows $($env:PROCESSOR_ARCHITECTURE). Supported: Windows x64"
    }
    $target = 'x86_64-pc-windows-msvc'
    # Windows PowerShell 5.1 may default to TLS 1.0.
    [Net.ServicePointManager]::SecurityProtocol = [Net.ServicePointManager]::SecurityProtocol -bor [Net.SecurityProtocolType]::Tls12

    $token = $env:GH_TOKEN
    if (-not $token) { $token = $env:GITHUB_TOKEN }
    if (-not $token -and (Get-Command gh -ErrorAction SilentlyContinue)) {
        # A gh that isn't logged in writes to stderr, which Windows
        # PowerShell 5.1 turns into a terminating error under 'Stop'.
        try { $t = (& gh auth token 2>$null) } catch { $t = $null }
        if ($LASTEXITCODE -eq 0 -and $t) { $token = "$t".Trim() }
    }
    $headers = @{}
    if ($token) { $headers['Authorization'] = "Bearer $token" }
    $base = $env:RYUEDA_DOWNLOAD_BASE

    function Get-Url($url, $out, $extra) {
        $h = $headers.Clone()
        if ($extra) { foreach ($k in $extra.Keys) { $h[$k] = $extra[$k] } }
        Invoke-WebRequest -UseBasicParsing -Uri $url -OutFile $out -Headers $h
    }
    function Get-Unreachable($what) {
        if ($token) { return "could not reach ${what}: no such release, or the token cannot read $repo" }
        return "could not reach $what. If the repository is private, set GH_TOKEN or run 'gh auth login' first"
    }

    $tmp = Join-Path ([IO.Path]::GetTempPath()) ("ryueda-install-" + [Guid]::NewGuid())
    New-Item -ItemType Directory -Path $tmp | Out-Null
    try {
        # Which version: the newest ryueda release, newest first as the
        # API lists them, drafts and pre-releases left out.
        $tag = $env:RYUEDA_VERSION
        if (-not $tag) {
            if ($base) {
                $releases = Get-Content -Raw (Join-Path $base 'releases') | ConvertFrom-Json
            } else {
                try {
                    $releases = Invoke-RestMethod -UseBasicParsing -Uri "$api/releases?per_page=100" -Headers $headers
                } catch { throw (Get-Unreachable $api) }
            }
            foreach ($r in $releases) {
                if ("$($r.tag_name)".StartsWith('ryueda-v') -and -not $r.draft -and -not $r.prerelease) {
                    $tag = $r.tag_name
                    break
                }
            }
            if (-not $tag) { throw 'no ryueda release yet; set RYUEDA_VERSION to a pre-release''s tag' }
        }
        if (-not $tag.StartsWith('ryueda-v')) {
            if ($tag.StartsWith('v')) { $tag = "ryueda-$tag" } else { $tag = "ryueda-v$tag" }
        }
        $version = $tag.Substring('ryueda-v'.Length)
        $asset = "ryueda-$version-$target.zip"
        Write-Host "ryueda-install: installing ryueda $version for $target"

        # Download the archive and the checksums.
        $zip = Join-Path $tmp $asset
        $sums = Join-Path $tmp 'SHA256SUMS'
        if ($base) {
            $dir = Join-Path (Join-Path $base 'download') $tag
            foreach ($name in @($asset, 'SHA256SUMS')) {
                $src = Join-Path $dir $name
                if (-not (Test-Path $src)) { throw "no $name in release $tag" }
                Copy-Item $src (Join-Path $tmp $name)
            }
        } elseif ($token) {
            # The API's asset endpoint works for private and public repositories.
            try {
                $release = Invoke-RestMethod -UseBasicParsing -Uri "$api/releases/tags/$tag" -Headers $headers
            } catch { throw (Get-Unreachable "release $tag") }
            foreach ($name in @($asset, 'SHA256SUMS')) {
                $a = $release.assets | Where-Object { $_.name -eq $name } | Select-Object -First 1
                if (-not $a) { throw "no $name in release $tag" }
                Get-Url $a.url (Join-Path $tmp $name) @{ Accept = 'application/octet-stream' }
            }
        } else {
            $dl = "https://github.com/$repo/releases/download/$tag"
            try { Get-Url "$dl/$asset" $zip $null } catch { throw (Get-Unreachable "$dl/$asset") }
            Get-Url "$dl/SHA256SUMS" $sums $null
        }

        # Verify before anything is installed.
        $want = $null
        foreach ($line in Get-Content $sums) {
            $f = $line -split '\s+', 2
            if ($f.Count -eq 2 -and $f[1].TrimStart('*') -eq $asset) { $want = $f[0].ToLowerInvariant() }
        }
        if (-not $want) { throw "$asset is not listed in SHA256SUMS" }
        $got = (Get-FileHash -Algorithm SHA256 $zip).Hash.ToLowerInvariant()
        if ($got -ne $want) { throw "checksum mismatch for ${asset}: expected $want, got $got" }

        # Install. A running .exe can't be overwritten but can be renamed.
        Expand-Archive -Path $zip -DestinationPath $tmp -Force
        $home_ = $env:RYUEDA_HOME
        if (-not $home_) { $home_ = Join-Path $env:USERPROFILE '.ryueda' }
        $bin = Join-Path $home_ 'bin'
        New-Item -ItemType Directory -Force -Path $bin | Out-Null
        $exe = Join-Path $bin 'ryueda.exe'
        $old = "$exe.old"
        if (Test-Path $exe) { & $exe daemon stop --all *> $null }
        Remove-Item -Force $old -ErrorAction SilentlyContinue
        if (Test-Path $exe) { Move-Item -Force $exe $old }
        Copy-Item (Join-Path $tmp "ryueda-$version-$target\ryueda.exe") $exe
        Remove-Item -Force $old -ErrorAction SilentlyContinue

        # PATH: the user's, which needs no admin rights, and this session's.
        $userPath = [Environment]::GetEnvironmentVariable('Path', 'User')
        $parts = @()
        if ($userPath) { $parts = $userPath -split ';' | Where-Object { $_ } }
        if ($parts -notcontains $bin) {
            if ($env:RYUEDA_NO_MODIFY_PATH -eq '1') {
                Write-Host "ryueda-install: add $bin to your PATH"
            } else {
                [Environment]::SetEnvironmentVariable('Path', (($parts + $bin) -join ';'), 'User')
                Write-Host "ryueda-install: added $bin to your user PATH; new terminals will find ryueda"
            }
        }
        if (($env:Path -split ';') -notcontains $bin) { $env:Path = "$bin;$env:Path" }

        & $exe version
    } finally {
        Remove-Item -Recurse -Force $tmp -ErrorAction SilentlyContinue
    }
}

Install-Ryueda
