#!/bin/sh
set -u
HERE=$(cd "$(dirname "$0")" && pwd)
SRC=$(cd "$HERE/../.." && pwd)
SHELL_BIN=${1:-sh}
TMP_ROOT=$(mktemp -d "${TMPDIR:-/tmp}/sn-install-scenarios.XXXXXX")
TMP_ROOT=$(cd "$TMP_ROOT" && pwd)
fail=0

check() {
    if [ "$1" = "ok" ]; then
        echo "  ПРОВЕРКА: $2"
    else
        echo "  ОШИБКА: $2"
        fail=1
    fi
}

fresh() {
    WORK=$(mktemp -d "$TMP_ROOT/work.XXXXXX")
    mkdir -p "$WORK/repo/deploy" "$WORK/state" "$WORK/bin"
    cp "$SRC/install.sh" "$WORK/repo/"
    cp "$SRC/deploy/.env.example" "$SRC/deploy/docker-compose.yml" "$SRC/deploy/compose.remnawave.yml" "$SRC/deploy/compose.https.yml" "$SRC/deploy/Caddyfile" "$WORK/repo/deploy/"
    R="$WORK/repo"
    S="$WORK/state"
    FAKE_STATE="$S"
    export FAKE_STATE
}

run() {
    env FAKE_STATE="$S" SN_BIN_DIR="$WORK/bin" SN_TEST_BUSY_PORTS="" PATH="$HERE/bin:$PATH" "$@"
}

INST() {
    printf '%s' "$R/install.sh"
}

echo "################ оболочка: $SHELL_BIN"

echo "=== A. первый запуск без вопросов"
fresh
run SN_OWNER_EMAIL=owner@example.com SN_DOMAIN=https://Panel.Example.com/ SN_HEALTH_TIMEOUT=30 FAKE_STARTING=2 "$SHELL_BIN" "$R/install.sh" > "$WORK/out1.txt" 2>&1 </dev/null
code=$?
sed -E 's/(Пароль владельца: +).*/\1<скрыт>/' "$WORK/out1.txt"
[ "$code" -eq 0 ] && check ok "код выхода 0" || check no "код выхода $code"
shown=$(sed -n 's/^ *Пароль владельца: *//p' "$WORK/out1.txt")
stored=$(cat "$S/password.txt" 2>/dev/null)
[ -n "$shown" ] && [ "$shown" = "$stored" ] && check ok "показанный пароль совпадает с переданным системе" || check no "пароли расходятся"
printf '%s' "$stored" | grep -Eq '^[A-HJ-NP-Za-km-z2-9]{20}$' && check ok "пароль: 20 знаков из разрешённого набора" || check no "формат пароля"
[ "$(cat "$S/owner.txt")" = "owner@example.com" ] && check ok "владелец создан с нужной почтой" || check no "почта владельца"
grep -q '^SN_PUBLIC_URL=https://Panel.Example.com$' "$R/deploy/.env" && check ok "адрес записан как https://Panel.Example.com" || check no "адрес: $(grep '^SN_PUBLIC_URL' "$R/deploy/.env")"
grep -qE '^POSTGRES_PASSWORD=[0-9a-f]{48}$' "$R/deploy/.env" && check ok "пароль базы создан" || check no "пароль базы"
[ "$(sed -n 's/^SN_SECRET_KEY=//p' "$R/deploy/.env" | base64 -d | wc -c | tr -d ' ')" = "32" ] && check ok "ключ шифрования: 32 байта" || check no "ключ шифрования"
[ "$(ls -l "$R/deploy/.env" | cut -c1-10)" = "-rw-------" ] && check ok "права на .env только у владельца" || check no "права на .env"
grep "create-admin" "$S/calls.log" | grep -q "$stored" && check no "пароль попал в аргументы команды" || check ok "пароль не попал в аргументы команды"
grep -q '^COMPOSE_FILE=.*remnawave' "$R/deploy/.env" && check no "сеть Remnawave подключена без панели" || check ok "без панели сеть Remnawave не подключается"
grep -q '^compose up -d --build' "$S/calls.log" && check ok "без .git собирает из исходников" || check no "нет сборки из исходников"
grep -q "^pull ghcr.io/stealthnet-app/stealthnet-rw-system:main\$" "$S/calls.log" && check ok "без .git пробует последнюю проверенную сборку" || check no "без .git не пробовал последнюю сборку"
chosen=$(sed -n 's/^SN_HTTP_PORT=//p' "$R/deploy/.env")
[ "$chosen" = "8080" ] && check ok "порт выбран: $chosen" || check no "порт: «$chosen»"
grep -q "Порт на сервере:   127.0.0.1:$chosen" "$WORK/out1.txt" && check ok "в итоге показан выбранный порт" || check no "в итоге не тот порт"

