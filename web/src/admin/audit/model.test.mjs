import { test } from 'node:test';
import assert from 'node:assert/strict';
import { EMPTY, actorFilterLabel, chipsOf, filterCount, filtersOf, paramsOf, queryOf, rangeOf, summaryOf, valueText, whatFilterLabel, whenShort } from './model.ts';

const meta = {
  admins: [{ id: 2, email: 'anna@studio.example', role: 'Администратор', disabled: false }],
  keys: [{ id: 4, name: 'CRM-выгрузка', owner: 'owner@studio.example', revoked: false }],
  modules: [{ id: 'polls', name: 'Опросы после оплаты' }],
  areas: [{ id: 'clients', title: 'Клиенты', actions: [{ code: 'client.block', title: 'Клиент заблокирован' }] }],
  search_min: 3,
};

test('фильтры из адреса проверяются, чужое отбрасывается', () => {
  const params = new URLSearchParams('actor=admin:2&what=area:clients&client=1042&period=week&q=WINTER20');
  assert.deepEqual(filtersOf(params), { ...EMPTY, actor: 'admin:2', what: 'area:clients', client: '1042', period: 'week', q: 'WINTER20' });
  const bad = new URLSearchParams("actor=admin:x&what=drop&client=-1&period=year&since=вчера");
  assert.deepEqual(filtersOf(bad), EMPTY);
});

test('фильтры переживают адрес без потерь, пустые не пишутся', () => {
  const filters = { ...EMPTY, actor: 'module:polls', what: 'action:client.block', period: 'custom', since: '2026-11-01', until: '2026-11-19' };
  assert.deepEqual(filtersOf(paramsOf(filters)), filters);
  assert.equal(paramsOf(EMPTY).toString(), '');
  assert.equal(filterCount(filters), 3);
});

test('запрос к серверу: раздел или действие, период, курсор', () => {
  const now = new Date(2026, 10, 19, 15, 30).getTime();
  const area = new URL(queryOf({ ...EMPTY, what: 'area:clients', period: 'today' }, null, now), 'http://x');
  assert.equal(area.searchParams.get('area'), 'clients');
  assert.equal(area.searchParams.get('action'), null);
  assert.equal(area.searchParams.get('since'), new Date(2026, 10, 19).toISOString());
  assert.equal(area.searchParams.get('limit'), '50');
  const action = new URL(queryOf({ ...EMPTY, what: 'action:client.block', client: '7', q: '  спам ' }, '1763542800000000.42', now), 'http://x');
  assert.equal(action.searchParams.get('action'), 'client.block');
  assert.equal(action.searchParams.get('client'), '7');
  assert.equal(action.searchParams.get('q'), 'спам');
  assert.equal(action.searchParams.get('cursor'), '1763542800000000.42');
  assert.deepEqual(rangeOf({ ...EMPTY, period: 'custom', since: '2026-11-01', until: '2026-11-19' }, now), {
    since: new Date(2026, 10, 1).toISOString(),
    until: new Date(2026, 10, 20).toISOString(),
  });
  assert.equal(rangeOf({ ...EMPTY, period: 'week' }, now).since, new Date(2026, 10, 13).toISOString());
});

test('подписи фильтров берутся из справочника', () => {
  assert.equal(actorFilterLabel('', meta), 'Все участники');
  assert.equal(actorFilterLabel('admin:2', meta), 'anna@studio.example');
  assert.equal(actorFilterLabel('key:4', meta), 'Ключ «CRM-выгрузка»');
  assert.equal(actorFilterLabel('module:polls', meta), 'Модуль «Опросы после оплаты»');
  assert.equal(actorFilterLabel('admin:9', null), 'Администратор №9');
  assert.equal(whatFilterLabel('area:clients', meta), 'Клиенты');
  assert.equal(whatFilterLabel('action:client.block', meta), 'Клиент заблокирован');
});

test('строка под действием: сначала что изменилось, потом причина или текст', () => {
  const base = { id: 1, at: '2026-11-19T10:00:00', action: 'x.y', title: '', area: null, known: true, actor: { kind: 'system' }, target: null, changes: [], fields: [], ip: null };
  assert.equal(summaryOf({ ...base, changes: [{ label: 'Устройств', before: '3', after: '5' }] }), 'Устройств: 3 → 5');
  assert.equal(summaryOf({ ...base, fields: [{ key: 'size', label: 'Размер', value: '12' }, { key: 'reason', label: 'Причина', value: 'Спам' }] }), 'Спам');
  assert.equal(summaryOf({ ...base, fields: [{ key: 'size', label: 'Размер', value: '12' }] }), null);
});

test('короткая дата без года для текущего года', () => {
  const now = new Date(2026, 10, 19, 15, 0).getTime();
  assert.equal(whenShort(new Date(2026, 10, 19, 9, 5).toISOString(), now), '19.11, 09:05');
  assert.equal(whenShort(new Date(2025, 0, 2, 23, 59).toISOString(), now), '02.01.2025, 23:59');
});

test('плашки выбранных фильтров снимаются по одной', () => {
  const filters = { ...EMPTY, actor: 'key:4', client: '7', period: 'month', q: 'спам' };
  const chips = chipsOf(filters, meta, { kind: 'client', label: '@maria_k', note: null, link: null, client_id: 7 });
  assert.deepEqual(
    chips.map((c) => c.label),
    ['Ключ «CRM-выгрузка»', '@maria_k', '30 дней', '«спам»'],
  );
  assert.deepEqual(chips[2].clear, { period: 'all', since: '', until: '' });
});

test('даты в подробностях показываются по-человечески, остальное как есть', () => {
  const iso = new Date(2026, 10, 19, 4, 36, 50).toISOString();
  assert.equal(valueText(iso), '19.11.2026, 04:36');
  assert.equal(valueText('2026-11-19'), '2026-11-19');
  assert.equal(valueText('Рассылка спама'), 'Рассылка спама');
});
