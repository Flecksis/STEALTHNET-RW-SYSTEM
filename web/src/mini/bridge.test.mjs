import { test } from 'node:test';
import assert from 'node:assert/strict';
import { bridgeApp, launchFrom, saveLaunch, savedLaunch, versionAtLeast } from './bridge.ts';
import { holdScreen } from './screen.ts';
import { insetVars, watchInsets } from './insets.ts';

const DATA = 'query_id=AA&user=%7B%22id%22%3A7%2C%22first_name%22%3A%22%D0%90%D0%BD%D0%BD%D0%B0%22%7D&auth_date=1764000000&hash=ff';
const HASH = `#tgWebAppData=${encodeURIComponent(DATA)}&tgWebAppVersion=9.1&tgWebAppPlatform=ios&tgWebAppThemeParams=${encodeURIComponent('{"bg_color":"#17212b"}')}`;

function fakeStore(initial = {}) {
  const map = new Map(Object.entries(initial));
  return {
    map,
    getItem: (key) => map.get(key) ?? null,
    setItem: (key, value) => {
      map.set(key, value);
    },
  };
}

function start(launch = launchFrom(HASH)) {
  const sent = [];
  const host = {};
  const proxy = { postEvent: (type, data) => sent.push([type, JSON.parse(data)]) };
  const app = bridgeApp(launch, proxy, host);
  const receive = (type, data) => host.Telegram.WebView.receiveEvent(type, data);
  return { app, host, sent, receive, names: () => sent.map(([name]) => name) };
}

test('параметры запуска Telegram читаются из адреса, чужие не берутся', () => {
  const launch = launchFrom(HASH);
  assert.equal(launch.tgWebAppData, DATA);
  assert.equal(launch.tgWebAppVersion, '9.1');
  assert.equal(launch.tgWebAppPlatform, 'ios');
  assert.deepEqual(launchFrom(''), {});
  assert.deepEqual(launchFrom('#plans'), {});
  assert.deepEqual(launchFrom('#a=1&b=2'), {});
  assert.deepEqual(launchFrom('#plans?tgWebAppVersion=8.0'), { tgWebAppVersion: '8.0' });
  assert.deepEqual(launchFrom('#tgWebAppData=a%3D1?b&tgWebAppVersion=8.0'), { tgWebAppData: 'a=1?b', tgWebAppVersion: '8.0' });
});

test('параметры запуска переживают перезагрузку в том виде, в каком их читает скрипт Telegram', () => {
  const store = fakeStore();
  saveLaunch(store, launchFrom(HASH));
  assert.deepEqual([...store.map.keys()], ['__telegram__initParams']);
  assert.deepEqual(savedLaunch(store), launchFrom(HASH));
  assert.deepEqual(savedLaunch(fakeStore()), {});
  assert.deepEqual(savedLaunch(fakeStore({ __telegram__initParams: 'мусор' })), {});
  assert.deepEqual(savedLaunch(fakeStore({ __telegram__initParams: '[1,2]' })), {});
  assert.deepEqual(savedLaunch(fakeStore({ __telegram__initParams: '{"_path":"plans","tgWebAppVersion":"8.0","tgWebAppData":5}' })), { tgWebAppVersion: '8.0' });
  const closed = {
    getItem: () => {
      throw new Error('SecurityError');
    },
    setItem: () => {
      throw new Error('SecurityError');
    },
  };
  assert.deepEqual(savedLaunch(closed), {});
  assert.doesNotThrow(() => saveLaunch(closed, { tgWebAppVersion: '8.0' }));
});

test('версии сравниваются по частям, а не как дроби', () => {
  assert.equal(versionAtLeast('8.0', '8.0'), true);
  assert.equal(versionAtLeast('7.10', '7.7'), true);
  assert.equal(versionAtLeast('7.10', '8.0'), false);
  assert.equal(versionAtLeast('10.1', '9.6'), true);
  assert.equal(versionAtLeast('8', '8.0'), true);
  assert.equal(versionAtLeast('', '6.0'), false);
});

