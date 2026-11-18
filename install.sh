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
    printf '  %s\n      сменить почту владельца\n' "$(command_for owner-email)"
    printf '  %s\n      выдать владельцу новый пароль\n' "$(command_for reset-password)"
    printf '  %s\n      снять блокировку входа владельца, если его почту подбирают;\n      с --ip вход с этого адреса не закроется, даже пока подбор идёт\n' "$(command_for 'unlock-login [--ip адрес]')"
    printf '  %s\n      скачать новую версию и обновить систему\n' "$(command_for update)"
    printf '  %s\n      сделать резервную копию сейчас\n' "$(command_for backup)"
    printf '  %s\n      восстановить базу из резервной копии; части из Telegram (.part001…)\n      соберутся сами, пароль копии спросит или возьмёт из SN_BACKUP_PASSWORD\n' "$(command_for 'restore файл')"
    printf '  %s\n      показать версию, состояние и итог последнего обновления\n' "$(command_for status)"
    printf '  %s\n      разрешить или запретить обновление из админки\n' "$(command_for 'helper on|off')"
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
        printf '    client_max_body_size 2g;\n'
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
    fi
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
                files="$files:compose.updater.yml"
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
            hint "Когда это исправится, сертификат выпустится сам, запускать ничего не нужно."
            return 0
        fi
        sleep 5
        waited=$((waited + 5))
    done
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
    if [ -n "${SN_HELPER_TEST:-}" ]; then
        return 0
    fi
    if [ "$(id -u)" -ne 0 ]; then
        HELPER_WHY="нужны права root"
        return 1
    fi
    if [ ! -d /run/systemd/system ] || ! command -v systemctl >/dev/null 2>&1; then
        HELPER_WHY="на сервере нет systemd"
        return 1
    fi
    case "$ROOT$UPDATER_STATE" in
        *[!A-Za-z0-9._/-]*)
            HELPER_WHY="в пути к папке системы есть пробелы или особые знаки"
            return 1
            ;;
    esac
    if ! owned_by_root "$ROOT" || ! owned_by_root "$ROOT/install.sh" || ! owned_by_root "$DEPLOY"; then
        HELPER_WHY="папку $ROOT может менять не только root"
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
    tmp="$STATUS_DIR/.helper.json.$$"
    printf '{"schema":1,"panel_updates":%s,"blocked":%s}\n' "$enabled" "$(json_str "$blocked")" > "$tmp"
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
    mkdir -p "$REQUEST_DIR" "$STATUS_DIR" "$RUN_DIR"
    chmod 755 "$UPDATER_STATE" "$IPC_DIR" "$STATUS_DIR"
    chmod 700 "$RUN_DIR" "$REQUEST_DIR"
    if [ -z "${SN_HELPER_TEST:-}" ]; then
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
    if ! systemctl daemon-reload >/dev/null 2>&1 \
        || ! systemctl enable --now stealthnet-rw-update.path >/dev/null 2>&1 \
        || ! systemctl enable stealthnet-rw-recover.service >/dev/null 2>&1; then
        HELPER_WHY="systemd не принял службу обновления"
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
        done_line "включено, обновлять может только владелец с паролем"
    else
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
    if ! take_request; then
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
    owner-email|--owner-email) change_owner_email ;;
    reset-password|--reset-password) reset_password ;;
    unlock-login|--unlock-login) shift; unlock_login "$@" ;;
    update|--update) update ;;
    status|--status) show_status ;;
    backup|--backup) backup_command ;;
    restore|--restore) shift; restore_command "$@" ;;
    helper|--helper) helper_command "${2:-}" ;;
    updater) updater_command "${2:-}" ;;
    start|--start) start_system ;;
    restart|--restart) restart_system ;;
    stop|--stop) stop_system "${2:-}" ;;
    logs|--logs) show_logs "${2:-}" ;;
    -h|--help|help) usage ;;
    *) usage >&2; exit 2 ;;
esac
