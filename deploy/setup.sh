#!/bin/sh
set -eu

cd "$(dirname "$0")"
umask 077

created=0
if [ ! -f .env ]; then
    cp .env.example .env
    created=1
fi

generated=""

ensure() {
    key=$1
    value=$2
    if grep -q "^${key}=." .env; then
        return 0
    fi
    if grep -q "^${key}=" .env; then
        tmp=$(mktemp)
        sed "s|^${key}=.*|${key}=${value}|" .env > "$tmp"
        cat "$tmp" > .env
        rm -f "$tmp"
    else
        printf '%s=%s\n' "$key" "$value" >> .env
    fi
    generated="${generated} ${key}"
}

random_hex() {
    head -c "$1" /dev/urandom | od -An -tx1 | tr -d ' \n'
}

random_base64() {
    head -c "$1" /dev/urandom | base64 | tr -d '\n'
}

ensure POSTGRES_PASSWORD "$(random_hex 24)"
ensure SN_SECRET_KEY "$(random_base64 32)"

chmod 600 .env

if [ "$created" -eq 1 ]; then
    echo "Создан файл deploy/.env"
fi
if [ -n "$generated" ]; then
    echo "Сгенерировано:${generated}"
    echo "Сохраните deploy/.env: без SN_SECRET_KEY не расшифровать ключи платёжных систем и токены из админки"
else
    echo "В deploy/.env уже всё задано, ничего не менялось"
fi
