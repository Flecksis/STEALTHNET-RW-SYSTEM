#!/bin/sh
set -eu

ROOT=$(cd "$(dirname "$0")" && pwd)
DEPLOY="$ROOT/deploy"
ENV_FILE="$DEPLOY/.env"
IMAGE_REPO=${SN_IMAGE_REPO:-ghcr.io/stealthnet-app/stealthnet-rw-system}
HEALTH_TIMEOUT=${SN_HEALTH_TIMEOUT:-300}
SITE_TIMEOUT=${SN_SITE_TIMEOUT:-90}
SNIPPET_FILE="$DEPLOY/proxy-snippet.txt"
WWW_HINT=""
SITE_READY=""
BIN_DIR=${SN_BIN_DIR:-/usr/local/bin}
WRAPPER="$BIN_DIR/stealthnet"
SHORT=""
USE_PREBUILT=0
TTY_SAVED=""
UPDATER_STATE=${SN_UPDATER_STATE:-/var/lib/stealthnet-rw}
SYSTEMD_DIR=${SN_SYSTEMD_DIR:-/etc/systemd/system}
APP_UID=${SN_UPDATER_APP_UID:-10001}
VERIFY_TIMEOUT=${SN_VERIFY_TIMEOUT:-90}
VERIFY_PAUSE=${SN_VERIFY_PAUSE:-2}
BACKUPS_DIR="$DEPLOY/backups"
PANEL_BACKUPS="$BACKUPS_DIR/panel"
APP_BACKUPS=/var/lib/stealthnet/backups
IPC_DIR="$UPDATER_STATE/ipc"
STATUS_DIR="$IPC_DIR/status"
REQUEST_DIR="$IPC_DIR/request"
RUN_DIR="$UPDATER_STATE/run"
CLI_UPDATE=""
MODULES_DIR="$UPDATER_STATE/modules"
MODULE_STATUS_DIR="$STATUS_DIR/modules"
MODULE_UID=${SN_MODULE_UID:-10100}
MODULES_NET=stealthnet-modules
MODULES_INNER=stealthnet-modules-internal
PANEL_ALIAS=stealthnet-panel
HELPER_CODE=""
M_ID=""
M_ACTION=""
M_MODULE=""
M_PACKAGE=""
M_DATA=""
M_STATE=""
M_ERROR=""
M_MESSAGE=null
M_STARTED=""
M_FINISHED=""
M_STEPS=""
M_MEMORY=""
M_DATA_BYTES=""
M_RESTORED=""
M_WORK=""
M_DIR=""
M_IMAGE=""
M_LIMIT=0

if [ -t 1 ] && [ -z "${NO_COLOR:-}" ]; then
    BOLD=$(printf '\033[1m')
    DIM=$(printf '\033[2m')
    GREEN=$(printf '\033[32m')
    RED=$(printf '\033[31m')
    RESET=$(printf '\033[0m')
else
    BOLD=""
    DIM=""
    GREEN=""
    RED=""
    RESET=""
fi

step() {
    printf '%s• %s%s\n' "$BOLD" "$1" "$RESET"
}

done_line() {
    printf '  %s%s%s\n' "$GREEN" "$1" "$RESET"
}

hint() {
    printf '  %s%s%s\n' "$DIM" "$1" "$RESET"
}

die() {
    printf '\n%sНе получилось: %s%s\n' "$RED" "$1" "$RESET" >&2
    shift
    for line in "$@"; do
        printf '%s\n' "$line" >&2
    done
    exit 1
}

detect_short() {
    SHORT=""
    if [ -f "$WRAPPER" ] && grep -Fq "\"$ROOT/install.sh\"" "$WRAPPER" 2>/dev/null; then
        case ":$PATH:" in
            *":$BIN_DIR:"*) SHORT=stealthnet ;;
            *) SHORT=$WRAPPER ;;
        esac
    fi
}

install_short() {
    case "$ROOT" in
        *\"*|*\$*|*\`*|*\\*) return 0 ;;
    esac
    if [ ! -d "$BIN_DIR" ] || [ ! -w "$BIN_DIR" ]; then
        return 0
    fi
    if [ -e "$WRAPPER" ] && ! grep -Fq 'install.sh" "$@"' "$WRAPPER" 2>/dev/null; then
        return 0
    fi
    if printf '#!/bin/sh\nexec "%s/install.sh" "$@"\n' "$ROOT" > "$WRAPPER" 2>/dev/null; then
        chmod 755 "$WRAPPER" 2>/dev/null || true
    fi
    detect_short
}

command_for() {
    if [ -n "$SHORT" ]; then
        if [ "$1" = "install" ]; then
            printf '%s' "$SHORT"
        else
            printf '%s %s' "$SHORT" "$1"
        fi
    elif [ "$1" = "install" ]; then
        printf '%s' "$ROOT/install.sh"
    else
        printf '%s --%s' "$ROOT/install.sh" "$1"
    fi
}

usage() {
    printf 'STEALTHNET RW SYSTEM\n\n'
    printf '  %s\n      установить или продолжить установку\n' "$(command_for install)"
    printf '  %s\n      указать или сменить домен панели\n' "$(command_for domain)"
    printf '  %s\n      выпустить сертификат HTTPS заново, если он не выпустился;\n      с --force — заменить уже выпущенный\n' "$(command_for cert)"
    printf '  %s\n      сменить почту владельца\n' "$(command_for owner-email)"
    printf '  %s\n      выдать владельцу новый пароль\n' "$(command_for reset-password)"
    printf '  %s\n      снять блокировку входа владельца, если его почту подбирают;\n      с --ip вход с этого адреса не закроется, даже пока подбор идёт\n' "$(command_for 'unlock-login [--ip адрес]')"
    printf '  %s\n      скачать новую версию и обновить систему\n' "$(command_for update)"
    printf '  %s\n      сделать резервную копию сейчас\n' "$(command_for backup)"
    printf '  %s\n      восстановить базу из резервной копии; части из Telegram (.part001…)\n      соберутся сами, пароль копии спросит или возьмёт из SN_BACKUP_PASSWORD\n' "$(command_for 'restore файл')"
    printf '  %s\n      показать версию, состояние и итог последнего обновления\n' "$(command_for status)"
    printf '  %s\n      разрешить или запретить обновление из админки\n' "$(command_for 'helper on|off')"
    printf '  %s\n      разрешить или запретить установку модулей из архива в админке;\n      без слова — список модулей\n' "$(command_for 'modules on|off')"
    printf '  %s\n      запустить систему, например после перезагрузки сервера\n' "$(command_for start)"
    printf '  %s\n      перезапустить систему\n' "$(command_for restart)"
    printf '  %s\n      остановить систему\n' "$(command_for stop)"
    printf '  %s\n      журнал приложения; с -f — следить\n' "$(command_for logs)"
    printf '\nБез вопросов: задайте SN_OWNER_EMAIL и, если нужно, SN_DOMAIN.\n'
    printf 'Собрать из исходников вместо готовой сборки: SN_BUILD=1 %s\n' "$(command_for install)"
}

can_ask() {
    [ -t 0 ] || ( : < /dev/tty ) 2>/dev/null
}

on_tty() {
    if [ -t 0 ]; then
        "$@"
    else
        "$@" < /dev/tty
    fi
}

restore_tty() {
    if [ -n "$TTY_SAVED" ]; then
        on_tty stty "$TTY_SAVED" 2>/dev/null || true
        TTY_SAVED=""
    fi
}

drain_input() {
    TTY_SAVED=$(on_tty stty -g 2>/dev/null) || TTY_SAVED=""
    if [ -z "$TTY_SAVED" ]; then
        return 0
    fi
    if on_tty stty -icanon -echo min 0 time 0 2>/dev/null; then
        while [ "$(on_tty dd bs=256 count=1 2>/dev/null | wc -c | tr -d ' ')" != "0" ]; do
            :
        done
    fi
    restore_tty
}

ask_hidden() {
    ANSWER=""
    if ! can_ask; then
        die "некому ввести пароль копии" \
            "Передайте его так: SN_BACKUP_PASSWORD='пароль' $(command_for 'restore файл')"
    fi
    drain_input
    printf '%s%s%s ' "$BOLD" "$1" "$RESET"
    TTY_SAVED=$(on_tty stty -g 2>/dev/null) || TTY_SAVED=""
    on_tty stty -echo 2>/dev/null || true
    if [ -t 0 ]; then
        IFS= read -r ANSWER || ANSWER=""
    else
        IFS= read -r ANSWER < /dev/tty || ANSWER=""
    fi
    restore_tty
    printf '\n'
}

ask() {
    ANSWER=""
    if ! can_ask; then
        die "некому ответить на вопрос «$1»" \
            "Запустите установку в терминале или задайте SN_OWNER_EMAIL и SN_DOMAIN."
    fi
    drain_input
    printf '%s%s%s ' "$BOLD" "$1" "$RESET"
    if [ -t 0 ]; then
        read -r ANSWER || ANSWER=""
    else
        read -r ANSWER < /dev/tty || ANSWER=""
    fi
    ANSWER=$(printf '%s' "$ANSWER" | tr -d '\r')
}

trap restore_tty EXIT
trap 'restore_tty; exit 130' INT TERM

env_value() {
    sed -n "s|^$1=||p" "$ENV_FILE" | tail -n 1
}

set_env() {
    key=$1
    value=$2
    if grep -q "^${key}=" "$ENV_FILE"; then
        tmp=$(mktemp)
        sed "s|^${key}=.*|${key}=${value}|" "$ENV_FILE" > "$tmp"
        cat "$tmp" > "$ENV_FILE"
        rm -f "$tmp"
    else
        printf '%s=%s\n' "$key" "$value" >> "$ENV_FILE"
    fi
}

random_hex() {
    head -c "$1" /dev/urandom | od -An -tx1 | tr -d ' \n'
}

random_base64() {
    head -c "$1" /dev/urandom | base64 | tr -d '\n'
}

random_password() {
    head -c 1024 /dev/urandom | LC_ALL=C tr -dc 'A-HJ-NP-Za-km-z2-9' | head -c 20
}

compose() {
    case "${1:-}" in
        up|run)
            if [ -f "$ENV_FILE" ]; then
                ensure_networks
            fi
            ;;
    esac
    (cd "$DEPLOY" && docker compose "$@")
}

database() {
    compose exec -T postgres psql -U stealthnet -d stealthnet -Atc "$1"
}

check_docker() {
    step "Docker"
    if ! command -v docker >/dev/null 2>&1; then
        if [ "$(uname -s)" = "Linux" ] && [ "$(id -u)" -eq 0 ] && command -v curl >/dev/null 2>&1; then
            ask "Docker не установлен. Установить сейчас? [Д/н]"
            case "$ANSWER" in
                n*|N*|н*|Н*) die "без Docker установка невозможна" ;;
            esac
            curl -fsSL https://get.docker.com | sh
        else
            die "Docker не установлен" \
                "Установите его командой: curl -fsSL https://get.docker.com | sudo sh" \
                "Затем запустите установку снова."
        fi
    fi
    if ! docker compose version >/dev/null 2>&1; then
        die "не найден Docker Compose" \
            "Обновите Docker: curl -fsSL https://get.docker.com | sudo sh"
    fi
    if ! docker info >/dev/null 2>&1; then
        die "Docker не отвечает" \
            "Если он не запущен: sudo systemctl start docker" \
            "Если не хватает прав: sudo $ROOT/install.sh"
    fi
    done_line "готов"
}

prepare_env() {
    step "Секреты"
    umask 077
    if [ ! -f "$ENV_FILE" ]; then
        cp "$DEPLOY/.env.example" "$ENV_FILE"
        set_env SN_PUBLIC_URL ""
    fi
    made=""
    if [ -z "$(env_value POSTGRES_PASSWORD)" ]; then
        set_env POSTGRES_PASSWORD "$(random_hex 24)"
        made="пароль базы"
    fi
    if [ -z "$(env_value SN_SECRET_KEY)" ]; then
        set_env SN_SECRET_KEY "$(random_base64 32)"
        made="${made:+$made и }ключ шифрования"
    fi
    chmod 600 "$ENV_FILE"
    if [ -n "$made" ]; then
        done_line "созданы: $made"
    else
        done_line "уже созданы, не меняю"
    fi
}

clean_domain() {
    printf '%s' "$1" | sed -e 's|^[[:space:]]*||' -e 's|[[:space:]]*$||' -e 's|^[a-zA-Z]*://||' -e 's|/.*$||'
}

valid_domain() {
    printf '%s' "$1" | grep -Eq '^[A-Za-z0-9]([A-Za-z0-9.-]*[A-Za-z0-9])?(:[0-9]+)?$'
}

ask_domain() {
    while :; do
        ask "Домен:"
        domain=$(clean_domain "$ANSWER")
        if [ -z "$domain" ]; then
            if [ "$1" = "optional" ]; then
                ask "Домен не введён. Продолжить без него? [д/Н]"
                case "$ANSWER" in
                    д*|Д*|y*|Y*) return 0 ;;
                esac
            fi
            continue
        fi
        if valid_domain "$domain"; then
            return 0
        fi
        hint "Не похоже на домен. Нужен вид example.com, без пробелов."
    done
}

prepare_domain() {
    step "Адрес панели"
    current=$(env_value SN_PUBLIC_URL)
    if [ -n "$current" ]; then
        done_line "$current"
        return 0
    fi
    if [ "${SN_DOMAIN+set}" = "set" ]; then
        domain=$(clean_domain "$SN_DOMAIN")
    elif can_ask; then
        hint "Основной домен сервиса, например example.com, а не поддомен: на нём откроется лендинг."
        hint "Админка и кабинет будут на том же домене: example.com/admin и example.com/cabinet."
        ask_domain optional
    else
        domain=""
    fi
    if [ -z "$domain" ]; then
        done_line "без домена; указать позже: $(command_for domain)"
        return 0
    fi
    if ! valid_domain "$domain"; then
        die "«$domain» не похоже на домен" "Нужен вид example.com."
    fi
    set_env SN_PUBLIC_URL "https://$domain"
    done_line "https://$domain"
}

port_busy() {
    if [ "${SN_TEST_BUSY_PORTS+set}" = "set" ]; then
        case " $SN_TEST_BUSY_PORTS " in
            *" $1 "*) return 0 ;;
        esac
        return 1
    fi
    if command -v ss >/dev/null 2>&1; then
        [ -n "$(ss -ltnH "sport = :$1" 2>/dev/null)" ]
    elif command -v lsof >/dev/null 2>&1; then
        lsof -nP -iTCP:"$1" -sTCP:LISTEN >/dev/null 2>&1
    elif command -v netstat >/dev/null 2>&1; then
        netstat -an 2>/dev/null | grep -Eq "[.:]$1[[:space:]].*LISTEN"
    else
        return 1
    fi
}

prepare_port() {
    if [ -n "$(env_value SN_HTTP_PORT)" ]; then
        return 0
    fi
    port=8080
    while port_busy "$port"; do
        port=$((port + 1))
        if [ "$port" -gt 8099 ]; then
            die "все порты с 8080 по 8099 заняты" \
                "Освободите один из них или впишите свободный порт в $ENV_FILE, строка SN_HTTP_PORT."
        fi
    done
    set_env SN_HTTP_PORT "$port"
    if [ "$port" -ne 8080 ]; then
        step "Порт"
        done_line "8080 занят другой программой, беру $port"
    fi
}

link_remnawave() {
    if [ "$(env_value SN_REMNAWAVE_NETWORK)" = "1" ]; then
        return 0
    fi
    case "$(env_value COMPOSE_FILE)" in
        *compose.remnawave.yml*)
            set_env SN_REMNAWAVE_NETWORK 1
            return 0
            ;;
    esac
    if docker network inspect remnawave-network >/dev/null 2>&1; then
        set_env SN_REMNAWAVE_NETWORK 1
        step "Remnawave"
        done_line "панель найдена на этом сервере, подключаюсь к её сети"
    fi
}

site_host() {
    url=$(env_value SN_PUBLIC_URL)
    printf '%s' "${url#https://}"
}

www_host() {
    case "$1" in
        [Ww][Ww][Ww].*|*:*) return 1 ;;
        *.*) ;;
        *) return 1 ;;
    esac
    case "$1" in
        *[!0-9.]*) printf 'www.%s' "$1" ;;
        *) return 1 ;;
    esac
}

host_addresses() {
    getent ahosts "$1" 2>/dev/null | awk '{ print $1 }' | sort -u
}

same_server() {
    command -v getent >/dev/null 2>&1 || return 1
    site_addrs=$(host_addresses "$1")
    www_addrs=$(host_addresses "$2")
    [ -n "$site_addrs" ] && [ -n "$www_addrs" ] || return 1
    for www_ip in $www_addrs; do
        for site_ip in $site_addrs; do
            [ "$www_ip" = "$site_ip" ] && return 0
        done
    done
    return 1
}

