import { registerHooks } from 'node:module';
import { readFileSync } from 'node:fs';
import { test } from 'node:test';
import assert from 'node:assert/strict';

registerHooks({
  resolve(specifier, context, next) {
    if (!specifier.startsWith('.') || specifier.includes('?')) {
      return next(specifier, context);
    }
    const file = /\.[a-z]+$/.test(specifier) ? specifier : `${specifier}.ts`;
    return next(file + (/\?jump=\d+$/.exec(context.parentURL ?? '')?.[0] ?? ''), context);
  },
});

const DATA = 'query_id=AA&user=%7B%22id%22%3A7%7D&hash=ff';
const LAUNCH = { tgWebAppData: DATA, tgWebAppVersion: '8.0', tgWebAppPlatform: 'ios' };
const TOKEN = 'ab'.repeat(32);
const HAPP = 'happ://add/https://sub.example.com/s3cret';
const SPOT = { platform: 'ios', app: 'happ', step: 1 };

function sdk(opened) {
  return {
    initData: DATA,
    ready() {},
    openTelegramLink: (url) => opened.push(['t.me', url]),
    openLink(url) {
      const { protocol, href } = new URL(url, 'https://panel.example/app');
      if (protocol !== 'http:' && protocol !== 'https:') {
        throw Error('WebAppTgUrlInvalid');
      }
      opened.push(['browser', href]);
    },
  };
}

let round = 0;

async function start({ telegram = 'sdk', answer = { status: 200, body: { token: TOKEN, expires_at: '2026-11-25T10:00:00Z' } } } = {}) {
  const opened = [];
  const sent = [];
  const asked = [];
  const timers = [];
  const page = {
    location: { hash: telegram === 'none' ? '' : `#tgWebAppData=${encodeURIComponent(DATA)}&tgWebAppVersion=8.0&tgWebAppPlatform=ios`, origin: 'https://panel.example', href: 'https://panel.example/app/connect' },
    sessionStorage: { getItem: () => (telegram === 'none' ? null : JSON.stringify(LAUNCH)), setItem: () => {} },
    setTimeout: (run, ms) => timers.push({ run, ms }),
    clearTimeout: () => timers.splice(0),
    ...(telegram === 'bridge' ? { TelegramWebviewProxy: { postEvent: (type, data) => sent.push([type, JSON.parse(data)]) } } : {}),
    ...(telegram === 'sdk' ? { Telegram: { WebApp: sdk(opened) } } : {}),
  };
  globalThis.window = page;
  globalThis.document = { createElement: () => ({}), head: { appendChild: (node) => node.onerror() } };
  globalThis.fetch = async (path, options) => {
    asked.push([path, options.method, JSON.parse(options.body)]);
    if (answer === 'offline') {
      throw new TypeError('Failed to fetch');
    }
    if (answer === 'slow') {
      return new Promise((resolve, reject) => {
        options.signal.addEventListener('abort', () => reject(new DOMException('aborted', 'AbortError')));
        for (const { run } of timers.splice(0)) {
          run();
        }
      });
    }
    return { ok: answer.status < 400, status: answer.status, text: async () => JSON.stringify(answer.body) };
  };
  round += 1;
  const tg = await import(`./telegram.ts?jump=${round}`);
  const { jump, needsJump, JUMP_WAIT_MS } = await import(`./jump.ts?jump=${round}`);
  if (telegram === 'bridge') {
    await tg.loadTelegram();
    sent.length = 0;
  }
  return { tg, jump, needsJump, JUMP_WAIT_MS, page, opened, sent, asked };
}

test('в Telegram ссылка приложения открывается через страницу панели в браузере', async () => {
  const { jump, needsJump, opened, asked, tg } = await start();
  assert.throws(() => tg.openExternal(HAPP), /WebAppTgUrlInvalid/);
  assert.equal(needsJump(HAPP), true);
  await jump(42, SPOT);
  assert.deepEqual(asked, [['/api/client/subscriptions/42/app-link', 'POST', SPOT]]);
  assert.deepEqual(opened, [['browser', `https://panel.example/open-app#${TOKEN}`]]);
  assert.ok(!JSON.stringify([asked, opened]).includes('s3cret'));
});

