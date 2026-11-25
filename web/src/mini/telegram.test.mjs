import { registerHooks } from 'node:module';
import { test } from 'node:test';
import assert from 'node:assert/strict';

registerHooks({
  resolve(specifier, context, next) {
    return next(specifier.startsWith('.') && !/\.[a-z]+(\?.*)?$/.test(specifier) ? `${specifier}.ts` : specifier, context);
  },
});

const DATA = 'query_id=AA&user=%7B%22id%22%3A7%2C%22first_name%22%3A%22%D0%90%D0%BD%D0%BD%D0%B0%22%2C%22language_code%22%3A%22ru%22%7D&hash=ff';
const HASH = `#tgWebAppData=${encodeURIComponent(DATA)}&tgWebAppVersion=8.0&tgWebAppPlatform=android`;

let round = 0;

async function open({ hash = '', saved = null, proxy = true, storage = true } = {}) {
  const scripts = [];
  const sent = [];
  const kept = new Map(saved ? [['__telegram__initParams', JSON.stringify(saved)]] : []);
  const timers = [];
  const page = {
    location: { hash },
    setTimeout: (cb, ms) => timers.push({ cb, ms }),
    ...(proxy ? { TelegramWebviewProxy: { postEvent: (type, data) => sent.push([type, JSON.parse(data)]) } } : {}),
  };
  Object.defineProperty(page, 'sessionStorage', {
    get() {
      if (!storage) {
        throw new Error('SecurityError');
      }
      return { getItem: (key) => kept.get(key) ?? null, setItem: (key, value) => kept.set(key, value) };
    },
  });
  globalThis.window = page;
  globalThis.document = { createElement: () => ({}), head: { appendChild: (node) => scripts.push(node) } };
  round += 1;
  const telegram = await import(`./telegram.ts?round=${round}`);
  const sdk = { initData: DATA, name: 'sdk', ready() {} };
  return {
    telegram,
    page,
    scripts,
    sent,
    kept,
    sdk,
    arrive(index = 0) {
      page.Telegram = { ...page.Telegram, WebApp: sdk };
      scripts[index].onload();
    },
    fail(index = 0) {
      scripts[index].onerror();
    },
    wait() {
      for (const { cb } of timers.splice(0)) {
        cb();
      }
    },
  };
}

test('в обычном браузере скрипт Telegram не загружается и ничего не меняется', async () => {
  const { telegram, scripts, sent, kept } = await open();
  assert.equal(await telegram.loadTelegram(), null);
  assert.equal(telegram.webApp(), null);
  assert.equal(telegram.initData(), '');
  assert.equal(telegram.launchData(), '');
  assert.equal(scripts.length, 0);
  assert.equal(sent.length, 0);
  assert.equal(kept.size, 0);
});

test('страница без хранилища в обычном браузере открывается', async () => {
  const { telegram, scripts } = await open({ storage: false });
  assert.equal(await telegram.loadTelegram(), null);
  assert.equal(scripts.length, 0);
});

test('в Telegram используется его скрипт, запасной мост не включается', async () => {
  const opened = await open({ hash: HASH });
  const { telegram, scripts, sent, kept, sdk } = opened;
  const loading = telegram.loadTelegram();
  assert.equal(scripts[0].src, 'https://telegram.org/js/telegram-web-app.js');
  opened.arrive();
  assert.equal(await loading, sdk);
  assert.equal(telegram.webApp(), sdk);
  assert.equal(telegram.initData(), DATA);
  assert.equal(telegram.telegramName(), 'Анна');
  opened.wait();
  assert.equal(sent.length, 0);
  assert.equal(JSON.parse(kept.get('__telegram__initParams')).tgWebAppData, DATA);
  assert.equal(await telegram.loadTelegram(), sdk);
  assert.equal(scripts.length, 1);
});

test('скрипт Telegram недоступен: приложение говорит с клиентом напрямую', async () => {
  const opened = await open({ hash: HASH });
  const { telegram, sent } = opened;
  const loading = telegram.loadTelegram();
  opened.fail();
  const app = await loading;
  assert.equal(app, telegram.webApp());
  assert.equal(app.platform, 'android');
  assert.equal(telegram.initData(), DATA);
  app.requestFullscreen();
  assert.deepEqual(sent.map(([name]) => name), ['web_app_request_safe_area', 'web_app_request_content_safe_area', 'web_app_request_fullscreen']);
  telegram.openExternal('https://pay.example/x');
  telegram.openExternal('https://t.me/demo_bot');
  assert.deepEqual(sent.slice(-2), [
    ['web_app_open_link', { url: 'https://pay.example/x' }],
    ['web_app_open_tg_link', { path_full: '/demo_bot' }],
  ]);
});

test('скрипт Telegram долго не отвечает: сначала запасной мост, потом настоящий скрипт', async () => {
  const opened = await open({ hash: HASH });
  const { telegram, sdk } = opened;
  const late = [];
  const loading = telegram.loadTelegram((app) => late.push(app));
  opened.wait();
  const first = await loading;
  assert.notEqual(first, sdk);
  assert.equal(telegram.webApp(), first);
  assert.deepEqual(late, []);
  opened.arrive();
  assert.equal(telegram.webApp(), sdk);
  assert.deepEqual(late, []);
  opened.wait();
  assert.deepEqual(late, [sdk]);
});

test('скрипт не загрузился и после ожидания: второй раз ничего не происходит', async () => {
  const opened = await open({ hash: HASH });
  const late = [];
  const loading = opened.telegram.loadTelegram((app) => late.push(app));
  opened.wait();
  await loading;
  opened.fail();
  opened.wait();
  assert.deepEqual(late, []);
});

test('в Telegram Web нет родного моста: без скрипта приложение работает как раньше', async () => {
  const opened = await open({ hash: HASH, proxy: false });
  const loading = opened.telegram.loadTelegram();
  opened.fail();
  assert.equal(await loading, null);
  assert.equal(opened.telegram.webApp(), null);
  assert.equal(opened.telegram.initData(), DATA);
});

test('после перезагрузки страницы адрес уже без данных, но Telegram узнаётся по сохранённым', async () => {
  const opened = await open({ saved: { tgWebAppData: DATA, tgWebAppVersion: '8.0', tgWebAppPlatform: 'ios' } });
  const { telegram, scripts, sdk } = opened;
  assert.equal(telegram.launchData(), '');
  const loading = telegram.loadTelegram();
  assert.equal(scripts.length, 1);
  opened.arrive();
  assert.equal(await loading, sdk);
});

test('без доступа к хранилищу первый вход в Telegram работает', async () => {
  const opened = await open({ hash: HASH, storage: false });
  const loading = opened.telegram.loadTelegram();
  opened.arrive();
  assert.equal(await loading, opened.sdk);
});