echo "=== B. повторный запуск"
cp "$R/deploy/.env" "$WORK/before.env"
run SN_OWNER_EMAIL=other@example.com "$SHELL_BIN" "$R/install.sh" > "$WORK/out2.txt" 2>&1 </dev/null
code=$?
[ "$code" -eq 0 ] && check ok "код выхода 0" || check no "код выхода $code"
cmp -s "$R/deploy/.env" "$WORK/before.env" && check ok ".env не изменился" || check no ".env изменился"
grep -q 'Пароль владельца:' "$WORK/out2.txt" && check no "пароль показан повторно" || check ok "пароль повторно не показан"
grep -q 'уже создан: owner@example.com' "$WORK/out2.txt" && check ok "владелец распознан как созданный" || check no "владелец не распознан"
[ "$(cat "$S/password.txt")" = "$stored" ] && check ok "пароль владельца не менялся" || check no "пароль сменился"
[ "$(cat "$S/owner.txt")" = "owner@example.com" ] && check ok "почта владельца не менялась" || check no "почта сменилась"

echo "=== C. новый пароль владельцу"
run "$SHELL_BIN" "$R/install.sh" --reset-password > "$WORK/out3.txt" 2>&1 </dev/null
code=$?
[ "$code" -eq 0 ] && check ok "код выхода 0" || check no "код выхода $code"
new=$(sed -n 's/^ *Пароль владельца: *//p' "$WORK/out3.txt")
[ -n "$new" ] && [ "$new" != "$stored" ] && [ "$new" = "$(cat "$S/password.txt")" ] && check ok "выдан новый пароль и он передан системе" || check no "сброс пароля"
[ "$(cat "$S/owner.txt")" = "owner@example.com" ] && check ok "пароль сменён у того же владельца" || check no "сменён не тот владелец"

echo "=== D. домен не задан"
fresh
run SN_OWNER_EMAIL=owner@example.com SN_DOMAIN= "$SHELL_BIN" "$R/install.sh" > "$WORK/out.txt" 2>&1 </dev/null
code=$?
[ "$code" -eq 0 ] && check ok "код выхода 0" || check no "код выхода $code"
grep -q '^SN_PUBLIC_URL=$' "$R/deploy/.env" && check ok "вместо адреса-примера пусто" || check no "адрес: $(grep '^SN_PUBLIC_URL' "$R/deploy/.env")"
grep -q 'Адрес панели:      не задан' "$WORK/out.txt" && check ok "в итоге сказано, что адрес не задан" || check no "в итоге нет строки про адрес"

echo "=== E. панель Remnawave на этом сервере"
fresh
run FAKE_REMNA_EXIT=0 SN_OWNER_EMAIL=owner@example.com SN_DOMAIN=panel.test "$SHELL_BIN" "$R/install.sh" > "$WORK/out.txt" 2>&1 </dev/null
grep -q '^COMPOSE_FILE=docker-compose.yml:compose.https.yml:compose.remnawave.yml$' "$R/deploy/.env" && check ok "подключение к сети Remnawave записано" || check no "COMPOSE_FILE: $(grep '^COMPOSE_FILE' "$R/deploy/.env")"
grep -q 'панель найдена' "$WORK/out.txt" && check ok "человеку об этом сказано" || check no "нет сообщения"

echo "=== F. некому отвечать и почта не задана"
fresh
run SN_DOMAIN=panel.test "$SHELL_BIN" "$R/install.sh" > "$WORK/out.txt" 2>&1 </dev/null &
pid=$!
( sleep 20; kill "$pid" 2>/dev/null ) &
watchdog=$!
wait "$pid"
code=$?
kill "$watchdog" 2>/dev/null
[ "$code" -eq 1 ] && check ok "остановился с кодом 1, а не завис" || check no "код выхода $code"
grep -q 'некому ответить' "$WORK/out.txt" && check ok "объяснил, что делать" || check no "нет объяснения: $(tail -3 "$WORK/out.txt")"
[ ! -s "$S/owner.txt" ] && check ok "владелец не создан" || check no "владелец создан без почты"

echo "=== G. сборка упала"
fresh
run FAKE_UP_EXIT=1 SN_OWNER_EMAIL=owner@example.com SN_DOMAIN=panel.test "$SHELL_BIN" "$R/install.sh" > "$WORK/out.txt" 2>&1 </dev/null
code=$?
[ "$code" -eq 1 ] && check ok "код выхода 1" || check no "код выхода $code"
grep -q "запустите установку ещё раз: $R/install.sh" "$WORK/out.txt" && check ok "сказано, как продолжить, с полным путём" || check no "нет подсказки"
cp "$R/deploy/.env" "$WORK/before.env"
run SN_OWNER_EMAIL=owner@example.com "$SHELL_BIN" "$R/install.sh" > "$WORK/out2.txt" 2>&1 </dev/null
code=$?
[ "$code" -eq 0 ] && check ok "повторный запуск после сбоя дошёл до конца" || check no "повтор: код $code"
cmp -s "$R/deploy/.env" "$WORK/before.env" && check ok "секреты после сбоя сохранились" || check no "секреты изменились"

