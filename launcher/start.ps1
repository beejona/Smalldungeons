# Starts Rustclear: the server, then ViaProxy so 26.x clients can join.
# Fetches a portable Java 21 and ViaProxy on first run if they aren't there.
# Works from an install folder (RustClear.exe next to this file) or from a source checkout
# (target\release\RustClear.exe). Written for Windows PowerShell 5.1, which every Windows 10/11 has.
#
# Env overrides: VIAPROXY_VERSION, BIND (default 127.0.0.1:25565; 0.0.0.0:25565 lets other
# machines join, and the server is offline mode, so anyone who reaches it can join under any name).

$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'
[Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12

$here = Split-Path -Parent $MyInvocation.MyCommand.Path
$viaVersion = if ($env:VIAPROXY_VERSION) { $env:VIAPROXY_VERSION } else { '3.4.14' }
$bind = if ($env:BIND) { $env:BIND } else { '127.0.0.1:25565' }
$serverAddr = '127.0.0.1'
$serverPort = 4972

function Find-Server {
    foreach ($p in @("$here\RustClear.exe", "$here\..\target\release\RustClear.exe")) {
        if (Test-Path $p) { return (Resolve-Path $p).Path }
    }
    return $null
}

function Get-JavaMajor($java) {
    # java -version prints to stderr, which Windows PowerShell turns into errors under 'Stop'.
    $ErrorActionPreference = 'Continue'
    try {
        $out = & $java -version 2>&1 | Out-String
        if ($out -match 'version "(\d+)') { return [int]$Matches[1] }
    } catch {}
    return 0
}

function Find-Java {
    $portable = Get-ChildItem "$here\java" -Filter java.exe -Recurse -ErrorAction SilentlyContinue | Select-Object -First 1
    if ($portable) { return $portable.FullName }
    $onPath = Get-Command java -ErrorAction SilentlyContinue
    if ($onPath -and (Get-JavaMajor $onPath.Source) -ge 21) { return $onPath.Source }
    Write-Host 'Downloading Java 21 (one time, about 45 MB)...'
    $zip = "$env:TEMP\rustclear-jre.zip"
    Invoke-WebRequest -UseBasicParsing -Uri 'https://api.adoptium.net/v3/binary/latest/21/ga/windows/x64/jre/hotspot/normal/eclipse' -OutFile $zip
    Expand-Archive -Path $zip -DestinationPath "$here\java" -Force
    Remove-Item $zip
    return (Get-ChildItem "$here\java" -Filter java.exe -Recurse | Select-Object -First 1).FullName
}

function Find-ViaProxy {
    $dir = "$here\proxy"
    $jar = "$dir\ViaProxy-$viaVersion.jar"
    if (-not (Test-Path $jar)) {
        New-Item -ItemType Directory -Force -Path $dir | Out-Null
        Write-Host "Downloading ViaProxy $viaVersion (one time)..."
        Invoke-WebRequest -UseBasicParsing -Uri "https://github.com/ViaVersion/ViaProxy/releases/download/v$viaVersion/ViaProxy-$viaVersion.jar" -OutFile "$jar.part"
        Move-Item "$jar.part" $jar
    }
    return $jar
}

function Test-Port($hostName, $port) {
    $client = New-Object Net.Sockets.TcpClient
    try { $client.Connect($hostName, $port); return $true } catch { return $false } finally { $client.Close() }
}

$server = Find-Server
if (-not $server) {
    Write-Host 'RustClear.exe not found. Run install.ps1 first (see the README).' -ForegroundColor Red
    exit 1
}
$java = Find-Java
$jar = Find-ViaProxy

if (Test-Port $serverAddr $serverPort) {
    Write-Host "Something is already listening on port $serverPort; using it as the server."
    $serverProc = $null
} else {
    Write-Host 'Starting the Rustclear server (it needs about 8 GB of memory and up to a minute)...'
    $serverProc = Start-Process -FilePath $server -WorkingDirectory (Split-Path $server) -PassThru
    $deadline = (Get-Date).AddMinutes(4)
    while (-not (Test-Port $serverAddr $serverPort)) {
        if ($serverProc.HasExited) {
            Write-Host 'The server closed before it was ready. Check its window for the error.' -ForegroundColor Red
            exit 1
        }
        if ((Get-Date) -gt $deadline) {
            Write-Host 'The server did not start listening within 4 minutes.' -ForegroundColor Red
            exit 1
        }
        Start-Sleep -Seconds 2
    }
}

Write-Host 'Starting ViaProxy for 26.x clients...'
$proxyArgs = @('-jar', "`"$jar`"", 'cli',
    '--target-address', "${serverAddr}:$serverPort",
    '--target-version', '1.8.x',
    '--bind-address', $bind,
    '--auth-method', 'NONE',
    '--chat-signing', 'false')
$proxyProc = Start-Process -FilePath $java -ArgumentList $proxyArgs -WorkingDirectory (Split-Path $jar) -PassThru
$joinPort = [int]$bind.Split(':')[-1]
$deadline = (Get-Date).AddMinutes(1)
while (-not (Test-Port '127.0.0.1' $joinPort)) {
    if ($proxyProc.HasExited -or (Get-Date) -gt $deadline) {
        Write-Host 'ViaProxy did not start. 1.8.9 clients can still join the server directly.' -ForegroundColor Red
        break
    }
    Start-Sleep -Seconds 1
}

Write-Host ''
Write-Host 'Rustclear is running.' -ForegroundColor Green
Write-Host "  Minecraft 26.1 to 26.3: join localhost:$joinPort"
Write-Host "  Minecraft 1.8.9:        join ${serverAddr}:$serverPort"
Write-Host ''
Read-Host 'Press Enter here to stop Rustclear'

foreach ($p in @($proxyProc, $serverProc)) {
    if ($p -and -not $p.HasExited) { Stop-Process -Id $p.Id -Force -ErrorAction SilentlyContinue }
}
