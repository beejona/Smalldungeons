# Rustclear one-step installer for Windows. In PowerShell:
#   irm https://raw.githubusercontent.com/beejona/Smalldungeons/main/install.ps1 | iex
#
# Installs to %LOCALAPPDATA%\Rustclear and adds a "Rustclear" shortcut to the desktop and Start menu.
# If this repo has a published release with Rustclear-windows.zip, it downloads that (no compilers
# needed). Otherwise it builds from source, installing Git, Rust and the C++ build tools with winget
# where they're missing. Running it again updates an existing install.
#
# Env overrides: RUSTCLEAR_DIR (install folder), RUSTCLEAR_FROM_SOURCE=1 (skip the release check),
# RUSTCLEAR_REPO (owner/name, default beejona/Smalldungeons), RUSTCLEAR_BRANCH (default main),
# RUSTCLEAR_DATA_REPO (git URL of the room data), RUSTCLEAR_NO_START=1 (don't start it afterwards).

function Install-Rustclear {
    $ErrorActionPreference = 'Stop'
    $ProgressPreference = 'SilentlyContinue'
    [Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12

    $dir = if ($env:RUSTCLEAR_DIR) { $env:RUSTCLEAR_DIR } else { "$env:LOCALAPPDATA\Rustclear" }
    $repo = if ($env:RUSTCLEAR_REPO) { $env:RUSTCLEAR_REPO } else { 'beejona/Smalldungeons' }
    $branch = if ($env:RUSTCLEAR_BRANCH) { $env:RUSTCLEAR_BRANCH } else { 'main' }
    $dataRepo = if ($env:RUSTCLEAR_DATA_REPO) { $env:RUSTCLEAR_DATA_REPO } else { 'https://github.com/Moo-ody/Evensmallerdungeonsdata.git' }
    New-Item -ItemType Directory -Force -Path $dir | Out-Null

    $asset = $null
    if (-not $env:RUSTCLEAR_FROM_SOURCE) {
        try {
            $release = Invoke-RestMethod -UseBasicParsing -Uri "https://api.github.com/repos/$repo/releases/latest"
            $asset = $release.assets | Where-Object { $_.name -like 'Rustclear-windows*.zip' } | Select-Object -First 1
        } catch {}
    }

    if ($asset) {
        Write-Host "Downloading Rustclear $($release.tag_name)..."
        $zip = "$env:TEMP\Rustclear-windows.zip"
        Invoke-WebRequest -UseBasicParsing -Uri $asset.browser_download_url -OutFile $zip
        Expand-Archive -Path $zip -DestinationPath $dir -Force
        Remove-Item $zip
    } else {
        Write-Host 'No prebuilt release found, so building Rustclear from source.'
        Install-BuildTools
        $src = "$dir\source"
        $data = "$dir\data"
        Sync-Repo "https://github.com/$repo.git" $branch $src
        Sync-Repo $dataRepo 'main' $data
        if (Test-Path "$src\src\room_data") { Remove-Item -Recurse -Force "$src\src\room_data" }
        Copy-Item -Recurse "$data\room_data" "$src\src\room_data"
        Write-Host 'Compiling (the first build takes several minutes)...'
        Push-Location $src
        try {
            & cargo build --release
            if ($LASTEXITCODE -ne 0) { throw 'cargo build failed; the error is above.' }
        } finally { Pop-Location }
        Copy-Item -Force "$src\target\release\RustClear.exe" $dir
        Copy-Item -Force "$src\launcher\start.ps1", "$src\launcher\Start Rustclear.bat" $dir
    }

    $bat = "$dir\Start Rustclear.bat"
    $shell = New-Object -ComObject WScript.Shell
    $startMenu = [Environment]::GetFolderPath('Programs')
    foreach ($place in @([Environment]::GetFolderPath('Desktop'), $startMenu)) {
        $link = $shell.CreateShortcut("$place\Rustclear.lnk")
        $link.TargetPath = $bat
        $link.WorkingDirectory = $dir
        $link.Save()
    }

    Write-Host ''
    Write-Host "Rustclear is installed in $dir." -ForegroundColor Green
    Write-Host 'Start it any time with the Rustclear shortcut on your desktop or in the Start menu.'
    if (-not $env:RUSTCLEAR_NO_START) {
        & powershell -NoProfile -ExecutionPolicy Bypass -File "$dir\start.ps1"
    }
}

function Update-SessionPath {
    $env:Path = [Environment]::GetEnvironmentVariable('Path', 'Machine') + ';' +
        [Environment]::GetEnvironmentVariable('Path', 'User') + ";$env:USERPROFILE\.cargo\bin"
}

function Test-Msvc {
    $vswhere = "${env:ProgramFiles(x86)}\Microsoft Visual Studio\Installer\vswhere.exe"
    if (-not (Test-Path $vswhere)) { return $false }
    $found = & $vswhere -products * -latest -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath
    return [bool]$found
}

function Invoke-Winget($id, $extra) {
    if (-not (Get-Command winget -ErrorAction SilentlyContinue)) {
        throw "winget isn't available, so $id can't be installed automatically. Install 'App Installer' from the Microsoft Store, then run this again."
    }
    Write-Host "Installing $id..."
    $wingetArgs = @('install', '--id', $id, '-e', '--source', 'winget', '--accept-package-agreements', '--accept-source-agreements') + $extra
    & winget @wingetArgs
    # winget returns non-zero for "already installed"; the checks after it decide whether it worked.
    Update-SessionPath
}

function Install-BuildTools {
    Update-SessionPath
    if (-not (Test-Msvc)) {
        Invoke-Winget 'Microsoft.VisualStudio.2022.BuildTools' @('--override', '--wait --passive --add Microsoft.VisualStudio.Workload.VCTools --includeRecommended')
        if (-not (Test-Msvc)) { throw 'The C++ build tools did not install. Install "Desktop development with C++" from https://visualstudio.microsoft.com/visual-cpp-build-tools then run this again.' }
    }
    if (-not (Get-Command git -ErrorAction SilentlyContinue)) {
        Invoke-Winget 'Git.Git' @()
        if (-not (Get-Command git -ErrorAction SilentlyContinue)) { throw 'Git did not install. Install it from https://git-scm.com/downloads then run this again.' }
    }
    if (-not (Get-Command rustup -ErrorAction SilentlyContinue)) {
        Invoke-Winget 'Rustlang.Rustup' @()
        if (-not (Get-Command rustup -ErrorAction SilentlyContinue)) { throw 'Rust did not install. Install it from https://rustup.rs then run this again.' }
    }
    & rustup default stable | Out-Null
}

function Sync-Repo($url, $branch, $path) {
    if (Test-Path "$path\.git") {
        & git -C $path fetch --depth 1 origin $branch
        & git -C $path reset --hard FETCH_HEAD
    } else {
        & git clone --depth 1 --branch $branch $url $path
    }
    if ($LASTEXITCODE -ne 0) { throw "Could not download $url." }
}

Install-Rustclear
