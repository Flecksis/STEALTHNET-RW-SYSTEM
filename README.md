# STEALTHNET на Rust

Панель продажи VPN поверх Remnawave 3.4.11: Telegram-бот, веб-кабинет, мини-приложение и админка. Один бинарник `stealthnet`, внутри — ядро и включаемые модули.

Сейчас в репозитории основа кода: крейты, контракты между ними, схема базы, приём вебхуков, вход администратора, расчёт цены. Сценарии, которые ещё не написаны, отвечают понятной ошибкой `not_implemented` (HTTP 501). Что и в каком порядке дописывать — в `docs/ARCHITECTURE.md`.

## Сборка

Нужны Rust 1.94 (версия закреплена в `rust-toolchain.toml`) и PostgreSQL 16.

```sh
cargo build --release -p sn-app
./target/release/stealthnet help
```

## Запуск без Docker

```sh
export DATABASE_URL=postgres://stealthnet:пароль@127.0.0.1:5432/stealthnet
export SN_SECRET_KEY=$(./target/release/stealthnet gen-secret-key)
./target/release/stealthnet migrate
SN_ADMIN_PASSWORD='пароль владельца' ./target/release/stealthnet create-admin --email owner@example.com
./target/release/stealthnet serve
```

`SN_SECRET_KEY` сохраните: им зашифрованы ключи платёжных систем и токены, которые вводятся в админке. С другим ключом они не расшифруются.

Повторный `create-admin` с той же почтой меняет пароль и закрывает все сессии владельца.

## Docker

```sh
cd deploy
cp .env.example .env
docker compose up -d --build
docker compose exec -e SN_ADMIN_PASSWORD='пароль' app stealthnet create-admin --email owner@example.com
```

`POSTGRES_PASSWORD` лучше взять из `openssl rand -hex 24`: он попадает в строку подключения, и спецсимволы её ломают.

Если Remnawave стоит на этом же сервере в своём compose, подключите приложение к её сети и укажите `REMNAWAVE_URL=http://remnawave:3000`:

```sh
docker compose -f docker-compose.yml -f compose.remnawave.yml up -d --build
```

## Переменные окружения

| Переменная | По умолчанию | Что это |
|---|---|---|
| `DATABASE_URL` | — | Строка подключения к PostgreSQL 16, обязательна |
| `SN_BIND` | `0.0.0.0:8080` | Адрес HTTP-сервера |
| `SN_PUBLIC_URL` | — | Внешний адрес панели, `https://…`; из него строятся ссылки для вебхуков и оплаты |
| `SN_SECRET_KEY` | — | 32 байта в base64 для шифрования секретов (`stealthnet gen-secret-key`) |
| `SN_DB_MAX_CONNECTIONS` | `16` | Размер пула соединений |
| `SN_MIGRATE_ON_START` | `true` | Применять миграции при `serve` |
| `SN_RUN_WORKER` | `true` | Запускать фоновые задачи в этом процессе |
| `SN_RUN_BOT` | `true` | Запускать бота в этом процессе |
| `SN_TRUSTED_PROXIES` | loopback и шлюз Docker | IP обратных прокси через запятую; только им верим в `X-Forwarded-For`. Явный список заменяет умолчания. Прокси обязан дописывать `X-Forwarded-For` (nginx: `proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;`). Если порт приложения опубликован наружу без прокси, задайте список явно |
| `SN_CORS_ORIGINS` | — | Разрешённые источники для браузера через запятую |
| `SN_LOG` | `info` | Фильтр логов в формате `tracing` |
| `SN_LOG_FORMAT` | `text` | `text` или `json` |
| `REMNAWAVE_URL` | — | Адрес панели Remnawave, пока он не задан в админке |
| `REMNAWAVE_TOKEN` | — | API-токен Remnawave, пока он не задан в админке |
| `REMNAWAVE_COOKIE` | — | Кука защиты панели в формате `имя=значение` или `имя:значение` |
| `REMNAWAVE_WEBHOOK_SECRET` | — | Совпадает с `WEBHOOK_SECRET_HEADER` в `.env` Remnawave |
| `TELEGRAM_BOT_TOKEN` | — | Токен бота, пока он не задан в админке |
| `SN_ADMIN_PASSWORD` | — | Только для `create-admin`, иначе пароль читается из stdin |

Всё, что меняет оператор (валюта, платёжные системы, модули, тарифы), хранится в базе. Переменные `REMNAWAVE_*` и `TELEGRAM_BOT_TOKEN` нужны для первого запуска: значения из админки важнее.

Вебхуки Remnawave принимаются на `POST {SN_PUBLIC_URL}/api/remnawave/webhook`, платёжных систем — на `POST {SN_PUBLIC_URL}/api/webhooks/pay/{провайдер}`.

## Проверки

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
SN_TEST_DATABASE_URL=postgresql://postgres@127.0.0.1:5432/sn_test cargo test --workspace
```

Тесты с базой запускаются, только если задана `SN_TEST_DATABASE_URL`. Имя базы обязано начинаться с `sn_test`: каждый тест создаёт свою схему, накатывает миграции и удаляет схему после себя. В CI это делает сервис `postgres:16`.

## Крейты

| Крейт | Что внутри |
|---|---|
| `sn-domain` | Типы и правила без ввода-вывода: деньги, тарифы, цена, план выдачи, статусы, события, каталог модулей |
| `sn-remnawave` | Клиент API Remnawave 3.4.11, проверка вебхуков, тестовая подделка панели |
| `sn-store` | PostgreSQL: пул, миграции, репозитории, шифрование секретов |
| `sn-payments` | Типаж платёжной системы, реестр по валюте установки, провайдеры |
| `sn-modules` | Типаж модуля, реестр, включение и выключение без перезапуска |
| `sn-services` | Сценарии ядра: вход, покупка, приём оплаты, выдача, синхронизация с Remnawave |
| `sn-server` | HTTP на axum: админка, кабинет и мини-приложение, вебхуки |
| `sn-worker` | Очередь задач и периодические работы |
| `sn-bot` | Telegram-бот |
| `sn-app` | Бинарник `stealthnet`: конфиг, логи, команды |
