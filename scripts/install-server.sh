#!/bin/sh
# OurShell sync server installer for Linux, Synology DSM and macOS.
#
#   curl -fsSL https://github.com/NakharinA/OurShell/releases/latest/download/install-server.sh | sh
#
# Non-interactive (flags after `sh -s --`):
#
#   curl -fsSL .../install-server.sh | sh -s -- --yes --db new --port 8443
#
# It writes docker-compose.yml, .env and backup.sh into an install directory,
# starts the server (and, unless you point it at an existing database, a
# PostgreSQL container), then prints the address of the setup wizard and the
# one-time setup code. The wizard -- not this script -- generates the
# server's master key. Re-run it to upgrade. Run with --help for every flag.
#
# The whole script is one function called on the last line, so a truncated
# download can't run half of it.

set -eu

OURSHELL_VERSION="${OURSHELL_VERSION:-__OURSHELL_VERSION__}"

REPO="NakharinA/OurShell"
IMAGE_REPO="ghcr.io/nakharina/ourshell-server"

# --- output helpers ----------------------------------------------------------

if [ -t 1 ]; then
  B="$(printf '\033[1m')"; R="$(printf '\033[31m')"; Y="$(printf '\033[33m')"
  G="$(printf '\033[32m')"; N="$(printf '\033[0m')"
else
  B=""; R=""; Y=""; G=""; N=""
fi

say() { printf '%s==>%s %s\n' "$B" "$N" "$*"; }
ok() { printf '%s  ok%s %s\n' "$G" "$N" "$*"; }
warn() { printf '%swarning:%s %s\n' "$Y" "$N" "$*" >&2; }
die() { printf '%serror:%s %s\n' "$R" "$N" "$*" >&2; exit 1; }

usage() {
  cat <<'EOF'
OurShell sync server installer

Usage: install-server.sh [options]
       curl -fsSL https://github.com/NakharinA/OurShell/releases/latest/download/install-server.sh | sh -s -- [options]

Options (every option also has an interactive prompt):
  -y, --yes               Accept defaults, never prompt (needed without a terminal)
      --dir DIR           Install directory (default: /opt/ourshell on Linux,
                          /volume1/docker/ourshell on Synology, ~/ourshell on macOS)
      --db new|existing   Start a bundled PostgreSQL 17 (new, default) or use your own
      --database-url URL  postgres://user:password@host:5432/db (with --db existing);
                          localhost/127.0.0.1/::1 are rewritten to host.docker.internal
      --port PORT         Host port for the server's HTTP listener (default 8443)
      --listen ADDR       Host address the HTTP port is published on (default: every
                          interface); 127.0.0.1 = reachable only from this machine,
                          e.g. through a reverse proxy running here
      --behind-proxy      Run behind your own reverse proxy / ingress / Cloudflare
                          Tunnel, which handles HTTPS (plain HTTP only here)
      --trusted-proxies LIST
                          With --behind-proxy: comma-separated proxy IPs/CIDRs whose
                          X-Forwarded-For/-Proto are trusted (default: loopback and
                          private networks)
      --domain DOMAIN     Get a Let's Encrypt certificate for DOMAIN and serve HTTPS
                          (not with --behind-proxy). Let's Encrypt always connects to
                          port 443: forward public TCP 443 to --https-port
      --email EMAIL       Contact email for Let's Encrypt (with --domain)
      --https-port PORT   With --domain: host port for built-in HTTPS (default 3000)
      --public-url URL    Address clients use, e.g. https://sync.example.com
      --image REF         Server image (default ghcr.io/nakharina/ourshell-server:<version>)
      --project NAME      Docker Compose project name (default ourshell)
      --upgrade           Upgrade an existing install (pull the image, restart; data kept)
      --uninstall         Stop and remove the containers (volumes and files are kept)
      --purge             With --uninstall: also delete the data volumes and install dir
  -h, --help              Show this help

Environment: OURSHELL_VERSION picks the release (image tag) to install.
EOF
}

# --- small utilities ---------------------------------------------------------

have() { command -v "$1" >/dev/null 2>&1; }

# True when a terminal is available for prompts (also when stdin is a pipe).
# (A subshell: a failed redirection on a special builtin like `:` would exit
# a POSIX shell outright.)
has_tty() { (true </dev/tty) 2>/dev/null; }

# ask PROMPT DEFAULT -> sets ANSWER (the default with --yes or empty input).
ask() {
  ANSWER="$2"
  [ "$YES" = 1 ] && return 0
  if [ -n "$2" ]; then
    printf '%s [%s]: ' "$1" "$2" >/dev/tty
  else
    printf '%s: ' "$1" >/dev/tty
  fi
  IFS= read -r _reply </dev/tty || _reply=""
  [ -n "$_reply" ] && ANSWER="$_reply"
  return 0
}

# confirm PROMPT DEFAULT(y|n) -> exit status 0 for yes.
confirm() {
  if [ "$YES" = 1 ]; then return 0; fi
  if [ "$2" = y ]; then _hint="Y/n"; else _hint="y/N"; fi
  printf '%s [%s]: ' "$1" "$_hint" >/dev/tty
  IFS= read -r _reply </dev/tty || _reply=""
  [ -z "$_reply" ] && _reply="$2"
  case "$_reply" in [yY]*) return 0 ;; *) return 1 ;; esac
}

random_string() { # LENGTH CHARSET
  LC_ALL=C tr -dc "$2" </dev/urandom 2>/dev/null | head -c "$1"
}