echo "=== H. система не поднялась"
fresh
run FAKE_HEALTH=unhealthy SN_HEALTH_TIMEOUT=6 SN_OWNER_EMAIL=owner@example.com SN_DOMAIN=panel.test "$SHELL_BIN" "$R/install.sh" > "$WORK/out.txt" 2>&1 </dev/null
code=$?
[ "$code" -eq 1 ] && check ok "код выхода 1" || check no "код выхода $code"
grep -q 'не ответила за 6 секунд' "$WORK/out.txt" && check ok "сказано, что система не ответила" || check no "нет сообщения"
[ ! -s "$S/owner.txt" ] && check ok "владелец не создавался" || check no "владелец создан на неработающей системе"

echo "=== I. Docker не отвечает"
fresh
run FAKE_INFO_EXIT=1 SN_OWNER_EMAIL=owner@example.com SN_DOMAIN=panel.test "$SHELL_BIN" "$R/install.sh" > "$WORK/out.txt" 2>&1 </dev/null
code=$?
[ "$code" -eq 1 ] && check ok "код выхода 1" || check no "код выхода $code"
grep -q "sudo $R/install.sh" "$WORK/out.txt" && check ok "подсказка про права с полным путём" || check no "нет подсказки"
[ ! -f "$R/deploy/.env" ] && check ok ".env не создавался" || check no ".env создан до проверки Docker"

echo "=== J. неверные домен и почта из окружения"
fresh
run SN_OWNER_EMAIL=owner@example.com SN_DOMAIN="bad domain" "$SHELL_BIN" "$R/install.sh" > "$WORK/out.txt" 2>&1 </dev/null
[ $? -eq 1 ] && grep -q 'не похоже на домен' "$WORK/out.txt" && check ok "неверный домен отклонён" || check no "неверный домен принят"
fresh
run SN_OWNER_EMAIL=not-an-email SN_DOMAIN=panel.test "$SHELL_BIN" "$R/install.sh" > "$WORK/out.txt" 2>&1 </dev/null
[ $? -eq 1 ] && grep -q 'не похоже на почту' "$WORK/out.txt" && check ok "неверная почта отклонена" || check no "неверная почта принята"
[ ! -s "$S/owner.txt" ] && check ok "владелец с неверной почтой не создан" || check no "владелец создан"

SHA=0123456789abcdef0123456789abcdef01234567
REPO=ghcr.io/stealthnet-app/stealthnet-rw-system

echo "=== K. готовая сборка есть"
fresh
mkdir -p "$R/.git/refs/heads"
printf 'ref: refs/heads/main\n' > "$R/.git/HEAD"
printf '%s\n' "$SHA" > "$R/.git/refs/heads/main"
run FAKE_PULL_EXIT=0 SN_OWNER_EMAIL=owner@example.com SN_DOMAIN=panel.test "$SHELL_BIN" "$R/install.sh" > "$WORK/out.txt" 2>&1 </dev/null
code=$?
[ "$code" -eq 0 ] && check ok "код выхода 0" || check no "код выхода $code"
grep -q "^pull $REPO:sha-$SHA\$" "$S/calls.log" && check ok "скачивает сборку именно этого коммита" || check no "pull: $(grep '^pull' "$S/calls.log")"
grep -q '^compose up -d --no-build' "$S/calls.log" && check ok "запускает без сборки" || check no "нет запуска без сборки"
grep -q -- '--build' "$S/calls.log" && check no "собирал из исходников" || check ok "из исходников не собирал"
grep -q "^SN_IMAGE=$REPO:sha-$SHA\$" "$R/deploy/.env" && check ok "сборка записана в .env" || check no "SN_IMAGE: $(grep '^SN_IMAGE' "$R/deploy/.env")"
grep -q 'готовая сборка скачана' "$WORK/out.txt" && check ok "человеку сказано про готовую сборку" || check no "нет сообщения"
[ -s "$S/owner.txt" ] && check ok "владелец создан" || check no "владелец не создан"

