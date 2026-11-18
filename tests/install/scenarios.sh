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
    cp "$SRC/deploy/.env.example" "$SRC/deploy/docker-compose.yml" "$SRC/deploy/compose.remnawave.yml" "$SRC/deploy/compose.https.yml" "$SRC/deploy/compose.updater.yml" "$SRC/deploy/Caddyfile" "$WORK/repo/deploy/"
    R="$WORK/repo"
    S="$WORK/state"
    FAKE_STATE="$S"
    export FAKE_STATE
}

run() {
    env FAKE_STATE="$S" SN_BIN_DIR="$WORK/bin" SN_TEST_BUSY_PORTS="" SN_UPDATER_STATE="$WORK/updater" SN_SYSTEMD_DIR="$WORK/systemd" SN_VERIFY_PAUSE=0 PATH="$HERE/bin:$PATH" "$@"
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
run FAKE_PULL_OK=":main" FAKE_OTHER_DIGEST="sn-local@sha256:9999" SN_OWNER_EMAIL=owner@example.com SN_DOMAIN=panel.test "$SHELL_BIN" "$R/install.sh" > "$WORK/out.txt" 2>&1 </dev/null
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
run "$SHELL_BIN" "$R/install.sh" start > "$WORK/out.txt" 2>&1 </dev/null
[ $? -eq 1 ] && grep -q 'ещё не установлена' "$WORK/out.txt" && check ok "запуск до установки объяснён" || check no "запуск до установки"

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
grep -q 'client_max_body_size 2g;' "$R/deploy/proxy-snippet.txt" && check ok "nginx пропускает выгрузку старого бота до 2 ГБ" || check no "nginx режет большие файлы выгрузки"
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

echo "=== AG1. www ведёт на тот же сервер"
fresh
printf 'example.com 203.0.113.10\nwww.example.com 203.0.113.10\n' > "$S/hosts"
run SN_OWNER_EMAIL=owner@example.com SN_DOMAIN=example.com "$SHELL_BIN" "$R/install.sh" > "$WORK/out.txt" 2>&1 </dev/null
grep -q '^SN_SITE_WWW=www.example.com$' "$R/deploy/.env" && check ok "www передан веб-серверу" || check no "SN_SITE_WWW: $(grep '^SN_SITE_WWW' "$R/deploy/.env")"
grep -q 'Переадресация:     https://www.example.com → https://example.com$' "$WORK/out.txt" && check ok "в итоге сказано про переадресацию с www" || check no "нет строки про переадресацию"
grep -q 'Админка:           https://example.com/admin$' "$WORK/out.txt" && grep -q 'Кабинет клиентов:  https://example.com/cabinet$' "$WORK/out.txt" && check ok "в итоге адреса админки и кабинета" || check no "нет адресов админки и кабинета"
grep -q 'Чтобы открывался и www' "$WORK/out.txt" && check no "лишняя подсказка про www" || check ok "подсказки про www нет"

echo "=== AG2. записи www нет, потом появилась"
fresh
printf 'example.com 203.0.113.10\n' > "$S/hosts"
run SN_OWNER_EMAIL=owner@example.com SN_DOMAIN=example.com "$SHELL_BIN" "$R/install.sh" > "$WORK/out1.txt" 2>&1 </dev/null
grep -q '^SN_SITE_WWW=$' "$R/deploy/.env" && check ok "без записи www переадресация не включена" || check no "SN_SITE_WWW: $(grep '^SN_SITE_WWW' "$R/deploy/.env")"
grep -q 'Чтобы открывался и www.example.com' "$WORK/out1.txt" && check ok "подсказано, как включить www" || check no "нет подсказки про www"
printf 'www.example.com 203.0.113.10\n' >> "$S/hosts"
run SN_DOMAIN=example.com "$SHELL_BIN" "$R/install.sh" domain > "$WORK/out2.txt" 2>&1 </dev/null
grep -q '^SN_SITE_WWW=www.example.com$' "$R/deploy/.env" && check ok "после записи www команда домена включила переадресацию" || check no "SN_SITE_WWW: $(grep '^SN_SITE_WWW' "$R/deploy/.env")"
grep -q 'Переадресация:     https://www.example.com → https://example.com$' "$WORK/out2.txt" && check ok "после смены домена сказано про переадресацию" || check no "нет строки про переадресацию"

echo "=== AG3. www на другом сервере, поддомен или www выключен"
fresh
printf 'example.com 203.0.113.10\nwww.example.com 198.51.100.7\n' > "$S/hosts"
run SN_OWNER_EMAIL=owner@example.com SN_DOMAIN=example.com "$SHELL_BIN" "$R/install.sh" > "$WORK/out.txt" 2>&1 </dev/null
grep -q '^SN_SITE_WWW=$' "$R/deploy/.env" && check ok "чужой www не перехватывается" || check no "SN_SITE_WWW: $(grep '^SN_SITE_WWW' "$R/deploy/.env")"
fresh
printf 'panel.example.com 203.0.113.10\n' > "$S/hosts"
run SN_OWNER_EMAIL=owner@example.com SN_DOMAIN=panel.example.com "$SHELL_BIN" "$R/install.sh" > "$WORK/out.txt" 2>&1 </dev/null
grep -q 'Чтобы открывался и www' "$WORK/out.txt" && check no "для поддомена предложен www" || check ok "для поддомена www не предлагается"
fresh
printf 'example.com 203.0.113.10\nwww.example.com 203.0.113.10\n' > "$S/hosts"
run SN_WWW=0 SN_OWNER_EMAIL=owner@example.com SN_DOMAIN=example.com "$SHELL_BIN" "$R/install.sh" > "$WORK/out1.txt" 2>&1 </dev/null
grep -q '^SN_SITE_WWW=$' "$R/deploy/.env" && grep -q '^SN_WWW=0$' "$R/deploy/.env" && check ok "SN_WWW=0 выключает www и запоминается" || check no "SN_WWW=0 не сработал"
grep -q 'Чтобы открывался и www' "$WORK/out1.txt" && check no "подсказка при выключенном www" || check ok "при выключенном www подсказки нет"
run SN_DOMAIN=example.com "$SHELL_BIN" "$R/install.sh" domain > "$WORK/out2.txt" 2>&1 </dev/null
grep -q '^SN_SITE_WWW=$' "$R/deploy/.env" && check ok "смена домена помнит, что www выключен" || check no "www включился сам"

echo "=== AG4. www по просьбе, домен на www и чужой веб-сервер"
fresh
run SN_WWW=1 SN_OWNER_EMAIL=owner@example.com SN_DOMAIN=panel.localhost "$SHELL_BIN" "$R/install.sh" > "$WORK/out.txt" 2>&1 </dev/null
grep -q '^SN_SITE_WWW=www.panel.localhost$' "$R/deploy/.env" && check ok "SN_WWW=1 включает www без проверки записи" || check no "SN_SITE_WWW: $(grep '^SN_SITE_WWW' "$R/deploy/.env")"
fresh
run SN_WWW=1 SN_OWNER_EMAIL=owner@example.com SN_DOMAIN=www.example.com "$SHELL_BIN" "$R/install.sh" > "$WORK/out.txt" 2>&1 </dev/null
grep -q '^SN_SITE_WWW=$' "$R/deploy/.env" && check ok "для домена на www второй www не добавляется" || check no "SN_SITE_WWW: $(grep '^SN_SITE_WWW' "$R/deploy/.env")"
fresh
printf 'example.com 203.0.113.10\nwww.example.com 203.0.113.10\n' > "$S/hosts"
run SN_TEST_BUSY_PORTS="443" SN_OWNER_EMAIL=owner@example.com SN_DOMAIN=example.com "$SHELL_BIN" "$R/install.sh" > "$WORK/out.txt" 2>&1 </dev/null
grep -q 'redir https://example.com{uri} permanent' "$R/deploy/proxy-snippet.txt" && grep -q 'return 301 https://example.com\$request_uri;' "$R/deploy/proxy-snippet.txt" && check ok "в подсказке для чужого веб-сервера есть переадресация с www" || check no "в подсказке нет www"

echo "=== AG5. сертификат заново"
fresh
printf 'shop.example.org 203.0.113.10\n' > "$S/hosts"
run SN_OWNER_EMAIL=owner@example.com SN_DOMAIN=shop.example.org "$SHELL_BIN" "$R/install.sh" > "$WORK/out1.txt" 2>&1 </dev/null
grep -q "stealthnet cert" "$WORK/out1.txt" && check ok "команда сертификата есть в итоге установки" || check no "в итоге нет команды сертификата"
: > "$S/calls.log"
run SN_TEST_SERVER_IPS="203.0.113.10" "$SHELL_BIN" "$R/install.sh" cert > "$WORK/out2.txt" 2>&1 </dev/null
code=$?
[ "$code" -eq 0 ] && grep -q 'уже выпущен, https://shop.example.org открывается' "$WORK/out2.txt" && check ok "выпущенный сертификат не трогается" || check no "код $code: $(tail -2 "$WORK/out2.txt")"
grep -q 'shop.example.org → 203.0.113.10' "$WORK/out2.txt" && check ok "показано, куда указывает домен" || check no "нет адреса домена"
grep -q 'Адреса этого сервера' "$WORK/out2.txt" && check no "лишнее предупреждение про адрес" || check ok "адрес совпал, предупреждения нет"
grep -q '^compose restart caddy$' "$S/calls.log" && check no "веб-сервер перезапущен без нужды" || check ok "без нужды веб-сервер не перезапускается"
: > "$S/calls.log"
run FAKE_CERT=0 FAKE_CERT_AFTER_RESTART=1 SN_TEST_SERVER_IPS="198.51.100.7" "$SHELL_BIN" "$R/install.sh" cert > "$WORK/out3.txt" 2>&1 </dev/null
code=$?
[ "$code" -eq 0 ] && grep -q 'сертификат выпущен, https://shop.example.org открывается' "$WORK/out3.txt" && check ok "после перезапуска сертификат выпущен" || check no "код $code: $(tail -3 "$WORK/out3.txt")"
grep -q '^compose restart caddy$' "$S/calls.log" && check ok "веб-сервер перезапущен для новой попытки" || check no "веб-сервер не перезапускался"
grep -q 'Адреса этого сервера: 198.51.100.7' "$WORK/out3.txt" && check ok "предупреждено, что домен указывает не сюда" || check no "нет предупреждения про чужой адрес"
: > "$S/calls.log"
rm -f "$S/cert"
run FAKE_CERT=0 SN_SITE_TIMEOUT=3 FAKE_CADDY_LOG='{"level":"error","logger":"tls.obtain","msg":"could not get certificate from issuer","identifier":"shop.example.org","error":"HTTP 400 urn:ietf:params:acme:error:connection - 203.0.113.10: Timeout during connect (likely firewall problem)"}' "$SHELL_BIN" "$R/install.sh" cert > "$WORK/out4.txt" 2>&1 </dev/null
code=$?
[ "$code" -eq 1 ] && check ok "невыпущенный сертификат даёт код 1" || check no "код $code"
grep -q 'не достучался до порта 80' "$WORK/out4.txt" && grep -q 'Ошибка Caddy: HTTP 400' "$WORK/out4.txt" && check ok "причина из журнала Caddy показана" || check no "нет причины: $(tail -4 "$WORK/out4.txt")"
: > "$S/calls.log"
run "$SHELL_BIN" "$R/install.sh" cert --force > "$WORK/out5.txt" 2>&1 </dev/null
code=$?
[ "$code" -eq 0 ] && grep -q 'прежний сертификат удалён' "$WORK/out5.txt" && grep -q 'сертификат выпущен, https://shop.example.org открывается' "$WORK/out5.txt" && check ok "--force выпускает заново" || check no "код $code: $(tail -3 "$WORK/out5.txt")"
grep -q 'find /data/caddy/certificates -name shop.example.org.\* -delete' "$S/calls.log" && grep -q '^compose restart caddy$' "$S/calls.log" && check ok "--force удаляет сертификат и перезапускает веб-сервер" || check no "--force не удалил сертификат"
run "$SHELL_BIN" "$R/install.sh" cert --again > "$WORK/out6.txt" 2>&1 </dev/null
[ $? -eq 1 ] && grep -q 'не понял «--again»' "$WORK/out6.txt" && check ok "неизвестный ключ объяснён" || check no "неизвестный ключ"
fresh
run SN_TEST_BUSY_PORTS="443" SN_OWNER_EMAIL=owner@example.com SN_DOMAIN=shop.example.org "$SHELL_BIN" "$R/install.sh" > "$WORK/out1.txt" 2>&1 </dev/null
run "$SHELL_BIN" "$R/install.sh" cert > "$WORK/out2.txt" 2>&1 </dev/null
[ $? -eq 1 ] && grep -q 'выпускает ваш веб-сервер' "$WORK/out2.txt" && check ok "при чужом веб-сервере объяснено, где сертификат" || check no "чужой веб-сервер: $(tail -2 "$WORK/out2.txt")"
fresh
run SN_OWNER_EMAIL=owner@example.com SN_DOMAIN= "$SHELL_BIN" "$R/install.sh" > "$WORK/out1.txt" 2>&1 </dev/null
run "$SHELL_BIN" "$R/install.sh" cert > "$WORK/out2.txt" 2>&1 </dev/null
[ $? -eq 1 ] && grep -q 'домен не задан' "$WORK/out2.txt" && check ok "без домена сказано, что делать" || check no "без домена: $(tail -2 "$WORK/out2.txt")"

NEW=fedcba9876543210fedcba9876543210fedcba98
NEWDIGEST=ghcr.io/stealthnet-app/stealthnet-rw-system@sha256:2222
ME=$(id -u)
U="$WORK/updater"

helper_install() {
    fresh
    U="$WORK/updater"
    mkdir -p "$R/.git"
    printf '%s\n' "$SHA" > "$R/.git/HEAD"
    printf '%s|%s|%s|%s\n' "$REPO:sha-$SHA" "$SHA" "2026-11-17T10:00:00Z" "0.1.59" "$NEWDIGEST" "$NEW" "2026-11-17T13:00:00Z" "0.1.60" > "$S/labels"
    run SN_HELPER_TEST=1 SN_UPDATER_APP_UID="$ME" FAKE_PULL_OK=":sha-$SHA" SN_OWNER_EMAIL=owner@example.com SN_DOMAIN=panel.test "$SHELL_BIN" "$R/install.sh" > "$WORK/install.txt" 2>&1 </dev/null
}

request() {
    printf '{"id":"%s","action":"update","target":"%s","requested_at":"%s","admin_id":1}\n' "$1" "$2" "${3:-$(date -u +%Y-%m-%dT%H:%M:%SZ)}" > "$U/ipc/request/.tmp"
    mv "$U/ipc/request/.tmp" "$U/ipc/request/request.json"
}

updater() {
    run SN_HELPER_TEST=1 SN_UPDATER_APP_UID="$ME" "$@" "$SHELL_BIN" "$R/install.sh" updater run > "$WORK/updater.txt" 2>&1 </dev/null
}

field() {
    grep -o "\"$1\":\"[^\"]*\"" "$U/ipc/status/update.json" 2>/dev/null | sed -n '1p' | sed 's/^"[a-z_]*":"//; s/"$//'
}

echo "=== AH. помощник ставится вместе с системой"
helper_install
code=$?
[ "$code" -eq 0 ] && check ok "установка прошла" || check no "код выхода $code: $(tail -3 "$WORK/install.txt")"
grep -q 'Обновление из админки' "$WORK/install.txt" && grep -q 'включено, обновлять может только владелец с паролем' "$WORK/install.txt" && check ok "человеку сказано, что обновление из админки включено" || check no "нет сообщения о помощнике"
grep -qx "PathExists=$U/ipc/request/request.json" "$WORK/systemd/stealthnet-rw-update.path" && check ok "слежение за заявкой записано" || check no "нет unit-файла слежения"
grep -qx "ExecStart=/bin/sh $R/install.sh updater run" "$WORK/systemd/stealthnet-rw-update.service" && grep -qx "ExecStopPost=/bin/sh $R/install.sh updater recover" "$WORK/systemd/stealthnet-rw-update.service" && check ok "служба обновления и страховка записаны" || check no "служба обновления"
grep -qx 'StartLimitIntervalSec=0' "$WORK/systemd/stealthnet-rw-update.service" && check ok "у службы нет предела частоты запусков" || check no "предел частоты"
grep -qx "ExecStart=/bin/sh $R/install.sh updater recover" "$WORK/systemd/stealthnet-rw-recover.service" && check ok "восстановление при загрузке сервера записано" || check no "нет службы восстановления"
grep -qx 'enable --now stealthnet-rw-update.path' "$S/systemctl.log" && grep -qx 'enable stealthnet-rw-recover.service' "$S/systemctl.log" && check ok "службы включены" || check no "systemctl: $(tr '\n' ';' < "$S/systemctl.log")"
grep -q '^COMPOSE_FILE=.*compose.updater.yml$' "$R/deploy/.env" && grep -qx "SN_UPDATER_STATE=$U" "$R/deploy/.env" && check ok "папки обмена подключены к приложению" || check no "COMPOSE_FILE: $(grep '^COMPOSE_FILE' "$R/deploy/.env")"
grep -q '"panel_updates":true' "$U/ipc/status/helper.json" && check ok "приложение видит, что помощник включён" || check no "helper.json"
[ "$(ls -ld "$U/ipc/request" | cut -c1-10)" = "drwx------" ] && [ "$(ls -ld "$U/ipc/status" | cut -c1-10)" = "drwxr-xr-x" ] && [ "$(ls -ld "$U/run" | cut -c1-10)" = "drwx------" ] && check ok "права на папки обмена" || check no "права: $(ls -ld "$U/ipc/request" "$U/ipc/status" "$U/run")"
grep -qx "SN_IMAGE=$REPO:sha-$SHA" "$R/deploy/.env" && check ok "стоит сборка своего коммита" || check no "SN_IMAGE: $(grep '^SN_IMAGE' "$R/deploy/.env")"

echo "=== AI. без root помощник не ставится"
fresh
run SN_OWNER_EMAIL=owner@example.com SN_DOMAIN=panel.test "$SHELL_BIN" "$R/install.sh" > "$WORK/out.txt" 2>&1 </dev/null
grep -q 'не включено: нужны права root' "$WORK/out.txt" && check ok "объяснено, почему обновление из админки не включено" || check no "нет объяснения"
[ ! -e "$WORK/systemd/stealthnet-rw-update.path" ] && ! grep -q 'compose.updater.yml' "$R/deploy/.env" && check ok "unit-файлов и папок обмена нет" || check no "помощник поставлен без root"

echo "=== AJ. обновление по заявке из админки"
helper_install
ID=0b6c5d0e-6a1f-4c1e-9d55-3f0c2a7b9e10
request "$ID" "$NEW"
: > "$S/calls.log"
updater FAKE_PULL_OK=":main" FAKE_DIGEST="$NEWDIGEST" FAKE_OTHER_DIGEST="sn-local@sha256:9999"
code=$?
[ "$code" -eq 0 ] && check ok "помощник отработал" || check no "код выхода $code: $(tail -3 "$WORK/updater.txt")"
[ "$(field state)" = "done" ] && check ok "итог: обновлено" || check no "итог: $(cat "$U/ipc/status/update.json")"
[ "$(field id)" = "$ID" ] && [ "$(field from)" = "$SHA" ] && [ "$(field to)" = "$NEW" ] && check ok "в ходе номер заявки и версии" || check no "ход: $(cat "$U/ipc/status/update.json")"
[ "$(field from_version)" = "0.1.59" ] && [ "$(field to_version)" = "0.1.60" ] && check ok "в ходе человеческие имена версий" || check no "имена версий: $(cat "$U/ipc/status/update.json")"
[ "$(grep -o '"state":"done"' "$U/ipc/status/update.json" | wc -l | tr -d ' ')" = "7" ] && check ok "все шесть шагов сделаны" || check no "шаги: $(cat "$U/ipc/status/update.json")"
[ ! -e "$U/ipc/request/request.json" ] && check ok "заявка забрана" || check no "заявка осталась"
grep -qx "SN_IMAGE=$NEWDIGEST" "$R/deploy/.env" && check ok "запущена новая сборка, закреплённая по отпечатку" || check no "SN_IMAGE: $(grep '^SN_IMAGE' "$R/deploy/.env")"
grep -q "image=$NEWDIGEST .*@postgres:5432/stealthnet_next" "$S/migrations" && check ok "миграции новой версии шли на копии базы" || check no "миграции: $(cat "$S/migrations" 2>/dev/null)"
grep -qx 'stealthnet|' "$S/dbs" && grep -qx "stealthnet_prev|stealthnet-update:$ID:prev" "$S/dbs" && ! grep -q '^stealthnet_next|' "$S/dbs" && check ok "базы переставлены, прежняя сохранена с меткой" || check no "базы: $(tr '\n' ';' < "$S/dbs")"
grep -n '^compose stop -t 60 app$' "$S/calls.log" > /dev/null && [ "$(grep -n 'pg_dump' "$S/calls.log" | cut -d: -f1)" -gt "$(grep -n '^compose stop -t 60 app$' "$S/calls.log" | cut -d: -f1)" ] && check ok "копия базы снята после остановки панели" || check no "порядок остановки и копии"
dump=$(ls "$R/deploy/backups"/*.dump 2>/dev/null | head -n 1)
[ -n "$dump" ] && [ -f "${dump%.dump}.env" ] && [ "$(ls -l "$dump" | cut -c1-10)" = "-rw-------" ] && check ok "копия базы и файла с ключами лежит рядом, только для root" || check no "копия: $(ls -l "$R/deploy/backups" 2>&1)"
grep -q "\"id\":\"$ID\".*\"state\":\"done\"" "$U/ipc/status/history.jsonl" && check ok "попытка записана в историю" || check no "история: $(cat "$U/ipc/status/history.jsonl" 2>/dev/null)"
[ ! -e "$U/run/journal" ] && [ ! -e "$U/run/current" ] && [ ! -e "$U/run/lock" ] && check ok "журнал шагов и блокировка убраны" || check no "остались: $(ls "$U/run")"
grep -q '"downtime":[0-9]' "$U/ipc/status/update.json" && check ok "записано, сколько панель была недоступна" || check no "нет простоя"

echo "=== AK. пока владелец подтверждал, вышла другая сборка"
helper_install
request 1b6c5d0e-6a1f-4c1e-9d55-3f0c2a7b9e10 abcdefabcdefabcdefabcdefabcdefabcdefabcd
: > "$S/calls.log"
updater FAKE_PULL_OK=":main" FAKE_DIGEST="$NEWDIGEST"
[ "$(field state)" = "failed" ] && [ "$(field error)" = "stale_request" ] && check ok "отказ: заявка устарела" || check no "итог: $(cat "$U/ipc/status/update.json")"
! grep -q '^compose stop' "$S/calls.log" && grep -qx "SN_IMAGE=$REPO:sha-$SHA" "$R/deploy/.env" && check ok "панель не останавливалась, сборка прежняя" || check no "панель тронута"

echo "=== AL. миграция не прошла"
helper_install
ID=2b6c5d0e-6a1f-4c1e-9d55-3f0c2a7b9e10
request "$ID" "$NEW"
: > "$S/calls.log"
updater FAKE_PULL_OK=":main" FAKE_DIGEST="$NEWDIGEST" FAKE_MIGRATE_EXIT=1
[ "$(field state)" = "rolled_back" ] && [ "$(field error)" = "migrate_failed" ] && check ok "итог: возвращена прежняя версия" || check no "итог: $(cat "$U/ipc/status/update.json")"
grep -qx 'stealthnet|' "$S/dbs" && [ "$(wc -l < "$S/dbs" | tr -d ' ')" = "1" ] && check ok "база не тронута, копия удалена" || check no "базы: $(tr '\n' ';' < "$S/dbs")"
grep -qx "SN_IMAGE=$REPO:sha-$SHA" "$R/deploy/.env" && [ -f "$S/started" ] && [ "$(tail -n 1 "$S/started_images")" = "$REPO:sha-$SHA" ] && check ok "прежняя сборка снова запущена" || check no "запущено: $(tail -n 1 "$S/started_images")"
grep -q "\"blocked\":\"$NEW\"" "$U/ipc/status/helper.json" && check ok "версия помечена как не подошедшая серверу" || check no "helper.json: $(cat "$U/ipc/status/helper.json")"
request 3b6c5d0e-6a1f-4c1e-9d55-3f0c2a7b9e10 "$NEW"
updater FAKE_PULL_OK=":main" FAKE_DIGEST="$NEWDIGEST"
[ "$(field state)" = "failed" ] && [ "$(field error)" = "rate_limited" ] && check ok "повтор сразу после простоя отклонён" || check no "итог повтора: $(cat "$U/ipc/status/update.json")"
rm -f "$U/run/last_failure"
request 4b6c5d0e-6a1f-4c1e-9d55-3f0c2a7b9e10 "$NEW"
updater FAKE_PULL_OK=":main" FAKE_DIGEST="$NEWDIGEST"
[ "$(field error)" = "blocked" ] && check ok "не подошедшая версия из админки не ставится" || check no "итог: $(cat "$U/ipc/status/update.json")"

echo "=== AM. новая версия запустилась, но не отвечает"
helper_install
ID=5b6c5d0e-6a1f-4c1e-9d55-3f0c2a7b9e10
request "$ID" "$NEW"
updater FAKE_PULL_OK=":main" FAKE_DIGEST="$NEWDIGEST" FAKE_APP_HEALTH=1 SN_VERIFY_TIMEOUT=6
[ "$(field state)" = "attention" ] && [ "$(field error)" = "unhealthy" ] && check ok "итог: нужно внимание" || check no "итог: $(cat "$U/ipc/status/update.json")"
grep -qx "stealthnet_prev|stealthnet-update:$ID:prev" "$S/dbs" && grep -qx "SN_IMAGE=$NEWDIGEST" "$R/deploy/.env" && check ok "после запуска база сама не возвращается" || check no "базы: $(tr '\n' ';' < "$S/dbs")"
grep -q "\"blocked\":\"$NEW\"" "$U/ipc/status/helper.json" && check ok "версия помечена как не подошедшая" || check no "helper.json"

echo "=== AN. неверные заявки"
helper_install
ln -s /etc/passwd "$U/ipc/request/request.json"
updater
[ ! -e "$U/ipc/request/request.json" ] && [ ! -L "$U/ipc/request/request.json" ] && [ ! -e "$U/ipc/status/update.json" ] && check ok "ссылка вместо файла убрана и не исполнена" || check no "ссылка: $(ls -l "$U/ipc/request")"
printf '{"id":"6b6c5d0e-6a1f-4c1e-9d55-3f0c2a7b9e10","action":"rollback","target":"%s"}\n' "$NEW" > "$U/ipc/request/request.json"
: > "$S/calls.log"
updater FAKE_PULL_OK=":main"
[ "$(field state)" = "failed" ] && [ "$(field error)" = "invalid_request" ] && ! grep -q '^pull' "$S/calls.log" && check ok "заявка неверного вида отклонена до любых действий" || check no "итог: $(cat "$U/ipc/status/update.json" 2>/dev/null)"
request 7b6c5d0e-6a1f-4c1e-9d55-3f0c2a7b9e10 "$NEW" "2020-01-01T00:00:00Z"
updater FAKE_PULL_OK=":main"
[ "$(field error)" = "expired" ] && check ok "старая заявка отклонена" || check no "итог: $(cat "$U/ipc/status/update.json")"
mkdir "$U/ipc/request/request.json"
updater
[ ! -e "$U/ipc/request/request.json" ] && check ok "папка вместо заявки убрана" || check no "папка осталась"

echo "=== AO. обновление прервалось"
helper_install
ID=8b6c5d0e-6a1f-4c1e-9d55-3f0c2a7b9e10
printf 'stealthnet|stealthnet-update:%s:copy\nstealthnet_prev|stealthnet-update:%s:prev\n' "$ID" "$ID" > "$S/dbs"
printf 'begin\napp_stopped\nbackup_written\ndb_cloned\ndb_swapping\ndb_swapped\n' > "$U/run/journal"
printf 'U_ID=%s\nU_ORIGIN=panel\nU_ADMIN=1\nU_FROM=%s\nU_TO=%s\nU_STATE=running\nFROM_IMAGE=%s\nTO_IMAGE=%s\nSTART_EPOCH=%s\nDOWN_EPOCH=%s\nS_check=done\nS_download=done\nS_backup=done\nS_migrate=done\nS_start=running\n' "$ID" "$SHA" "$NEW" "$REPO:sha-$SHA" "$NEWDIGEST" "$(date +%s)" "$(date +%s)" > "$U/run/current"
rm -f "$S/started"
run SN_HELPER_TEST=1 "$SHELL_BIN" "$R/install.sh" updater recover > "$WORK/recover.txt" 2>&1 </dev/null
[ "$(field state)" = "rolled_back" ] && [ "$(field error)" = "interrupted" ] && check ok "итог: прерванное обновление откатано" || check no "итог: $(cat "$U/ipc/status/update.json" 2>/dev/null)"
grep -qx 'stealthnet|' "$S/dbs" && [ "$(wc -l < "$S/dbs" | tr -d ' ')" = "1" ] && check ok "базы возвращены по меткам, копия удалена" || check no "базы: $(tr '\n' ';' < "$S/dbs")"
[ -f "$S/started" ] && [ ! -e "$U/run/current" ] && check ok "прежняя версия запущена, журнал убран" || check no "не запущено"
printf 'begin\napp_stopped\nbackup_written\ndb_cloned\ndb_swapping\ndb_swapped\napp_starting\n' > "$U/run/journal"
printf 'U_ID=%s\nU_ORIGIN=panel\nU_TO=%s\nTO_IMAGE=%s\n' "$ID" "$NEW" "$NEWDIGEST" > "$U/run/current"
run SN_HELPER_TEST=1 "$SHELL_BIN" "$R/install.sh" updater recover > "$WORK/recover2.txt" 2>&1 </dev/null
[ "$(field state)" = "done" ] && check ok "если новая версия успела запуститься и отвечает — обновление засчитано" || check no "итог: $(cat "$U/ipc/status/update.json")"
run SN_HELPER_TEST=1 "$SHELL_BIN" "$R/install.sh" updater recover > "$WORK/recover3.txt" 2>&1 </dev/null
[ $? -eq 0 ] && check ok "без незаконченного обновления восстановление ничего не делает" || check no "recover без журнала"

echo "=== AP. выключение и включение обновления из админки"
helper_install
: > "$S/systemctl.log"
run SN_HELPER_TEST=1 "$SHELL_BIN" "$R/install.sh" helper off > "$WORK/off.txt" 2>&1 </dev/null
grep -qx 'disable --now stealthnet-rw-update.path' "$S/systemctl.log" && [ ! -e "$WORK/systemd/stealthnet-rw-update.path" ] && check ok "слежение выключено и убрано" || check no "systemctl: $(tr '\n' ';' < "$S/systemctl.log")"
grep -q '"panel_updates":false' "$U/ipc/status/helper.json" && grep -qx 'SN_UPDATER=off' "$R/deploy/.env" && check ok "приложение видит, что обновление выключено" || check no "helper.json: $(cat "$U/ipc/status/helper.json")"
run SN_HELPER_TEST=1 SN_OWNER_EMAIL=owner@example.com "$SHELL_BIN" "$R/install.sh" > "$WORK/rerun.txt" 2>&1 </dev/null
grep -q 'выключено. Включить:' "$WORK/rerun.txt" && [ ! -e "$WORK/systemd/stealthnet-rw-update.path" ] && check ok "повторная установка не включает его тайком" || check no "включилось само"
run SN_HELPER_TEST=1 "$SHELL_BIN" "$R/install.sh" helper on > "$WORK/on.txt" 2>&1 </dev/null
[ -e "$WORK/systemd/stealthnet-rw-update.path" ] && grep -q '"panel_updates":true' "$U/ipc/status/helper.json" && grep -qx 'SN_UPDATER=1' "$R/deploy/.env" && check ok "включается обратно командой" || check no "не включилось: $(tail -3 "$WORK/on.txt")"

echo "=== AQ. обновление командой идёт тем же путём"
helper_install
printf '%s|%s|%s\n' "$REPO:sha-$SHA" "$SHA" "2026-11-17T10:00:00Z" "$REPO:sha-$NEW" "$NEW" "2026-11-17T13:00:00Z" > "$S/labels"
printf '%s\n' "$NEW" > "$R/.git/HEAD"
: > "$S/calls.log"
: > "$S/migrations"
run SN_HELPER_TEST=1 FAKE_PULL_OK=":sha-$NEW" SN_OWNER_EMAIL=owner@example.com "$SHELL_BIN" "$R/install.sh" > "$WORK/cli.txt" 2>&1 </dev/null
code=$?
[ "$code" -eq 0 ] && check ok "обновление командой прошло" || check no "код выхода $code: $(tail -5 "$WORK/cli.txt")"
grep -q 'Резервная копия' "$WORK/cli.txt" && grep -q 'Обновление базы' "$WORK/cli.txt" && check ok "человек видит шаги" || check no "нет шагов"
grep -q "image=$REPO:sha-$NEW .*stealthnet_next" "$S/migrations" && grep -qx "SN_IMAGE=$REPO:sha-$NEW" "$R/deploy/.env" && check ok "миграции на копии, запущена новая сборка" || check no "миграции: $(cat "$S/migrations")"
grep -q '"origin":"command".*"state":"done"' "$U/ipc/status/history.jsonl" && check ok "в истории записано, что обновляли командой" || check no "история: $(tail -1 "$U/ipc/status/history.jsonl")"

echo "=== AR. более старая сборка клона не ставится поверх новой"
helper_install
printf '%s|%s|%s\n' "$REPO:sha-$SHA" "$SHA" "2026-11-17T10:00:00Z" "$NEWDIGEST" "$NEW" "2026-11-17T13:00:00Z" > "$S/labels"
sed "s|^SN_IMAGE=.*|SN_IMAGE=$NEWDIGEST|" "$R/deploy/.env" > "$WORK/env.tmp" && cat "$WORK/env.tmp" > "$R/deploy/.env"
printf '%s\n' "$NEWDIGEST" >> "$S/images"
: > "$S/calls.log"
run SN_HELPER_TEST=1 FAKE_PULL_OK=":sha-$SHA" SN_OWNER_EMAIL=owner@example.com "$SHELL_BIN" "$R/install.sh" > "$WORK/old.txt" 2>&1 </dev/null
grep -qx "SN_IMAGE=$NEWDIGEST" "$R/deploy/.env" && grep -q 'на сервере уже стоит сборка новее, оставляю её' "$WORK/old.txt" && check ok "новая сборка осталась" || check no "SN_IMAGE: $(grep '^SN_IMAGE' "$R/deploy/.env")"
! grep -q '^compose stop' "$S/calls.log" && check ok "панель не останавливалась" || check no "панель останавливалась"

echo "=== AS. состояние и сборка из исходников"
helper_install
request 9b6c5d0e-6a1f-4c1e-9d55-3f0c2a7b9e10 "$NEW"
updater FAKE_PULL_OK=":main" FAKE_DIGEST="$NEWDIGEST" FAKE_MIGRATE_EXIT=1
run SN_HELPER_TEST=1 "$SHELL_BIN" "$R/install.sh" status > "$WORK/status.txt" 2>&1 </dev/null
grep -q 'Панель:            работает' "$WORK/status.txt" && grep -q 'Обновление из админки: включено' "$WORK/status.txt" && grep -q 'не установлено (migrate_failed), прежняя версия работает' "$WORK/status.txt" && check ok "status рассказывает про панель и последнее обновление" || check no "status: $(cat "$WORK/status.txt")"
grep -q "Копии базы:        1" "$WORK/status.txt" && check ok "status показывает копии базы" || check no "нет копий в status"
grep -q "Версия:            0.1.59 от 17.11.2026 (0123456)" "$WORK/status.txt" && check ok "status называет версию по номеру и дате, хеш рядом" || check no "версия в status: $(grep 'Версия' "$WORK/status.txt")"
fresh
mkdir -p "$R/.git"
printf '%s\n' "$SHA" > "$R/.git/HEAD"
cp "$SRC/Cargo.toml" "$R/Cargo.toml"
run SN_BUILD=1 FAKE_GIT_COUNT=60 SN_OWNER_EMAIL=owner@example.com SN_DOMAIN=panel.test "$SHELL_BIN" "$R/install.sh" > "$WORK/out.txt" 2>&1 </dev/null
grep -q "^commit=$SHA date=20[0-9-]*T[0-9:]*Z version=0\.[0-9]*\.60$" "$S/builds" && check ok "при сборке из исходников в образ уходят коммит, дата и номер версии" || check no "builds: $(cat "$S/builds" 2>/dev/null)"
: > "$S/builds"
run SN_BUILD=1 FAKE_GIT_COUNT=60 FAKE_GIT_SHALLOW=true SN_OWNER_EMAIL=owner@example.com "$SHELL_BIN" "$R/install.sh" > "$WORK/out.txt" 2>&1 </dev/null
grep -q "^commit=$SHA date=20[0-9-]*T[0-9:]*Z version=$" "$S/builds" && check ok "у неполного клона номер не выдумывается" || check no "builds: $(cat "$S/builds" 2>/dev/null)"
fresh
mkdir -p "$R/.git"
printf '%s\n' "$SHA" > "$R/.git/HEAD"
run FAKE_GIT_STATUS=" M crates/x.rs" SN_OWNER_EMAIL=owner@example.com SN_DOMAIN=panel.test "$SHELL_BIN" "$R/install.sh" > "$WORK/out.txt" 2>&1 </dev/null
grep -q "^commit= date=" "$S/builds" && check ok "изменённые исходники не выдаются за опубликованный коммит" || check no "builds: $(cat "$S/builds" 2>/dev/null)"
echo "=== AT. запуск, перезапуск, остановка и журнал"
fresh
run SN_OWNER_EMAIL=owner@example.com SN_DOMAIN=panel.test "$SHELL_BIN" "$R/install.sh" > "$WORK/out.txt" 2>&1 </dev/null
grep -q "$WORK/bin/stealthnet start" "$WORK/out.txt" && check ok "в итоге установки есть команда запуска" || check no "в итоге нет команды запуска"
: > "$S/calls.log"
rm -f "$S/health_calls"
run FAKE_STARTING=2 "$SHELL_BIN" "$R/install.sh" start > "$WORK/start.txt" 2>&1 </dev/null
code=$?
[ "$code" -eq 0 ] && check ok "запуск прошёл" || check no "запуск: код $code, $(tail -2 "$WORK/start.txt")"
grep -q '^compose up -d --no-build --remove-orphans$' "$S/calls.log" && check ok "запуск поднимает контейнеры без сборки и скачивания" || check no "нет запуска контейнеров"
grep -q '^compose up -d --build' "$S/calls.log" && check no "запуск пересобирает систему" || check ok "запуск ничего не пересобирает"
grep -q '^pull ' "$S/calls.log" && check no "запуск скачивает сборку" || check ok "запуск ничего не скачивает"
grep -q 'система запущена и отвечает' "$WORK/start.txt" && check ok "после запуска проверено, что система отвечает" || check no "нет проверки после запуска"
grep -q 'Адрес панели: *https://panel.test' "$WORK/start.txt" && check ok "после запуска показан адрес" || check no "нет адреса после запуска"
: > "$S/calls.log"
run "$SHELL_BIN" "$R/install.sh" restart > "$WORK/restart.txt" 2>&1 </dev/null
code=$?
[ "$code" -eq 0 ] && grep -q '^compose restart$' "$S/calls.log" && check ok "перезапуск прошёл" || check no "перезапуск: код $code, $(tail -2 "$WORK/restart.txt")"
run FAKE_RESTART_EXIT=1 "$SHELL_BIN" "$R/install.sh" restart > "$WORK/restart2.txt" 2>&1 </dev/null
[ $? -eq 1 ] && grep -q 'перезапуск не прошёл' "$WORK/restart2.txt" && check ok "сбой перезапуска объяснён" || check no "сбой перезапуска"
: > "$S/calls.log"
run "$SHELL_BIN" "$R/install.sh" stop > "$WORK/stop.txt" 2>&1 </dev/null
code=$?
if [ "$code" -eq 1 ] && grep -q 'stop --yes' "$WORK/stop.txt" && ! grep -q '^compose stop$' "$S/calls.log"; then check ok "без терминала остановка требует --yes"; else check no "остановка без подтверждения: код $code"; fi
run "$SHELL_BIN" "$R/install.sh" stop --yes > "$WORK/stop2.txt" 2>&1 </dev/null
code=$?
if [ "$code" -eq 0 ] && grep -q '^compose stop$' "$S/calls.log" && grep -q 'Запустить снова:.*stealthnet start' "$WORK/stop2.txt"; then check ok "остановка с --yes прошла и подсказала запуск"; else check no "остановка с --yes: код $code"; fi
: > "$S/calls.log"
run "$SHELL_BIN" "$R/install.sh" logs > "$WORK/logs.txt" 2>&1 </dev/null
[ $? -eq 0 ] && grep -q '^compose logs --tail 200 app$' "$S/calls.log" && check ok "журнал показывается" || check no "журнал"
run "$SHELL_BIN" "$R/install.sh" logs -f > "$WORK/logs2.txt" 2>&1 </dev/null
[ $? -eq 0 ] && grep -q '^compose logs --tail 100 -f app$' "$S/calls.log" && check ok "журнал можно смотреть вживую" || check no "журнал -f"
run FAKE_INFO_EXIT=1 "$SHELL_BIN" "$R/install.sh" start > "$WORK/start2.txt" 2>&1 </dev/null
[ $? -eq 1 ] && grep -q 'Docker не отвечает' "$WORK/start2.txt" && grep -q 'sudo .*start' "$WORK/start2.txt" && check ok "без Docker запуск объясняет, что делать" || check no "запуск без Docker"
env FAKE_STATE="$S" SN_BIN_DIR="$WORK/bin" PATH="$HERE/bin:$WORK/bin:$PATH" stealthnet help > "$WORK/help.txt" 2>&1 </dev/null
grep -q '^  stealthnet restart$' "$WORK/help.txt" && grep -q '^  stealthnet logs$' "$WORK/help.txt" && check ok "новые команды есть в справке" || check no "в справке нет новых команд"

echo "=== AU. снятие блокировки входа владельца"
fresh
run "$SHELL_BIN" "$R/install.sh" unlock-login > "$WORK/out.txt" 2>&1 </dev/null
[ $? -eq 1 ] && grep -q 'ещё не установлена' "$WORK/out.txt" && check ok "до установки снимать нечего" || check no "снятие блокировки до установки"
run SN_OWNER_EMAIL=owner@example.com SN_DOMAIN=panel.test "$SHELL_BIN" "$R/install.sh" > "$WORK/out.txt" 2>&1 </dev/null
: > "$S/calls.log"
run "$SHELL_BIN" "$R/install.sh" unlock-login > "$WORK/unlock.txt" 2>&1 </dev/null
code=$?
if [ "$code" -eq 0 ] && grep -q '^compose exec -T app stealthnet unlock-login --email owner@example.com$' "$S/calls.log"; then check ok "блокировка снята у владельца"; else check no "снятие блокировки: код $code, $(tail -2 "$WORK/unlock.txt")"; fi
grep -q '^  Вход для owner@example.com с новых адресов был закрыт' "$WORK/unlock.txt" && check ok "ответ системы показан" || check no "ответ системы не показан"
: > "$S/calls.log"
run "$SHELL_BIN" "$R/install.sh" unlock-login --ip 203.0.113.7 --ip=2001:db8::7 > "$WORK/unlock2.txt" 2>&1 </dev/null
code=$?
if [ "$code" -eq 0 ] && grep -q '^compose exec -T app stealthnet unlock-login --email owner@example.com --ip 203.0.113.7 --ip 2001:db8::7$' "$S/calls.log"; then check ok "адреса переданы системе"; else check no "адреса: код $code, $(grep unlock-login "$S/calls.log")"; fi
: > "$S/calls.log"
run "$SHELL_BIN" "$R/install.sh" unlock-login --ip 'not an ip' > "$WORK/unlock3.txt" 2>&1 </dev/null
code=$?
if [ "$code" -eq 1 ] && grep -q 'не похоже на IP-адрес' "$WORK/unlock3.txt" && ! grep -q 'unlock-login' "$S/calls.log"; then check ok "неверный адрес отклонён до обращения к системе"; else check no "неверный адрес: код $code"; fi
run "$SHELL_BIN" "$R/install.sh" unlock-login --ip > "$WORK/unlock4.txt" 2>&1 </dev/null
[ $? -eq 1 ] && grep -q 'после --ip нужен адрес' "$WORK/unlock4.txt" && check ok "--ip без адреса объяснён" || check no "--ip без адреса"
run "$SHELL_BIN" "$R/install.sh" unlock-login --force > "$WORK/unlock5.txt" 2>&1 </dev/null
[ $? -eq 1 ] && grep -q 'не понял «--force»' "$WORK/unlock5.txt" && check ok "лишний аргумент объяснён" || check no "лишний аргумент"
run FAKE_UNLOCK_EXIT=1 "$SHELL_BIN" "$R/install.sh" unlock-login > "$WORK/unlock6.txt" 2>&1 </dev/null
[ $? -eq 1 ] && grep -q 'не удалось снять блокировку' "$WORK/unlock6.txt" && check ok "сбой команды объяснён" || check no "сбой команды"
env FAKE_STATE="$S" SN_BIN_DIR="$WORK/bin" PATH="$HERE/bin:$WORK/bin:$PATH" stealthnet help > "$WORK/help.txt" 2>&1 </dev/null
grep -q '^  stealthnet unlock-login \[--ip адрес\]$' "$WORK/help.txt" && check ok "команда есть в справке" || check no "в справке нет команды"

echo "=== AV. резервная копия и восстановление командой"
fresh
run "$SHELL_BIN" "$R/install.sh" backup > "$WORK/out.txt" 2>&1 </dev/null
[ $? -eq 1 ] && grep -q 'ещё не установлена' "$WORK/out.txt" && check ok "до установки копировать нечего" || check no "копия до установки"
run SN_OWNER_EMAIL=owner@example.com SN_DOMAIN=panel.test "$SHELL_BIN" "$R/install.sh" > "$WORK/out.txt" 2>&1 </dev/null
[ -d "$R/deploy/backups/panel" ] && [ "$(ls -ld "$R/deploy/backups/panel" | cut -c1-10)" = "drwx------" ] && check ok "папка копий панели создана только для владельца" || check no "папка копий: $(ls -ld "$R/deploy/backups/panel" 2>&1)"
grep -q "^SN_BACKUP_HOST_DIR=$R/deploy/backups/panel\$" "$R/deploy/.env" && check ok "путь к папке копий записан для админки" || check no "путь к копиям: $(grep SN_BACKUP_HOST_DIR "$R/deploy/.env")"
: > "$S/calls.log"
run "$SHELL_BIN" "$R/install.sh" backup > "$WORK/backup.txt" 2>&1 </dev/null
code=$?
if [ "$code" -eq 0 ] && grep -q '^compose exec -T app stealthnet backup$' "$S/calls.log" && grep -q 'Копия сохранена' "$WORK/backup.txt"; then check ok "копия сделана командой"; else check no "копия командой: код $code, $(tail -2 "$WORK/backup.txt")"; fi
run FAKE_BACKUP_EXIT=1 "$SHELL_BIN" "$R/install.sh" backup > "$WORK/backup2.txt" 2>&1 </dev/null
[ $? -eq 1 ] && grep -q 'копия не сделана' "$WORK/backup2.txt" && check ok "сбой копии объяснён" || check no "сбой копии"
run "$SHELL_BIN" "$R/install.sh" restore > "$WORK/restore0.txt" 2>&1 </dev/null
[ $? -eq 1 ] && grep -q 'укажите файл копии' "$WORK/restore0.txt" && check ok "без файла восстановление объясняет, что нужно" || check no "restore без файла"
run "$SHELL_BIN" "$R/install.sh" restore "$WORK/nope.tar.gz" > "$WORK/restore1.txt" 2>&1 </dev/null
[ $? -eq 1 ] && grep -q 'файл не найден' "$WORK/restore1.txt" && check ok "несуществующий файл объяснён" || check no "restore без файла на диске"
printf 'SNBACKUP-fake' > "$WORK/copy.tar.gz.enc"
: > "$S/calls.log"
run SN_BACKUP_PASSWORD='пароль копии' "$SHELL_BIN" "$R/install.sh" restore --yes "$WORK/copy.tar.gz.enc" > "$WORK/restore2.txt" 2>&1 </dev/null
code=$?
if [ "$code" -eq 0 ] && [ -f "$S/restored" ] && grep -q '^compose stop -t 60 app$' "$S/calls.log" && grep -q '^compose up -d --no-build' "$S/calls.log"; then check ok "восстановление остановило панель, заменило базу и запустило снова"; else check no "восстановление: код $code, $(tail -3 "$WORK/restore2.txt")"; fi
[ "$(cat "$S/restore_password.txt" 2>/dev/null)" = "пароль копии" ] && check ok "пароль копии передан через stdin" || check no "пароль копии"
grep -q 'пароль копии' "$S/calls.log" && check no "пароль копии попал в аргументы" || check ok "пароль копии не попал в аргументы"
grep -q 'restore --password-stdin --check /var/lib/stealthnet/backups/.restore-' "$S/calls.log" && check ok "файл со стороны передан через папку копий" || check no "путь файла: $(grep restore "$S/calls.log" | head -1)"
ls "$R/deploy/backups/panel"/.restore-* > /dev/null 2>&1 && check no "временная копия файла не удалена" || check ok "временная копия файла удалена"
rm -f "$S/restored"
run FAKE_RESTORE_CHECK_EXIT=1 SN_BACKUP_PASSWORD=p "$SHELL_BIN" "$R/install.sh" restore --yes "$WORK/copy.tar.gz.enc" > "$WORK/restore3.txt" 2>&1 </dev/null
code=$?
[ "$code" -eq 1 ] && grep -q 'копия не прошла проверку, база не менялась' "$WORK/restore3.txt" && [ ! -f "$S/restored" ] && check ok "битая копия остановлена до замены базы" || check no "битая копия: код $code"
printf 'a' > "$WORK/big.tar.gz.part001"
printf 'b' > "$WORK/big.tar.gz.part002"
: > "$S/calls.log"
run "$SHELL_BIN" "$R/install.sh" restore --yes "$WORK/big.tar.gz.part002" > "$WORK/restore4.txt" 2>&1 </dev/null
code=$?
[ "$code" -eq 0 ] && grep -q 'собрано: 2 части' "$WORK/restore4.txt" && check ok "части из Telegram собраны в один файл" || check no "части: код $code, $(tail -2 "$WORK/restore4.txt")"
run FAKE_RESTORE_EXIT=1 SN_BACKUP_PASSWORD=p "$SHELL_BIN" "$R/install.sh" restore --yes "$WORK/copy.tar.gz.enc" > "$WORK/restore5.txt" 2>&1 </dev/null
code=$?
[ "$code" -eq 1 ] && grep -q 'восстановление не прошло, база осталась прежней' "$WORK/restore5.txt" && [ -f "$S/started" ] && check ok "сбой восстановления объяснён, панель запущена снова" || check no "сбой восстановления: код $code"
run "$SHELL_BIN" "$R/install.sh" restore "$WORK/copy.tar.gz.enc" > "$WORK/restore6.txt" 2>&1 </dev/null
[ $? -eq 1 ] && grep -q 'некому ввести пароль копии' "$WORK/restore6.txt" && check ok "без терминала пароль копии просит через SN_BACKUP_PASSWORD" || check no "пароль без терминала: $(tail -2 "$WORK/restore6.txt")"
env FAKE_STATE="$S" SN_BIN_DIR="$WORK/bin" PATH="$HERE/bin:$WORK/bin:$PATH" stealthnet help > "$WORK/help.txt" 2>&1 </dev/null
grep -q '^  stealthnet backup$' "$WORK/help.txt" && grep -q '^  stealthnet restore файл$' "$WORK/help.txt" && check ok "команды копий есть в справке" || check no "в справке нет команд копий"

echo
if [ "$fail" -eq 0 ]; then echo "ИТОГ ($SHELL_BIN): все проверки прошли"; else echo "ИТОГ ($SHELL_BIN): есть ошибки"; fi
exit "$fail"
