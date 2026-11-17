#!/bin/sh
set -eu

ROOT=$(cd "$(dirname "$0")" && pwd)
DEPLOY="$ROOT/deploy"
ENV_FILE="$DEPLOY/.env"
IMAGE_REPO=${SN_IMAGE_REPO:-ghcr.io/stealthnet-app/stealthnet-rw-system}
HEALTH_TIMEOUT=${SN_HEALTH_TIMEOUT:-300}
BIN_DIR=${SN_BIN_DIR:-/usr/local/bin}
WRAPPER="$BIN_DIR/stealthnet"
SHORT=""
USE_PREBUILT=0
TTY_SAVED=""

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
    printf '  %s\n      скачать новую версию и обновить систему\n' "$(command_for update)"
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
        hint "Не похоже на домен. Нужен вид panel.example.com, без пробелов."
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
        hint "Домен, по которому будет открываться панель, например panel.example.com."
        ask_domain optional
    else
        domain=""
    fi
    if [ -z "$domain" ]; then
        done_line "без домена; указать позже: $(command_for domain)"
        return 0
    fi
    if ! valid_domain "$domain"; then
        die "«$domain» не похоже на домен" "Нужен вид panel.example.com."
    fi
    set_env SN_PUBLIC_URL "https://$domain"
    done_line "https://$domain"
}

port_busy() {
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
    if [ -n "$(env_value COMPOSE_FILE)" ]; then
        return 0
    fi
    if docker network inspect remnawave-network >/dev/null 2>&1; then
        set_env COMPOSE_FILE "docker-compose.yml:compose.remnawave.yml"
        step "Remnawave"
        done_line "панель найдена на этом сервере, подключаюсь к её сети"
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

use_image() {
    docker pull "$1" 2>/dev/null || return 1
    if [ "$(docker image inspect -f '{{.Architecture}}' "$1" 2>/dev/null)" != "$(host_arch)" ]; then
        docker image rm "$1" >/dev/null 2>&1 || true
        WRONG_ARCH=1
        return 1
    fi
    pinned=$1
    if [ "${2:-}" = "pin" ]; then
        digest=$(docker image inspect -f '{{index .RepoDigests 0}}' "$1" 2>/dev/null || true)
        case "$digest" in
            *@sha256:*) pinned=$digest ;;
        esac
    fi
    set_env SN_IMAGE "$pinned"
    USE_PREBUILT=1
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
    commit=$(source_commit | tr -d ' \n')
    if printf '%s' "$commit" | grep -Eq '^[0-9a-f]{40}$' && use_image "$IMAGE_REPO:sha-$commit"; then
        done_line "готовая сборка скачана"
        return 0
    fi
    current=$(env_value SN_IMAGE)
    if [ -n "$current" ] && docker image inspect "$current" >/dev/null 2>&1; then
        USE_PREBUILT=1
        done_line "новой сборки пока нет, оставляю установленную"
        return 0
    fi
    if use_image "$IMAGE_REPO:main" pin; then
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
}

summary() {
    url=$(env_value SN_PUBLIC_URL)
    port=$(env_value SN_HTTP_PORT)
    printf '\n%sГотово.%s\n\n' "$BOLD" "$RESET"
    if [ -n "$url" ]; then
        printf '  Адрес панели:      %s\n' "$url"
    else
        printf '  Адрес панели:      не задан\n'
    fi
    printf '  Порт на сервере:   127.0.0.1:%s\n' "${port:-8080}"
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
    prepare_image
    start_stack
    wait_healthy
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
    install_short
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
        hint "Домен, по которому будет открываться панель, например panel.example.com."
        ask_domain required
    fi
    if ! valid_domain "$domain"; then
        die "«$domain» не похоже на домен" "Нужен вид panel.example.com."
    fi
    set_env SN_PUBLIC_URL "https://$domain"
    done_line "https://$domain"
    step "Запуск"
    if ! compose up -d --no-build; then
        die "перезапуск не прошёл" "Сообщение об ошибке — выше."
    fi
    wait_healthy
    printf '\n  Адрес панели:      https://%s\n' "$domain"
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

detect_short

case "${1:-}" in
    ""|install) install ;;
    domain|--domain) change_domain ;;
    owner-email|--owner-email) change_owner_email ;;
    reset-password|--reset-password) reset_password ;;
    update|--update) update ;;
    -h|--help|help) usage ;;
    *) usage >&2; exit 2 ;;
esac