echo "=== L. готовая сборка для другого процессора"
fresh
mkdir -p "$R/.git"
printf '%s\n' "$SHA" > "$R/.git/HEAD"
run FAKE_PULL_EXIT=0 FAKE_HOST_ARCH=aarch64 FAKE_IMAGE_ARCH=amd64 SN_OWNER_EMAIL=owner@example.com SN_DOMAIN=panel.test "$SHELL_BIN" "$R/install.sh" > "$WORK/out.txt" 2>&1 </dev/null
code=$?
[ "$code" -eq 0 ] && check ok "код выхода 0" || check no "код выхода $code"
grep -q '^compose up -d --build' "$S/calls.log" && check ok "собирает из исходников" || check no "нет сборки"
grep -q "^image rm $REPO:sha-$SHA" "$S/calls.log" && check ok "чужая сборка удалена" || check no "чужая сборка осталась"
grep -q '^SN_IMAGE=$' "$R/deploy/.env" && check ok "в .env сборка не записана" || check no "SN_IMAGE: $(grep '^SN_IMAGE' "$R/deploy/.env")"
grep -q 'для другого процессора' "$WORK/out.txt" && check ok "человеку объяснено" || check no "нет объяснения"

echo "=== M. готовой сборки нет"
fresh
mkdir -p "$R/.git"
printf '%s\n' "$SHA" > "$R/.git/HEAD"
run FAKE_PULL_EXIT=1 SN_OWNER_EMAIL=owner@example.com SN_DOMAIN=panel.test "$SHELL_BIN" "$R/install.sh" > "$WORK/out.txt" 2>&1 </dev/null
code=$?
[ "$code" -eq 0 ] && check ok "код выхода 0" || check no "код выхода $code"
grep -q '^compose up -d --build' "$S/calls.log" && check ok "собирает из исходников" || check no "нет сборки"
grep -q 'manifest unknown' "$WORK/out.txt" && check no "показана сырая ошибка Docker" || check ok "сырая ошибка Docker скрыта"
grep -q 'соберу из исходников' "$WORK/out.txt" && check ok "человеку объяснено" || check no "нет объяснения"

echo "=== N. сборка из исходников по просьбе"
fresh
mkdir -p "$R/.git"
printf '%s\n' "$SHA" > "$R/.git/HEAD"
run SN_BUILD=1 FAKE_PULL_EXIT=0 SN_OWNER_EMAIL=owner@example.com SN_DOMAIN=panel.test "$SHELL_BIN" "$R/install.sh" > "$WORK/out.txt" 2>&1 </dev/null
grep -q '^pull' "$S/calls.log" && check no "скачивал сборку при SN_BUILD=1" || check ok "при SN_BUILD=1 сборку не скачивает"
grep -q '^compose up -d --build' "$S/calls.log" && check ok "собирает из исходников" || check no "нет сборки"

echo "=== O. повторный запуск без домена и без терминала"
fresh
run SN_OWNER_EMAIL=owner@example.com SN_DOMAIN=panel.example.com "$SHELL_BIN" "$R/install.sh" > "$WORK/out1.txt" 2>&1 </dev/null
cp "$R/deploy/.env" "$WORK/before.env"
run "$SHELL_BIN" "$R/install.sh" > "$WORK/out2.txt" 2>&1 </dev/null
code=$?
[ "$code" -eq 0 ] && check ok "код выхода 0" || check no "код выхода $code: $(tail -2 "$WORK/out2.txt")"
grep -q 'Домен:' "$WORK/out2.txt" && check no "снова спросил домен" || check ok "домен повторно не спрашивает"
cmp -s "$R/deploy/.env" "$WORK/before.env" && check ok ".env не изменился" || check no ".env изменился"
grep -q '^SN_PUBLIC_URL=https://panel.example.com$' "$R/deploy/.env" && check ok "адрес panel.example.com сохранён" || check no "адрес потерян"

echo "=== P. первый запуск без домена и без терминала"
fresh
run SN_OWNER_EMAIL=owner@example.com "$SHELL_BIN" "$R/install.sh" > "$WORK/out.txt" 2>&1 </dev/null
code=$?
[ "$code" -eq 0 ] && check ok "код выхода 0" || check no "код выхода $code: $(tail -2 "$WORK/out.txt")"
grep -q '^SN_PUBLIC_URL=$' "$R/deploy/.env" && check ok "адрес оставлен пустым" || check no "адрес: $(grep '^SN_PUBLIC_URL' "$R/deploy/.env")"
[ -s "$S/owner.txt" ] && check ok "владелец создан" || check no "владелец не создан"

DIGEST=ghcr.io/stealthnet-app/stealthnet-rw-system@sha256:1111

