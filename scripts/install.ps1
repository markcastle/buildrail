# Installs or updates BuildRail on Windows from the latest GitHub release.
# The same line does both. In PowerShell:
#
#   irm https://raw.githubusercontent.com/markcastle/buildrail/main/scripts/install.ps1 | iex
#
# What it does, in order:
#
#   1. Asks GitHub for the latest release and picks the Windows download by
#      the end of its name (never the full name, which carries a version).
#      Every release has one; if the latest ever did not, it installs the
#      newest published release that does, and says so.
#   2. Downloads it and the release's SHA256SUMS.txt, and refuses to go on
#      unless the download's SHA-256 matches. A truncated or tampered
#      download is never installed.
#   3. Stops a running copy, puts the new one at
#      %LOCALAPPDATA%\Programs\BuildRail\buildrail.exe (just for you, no
#      administrator prompt), adds a Start menu shortcut and that folder to
#      your PATH, and starts it.
#
# Set GITHUB_TOKEN first only while the repository is private.
#
# Errors are thrown rather than calling exit, because exit would close the
# PowerShell window that ran `irm ... | iex`.

$ErrorActionPreference = 'Stop'
# Windows PowerShell 5.1 draws a progress bar so slowly that it multiplies
# download times; the messages below say what is happening instead.
$ProgressPreference = 'SilentlyContinue'
# Windows PowerShell 5.1 on older Windows may not offer TLS 1.2 by default.
[Net.ServicePointManager]::SecurityProtocol = [Net.ServicePointManager]::SecurityProtocol -bor [Net.SecurityProtocolType]::Tls12

$repo = 'markcastle/buildrail'
$target = 'windows-x86_64'
$installDir = Join-Path $env:LOCALAPPDATA 'Programs\BuildRail'
$exePath = Join-Path $installDir 'buildrail.exe'

$headers = @{ 'User-Agent' = 'buildrail-install' }
if ($env:GITHUB_TOKEN) { $headers['Authorization'] = "Bearer $env:GITHUB_TOKEN" }

function Get-ReleaseFile($asset, $path) {
    if ($env:GITHUB_TOKEN) {
        # Private repositories only hand out release files through the API.
        $download = $headers.Clone()
        $download['Accept'] = 'application/octet-stream'
        Invoke-WebRequest -Uri $asset.url -OutFile $path -Headers $download -UseBasicParsing
    } else {
        Invoke-WebRequest -Uri $asset.browser_download_url -OutFile $path -Headers $headers -UseBasicParsing
    }
}

Write-Host 'Looking up the latest BuildRail release...'
try {
    $release = Invoke-RestMethod -Uri "https://api.github.com/repos/$repo/releases/latest" -Headers $headers
} catch {
    throw "BuildRail was not installed: could not read the latest release of $repo. If the repository is private, set GITHUB_TOKEN first. ($($_.Exception.Message))"
}
$tag = $release.tag_name

$zip = $release.assets | Where-Object { $_.name -like "*-$target.zip" } | Select-Object -First 1
if (-not $zip) {
    # Every release is built for Windows, so this should not happen; if it
    # does, install the newest published release that has a Windows build.
    Write-Host "The latest release, $tag, has no Windows download. Looking for the newest release that has one..."
    try {
        $all = Invoke-RestMethod -Uri "https://api.github.com/repos/$repo/releases?per_page=100" -Headers $headers
    } catch {
        throw "BuildRail was not installed: could not list the releases of $repo. ($($_.Exception.Message))"
    }
    $release = $all | Where-Object { -not $_.draft -and -not $_.prerelease -and ($_.assets | Where-Object { $_.name -like "*-$target.zip" }) } | Select-Object -First 1
    if (-not $release) { throw 'BuildRail was not installed: no published release has a Windows download.' }
    $tag = $release.tag_name
    Write-Host "Installing BuildRail $tag, the newest release with a Windows download."
    $zip = $release.assets | Where-Object { $_.name -like "*-$target.zip" } | Select-Object -First 1
}
$sums = $release.assets | Where-Object { $_.name -eq 'SHA256SUMS.txt' } | Select-Object -First 1
if (-not $sums) { throw "BuildRail was not installed: release $tag has no SHA256SUMS.txt, so its download cannot be checked." }