test('кнопка дополнительного шага передаёт свой номер', async () => {
  const { jump, asked } = await start();
  await jump(42, { platform: 'android', app: 'v2raytun', step: 3, button: 1 });
  assert.deepEqual(asked[0][2], { platform: 'android', app: 'v2raytun', step: 3, button: 1 });
});

test('без скрипта Telegram страница открывается через родной мост', async () => {
  const { jump, needsJump, sent } = await start({ telegram: 'bridge' });
  assert.equal(needsJump(HAPP), true);
  await jump(42, SPOT);
  assert.deepEqual(sent, [['web_app_open_link', { url: `https://panel.example/open-app#${TOKEN}` }]]);
});

test('вне Telegram приложение открывается схемой, как раньше, без запроса к серверу', async () => {
  const { needsJump, tg, page, asked } = await start({ telegram: 'none' });
  for (const link of [HAPP, 'hiddify://import/https://sub.example.com/s3cret']) {
    assert.equal(needsJump(link), false);
    tg.openExternal(link);
    assert.equal(page.location.href, link);
  }
  assert.deepEqual(asked, []);
});

test('обычные ссылки в Telegram открываются сразу, запрещённые схемы на страницу не уходят', async () => {
  const { needsJump, tg, opened, asked } = await start();
  for (const link of ['https://open.example/add?sub=https://sub.example.com/s3cret', 'https://t.me/demo_bot', 'http://open.example']) {
    assert.equal(needsJump(link), false, link);
  }
  tg.openExternal('https://open.example/add');
  tg.openExternal('https://t.me/demo_bot');
  assert.deepEqual(opened, [
    ['browser', 'https://open.example/add'],
    ['t.me', 'https://t.me/demo_bot'],
  ]);
  for (const link of ['javascript:alert(1)', 'data:text/html,x', 'file:///etc/passwd', 'mailto:a@example.com', '/cabinet', '']) {
    assert.equal(needsJump(link), false, link);
  }
  assert.deepEqual(asked, []);
});

test('отказ сервера виден клиенту, браузер не открывается', async () => {
  for (const [status, code, message] of [
    [422, 'app_link.no_app', 'Эту кнопку убрали. Вернитесь в Telegram и откройте «Подключение» заново'],
    [429, 'auth.rate_limited', 'Слишком много попыток. Попробуйте снова через 5 мин.'],
    [404, 'not_found', 'не найдено: подписка'],
  ]) {
    const { jump, opened } = await start({ answer: { status, body: { error: { code, message } } } });
    await assert.rejects(jump(42, SPOT), (err) => err.status === status && err.code === code && err.message === message);
    assert.deepEqual(opened, []);
  }
});

test('без интернета и при долгом ответе клиент видит понятный текст', async () => {
  const offline = await start({ answer: 'offline' });
  await assert.rejects(offline.jump(42, SPOT), /Нет связи с сервером/);
  assert.deepEqual(offline.opened, []);
  const slow = await start({ answer: 'slow' });
  assert.ok(slow.JUMP_WAIT_MS < 10_000);
  await assert.rejects(slow.jump(42, SPOT), /Сервер долго не отвечает/);
  assert.deepEqual(slow.opened, []);
});

test('если Telegram не открыл браузер, ошибка не остаётся в консоли', async () => {
  const { jump, page } = await start();
  page.Telegram.WebApp.openLink = () => {
    throw Error('WebAppMethodUnsupported');
  };
  await assert.rejects(jump(42, SPOT), /Telegram не открыл браузер/);
});

test('кнопки подключения не отдают схему приложения Telegram напрямую', () => {
  const steps = readFileSync(new URL('./MiniSteps.tsx', import.meta.url), 'utf8');
  assert.ok(!/openExternal\(fillLink/.test(steps));
  assert.match(steps, /openApp\(fillLink\(link, url, brand\), spot\)/);
  assert.match(steps, /openApp\(link, \{ \.\.\.spot, button \}\)/);
});