prepare_www() {
    WWW_HINT=""
    if [ "${SN_WWW+set}" = "set" ]; then
        set_env SN_WWW "$SN_WWW"
    fi
    www_name=$(www_host "$1") || www_name=""
    www_choice=$(env_value SN_WWW)
    if [ -z "$www_name" ] || [ "$www_choice" = "0" ]; then
        set_env SN_SITE_WWW ""
        return 0
    fi
    if [ "$www_choice" = "1" ]; then
        set_env SN_SITE_WWW "$www_name"
        return 0
    fi
    case "$1" in
        *.localhost)
            set_env SN_SITE_WWW ""
            return 0
            ;;
    esac
    if same_server "$1" "$www_name"; then
        set_env SN_SITE_WWW "$www_name"
    else
        set_env SN_SITE_WWW ""
        case "$1" in
            *.*.*) ;;
            *) WWW_HINT=$www_name ;;
        esac
    fi
}

site_lines() {
    printf '  Админка:           %s/admin\n' "$1"
    printf '  Кабинет клиентов:  %s/cabinet\n' "$1"
    www_site=$(env_value SN_SITE_WWW)
    if [ -n "$www_site" ]; then
        printf '  Переадресация:     https://%s → %s\n' "$www_site" "$1"
    fi
}

www_hint() {
    if [ -n "$WWW_HINT" ]; then
        printf '\nЧтобы открывался и %s, направьте его на этот сервер: такая же A-запись,\n' "$WWW_HINT"
        printf 'как у основного домена, или CNAME на него. Потом выполните %s ещё раз.\n' "$(command_for domain)"
    fi
}

write_snippet() {
    host=$1
    port=$(env_value SN_HTTP_PORT)
    www_site=$(env_value SN_SITE_WWW)
    dollar='$'
    {
        printf 'Caddy:\n\n%s {\n    reverse_proxy 127.0.0.1:%s\n}\n\n' "$host" "${port:-8080}"
        if [ -n "$www_site" ]; then
            printf '%s {\n    redir https://%s{uri} permanent\n}\n\n' "$www_site" "$host"
        fi
        printf 'nginx, внутри блока server с сертификатом для %s:\n\n' "$host"
        printf 'location / {\n'
        printf '    proxy_pass http://127.0.0.1:%s;\n' "${port:-8080}"
        printf '    client_max_body_size 20g;\n'
        printf '    proxy_request_buffering off;\n'
        printf '    proxy_set_header Host %shost;\n' "$dollar"
        printf '    proxy_set_header X-Forwarded-For %sproxy_add_x_forwarded_for;\n' "$dollar"
        printf '    proxy_set_header X-Forwarded-Proto %sscheme;\n' "$dollar"
        printf '}\n'
        if [ -n "$www_site" ]; then
            printf '\nnginx, отдельный блок server с сертификатом для %s:\n\n' "$www_site"
            printf 'return 301 https://%s%srequest_uri;\n' "$host" "$dollar"
        fi
    } > "$SNIPPET_FILE"
}

prepare_https() {
    host=$(site_host)
    if [ -z "$host" ]; then
        set_env SN_HTTPS ""
        set_env SN_SITE ""
        set_env SN_SITE_WWW ""
        return 0
    fi
    set_env SN_SITE "$host"
    prepare_www "$host"
    if [ "$(env_value SN_HTTPS)" = "caddy" ]; then
        prepare_firewall
        return 0
    fi
    step "HTTPS"
    case "$host" in
        *:*)
            set_env SN_HTTPS manual
            write_snippet "$host"
            done_line "адрес с портом: сертификат и прокси настраиваются отдельно, подсказка будет в конце"
            return 0
            ;;
    esac
    if port_busy 80 || port_busy 443; then
        set_env SN_HTTPS manual
        write_snippet "$host"
        done_line "порты 80 и 443 заняты другой программой, подсказка будет в конце"
    else
        set_env SN_HTTPS caddy
        done_line "сертификат выпустится сам при запуске"
        prepare_firewall
    fi
}

ufw_allows() {
    printf '%s\n' "$2" | awk -v port="$1" '
        /^Default:/ && /allow \(incoming\)/ { found = 1 }
        / ALLOW / {
            if ($0 ~ / Full /) { found = 1 }
            split($1, spec, "/")
            n = split(spec[1], ports, ",")
            for (i = 1; i <= n; i++) {
                if (ports[i] == port) { found = 1 }
                if (split(ports[i], range, ":") == 2 && port + 0 >= range[1] + 0 && port + 0 <= range[2] + 0) { found = 1 }
            }
        }
        END { exit found ? 0 : 1 }'
}

firewalld_allows() {
    case "$1" in
        80) service=http ;;
        *) service=https ;;
    esac
    case " $(firewall-cmd --list-services 2>/dev/null) " in
        *" $service "*) return 0 ;;
    esac
    case " $(firewall-cmd --list-ports 2>/dev/null) " in
        *" $1/tcp "*) return 0 ;;
    esac
    return 1
}

check_firewall() {
    FIREWALL=""
    CLOSED_PORTS=""
    ufw_out=""
    if command -v ufw >/dev/null 2>&1; then
        ufw_out=$(ufw status verbose 2>/dev/null || true)
    fi
    if printf '%s\n' "$ufw_out" | grep -q '^Status: active'; then
        for port in 80 443; do
            ufw_allows "$port" "$ufw_out" || CLOSED_PORTS="$CLOSED_PORTS $port"
        done
        CLOSED_PORTS=${CLOSED_PORTS# }
        if [ -n "$CLOSED_PORTS" ]; then
            FIREWALL=ufw
        fi
    elif command -v firewall-cmd >/dev/null 2>&1 && [ "$(firewall-cmd --state 2>/dev/null)" = "running" ]; then
        for port in 80 443; do
            firewalld_allows "$port" || CLOSED_PORTS="$CLOSED_PORTS $port"
        done
        CLOSED_PORTS=${CLOSED_PORTS# }
        if [ -n "$CLOSED_PORTS" ]; then
            FIREWALL=firewalld
        fi
    fi
}

open_ports_command() {
    case "$FIREWALL" in
        ufw) printf 'ufw allow 80/tcp && ufw allow 443/tcp' ;;
        firewalld) printf 'firewall-cmd --permanent --add-service=http --add-service=https && firewall-cmd --reload' ;;
    esac
}

open_ports() {
    case "$FIREWALL" in
        ufw) ufw allow 80/tcp >/dev/null 2>&1 && ufw allow 443/tcp >/dev/null 2>&1 ;;
        firewalld) firewall-cmd --permanent --add-service=http --add-service=https >/dev/null 2>&1 && firewall-cmd --reload >/dev/null 2>&1 ;;
        *) return 1 ;;
    esac
}

prepare_firewall() {
    check_firewall
    if [ -z "$FIREWALL" ]; then
        return 0
    fi
    step "Файрвол"
    case "$CLOSED_PORTS" in
        "80 443") hint "$FIREWALL закрывает порты 80 и 443: без них сертификат не выпустится и панель не откроется по HTTPS." ;;
        80) hint "$FIREWALL закрывает порт 80: без него сертификат HTTPS не выпустится." ;;
        *) hint "$FIREWALL закрывает порт 443: без него панель не откроется по HTTPS." ;;
    esac
    wanted=""
    if [ "${SN_OPEN_PORTS:-}" = "1" ]; then
        wanted=1
    elif [ "${SN_OPEN_PORTS:-}" != "0" ] && can_ask; then
        ask "Открыть порты 80 и 443 в $FIREWALL? [Д/н]"
        case "$ANSWER" in
            н*|Н*|n*|N*) ;;
            *) wanted=1 ;;
        esac
    fi
    if [ -n "$wanted" ] && open_ports; then
        done_line "порты 80 и 443 открыты в $FIREWALL"
        return 0
    fi
    hint "Откройте их командой: $(open_ports_command)"
}

write_compose_files() {
    files="docker-compose.yml"
    if [ "$(env_value SN_HTTPS)" = "caddy" ]; then
        files="$files:compose.https.yml"
    fi
    if [ "$(env_value SN_REMNAWAVE_NETWORK)" = "1" ]; then
        files="$files:compose.remnawave.yml"
    fi
    case "$(env_value SN_UPDATER)" in
        1|off)
            if [ -n "$(env_value SN_UPDATER_STATE)" ]; then
                files="$files:compose.modules.yml:compose.updater.yml"
            fi
            ;;
    esac
    set_env COMPOSE_FILE "$files"
}

check_site() {
    SITE_READY=""
    if [ "$(env_value SN_HTTPS)" != "caddy" ]; then
        return 0
    fi
    step "Сайт"
    host=$(site_host)
    waited=0
    while :; do
        if [ -n "$(compose exec -T caddy find /data/caddy/certificates -name "$host.crt" 2>/dev/null)" ]; then
            SITE_READY=1
            done_line "сертификат выпущен, https://$host открывается"
            return 0
        fi
        if [ "$waited" -ge "$SITE_TIMEOUT" ]; then
            SITE_READY=0
            hint "Сертификат пока не выпущен. Обычно причина одна из двух:"
            hint "домен ещё не указывает на этот сервер или порты 80 и 443 закрыты снаружи."
            cert_errors
            hint "Порты закрывает и файрвол в панели хостинга, если он там включён: его установщик не видит."
            hint "Когда это исправится, сертификат выпустится сам. Чтобы не ждать: $(command_for cert)"
            return 0
        fi
        sleep 5
        waited=$((waited + 5))
    done
}

server_addresses() {
    if [ "${SN_TEST_SERVER_IPS+set}" = "set" ]; then
        printf '%s' "$SN_TEST_SERVER_IPS"
        return 0
    fi
    hostname -I 2>/dev/null | tr -s ' \n' '  ' | sed 's/^ //; s/ $//'
}

show_dns() {
    dns=$(host_addresses "$1" | tr '\n' ' ' | sed 's/ $//')
    if [ -z "$dns" ]; then
        hint "$1 не находится в DNS: добавьте A-запись с адресом этого сервера."
        return 0
    fi
    done_line "$1 → $dns"
    own=$(server_addresses)
    [ -n "$own" ] || return 0
    for dns_ip in $dns; do
        case "$dns_ip" in
            127.*|::1) return 0 ;;
        esac
        case " $own " in
            *" $dns_ip "*) return 0 ;;
        esac
    done
    hint "Адреса этого сервера: $own. Если перед ним нет Cloudflare или NAT, исправьте A-запись."
}

has_cert() {
    [ -n "$(compose exec -T caddy find /data/caddy/certificates -name "$1.crt" 2>/dev/null)" ]
}

cert_reason() {
    case "$1" in
        *NXDOMAIN*|*"no valid A records"*|*"no valid AAAA records"*)
            printf 'домен не находится в DNS' ;;
        *"Timeout during connect"*|*"onnection refused"*)
            printf 'Let'"'"'s Encrypt не достучался до порта 80: его закрывает файрвол сервера или хостинга' ;;
        *rateLimited*|*"too many"*)
            printf 'Let'"'"'s Encrypt временно ограничил выпуск для этого домена, повторите позже' ;;
        *unauthorized*|*"Invalid response"*)
            printf 'проверку получил не этот сервер: домен указывает на другой адрес или перед сервером другой прокси' ;;
    esac
}

cert_errors() {
    last=$(compose logs --no-color --since 30m caddy 2>/dev/null | grep '"error"' | grep -iE 'certificate|challenge|acme|obtain' | sed -n 's/.*"error":"\([^"]*\)".*/\1/p' | tail -n 1 | cut -c1-240)
    [ -n "$last" ] || return 0
    reason=$(cert_reason "$last")
    if [ -n "$reason" ]; then
        hint "Причина по журналу Caddy: $reason."
    fi
    hint "Ошибка Caddy: $last"
}

reissue_cert() {
    printf '%sSTEALTHNET RW SYSTEM — сертификат HTTPS%s\n\n' "$BOLD" "$RESET"
    if [ ! -f "$ENV_FILE" ]; then
        die "система ещё не установлена" "Запустите $(command_for install)"
    fi
    force=""
    case "${1:-}" in
        "") ;;
        --force) force=1 ;;
        *) die "не понял «$1»" "Команда: $(command_for cert) или $(command_for cert) --force" ;;
    esac
    host=$(site_host)
    if [ -z "$host" ]; then
        die "домен не задан" "Сначала укажите его: $(command_for domain)"
    fi
    if [ "$(env_value SN_HTTPS)" != "caddy" ]; then
        die "сертификат для $host выпускает ваш веб-сервер" "Порты 80 и 443 держит другая программа, блок для неё — в $SNIPPET_FILE"
    fi
    www_site=$(env_value SN_SITE_WWW)
    step "Домен"
    show_dns "$host"
    if [ -n "$www_site" ]; then
        show_dns "$www_site"
    fi
    prepare_firewall
    step "Сертификат"
    if [ -n "$force" ]; then
        hint "Let's Encrypt выдаёт не больше 5 одинаковых сертификатов в неделю, без нужды --force не повторяйте."
        for name in "$host" $www_site; do
            compose exec -T caddy find /data/caddy/certificates -name "$name.*" -delete >/dev/null 2>&1 || true
        done
        done_line "прежний сертификат удалён"
    elif has_cert "$host" && { [ -z "$www_site" ] || has_cert "$www_site"; }; then
        done_line "уже выпущен, https://$host открывается"
        hint "Если браузер всё же ругается на сертификат, выпустите его заново: $(command_for cert) --force"
        return 0
    fi
    if ! compose restart caddy >/dev/null 2>&1; then
        die "веб-сервер не перезапустился" "Журнал: cd $DEPLOY && docker compose logs caddy"
    fi
    done_line "веб-сервер перезапущен, выпуск начат заново"
    check_site
    if [ "$SITE_READY" = "0" ]; then
        exit 1
    fi
    if [ -n "$www_site" ] && ! has_cert "$www_site"; then
        hint "Для $www_site сертификат пока не выпущен, переадресация с www заработает, когда он выпустится."
        cert_errors
    fi
}

host_arch() {
    case "$(docker info -f '{{.Architecture}}' 2>/dev/null)" in
        x86_64|amd64) printf 'amd64' ;;
        aarch64|arm64) printf 'arm64' ;;
        *) printf 'unknown' ;;
    esac
}

source_commit() {
    head_file="$ROOT/.git/HEAD"
    [ -f "$head_file" ] || return 0
    head=$(cat "$head_file")
    case "$head" in
        "ref: "*)
            ref=${head#ref: }
            if [ -f "$ROOT/.git/$ref" ]; then
                cat "$ROOT/.git/$ref"
            elif [ -f "$ROOT/.git/packed-refs" ]; then
                sed -n "s|^\([0-9a-f]\{40\}\) $ref\$|\1|p" "$ROOT/.git/packed-refs" | head -n 1
            fi
            ;;
        *) printf '%s' "$head" ;;
    esac
}

source_changed() {
    command -v git >/dev/null 2>&1 || return 1
    changes=$(git -C "$ROOT" status --porcelain --untracked-files=no 2>/dev/null) || return 1
    [ -n "$changes" ]
}

repo_digest() {
    docker image inspect -f '{{range .RepoDigests}}{{println .}}{{end}}' "$2" 2>/dev/null | grep -F "$1@sha256:" | sed -n '1p' || true
}

use_image() {
    docker pull "$1" 2>/dev/null || return 1
    if [ "$(docker image inspect -f '{{.Architecture}}' "$1" 2>/dev/null)" != "$(host_arch)" ]; then
        docker image rm "$1" >/dev/null 2>&1 || true
        WRONG_ARCH=1
        return 1
    fi
    pinned=$1
    if [ "${2:-}" = "pin" ]; then
        digest=$(repo_digest "${1%:*}" "$1")
        case "$digest" in
            *@sha256:*) pinned=$digest ;;
        esac
    fi
    set_env SN_IMAGE "$pinned"
    USE_PREBUILT=1
}

keep_installed() {
    chosen=$(env_value SN_IMAGE)
    if [ -z "$1" ] || [ "$1" = "$chosen" ] || ! docker image inspect "$1" >/dev/null 2>&1; then
        return 1
    fi
    if later_than "$(image_created "$1")" "$(image_created "$chosen")"; then
        set_env SN_IMAGE "$1"
        done_line "на сервере уже стоит сборка новее, оставляю её"
        return 0
    fi
    return 1
}

