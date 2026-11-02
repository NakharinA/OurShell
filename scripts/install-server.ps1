<#
.SYNOPSIS
    OurShell sync server installer for Windows (Docker Desktop).

.DESCRIPTION
    Writes docker-compose.yml, .env and backup.sh into an install directory,
    starts the server (and, unless you point it at an existing database, a
    PostgreSQL container), then prints the address of the setup wizard and
    the one-time setup code. The wizard -- not this script -- generates the
    server's master key. Re-run it to upgrade.

    Interactive:
        irm https://github.com/NakharinA/OurShell/releases/latest/download/install-server.ps1 | iex

    With options:
        & ([scriptblock]::Create((irm https://github.com/NakharinA/OurShell/releases/latest/download/install-server.ps1))) -Yes -Db new -Port 8443

.PARAMETER Yes
    Accept defaults and never prompt.
.PARAMETER Dir
    Install directory (default: $env:ProgramData\OurShell).
.PARAMETER Db
    'new' starts a bundled PostgreSQL 17 (default); 'existing' uses -DatabaseUrl.
.PARAMETER DatabaseUrl
    postgres://user:password@host:5432/db; localhost/127.0.0.1/::1 become host.docker.internal.
.PARAMETER Port
    Host port for the server's HTTP listener (default 8443).
.PARAMETER Listen
    Host address the HTTP port is published on (default: every interface).
    127.0.0.1 = reachable only from this machine, e.g. through a reverse
    proxy running here.
.PARAMETER BehindProxy
    Run behind your own reverse proxy / ingress / Cloudflare Tunnel, which
    handles HTTPS (plain HTTP only here). Not with -Domain.
.PARAMETER TrustedProxies
    With -BehindProxy: comma-separated proxy IPs/CIDRs whose X-Forwarded-For
    and X-Forwarded-Proto are trusted (default: loopback and private networks).
.PARAMETER Domain
    Get a Let's Encrypt certificate for this domain and serve HTTPS. Let's
    Encrypt always connects to port 443: forward public TCP 443 to -HttpsPort.
.PARAMETER HttpsPort
    With -Domain: host port for the built-in HTTPS listener (default 3000).
.PARAMETER Email
    Contact email for Let's Encrypt (with -Domain).
.PARAMETER PublicUrl
    Address clients use, e.g. https://sync.example.com.
.PARAMETER Image
    Server image (default ghcr.io/nakharina/ourshell-server:<version>).
.PARAMETER Project
    Docker Compose project name (default ourshell).
.PARAMETER Upgrade
    Upgrade an existing install (pull the image, restart; data kept).
.PARAMETER Uninstall
    Stop and remove the containers (volumes and files are kept).
.PARAMETER Purge
    With -Uninstall: also delete the data volumes and the install directory.
#>
[Diagnostics.CodeAnalysis.SuppressMessageAttribute('PSAvoidUsingWriteHost', '', Justification = 'Interactive installer: colored console output is the UI')]
[Diagnostics.CodeAnalysis.SuppressMessageAttribute('PSReviewUnusedParameter', '', Justification = 'Parameters are read by the functions below (script scope)')]
[Diagnostics.CodeAnalysis.SuppressMessageAttribute('PSUseShouldProcessForStateChangingFunctions', '', Justification = 'Internal helpers of an installer, not cmdlets')]
# No [CmdletBinding()]: its -Debug common parameter has the alias -db, which
# would clash with -Db.
param(
    [switch]$Yes,
    [string]$Dir,
    [ValidateSet('', 'new', 'existing')][string]$Db = '',
    [string]$DatabaseUrl,
    [int]$Port = 0,
    [string]$Listen,
    [switch]$BehindProxy,
    [string]$TrustedProxies,
    [string]$Domain,
    [int]$HttpsPort = 0,
    [string]$Email,
    [string]$PublicUrl,
    [string]$Image,
    [string]$Project,
    [switch]$Upgrade,
    [switch]$Uninstall,
    [switch]$Purge
)

Set-StrictMode -Version 2.0
$ErrorActionPreference = 'Stop'

$OurShellVersion = if ($env:OURSHELL_VERSION) { $env:OURSHELL_VERSION } else { '__OURSHELL_VERSION__' }

$Repo = 'NakharinA/OurShell'
$ImageRepo = 'ghcr.io/nakharina/ourshell-server'

# --- output helpers ------------------------------------------------------------

function Say([string]$Msg) { Write-Host "==> $Msg" -ForegroundColor Cyan }
function Ok([string]$Msg) { Write-Host "  ok $Msg" -ForegroundColor Green }
function Warn([string]$Msg) { Write-Host "warning: $Msg" -ForegroundColor Yellow }
function Fail([string]$Msg) {
    Write-Host "error: $Msg" -ForegroundColor Red
    throw "OurShell installer: $Msg"
}

function Test-Interactive {
    if ([Environment]::GetCommandLineArgs() | Where-Object { $_ -like '-NonI*' }) { return $false }
    return [Environment]::UserInteractive
}

# Ask PROMPT DEFAULT -> the answer (the default with -Yes or empty input).
function Ask([string]$Prompt, [string]$Default = '') {
    if ($Yes) { return $Default }
    $label = if ($Default) { "$Prompt [$Default]" } else { $Prompt }
    $answer = Read-Host $label
    if ([string]::IsNullOrWhiteSpace($answer)) { return $Default }
    return $answer.Trim()
}

function Confirm-Choice([string]$Prompt, [bool]$Default = $true) {
    if ($Yes) { return $true }
    $hint = if ($Default) { 'Y/n' } else { 'y/N' }
    $answer = Read-Host "$Prompt [$hint]"
    if ([string]::IsNullOrWhiteSpace($answer)) { return $Default }
    return $answer.Trim() -match '^[yY]'
}

# --- native commands -------------------------------------------------------------
# Windows PowerShell 5.1 turns a native command's stderr into terminating
# errors under ErrorActionPreference=Stop, so native calls run with Continue.

function Invoke-Native {
    param([string]$Exe, [string[]]$Arguments, [switch]$Quiet)
    $old = $ErrorActionPreference
    $ErrorActionPreference = 'Continue'
    try {
        if ($Quiet) { & $Exe @Arguments *> $null } else { & $Exe @Arguments 2>&1 | ForEach-Object { Write-Host "$_" } }
        return $LASTEXITCODE
    } finally { $ErrorActionPreference = $old }
}

# Runs a native command and returns its stdout (stderr discarded);
# $script:NativeExit holds the exit code.
function Get-NativeOutput {
    param([string]$Exe, [string[]]$Arguments)
    $old = $ErrorActionPreference
    $ErrorActionPreference = 'Continue'
    try {
        $out = & $Exe @Arguments 2>$null
        $script:NativeExit = $LASTEXITCODE
        return (($out | ForEach-Object { "$_" }) -join "`n").Trim()
    } finally { $ErrorActionPreference = $old }
}

function Invoke-Compose {
    param([string[]]$Arguments, [switch]$Quiet)
    $base = @('compose', '-f', (Join-Path $script:InstallDir 'docker-compose.yml'), '-p', $script:ProjectName)
    return Invoke-Native -Exe 'docker' -Arguments ($base + $Arguments) -Quiet:$Quiet
}

# --- small utilities -------------------------------------------------------------

function Get-RandomString([int]$Length, [string]$Alphabet) {
    $rng = [System.Security.Cryptography.RandomNumberGenerator]::Create()
    $limit = 256 - (256 % $Alphabet.Length)   # rejection sampling: no modulo bias
    $sb = New-Object System.Text.StringBuilder
    $buf = New-Object byte[] 64
    while ($sb.Length -lt $Length) {
        $rng.GetBytes($buf)
        foreach ($b in $buf) {
            if ($b -lt $limit -and $sb.Length -lt $Length) { [void]$sb.Append($Alphabet[$b % $Alphabet.Length]) }
        }
    }
    $rng.Dispose()
    return $sb.ToString()
}

# XXXX-XXXX-XXXX-XXXX, Crockford base32 (no I, L, O, U): 80 bits.
function New-SetupCode {
    $raw = Get-RandomString 16 '0123456789ABCDEFGHJKMNPQRSTVWXYZ'
    return ($raw.Substring(0, 4), $raw.Substring(4, 4), $raw.Substring(8, 4), $raw.Substring(12, 4)) -join '-'
}

# One KEY=value line; single-quoted unless plainly safe (read the same way by
# docker compose and by backup.sh, which sources .env).
function Format-EnvLine([string]$Key, [string]$Value) {
    if ($Value.Contains("'")) { Fail "$Key must not contain a single quote" }
    if ($Value -match '^[A-Za-z0-9_.:/@%+=,-]*$') { return "$Key=$Value" }
    return "$Key='$Value'"
}

function Read-EnvFile([string]$Path) {
    $map = @{}
    foreach ($line in [IO.File]::ReadAllLines($Path)) {
        if ($line -match '^([A-Za-z_][A-Za-z0-9_]*)=(.*)$') {
            $v = $Matches[2]
            if ($v -match "^'(.*)'$" -or $v -match '^"(.*)"$') { $v = $Matches[1] }
            $map[$Matches[1]] = $v
        }
    }
    return $map
}

function Write-FileAtomic([string]$Path, [string]$Content) {
    $tmp = "$Path.tmp.$PID"
    [IO.File]::WriteAllText($tmp, ($Content -replace "`r`n", "`n"), (New-Object System.Text.UTF8Encoding($false)))
    Move-Item -LiteralPath $tmp -Destination $Path -Force
}

# .env holds secrets: only the current user, Administrators and SYSTEM.
function Protect-File([string]$Path) {
    $who = if ($env:USERDOMAIN) { "$($env:USERDOMAIN)\$($env:USERNAME)" } else { $env:USERNAME }
    $null = Invoke-Native -Exe 'icacls.exe' -Arguments @($Path, '/inheritance:r', '/grant:r', "${who}:F", '*S-1-5-32-544:F', '*S-1-5-18:F') -Quiet
}

function Set-EnvKey([string]$Path, [string]$Key, [string]$Value) {
    $lines = [IO.File]::ReadAllLines($Path) | Where-Object { $_ -notmatch "^$Key=" }
    $lines = @($lines) + (Format-EnvLine $Key $Value)
    Write-FileAtomic $Path (($lines -join "`n") + "`n")
}

function Get-LanIp {
    try {
        $cfg = Get-NetIPConfiguration -ErrorAction Stop |
            Where-Object { $_.IPv4DefaultGateway -and $_.NetAdapter.Status -eq 'Up' } | Select-Object -First 1
        if ($cfg) { return @($cfg.IPv4Address)[0].IPAddress }
    } catch { Write-Verbose "no default-route adapter: $_" }
    return 'localhost'
}

# The address this machine reaches the published HTTP port on.
function Get-LocalHost {
    if (-not $script:ListenAddr -or $script:ListenAddr -in @('0.0.0.0', '[::]')) { return '127.0.0.1' }
    return $script:ListenAddr
}

function Get-Health([int]$HostPort) {
    try {
        return Invoke-RestMethod -Uri "http://$(Get-LocalHost):$HostPort/v1/health" -TimeoutSec 4 -UseBasicParsing
    } catch { return $null }
}

# --- docker ------------------------------------------------------------------------

function Test-DockerUp { return (Invoke-Native -Exe 'docker' -Arguments @('info') -Quiet) -eq 0 }

function Assert-Docker {
    $desktop = Join-Path $env:ProgramFiles 'Docker\Docker\Docker Desktop.exe'
    if (-not (Get-Command docker -ErrorAction SilentlyContinue)) {
        $bin = Join-Path $env:ProgramFiles 'Docker\Docker\resources\bin'
        if (Test-Path (Join-Path $bin 'docker.exe')) {
            $env:Path = "$bin;$env:Path"
        } else {
            Fail "Docker Desktop isn't installed. Install it from https://docs.docker.com/desktop/setup/install/windows-install/ (or: winget install -e --id Docker.DockerDesktop), start it, then re-run this installer."
        }
    }
    if (-not (Test-DockerUp)) {
        if (-not (Test-Path $desktop)) { Fail "Docker isn't running ('docker info' failed). Start Docker and re-run." }
        if (-not (Confirm-Choice "Docker Desktop isn't running. Start it now?" $true)) { Fail 'Docker Desktop must be running.' }
        Say 'Starting Docker Desktop (this can take a minute) ...'
        Start-Process -FilePath $desktop | Out-Null
        $deadline = (Get-Date).AddSeconds(180)
        while (-not (Test-DockerUp)) {
            if ((Get-Date) -gt $deadline) { Fail "Docker Desktop didn't come up within 3 minutes; start it by hand and re-run." }
            Start-Sleep -Seconds 3
        }
        Ok 'Docker Desktop is running'
    }
    $osType = Get-NativeOutput -Exe 'docker' -Arguments @('info', '--format', '{{.OSType}}')
    if ($osType -and $osType -ne 'linux') { Fail "Docker Desktop is in Windows-containers mode; switch it to Linux containers and re-run." }
    if ((Invoke-Native -Exe 'docker' -Arguments @('compose', 'version') -Quiet) -ne 0) {
        Fail "'docker compose' (Compose v2) isn't available; update Docker Desktop and re-run."
    }
}

# --- generated files -----------------------------------------------------------------

# -ListenSet publishes the HTTP port only on OURSHELL_LISTEN from .env;
# otherwise Docker's default (every IPv4 and IPv6 interface) applies.
function New-ComposeText([bool]$NewDb, [bool]$Tls, [bool]$ListenSet) {
    $t = @'
# OurShell sync server -- generated by the OurShell installer.
# Settings live in .env next to this file; the server's own configuration
# (admin password, sign-up policy, ...) is kept encrypted in the database and
# managed with the web setup wizard and /admin; its master key is in
# config.toml on the ourshell-data volume (keep a copy).
#
#   docker compose logs -f server    # logs
#   docker compose up -d             # start / apply .env changes
#   docker compose down              # stop (data is kept)
#
# To upgrade, re-run the installer (it rewrites this file).

services:
  server:
    image: ${OURSHELL_IMAGE:?set OURSHELL_IMAGE in .env}

'@
    $t += "    container_name: $script:ContainerName`n"
    $t += @'
    restart: unless-stopped
    env_file: .env
    environment:
      OURSHELL_DATA_DIR: /data
    volumes:
      - ourshell-data:/data
    ports:

'@
    if ($ListenSet) { $t += '      - "${OURSHELL_LISTEN:-0.0.0.0}:${OURSHELL_HTTP_PORT:-8443}:8443"' + "`n" }
    else { $t += '      - "${OURSHELL_HTTP_PORT:-8443}:8443"' + "`n" }
    if ($Tls) { $t += '      - "${OURSHELL_HTTPS_PORT:-3000}:8444"' + "`n" }
    if ($NewDb) {
        $t += @'
    depends_on:
      postgres:
        condition: service_healthy

  postgres:
    image: postgres:17
    restart: unless-stopped
    environment:
      POSTGRES_USER: ourshell
      POSTGRES_PASSWORD: ${POSTGRES_PASSWORD:?set POSTGRES_PASSWORD in .env}
      POSTGRES_DB: ourshell
    volumes:
      - ourshell-postgres-data:/var/lib/postgresql/data
    healthcheck:
      test: ["CMD-SHELL", "pg_isready -U ourshell -d ourshell"]
      interval: 5s
      timeout: 5s
      retries: 20

volumes:
  ourshell-data:
  ourshell-postgres-data:

'@
    } else {
        $t += @'
    extra_hosts:
      - "host.docker.internal:host-gateway"

volumes:
  ourshell-data:

'@
    }
    return $t
}

function Install-BackupScript {
    $dest = Join-Path $script:InstallDir 'backup.sh'
    # Run from a checkout: use the backup.sh next to this script. Under
    # `irm | iex` there's no script path, so download it.
    $self = Get-Variable -Name PSCommandPath -ValueOnly -ErrorAction SilentlyContinue
    $local = if ($self) { Join-Path (Split-Path -Parent $self) 'backup.sh' } else { $null }
    try {
        if ($local -and (Test-Path $local)) {
            Copy-Item -LiteralPath $local -Destination "$dest.tmp" -Force
        } else {
            Invoke-WebRequest -Uri $script:BackupUrl -OutFile "$dest.tmp" -UseBasicParsing -TimeoutSec 30
        }
        Move-Item -LiteralPath "$dest.tmp" -Destination $dest -Force
    } catch {
        Remove-Item -LiteralPath "$dest.tmp" -Force -ErrorAction SilentlyContinue
        Warn "couldn't download backup.sh from $($script:BackupUrl); get it later from the OurShell repository (scripts/backup.sh)."
    }
}

# --- database ----------------------------------------------------------------------

function Convert-LocalhostUrl([string]$Url) {
    return [regex]::Replace($Url, '^(postgres(ql)?://([^@/]*@)?)(localhost|127\.0\.0\.1|\[::1\])(?=[:/?]|$)', '${1}host.docker.internal')
}

function Test-ExistingDb([string]$Url) {
    Say 'Checking the database (with a temporary postgres:17 container) ...'
    $base = @('run', '--rm', '--add-host', 'host.docker.internal:host-gateway', 'postgres:17')
    if ((Invoke-Native -Exe 'docker' -Arguments ($base + @('pg_isready', '-t', '10', '-d', $Url)) -Quiet) -ne 0) {
        Warn "PostgreSQL at that address isn't accepting connections (from inside Docker)."
        if (-not (Confirm-Choice 'Continue anyway?' $false)) { Fail 'aborted; check the database URL and that PostgreSQL listens on an address containers can reach.' }
        return
    }
    $can = Get-NativeOutput -Exe 'docker' -Arguments ($base + @('psql', $Url, '-tAc', "SELECT has_database_privilege(current_database(), 'CREATE')"))
    if ($can -like 't*') { Ok 'database reachable; the role can create tables' }
    elseif ($can -like 'f*') { Warn 'the role in that URL lacks CREATE on the database; the server needs it to run its migrations (GRANT CREATE ON DATABASE <db> TO <role>).' }
    else { Warn 'database reachable, but logging in failed (wrong user/password or database name?). The setup wizard will show the exact error.' }
}

# --- health ------------------------------------------------------------------------

function Wait-Healthy([int]$HostPort) {
    Say "Waiting for the server to come up on port $HostPort ..."
    $deadline = (Get-Date).AddSeconds(120)
    while ((Get-Date) -lt $deadline) {
        $h = Get-Health $HostPort
        if ($h) {
            Ok ("health: " + ($h | ConvertTo-Json -Compress))
            if ($h.PSObject.Properties['mode']) { return [string]$h.mode }
            return 'ready'
        }
        Start-Sleep -Seconds 2
    }
    Warn "the server didn't answer on http://$(Get-LocalHost):$HostPort/v1/health within 120 seconds. Last log lines:"
    $null = Invoke-Compose @('logs', '--tail', '40', 'server')
    return $null
}

function Show-Box([string]$Mode, [int]$HostPort, [string]$Code, [string]$TlsDomain, [bool]$Proxy, [string]$PublicUrl) {
    $base = if ((Get-LocalHost) -eq '127.0.0.1' -and $script:ListenAddr -ne '127.0.0.1') { "http://$(Get-LanIp):$HostPort" } else { "http://$(Get-LocalHost):$HostPort" }
    $via = if ($PublicUrl -like 'https://*') { $PublicUrl.TrimEnd('/') } else { '' }
    $line = '=' * 66
    Write-Host ''
    Write-Host $line -ForegroundColor White
    if ($Mode -eq 'setup') {
        Write-Host ' OurShell sync server is running -- finish setup in your browser' -ForegroundColor White
        Write-Host $line -ForegroundColor White
        Write-Host ''
        Write-Host "  Open:        $base/setup#code=$Code"
        if ($Proxy -and $via) { Write-Host "               or through your proxy: $via/setup#code=$Code" }
        Write-Host "  Setup code:  $Code"
        Write-Host ''
        Write-Host '  The wizard asks for the database (already filled in), the address'
        Write-Host '  clients use, sign-up rules and an /admin password, then shows the'
        Write-Host "  server's master key: SAVE IT (backups can't be restored without it)."
    } else {
        Write-Host ' OurShell sync server is up' -ForegroundColor White
        Write-Host $line -ForegroundColor White
        Write-Host ''
        Write-Host "  Server:      $base"
        Write-Host "  Admin page:  $base/admin"
    }
    if ($Proxy) {
        Write-Host ''
        Write-Host "  Proxy:       point your reverse proxy at $base (plain HTTP);"
        Write-Host '               it handles HTTPS and must pass Host, X-Forwarded-For'
        Write-Host '               and X-Forwarded-Proto: https.'
        if ($via) { Write-Host "  Public URL:  $via" }
    } elseif ($TlsDomain) {
        Write-Host ''
        Write-Host "  HTTPS:       https://$TlsDomain (certificate is requested on first start)"
        if ($script:HttpsPort -ne 443) {
            Write-Host "               Let's Encrypt connects to port 443: forward public TCP 443 on"
            Write-Host "               your router/firewall to port $($script:HttpsPort) here, or the certificate"
            Write-Host "               cannot be issued. Without the forward: https://${TlsDomain}:$($script:HttpsPort)"
        }
    }
    Write-Host ''
    Write-Host "  Install dir: $($script:InstallDir)"
    Write-Host "  Logs:        cd `"$($script:InstallDir)`"; docker compose logs -f server"
    Write-Host "  Backup:      run backup.sh there from Git Bash or WSL (database dump + config.toml),"
    Write-Host "               or: docker cp $($script:ContainerName):/data/config.toml .  (keep it safe)"
    Write-Host '  Upgrade:     re-run this installer, or add -Upgrade'
    Write-Host '  Uninstall:   re-run with -Uninstall (add -Purge to delete all data)'
    Write-Host $line -ForegroundColor White
    Write-Host ''
}

function Update-ServerImage([bool]$NewDb, [string]$ImageRef) {
    Say 'Pulling images ...'
    $localOnly = -not $ImageRef.Contains('/')
    $haveLocal = (Invoke-Native -Exe 'docker' -Arguments @('image', 'inspect', $ImageRef) -Quiet) -eq 0
    if ($localOnly -and $haveLocal) {
        Say "Using the local image $ImageRef (not pulling)."
    } elseif ((Invoke-Compose @('pull', 'server')) -ne 0) {
        if ($haveLocal) { Warn "couldn't pull the server image; using the local copy." }
        else { Fail "couldn't pull the server image $ImageRef." }
    }
    if ($NewDb -and (Invoke-Compose @('pull', 'postgres')) -ne 0) { Warn "couldn't pull postgres:17; trying the local copy" }
}

# --- modes -----------------------------------------------------------------------

function Invoke-Uninstall {
    if (-not (Test-Path (Join-Path $script:InstallDir 'docker-compose.yml'))) { Fail "no OurShell install found in $($script:InstallDir) (pass -Dir)." }
    if ($Purge) {
        if (-not $Yes) {
            Warn "-Purge deletes the database, config.toml (with the master key) and everything in $($script:InstallDir)."
            if ((Read-Host "Type 'delete' to confirm") -ne 'delete') { Fail 'aborted.' }
        }
        Say 'Stopping OurShell and deleting its volumes ...'
        if ((Invoke-Compose @('down', '-v', '--remove-orphans')) -ne 0) { Fail "'docker compose down' failed." }
        Remove-Item -LiteralPath $script:InstallDir -Recurse -Force
        Ok "removed containers, volumes and $($script:InstallDir)"
    } else {
        Say "Stopping OurShell (data volumes and $($script:InstallDir) are kept) ..."
        if ((Invoke-Compose @('down', '--remove-orphans')) -ne 0) { Fail "'docker compose down' failed." }
        Ok 'removed containers; re-run the installer to start it again, or -Uninstall -Purge to delete the data'
    }
}

function Invoke-Upgrade {
    $envPath = Join-Path $script:InstallDir '.env'
    if (-not (Test-Path $envPath)) { Fail "$($script:InstallDir) has a docker-compose.yml but no .env; it wasn't set up by this installer." }
    $envMap = Read-EnvFile $envPath
    $old = $envMap['OURSHELL_IMAGE']
    if (-not $old) { Fail "$envPath has no OURSHELL_IMAGE; it wasn't set up by this installer." }
    if ($Image) { $new = $Image }
    elseif ($old.StartsWith("${ImageRepo}:")) { $new = $script:DefaultImage }
    else { $new = $old; Say "Keeping the custom image $old (pass -Image to change it)." }
    $hostPort = if ($envMap['OURSHELL_HTTP_PORT']) { [int]$envMap['OURSHELL_HTTP_PORT'] } else { 8443 }
    $newDb = [bool]$envMap['POSTGRES_PASSWORD']
    $tlsDomain = $envMap['OURSHELL_TLS_DOMAIN']
    $proxy = [string]$envMap['OURSHELL_BEHIND_PROXY'] -match '^(true|1|yes|on)$'
    # Behind a proxy the server ignores a TLS domain: don't publish HTTPS.
    if ($proxy) { $tlsDomain = '' }
    $script:HttpsPort = if ($envMap['OURSHELL_HTTPS_PORT']) { [int]$envMap['OURSHELL_HTTPS_PORT'] } else { 0 }
    if ($tlsDomain -and $script:HttpsPort -eq 0) {
        # Older installs published HTTPS on host port 443 without writing the
        # port down; the compose default is now 3000, so pin 443 for them
        # rather than silently moving their HTTPS port.
        $script:HttpsPort = 443
        Set-EnvKey -Path $envPath -Key 'OURSHELL_HTTPS_PORT' -Value '443'
    }
    $script:ListenAddr = [string]$envMap['OURSHELL_LISTEN']

    Say "Upgrading OurShell in $($script:InstallDir): $old -> $new"
    Set-EnvKey $envPath 'OURSHELL_IMAGE' $new
    Set-EnvKey $envPath 'COMPOSE_PROJECT_NAME' $script:ProjectName
    Protect-File $envPath
    $compose = Join-Path $script:InstallDir 'docker-compose.yml'
    Copy-Item -LiteralPath $compose -Destination "$compose.bak" -Force
    Write-FileAtomic $compose (New-ComposeText -NewDb $newDb -Tls ([bool]$tlsDomain) -ListenSet ([bool]$script:ListenAddr))
    Install-BackupScript
    Update-ServerImage $newDb $new
    if ((Invoke-Compose @('up', '-d', '--remove-orphans')) -ne 0) { Fail "'docker compose up' failed; the previous compose file is docker-compose.yml.bak." }
    $mode = Wait-Healthy $hostPort
    if (-not $mode) { Fail "the upgraded server isn't healthy; see the logs above. The previous compose file is docker-compose.yml.bak." }
    Show-Box $mode $hostPort $envMap['OURSHELL_SETUP_CODE'] $tlsDomain $proxy ([string]$envMap['PUBLIC_URL'])
}

function Invoke-Install {
    $db = $Db
    $dbUrlIn = $DatabaseUrl
    if (-not $db) {
        if ($dbUrlIn) { $db = 'existing' }
        else {
            Say 'OurShell stores its data in PostgreSQL.'
            $db = Ask 'Start a new PostgreSQL container, or use an existing server? (new/existing)' 'new'
        }
    }
    if ($db -notin @('new', 'existing')) { Fail "-Db must be 'new' or 'existing'" }
    $pgPassword = ''
    if ($db -eq 'existing') {
        if (-not $dbUrlIn) {
            if ($Yes) { Fail '-Db existing needs -DatabaseUrl postgres://user:password@host:5432/db' }
            $dbUrlIn = Ask 'PostgreSQL URL (postgres://user:password@host:5432/db)'
        }
        if ($dbUrlIn -notmatch '^postgres(ql)?://') { Fail 'the database URL must start with postgres:// or postgresql://' }
        $dbUrl = Convert-LocalhostUrl $dbUrlIn
        if ($dbUrl -ne $dbUrlIn) {
            Say "The server runs in a container, so localhost there isn't this machine: using host.docker.internal instead."
            Say 'Make sure PostgreSQL listens on an address containers can reach (listen_addresses, pg_hba.conf).'
        }
        Warn 'the database role must be allowed to CREATE in that database (the server creates its own tables).'
        Test-ExistingDb $dbUrl
    } else {
        $pgPassword = Get-RandomString 32 'ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789'
        $dbUrl = "postgres://ourshell:$pgPassword@postgres:5432/ourshell"
    }

    $hostPort = $Port
    if ($hostPort -le 0) { $hostPort = [int](Ask 'HTTP port on this machine' '8443') }
    if ($hostPort -lt 1 -or $hostPort -gt 65535) { Fail '-Port must be between 1 and 65535' }
    $tlsDomain = $Domain
    $tlsEmail = $Email
    $proxy = [bool]$BehindProxy
    $listenAddr = $Listen
    $trusted = $TrustedProxies
    $publicUrl = $PublicUrl
    if ($tlsDomain) { $https = '3' }
    elseif ($proxy) { $https = '2' }
    elseif ($Yes) { $https = '1' }
    else {
        Say 'How will clients reach the server?'
        Write-Host '  1) plain HTTP on your LAN or VPN (no HTTPS)'
        Write-Host '  2) behind my own reverse proxy / ingress / Cloudflare Tunnel, which handles'
        Write-Host '     HTTPS -- recommended if you already have one (Caddy, nginx, Traefik,'
        Write-Host "     Kubernetes, IIS, Synology's reverse proxy ...)"
        Write-Host "  3) built-in HTTPS with a Let's Encrypt certificate (needs a domain and"
        Write-Host '     public port 443 forwarded to this machine)'
        $https = Ask 'Choose 1, 2 or 3' '1'
    }
    switch ($https) {
        '1' { }
        '2' {
            $proxy = $true
            if (-not $listenAddr -and -not $Yes) {
                if (Confirm-Choice 'Does the proxy run directly on this machine (not in another container)? Then the server listens on 127.0.0.1 only' $true) { $listenAddr = '127.0.0.1' }
            }
            if (-not $publicUrl -and -not $Yes) {
                $publicUrl = Ask 'Public HTTPS address the proxy serves, e.g. https://sync.example.com (empty = decide in the setup wizard)'
            }
            if (-not $trusted -and -not $Yes) {
                $trusted = Ask 'Proxy IPs/CIDRs to trust for X-Forwarded-* headers, comma-separated (empty = loopback and private networks)'
            }
        }
        '3' {
            if (-not $tlsDomain) {
                $tlsDomain = Ask "Domain for the Let's Encrypt certificate (its DNS must point at this host)"
                if (-not $tlsDomain) { Fail 'built-in HTTPS needs a domain.' }
            }
            if ($HttpsPort -le 0 -and -not $Yes) {
                $script:HttpsPort = [int](Ask 'Host port for HTTPS (forward public TCP 443 on your router to it; 443 = no forward needed)' '3000')
            }
        }
        default { Fail 'please choose 1, 2 or 3.' }
    }
    if ($proxy -and -not $listenAddr) { $listenAddr = '0.0.0.0' }
    if ($listenAddr) {
        if ($listenAddr -notmatch '^[0-9A-Fa-f.:\[\]]+$') { Fail "-Listen takes an IP address such as 127.0.0.1 or 0.0.0.0 (got $listenAddr)" }
        if ($listenAddr.Contains(':') -and -not $listenAddr.StartsWith('[')) { $listenAddr = "[$listenAddr]" }
    }
    $script:ListenAddr = $listenAddr
    $trusted = ([string]$trusted) -replace '\s', ''
    if ($trusted -and $trusted -notmatch '^[0-9A-Fa-f.:/,]+$') { Fail '-TrustedProxies takes comma-separated IP addresses or CIDRs, e.g. 10.0.0.0/8,192.168.1.5' }
    if ($trusted -and -not $proxy) { Warn '-TrustedProxies only matters with -BehindProxy; ignoring it.'; $trusted = '' }
    if ($proxy -and $publicUrl -and $publicUrl -notlike 'https://*') {
        Warn "behind a proxy that handles HTTPS, the public URL is normally https://... (got $publicUrl)."
    }
    if ($tlsDomain) {
        if ($script:HttpsPort -le 0) { $script:HttpsPort = 3000 }
        if ($script:HttpsPort -gt 65535) { Fail '-HttpsPort must be between 1 and 65535' }
        if ($script:HttpsPort -eq $hostPort) { Fail "the HTTP and HTTPS ports must differ (both are $hostPort)." }
        if ($script:HttpsPort -ne 443) {
            Warn "Let's Encrypt always connects to port 443: forward public TCP 443 on your router/firewall to port $($script:HttpsPort) on this host, or the certificate can't be issued."
        }
    } elseif ($script:HttpsPort -gt 0) {
        Warn '-HttpsPort only matters with built-in HTTPS (-Domain); ignoring it.'
        $script:HttpsPort = 0
    }
    if ($tlsDomain -and -not $tlsEmail -and -not $Yes) { $tlsEmail = Ask "Email for Let's Encrypt expiry notices (optional)" }
    $defaultUrl = if ($tlsDomain) { "https://$tlsDomain" } else { '' }
    if (-not $publicUrl) {
        $publicUrl = if ($Yes) { $defaultUrl } else { Ask 'Address clients use to reach the server (empty = decide in the setup wizard)' $defaultUrl }
    }

    $code = New-SetupCode

    Say "Writing $($script:InstallDir) ..."
    New-Item -ItemType Directory -Force -Path $script:InstallDir | Out-Null
    $lines = @(
        '# OurShell sync server settings -- generated by the installer.',
        '# Secrets live here: keep this file private.',
        (Format-EnvLine 'COMPOSE_PROJECT_NAME' $script:ProjectName),
        (Format-EnvLine 'OURSHELL_IMAGE' $script:ImageRef),
        (Format-EnvLine 'OURSHELL_HTTP_PORT' "$hostPort"),
        (Format-EnvLine 'DATABASE_URL' $dbUrl)
    )
    if ($pgPassword) { $lines += Format-EnvLine 'POSTGRES_PASSWORD' $pgPassword }
    $lines += '# One-time code for the /setup wizard (unused once setup is done).'
    $lines += Format-EnvLine 'OURSHELL_SETUP_CODE' $code
    if ($publicUrl) { $lines += Format-EnvLine 'PUBLIC_URL' $publicUrl }
    if ($tlsDomain) { $lines += Format-EnvLine 'OURSHELL_TLS_DOMAIN' $tlsDomain }
    if ($tlsDomain) { $lines += Format-EnvLine 'OURSHELL_HTTPS_PORT' "$($script:HttpsPort)" }
    if ($tlsEmail) { $lines += Format-EnvLine 'OURSHELL_TLS_EMAIL' $tlsEmail }
    if ($proxy) {
        $lines += '# Behind a reverse proxy that terminates HTTPS (plain HTTP only here).'
        $lines += Format-EnvLine 'OURSHELL_BEHIND_PROXY' 'true'
        if ($trusted) { $lines += Format-EnvLine 'OURSHELL_TRUSTED_PROXIES' $trusted }
    }
    if ($listenAddr) {
        $lines += '# Host address the HTTP port is published on (127.0.0.1 = this machine only).'
        $lines += Format-EnvLine 'OURSHELL_LISTEN' $listenAddr
    }
    $lines += Format-EnvLine 'RUST_LOG' 'info'
    $envPath = Join-Path $script:InstallDir '.env'
    Write-FileAtomic $envPath (($lines -join "`n") + "`n")
    Protect-File $envPath
    $newDb = $db -eq 'new'
    Write-FileAtomic (Join-Path $script:InstallDir 'docker-compose.yml') (New-ComposeText -NewDb $newDb -Tls ([bool]$tlsDomain) -ListenSet ([bool]$listenAddr))
    Install-BackupScript
    Ok 'wrote docker-compose.yml, .env and backup.sh'

    Update-ServerImage $newDb $script:ImageRef
    Say 'Starting OurShell ...'
    if ((Invoke-Compose @('up', '-d', '--remove-orphans')) -ne 0) { Fail "'docker compose up' failed; see the output above." }
    $mode = Wait-Healthy $hostPort
    if (-not $mode) { Fail "the server didn't become healthy; see the logs above (docker compose logs server in $($script:InstallDir))." }
    Show-Box $mode $hostPort $code $tlsDomain $proxy $publicUrl
}

# --- main ------------------------------------------------------------------------

function Invoke-Main {
    if ($Purge -and -not $Uninstall) { Fail '-Purge only goes with -Uninstall' }
    if ($BehindProxy -and $Domain) {
        Fail "-BehindProxy and -Domain don't go together: behind a reverse proxy, the proxy handles HTTPS. Drop -Domain (and set up the certificate on your proxy), or drop -BehindProxy to use the built-in Let's Encrypt certificate."
    }
    $script:ListenAddr = ''
    $script:HttpsPort = $HttpsPort
    $script:ProjectName = if ($Project) { $Project } else { 'ourshell' }
    if ($script:ProjectName -notmatch '^[a-z0-9_-]+$') { Fail '-Project may only contain lowercase letters, digits, - and _' }
    # `ourshell-server` for the default project; an install under another
    # -Project (e.g. a test next to a real one) gets its own name.
    $script:ContainerName = if ($script:ProjectName -eq 'ourshell') { 'ourshell-server' } else { "$($script:ProjectName)-server" }

    # Release this script belongs to; a git checkout still has the placeholder.
    if ($OurShellVersion.StartsWith('__') -or -not $OurShellVersion) {
        $tag = 'latest'; $gitRef = 'main'
    } else {
        $tag = $OurShellVersion.TrimStart('v'); $gitRef = "v$tag"
    }
    $script:DefaultImage = "${ImageRepo}:$tag"
    $script:BackupUrl = "https://raw.githubusercontent.com/$Repo/$gitRef/scripts/backup.sh"

    if (-not $Yes -and -not (Test-Interactive)) {
        Fail "no console to ask questions on. Re-run with -Yes and options, e.g.: & ([scriptblock]::Create((irm https://github.com/$Repo/releases/latest/download/install-server.ps1))) -Yes -Db new"
    }

    Say "OurShell sync server installer (version $tag, Windows)"
    $installDir = $Dir
    if (-not $installDir) { $installDir = Ask 'Install directory' (Join-Path $env:ProgramData 'OurShell') }
    $script:InstallDir = [IO.Path]::GetFullPath($installDir)

    Assert-Docker

    if ($Uninstall) { Invoke-Uninstall; return }

    if (Test-Path (Join-Path $script:InstallDir 'docker-compose.yml')) {
        if (-not $Upgrade) {
            Say "Found an existing OurShell install in $($script:InstallDir)."
            if (-not (Confirm-Choice 'Upgrade it (pull the new image and restart; your data is kept)?' $true)) {
                Fail 'nothing changed. To reinstall from scratch, run with -Uninstall -Purge first.'
            }
        }
        Invoke-Upgrade
        return
    }
    if ($Upgrade) { Fail "no existing install in $($script:InstallDir) to upgrade (pass -Dir)." }

    $script:ImageRef = if ($Image) { $Image } else { $script:DefaultImage }
    Invoke-Install
}

Invoke-Main