echo "=== Q. сборки этого коммита ещё нет, есть последняя проверенная"
fresh
mkdir -p "$R/.git"
printf '%s\n' "$SHA" > "$R/.git/HEAD"
run FAKE_PULL_OK=":main" SN_OWNER_EMAIL=owner@example.com SN_DOMAIN=panel.test "$SHELL_BIN" "$R/install.sh" > "$WORK/out.txt" 2>&1 </dev/null
code=$?
[ "$code" -eq 0 ] && check ok "код выхода 0" || check no "код выхода $code"
grep -q "^pull $REPO:sha-$SHA\$" "$S/calls.log" && check ok "сначала пробует сборку своего коммита" || check no "не пробовал сборку своего коммита"
grep -q "^pull $REPO:main\$" "$S/calls.log" && check ok "затем берёт последнюю проверенную" || check no "не взял последнюю проверенную"
grep -q -- '--build' "$S/calls.log" && check no "собирал из исходников" || check ok "из исходников не собирал"
grep -q '^compose up -d --no-build' "$S/calls.log" && check ok "запускает без сборки" || check no "нет запуска без сборки"
grep -q "^SN_IMAGE=$DIGEST\$" "$R/deploy/.env" && check ok "сборка закреплена по отпечатку" || check no "SN_IMAGE: $(grep '^SN_IMAGE' "$R/deploy/.env")"
grep -q 'готовая сборка скачана' "$WORK/out.txt" && check ok "человеку сказано про готовую сборку" || check no "нет сообщения"

echo "=== R. повторный запуск, новой сборки по-прежнему нет"
cp "$R/deploy/.env" "$WORK/before.env"
: > "$S/calls.log"
run SN_OWNER_EMAIL=owner@example.com "$SHELL_BIN" "$R/install.sh" > "$WORK/out2.txt" 2>&1 </dev/null
code=$?
[ "$code" -eq 0 ] && check ok "код выхода 0" || check no "код выхода $code"
cmp -s "$R/deploy/.env" "$WORK/before.env" && check ok ".env не изменился" || check no ".env изменился"
grep -q "^pull $REPO:main\$" "$S/calls.log" && check no "повторно скачивал последнюю сборку" || check ok "установленную сборку не меняет"
grep -q 'оставляю установленную' "$WORK/out2.txt" && check ok "человеку сказано, что сборка прежняя" || check no "нет сообщения"
grep -q -- '--build' "$S/calls.log" && check no "собирал из исходников" || check ok "из исходников не собирал"

echo "=== S. сборка своего коммита появилась"
: > "$S/calls.log"
run FAKE_PULL_OK=":sha-$SHA" SN_OWNER_EMAIL=owner@example.com "$SHELL_BIN" "$R/install.sh" > "$WORK/out3.txt" 2>&1 </dev/null
code=$?
[ "$code" -eq 0 ] && check ok "код выхода 0" || check no "код выхода $code"
grep -q "^SN_IMAGE=$REPO:sha-$SHA\$" "$R/deploy/.env" && check ok "перешёл на сборку своего коммита" || check no "SN_IMAGE: $(grep '^SN_IMAGE' "$R/deploy/.env")"

echo "=== T. без .git, последняя проверенная есть"
fresh
run FAKE_PULL_OK=":main" SN_OWNER_EMAIL=owner@example.com SN_DOMAIN=panel.test "$SHELL_BIN" "$R/install.sh" > "$WORK/out.txt" 2>&1 </dev/null
code=$?
[ "$code" -eq 0 ] && check ok "код выхода 0" || check no "код выхода $code"
grep -q "^SN_IMAGE=$DIGEST\$" "$R/deploy/.env" && check ok "взята последняя проверенная сборка" || check no "SN_IMAGE: $(grep '^SN_IMAGE' "$R/deploy/.env")"
grep -q -- '--build' "$S/calls.log" && check no "собирал из исходников" || check ok "из исходников не собирал"

echo "=== U. смена домена после установки"
fresh
run SN_OWNER_EMAIL=owner@example.com SN_DOMAIN= "$SHELL_BIN" "$R/install.sh" > "$WORK/out1.txt" 2>&1 </dev/null
grep -q 'Адрес панели:      не задан' "$WORK/out1.txt" && grep -q "$WORK/bin/stealthnet domain" "$WORK/out1.txt" && check ok "в итоге сказано, как указать домен" || check no "нет подсказки про домен"
: > "$S/calls.log"
run SN_DOMAIN=https://new.example.org/ "$SHELL_BIN" "$R/install.sh" --domain > "$WORK/out2.txt" 2>&1 </dev/null
code=$?
[ "$code" -eq 0 ] && check ok "код выхода 0" || check no "код выхода $code: $(tail -2 "$WORK/out2.txt")"
grep -q '^SN_PUBLIC_URL=https://new.example.org$' "$R/deploy/.env" && check ok "домен записан" || check no "домен: $(grep '^SN_PUBLIC_URL' "$R/deploy/.env")"
grep -q '^compose up -d --no-build' "$S/calls.log" && check ok "система перезапущена с новым адресом" || check no "нет перезапуска"
run SN_DOMAIN="bad domain" "$SHELL_BIN" "$R/install.sh" --domain > "$WORK/out3.txt" 2>&1 </dev/null
[ $? -eq 1 ] && grep -q '^SN_PUBLIC_URL=https://new.example.org$' "$R/deploy/.env" && check ok "неверный домен отклонён, прежний сохранён" || check no "неверный домен принят"