prepare_image() {
    step "Сборка"
    WRONG_ARCH=0
    if [ -n "${SN_BUILD:-}" ] || source_changed; then
        set_env SN_IMAGE ""
        done_line "соберу из исходников: это 10–20 минут"
        return 0
    fi
    hint "Скачиваю готовую сборку."
    before=$(env_value SN_IMAGE)
    repo=$(image_repo)
    commit=$(source_commit | tr -d ' \n')
    if printf '%s' "$commit" | grep -Eq '^[0-9a-f]{40}$' && use_image "$repo:sha-$commit"; then
        if keep_installed "$before"; then
            return 0
        fi
        done_line "готовая сборка скачана"
        return 0
    fi
    current=$(env_value SN_IMAGE)
    if [ -n "$current" ] && docker image inspect "$current" >/dev/null 2>&1; then
        USE_PREBUILT=1
        done_line "новой сборки пока нет, оставляю установленную"
        return 0
    fi
    if use_image "$repo:main" pin; then
        done_line "готовая сборка скачана"
        return 0
    fi
    set_env SN_IMAGE ""
    if [ "$WRONG_ARCH" -eq 1 ]; then
        hint "Готовая сборка есть только для другого процессора."
    fi
    done_line "готовой сборки для этого сервера нет, соберу из исходников: это 10–20 минут"
}

start_stack() {
    step "Запуск"
    if [ "$USE_PREBUILT" -eq 1 ]; then
        if ! compose up -d --no-build; then
            die "запуск не прошёл" \
                "Сообщение об ошибке — выше. После исправления запустите установку ещё раз: $(command_for install)" \
                "Она продолжится с этого места, созданные секреты сохранятся."
        fi
    else
        hint "Ниже ход сборки."
        SN_BUILD_COMMIT=""
        SN_BUILD_VERSION=""
        if ! source_changed; then
            SN_BUILD_COMMIT=$(source_commit | tr -d ' \n')
            SN_BUILD_VERSION=$(build_version)
        fi
        SN_BUILD_DATE=$(now_iso)
        export SN_BUILD_COMMIT SN_BUILD_DATE SN_BUILD_VERSION
        if ! compose up -d --build; then
            die "сборка или запуск не прошли" \
                "Сообщение об ошибке — выше. После исправления запустите установку ещё раз: $(command_for install)" \
                "Она продолжится с этого места, созданные секреты сохранятся."
        fi
    fi
}

wait_healthy() {
    step "Проверка"
    waited=0
    while :; do
        id=$(compose ps -q app 2>/dev/null || true)
        state=""
        if [ -n "$id" ]; then
            state=$(docker inspect -f '{{if .State.Health}}{{.State.Health.Status}}{{else}}{{.State.Status}}{{end}}' "$id" 2>/dev/null || true)
        fi
        if [ "$state" = "healthy" ]; then
            done_line "система запущена и отвечает"
            return 0
        fi
        if [ "$waited" -ge "$HEALTH_TIMEOUT" ]; then
            compose logs --tail 40 app >&2 || true
            die "система не ответила за $HEALTH_TIMEOUT секунд" \
                "Последние строки журнала — выше. Полный журнал: cd $DEPLOY && docker compose logs app"
        fi
        sleep 3
        waited=$((waited + 3))
    done
}

find_owner() {
    if ! EXISTING_OWNER=$(database "SELECT email FROM admins WHERE role = 'owner' ORDER BY id LIMIT 1"); then
        die "не удалось прочитать базу" "Журнал: cd $DEPLOY && docker compose logs postgres app"
    fi
}

read_email() {
    if [ -n "${SN_OWNER_EMAIL:-}" ]; then
        NEW_EMAIL=$SN_OWNER_EMAIL
    else
        hint "Почта владельца — это логин для входа в админку."
        while :; do
            ask "Почта:"
            NEW_EMAIL=$ANSWER
            case "$NEW_EMAIL" in
                *" "*|"") ;;
                *@*.*) break ;;
            esac
            hint "Не похоже на почту. Нужен вид name@example.com."
        done
    fi
    case "$NEW_EMAIL" in
        *" "*|"") die "почта владельца не задана" ;;
        *@*.*) ;;
        *) die "«$NEW_EMAIL» не похоже на почту" ;;
    esac
}

set_owner_password() {
    email=$1
    password=$(random_password)
    if [ "${#password}" -ne 20 ]; then
        die "не удалось создать пароль"
    fi
    if ! printf '%s\n' "$password" | compose exec -T app stealthnet create-admin --email "$email" >/dev/null; then
        die "не удалось сохранить владельца" "Журнал: cd $DEPLOY && docker compose logs app"
    fi
    OWNER_EMAIL=$email
    OWNER_PASSWORD=$password
}

commands_list() {
    printf '\n%s\n' "$1"
    printf '  %s\n      указать или сменить домен\n' "$(command_for domain)"
    printf '  %s\n      выпустить сертификат HTTPS заново\n' "$(command_for cert)"
    printf '  %s\n      сменить почту владельца\n' "$(command_for owner-email)"
    printf '  %s\n      выдать владельцу новый пароль\n' "$(command_for reset-password)"
    printf '  %s\n      обновить систему\n' "$(command_for update)"
    printf '  %s\n      состояние и итог последнего обновления\n' "$(command_for status)"
    printf '  %s\n      запустить, например после перезагрузки сервера\n' "$(command_for start)"
    printf '  %s\n      перезапустить\n' "$(command_for restart)"
}

summary() {
    url=$(env_value SN_PUBLIC_URL)
    port=$(env_value SN_HTTP_PORT)
    mode=$(env_value SN_HTTPS)
    printf '\n%sГотово.%s\n\n' "$BOLD" "$RESET"
    if [ -z "$url" ]; then
        printf '  Адрес панели:      не задан\n'
    elif [ "$mode" = "caddy" ] && [ "$SITE_READY" = "0" ]; then
        printf '  Адрес панели:      %s (сертификат ещё выпускается)\n' "$url"
    else
        printf '  Адрес панели:      %s\n' "$url"
    fi
    if [ -n "$url" ]; then
        site_lines "$url"
    fi
    printf '  Порт на сервере:   127.0.0.1:%s\n' "${port:-8080}"
    built=$(env_value SN_IMAGE)
    printf '  Версия:            %s\n' "$(image_name "${built:-stealthnet:local}")"
    if [ -n "${OWNER_PASSWORD:-}" ]; then
        printf '  Почта владельца:   %s\n' "$OWNER_EMAIL"
        printf '  Пароль владельца:  %s\n' "$OWNER_PASSWORD"
        printf '\n%sПароль показан один раз — сохраните его.%s\n' "$BOLD" "$RESET"
    elif [ -n "${OWNER_EMAIL:-}" ]; then
        printf '  Владелец:          %s (уже создан, пароль не менялся)\n' "$OWNER_EMAIL"
    fi
    if [ -n "$SHORT" ]; then
        commands_list "Дальше — этими командами, из любой папки:"
    else
        commands_list "Дальше — этими командами:"
    fi
    if [ -n "$url" ] && [ "$mode" = "manual" ]; then
        printf '\nЧтобы адрес заработал, добавьте в программу, которая держит порты 80 и 443,\n'
        printf 'блок из файла %s\n' "$SNIPPET_FILE"
    fi
    www_hint
    printf '\nСекреты лежат в %s. Сохраните копию этого файла:\n' "$ENV_FILE"
    printf 'без него не расшифровать ключи платёжных систем и токены из админки.\n'
}

install() {
    printf '%sSTEALTHNET RW SYSTEM — установка%s\n\n' "$BOLD" "$RESET"
    check_docker
    prepare_env
    prepare_domain
    prepare_port
    link_remnawave
    prepare_https
    prepare_helper
    write_compose_files
    prepare_backups
    OLD_IMAGE=$(env_value SN_IMAGE)
    RUNNING_APP=$(compose ps -q app 2>/dev/null || true)
    prepare_image
    if [ "$USE_PREBUILT" -eq 1 ] && [ -n "$RUNNING_APP" ] && [ "$(env_value SN_IMAGE)" != "$OLD_IMAGE" ]; then
        cli_update
    else
        start_stack
    fi
    wait_healthy
    check_site
    install_short || true
    step "Владелец"
    find_owner
    if [ -n "$EXISTING_OWNER" ]; then
        OWNER_EMAIL=$EXISTING_OWNER
        done_line "уже создан: $EXISTING_OWNER"
    else
        read_email
        set_owner_password "$NEW_EMAIL"
        done_line "создан, пароль сгенерирован"
    fi
    summary
}

reset_password() {
    printf '%sSTEALTHNET RW SYSTEM — новый пароль владельца%s\n\n' "$BOLD" "$RESET"
    if [ ! -f "$ENV_FILE" ]; then
        die "система ещё не установлена" "Запустите $(command_for install)"
    fi
    wait_healthy
    step "Владелец"
    find_owner
    if [ -z "$EXISTING_OWNER" ]; then
        die "владелец ещё не создан" "Запустите $(command_for install)"
    fi
    set_owner_password "$EXISTING_OWNER"
    done_line "пароль заменён, прежние сессии закрыты"
    printf '\n  Почта владельца:   %s\n' "$OWNER_EMAIL"
    printf '  Пароль владельца:  %s\n' "$OWNER_PASSWORD"
    printf '\n%sПароль показан один раз — сохраните его.%s\n' "$BOLD" "$RESET"
}

valid_ip() {
    case "$1" in
        ""|*[!0-9A-Fa-f.:]*) return 1 ;;
        *[.:]*) return 0 ;;
    esac
    return 1
}

unlock_login() {
    printf '%sSTEALTHNET RW SYSTEM — снять блокировку входа владельца%s\n\n' "$BOLD" "$RESET"
    need_installed
    left=$#
    while [ "$left" -gt 0 ]; do
        arg=$1
        shift
        left=$((left - 1))
        case "$arg" in
            --ip)
                if [ "$left" -eq 0 ]; then
                    die "после --ip нужен адрес" "Например: $(command_for unlock-login) --ip 203.0.113.7"
                fi
                address=$1
                shift
                left=$((left - 1))
                ;;
            --ip=*) address=${arg#--ip=} ;;
            *)
                die "не понял «$arg»" \
                    "Снять блокировку: $(command_for unlock-login)" \
                    "И сделать свой адрес привычным: $(command_for unlock-login) --ip 203.0.113.7"
                ;;
        esac
        if ! valid_ip "$address"; then
            die "«$address» не похоже на IP-адрес" "Нужен вид 203.0.113.7 или 2001:db8::7"
        fi
        set -- "$@" --ip "$address"
    done
    wait_healthy
    step "Владелец"
    find_owner
    if [ -z "$EXISTING_OWNER" ]; then
        die "владелец ещё не создан" "Запустите $(command_for install)"
    fi
    if ! report=$(compose exec -T app stealthnet unlock-login --email "$EXISTING_OWNER" "$@"); then
        die "не удалось снять блокировку" "Журнал: cd $DEPLOY && docker compose logs app"
    fi
    printf '%s\n' "$report" | while IFS= read -r line; do
        printf '  %s\n' "$line"
    done
}

change_domain() {
    printf '%sSTEALTHNET RW SYSTEM — домен панели%s\n\n' "$BOLD" "$RESET"
    if [ ! -f "$ENV_FILE" ]; then
        die "система ещё не установлена" "Запустите $(command_for install)"
    fi
    step "Адрес панели"
    current=$(env_value SN_PUBLIC_URL)
    if [ -n "$current" ]; then
        hint "Сейчас: $current"
    fi
    if [ "${SN_DOMAIN+set}" = "set" ]; then
        domain=$(clean_domain "$SN_DOMAIN")
    else
        hint "Основной домен сервиса, например example.com, а не поддомен: на нём откроется лендинг."
        hint "Админка и кабинет будут на том же домене: example.com/admin и example.com/cabinet."
        ask_domain required
    fi
    if ! valid_domain "$domain"; then
        die "«$domain» не похоже на домен" "Нужен вид example.com."
    fi
    set_env SN_PUBLIC_URL "https://$domain"
    done_line "https://$domain"
    link_remnawave
    prepare_https
    write_compose_files
    step "Запуск"
    if ! compose up -d --no-build --remove-orphans; then
        die "перезапуск не прошёл" "Сообщение об ошибке — выше."
    fi
    wait_healthy
    check_site
    mode=$(env_value SN_HTTPS)
    if [ "$mode" = "caddy" ] && [ "$SITE_READY" = "0" ]; then
        printf '\n  Адрес панели:      https://%s (сертификат ещё выпускается)\n' "$domain"
    else
        printf '\n  Адрес панели:      https://%s\n' "$domain"
    fi
    site_lines "https://$domain"
    if [ "$mode" = "manual" ]; then
        printf '\nЧтобы адрес заработал, добавьте в программу, которая держит порты 80 и 443,\n'
        printf 'блок из файла %s\n' "$SNIPPET_FILE"
    fi
    www_hint
}

change_owner_email() {
    printf '%sSTEALTHNET RW SYSTEM — почта владельца%s\n\n' "$BOLD" "$RESET"
    if [ ! -f "$ENV_FILE" ]; then
        die "система ещё не установлена" "Запустите $(command_for install)"
    fi
    wait_healthy
    step "Владелец"
    find_owner
    if [ -z "$EXISTING_OWNER" ]; then
        die "владелец ещё не создан" "Запустите $(command_for install)"
    fi
    hint "Сейчас: $EXISTING_OWNER"
    read_email
    query="UPDATE admins SET email = :'email' WHERE id = (SELECT id FROM admins WHERE role = 'owner' ORDER BY id LIMIT 1);"
    if ! printf '%s\n' "$query" | compose exec -T postgres psql -U stealthnet -d stealthnet -q -v ON_ERROR_STOP=1 -v "email=$NEW_EMAIL" -f - >/dev/null 2>&1; then
        die "не удалось сменить почту" "Возможно, эта почта уже занята другим администратором."
    fi
    done_line "почта заменена, пароль прежний"
    printf '\n  Почта владельца:   %s\n' "$NEW_EMAIL"
}

update() {
    printf '%sSTEALTHNET RW SYSTEM — обновление%s\n\n' "$BOLD" "$RESET"
    step "Новая версия"
    if ! command -v git >/dev/null 2>&1 || [ ! -d "$ROOT/.git" ]; then
        die "эта команда обновляет только установку, сделанную через git clone"
    fi
    if ! git -C "$ROOT" pull --ff-only; then
        die "не удалось скачать новую версию" "Сообщение об ошибке — выше."
    fi
    done_line "скачана"
    exec "$ROOT/install.sh"
}

now_iso() {
    date -u +%Y-%m-%dT%H:%M:%SZ
}

iso_epoch() {
    date -u -d "$1" +%s 2>/dev/null || TZ=UTC date -j -f '%Y-%m-%dT%H:%M:%SZ' "$1" +%s 2>/dev/null || true
}

json_str() {
    if [ -n "$1" ]; then
        printf '"%s"' "$1"
    else
        printf 'null'
    fi
}

json_num() {
    if [ -n "$1" ]; then
        printf '%s' "$1"
    else
        printf 'null'
    fi
}

image_repo() {
    if [ -n "${SN_IMAGE_REPO:-}" ]; then
        printf '%s' "$SN_IMAGE_REPO"
    elif [ -f "$ENV_FILE" ] && [ -n "$(env_value SN_IMAGE_REPO)" ]; then
        env_value SN_IMAGE_REPO
    else
        printf '%s' "$IMAGE_REPO"
    fi
}

image_label() {
    docker image inspect -f "{{index .Config.Labels \"$2\"}}" "$1" 2>/dev/null | sed -n '1p' || true
}

build_version() {
    command -v git >/dev/null 2>&1 || return 0
    [ "$(git -C "$ROOT" rev-parse --is-shallow-repository 2>/dev/null || true)" = "false" ] || return 0
    count=$(git -C "$ROOT" rev-list --count HEAD 2>/dev/null || true)
    case "$count" in
        ""|*[!0-9]*) return 0 ;;
    esac
    base=$(sed -n 's/^version = "\([0-9][0-9]*\.[0-9][0-9]*\)\.[0-9][0-9]*"$/\1/p' "$ROOT/Cargo.toml" 2>/dev/null | sed -n '1p')
    if [ -n "$base" ]; then
        printf '%s.%s' "$base" "$count"
    fi
    return 0
}

image_version() {
    version=$(image_label "$1" org.opencontainers.image.version)
    if printf '%s' "$version" | grep -Eqx '[0-9]{1,9}\.[0-9]{1,9}\.[0-9]{1,9}'; then
        printf '%s' "$version"
    fi
    return 0
}

ru_date() {
    printf '%.10s' "$1" | sed 's/^\([0-9][0-9]*\)-\([0-9][0-9]*\)-\([0-9][0-9]*\)$/\3.\2.\1/'
}

image_name() {
    version=$(image_version "$1")
    created=$(image_created "$1")
    if [ -n "$version" ] && [ -n "$created" ]; then
        printf '%s от %s' "$version" "$(ru_date "$created")"
    elif [ -n "$version" ]; then
        printf '%s' "$version"
    elif [ -n "$created" ]; then
        printf 'сборка от %s' "$(ru_date "$created")"
    else
        printf 'сборка без номера'
    fi
}

