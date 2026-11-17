![STEALTHNET RW SYSTEM](docs/media/cover.svg)

<p align="center">
  <a href="#установка"><img src="docs/media/navigation/button-install.svg" width="174" height="58" alt="Установить" /></a>
  <a href="docs/ARCHITECTURE.md"><img src="docs/media/navigation/button-docs.svg" width="190" height="58" alt="Документация" /></a>
  <a href="https://t.me/stealthnet_admin_panel"><img src="docs/media/navigation/button-community.svg" width="174" height="58" alt="Наша группа в Telegram" /></a>
  <a href="https://github.com/STEALTHNET-APP/STEALTHNET-RW-SYSTEM/releases"><img src="docs/media/navigation/button-releases.svg" width="190" height="58" alt="Релизы" /></a>
</p>

**STEALTHNET RW SYSTEM** — система продажи VPN-подписок поверх Remnawave: Telegram-бот, кабинет клиента, мини-приложение и админка. Устанавливается на ваш сервер; работает как ядро с модулями, которые включаются в настройках.

**v0.1.0 · в разработке: готова серверная часть — покупка, платёжные системы, выдача подписок в Remnawave и Telegram-бот.**

<p align="center">
  <a href="https://github.com/STEALTHNET-APP/STEALTHNET-RW-SYSTEM/actions/workflows/ci.yml"><img src="https://github.com/STEALTHNET-APP/STEALTHNET-RW-SYSTEM/actions/workflows/ci.yml/badge.svg" alt="Проверки сборки и тесты" /></a>
  <a href="LICENSE"><img src="https://img.shields.io/badge/license-AGPL--3.0-72b5ff?style=flat-square" alt="Лицензия AGPL-3.0" /></a>
  <img src="https://img.shields.io/badge/backend-Rust-e7a36d?style=flat-square" alt="Rust" />
  <img src="https://img.shields.io/badge/database-PostgreSQL-72b5ff?style=flat-square" alt="PostgreSQL" />
</p>

## От оплаты до подписки

| Продажа и оплата | Подписки Remnawave | Управление |
|---|---|---|
| Тарифы и категории оператор создаёт сам | Выдача и продление сразу после оплаты | Роли администраторов и вход со вторым фактором |
| Восемь способов оплаты, баланс и Telegram Stars | Лимиты трафика и устройств, сквады, ноды | Клиенты: поиск, история, обращения в поддержку |
| Пробный период, автопродление и возвраты | Сверка с панелью и события по вебхукам | Модули: промокоды, рефералы, UTM-метки, доп. опции |

## Состояние

Проект в разработке, выпусков пока нет.

| Готово | В работе |
|---|---|
| Серверная часть: API админки, кабинета и мини-приложения | Интерфейс админки, кабинета и мини-приложения |
| Telegram-бот: покупка, оплата, вход на сайт, уведомления | Установщик одной командой и готовые сборки |
| Оплата: Crypto Pay, Telegram Stars, Heleket, Lava.top, ЮKassa, Platega, ЮMoney, CloudPayments | Обновление из админки |
| Выдача, продление, автопродление и возвраты через Remnawave 3.4 | Перенос данных из [remnawave-STEALTHNET-Bot](https://github.com/STEALTHNET-APP/remnawave-STEALTHNET-Bot) |

## Установка

Установщик одной командой выйдет вместе с первым выпуском. Пока систему можно запустить из исходников в Docker: нужны Docker с Compose и доступ к этому репозиторию.

```sh
git clone https://github.com/STEALTHNET-APP/STEALTHNET-RW-SYSTEM.git
cd STEALTHNET-RW-SYSTEM/deploy
cp .env.example .env
docker compose up -d --build
docker compose exec -e SN_ADMIN_PASSWORD='пароль' app stealthnet create-admin --email owner@example.com
```

Перед запуском заполните `.env`:

- `POSTGRES_PASSWORD` — возьмите из `openssl rand -hex 24`: пароль попадает в строку подключения, и спецсимволы её ломают;
- `SN_SECRET_KEY` — возьмите из `openssl rand -base64 32` и сохраните: им зашифрованы ключи платёжных систем и токены, которые вводятся в админке, с другим ключом они не расшифруются;
- `SN_PUBLIC_URL` — внешний адрес панели, начинается с `https://`.

Если Remnawave стоит на этом же сервере в своём compose, подключите приложение к её сети и укажите `REMNAWAVE_URL=http://remnawave:3000`:

```sh
docker compose -f docker-compose.yml -f compose.remnawave.yml up -d --build
```

Повторный `create-admin` с той же почтой меняет пароль и закрывает все сессии владельца.

## Запуск без Docker

Нужны Rust 1.94 (версия закреплена в `rust-toolchain.toml`) и PostgreSQL 16.

```sh
cargo build --release -p sn-app
export DATABASE_URL=postgres://stealthnet:пароль@127.0.0.1:5432/stealthnet
export SN_SECRET_KEY=$(./target/release/stealthnet gen-secret-key)
./target/release/stealthnet migrate
SN_ADMIN_PASSWORD='пароль владельца' ./target/release/stealthnet create-admin --email owner@example.com
./target/release/stealthnet serve
```

Список команд — `./target/release/stealthnet help`.

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

## Разработка

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
SN_TEST_DATABASE_URL=postgresql://postgres@127.0.0.1:5432/sn_test cargo test --workspace
```

Тесты с базой запускаются, только если задана `SN_TEST_DATABASE_URL`. Имя базы обязано начинаться с `sn_test`: каждый тест создаёт свою схему, накатывает миграции и удаляет схему после себя. В CI это делает сервис `postgres:16`.

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

## Документация и сообщество

| Раздел | Что внутри |
|---|---|
| [Архитектура](docs/ARCHITECTURE.md) | Ядро и модули, поток покупки, платежи, выдача, очередь задач, безопасность |
| [Релизы](https://github.com/STEALTHNET-APP/STEALTHNET-RW-SYSTEM/releases) | Версии и изменения |

<p align="center"><a href="https://t.me/stealthnet_admin_panel"><img src="docs/media/navigation/button-community.svg" width="174" height="58" alt="Присоединиться к группе STEALTHNET в Telegram" /></a></p>

Часть экосистемы [STEALTHNET](https://github.com/STEALTHNET-APP). Лицензия [AGPL-3.0](LICENSE).