echo "=== V. смена почты владельца"
run SN_OWNER_EMAIL=boss@example.org "$SHELL_BIN" "$R/install.sh" --owner-email > "$WORK/out4.txt" 2>&1 </dev/null
code=$?
[ "$code" -eq 0 ] && check ok "код выхода 0" || check no "код выхода $code: $(tail -2 "$WORK/out4.txt")"
[ "$(cat "$S/owner.txt")" = "boss@example.org" ] && check ok "почта владельца заменена" || check no "почта: $(cat "$S/owner.txt")"
grep -q 'Сейчас: owner@example.com' "$WORK/out4.txt" && check ok "показана прежняя почта" || check no "прежняя почта не показана"
run FAKE_UPDATE_EXIT=1 SN_OWNER_EMAIL=taken@example.org "$SHELL_BIN" "$R/install.sh" --owner-email > "$WORK/out5.txt" 2>&1 </dev/null
[ $? -eq 1 ] && [ "$(cat "$S/owner.txt")" = "boss@example.org" ] && check ok "занятая почта отклонена, прежняя сохранена" || check no "занятая почта принята"

echo "=== W. команды до установки"
fresh
run "$SHELL_BIN" "$R/install.sh" --domain > "$WORK/out.txt" 2>&1 </dev/null
[ $? -eq 1 ] && grep -q 'ещё не установлена' "$WORK/out.txt" && check ok "смена домена до установки объяснена" || check no "смена домена до установки"
run "$SHELL_BIN" "$R/install.sh" --owner-email > "$WORK/out.txt" 2>&1 </dev/null
[ $? -eq 1 ] && grep -q 'ещё не установлена' "$WORK/out.txt" && check ok "смена почты до установки объяснена" || check no "смена почты до установки"

echo "=== X. короткая команда"
fresh
run SN_OWNER_EMAIL=owner@example.com SN_DOMAIN=panel.test "$SHELL_BIN" "$R/install.sh" > "$WORK/out.txt" 2>&1 </dev/null
[ -x "$WORK/bin/stealthnet" ] && check ok "команда stealthnet установлена" || check no "команда не установлена"
grep -q "$WORK/bin/stealthnet reset-password" "$WORK/out.txt" && check ok "в итоге команды показаны через неё" || check no "в итоге нет коротких команд"
grep -q '\./install\.sh' "$WORK/out.txt" && check no "в выводе остался относительный путь" || check ok "относительных путей в выводе нет"
run "$WORK/bin/stealthnet" --help > "$WORK/help.txt" 2>&1 </dev/null
[ $? -eq 0 ] && grep -q 'сменить почту владельца' "$WORK/help.txt" && check ok "короткая команда запускает установщик" || check no "короткая команда не работает"
: > "$S/calls.log"
env FAKE_STATE="$S" SN_BIN_DIR="$WORK/bin" PATH="$HERE/bin:$WORK/bin:$PATH" SN_DOMAIN=short.example.org stealthnet domain > "$WORK/short.txt" 2>&1 </dev/null
[ $? -eq 0 ] && grep -q '^SN_PUBLIC_URL=https://short.example.org$' "$R/deploy/.env" && check ok "stealthnet domain работает из любой папки" || check no "stealthnet domain не сработал: $(tail -2 "$WORK/short.txt")"
env FAKE_STATE="$S" SN_BIN_DIR="$WORK/bin" PATH="$HERE/bin:$WORK/bin:$PATH" stealthnet help > "$WORK/help2.txt" 2>&1 </dev/null
grep -q '^  stealthnet domain$' "$WORK/help2.txt" && check ok "в справке короткие команды" || check no "в справке нет коротких команд"

echo "=== Y. короткую команду поставить нельзя"
fresh
run SN_BIN_DIR="$WORK/missing" SN_OWNER_EMAIL=owner@example.com SN_DOMAIN=panel.test "$SHELL_BIN" "$R/install.sh" > "$WORK/out.txt" 2>&1 </dev/null
code=$?
[ "$code" -eq 0 ] && check ok "установка прошла" || check no "код выхода $code"
grep -q "$R/install.sh --reset-password" "$WORK/out.txt" && check ok "команды показаны с полным путём" || check no "нет команд с полным путём"
fresh
printf 'чужой файл\n' > "$WORK/bin/stealthnet"
run SN_OWNER_EMAIL=owner@example.com SN_DOMAIN=panel.test "$SHELL_BIN" "$R/install.sh" > "$WORK/out.txt" 2>&1 </dev/null
[ "$(cat "$WORK/bin/stealthnet")" = "чужой файл" ] && check ok "чужой файл с тем же именем не тронут" || check no "чужой файл перезаписан"
grep -q "$R/install.sh --domain" "$WORK/out.txt" && check ok "команды показаны с полным путём" || check no "нет команд с полным путём"