image_commit() {
    rev=$(image_label "$1" org.opencontainers.image.revision)
    if printf '%s' "$rev" | grep -Eqx '[0-9a-f]{40}'; then
        printf '%s' "$rev"
    fi
    return 0
}

image_created() {
    created=$(image_label "$1" org.opencontainers.image.created)
    case "$created" in
        ""|"<no value>") created=$(docker image inspect -f '{{.Created}}' "$1" 2>/dev/null | sed -n '1p' || true) ;;
    esac
    printf '%.19s' "$created"
}

later_than() {
    [ -n "$1" ] && [ -n "$2" ] && [ "$1" != "$2" ] \
        && [ "$(printf '%s\n%s\n' "$1" "$2" | LC_ALL=C sort | tail -n 1)" = "$1" ]
}

owned_by_root() {
    info=$(ls -ldn "$1" 2>/dev/null) || return 1
    set -- $info
    [ "${3:-}" = "0" ] || return 1
    case "${1:-}" in
        ?????w*|????????w*) return 1 ;;
    esac
    return 0
}

helper_possible() {
    HELPER_WHY=""
    HELPER_CODE=""
    if [ -n "${SN_HELPER_TEST:-}" ]; then
        return 0
    fi
    if [ "$(id -u)" -ne 0 ]; then
        HELPER_WHY="нужны права root"
        HELPER_CODE=no_root
        return 1
    fi
    if [ ! -d /run/systemd/system ] || ! command -v systemctl >/dev/null 2>&1; then
        HELPER_WHY="на сервере нет systemd"
        HELPER_CODE=no_systemd
        return 1
    fi
    case "$ROOT$UPDATER_STATE" in
        *[!A-Za-z0-9._/-]*)
            HELPER_WHY="в пути к папке системы есть пробелы или особые знаки"
            HELPER_CODE=path
            return 1
            ;;
    esac
    if ! owned_by_root "$ROOT" || ! owned_by_root "$ROOT/install.sh" || ! owned_by_root "$DEPLOY"; then
        HELPER_WHY="папку $ROOT может менять не только root"
        HELPER_CODE=owner
        return 1
    fi
    return 0
}

helper_blocked() {
    sed -n 's/.*"blocked":"\([0-9a-f]\{40\}\)".*/\1/p' "$STATUS_DIR/helper.json" 2>/dev/null | sed -n '1p' || true
}

write_helper_json() {
    enabled=$1
    blocked=${2-$(helper_blocked)}
    modules=false
    if [ "$enabled" = "true" ] && modules_on; then
        modules=true
    fi
    tmp="$STATUS_DIR/.helper.json.$$"
    printf '{"schema":2,"panel_updates":%s,"modules":%s,"blocked":%s}\n' "$enabled" "$modules" "$(json_str "$blocked")" > "$tmp"
    chmod 644 "$tmp"
    mv -f "$tmp" "$STATUS_DIR/helper.json"
}

helper_enabled() {
    grep -q '"panel_updates":true' "$STATUS_DIR/helper.json" 2>/dev/null
}

write_units() {
    mkdir -p "$SYSTEMD_DIR"
    {
        printf '[Unit]\nDescription=STEALTHNET RW: заявки на обновление из админки\n\n'
        printf '[Path]\nPathExists=%s/request.json\nUnit=stealthnet-rw-update.service\n\n' "$REQUEST_DIR"
        printf '[Install]\nWantedBy=multi-user.target\n'
    } > "$SYSTEMD_DIR/stealthnet-rw-update.path"
    {
        printf '[Unit]\nDescription=STEALTHNET RW: обновление\nAfter=docker.service\nWants=docker.service\nStartLimitIntervalSec=0\n\n'
        printf '[Service]\nType=oneshot\nEnvironment=SN_UPDATER_STATE=%s\nEnvironment=HOME=/root\n' "$UPDATER_STATE"
        printf 'ExecStart=/bin/sh %s/install.sh updater run\n' "$ROOT"
        printf 'ExecStopPost=/bin/sh %s/install.sh updater recover\n' "$ROOT"
        printf 'TimeoutStartSec=30min\nUMask=0077\nNoNewPrivileges=yes\nPrivateTmp=yes\n'
    } > "$SYSTEMD_DIR/stealthnet-rw-update.service"
    {
        printf '[Unit]\nDescription=STEALTHNET RW: завершение прерванного обновления\nAfter=docker.service\nWants=docker.service\n\n'
        printf '[Service]\nType=oneshot\nEnvironment=SN_UPDATER_STATE=%s\nEnvironment=HOME=/root\n' "$UPDATER_STATE"
        printf 'ExecStart=/bin/sh %s/install.sh updater recover\n\n' "$ROOT"
        printf '[Install]\nWantedBy=multi-user.target\n'
    } > "$SYSTEMD_DIR/stealthnet-rw-recover.service"
    chmod 644 "$SYSTEMD_DIR/stealthnet-rw-update.path" "$SYSTEMD_DIR/stealthnet-rw-update.service" "$SYSTEMD_DIR/stealthnet-rw-recover.service"
}

helper_dirs() {
    mkdir -p "$REQUEST_DIR" "$STATUS_DIR" "$RUN_DIR" "$MODULE_STATUS_DIR" "$MODULES_DIR"
    chmod 755 "$UPDATER_STATE" "$IPC_DIR" "$STATUS_DIR"
    chmod 700 "$RUN_DIR" "$REQUEST_DIR" "$MODULE_STATUS_DIR" "$MODULES_DIR"
    if [ -z "${SN_HELPER_TEST:-}" ]; then
        chown "$APP_UID:$APP_UID" "$MODULE_STATUS_DIR"
        chown "$APP_UID:$APP_UID" "$REQUEST_DIR"
    fi
}

helper_on() {
    helper_possible || return 1
    helper_dirs
    write_helper_json true
    write_units
    set_env SN_UPDATER_STATE "$UPDATER_STATE"
    set_env SN_UPDATER 1
    if [ -n "${SN_IMAGE_REPO:-}" ]; then
        set_env SN_IMAGE_REPO "$SN_IMAGE_REPO"
    fi
    write_compose_files
    ensure_networks
    if ! systemctl daemon-reload >/dev/null 2>&1 \
        || ! systemctl enable --now stealthnet-rw-update.path >/dev/null 2>&1 \
        || ! systemctl enable stealthnet-rw-recover.service >/dev/null 2>&1; then
        HELPER_WHY="systemd не принял службу обновления"
        HELPER_CODE=systemd_failed
        write_helper_json false
        return 1
    fi
    return 0
}

prepare_helper() {
    case "$(env_value SN_UPDATER)" in
        off)
            step "Обновление из админки"
            done_line "выключено. Включить: $(command_for 'helper on')"
            return 0
            ;;
    esac
    step "Обновление из админки"
    if helper_on; then
        set_env SN_HELPER_WHY ""
        done_line "включено, обновлять может только владелец с паролем"
        if modules_on; then
            done_line "модули из архива ставит владелец в админке"
        fi
    else
        set_env SN_HELPER_WHY "$HELPER_CODE"
        hint "не включено: $HELPER_WHY. Обновлять можно командой $(command_for update)"
    fi
}

helper_command() {
    printf '%sSTEALTHNET RW SYSTEM — обновление из админки%s\n\n' "$BOLD" "$RESET"
    if [ ! -f "$ENV_FILE" ]; then
        die "система ещё не установлена" "Запустите $(command_for install)"
    fi
    case "${1:-}" in
        on)
            set_env SN_UPDATER ""
            if ! helper_on; then
                die "не включилось: $HELPER_WHY"
            fi
            step "Перезапуск"
            if ! compose up -d --no-build; then
                die "перезапуск не прошёл" "Сообщение об ошибке — выше."
            fi
            wait_healthy
            done_line "Обновление из админки включено. Обновлять сможет только владелец, с паролем."
            ;;
        off)
            if helper_possible || [ "$(id -u)" -eq 0 ]; then
                systemctl disable --now stealthnet-rw-update.path >/dev/null 2>&1 || true
                systemctl disable stealthnet-rw-recover.service >/dev/null 2>&1 || true
                rm -f "$SYSTEMD_DIR/stealthnet-rw-update.path" "$SYSTEMD_DIR/stealthnet-rw-update.service" "$SYSTEMD_DIR/stealthnet-rw-recover.service"
                systemctl daemon-reload >/dev/null 2>&1 || true
            else
                die "нужны права root" "Запустите: sudo $(command_for 'helper off')"
            fi
            if [ -d "$STATUS_DIR" ]; then
                write_helper_json false
                rm -f "$REQUEST_DIR/request.json"
            fi
            set_env SN_UPDATER off
            done_line "Обновление из админки выключено. Обновлять можно командой: sudo $(command_for update)"
            ;;
        *)
            die "нужно слово on или off" "Включить: $(command_for 'helper on')" "Выключить: $(command_for 'helper off')"
            ;;
    esac
}

log_line() {
    printf '%s %s\n' "$(now_iso)" "$1" >> "$RUN_DIR/updater.log" 2>/dev/null || true
}

pg() {
    compose exec -T postgres psql -U stealthnet -d postgres -v ON_ERROR_STOP=1 -Atq -c "$1"
}

pg_retry() {
    tries=0
    while ! pg "$1" >/dev/null 2>&1; do
        tries=$((tries + 1))
        if [ "$tries" -ge 5 ]; then
            return 1
        fi
        sleep 1
    done
    return 0
}

db_exists() {
    [ "$(pg "SELECT 1 FROM pg_database WHERE datname = '$1'" 2>/dev/null || true)" = "1" ]
}

db_label() {
    pg "SELECT coalesce(shobj_description(oid, 'pg_database'), '') FROM pg_database WHERE datname = '$1'" 2>/dev/null || true
}

save_run() {
    {
        for key in U_ID U_ORIGIN U_ADMIN U_FROM U_TO U_FROM_V U_TO_V U_STATE U_ERROR U_STARTED U_FINISHED U_BACKUP START_EPOCH DOWN_EPOCH FROM_IMAGE TO_IMAGE \
            S_check S_download S_backup S_migrate S_start S_verify T_check T_download T_backup T_migrate T_start T_verify; do
            eval "value=\${$key:-}"
            printf '%s=%s\n' "$key" "$value"
        done
    } > "$RUN_DIR/current.tmp"
    mv -f "$RUN_DIR/current.tmp" "$RUN_DIR/current"
}

load_run() {
    while IFS='=' read -r key value; do
        case "$key" in
            U_ID|U_ORIGIN|U_ADMIN|U_FROM|U_TO|U_FROM_V|U_TO_V|U_STATE|U_ERROR|U_STARTED|U_FINISHED|U_BACKUP|START_EPOCH|DOWN_EPOCH|FROM_IMAGE|TO_IMAGE|S_*|T_*)
                case "$value" in
                    *[!A-Za-z0-9:._@/+-]*) value="" ;;
                esac
                eval "$key=\$value"
                ;;
        esac
    done < "$RUN_DIR/current"
}

reset_run() {
    U_ID=""
    U_ORIGIN=""
    U_ADMIN=""
    U_FROM=""
    U_TO=""
    U_FROM_V=""
    U_TO_V=""
    U_STATE=""
    U_ERROR=""
    U_STARTED=""
    U_FINISHED=""
    U_BACKUP=""
    START_EPOCH=""
    DOWN_EPOCH=""
    FROM_IMAGE=""
    TO_IMAGE=""
    for key in check download backup migrate start verify; do
        eval "S_$key=pending; T_$key=''; B_$key=''"
    done
}

status_json() {
    printf '{"schema":1,"id":%s,"origin":"%s","admin_id":%s,"from":%s,"to":%s,"from_version":%s,"to_version":%s,"state":"%s","steps":[' \
        "$(json_str "$U_ID")" "$U_ORIGIN" "$(json_num "$U_ADMIN")" "$(json_str "$U_FROM")" "$(json_str "$U_TO")" "$(json_str "$U_FROM_V")" "$(json_str "$U_TO_V")" "$U_STATE"
    sep=""
    for key in check download backup migrate start verify; do
        eval "st=\${S_$key:-pending}; sec=\${T_$key:-}"
        printf '%s{"key":"%s","state":"%s","seconds":%s}' "$sep" "$key" "$st" "$(json_num "$sec")"
        sep=","
    done
    seconds=""
    downtime=""
    if [ -n "$U_FINISHED" ] && [ -n "$START_EPOCH" ]; then
        seconds=$(( $(date +%s) - START_EPOCH ))
    fi
    if [ -n "$DOWN_EPOCH" ]; then
        downtime=$(( ${UP_EPOCH:-$(date +%s)} - DOWN_EPOCH ))
    fi
    printf '],"started_at":%s,"finished_at":%s,"error":%s,"seconds":%s,"downtime":%s,"backup":%s}\n' \
        "$(json_str "$U_STARTED")" "$(json_str "$U_FINISHED")" "$(json_str "$U_ERROR")" "$(json_num "$seconds")" "$(json_num "$downtime")" "$(json_str "$U_BACKUP")"
}

write_status() {
    save_run
    if [ ! -d "$STATUS_DIR" ]; then
        return 0
    fi
    tmp="$STATUS_DIR/.update.json.$$"
    status_json > "$tmp"
    chmod 644 "$tmp"
    mv -f "$tmp" "$STATUS_DIR/update.json"
}

append_history() {
    if [ ! -d "$STATUS_DIR" ]; then
        return 0
    fi
    file="$STATUS_DIR/history.jsonl"
    tmp="$STATUS_DIR/.history.$$"
    {
        if [ -f "$file" ]; then
            tail -n 19 "$file"
        fi
        status_json
    } > "$tmp"
    chmod 644 "$tmp"
    mv -f "$tmp" "$file"
}

step_begin() {
    eval "S_$1=running; B_$1=$(date +%s)"
    if [ -n "${CLI_UPDATE:-}" ]; then
        case "$1" in
            check) step "Проверка выпуска" ;;
            download) step "Загрузка" ;;
            backup) step "Резервная копия" ;;
            migrate) step "Обновление базы" ;;
            start) step "Запуск новой версии" ;;
            verify) step "Проверка работы" ;;
        esac
    fi
    write_status
}

step_end() {
    eval "began=\${B_$1:-}"
    eval "S_$1=$2; T_$1=$(( $(date +%s) - ${began:-$(date +%s)} ))"
    if [ -n "${CLI_UPDATE:-}" ] && [ "$2" = "done" ]; then
        eval "spent=\$T_$1"
        done_line "готово за $spent с"
    fi
    write_status
}

journal() {
    printf '%s\n' "$1" >> "$RUN_DIR/journal"
    sync 2>/dev/null || true
}

journal_has() {
    grep -qx "$1" "$RUN_DIR/journal" 2>/dev/null
}

fail_step() {
    FAIL_KIND=$1
    FAIL_CODE=$2
    for key in check download backup migrate start verify; do
        eval "st=\${S_$key:-pending}"
        if [ "$st" = "running" ]; then
            step_end "$key" failed
        fi
    done
    log_line "заявка $U_ID: шаг не прошёл, код $FAIL_CODE"
    return 1
}

lock_take() {
    lockdir="$RUN_DIR/lock"
    waited=0
    while ! mkdir "$lockdir" 2>/dev/null; do
        holder=$(cat "$lockdir/pid" 2>/dev/null || true)
        if [ -n "$holder" ] && ! kill -0 "$holder" 2>/dev/null; then
            rm -rf "$lockdir"
            continue
        fi
        if [ "$waited" -ge "$1" ]; then
            return 1
        fi
        if [ -n "$U_ID" ] && [ "$U_STATE" != "waiting" ]; then
            U_STATE=waiting
            write_status
        fi
        sleep 5
        waited=$((waited + 5))
    done
    printf '%s\n' "$$" > "$lockdir/pid"
    return 0
}

lock_release() {
    holder=$(cat "$RUN_DIR/lock/pid" 2>/dev/null || true)
    if [ "$holder" = "$$" ]; then
        rm -rf "$RUN_DIR/lock"
    fi
}

space_ok() {
    size=$(pg "SELECT pg_database_size('stealthnet')" 2>/dev/null || true)
    case "$size" in
        ""|*[!0-9]*) return 0 ;;
    esac
    need=$(( size / 1024 * 3 + 1048576 ))
    free=$(df -Pk "$DEPLOY" 2>/dev/null | awk 'NR == 2 { print $4 }' || true)
    case "$free" in
        ""|*[!0-9]*) return 0 ;;
    esac
    [ "$free" -ge "$need" ]
}

