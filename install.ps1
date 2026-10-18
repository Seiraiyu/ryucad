# Install ryucad from a GitHub release: native Windows x64.
#
#   irm https://github.com/Seiraiyu/ryucad/releases/latest/download/install.ps1 | iex
#
# Environment:
#   RYUCAD_VERSION          tag to install (default: the latest release)
#   RYUCAD_HOME             install root (default: ~\.ryucad); the binary goes in bin\
#   RYUCAD_NO_MODIFY_PATH   set to 1 to leave the user PATH alone
#   RYUCAD_NO_APP           set to 1 to install the ryucad command only, not the
#                           desktop app
#   GH_TOKEN, GITHUB_TOKEN  a token for a private repository (else `gh auth token`)
#   RYUCAD_DOWNLOAD_BASE    a directory laid out as <base>\latest and
#                           <base>\download\<tag>\<asset>; used by tests
#
# Runs in the caller's session under `iex`, so it never calls `exit`.

function Install-Ryucad {
    # An installer talks to the person running it.
    [Diagnostics.CodeAnalysis.SuppressMessageAttribute('PSAvoidUsingWriteHost', '')]
    param()
    $ErrorActionPreference = 'Stop'
    $ProgressPreference = 'SilentlyContinue'
    $repo = 'Seiraiyu/ryucad'
    $api = "https://api.github.com/repos/$repo"

    if ($env:OS -ne 'Windows_NT') {
        throw 'install.ps1 is for Windows; on Linux, macOS and WSL use install.sh'
    }
    if ($env:PROCESSOR_ARCHITECTURE -ne 'AMD64') {
        throw "no ryucad build for Windows $($env:PROCESSOR_ARCHITECTURE). Supported: Windows x64"
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
    $base = $env:RYUCAD_DOWNLOAD_BASE

    function Get-Url($url, $out, $extra) {
        $h = $headers.Clone()
        if ($extra) { foreach ($k in $extra.Keys) { $h[$k] = $extra[$k] } }
        Invoke-WebRequest -UseBasicParsing -Uri $url -OutFile $out -Headers $h
    }
    function Get-Unreachable($what) {
        if ($token) { return "could not reach ${what}: no such release, or the token cannot read $repo" }
        return "could not reach $what. If the repository is private, set GH_TOKEN or run 'gh auth login' first"
    }

    $tmp = Join-Path ([IO.Path]::GetTempPath()) ("ryucad-install-" + [Guid]::NewGuid())
    New-Item -ItemType Directory -Path $tmp | Out-Null
    try {
        # Which version.
        $tag = $env:RYUCAD_VERSION
        if (-not $tag) {
            if ($base) {
                $tag = (Get-Content -Raw (Join-Path $base 'latest')).Trim()
            } else {
                try {
                    $tag = (Invoke-RestMethod -UseBasicParsing -Uri "$api/releases/latest" -Headers $headers).tag_name
                } catch { throw (Get-Unreachable $api) }
            }
        }
        if (-not $tag.StartsWith('v')) { $tag = "v$tag" }
        $version = $tag.Substring(1)
        $asset = "ryucad-$version-$target.zip"
        Write-Host "ryucad-install: installing ryucad $version for $target"

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
        $home_ = $env:RYUCAD_HOME
        if (-not $home_) { $home_ = Join-Path $env:USERPROFILE '.ryucad' }
        $bin = Join-Path $home_ 'bin'
        New-Item -ItemType Directory -Force -Path $bin | Out-Null
        $exe = Join-Path $bin 'ryucad.exe'
        $old = "$exe.old"
        if (Test-Path $exe) { & $exe daemon stop --all *> $null }
        Remove-Item -Force $old -ErrorAction SilentlyContinue
        if (Test-Path $exe) { Move-Item -Force $exe $old }
        Copy-Item (Join-Path $tmp "ryucad-$version-$target\ryucad.exe") $exe
        Remove-Item -Force $old -ErrorAction SilentlyContinue

        # The desktop app, unless only the command is wanted, and "Open in
        # ryucad" on folders in Explorer, for this user only (no admin).
        $appSrc = Join-Path $tmp "ryucad-$version-$target\ryucad-app.exe"
        if ($env:RYUCAD_NO_APP -eq '1') {
            Write-Host 'ryucad-install: RYUCAD_NO_APP=1: the desktop app is not installed'
        } elseif (Test-Path $appSrc) {
            $app = Join-Path $bin 'ryucad-app.exe'
            $appOld = "$app.old"
            Remove-Item -Force $appOld -ErrorAction SilentlyContinue
            if (Test-Path $app) { Move-Item -Force $app $appOld }
            Copy-Item $appSrc $app
            Remove-Item -Force $appOld -ErrorAction SilentlyContinue
            foreach ($key in @('HKCU:\Software\Classes\Directory\shell\ryucad',
                               'HKCU:\Software\Classes\Directory\Background\shell\ryucad')) {
                New-Item -Path "$key\command" -Force | Out-Null
                Set-Item -Path $key -Value 'Open in ryucad'
                Set-ItemProperty -Path $key -Name 'Icon' -Value "`"$app`""
                Set-Item -Path "$key\command" -Value "`"$app`" `"%V`""
            }
        }

        # PATH: the user's, which needs no admin rights, and this session's.
        $userPath = [Environment]::GetEnvironmentVariable('Path', 'User')
        $parts = @()
        if ($userPath) { $parts = $userPath -split ';' | Where-Object { $_ } }
        if ($parts -notcontains $bin) {
            if ($env:RYUCAD_NO_MODIFY_PATH -eq '1') {
                Write-Host "ryucad-install: add $bin to your PATH"
            } else {
                [Environment]::SetEnvironmentVariable('Path', (($parts + $bin) -join ';'), 'User')
                Write-Host "ryucad-install: added $bin to your user PATH; new terminals will find ryucad"
            }
        }
        if (($env:Path -split ';') -notcontains $bin) { $env:Path = "$bin;$env:Path" }

        & $exe version
    } finally {
        Remove-Item -Recurse -Force $tmp -ErrorAction SilentlyContinue
    }
}

Install-Ryucad