echo "=== Z. обновление"
fresh
mkdir -p "$R/.git"
printf '%s\n' "$SHA" > "$R/.git/HEAD"
run SN_OWNER_EMAIL=owner@example.com SN_DOMAIN=panel.test "$SHELL_BIN" "$R/install.sh" > "$WORK/out1.txt" 2>&1 </dev/null
: > "$S/calls.log"
run "$SHELL_BIN" "$R/install.sh" update > "$WORK/out2.txt" 2>&1 </dev/null
code=$?
[ "$code" -eq 0 ] && check ok "обновление прошло" || check no "код выхода $code: $(tail -2 "$WORK/out2.txt")"
grep -q "^-C $R pull --ff-only\$" "$S/git.log" && check ok "новая версия скачивается" || check no "git pull не вызван"
grep -q '^compose up -d' "$S/calls.log" && check ok "после скачивания система перезапущена" || check no "нет перезапуска"
grep -q 'уже создан: owner@example.com' "$WORK/out2.txt" && check ok "владелец сохранён" || check no "владелец не распознан"
: > "$S/calls.log"
run FAKE_GIT_PULL_EXIT=1 "$SHELL_BIN" "$R/install.sh" update > "$WORK/out3.txt" 2>&1 </dev/null
[ $? -eq 1 ] && ! grep -q '^compose up' "$S/calls.log" && check ok "при сбое скачивания система не трогается" || check no "сбой скачивания не остановил обновление"
fresh
run "$SHELL_BIN" "$R/install.sh" update > "$WORK/out.txt" 2>&1 </dev/null
[ $? -eq 1 ] && grep -q 'git clone' "$WORK/out.txt" && check ok "без git обновление объяснено" || check no "без git нет объяснения"

echo "=== AA. изменённые исходники"
fresh
mkdir -p "$R/.git"
printf '%s\n' "$SHA" > "$R/.git/HEAD"
run FAKE_GIT_STATUS=" M crates/x.rs" FAKE_PULL_EXIT=0 SN_OWNER_EMAIL=owner@example.com SN_DOMAIN=panel.test "$SHELL_BIN" "$R/install.sh" > "$WORK/out.txt" 2>&1 </dev/null
grep -q '^pull' "$S/calls.log" && check no "скачал сборку при изменённых исходниках" || check ok "при изменённых исходниках сборку не скачивает"
grep -q '^compose up -d --build' "$S/calls.log" && check ok "собирает из исходников" || check no "нет сборки"

echo "=== AB. HTTPS поднимается сам"
fresh
run SN_OWNER_EMAIL=owner@example.com SN_DOMAIN=shop.example.org "$SHELL_BIN" "$R/install.sh" > "$WORK/out.txt" 2>&1 </dev/null
code=$?
[ "$code" -eq 0 ] && check ok "код выхода 0" || check no "код выхода $code"
grep -q '^SN_HTTPS=caddy$' "$R/deploy/.env" && check ok "выбран свой веб-сервер с сертификатом" || check no "SN_HTTPS: $(grep '^SN_HTTPS' "$R/deploy/.env")"
grep -q '^SN_SITE=shop.example.org$' "$R/deploy/.env" && check ok "домен передан веб-серверу" || check no "SN_SITE: $(grep '^SN_SITE' "$R/deploy/.env")"
grep -q '^COMPOSE_FILE=docker-compose.yml:compose.https.yml$' "$R/deploy/.env" && check ok "веб-сервер добавлен к службам" || check no "COMPOSE_FILE: $(grep '^COMPOSE_FILE' "$R/deploy/.env")"
grep -q 'сертификат выпущен, https://shop.example.org открывается' "$WORK/out.txt" && check ok "человеку сказано, что сайт открывается" || check no "нет сообщения про сайт"
grep -q 'Адрес панели:      https://shop.example.org$' "$WORK/out.txt" && check ok "в итоге адрес без оговорок" || check no "в итоге адрес с оговоркой"
cp "$R/deploy/.env" "$WORK/before.env"
run SN_TEST_BUSY_PORTS="80 443" "$SHELL_BIN" "$R/install.sh" > "$WORK/out2.txt" 2>&1 </dev/null
cmp -s "$R/deploy/.env" "$WORK/before.env" && check ok "повторный запуск не путает свой веб-сервер с чужим" || check no "повторный запуск сменил режим: $(grep '^SN_HTTPS' "$R/deploy/.env")"