# XXXX-XXXX-XXXX-XXXX, Crockford base32 (no I, L, O, U): 80 bits.
setup_code() {
  random_string 16 '0-9A-HJKMNP-TV-Z' | sed 's/\(....\)\(....\)\(....\)\(....\)/\1-\2-\3-\4/'
}

download() { # URL FILE
  if have curl; then
    curl -fsSL --retry 2 --connect-timeout 10 -o "$2" "$1"
  elif have wget; then
    wget -q -T 15 -O "$2" "$1"
  else
    return 1
  fi
}

http_get() { # URL -> body on stdout
  if have curl; then
    curl -fsS --max-time 4 "$1" 2>/dev/null
  elif have wget; then
    wget -q -T 4 -O - "$1" 2>/dev/null
  else
    return 2
  fi
}

# Values in .env are single-quoted unless they are plainly safe, so both
# `docker compose` and a POSIX shell (`backup.sh` sources .env) read them
# literally.
env_line() { # KEY VALUE
  case "$2" in
    *"'"*) die "$1 must not contain a single quote" ;;
    *[!A-Za-z0-9_.:/@%+=,-]*) printf "%s='%s'\n" "$1" "$2" ;;
    *) printf '%s=%s\n' "$1" "$2" ;;
  esac
}

env_get() { # FILE KEY -> value (quotes stripped), empty if missing
  sed -n "s/^$2=//p" "$1" | tail -n 1 | sed "s/^'\(.*\)'\$/\1/; s/^\"\(.*\)\"\$/\1/"
}

env_set() { # FILE KEY VALUE (replaces or appends, atomically)
  _tmp="$1.tmp.$$"
  { grep -v "^$2=" "$1" || true; env_line "$2" "$3"; } >"$_tmp"
  chmod 600 "$_tmp"
  mv -f "$_tmp" "$1"
}

lan_ip() {
  _ip=""
  if have hostname; then _ip="$(hostname -I 2>/dev/null | awk '{print $1}')" || _ip=""; fi
  if ! is_ipv4 "$_ip" && have ip; then
    _ip="$(ip route get 1 2>/dev/null | sed -n 's/.* src \([0-9.]*\).*/\1/p' | head -n 1)" || _ip=""
  fi
  if ! is_ipv4 "$_ip" && have ipconfig; then
    _ip="$(ipconfig getifaddr en0 2>/dev/null || ipconfig getifaddr en1 2>/dev/null)" || _ip=""
  fi
  is_ipv4 "$_ip" || _ip="localhost"
  printf '%s' "$_ip"
}

is_ipv4() {
  printf '%s' "$1" | grep -Eq '^[0-9]{1,3}(\.[0-9]{1,3}){3}$'
}

# --- platform, root and docker -----------------------------------------------

detect_platform() {
  if [ -f /etc/synoinfo.conf ]; then
    PLATFORM=synology
    PATH="/usr/local/bin:/usr/local/sbin:$PATH"; export PATH
    DEFAULT_DIR="/volume1/docker/ourshell"
  else
    case "$(uname -s 2>/dev/null)" in
      Linux) PLATFORM=linux; DEFAULT_DIR="/opt/ourshell" ;;
      Darwin) PLATFORM=macos; DEFAULT_DIR="$HOME/ourshell" ;;
      *) PLATFORM=other; DEFAULT_DIR="$HOME/ourshell" ;;
    esac
  fi
}

is_root() { [ "$(id -u 2>/dev/null || echo 1)" = 0 ]; }

# The nearest existing directory at or above $1.
existing_parent() {
  _d="$1"
  while [ ! -d "$_d" ]; do
    _d="$(dirname "$_d")"
  done
  printf '%s' "$_d"
}

# Re-runs this script under sudo with the options parsed so far.
reexec_sudo() { # REASON
  have sudo || die "$1, and sudo isn't available: run this installer as root."
  if [ -f "$0" ] && grep -q 'OurShell sync server installer' "$0" 2>/dev/null; then
    _script="$0"
  else
    # Piped into sh: fetch a copy of this script to re-run.
    _script="$(mktemp "${TMPDIR:-/tmp}/ourshell-install.XXXXXX")"
    download "$SCRIPT_URL" "$_script" || die "$1. Couldn't re-download the installer; run it again with: curl -fsSL $SCRIPT_URL | sudo sh"
  fi
  say "$1 -- re-running with sudo (you may be asked for your password)."
  set --
  [ "$YES" = 1 ] && set -- "$@" --yes
  [ -n "$DIR" ] && set -- "$@" --dir "$DIR"
  [ -n "$DB" ] && set -- "$@" --db "$DB"
  [ -n "$DATABASE_URL_IN" ] && set -- "$@" --database-url "$DATABASE_URL_IN"
  [ -n "$PORT" ] && set -- "$@" --port "$PORT"
  [ -n "$LISTEN" ] && set -- "$@" --listen "$LISTEN"
  [ "$BEHIND_PROXY" = 1 ] && set -- "$@" --behind-proxy
  [ -n "$TRUSTED_PROXIES" ] && set -- "$@" --trusted-proxies "$TRUSTED_PROXIES"
  [ -n "$DOMAIN" ] && set -- "$@" --domain "$DOMAIN"
  [ -n "$HTTPS_PORT" ] && set -- "$@" --https-port "$HTTPS_PORT"
  [ -n "$EMAIL" ] && set -- "$@" --email "$EMAIL"
  [ -n "$PUBLIC_URL_IN" ] && set -- "$@" --public-url "$PUBLIC_URL_IN"
  [ -n "$IMAGE" ] && set -- "$@" --image "$IMAGE"
  [ -n "$PROJECT" ] && set -- "$@" --project "$PROJECT"
  [ "$MODE" = upgrade ] && set -- "$@" --upgrade
  [ "$MODE" = uninstall ] && set -- "$@" --uninstall
  [ "$PURGE" = 1 ] && set -- "$@" --purge
  exec sudo env OURSHELL_VERSION="$OURSHELL_VERSION" sh "$_script" "$@"
}