$work = Join-Path ([IO.Path]::GetTempPath()) ("buildrail-install-" + [Guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $work -Force | Out-Null
try {
    $zipPath = Join-Path $work $zip.name
    $sumsPath = Join-Path $work 'SHA256SUMS.txt'
    Write-Host "Downloading BuildRail $tag ($([math]::Round($zip.size / 1MB, 1)) MB)..."
    Get-ReleaseFile $zip $zipPath
    Get-ReleaseFile $sums $sumsPath

    # Lines look like "<64 hex>  <file name>" (sha256sum's format).
    $expected = $null
    foreach ($line in Get-Content -Path $sumsPath) {
        if ($line -match '^([0-9a-fA-F]{64}) [ *](.+)$' -and $Matches[2].Trim() -eq $zip.name) {
            $expected = $Matches[1].ToLowerInvariant()
            break
        }
    }
    if (-not $expected) { throw "BuildRail was not installed: SHA256SUMS.txt does not list $($zip.name)." }
    $actual = (Get-FileHash -Path $zipPath -Algorithm SHA256).Hash.ToLowerInvariant()
    if ($actual -ne $expected) {
        throw "BuildRail was not installed: the download does not match its checksum (expected $expected, got $actual). Nothing was changed; try again."
    }
    Write-Host 'Checksum matches.'

    # It came from the internet; without this Windows asks before running it.
    Unblock-File -Path $zipPath
    $unpacked = Join-Path $work 'unpacked'
    Expand-Archive -Path $zipPath -DestinationPath $unpacked -Force
    $newExe = Get-ChildItem -Path $unpacked -Recurse -Filter 'buildrail.exe' | Select-Object -First 1
    if (-not $newExe) { throw 'BuildRail was not installed: the download did not contain buildrail.exe.' }

    # A running copy holds its file open, and Windows will not replace it.
    $running = Get-Process -Name 'buildrail' -ErrorAction SilentlyContinue
    if ($running) {
        Write-Host 'Stopping the running copy...'
        $running | Stop-Process -Force -ErrorAction SilentlyContinue
        $running | Wait-Process -Timeout 10 -ErrorAction SilentlyContinue
    }

    New-Item -ItemType Directory -Path $installDir -Force | Out-Null
    Copy-Item -Path $newExe.FullName -Destination $exePath -Force
    Unblock-File -Path $exePath
} finally {
    Remove-Item -Path $work -Recurse -Force -ErrorAction SilentlyContinue
}

# A Start menu entry, so it can be found and pinned like any other app.
$shortcutPath = Join-Path ([Environment]::GetFolderPath('Programs')) 'BuildRail.lnk'
try {
    $shell = New-Object -ComObject WScript.Shell
    $shortcut = $shell.CreateShortcut($shortcutPath)
    $shortcut.TargetPath = $exePath
    $shortcut.WorkingDirectory = $installDir
    $shortcut.Description = 'BuildRail: your builds at a glance'
    $shortcut.Save()
} catch {
    Write-Host "Could not add the Start menu shortcut ($($_.Exception.Message)). BuildRail is installed all the same."
}

# On your PATH, so `buildrail --help` works in a new terminal.
$userPath = [Environment]::GetEnvironmentVariable('Path', 'User')
$parts = @($userPath -split ';' | Where-Object { $_ })
if ($parts -notcontains $installDir) {
    [Environment]::SetEnvironmentVariable('Path', (($parts + $installDir) -join ';'), 'User')
    Write-Host "Added $installDir to your PATH for new terminals."
}

Start-Process -FilePath $exePath -WorkingDirectory $installDir
Write-Host "BuildRail $tag is installed and running: look along the bottom of your screen."
Write-Host 'Run the same line again at any time to update.'