echo "=== AC. сертификат ещё не выпущен"
fresh
run FAKE_CERT=0 SN_SITE_TIMEOUT=3 SN_OWNER_EMAIL=owner@example.com SN_DOMAIN=shop.example.org "$SHELL_BIN" "$R/install.sh" > "$WORK/out.txt" 2>&1 </dev/null
code=$?
[ "$code" -eq 0 ] && check ok "установка дошла до конца" || check no "код выхода $code"
grep -q 'https://shop.example.org (сертификат ещё выпускается)' "$WORK/out.txt" && check ok "в итоге сказано, что сертификат выпускается" || check no "нет оговорки про сертификат"
grep -q 'домен ещё не указывает на этот сервер' "$WORK/out.txt" && check ok "названа вероятная причина" || check no "причина не названа"
[ -s "$S/owner.txt" ] && check ok "владелец создан" || check no "владелец не создан"

echo "=== AD. порты 80 и 443 заняты"
fresh
run SN_TEST_BUSY_PORTS="443" SN_OWNER_EMAIL=owner@example.com SN_DOMAIN=shop.example.org "$SHELL_BIN" "$R/install.sh" > "$WORK/out.txt" 2>&1 </dev/null
code=$?
[ "$code" -eq 0 ] && check ok "занятые порты не отказ" || check no "код выхода $code"
grep -q '^SN_HTTPS=manual$' "$R/deploy/.env" && check ok "свой веб-сервер не ставится" || check no "SN_HTTPS: $(grep '^SN_HTTPS' "$R/deploy/.env")"
grep -q '^COMPOSE_FILE=docker-compose.yml$' "$R/deploy/.env" && check ok "лишних служб нет" || check no "COMPOSE_FILE: $(grep '^COMPOSE_FILE' "$R/deploy/.env")"
grep -q 'reverse_proxy 127.0.0.1:8080' "$R/deploy/proxy-snippet.txt" && grep -q 'proxy_set_header X-Forwarded-For \$proxy_add_x_forwarded_for;' "$R/deploy/proxy-snippet.txt" && check ok "подсказка для чужого веб-сервера записана" || check no "нет подсказки"
grep -q "блок из файла $R/deploy/proxy-snippet.txt" "$WORK/out.txt" && check ok "в итоге сказано, где подсказка" || check no "в итоге нет пути к подсказке"

echo "=== AE. занятый порт приложения"
fresh
run SN_TEST_BUSY_PORTS="8080 8081" SN_OWNER_EMAIL=owner@example.com SN_DOMAIN=shop.example.org "$SHELL_BIN" "$R/install.sh" > "$WORK/out.txt" 2>&1 </dev/null
grep -q '^SN_HTTP_PORT=8082$' "$R/deploy/.env" && check ok "взят первый свободный порт 8082" || check no "порт: $(grep '^SN_HTTP_PORT' "$R/deploy/.env")"
grep -q '8080 занят другой программой, беру 8082' "$WORK/out.txt" && check ok "человеку сказано про порт" || check no "нет сообщения про порт"

echo "=== AF. домен указан позже"
fresh
run SN_OWNER_EMAIL=owner@example.com SN_DOMAIN= "$SHELL_BIN" "$R/install.sh" > "$WORK/out1.txt" 2>&1 </dev/null
grep -q '^COMPOSE_FILE=docker-compose.yml$' "$R/deploy/.env" && check ok "без домена веб-сервер не ставится" || check no "COMPOSE_FILE: $(grep '^COMPOSE_FILE' "$R/deploy/.env")"
run SN_DOMAIN=late.example.org "$SHELL_BIN" "$R/install.sh" domain > "$WORK/out2.txt" 2>&1 </dev/null
code=$?
[ "$code" -eq 0 ] && check ok "команда домена прошла" || check no "код выхода $code: $(tail -2 "$WORK/out2.txt")"
grep -q '^SN_HTTPS=caddy$' "$R/deploy/.env" && grep -q '^COMPOSE_FILE=docker-compose.yml:compose.https.yml$' "$R/deploy/.env" && check ok "после указания домена поднят HTTPS" || check no "HTTPS не включён"
grep -q 'сертификат выпущен, https://late.example.org открывается' "$WORK/out2.txt" && check ok "человеку сказано, что сайт открывается" || check no "нет сообщения про сайт"

echo "=== AG. адрес с портом"
fresh
run SN_OWNER_EMAIL=owner@example.com SN_DOMAIN=shop.example.org:8443 "$SHELL_BIN" "$R/install.sh" > "$WORK/out.txt" 2>&1 </dev/null
grep -q '^SN_HTTPS=manual$' "$R/deploy/.env" && check ok "для адреса с портом свой веб-сервер не ставится" || check no "SN_HTTPS: $(grep '^SN_HTTPS' "$R/deploy/.env")"

echo
if [ "$fail" -eq 0 ]; then echo "ИТОГ ($SHELL_BIN): все проверки прошли"; else echo "ИТОГ ($SHELL_BIN): есть ошибки"; fi
exit "$fail"
