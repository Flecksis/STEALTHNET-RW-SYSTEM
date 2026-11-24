import { test } from 'node:test';
import assert from 'node:assert/strict';
import {
  cpuLimit,
  cpuShare,
  dataLine,
  fileKind,
  healthProblem,
  jobFailure,
  memoryLine,
  memoryText,
  plainSummary,
  platformsText,
  runChanges,
  runningNote,
  sizeText,
  stateTitle,
  zipBlocked,
} from './package.ts';

const NB = ' ';

const run = (over = {}) => ({
  port: 8095,
  memory_mb: 256,
  cpus: '1.0',
  internet: true,
  volumes: [{ name: 'data', path: '/data' }],
  health_path: '/healthz',
  events_path: null,
  page_path: '/',
  ...over,
});

const preview = (over = {}) => ({
  id: 'nodes',
  name: 'Автоустановка нод',
  version: '1.1.0',
  author: 'STEALTHNET',
  description: null,
  site: null,
  webhook: null,
  page: { url: '', path: '/', title: 'Установка нод', permission: 'infrastructure', permission_title: 'Инфраструктура' },
  settings: [],
  events: [],
  permissions: [],
  sensitive: false,
  installed: null,
  moved: [],
  run: run(),
  previous_run: null,
  package: { bytes: 1, sha256: 'x', signature: { state: 'unsigned' }, platforms: ['linux/amd64'], processor: 'x86_64', image_bytes: 1 },
  ...over,
});

test('размеры и память по-русски', () => {
  assert.equal(sizeText(19.4 * 1024 * 1024), `19,4${NB}МБ`);
  assert.equal(sizeText(18 * 1024 * 1024), `18${NB}МБ`);
  assert.equal(sizeText(500 * 1024 * 1024), `500${NB}МБ`);
  assert.equal(sizeText(300), `1${NB}КБ`);
  assert.equal(sizeText(0), `0${NB}КБ`);
  assert.equal(memoryText(256), `256${NB}МБ`);
  assert.equal(memoryText(2048), `2${NB}ГБ`);
  assert.equal(memoryText(1536), `1,5${NB}ГБ`);
});

test('процессор словами', () => {
  assert.equal(cpuShare('0.5'), 'половину ядра');
  assert.equal(cpuShare('1.0'), 'одно ядро');
  assert.equal(cpuShare('2.0'), 'два ядра');
  assert.equal(cpuShare('1.5'), `1,5${NB}ядра`);
  assert.equal(cpuLimit('0.5'), 'до половины ядра');
  assert.equal(cpuLimit('1.0'), `до 1${NB}ядра`);
  assert.equal(cpuLimit('2.0'), `до 2${NB}ядер`);
  assert.equal(cpuLimit('1.5'), `до 1,5${NB}ядра`);
});

test('абзац для модуля без прав и событий — как на макете', () => {
  assert.equal(
    plainSummary(preview()),
    `На этом сервере модуль будет работать отдельной программой: до 256${NB}МБ памяти и одно ядро процессора, с выходом в интернет. ` +
      'В панели он ничего не читает и не меняет: у его ключа нет прав, события ему не приходят. ' +
      'Его страница «Установка нод» появится у администраторов с правом «Инфраструктура».',
  );
});

test('абзац называет права и события, снятые не считает', () => {
  const text = plainSummary(
    preview({
      page: null,
      run: run({ memory_mb: 128, cpus: '0.5', internet: false }),
      permissions: [
        { key: 'clients_read', title: 'Клиенты: просмотр', about: '', sensitive: false, change: 'same' },
        { key: 'money', title: 'Деньги', about: '', sensitive: true, change: 'added' },
        { key: 'tariffs', title: 'Тарифы', about: '', sensitive: false, change: 'removed' },
      ],
      events: [{ kind: 'payment.succeeded', title: 'Оплата прошла', about: '', change: 'same' }],
    }),
  );
  assert.equal(
    text,
    `На этом сервере модуль будет работать отдельной программой: до 128${NB}МБ памяти и половину ядра процессора, без выхода в интернет. ` +
      'В панели его ключ получит права «Клиенты: просмотр» и «Деньги», и ему будет приходить событие «Оплата прошла».',
  );
  const onlyEvents = plainSummary(preview({ page: null, events: [{ kind: 'a', title: 'А', about: '', change: 'same' }, { kind: 'b', title: 'Б', about: '', change: 'same' }] }));
  assert.match(onlyEvents, /у его ключа нет прав, но ему будут приходить события «А» и «Б»\.$/);
});