choose_dir() {
  if [ -z "$DIR" ]; then
    if [ "$PLATFORM" = linux ] && ! is_root && [ "$YES" != 1 ]; then
      say "The default install directory is $DEFAULT_DIR, which needs root."
      ask "Install directory (or $HOME/ourshell to install without root)" "$DEFAULT_DIR"
      DIR="$ANSWER"
    elif [ "$PLATFORM" = linux ] && ! is_root && ! sudo -n true 2>/dev/null; then
      DIR="$HOME/ourshell"
      warn "not root and sudo needs a password: installing into $DIR"
    else
      ask "Install directory" "$DEFAULT_DIR"
      DIR="$ANSWER"
    fi
  fi
  case "$DIR" in /*) ;; [A-Za-z]:*) ;; *) DIR="$(pwd)/$DIR" ;; esac

  if is_root; then return 0; fi
  if [ "$PLATFORM" = synology ]; then
    reexec_sudo "Docker on Synology DSM needs root"
  fi
  if [ ! -w "$(existing_parent "$DIR")" ]; then
    reexec_sudo "$DIR isn't writable by $(id -un 2>/dev/null || echo you)"
  fi
}

install_docker() {
  [ "$PLATFORM" = linux ] || return 1
  confirm "Docker isn't installed. Install it now with the official script from https://get.docker.com?" y || return 1
  is_root || reexec_sudo "Installing Docker needs root"
  _tmp="$(mktemp "${TMPDIR:-/tmp}/get-docker.XXXXXX")"
  download https://get.docker.com "$_tmp" || die "couldn't download https://get.docker.com"
  sh "$_tmp"
  rm -f "$_tmp"
  if have systemctl; then systemctl enable --now docker >/dev/null 2>&1 || true; fi
}

check_docker() {
  if ! have docker; then
    case "$PLATFORM" in
      synology) die "Docker isn't installed: install Container Manager from the DSM Package Center, then re-run this installer." ;;
      macos) die "Docker isn't installed: install Docker Desktop (https://docs.docker.com/desktop/) or OrbStack, start it, then re-run this installer." ;;
      linux) install_docker || die "Docker is required: see https://docs.docker.com/engine/install/" ;;
      *) die "Docker isn't installed: install Docker, then re-run this installer." ;;
    esac
    have docker || die "Docker still isn't on PATH after installing it."
  fi
  if ! docker info >/dev/null 2>&1; then
    if ! is_root && [ -S /var/run/docker.sock ] && [ ! -w /var/run/docker.sock ]; then
      reexec_sudo "Your user can't talk to Docker (not in the docker group)"
    fi
    case "$PLATFORM" in
      synology) die "Docker isn't running: open Container Manager in DSM once, then re-run." ;;
      macos) die "Docker isn't running: start Docker Desktop (or OrbStack), then re-run." ;;
      *) die "Docker isn't running or isn't reachable ('docker info' failed). Start it (e.g. 'sudo systemctl start docker') and re-run." ;;
    esac
  fi
  if docker compose version >/dev/null 2>&1; then
    COMPOSE=v2
  elif have docker-compose; then
    COMPOSE=v1
  else
    die "Docker Compose v2 is required ('docker compose'). Install the docker-compose-plugin package (or Docker Desktop) and re-run."
  fi
}

# Docker Compose, run from the install directory for our project.
dc() {
  if [ "$COMPOSE" = v2 ]; then
    (cd "$DIR" && docker compose -p "$PROJECT" "$@")
  else
    (cd "$DIR" && docker-compose -p "$PROJECT" "$@")
  fi
}

# --- generated files ---------------------------------------------------------

# write_compose USE_NEW_DB(0|1) USE_TLS(0|1) LISTEN_SET(0|1)
# LISTEN_SET=1 publishes the HTTP port only on OURSHELL_LISTEN from .env;
# otherwise Docker's default (every IPv4 and IPv6 interface) applies.
write_compose() {
  _tmp="$DIR/docker-compose.yml.tmp.$$"
  {
    cat <<'EOF'
# OurShell sync server -- generated by the OurShell installer.
# Settings live in .env next to this file; the server's own configuration
# (master key, admin password, sign-up policy) lives in config.toml on the
# ourshell-data volume and is managed with the web setup wizard and /admin.
#
#   docker compose logs -f server    # logs
#   docker compose up -d             # start / apply .env changes
#   docker compose down              # stop (data is kept)
#
# To upgrade, re-run the installer (it rewrites this file).

services:
  server:
    image: ${OURSHELL_IMAGE:?set OURSHELL_IMAGE in .env}
EOF
    printf '    container_name: %s\n' "$CNAME"
    cat <<'EOF'
    restart: unless-stopped
    env_file: .env
    environment:
      OURSHELL_DATA_DIR: /data
    volumes:
      - ourshell-data:/data
    ports:
EOF
    # shellcheck disable=SC2016 # literal: Compose expands it
    if [ "$3" = 1 ]; then
      printf '%s\n' '      - "${OURSHELL_LISTEN:-0.0.0.0}:${OURSHELL_HTTP_PORT:-8443}:8443"'
    else
      printf '%s\n' '      - "${OURSHELL_HTTP_PORT:-8443}:8443"'
    fi
    if [ "$2" = 1 ]; then
      # shellcheck disable=SC2016 # literal: Compose expands it
      printf '%s\n' '      - "${OURSHELL_HTTPS_PORT:-3000}:8444"'
    fi
    if [ "$1" = 1 ]; then
      cat <<'EOF'
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
EOF
    else
      cat <<'EOF'
    extra_hosts:
      - "host.docker.internal:host-gateway"

volumes:
  ourshell-data:
EOF
    fi
  } >"$_tmp"
  mv -f "$_tmp" "$DIR/docker-compose.yml"
}

install_backup_script() {
  _here="$(dirname "$0")"
  if [ -f "$0" ] && [ -f "$_here/backup.sh" ] && grep -q 'OurShell' "$_here/backup.sh" 2>/dev/null; then
    cp "$_here/backup.sh" "$DIR/backup.sh.tmp.$$"
  elif ! download "$BACKUP_URL" "$DIR/backup.sh.tmp.$$"; then
    rm -f "$DIR/backup.sh.tmp.$$"
    warn "couldn't download backup.sh from $BACKUP_URL; get it later from the OurShell repository (scripts/backup.sh)."
    return 0
  fi
  chmod 755 "$DIR/backup.sh.tmp.$$"
  mv -f "$DIR/backup.sh.tmp.$$" "$DIR/backup.sh"
}

# --- database ----------------------------------------------------------------

rewrite_localhost() { # URL -> URL with a loopback host replaced by host.docker.internal
  printf '%s' "$1" | sed -E 's#^(postgres(ql)?://([^@/]*@)?)(localhost|127\.0\.0\.1|\[::1\])([:/?]|$)#\1host.docker.internal\5#'
}

check_existing_db() { # URL
  say "Checking the database (with a temporary postgres:17 container) ..."
  if ! docker run --rm --add-host host.docker.internal:host-gateway postgres:17 \
    pg_isready -t 10 -d "$1" >/dev/null 2>&1; then
    warn "PostgreSQL at that address isn't accepting connections (from inside Docker)."
    confirm "Continue anyway?" n || die "aborted; check the database URL and that PostgreSQL listens on an address containers can reach."
    return 0
  fi
  _can="$(docker run --rm --add-host host.docker.internal:host-gateway postgres:17 \
    psql "$1" -tAc "SELECT has_database_privilege(current_database(), 'CREATE')" 2>/dev/null)" || _can=""
  case "$_can" in
    t*) ok "database reachable; the role can create tables" ;;
    f*) warn "the role in that URL lacks CREATE on the database; the server needs it to run its migrations (GRANT CREATE ON DATABASE <db> TO <role>)." ;;
    *) warn "database reachable, but logging in failed (wrong user/password or database name?). The setup wizard will show the exact error." ;;
  esac
}

# --- health ------------------------------------------------------------------

# Waits up to ~120 s for /v1/health; sets HEALTH (the JSON) and HEALTH_MODE.
wait_healthy() {
  say "Waiting for the server to come up on port $PORT ..."
  HEALTH=""; HEALTH_MODE=""
  _i=0
  while [ "$_i" -lt 60 ]; do
    if HEALTH="$(http_get "http://$(local_host):$PORT/v1/health")"; then
      HEALTH_MODE="$(printf '%s' "$HEALTH" | sed -n 's/.*"mode" *: *"\([a-z]*\)".*/\1/p')"
      [ -n "$HEALTH_MODE" ] || HEALTH_MODE="ready"
      ok "health: $HEALTH"
      return 0
    elif [ $? = 2 ]; then
      # Neither curl nor wget: use the container's own healthcheck.
      _cid="$(dc ps -q server 2>/dev/null)" || _cid=""
      if [ -n "$_cid" ] && [ "$(docker inspect -f '{{if .State.Health}}{{.State.Health.Status}}{{end}}' "$_cid" 2>/dev/null)" = healthy ]; then
        HEALTH='{"status":"ok"}'
        HEALTH_MODE="$(dc logs --tail 200 server 2>/dev/null | grep -q 'not set up yet' && echo setup || echo ready)"
        ok "container reports healthy"
        return 0
      fi
    fi
    _i=$((_i + 1))
    sleep 2
  done
  warn "the server didn't answer on http://$(local_host):$PORT/v1/health within 120 seconds. Last log lines:"
  dc logs --tail 40 server >&2 || true
  return 1
}