backup_db() {
    mkdir -p "$BACKUPS_DIR"
    chmod 700 "$BACKUPS_DIR"
    name="$(date -u +%Y-%m-%d_%H%M%S)_$(printf '%.7s' "${U_FROM:-unknown}")"
    if ! compose exec -T postgres pg_dump -U stealthnet -d stealthnet -Fc > "$BACKUPS_DIR/$name.dump" 2>/dev/null; then
        rm -f "$BACKUPS_DIR/$name.dump"
        return 1
    fi
    if [ ! -s "$BACKUPS_DIR/$name.dump" ] || ! compose exec -T postgres pg_restore --list < "$BACKUPS_DIR/$name.dump" > /dev/null 2>&1; then
        rm -f "$BACKUPS_DIR/$name.dump"
        return 1
    fi
    cp "$ENV_FILE" "$BACKUPS_DIR/$name.env"
    chmod 600 "$BACKUPS_DIR/$name.dump" "$BACKUPS_DIR/$name.env"
    U_BACKUP=$name
    ls -1t "$BACKUPS_DIR"/*.dump 2>/dev/null | tail -n +4 | while IFS= read -r old; do
        rm -f "$old" "${old%.dump}.env"
    done
    return 0
}

app_ready() {
    id=$(compose ps -q app 2>/dev/null || true)
    [ -n "$id" ] || return 1
    docker exec "$id" curl -fsS http://127.0.0.1:8080/api/health >/dev/null 2>&1
}

verify_app() {
    limit=$1
    image=$2
    waited=0
    streak=0
    while [ "$waited" -lt "$limit" ]; do
        id=$(compose ps -q app 2>/dev/null || true)
        if [ -n "$id" ]; then
            restarts=$(docker inspect -f '{{.RestartCount}}' "$id" 2>/dev/null || true)
            running=$(docker inspect -f '{{.Config.Image}}' "$id" 2>/dev/null || true)
            if [ -n "$restarts" ] && [ "$restarts" != "0" ]; then
                return 1
            fi
            if [ -n "$image" ] && [ -n "$running" ] && [ "$running" != "$image" ]; then
                return 1
            fi
            if docker exec "$id" curl -fsS http://127.0.0.1:8080/api/health >/dev/null 2>&1; then
                streak=$((streak + 1))
                if [ "$streak" -ge 5 ]; then
                    return 0
                fi
            else
                streak=0
            fi
        fi
        sleep "$VERIFY_PAUSE"
        waited=$((waited + 2))
    done
    return 1
}

restore_before_start() {
    if journal_has db_swapping && [ "$(db_label stealthnet)" = "stealthnet-update:$U_ID:copy" ]; then
        pg_retry "BEGIN; ALTER DATABASE stealthnet RENAME TO stealthnet_next; ALTER DATABASE stealthnet_prev RENAME TO stealthnet; COMMIT;" || true
        pg_retry "COMMENT ON DATABASE stealthnet IS NULL" || true
    fi
    if db_exists stealthnet_next && [ "$(db_label stealthnet_next)" = "stealthnet-update:$U_ID:copy" ]; then
        pg_retry "DROP DATABASE stealthnet_next" || true
    fi
    if [ -n "$FROM_IMAGE" ]; then
        set_env SN_IMAGE "$FROM_IMAGE"
    fi
    compose up -d --no-build >/dev/null 2>&1 || true
    if [ -n "$DOWN_EPOCH" ]; then
        verify_app 60 "" >/dev/null 2>&1 || true
        UP_EPOCH=$(date +%s)
    fi
}

block_version() {
    if [ "$U_ORIGIN" = "panel" ] && [ -n "$U_TO" ] && [ -f "$STATUS_DIR/helper.json" ]; then
        if helper_enabled; then
            write_helper_json true "$U_TO"
        else
            write_helper_json false "$U_TO"
        fi
    fi
}

finish_run() {
    U_STATE=$1
    U_ERROR=${2:-}
    U_FINISHED=$(now_iso)
    if [ -n "$DOWN_EPOCH" ] && [ -z "${UP_EPOCH:-}" ]; then
        UP_EPOCH=$(date +%s)
    fi
    case "$U_ERROR" in
        migrate_failed|start_failed|unhealthy) block_version ;;
    esac
    if [ "$U_STATE" = "done" ] && [ -f "$STATUS_DIR/helper.json" ] && [ -n "$(helper_blocked)" ]; then
        if helper_enabled; then
            write_helper_json true ""
        else
            write_helper_json false ""
        fi
    fi
    if [ "$U_ORIGIN" = "panel" ] && [ -n "$DOWN_EPOCH" ] && [ "$U_STATE" != "done" ]; then
        date +%s > "$RUN_DIR/last_failure"
    fi
    write_status
    append_history
    log_line "заявка $U_ID: итог $U_STATE ${U_ERROR}"
    rm -f "$RUN_DIR/journal" "$RUN_DIR/current"
}

update_steps() {
    step_begin check
    if ! docker info >/dev/null 2>&1; then
        fail_step failed docker_unavailable
        return 1
    fi
    if [ ! -f "$ENV_FILE" ] || [ -z "$(compose ps -q postgres 2>/dev/null || true)" ]; then
        fail_step failed internal
        return 1
    fi
    if [ "$U_ORIGIN" = "panel" ]; then
        last=$(cat "$RUN_DIR/last_failure" 2>/dev/null || true)
        case "$last" in
            ""|*[!0-9]*) ;;
            *)
                if [ $(( $(date +%s) - last )) -lt 600 ]; then
                    fail_step failed rate_limited
                    return 1
                fi
                ;;
        esac
        if [ -n "$U_TO" ] && [ "$(helper_blocked)" = "$U_TO" ]; then
            fail_step failed blocked
            return 1
        fi
    fi
    if ! space_ok; then
        fail_step failed no_space
        return 1
    fi
    step_end check done

    step_begin download
    FROM_IMAGE=$(env_value SN_IMAGE)
    current_image=$FROM_IMAGE
    if [ -z "$current_image" ]; then
        current_image=stealthnet:local
    fi
    if [ -z "$TO_IMAGE" ]; then
        ref="$(image_repo):main"
        if ! docker pull "$ref" >/dev/null 2>&1; then
            fail_step failed download_failed
            return 1
        fi
        if [ "$(docker image inspect -f '{{.Architecture}}' "$ref" 2>/dev/null || true)" != "$(host_arch)" ]; then
            fail_step failed wrong_arch
            return 1
        fi
        TO_IMAGE=$(repo_digest "$(image_repo)" "$ref")
        case "$TO_IMAGE" in
            *@sha256:*) ;;
            *) TO_IMAGE=$ref ;;
        esac
    fi
    rev=$(image_commit "$TO_IMAGE")
    U_TO_V=$(image_version "$TO_IMAGE")
    if [ "$U_ORIGIN" = "panel" ] && [ "$rev" != "$U_TO" ]; then
        U_TO=$rev
        fail_step failed stale_request
        return 1
    fi
    U_TO=$rev
    if [ "$TO_IMAGE" = "$current_image" ] || { [ -n "$rev" ] && [ "$rev" = "$U_FROM" ]; }; then
        fail_step failed no_update
        return 1
    fi
    new_created=$(image_created "$TO_IMAGE")
    old_created=$(image_created "$current_image")
    if [ -n "$old_created" ] && [ -n "$new_created" ] && ! later_than "$new_created" "$old_created"; then
        fail_step failed no_update
        return 1
    fi
    step_end download done

    step_begin backup
    journal begin
    write_status
    if ! compose stop -t 60 app >/dev/null 2>&1; then
        fail_step restore backup_failed
        return 1
    fi
    DOWN_EPOCH=$(date +%s)
    journal app_stopped
    if ! backup_db; then
        fail_step restore backup_failed
        return 1
    fi
    journal backup_written
    step_end backup done

    step_begin migrate
    if db_exists stealthnet_prev; then
        case "$(db_label stealthnet_prev)" in
            stealthnet-update:*:prev) pg "DROP DATABASE stealthnet_prev" >/dev/null 2>&1 || true ;;
        esac
    fi
    if db_exists stealthnet_prev || db_exists stealthnet_next; then
        case "$(db_label stealthnet_next)" in
            stealthnet-update:*:copy) pg "DROP DATABASE stealthnet_next" >/dev/null 2>&1 || true ;;
        esac
    fi
    if db_exists stealthnet_prev || db_exists stealthnet_next; then
        fail_step restore internal
        return 1
    fi
    if ! pg_retry "CREATE DATABASE stealthnet_next TEMPLATE stealthnet" \
        || ! pg_retry "COMMENT ON DATABASE stealthnet_next IS 'stealthnet-update:$U_ID:copy'"; then
        fail_step restore internal
        return 1
    fi
    journal db_cloned
    password=$(env_value POSTGRES_PASSWORD)
    if ! (cd "$DEPLOY" && SN_IMAGE="$TO_IMAGE" docker compose run --rm --no-deps -T \
        -e "DATABASE_URL=postgres://stealthnet:$password@postgres:5432/stealthnet_next" \
        -e SN_RUN_BOT=false -e SN_RUN_WORKER=false app migrate) >> "$RUN_DIR/updater.log" 2>&1; then
        fail_step restore migrate_failed
        return 1
    fi
    step_end migrate done

    step_begin start
    journal db_swapping
    if ! pg_retry "BEGIN; ALTER DATABASE stealthnet RENAME TO stealthnet_prev; ALTER DATABASE stealthnet_next RENAME TO stealthnet; COMMIT;"; then
        fail_step restore internal
        return 1
    fi
    pg "COMMENT ON DATABASE stealthnet_prev IS 'stealthnet-update:$U_ID:prev'" >/dev/null 2>&1 || true
    journal db_swapped
    journal app_starting
    set_env SN_IMAGE "$TO_IMAGE"
    if ! compose up -d --no-build >> "$RUN_DIR/updater.log" 2>&1; then
        fail_step attention start_failed
        return 1
    fi
    step_end start done

    step_begin verify
    if ! verify_app "$VERIFY_TIMEOUT" "$TO_IMAGE"; then
        fail_step attention unhealthy
        return 1
    fi
    UP_EPOCH=$(date +%s)
    journal committed
    pg "COMMENT ON DATABASE stealthnet IS NULL" >/dev/null 2>&1 || true
    step_end verify done
    return 0
}

update_run() {
    reset_run
    U_ORIGIN=$1
    U_ID=$2
    U_ADMIN=$3
    U_TO=$4
    TO_IMAGE=${5:-}
    UP_EPOCH=""
    FAIL_KIND=""
    FAIL_CODE=""
    U_STARTED=$(now_iso)
    START_EPOCH=$(date +%s)
    current=$(env_value SN_IMAGE)
    U_FROM=$(image_commit "${current:-stealthnet:local}")
    U_FROM_V=$(image_version "${current:-stealthnet:local}")
    U_STATE=running
    rm -f "$RUN_DIR/journal"
    write_status
    log_line "заявка $U_ID: начало, источник $U_ORIGIN"
    if update_steps; then
        finish_run done
        return 0
    fi
    case "$FAIL_KIND" in
        restore)
            restore_before_start
            finish_run rolled_back "$FAIL_CODE"
            ;;
        attention)
            finish_run attention "$FAIL_CODE"
            ;;
        *)
            finish_run failed "$FAIL_CODE"
            ;;
    esac
    return 1
}

take_request() {
    req="$REQUEST_DIR/request.json"
    if [ ! -e "$req" ] && [ ! -L "$req" ]; then
        sleep 2
        return 1
    fi
    mkdir -p "$RUN_DIR/taken"
    chmod 700 "$RUN_DIR/taken"
    taken="$RUN_DIR/taken/request.$$"
    rm -rf "$taken"
    if ! mv -f "$req" "$taken" 2>/dev/null; then
        rm -rf "$req"
        return 1
    fi
    if [ -L "$taken" ] || [ ! -f "$taken" ]; then
        rm -rf "$taken"
        log_line "заявка отклонена: это не обычный файл"
        sleep 2
        return 1
    fi
    info=$(ls -ln "$taken" 2>/dev/null || true)
    set -- $info
    links=${2:-0}
    uid=${3:-}
    size=${5:-99999}
    case "$size" in
        ""|*[!0-9]*) size=99999 ;;
    esac
    if [ "$links" != "1" ] || [ "$uid" != "$APP_UID" ] || [ "$size" -gt 1024 ]; then
        rm -f "$taken"
        log_line "заявка отклонена: чужой, большой или связанный файл"
        sleep 2
        return 1
    fi
    line=$(head -c 1100 "$taken" | head -n 1)
    rm -f "$taken"
    case "$line" in
        *'"action":"module-'*)
            if take_module_request "$line"; then
                return 0
            fi
            return 1
            ;;
    esac
    REQ_ID=$(printf '%s' "$line" | sed -n 's/^{"id":"\([0-9a-f-]\{36\}\)".*/\1/p')
    if ! printf '%s\n' "$line" | grep -Eqx '\{"id":"[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}","action":"update","target":"[0-9a-f]{40}","requested_at":"[0-9]{4}-[0-9]{2}-[0-9]{2}T[0-9]{2}:[0-9]{2}:[0-9]{2}Z","admin_id":[0-9]{1,18}\}'; then
        log_line "заявка отклонена: неверный вид"
        if [ -n "$REQ_ID" ]; then
            reset_run
            U_ID=$REQ_ID
            U_ORIGIN=panel
            U_STARTED=$(now_iso)
            finish_run failed invalid_request
        fi
        return 1
    fi
    REQ_TARGET=$(printf '%s' "$line" | sed -n 's/.*"target":"\([0-9a-f]\{40\}\)".*/\1/p')
    REQ_ADMIN=$(printf '%s' "$line" | sed -n 's/.*"admin_id":\([0-9]\{1,18\}\)}$/\1/p')
    REQ_AT=$(printf '%s' "$line" | sed -n 's/.*"requested_at":"\([^"]*\)".*/\1/p')
    at=$(iso_epoch "$REQ_AT")
    if [ -n "$at" ] && [ $(( $(date +%s) - at )) -gt 600 ]; then
        reset_run
        U_ID=$REQ_ID
        U_ORIGIN=panel
        U_ADMIN=$REQ_ADMIN
        U_TO=$REQ_TARGET
        U_STARTED=$(now_iso)
        finish_run failed expired
        return 1
    fi
    return 0
}

updater_run() {
    trap - INT TERM
    reset_run
    M_ID=""
    if ! take_request; then
        return 0
    fi
    if [ -n "$M_ID" ]; then
        module_run
        return 0
    fi
    U_ID=$REQ_ID
    U_ORIGIN=panel
    U_ADMIN=$REQ_ADMIN
    U_TO=$REQ_TARGET
    U_STARTED=$(now_iso)
    if ! helper_enabled; then
        finish_run failed blocked
        return 0
    fi
    if ! lock_take 180; then
        finish_run failed busy
        return 0
    fi
    update_run panel "$REQ_ID" "$REQ_ADMIN" "$REQ_TARGET" "" || true
    lock_release
    return 0
}

updater_recover() {
    if [ -f "$RUN_DIR/module-current" ]; then
        module_recover
    fi
    if [ ! -f "$RUN_DIR/current" ]; then
        return 0
    fi
    reset_run
    load_run
    UP_EPOCH=""
    if ! lock_take 30; then
        return 0
    fi
    log_line "заявка $U_ID: обновление прервалось, довожу до конца"
    if journal_has committed; then
        finish_run done
    elif journal_has app_starting; then
        if [ -n "$TO_IMAGE" ] && verify_app 30 "$TO_IMAGE"; then
            finish_run done
        else
            finish_run attention interrupted
        fi
    elif journal_has app_stopped || journal_has begin; then
        restore_before_start
        finish_run rolled_back interrupted
    else
        finish_run failed interrupted
    fi
    lock_release
    return 0
}

modules_on() {
    [ "$(env_value SN_MODULES)" != "off" ]
}

modules_enabled() {
    grep -q '"modules":true' "$STATUS_DIR/helper.json" 2>/dev/null
}

panel_image() {
    image=$(env_value SN_IMAGE)
    printf '%s' "${image:-stealthnet:local}"
}

ensure_networks() {
    case ":$(env_value COMPOSE_FILE):" in
        *:compose.modules.yml:*) ;;
        *) return 0 ;;
    esac
    if ! docker network inspect "$MODULES_NET" >/dev/null 2>&1; then
        docker network create --label org.stealthnet.modules=1 "$MODULES_NET" >/dev/null 2>&1 || true
    fi
    if ! docker network inspect "$MODULES_INNER" >/dev/null 2>&1; then
        docker network create --internal --label org.stealthnet.modules=1 "$MODULES_INNER" >/dev/null 2>&1 || true
    fi
}

valid_module_id() {
    printf '%s' "$1" | grep -Eqx '[a-z][a-z0-9-]{1,39}'
}

valid_module_image() {
    printf '%s' "$1" | grep -Eqx 'stealthnet-module/[a-z][a-z0-9-]{1,39}:[0-9a-f]{12}'
}