test('запасной мост отвечает Telegram теми же событиями, что и скрипт', () => {
  const { app, sent } = start();
  assert.deepEqual(sent, [
    ['web_app_request_safe_area', ''],
    ['web_app_request_content_safe_area', ''],
  ]);
  sent.length = 0;
  app.ready();
  app.expand();
  app.disableVerticalSwipes();
  app.requestFullscreen();
  app.setHeaderColor('#0B0C0F');
  app.setBackgroundColor('#0B0C0F');
  app.openLink('https://pay.example/a?b=1');
  app.openTelegramLink('https://t.me/demo_bot?start=x');
  app.downloadFile({ url: 'https://panel.example/f', file_name: 'f.pdf' });
  app.BackButton.show();
  app.BackButton.hide();
  app.close();
  assert.deepEqual(sent, [
    ['web_app_ready', ''],
    ['web_app_expand', ''],
    ['web_app_setup_swipe_behavior', { allow_vertical_swipe: false }],
    ['web_app_request_fullscreen', ''],
    ['web_app_set_header_color', { color: '#0B0C0F' }],
    ['web_app_set_background_color', { color: '#0B0C0F' }],
    ['web_app_open_link', { url: 'https://pay.example/a?b=1' }],
    ['web_app_open_tg_link', { path_full: '/demo_bot?start=x' }],
    ['web_app_request_file_download', { url: 'https://panel.example/f', file_name: 'f.pdf' }],
    ['web_app_setup_back_button', { is_visible: true }],
    ['web_app_setup_back_button', { is_visible: false }],
    ['web_app_close', {}],
  ]);
});

test('ссылки проверяются как в скрипте Telegram: относительные дополняются, чужие схемы не открываются', () => {
  const sent = [];
  const host = { location: { href: 'https://panel.example/app/plans#x' } };
  const app = bridgeApp({ tgWebAppVersion: '8.0' }, { postEvent: (type, data) => sent.push([type, JSON.parse(data)]) }, host);
  sent.length = 0;
  app.openLink('/api/pay/42?x=1');
  app.downloadFile({ url: '/files/7', file_name: 'a.pdf' });
  assert.deepEqual(sent, [
    ['web_app_open_link', { url: 'https://panel.example/api/pay/42?x=1' }],
    ['web_app_request_file_download', { url: 'https://panel.example/files/7', file_name: 'a.pdf' }],
  ]);
  for (const url of ['happ://add/x', 'javascript:alert(1)', 'data:text/html,x', 'tg://resolve?domain=x']) {
    assert.throws(() => app.openLink(url), /WebAppTgUrlInvalid/, url);
    assert.throws(() => app.openTelegramLink(url), /WebAppTgUrlInvalid/, url);
    assert.throws(() => app.downloadFile({ url, file_name: 'a' }), /WebAppTgUrlInvalid/, url);
  }
  assert.throws(() => app.downloadFile({ url: 'http://panel.example/f', file_name: 'a' }), /WebAppTgUrlInvalid/);
  assert.equal(sent.length, 2);
});

test('запасной мост знает вход, версию, платформу и тему', () => {
  const { app } = start();
  assert.equal(app.initData, DATA);
  assert.equal(app.platform, 'ios');
  assert.equal(app.colorScheme, 'dark');
  assert.equal(app.isFullscreen, false);
  assert.equal(app.isVersionAtLeast('9.0'), true);
  assert.equal(app.isVersionAtLeast('9.2'), false);
  assert.equal(start({ tgWebAppThemeParams: '{"bg_color":"#ffffff"}' }).app.colorScheme, 'light');
  assert.equal(start({ tgWebAppThemeParams: 'мусор' }).app.colorScheme, undefined);
  assert.equal(start({ tgWebAppFullscreen: '1' }).app.isFullscreen, true);
  const old = start({ tgWebAppVersion: '6.0' });
  assert.deepEqual(old.sent, []);
  old.app.BackButton.show();
  assert.deepEqual(old.sent, []);
  assert.throws(() => old.app.downloadFile({ url: 'https://panel.example/f', file_name: 'f.pdf' }), /WebAppMethodUnsupported/);
});