# The address this machine reaches the published HTTP port on.
local_host() {
  case "$LISTEN" in
    ''|0.0.0.0|'[::]') printf '127.0.0.1' ;;
    *) printf '%s' "$LISTEN" ;;
  esac
}

print_box() { # MODE(setup|ready)
  case "$LISTEN" in
    ''|0.0.0.0|'[::]') _base="http://$(lan_ip):$PORT" ;;
    *) _base="http://$LISTEN:$PORT" ;;
  esac
  _via=""
  case "$PUBLIC_URL_IN" in https://*) _via="${PUBLIC_URL_IN%/}" ;; esac
  printf '\n%s' "$B"
  printf '%s\n' "=================================================================="
  if [ "$1" = setup ]; then
    printf '%s\n' " OurShell sync server is running -- finish setup in your browser"
    printf '%s\n' "=================================================================="
    printf '%s' "$N"
    printf '\n  Open:        %s/setup#code=%s\n' "$_base" "$CODE"
    if [ "$BEHIND_PROXY" = 1 ] && [ -n "$_via" ]; then
      printf '               or through your proxy: %s/setup#code=%s\n' "$_via" "$CODE"
    fi
    printf '  Setup code:  %s\n' "$CODE"
    printf '\n  The wizard asks for the database (already filled in), the address\n'
    printf '  clients use, sign-up rules and an /admin password, then shows the\n'
    printf "  server's master key: SAVE IT (backups can't be restored without it).\n"
  else
    printf '%s\n' " OurShell sync server is up"
    printf '%s\n' "=================================================================="
    printf '%s' "$N"
    printf '\n  Server:      %s\n' "$_base"
    printf '  Admin page:  %s/admin\n' "$_base"
  fi
  if [ "$BEHIND_PROXY" = 1 ]; then
    printf '\n  Proxy:       point your reverse proxy at %s (plain HTTP);\n' "$_base"
    printf '               it handles HTTPS and must pass Host, X-Forwarded-For\n'
    printf '               and X-Forwarded-Proto: https.\n'
    if [ -n "$_via" ]; then
      printf '  Public URL:  %s\n' "$_via"
    fi
  elif [ -n "$DOMAIN" ]; then
    printf '\n  HTTPS:       https://%s (certificate is requested on first start)\n' "$DOMAIN"
    if [ "$HTTPS_PORT" != 443 ]; then
      printf "               Let's Encrypt connects to port 443: forward public TCP 443 on\n"
      printf "               your router/firewall to port %s here, or the certificate\n" "$HTTPS_PORT"
      printf '               cannot be issued. Without the forward: https://%s:%s\n' "$DOMAIN" "$HTTPS_PORT"
    fi
  fi
  printf '\n  Install dir: %s\n' "$DIR"
  printf '  Logs:        cd %s && docker compose logs -f server\n' "$DIR"
  printf '  Backup:      cd %s && ./backup.sh   (database dump + config.toml)\n' "$DIR"
  printf '               keep config.toml safe too: docker cp %s:/data/config.toml .\n' "$CNAME"
  printf '  Upgrade:     re-run this installer, or add --upgrade\n'
  printf '  Uninstall:   re-run with --uninstall (add --purge to delete all data)\n'
  printf '%s%s%s\n\n' "$B" "==================================================================" "$N"
}