m_own() {
    chmod 600 "$1"
    if [ -z "${SN_HELPER_TEST:-}" ]; then
        chown "$APP_UID:$APP_UID" "$1" 2>/dev/null || true
    fi
}

module_steps_for() {
    case "$1" in
        module-install|module-update) printf 'check load start verify' ;;
        module-start|module-restart|module-secrets) printf 'start verify' ;;
        module-stop) printf 'stop' ;;
        module-logs) printf 'logs' ;;
        module-remove) printf 'stop clean' ;;
    esac
}

m_reset() {
    M_STATE=running
    M_ERROR=""
    M_MESSAGE=null
    M_FINISHED=""
    M_MEMORY=""
    M_DATA_BYTES=""
    M_RESTORED=""
    M_STARTED=$(now_iso)
    M_STEPS=$(module_steps_for "$M_ACTION")
    for key in check load start verify stop clean logs; do
        eval "MS_$key=pending; MT_$key=''; MB_$key=''"
    done
}

m_container_json() {
    info=$(docker inspect -f '{{.State.Status}}|{{.State.StartedAt}}|{{.RestartCount}}|{{.Config.Image}}' "sn-mod-$M_MODULE" 2>/dev/null || true)
    if [ -z "$info" ]; then
        printf '{"state":"missing","checked_at":"%s"}' "$(now_iso)"
        return 0
    fi
    c_status=${info%%|*}
    rest=${info#*|}
    c_started=${rest%%|*}
    rest=${rest#*|}
    c_restarts=${rest%%|*}
    c_image=${rest#*|}
    case "$c_status" in
        created|running|paused|restarting|removing|exited|dead) ;;
        *) c_status=unknown ;;
    esac
    printf '%s' "$c_started" | grep -Eqx '[0-9]{4}-[0-9]{2}-[0-9]{2}T[0-9:.]+Z' || c_started=""
    case "$c_started" in 0001-*) c_started="" ;; esac
    case "$c_restarts" in ""|*[!0-9]*) c_restarts="" ;; esac
    valid_module_image "$c_image" || c_image=""
    printf '{"state":"%s","started_at":%s,"restarts":%s,"image":%s,"memory_bytes":%s,"data_bytes":%s,"checked_at":"%s"}' \
        "$c_status" "$(json_str "$c_started")" "$(json_num "$c_restarts")" "$(json_str "$c_image")" \
        "$(json_num "$M_MEMORY")" "$(json_num "$M_DATA_BYTES")" "$(now_iso)"
}

m_status_json() {
    printf '{"schema":1,"module":"%s","job":{"id":"%s","action":"%s","state":"%s","steps":[' "$M_MODULE" "$M_ID" "$M_ACTION" "$M_STATE"
    sep=""
    for key in $M_STEPS; do
        eval "st=\${MS_$key:-pending}; sec=\${MT_$key:-}"
        printf '%s{"key":"%s","state":"%s","seconds":%s}' "$sep" "$key" "$st" "$(json_num "$sec")"
        sep=","
    done
    printf '],"error":%s,"message":%s,"restored":%s,"started_at":%s,"finished_at":%s},"container":' \
        "$(json_str "$M_ERROR")" "${M_MESSAGE:-null}" "${M_RESTORED:-null}" "$(json_str "$M_STARTED")" "$(json_str "$M_FINISHED")"
    m_container_json
    printf '}\n'
}

m_write_status() {
    if [ ! -d "$MODULE_STATUS_DIR" ]; then
        return 0
    fi
    tmp="$MODULE_STATUS_DIR/.$M_MODULE.json.$$"
    m_status_json > "$tmp"
    m_own "$tmp"
    mv -f "$tmp" "$MODULE_STATUS_DIR/$M_MODULE.json"
}

m_step_begin() {
    eval "MS_$1=running; MB_$1=$(date +%s)"
    m_write_status
}

m_step_end() {
    eval "began=\${MB_$1:-}"
    eval "MS_$1=$2; MT_$1=$(( $(date +%s) - ${began:-$(date +%s)} ))"
    m_write_status
}

m_fail() {
    M_ERROR=$1
    for key in $M_STEPS; do
        eval "st=\${MS_$key:-pending}"
        if [ "$st" = "running" ]; then
            m_step_end "$key" failed
        fi
    done
    log_line "модуль $M_MODULE, заявка $M_ID: шаг не прошёл, код $1"
    return 1
}

m_finish() {
    if [ "$M_STATE" = "running" ]; then
        M_STATE=$1
        if [ "$1" = "failed" ] && [ -n "$M_RESTORED" ]; then
            M_STATE=rolled_back
        fi
    fi
    if [ -n "${2:-}" ]; then
        M_ERROR=$2
    fi
    if [ "$M_ACTION" != "module-remove" ] && [ -z "$M_MEMORY$M_DATA_BYTES" ] && valid_module_id "$M_MODULE"; then
        m_collect_stats
    fi
    M_FINISHED=$(now_iso)
    m_write_status
    rm -f "${RUN_DIR:?}/module-current"
    log_line "модуль $M_MODULE, заявка $M_ID ($M_ACTION): итог $M_STATE $M_ERROR"
}