test('при обновлении видно, что меняется', () => {
  const changes = runChanges(
    preview({
      installed: { version: '1.0.0', enabled: true },
      previous_run: run({ memory_mb: 128, cpus: '0.5', internet: false, volumes: [] }),
      permissions: [{ key: 'money', title: 'Деньги', about: '', sensitive: true, change: 'added' }],
    }),
  );
  assert.deepEqual(
    changes.map((c) => [c.label, c.before, c.after]),
    [
      ['Версия', '1.0.0', '1.1.0'],
      ['Память', `до 128${NB}МБ`, `до 256${NB}МБ`],
      ['Процессор', 'половина ядра', 'одно ядро'],
      ['Интернет', 'нет', 'есть'],
      ['Папки данных', 'нет', '«data»'],
      ['Права ключа API', 'нет', '«Деньги»'],
    ],
  );
  assert.deepEqual(runChanges(preview({ installed: { version: '1.1.0', enabled: true }, previous_run: run() })), []);
});

test('архив или файл описания различаются по имени и типу', () => {
  assert.equal(fileKind('sn-node-autoinstall-1.1.0.ZIP', ''), 'zip');
  assert.equal(fileKind('module', 'application/x-zip-compressed'), 'zip');
  assert.equal(fileKind('module.json', ''), 'json');
  assert.equal(fileKind('module.txt', 'text/plain'), null);
  assert.equal(platformsText(['linux/amd64', 'linux/arm64']), 'Сборки для x86_64 и ARM64');
  assert.equal(platformsText(['linux/arm64']), 'Сборка для ARM64');
});

test('без помощника или не владельцу архив не предлагается', () => {
  const info = (state, owner = true) => ({
    owner,
    totp_required: false,
    packages: { state, why: null, busy: false },
    reason: 'панель установлена без прав root, поэтому помощника на сервере нет',
    processor: 'x86_64',
    max_bytes: 500 * 1024 * 1024,
  });
  assert.equal(zipBlocked(info('ready')), null);
  assert.equal(zipBlocked(info('ready', false))?.title, 'Модули из архива ставит владелец');
  assert.deepEqual(zipBlocked(info('ready', false))?.steps, []);
  const none = zipBlocked(info('no_helper'));
  assert.match(none?.about ?? '', /\. Панель установлена без прав root, поэтому помощника на сервере нет\.$/);
  assert.deepEqual(
    none?.steps.map((s) => s.command ?? null),
    ['sudo stealthnet helper on', null],
  );
  assert.equal(zipBlocked(info('outdated'))?.steps[0]?.command, 'sudo stealthnet update');
  assert.equal(zipBlocked(info('modules_off'))?.steps[0]?.command, 'sudo stealthnet modules on');
  assert.equal(zipBlocked(info('helper_off'))?.steps[0]?.command, 'sudo stealthnet helper on');
});

test('строки раздела «На сервере»', () => {
  const now = new Date(2026, 10, 24, 15, 0);
  const started = new Date(2026, 10, 21, 14, 0).toISOString();
  const container = { state: 'running', started_at: started, restarts: 0, image: null, memory_bytes: 61 * 1024 * 1024, data_bytes: 3.2 * 1024 * 1024, checked_at: null };
  const health = { ok: true, at: now.toISOString(), error: null, since: null };
  assert.equal(stateTitle('running'), 'Работает');
  assert.equal(stateTitle('что-то новое'), 'Состояние неизвестно');
  assert.equal(runningNote('running', container, health, now), `3${NB}дня без перезапусков · отвечает на проверку`);
  assert.equal(runningNote('restarting', { ...container, state: 'restarting', restarts: 15 }, null, now), 'перезапусков: 15');
  assert.equal(memoryLine(run(), container), `61${NB}МБ из 256${NB}МБ · процессор до 1${NB}ядра`);
  assert.equal(memoryLine(run(), null), `до 256${NB}МБ · процессор до 1${NB}ядра`);
  assert.equal(dataLine(run(), container), `папка «data», 3,2${NB}МБ`);
  assert.equal(dataLine(run({ volumes: [] }), container), 'модуль ничего не хранит');
  const sick = { ok: false, at: now.toISOString(), error: 'модуль не ответил за 10 секунд', since: new Date(2026, 10, 24, 14, 2).toISOString() };
  assert.equal(healthProblem('unhealthy', sick, now), 'Не отвечает на проверку с 14:02: модуль не ответил за 10 секунд');
  assert.equal(healthProblem('running', sick, now), null);
});

test('итог заявки словами', () => {
  const job = (state, action = 'module-install') => ({ id: 'a', action, state, steps: [], error: 'Контейнер модуля не запустился', requested_at: null, finished_at: null });
  assert.equal(jobFailure(job('failed')), 'Установка не прошла: Контейнер модуля не запустился');
  assert.equal(jobFailure(job('rolled_back', 'module-update')), 'Новая версия не заработала, вернулась прежняя: Контейнер модуля не запустился');
  assert.equal(jobFailure(job('done')), null);
  assert.equal(jobFailure(job('running')), null);
});