# --- modes -------------------------------------------------------------------

do_uninstall() {
  [ -f "$DIR/docker-compose.yml" ] || die "no OurShell install found in $DIR (pass --dir)."
  if [ "$PURGE" = 1 ]; then
    if [ "$YES" != 1 ]; then
      warn "--purge deletes the database, config.toml (with the master key) and everything in $DIR."
      ask "Type 'delete' to confirm" ""
      [ "$ANSWER" = delete ] || die "aborted."
    fi
    say "Stopping OurShell and deleting its volumes ..."
    dc down -v --remove-orphans
    rm -rf "$DIR"
    ok "removed containers, volumes and $DIR"
  else
    say "Stopping OurShell (data volumes and $DIR are kept) ..."
    dc down --remove-orphans
    ok "removed containers; re-run the installer in $DIR to start it again, or --uninstall --purge to delete the data"
  fi
}

do_upgrade() {
  _env="$DIR/.env"
  [ -f "$_env" ] || die "$DIR has a docker-compose.yml but no .env; it wasn't set up by this installer."
  _old="$(env_get "$_env" OURSHELL_IMAGE)"
  [ -n "$_old" ] || die "$_env has no OURSHELL_IMAGE; it wasn't set up by this installer."
  if [ -n "$IMAGE" ]; then
    _new="$IMAGE"
  else
    case "$_old" in
      "$IMAGE_REPO":*) _new="$DEFAULT_IMAGE" ;;
      *) _new="$_old"; say "Keeping the custom image $_old (pass --image to change it)." ;;
    esac
  fi
  PORT="$(env_get "$_env" OURSHELL_HTTP_PORT)"; PORT="${PORT:-8443}"
  DOMAIN="$(env_get "$_env" OURSHELL_TLS_DOMAIN)"
  CODE="$(env_get "$_env" OURSHELL_SETUP_CODE)"
  LISTEN="$(env_get "$_env" OURSHELL_LISTEN)"
  PUBLIC_URL_IN="$(env_get "$_env" PUBLIC_URL)"
  BEHIND_PROXY=0
  case "$(env_get "$_env" OURSHELL_BEHIND_PROXY)" in [tT]rue|TRUE|1|[yY]es|on) BEHIND_PROXY=1 ;; esac
  _newdb=0; [ -n "$(env_get "$_env" POSTGRES_PASSWORD)" ] && _newdb=1
  # Behind a proxy the server ignores a TLS domain: don't publish HTTPS.
  _tls=0; [ -n "$DOMAIN" ] && [ "$BEHIND_PROXY" != 1 ] && _tls=1
  HTTPS_PORT="$(env_get "$_env" OURSHELL_HTTPS_PORT)"
  if [ "$_tls" = 1 ] && [ -z "$HTTPS_PORT" ]; then
    # Older installs published HTTPS on host port 443 without writing the
    # port down; the compose default is now 3000, so pin 443 for them rather
    # than silently moving their HTTPS port.
    HTTPS_PORT=443
    env_set "$_env" OURSHELL_HTTPS_PORT 443
  fi
  _listen=0; [ -n "$LISTEN" ] && _listen=1

  say "Upgrading OurShell in $DIR: $_old -> $_new"
  env_set "$_env" OURSHELL_IMAGE "$_new"
  env_set "$_env" COMPOSE_PROJECT_NAME "$PROJECT"
  cp "$DIR/docker-compose.yml" "$DIR/docker-compose.yml.bak"
  write_compose "$_newdb" "$_tls" "$_listen"
  install_backup_script
  pull_images "$_newdb"
  dc up -d --remove-orphans
  wait_healthy || die "the upgraded server isn't healthy; see the logs above (cd $DIR && docker compose logs server). The previous compose file is docker-compose.yml.bak."
  print_box "$HEALTH_MODE"
}