test('события клиента доходят обоими путями: Android зовёт WebView, iOS и компьютер — TelegramGameProxy', () => {
  const { app, host } = start();
  const seen = [];
  app.onEvent('fullscreenChanged', () => seen.push(['changed', app.isFullscreen]));
  app.onEvent('fullscreenFailed', (payload) => seen.push(['failed', payload.error]));
  app.onEvent('activated', () => seen.push(['activated']));
  app.onEvent('deactivated', () => seen.push(['deactivated']));
  const back = () => seen.push(['back']);
  app.BackButton.onClick(back);
  host.Telegram.WebView.receiveEvent('fullscreen_changed', { is_fullscreen: true });
  host.TelegramGameProxy.receiveEvent('fullscreen_changed', { is_fullscreen: false });
  host.Telegram.WebView.receiveEvent('fullscreen_failed', { error: 'UNSUPPORTED' });
  host.TelegramGameProxy.receiveEvent('visibility_changed', { is_visible: true });
  host.TelegramGameProxy.receiveEvent('visibility_changed', { is_visible: false });
  host.Telegram.WebView.receiveEvent('back_button_pressed', null);
  app.BackButton.offClick(back);
  host.Telegram.WebView.receiveEvent('back_button_pressed');
  host.Telegram.WebView.receiveEvent('unknown_event', 5);
  assert.deepEqual(seen, [['changed', true], ['changed', false], ['failed', 'UNSUPPORTED'], ['activated'], ['deactivated'], ['back']]);
  host.Telegram.WebView.receiveEvent('fullscreen_failed', { error: 'ALREADY_FULLSCREEN' });
  assert.equal(app.isFullscreen, true);
});

test('ошибка одного обработчика не мешает остальным', () => {
  const { app, receive } = start();
  const seen = [];
  app.onEvent('activated', () => {
    throw new Error('boom');
  });
  app.onEvent('activated', () => seen.push('ok'));
  assert.doesNotThrow(() => receive('visibility_changed', { is_visible: true }));
  assert.deepEqual(seen, ['ok']);
});

test('отступы под часы и кнопки Telegram приходят и без скрипта', () => {
  const { app, receive } = start();
  const vars = new Map();
  const stop = watchInsets(app, {
    style: {
      setProperty: (name, value) => vars.set(name, value),
      removeProperty: (name) => {
        vars.delete(name);
        return '';
      },
    },
  });
  assert.equal(vars.get('--mn-tg-safe-top'), '0px');
  receive('safe_area_changed', { top: 59, bottom: 34, left: 0, right: 0 });
  receive('content_safe_area_changed', { top: 46, bottom: 0, left: 0, right: 0 });
  assert.equal(vars.get('--mn-tg-safe-top'), '59px');
  assert.equal(vars.get('--mn-tg-safe-bottom'), '34px');
  assert.equal(vars.get('--mn-tg-content-top'), '46px');
  receive('safe_area_changed', null);
  assert.deepEqual(insetVars(app)['--mn-tg-safe-top'], '59px');
  stop();
});

test('полный экран и запрет свайпа работают через запасной мост', (t) => {
  t.mock.timers.enable({ apis: ['setTimeout'] });
  const { app, receive, names } = start();
  const root = { dataset: {} };
  const stop = holdScreen(app, root);
  assert.deepEqual(names().slice(2), ['web_app_ready', 'web_app_expand', 'web_app_setup_swipe_behavior', 'web_app_request_fullscreen']);
  receive('fullscreen_changed', { is_fullscreen: true, blur_enabled: true });
  t.mock.timers.tick(60_000);
  assert.equal(names().filter((name) => name === 'web_app_request_fullscreen').length, 1);
  assert.deepEqual(root.dataset, {});
  stop();

  const old = start({ tgWebAppVersion: '7.2', tgWebAppPlatform: 'android' });
  const oldRoot = { dataset: {} };
  holdScreen(old.app, oldRoot);
  assert.deepEqual(old.names(), ['web_app_ready', 'web_app_expand']);
  assert.equal(oldRoot.dataset.tgSwipes, 'css');
});

test('объект Telegram, который уже есть на странице, не теряется', () => {
  const host = { Telegram: { Utils: 1 } };
  bridgeApp({ tgWebAppVersion: '8.0' }, { postEvent: () => undefined }, host);
  assert.equal(host.Telegram.Utils, 1);
  assert.equal(typeof host.Telegram.WebView.receiveEvent, 'function');
});