read_spec() {
    SP_MODULE=""
    SP_IMAGE=""
    SP_PORT=""
    SP_HEALTH=""
    SP_MEMORY=""
    SP_CPUS=""
    SP_INTERNET=""
    SP_VOLUMES=""
    if [ ! -f "$1" ] || [ -L "$1" ]; then
        return 1
    fi
    size=$(wc -c < "$1" | tr -d ' ')
    if [ "${size:-99999}" -gt 4096 ]; then
        return 1
    fi
    while IFS= read -r line || [ -n "$line" ]; do
        case "$line" in
            module=*)
                v=${line#module=}
                valid_module_id "$v" || return 1
                SP_MODULE=$v
                ;;
            image=*)
                v=${line#image=}
                valid_module_image "$v" || return 1
                SP_IMAGE=$v
                ;;
            port=*)
                v=${line#port=}
                printf '%s' "$v" | grep -Eqx '[1-9][0-9]{3,4}' || return 1
                if [ "$v" -lt 1024 ] || [ "$v" -gt 65535 ]; then
                    return 1
                fi
                SP_PORT=$v
                ;;
            health=*)
                v=${line#health=}
                printf '%s' "$v" | grep -Eqx '/[A-Za-z0-9._~/-]{0,199}' || return 1
                case "$v" in *//*|*/./*|*/../*|*/.|*/..) return 1 ;; esac
                SP_HEALTH=$v
                ;;
            memory=*)
                v=${line#memory=}
                printf '%s' "$v" | grep -Eqx '[1-9][0-9]{1,3}' || return 1
                if [ "$v" -lt 64 ] || [ "$v" -gt 2048 ]; then
                    return 1
                fi
                SP_MEMORY=$v
                ;;
            cpus=*)
                v=${line#cpus=}
                printf '%s' "$v" | grep -Eqx '0\.[1-9]|1\.[0-9]|2\.0' || return 1
                SP_CPUS=$v
                ;;
            internet=*)
                v=${line#internet=}
                case "$v" in
                    0|1) SP_INTERNET=$v ;;
                    *) return 1 ;;
                esac
                ;;
            volume=*)
                v=${line#volume=}
                printf '%s' "$v" | grep -Eqx '[a-z][a-z0-9]{0,15}:/[A-Za-z0-9._~-][A-Za-z0-9._~/-]{0,198}' || return 1
                case "$v" in
                    *//*|*/./*|*/../*|*/.|*/..) return 1 ;;
                    *:/proc|*:/proc/*|*:/sys|*:/sys/*|*:/dev|*:/dev/*|*:/tmp|*:/tmp/*|*:/run|*:/run/*) return 1 ;;
                esac
                SP_VOLUMES="$SP_VOLUMES $v"
                ;;
            *) return 1 ;;
        esac
    done < "$1"
    count=$(printf '%s' "$SP_VOLUMES" | wc -w | tr -d ' ')
    if [ "$count" -gt 3 ]; then
        return 1
    fi
    [ -n "$SP_MODULE" ] && [ -n "$SP_IMAGE" ] && [ -n "$SP_PORT" ] && [ -n "$SP_HEALTH" ] \
        && [ -n "$SP_MEMORY" ] && [ -n "$SP_CPUS" ] && [ -n "$SP_INTERNET" ]
}

m_file_from_app() {
    if [ -L "$1" ] || [ ! -f "$1" ]; then
        return 1
    fi
    info=$(ls -ln "$1" 2>/dev/null || true)
    set -- $info
    links=${2:-0}
    uid=${3:-}
    size=${5:-}
    case "$size" in
        ""|*[!0-9]*) return 1 ;;
    esac
    [ "$links" = "1" ] && [ "$uid" = "$APP_UID" ] && [ "$size" -le "$M_LIMIT" ]
}

m_take_package() {
    pkg="$REQUEST_DIR/pkg-$M_PACKAGE.zip"
    M_WORK="${MODULES_DIR:?}/.work/${M_ID:?}"
    if [ ! -e "$pkg" ] && [ ! -L "$pkg" ]; then
        return 1
    fi
    mkdir -p "$MODULES_DIR/.work"
    chmod 700 "$MODULES_DIR" "$MODULES_DIR/.work"
    rm -rf "${M_WORK:?}"
    mkdir -p "$M_WORK/in" "$M_WORK/out"
    chmod 700 "$M_WORK"
    chmod 755 "$M_WORK/in"
    if ! mv -f "$pkg" "$M_WORK/in/package.zip" 2>/dev/null; then
        rm -rf "${REQUEST_DIR:?}/pkg-${M_PACKAGE:?}.zip"
        return 1
    fi
    M_LIMIT=524288000
    if ! m_file_from_app "$M_WORK/in/package.zip"; then
        log_line "модуль $M_MODULE, заявка $M_ID: архив отклонён — чужой, большой или связанный файл"
        rm -rf "${M_WORK:?}"
        return 1
    fi
    chmod 644 "$M_WORK/in/package.zip"
    chmod 700 "$M_WORK/out"
    if [ -z "${SN_HELPER_TEST:-}" ]; then
        chown 0:0 "$M_WORK/in/package.zip"
        chown "$APP_UID:$APP_UID" "$M_WORK/out"
    fi
    return 0
}

m_space_ok() {
    size=$(wc -c < "$M_WORK/in/package.zip" 2>/dev/null | tr -d ' ')
    case "$size" in
        ""|*[!0-9]*) return 0 ;;
    esac
    need=$(( size / 1024 * 6 + 524288 ))
    free=$(df -Pk "$MODULES_DIR" 2>/dev/null | awk 'NR == 2 { print $4 }' || true)
    case "$free" in
        ""|*[!0-9]*) return 0 ;;
    esac
    [ "$free" -ge "$need" ]
}

m_unpack() {
    docker run --rm --network none --read-only --cap-drop ALL --security-opt no-new-privileges \
        --user "$APP_UID:$APP_UID" --memory 1g --pids-limit 64 \
        --mount "type=bind,source=$M_WORK/in,target=/in,readonly" \
        --mount "type=bind,source=$M_WORK/out,target=/out" \
        --entrypoint stealthnet "$(panel_image)" module-unpack /in/package.zip /out >> "$RUN_DIR/updater.log" 2>&1
}

m_error_message() {
    file="$M_WORK/out/error.json"
    if [ -L "$file" ] || [ ! -f "$file" ]; then
        printf 'null'
        return 0
    fi
    text=$(head -c 2000 "$file" | sed -n '1p')
    if printf '%s' "$text" | grep -Eqx '"([^"\\[:cntrl:]]|\\["\\/bfnrtu])*"'; then
        printf '%s' "$text"
    else
        printf 'null'
    fi
}

m_load() {
    loaded=$(docker load -i "$M_WORK/out/image.tar" 2>>"$RUN_DIR/updater.log") || return 1
    printf '%s\n' "$loaded" >> "$RUN_DIR/updater.log"
    count=$(printf '%s\n' "$loaded" | grep -c '^Loaded image' || true)
    if [ "$count" != "1" ]; then
        return 1
    fi
    printf '%s\n' "$loaded" | grep -Eqx "Loaded image: (docker\.io/)?$SP_IMAGE" || return 1
    if [ "$(docker image inspect -f '{{.Architecture}}' "$SP_IMAGE" 2>/dev/null || true)" != "$(host_arch)" ]; then
        return 2
    fi
    return 0
}

m_install_secrets() {
    cred="$REQUEST_DIR/$M_ID.cred"
    if [ ! -e "$cred" ] && [ ! -L "$cred" ]; then
        return 1
    fi
    mkdir -p "$RUN_DIR/taken"
    chmod 700 "$RUN_DIR/taken"
    taken="${RUN_DIR:?}/taken/cred.$$"
    rm -rf "$taken"
    if ! mv -f "$cred" "$taken" 2>/dev/null; then
        rm -rf "${REQUEST_DIR:?}/${M_ID:?}.cred"
        return 1
    fi
    M_LIMIT=512
    if ! m_file_from_app "$taken"; then
        rm -rf "$taken"
        return 1
    fi
    key=$(sed -n '1p' "$taken")
    secret=$(sed -n '2p' "$taken")
    lines=$(wc -l < "$taken" | tr -d ' ')
    rm -f "$taken"
    [ "$lines" = "2" ] || return 1
    printf '%s' "$key" | grep -Eqx 'snk_[0-9a-f]{48}' || return 1
    printf '%s' "$secret" | grep -Eqx 'whsec_[A-Za-z0-9+/]{43}=' || return 1
    dir="$MODULES_DIR/$M_MODULE/secrets"
    mkdir -p "$dir"
    chmod 700 "$dir"
    (
        umask 077
        printf '%s\n' "$key" > "$dir/.sn_api_key.$$" && mv -f "$dir/.sn_api_key.$$" "$dir/sn_api_key"
        printf '%s\n' "$secret" > "$dir/.sn_signing_secret.$$" && mv -f "$dir/.sn_signing_secret.$$" "$dir/sn_signing_secret"
    ) || return 1
    chmod 400 "$dir/sn_api_key" "$dir/sn_signing_secret"
    chmod 500 "$dir"
    if [ -z "${SN_HELPER_TEST:-}" ]; then
        chown "$MODULE_UID:$MODULE_UID" "$dir" "$dir/sn_api_key" "$dir/sn_signing_secret"
    fi
    return 0
}

m_connect_app() {
    app=$(compose ps -q app 2>/dev/null || true)
    if [ -z "$app" ]; then
        return 0
    fi
    for net in "$MODULES_NET" "$MODULES_INNER"; do
        docker network connect --alias "$PANEL_ALIAS" "$net" "$app" >/dev/null 2>&1 || true
    done
}

m_prepare_volumes() {
    for vol in $SP_VOLUMES; do
        vname="sn-mod-$M_MODULE-${vol%%:*}"
        if ! docker volume inspect "$vname" >/dev/null 2>&1; then
            docker volume create --label "org.stealthnet.module=$M_MODULE" "$vname" >/dev/null 2>>"$RUN_DIR/updater.log" || return 1
        fi
        docker run --rm --network none --user 0:0 --cap-drop ALL --cap-add CHOWN \
            --mount "type=volume,source=$vname,target=/v" \
            --entrypoint chown "$(panel_image)" "$MODULE_UID:$MODULE_UID" /v >/dev/null 2>>"$RUN_DIR/updater.log" || return 1
    done
    return 0
}

m_run_container() {
    run_image=$1
    net=$MODULES_NET
    if [ "$SP_INTERNET" != "1" ]; then
        net=$MODULES_INNER
    fi
    public=$(env_value SN_PUBLIC_URL)
    printf '%s' "$public" | grep -Eqx 'https?://[A-Za-z0-9.-]+(:[0-9]{1,5})?' || public=""
    set -- run -d --name "sn-mod-$M_MODULE" --hostname "sn-mod-$M_MODULE" \
        --label "org.stealthnet.module=$M_MODULE" --restart unless-stopped \
        --network "$net" --user "$MODULE_UID:$MODULE_UID" --read-only \
        --tmpfs "/tmp:rw,noexec,nosuid,nodev,size=64m" \
        --cap-drop ALL --security-opt no-new-privileges \
        --memory "${SP_MEMORY}m" --memory-swap "${SP_MEMORY}m" --cpus "$SP_CPUS" --pids-limit 256 \
        --log-driver json-file --log-opt max-size=10m --log-opt max-file=3 \
        -e "SN_PANEL_URL=http://$PANEL_ALIAS:8080" -e "SN_PUBLIC_URL=$public" -e "SN_MODULE_ID=$M_MODULE" \
        -e "SN_API_KEY_FILE=/run/secrets/sn_api_key" -e "SN_SIGNING_SECRET_FILE=/run/secrets/sn_signing_secret" \
        --mount "type=bind,source=$MODULES_DIR/$M_MODULE/secrets,target=/run/secrets,readonly"
    for vol in $SP_VOLUMES; do
        set -- "$@" --mount "type=volume,source=sn-mod-$M_MODULE-${vol%%:*},target=${vol#*:},volume-nocopy"
    done
    set -- "$@" "$run_image"
    docker "$@" >/dev/null 2>>"$RUN_DIR/updater.log"
}

m_probe() {
    app=$(compose ps -q app 2>/dev/null || true)
    if [ -z "$app" ]; then
        return 2
    fi
    docker exec "$app" curl -fsS -o /dev/null --max-time 3 "http://sn-mod-$M_MODULE:$SP_PORT$SP_HEALTH" >/dev/null 2>&1
}

m_wait_healthy() {
    waited=0
    limit=${SN_MODULE_VERIFY_TIMEOUT:-90}
    while [ "$waited" -lt "$limit" ]; do
        state=$(docker inspect -f '{{.State.Status}}' "sn-mod-$M_MODULE" 2>/dev/null || true)
        if [ "$state" = "running" ]; then
            m_probe
            case $? in
                0|2) return 0 ;;
            esac
        fi
        sleep "$VERIFY_PAUSE"
        waited=$((waited + 2))
    done
    return 1
}

m_write_logs() {
    if [ ! -d "$MODULE_STATUS_DIR" ]; then
        return 0
    fi
    tmp="$MODULE_STATUS_DIR/.$M_MODULE.log.$$"
    docker logs --tail 200 "sn-mod-$M_MODULE" 2>&1 | tail -c 65536 > "$tmp" || true
    m_own "$tmp"
    mv -f "$tmp" "$MODULE_STATUS_DIR/$M_MODULE.log"
}

m_volumes() {
    docker volume ls -q --filter "label=org.stealthnet.module=$M_MODULE" 2>/dev/null \
        | grep -Ex "sn-mod-$M_MODULE-[a-z][a-z0-9]{0,15}" || true
}

m_collect_stats() {
    M_MEMORY=""
    if [ "$(docker inspect -f '{{.State.Status}}' "sn-mod-$M_MODULE" 2>/dev/null || true)" = "running" ]; then
        M_MEMORY=$(docker stats --no-stream --format '{{.MemUsage}}' "sn-mod-$M_MODULE" 2>/dev/null | awk '
        NR == 1 {
            v = $1; u = $1
            sub(/[A-Za-z]+$/, "", v); sub(/^[0-9.]+/, "", u)
            m = 1
            if (u == "KiB") m = 1024; else if (u == "MiB") m = 1048576; else if (u == "GiB") m = 1073741824
            else if (u == "kB") m = 1000; else if (u == "MB") m = 1000000; else if (u == "GB") m = 1000000000
            if (v ~ /^[0-9.]+$/) printf "%d", v * m
        }' || true)
    fi
    case "$M_MEMORY" in ""|*[!0-9]*) M_MEMORY="" ;; esac
    total=""
    for vname in $(m_volumes); do
        point=$(docker volume inspect -f '{{.Mountpoint}}' "$vname" 2>/dev/null || true)
        if [ -z "$point" ] || [ ! -d "$point" ]; then
            continue
        fi
        kb=$(du -sk "$point" 2>/dev/null | awk '{ print $1 }')
        case "$kb" in ""|*[!0-9]*) continue ;; esac
        total=$(( ${total:-0} + kb * 1024 ))
    done
    M_DATA_BYTES=$total
}

m_prune_images() {
    docker image ls --format '{{.Repository}}:{{.Tag}}' "stealthnet-module/$M_MODULE" 2>/dev/null | while IFS= read -r ref; do
        valid_module_image "$ref" || continue
        if [ "$ref" = "${1:-}" ] || [ "$ref" = "${2:-}" ]; then
            continue
        fi
        docker image rm "$ref" >/dev/null 2>&1 || true
    done
}

m_saved() {
    M_DIR="$MODULES_DIR/$M_MODULE"
    if ! read_spec "$M_DIR/spec" || [ "$SP_MODULE" != "$M_MODULE" ]; then
        return 1
    fi
    M_IMAGE=$(sed -n '1p' "$M_DIR/image" 2>/dev/null || true)
    valid_module_image "$M_IMAGE"
}

m_rollback() {
    old_image=$1
    docker rm -f "sn-mod-$M_MODULE" >/dev/null 2>&1 || true
    if [ -z "$old_image" ] || [ ! -f "$M_DIR/spec.previous" ]; then
        return 1
    fi
    cp "$M_DIR/spec.previous" "$M_DIR/spec"
    printf '%s\n' "$old_image" > "$M_DIR/image"
    if ! read_spec "$M_DIR/spec" || ! m_run_container "$old_image"; then
        return 1
    fi
    if m_wait_healthy; then
        M_RESTORED=true
    else
        M_RESTORED=false
    fi
    return 0
}

module_install() {
    M_DIR="$MODULES_DIR/$M_MODULE"
    m_step_begin check
    if ! docker info >/dev/null 2>&1; then
        m_fail docker_unavailable
        return 1
    fi
    if ! m_take_package; then
        m_fail package_missing
        return 1
    fi
    if ! m_space_ok; then
        rm -rf "${M_WORK:?}"
        m_fail no_space
        return 1
    fi
    if ! m_unpack; then
        M_MESSAGE=$(m_error_message)
        rm -rf "${M_WORK:?}"
        m_fail package_invalid
        return 1
    fi
    if ! read_spec "$M_WORK/out/spec" || [ "$SP_MODULE" != "$M_MODULE" ]; then
        rm -rf "${M_WORK:?}"
        m_fail package_invalid
        return 1
    fi
    mkdir -p "$M_DIR"
    chmod 700 "$M_DIR"
    cp "$M_WORK/out/spec" "$M_DIR/spec.next"
    m_step_end check done

    m_step_begin load
    if m_load; then
        loaded=0
    else
        loaded=$?
    fi
    rm -rf "${M_WORK:?}"
    case "$loaded" in
        0) ;;
        2)
            docker image rm "$SP_IMAGE" >/dev/null 2>&1 || true
            m_fail wrong_arch
            return 1
            ;;
        *)
            m_fail load_failed
            return 1
            ;;
    esac
    m_step_end load done

    m_step_begin start
    if [ "$M_ACTION" = "module-install" ] || [ ! -f "$M_DIR/secrets/sn_api_key" ]; then
        if ! m_install_secrets; then
            m_fail invalid_request
            return 1
        fi
    fi
    ensure_networks
    m_connect_app
    if ! m_prepare_volumes; then
        m_fail start_failed
        return 1
    fi
    old_image=$(sed -n '1p' "$M_DIR/image" 2>/dev/null || true)
    valid_module_image "$old_image" || old_image=""
    rm -f "${M_DIR:?}/spec.previous"
    if [ -n "$old_image" ] && [ -f "$M_DIR/spec" ]; then
        cp "$M_DIR/spec" "$M_DIR/spec.previous"
    fi
    new_image=$SP_IMAGE
    docker rm -f "sn-mod-$M_MODULE" >/dev/null 2>&1 || true
    if ! m_run_container "$new_image"; then
        m_rollback "$old_image" || true
        m_fail start_failed
        return 1
    fi
    mv -f "$M_DIR/spec.next" "$M_DIR/spec"
    printf '%s\n' "$new_image" > "$M_DIR/image"
    m_step_end start done

    m_step_begin verify
    if ! m_wait_healthy; then
        m_write_logs
        if ! m_rollback "$old_image"; then
            docker stop -t 10 "sn-mod-$M_MODULE" >/dev/null 2>&1 || true
        fi
        m_fail unhealthy
        return 1
    fi
    m_step_end verify done
    m_write_logs
    m_prune_images "$new_image" "$old_image"
    return 0
}

module_start() {
    m_step_begin start
    if ! m_saved; then
        m_fail not_installed
        return 1
    fi
    started=0
    if docker inspect "sn-mod-$M_MODULE" >/dev/null 2>&1; then
        if [ "$M_ACTION" = "module-start" ]; then
            docker start "sn-mod-$M_MODULE" >/dev/null 2>>"$RUN_DIR/updater.log" || started=1
        else
            docker restart -t 20 "sn-mod-$M_MODULE" >/dev/null 2>>"$RUN_DIR/updater.log" || started=1
        fi
    else
        ensure_networks
        m_connect_app
        if ! m_prepare_volumes || ! m_run_container "$M_IMAGE"; then
            started=1
        fi
    fi
    if [ "$started" -ne 0 ]; then
        m_fail start_failed
        return 1
    fi
    m_step_end start done
    m_step_begin verify
    if ! m_wait_healthy; then
        m_write_logs
        m_fail unhealthy
        return 1
    fi
    m_step_end verify done
    m_write_logs
    return 0
}

module_secrets() {
    if ! m_install_secrets; then
        m_step_begin start
        m_fail invalid_request
        return 1
    fi
    module_start
}

module_stop() {
    m_step_begin stop
    docker stop -t 20 "sn-mod-$M_MODULE" >/dev/null 2>&1 || true
    m_step_end stop done
    return 0
}

module_logs() {
    m_step_begin logs
    m_collect_stats
    m_write_logs
    m_step_end logs done
    return 0
}

module_remove() {
    m_step_begin stop
    docker rm -f "sn-mod-$M_MODULE" >/dev/null 2>&1 || true
    m_step_end stop done
    m_step_begin clean
    m_prune_images "" ""
    if [ "$M_DATA" = "erase" ]; then
        for vname in $(m_volumes); do
            docker volume rm "$vname" >/dev/null 2>>"$RUN_DIR/updater.log" || true
        done
        rm -f "${MODULE_STATUS_DIR:?}/${M_MODULE:?}.log"
    fi
    if [ -d "${MODULES_DIR:?}/${M_MODULE:?}" ]; then
        chmod -R u+w "${MODULES_DIR:?}/${M_MODULE:?}" 2>/dev/null || true
        rm -rf "${MODULES_DIR:?}/${M_MODULE:?}"
    fi
    m_step_end clean done
    return 0
}

module_run() {
    m_reset
    if ! modules_enabled; then
        m_finish failed modules_off
        return 0
    fi
    if ! lock_take 180; then
        m_finish failed busy
        return 0
    fi
    printf '%s %s %s\n' "$M_MODULE" "$M_ID" "$M_ACTION" > "$RUN_DIR/module-current"
    m_write_status
    log_line "модуль $M_MODULE, заявка $M_ID ($M_ACTION): начало"
    result=0
    case "$M_ACTION" in
        module-install|module-update) module_install || result=1 ;;
        module-start|module-restart) module_start || result=1 ;;
        module-secrets) module_secrets || result=1 ;;
        module-stop) module_stop || result=1 ;;
        module-logs) module_logs || result=1 ;;
        module-remove) module_remove || result=1 ;;
        *) m_fail invalid_request || result=1 ;;
    esac
    if [ "$result" -eq 0 ]; then
        m_finish done
    else
        m_finish failed
    fi
    lock_release
    return 0
}

module_recover() {
    line=$(sed -n '1p' "$RUN_DIR/module-current" 2>/dev/null || true)
    rm -f "${RUN_DIR:?}/module-current"
    set -- $line
    if [ "$#" -ne 3 ] || ! valid_module_id "$1"; then
        return 0
    fi
    printf '%s' "$2" | grep -Eqx '[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}' || return 0
    case "$3" in
        module-install|module-update|module-start|module-stop|module-restart|module-logs|module-secrets|module-remove) ;;
        *) return 0 ;;
    esac
    M_MODULE=$1
    M_ID=$2
    M_ACTION=$3
    m_reset
    m_finish failed interrupted
}

take_module_request() {
    req_line=$1
    M_ID=""
    M_ACTION=""
    M_MODULE=""
    M_PACKAGE=""
    M_DATA=""
    uuid='[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}'
    stamp='[0-9]{4}-[0-9]{2}-[0-9]{2}T[0-9]{2}:[0-9]{2}:[0-9]{2}Z'
    module='[a-z][a-z0-9-]{1,39}'
    tail="\"requested_at\":\"$stamp\",\"admin_id\":[0-9]{1,18}\\}"
    if printf '%s\n' "$req_line" | grep -Eqx "\\{\"id\":\"$uuid\",\"action\":\"module-(install|update)\",\"module\":\"$module\",\"package\":\"$uuid\",$tail"; then
        M_PACKAGE=$(printf '%s' "$req_line" | sed -n 's/.*"package":"\([0-9a-f-]\{36\}\)".*/\1/p')
    elif printf '%s\n' "$req_line" | grep -Eqx "\\{\"id\":\"$uuid\",\"action\":\"module-remove\",\"module\":\"$module\",\"data\":\"(keep|erase)\",$tail"; then
        M_DATA=$(printf '%s' "$req_line" | sed -n 's/.*"data":"\([a-z]*\)".*/\1/p')
    elif ! printf '%s\n' "$req_line" | grep -Eqx "\\{\"id\":\"$uuid\",\"action\":\"module-(start|stop|restart|logs|secrets)\",\"module\":\"$module\",$tail"; then
        log_line "заявка модуля отклонена: неверный вид"
        return 1
    fi
    M_ID=$(printf '%s' "$req_line" | sed -n 's/^{"id":"\([0-9a-f-]\{36\}\)".*/\1/p')
    M_ACTION=$(printf '%s' "$req_line" | sed -n 's/.*"action":"\(module-[a-z]*\)".*/\1/p')
    M_MODULE=$(printf '%s' "$req_line" | sed -n 's/.*"module":"\([a-z0-9-]*\)".*/\1/p')
    req_at=$(printf '%s' "$req_line" | sed -n 's/.*"requested_at":"\([^"]*\)".*/\1/p')
    at=$(iso_epoch "$req_at")
    if [ -n "$at" ] && [ $(( $(date +%s) - at )) -gt 600 ]; then
        m_reset
        m_finish failed expired
        rm -f "${REQUEST_DIR:?}/${M_ID:?}.cred"
        if [ -n "$M_PACKAGE" ]; then
            rm -f "${REQUEST_DIR:?}/pkg-${M_PACKAGE:?}.zip"
        fi
        M_ID=""
        return 1
    fi
    return 0
}

modules_command() {
    printf '%sSTEALTHNET RW SYSTEM — модули из архива%s\n\n' "$BOLD" "$RESET"
    if [ ! -f "$ENV_FILE" ]; then
        die "система ещё не установлена" "Запустите $(command_for install)"
    fi
    case "${1:-}" in
        on|off)
            if [ "$(id -u)" -ne 0 ] && [ -z "${SN_HELPER_TEST:-}" ]; then
                die "нужны права root" "Запустите: sudo $(command_for "modules $1")"
            fi
            set_env SN_MODULES "$1"
            if [ -d "$STATUS_DIR" ]; then
                if helper_enabled; then
                    write_helper_json true
                else
                    write_helper_json false
                fi
            fi
            if [ "$1" = "off" ]; then
                done_line "Модули из админки запрещены: их нельзя ставить, обновлять, перезапускать и удалять. Уже поставленные модули продолжают работать."
            elif helper_enabled; then
                done_line "Модули из админки разрешены: ставить, обновлять и удалять их может владелец."
            else
                done_line "Разрешено, но помощник на сервере выключен. Включить: sudo $(command_for 'helper on')"
            fi
            ;;
        "")
            list_modules
            ;;
        *)
            die "нужно слово on или off" "Разрешить: $(command_for 'modules on')" "Запретить: $(command_for 'modules off')"
            ;;
    esac
}

list_modules() {
    if modules_on; then
        printf '  Модули из админки: разрешены (запретить: %s)\n' "$(command_for 'modules off')"
    else
        printf '  Модули из админки: запрещены (разрешить: %s)\n' "$(command_for 'modules on')"
    fi
    found=$(docker ps -a --filter label=org.stealthnet.module --format '{{.Label "org.stealthnet.module"}}|{{.State}}|{{.Image}}' 2>/dev/null || true)
    if [ -z "$found" ]; then
        printf '  Модулей из архива нет\n'
        return 0
    fi
    printf '%s\n' "$found" | while IFS='|' read -r name state image; do
        valid_module_id "$name" || continue
        case "$state" in
            running) shown="работает" ;;
            exited|created) shown="остановлен" ;;
            restarting) shown="перезапускается" ;;
            *) shown=$state ;;
        esac
        printf '  Модуль %s: %s (%s)\n' "$name" "$shown" "$image"
    done
}

run_dir_ready() {
    if mkdir -p "$RUN_DIR" 2>/dev/null && [ -w "$RUN_DIR" ]; then
        chmod 700 "$RUN_DIR"
        return 0
    fi
    RUN_DIR="$DEPLOY/.update"
    mkdir -p "$RUN_DIR"
    chmod 700 "$RUN_DIR"
}

updater_command() {
    if [ ! -f "$ENV_FILE" ]; then
        exit 0
    fi
    run_dir_ready
    case "${1:-}" in
        run) updater_run ;;
        recover) updater_recover ;;
        *) usage >&2; exit 2 ;;
    esac
}