pull_images() { # USE_NEW_DB
  say "Pulling images ..."
  _img="$(env_get "$DIR/.env" OURSHELL_IMAGE)"
  case "$_img" in
    */*) _local_only=0 ;;
    *) _local_only=1 ;; # e.g. ourshell-server:local, built on this machine
  esac
  if [ "$_local_only" = 1 ] && docker image inspect "$_img" >/dev/null 2>&1; then
    say "Using the local image $_img (not pulling)."
  elif ! dc pull server; then
    if docker image inspect "$_img" >/dev/null 2>&1; then
      warn "couldn't pull the server image; using the local copy."
    else
      die "couldn't pull the server image $_img."
    fi
  fi
  if [ "$1" = 1 ]; then dc pull postgres || warn "couldn't pull postgres:17; trying the local copy"; fi
}

# Validates LISTEN (an IP address; IPv6 gets the brackets Compose needs).
check_listen() {
  [ -n "$LISTEN" ] || return 0
  case "$LISTEN" in
    *[!0-9A-Fa-f.:\[\]]*) die "--listen takes an IP address such as 127.0.0.1 or 0.0.0.0 (got $LISTEN)" ;;
    \[*) ;;
    *:*) LISTEN="[$LISTEN]" ;;
  esac
}

do_install() {
  # Database.
  if [ -z "$DB" ]; then
    if [ -n "$DATABASE_URL_IN" ]; then DB=existing; else
      say "OurShell stores its data in PostgreSQL."
      ask "Start a new PostgreSQL container, or use an existing server? (new/existing)" new
      DB="$ANSWER"
    fi
  fi
  case "$DB" in new|existing) ;; *) die "--db must be 'new' or 'existing'" ;; esac
  DATABASE_URL=""; PG_PASSWORD=""
  if [ "$DB" = existing ]; then
    if [ -z "$DATABASE_URL_IN" ]; then
      [ "$YES" = 1 ] && die "--db existing needs --database-url postgres://user:password@host:5432/db"
      ask "PostgreSQL URL (postgres://user:password@host:5432/db)" ""
      DATABASE_URL_IN="$ANSWER"
    fi
    case "$DATABASE_URL_IN" in
      postgres://*|postgresql://*) ;;
      *) die "the database URL must start with postgres:// or postgresql://" ;;
    esac
    DATABASE_URL="$(rewrite_localhost "$DATABASE_URL_IN")"
    if [ "$DATABASE_URL" != "$DATABASE_URL_IN" ]; then
      say "The server runs in a container, so localhost there isn't this machine: using host.docker.internal instead."
      say "Make sure PostgreSQL listens on an address containers can reach (listen_addresses, pg_hba.conf)."
    fi
    warn "the database role must be allowed to CREATE in that database (the server creates its own tables)."
    check_existing_db "$DATABASE_URL"
  else
    PG_PASSWORD="$(random_string 32 'A-Za-z0-9')"
    [ "${#PG_PASSWORD}" = 32 ] || die "couldn't read random bytes from /dev/urandom"
    DATABASE_URL="postgres://ourshell:$PG_PASSWORD@postgres:5432/ourshell"
  fi

  # Network.
  if [ -z "$PORT" ]; then ask "HTTP port on this machine" 8443; PORT="$ANSWER"; fi
  case "$PORT" in ''|*[!0-9]*) die "--port must be a number" ;; esac
  if [ -n "$DOMAIN" ]; then
    _https=3
  elif [ "$BEHIND_PROXY" = 1 ]; then
    _https=2
  elif [ "$YES" = 1 ]; then
    _https=1
  else
    say "How will clients reach the server?"
    printf '  1) plain HTTP on your LAN or VPN (no HTTPS)\n' >/dev/tty
    printf '  2) behind my own reverse proxy / ingress / Cloudflare Tunnel, which handles\n' >/dev/tty
    printf '     HTTPS -- recommended if you already have one (Caddy, nginx, Traefik,\n' >/dev/tty
    printf "     Kubernetes, Synology's reverse proxy ...)\n" >/dev/tty
    printf "  3) built-in HTTPS with a Let's Encrypt certificate (needs a domain and\n" >/dev/tty
    printf '     public port 443 forwarded to this machine)\n' >/dev/tty
    ask "Choose 1, 2 or 3" 1
    _https="$ANSWER"
  fi
  case "$_https" in
    1) ;;
    2)
      BEHIND_PROXY=1
      if [ -z "$LISTEN" ] && [ "$YES" != 1 ]; then
        if confirm "Does the proxy run directly on this machine (not in another container)? Then the server listens on 127.0.0.1 only" y; then
          LISTEN=127.0.0.1
        fi
      fi
      if [ -z "$PUBLIC_URL_IN" ] && [ "$YES" != 1 ]; then
        ask "Public HTTPS address the proxy serves, e.g. https://sync.example.com (empty = decide in the setup wizard)" ""
        PUBLIC_URL_IN="$ANSWER"
      fi
      if [ -z "$TRUSTED_PROXIES" ] && [ "$YES" != 1 ]; then
        ask "Proxy IPs/CIDRs to trust for X-Forwarded-* headers, comma-separated (empty = loopback and private networks)" ""
        TRUSTED_PROXIES="$ANSWER"
      fi
      ;;
    3)
      if [ -z "$DOMAIN" ]; then
        ask "Domain for the Let's Encrypt certificate (its DNS must point at this host)" ""
        DOMAIN="$ANSWER"
        [ -n "$DOMAIN" ] || die "built-in HTTPS needs a domain."
      fi
      if [ -z "$HTTPS_PORT" ] && [ "$YES" != 1 ]; then
        ask "Host port for HTTPS (forward public TCP 443 on your router to it; 443 = no forward needed)" 3000
        HTTPS_PORT="$ANSWER"
      fi
      ;;
    *) die "please choose 1, 2 or 3." ;;
  esac
  [ "$BEHIND_PROXY" = 1 ] && [ -z "$LISTEN" ] && LISTEN=0.0.0.0
  check_listen
  TRUSTED_PROXIES="$(printf '%s' "$TRUSTED_PROXIES" | tr -d ' ')"
  case "$TRUSTED_PROXIES" in
    *[!0-9A-Fa-f.:/,]*) die "--trusted-proxies takes comma-separated IP addresses or CIDRs, e.g. 10.0.0.0/8,192.168.1.5" ;;
  esac
  if [ -n "$TRUSTED_PROXIES" ] && [ "$BEHIND_PROXY" != 1 ]; then
    warn "--trusted-proxies only matters with --behind-proxy; ignoring it."
    TRUSTED_PROXIES=""
  fi
  if [ "$BEHIND_PROXY" = 1 ]; then
    case "$PUBLIC_URL_IN" in
      ''|https://*) ;;
      *) warn "behind a proxy that handles HTTPS, the public URL is normally https://... (got $PUBLIC_URL_IN)." ;;
    esac
  fi
  if [ -n "$DOMAIN" ] && [ -z "$EMAIL" ] && [ "$YES" != 1 ]; then
    ask "Email for Let's Encrypt expiry notices (optional)" ""
    EMAIL="$ANSWER"
  fi
  if [ -n "$DOMAIN" ]; then
    HTTPS_PORT="${HTTPS_PORT:-3000}"
    case "$HTTPS_PORT" in ''|*[!0-9]*) die "--https-port must be a number" ;; esac
    [ "$HTTPS_PORT" = "$PORT" ] && die "the HTTP and HTTPS ports must differ (both are $PORT)."
    if [ "$HTTPS_PORT" != 443 ]; then
      warn "Let's Encrypt always connects to port 443: forward public TCP 443 on your router/firewall to port $HTTPS_PORT on this host, or the certificate can't be issued."
    elif [ "$PLATFORM" = synology ]; then
      warn "DSM's own web server usually holds port 443. Free it, or use the default HTTPS port 3000 and forward 443 to it on your router."
    fi
  elif [ -n "$HTTPS_PORT" ]; then
    warn "--https-port only matters with built-in HTTPS (--domain); ignoring it."
    HTTPS_PORT=""
  fi
  _default_url=""; [ -n "$DOMAIN" ] && _default_url="https://$DOMAIN"
  if [ -z "$PUBLIC_URL_IN" ]; then
    if [ "$YES" = 1 ]; then PUBLIC_URL_IN="$_default_url"; else
      ask "Address clients use to reach the server (empty = decide in the setup wizard)" "$_default_url"
      PUBLIC_URL_IN="$ANSWER"
    fi
  fi

  CODE="$(setup_code)"
  [ "${#CODE}" = 19 ] || die "couldn't generate a setup code"

  say "Writing $DIR ..."
  mkdir -p "$DIR"
  chmod 755 "$DIR" 2>/dev/null || true
  _tmp="$DIR/.env.tmp.$$"
  (
    umask 077
    {
      printf '# OurShell sync server settings -- generated by the installer.\n'
      printf '# Secrets live here: keep this file private (mode 600).\n'
      env_line COMPOSE_PROJECT_NAME "$PROJECT"
      env_line OURSHELL_IMAGE "$IMAGE"
      env_line OURSHELL_HTTP_PORT "$PORT"
      env_line DATABASE_URL "$DATABASE_URL"
      [ -n "$PG_PASSWORD" ] && env_line POSTGRES_PASSWORD "$PG_PASSWORD"
      printf '# One-time code for the /setup wizard (unused once setup is done).\n'
      env_line OURSHELL_SETUP_CODE "$CODE"
      [ -n "$PUBLIC_URL_IN" ] && env_line PUBLIC_URL "$PUBLIC_URL_IN"
      [ -n "$DOMAIN" ] && env_line OURSHELL_TLS_DOMAIN "$DOMAIN"
      [ -n "$DOMAIN" ] && env_line OURSHELL_HTTPS_PORT "$HTTPS_PORT"
      [ -n "$EMAIL" ] && env_line OURSHELL_TLS_EMAIL "$EMAIL"
      if [ "$BEHIND_PROXY" = 1 ]; then
        printf '# Behind a reverse proxy that terminates HTTPS (plain HTTP only here).\n'
        env_line OURSHELL_BEHIND_PROXY true
        [ -n "$TRUSTED_PROXIES" ] && env_line OURSHELL_TRUSTED_PROXIES "$TRUSTED_PROXIES"
      fi
      if [ -n "$LISTEN" ]; then
        printf '# Host address the HTTP port is published on (127.0.0.1 = this machine only).\n'
        env_line OURSHELL_LISTEN "$LISTEN"
      fi
      env_line RUST_LOG info
    } >"$_tmp"
  )
  chmod 600 "$_tmp"
  mv -f "$_tmp" "$DIR/.env"
  _newdb=0; [ "$DB" = new ] && _newdb=1
  _tls=0; [ -n "$DOMAIN" ] && _tls=1
  _listen=0; [ -n "$LISTEN" ] && _listen=1
  write_compose "$_newdb" "$_tls" "$_listen"
  install_backup_script
  ok "wrote docker-compose.yml, .env and backup.sh"

  pull_images "$_newdb"
  say "Starting OurShell ..."
  dc up -d --remove-orphans
  wait_healthy || die "the server didn't become healthy; see the logs above (cd $DIR && docker compose logs server)."
  print_box "$HEALTH_MODE"
}

# --- main --------------------------------------------------------------------

main() {
  YES=0; DIR=""; DB=""; DATABASE_URL_IN=""; PORT=""; DOMAIN=""; EMAIL=""; HTTPS_PORT=""
  PUBLIC_URL_IN=""; IMAGE=""; PROJECT=""; MODE=""; PURGE=0
  BEHIND_PROXY=0; TRUSTED_PROXIES=""; LISTEN=""

  while [ $# -gt 0 ]; do
    case "$1" in
      --*=*) _opt="${1%%=*}"; _val="${1#*=}"; shift; set -- "$_opt" "$_val" "$@"; continue ;;
    esac
    case "$1" in
      -y|--yes) YES=1 ;;
      --dir|--db|--database-url|--port|--domain|--email|--public-url|--image|--project|--listen|--trusted-proxies|--https-port)
        [ $# -ge 2 ] || die "$1 needs a value"
        case "$1" in
          --dir) DIR="$2" ;;
          --db) DB="$2" ;;
          --database-url) DATABASE_URL_IN="$2" ;;
          --port) PORT="$2" ;;
          --domain) DOMAIN="$2" ;;
          --email) EMAIL="$2" ;;
          --public-url) PUBLIC_URL_IN="$2" ;;
          --image) IMAGE="$2" ;;
          --project) PROJECT="$2" ;;
          --listen) LISTEN="$2" ;;
          --https-port) HTTPS_PORT="$2" ;;
          --trusted-proxies) TRUSTED_PROXIES="$2" ;;
        esac
        shift ;;
      --behind-proxy) BEHIND_PROXY=1 ;;
      --upgrade) MODE=upgrade ;;
      --uninstall) MODE=uninstall ;;
      --purge) PURGE=1 ;;
      -h|--help) usage; exit 0 ;;
      *) usage >&2; die "unknown option: $1" ;;
    esac
    shift
  done
  [ "$PURGE" = 1 ] && [ "$MODE" != uninstall ] && die "--purge only goes with --uninstall"
  if [ "$BEHIND_PROXY" = 1 ] && [ -n "$DOMAIN" ]; then
    die "--behind-proxy and --domain don't go together: behind a reverse proxy, the proxy handles HTTPS. Drop --domain (and set up the certificate on your proxy), or drop --behind-proxy to use the built-in Let's Encrypt certificate."
  fi
  PROJECT="${PROJECT:-ourshell}"
  case "$PROJECT" in *[!a-z0-9_-]*|'') die "--project may only contain lowercase letters, digits, - and _" ;; esac
  # `ourshell-server` for the default project; an install under another
  # --project (e.g. a test next to a real one) gets its own name.
  if [ "$PROJECT" = ourshell ]; then CNAME="ourshell-server"; else CNAME="$PROJECT-server"; fi

  # Release this script belongs to; a git checkout still has the placeholder.
  case "$OURSHELL_VERSION" in
    __*|'') TAG="latest"; GIT_REF="main"
      SCRIPT_URL="https://github.com/$REPO/releases/latest/download/install-server.sh" ;;
    *) OURSHELL_VERSION="${OURSHELL_VERSION#v}"; TAG="$OURSHELL_VERSION"; GIT_REF="v$OURSHELL_VERSION"
      SCRIPT_URL="https://github.com/$REPO/releases/download/v$OURSHELL_VERSION/install-server.sh" ;;
  esac
  DEFAULT_IMAGE="$IMAGE_REPO:$TAG"
  BACKUP_URL="https://raw.githubusercontent.com/$REPO/$GIT_REF/scripts/backup.sh"

  if [ "$YES" != 1 ] && ! has_tty; then
    die "no terminal to ask questions on. Re-run non-interactively, e.g.:
  curl -fsSL https://github.com/$REPO/releases/latest/download/install-server.sh | sh -s -- --yes --db new --port 8443
(see --help for every option)"
  fi

  detect_platform
  say "OurShell sync server installer (version ${TAG}, $PLATFORM)"
  choose_dir
  check_docker

  if [ "$MODE" = uninstall ]; then
    do_uninstall
    return 0
  fi

  if [ -f "$DIR/docker-compose.yml" ]; then
    if [ "$MODE" != upgrade ]; then
      say "Found an existing OurShell install in $DIR."
      confirm "Upgrade it (pull the new image and restart; your data is kept)?" y ||
        die "nothing changed. To reinstall from scratch, run with --uninstall --purge first."
    fi
    do_upgrade
    return 0
  fi
  [ "$MODE" = upgrade ] && die "no existing install in $DIR to upgrade (pass --dir)."

  IMAGE="${IMAGE:-$DEFAULT_IMAGE}"
  do_install
}

main "$@"