show_status() {
    printf '%sSTEALTHNET RW SYSTEM — состояние%s\n\n' "$BOLD" "$RESET"
    if [ ! -f "$ENV_FILE" ]; then
        die "система ещё не установлена" "Запустите $(command_for install)"
    fi
    image=$(env_value SN_IMAGE)
    commit=$(image_commit "${image:-stealthnet:local}")
    if [ -n "$commit" ]; then
        printf '  Версия:            %s (%.7s)\n' "$(image_name "${image:-stealthnet:local}")" "$commit"
    else
        printf '  Версия:            %s\n' "$(image_name "${image:-stealthnet:local}")"
    fi
    url=$(env_value SN_PUBLIC_URL)
    if [ -n "$url" ]; then
        printf '  Адрес панели:      %s\n' "$url"
    else
        printf '  Адрес панели:      не задан (указать: %s)\n' "$(command_for domain)"
    fi
    id=$(compose ps -q app 2>/dev/null || true)
    state=""
    if [ -n "$id" ]; then
        state=$(docker inspect -f '{{if .State.Health}}{{.State.Health.Status}}{{else}}{{.State.Status}}{{end}}' "$id" 2>/dev/null || true)
    fi
    case "$state" in
        healthy) printf '  Панель:            работает\n' ;;
        starting) printf '  Панель:            запускается\n' ;;
        "") printf '  Панель:            не запущена\n' ;;
        *) printf '  Панель:            %s\n' "$state" ;;
    esac
    if helper_enabled; then
        printf '  Обновление из админки: включено\n'
    else
        printf '  Обновление из админки: выключено (включить: %s)\n' "$(command_for 'helper on')"
    fi
    if [ -d "$MODULES_DIR" ]; then
        list_modules
    fi
    if [ -f "$STATUS_DIR/update.json" ]; then
        last_state=$(sed -n 's/.*"state":"\([a-z_]*\)","steps".*/\1/p' "$STATUS_DIR/update.json" | sed -n '1p')
        last_error=$(sed -n 's/.*"error":"\([a-z_]*\)".*/\1/p' "$STATUS_DIR/update.json" | sed -n '1p')
        last_at=$(sed -n 's/.*"started_at":"\([^"]*\)".*/\1/p' "$STATUS_DIR/update.json" | sed -n '1p')
        last_to=$(sed -n 's/.*"to_version":"\([0-9.]*\)".*/\1/p' "$STATUS_DIR/update.json" | sed -n '1p')
        case "$last_state" in
            done) printf '  Последнее обновление: %s, установлено%s\n' "$last_at" "${last_to:+ (версия $last_to)}" ;;
            running|waiting) printf '  Последнее обновление: идёт сейчас\n' ;;
            rolled_back) printf '  Последнее обновление: %s, не установлено (%s), прежняя версия работает\n' "$last_at" "$last_error" ;;
            attention) printf '  Последнее обновление: %s, новая версия запущена с ошибками (%s)\n' "$last_at" "$last_error"
                printf '  Журнал панели: cd %s && docker compose logs --tail 100 app\n' "$DEPLOY" ;;
            failed) printf '  Последнее обновление: %s, не началось (%s)\n' "$last_at" "$last_error" ;;
        esac
    fi
    count=$(ls -1 "$BACKUPS_DIR"/*.dump 2>/dev/null | wc -l | tr -d ' ')
    if [ "${count:-0}" -gt 0 ]; then
        newest=$(ls -1t "$BACKUPS_DIR"/*.dump 2>/dev/null | sed -n '1p')
        printf '  Копии базы:        %s, последняя %s\n' "$count" "$(basename "$newest" .dump)"
        printf '  Где лежат:         %s\n' "$BACKUPS_DIR"
    fi
    count=$(ls -1 "$PANEL_BACKUPS"/stealthnet-backup-* 2>/dev/null | wc -l | tr -d ' ')
    if [ "${count:-0}" -gt 0 ]; then
        newest=$(ls -1t "$PANEL_BACKUPS"/stealthnet-backup-* 2>/dev/null | sed -n '1p')
        printf '  Резервные копии:   %s, последняя %s\n' "$count" "$(basename "$newest")"
        printf '  Где лежат:         %s\n' "$PANEL_BACKUPS"
    fi
}

prepare_backups() {
    mkdir -p "$PANEL_BACKUPS" 2>/dev/null || true
    chmod 700 "$BACKUPS_DIR" "$PANEL_BACKUPS" 2>/dev/null || true
    if [ "$(id -u)" -eq 0 ]; then
        chown "$APP_UID:$APP_UID" "$PANEL_BACKUPS" 2>/dev/null || true
    fi
    if [ "$(env_value SN_BACKUP_HOST_DIR)" != "$PANEL_BACKUPS" ]; then
        set_env SN_BACKUP_HOST_DIR "$PANEL_BACKUPS"
    fi
}

backup_command() {
    printf '%sSTEALTHNET RW SYSTEM — резервная копия%s\n\n' "$BOLD" "$RESET"
    need_installed
    docker_ready backup
    prepare_backups
    if [ -z "$(compose ps -q app 2>/dev/null || true)" ]; then
        die "панель не запущена" "Запустите её: $(command_for start)"
    fi
    step "Копия"
    if ! compose exec -T app stealthnet backup; then
        die "копия не сделана" "Сообщение об ошибке — выше."
    fi
    printf '\nКопии лежат в папке %s\n' "$PANEL_BACKUPS"
}

restore_tool() {
    printf '%s\n' "$RESTORE_PASSWORD" | compose run --rm --no-deps -T \
        -e SN_RUN_BOT=false -e SN_RUN_WORKER=false \
        app restore --password-stdin "$@" "$APP_BACKUPS/$RESTORE_NAME"
}

restore_command() {
    printf '%sSTEALTHNET RW SYSTEM — восстановление из копии%s\n\n' "$BOLD" "$RESET"
    need_installed
    yes=""
    file=""
    for arg in "$@"; do
        case "$arg" in
            --yes|-y) yes=1 ;;
            -*) die "не понял «$arg»" "Пример: $(command_for 'restore /root/stealthnet-backup-20261118-040000-auto.tar.gz')" ;;
            *)
                if [ -n "$file" ]; then
                    die "лишний аргумент «$arg»" "Укажите один файл копии."
                fi
                file=$arg
                ;;
        esac
    done
    if [ -z "$file" ]; then
        die "укажите файл копии" \
            "Пример: $(command_for 'restore /root/stealthnet-backup-20261118-040000-auto.tar.gz')" \
            "Копии панели лежат в папке $PANEL_BACKUPS"
    fi
    docker_ready restore
    prepare_backups
    RESTORE_COPY=""
    case "$file" in
        *.part[0-9][0-9][0-9])
            base=${file%.part[0-9][0-9][0-9]}
            if [ ! -f "$base.part001" ]; then
                die "нет первой части копии: $base.part001"
            fi
            RESTORE_NAME=".restore-$(random_hex 4)-$(basename "$base")"
            RESTORE_COPY="$PANEL_BACKUPS/$RESTORE_NAME"
            step "Сборка частей"
            if ! cat "$base".part[0-9][0-9][0-9] > "$RESTORE_COPY"; then
                rm -f "$RESTORE_COPY"
                die "части копии не собрались: не хватает места на диске?"
            fi
            done_line "собрано: $(ls -1 "$base".part[0-9][0-9][0-9] | wc -l | tr -d ' ') части"
            ;;
        *)
            if [ ! -f "$file" ]; then
                die "файл не найден: $file"
            fi
            dir=$(cd "$(dirname "$file")" && pwd)
            if [ "$dir" = "$(cd "$PANEL_BACKUPS" && pwd)" ]; then
                RESTORE_NAME=$(basename "$file")
            else
                RESTORE_NAME=".restore-$(random_hex 4)-$(basename "$file")"
                RESTORE_COPY="$PANEL_BACKUPS/$RESTORE_NAME"
                if ! cp "$file" "$RESTORE_COPY"; then
                    rm -f "$RESTORE_COPY"
                    die "файл не скопировался в папку копий: не хватает места на диске?"
                fi
            fi
            ;;
    esac
    if [ -n "$RESTORE_COPY" ]; then
        chmod 600 "$RESTORE_COPY" 2>/dev/null || true
        if [ "$(id -u)" -eq 0 ]; then
            chown "$APP_UID:$APP_UID" "$RESTORE_COPY" 2>/dev/null || true
        fi
    fi
    RESTORE_PASSWORD=""
    if [ "$(head -c 8 "$PANEL_BACKUPS/$RESTORE_NAME" 2>/dev/null)" = "SNBACKUP" ]; then
        if [ -n "${SN_BACKUP_PASSWORD:-}" ]; then
            RESTORE_PASSWORD=$SN_BACKUP_PASSWORD
        else
            ask_hidden "Пароль копии:"
            RESTORE_PASSWORD=$ANSWER
        fi
    fi
    step "Проверка копии"
    if ! restore_tool --check; then
        if [ -n "$RESTORE_COPY" ]; then
            rm -f "$RESTORE_COPY"
        fi
        die "копия не прошла проверку, база не менялась" "Сообщение об ошибке — выше."
    fi
    if [ -z "$yes" ]; then
        printf '\n'
        ask "Заменить текущую базу этой копией? Панель и бот остановятся на время восстановления, перед заменой сохраню копию текущей базы. [д/Н]"
        case "$ANSWER" in
            д*|Д*|y*|Y*) ;;
            *)
                if [ -n "$RESTORE_COPY" ]; then
                    rm -f "$RESTORE_COPY"
                fi
                printf 'Ничего не сделано.\n'
                return 0
                ;;
        esac
    fi
    step "Остановка панели"
    if ! compose stop -t 60 app >/dev/null 2>&1; then
        die "панель не остановилась" "Сообщение об ошибке — выше. База не менялась."
    fi
    step "Восстановление"
    if ! restore_tool --yes; then
        if [ -n "$RESTORE_COPY" ]; then
            rm -f "$RESTORE_COPY"
        fi
        compose up -d --no-build >/dev/null 2>&1 || true
        die "восстановление не прошло, база осталась прежней" \
            "Сообщение об ошибке — выше. Панель снова запускается: $(command_for status)"
    fi
    if [ -n "$RESTORE_COPY" ]; then
        rm -f "$RESTORE_COPY"
    fi
    step "Запуск"
    if ! compose up -d --no-build; then
        die "панель не запустилась" "Сообщение об ошибке — выше."
    fi
    wait_healthy
    done_line "база восстановлена"
    show_address
}

cli_update() {
    NEW_IMAGE=$(env_value SN_IMAGE)
    set_env SN_IMAGE "$OLD_IMAGE"
    run_dir_ready
    if ! lock_take 180; then
        die "сейчас идёт другое обновление" "Дождитесь его конца и запустите команду ещё раз: $(command_for update)"
    fi
    hint "Панель будет недоступна около минуты. Перед обновлением сохраню копию базы."
    CLI_UPDATE=1
    if update_run command "cmd-$(random_hex 8)" "" "" "$NEW_IMAGE"; then
        CLI_UPDATE=""
        lock_release
        return 0
    fi
    CLI_UPDATE=""
    lock_release
    case "$U_STATE:$U_ERROR" in
        failed:no_update)
            set_env SN_IMAGE "$OLD_IMAGE"
            start_stack
            ;;
        rolled_back:*)
            die "новая версия не установлена ($U_ERROR), работает прежняя, данные не изменились" \
                "Журнал обновления: $RUN_DIR/updater.log"
            ;;
        attention:*)
            die "новая версия запущена, но не отвечает ($U_ERROR)" \
                "База уже обновлена, поэтому прежнюю версию я сам не возвращаю." \
                "Журнал панели: cd $DEPLOY && docker compose logs --tail 100 app" \
                "Копия базы перед обновлением: $BACKUPS_DIR/$U_BACKUP.dump"
            ;;
        *)
            die "обновление не началось ($U_ERROR), работает прежняя версия" \
                "Журнал обновления: $RUN_DIR/updater.log"
            ;;
    esac
}

need_installed() {
    if [ ! -f "$ENV_FILE" ]; then
        die "система ещё не установлена" "Запустите $(command_for install)"
    fi
}

docker_ready() {
    if ! command -v docker >/dev/null 2>&1; then
        die "Docker не установлен" \
            "Установите его командой: curl -fsSL https://get.docker.com | sudo sh"
    fi
    if docker info >/dev/null 2>&1; then
        return 0
    fi
    if [ "$(id -u)" -eq 0 ] && command -v systemctl >/dev/null 2>&1; then
        hint "Docker не запущен, запускаю его."
        systemctl start docker >/dev/null 2>&1 || true
        if docker info >/dev/null 2>&1; then
            return 0
        fi
    fi
    die "Docker не отвечает" \
        "Если он не запущен: sudo systemctl start docker" \
        "Если не хватает прав: sudo $(command_for "$1")"
}

boot_with_server() {
    if [ "$(id -u)" -ne 0 ] || ! command -v systemctl >/dev/null 2>&1; then
        return 0
    fi
    if [ "$(systemctl is-enabled docker 2>/dev/null)" = "enabled" ]; then
        return 0
    fi
    if systemctl enable docker >/dev/null 2>&1; then
        hint "Docker теперь запускается вместе с сервером: после перезагрузки система поднимется сама."
    fi
}

show_address() {
    url=$(env_value SN_PUBLIC_URL)
    if [ -n "$url" ]; then
        printf '\n  Адрес панели:      %s\n' "$url"
    fi
}

start_system() {
    printf '%sSTEALTHNET RW SYSTEM — запуск%s\n\n' "$BOLD" "$RESET"
    need_installed
    docker_ready start
    step "Запуск"
    if ! compose up -d --no-build --remove-orphans; then
        die "запуск не прошёл" "Сообщение об ошибке — выше."
    fi
    done_line "контейнеры запущены"
    wait_healthy
    boot_with_server
    show_address
}

restart_system() {
    printf '%sSTEALTHNET RW SYSTEM — перезапуск%s\n\n' "$BOLD" "$RESET"
    need_installed
    docker_ready restart
    step "Перезапуск"
    hint "Панель и бот будут недоступны несколько секунд."
    if ! compose up -d --no-build --remove-orphans; then
        die "запуск не прошёл" "Сообщение об ошибке — выше."
    fi
    if ! compose restart; then
        die "перезапуск не прошёл" "Сообщение об ошибке — выше."
    fi
    done_line "перезапущено"
    wait_healthy
    show_address
}

stop_system() {
    printf '%sSTEALTHNET RW SYSTEM — остановка%s\n\n' "$BOLD" "$RESET"
    need_installed
    case "${1:-}" in
        --yes|-y) ;;
        "")
            if ! can_ask; then
                die "некому подтвердить остановку" \
                    "Без вопросов: $(command_for stop) --yes"
            fi
            ask "Остановить STEALTHNET? Панель, бот и приём оплат не будут работать, пока не запустите снова. [д/Н]"
            case "$ANSWER" in
                д*|Д*|y*|Y*) ;;
                *)
                    printf 'Ничего не сделано.\n'
                    return 0
                    ;;
            esac
            ;;
        *) die "не понял «$1»" "Остановить без вопросов: $(command_for stop) --yes" ;;
    esac
    docker_ready stop
    step "Остановка"
    if ! compose stop; then
        die "остановить не получилось" "Сообщение об ошибке — выше."
    fi
    done_line "остановлено"
    printf '\nЗапустить снова: %s\n' "$(command_for start)"
}

show_logs() {
    need_installed
    docker_ready logs
    case "${1:-}" in
        "") compose logs --tail 200 app ;;
        -f|--follow) compose logs --tail 100 -f app ;;
        *) die "не понял «$1»" "Журнал: $(command_for logs), следить: $(command_for logs) -f" ;;
    esac
}

detect_short

case "${1:-}" in
    ""|install) install ;;
    domain|--domain) change_domain ;;
    cert|--cert) shift; reissue_cert "$@" ;;
    owner-email|--owner-email) change_owner_email ;;
    reset-password|--reset-password) reset_password ;;
    unlock-login|--unlock-login) shift; unlock_login "$@" ;;
    update|--update) update ;;
    status|--status) show_status ;;
    backup|--backup) backup_command ;;
    restore|--restore) shift; restore_command "$@" ;;
    helper|--helper) helper_command "${2:-}" ;;
    modules|--modules) modules_command "${2:-}" ;;
    updater) updater_command "${2:-}" ;;
    start|--start) start_system ;;
    restart|--restart) restart_system ;;
    stop|--stop) stop_system "${2:-}" ;;
    logs|--logs) show_logs "${2:-}" ;;
    -h|--help|help) usage ;;
    *) usage >&2; exit 2 ;;
esac
