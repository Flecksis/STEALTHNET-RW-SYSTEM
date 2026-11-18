import { test } from 'node:test';
import assert from 'node:assert/strict';
import { UTM_TTL_MS, savedTags, startParam, tagsFrom, withBotStart } from './utmTags.ts';

test('метки берутся из адреса, неподходящие значения отбрасываются', () => {
  assert.deepEqual(tagsFrom('?utm_source=tiktok&utm_medium=cpc&utm_campaign=osen-shorts&utm_content=%D0%B2%D0%B8%D0%B4%D0%B5%D0%BE&utm_term=a__b&ref=ABC'), {
    source: 'tiktok',
    medium: 'cpc',
    campaign: 'osen-shorts',
  });
  assert.deepEqual(tagsFrom('?utm_source=%20telegram_ads%20'), { source: 'telegram_ads' });
  assert.equal(tagsFrom('?utm_source=bad%20value'), null);
  assert.equal(tagsFrom(`?utm_source=${'a'.repeat(65)}`), null);
  assert.equal(tagsFrom('?ref=ABC'), null);
});

test('сохранённая метка живёт тридцать дней', () => {
  const now = 1_800_000_000_000;
  const raw = JSON.stringify({ source: 'vk', campaign: 'winter', at: now - 1000 });
  assert.deepEqual(savedTags(raw, now), { source: 'vk', campaign: 'winter' });
  assert.equal(savedTags(JSON.stringify({ source: 'vk', at: now - UTM_TTL_MS - 1 }), now), null);
  assert.equal(savedTags(JSON.stringify({ source: 'vk' }), now), null);
  assert.equal(savedTags('{', now), null);
  assert.equal(savedTags(null, now), null);
});

test('параметр бота собирается как в боте и не длиннее 64 символов', () => {
  assert.equal(startParam({ source: 'tiktok', medium: 'cpc', campaign: 'winter-sale' }, null), 's_tiktok__m_cpc__k_winter-sale');
  assert.equal(startParam({ source: 'tiktok' }, 'FRIEND42'), 'ref_FRIEND42__s_tiktok');
  assert.equal(startParam(null, 'FRIEND42'), 'ref_FRIEND42');
  assert.equal(startParam(null, null), null);
  const long = startParam({ source: 'a'.repeat(40), medium: 'b'.repeat(30), campaign: 'c'.repeat(10) }, 'FRIEND42');
  assert.equal(long, `ref_FRIEND42__s_${'a'.repeat(40)}`);
  assert.ok(long.length <= 64);
});

test('метка добавляется только к ссылке на своего бота без параметра', () => {
  assert.equal(withBotStart('https://t.me/Stealth_Bot', 'stealth_bot', 's_vk'), 'https://t.me/Stealth_Bot?start=s_vk');
  assert.equal(withBotStart('https://telegram.me/stealth_bot/', '@stealth_bot', 's_vk'), 'https://telegram.me/stealth_bot/?start=s_vk');
  assert.equal(withBotStart('https://t.me/stealth_bot?start=gift_X', 'stealth_bot', 's_vk'), 'https://t.me/stealth_bot?start=gift_X');
  assert.equal(withBotStart('https://t.me/other_bot', 'stealth_bot', 's_vk'), 'https://t.me/other_bot');
  assert.equal(withBotStart('https://t.me/stealth_bot/app', 'stealth_bot', 's_vk'), 'https://t.me/stealth_bot/app');
  assert.equal(withBotStart('https://example.com/stealth_bot', 'stealth_bot', 's_vk'), 'https://example.com/stealth_bot');
  assert.equal(withBotStart('https://t.me/stealth_bot', 'stealth_bot', null), 'https://t.me/stealth_bot');
  assert.equal(withBotStart('https://t.me/stealth_bot', null, 's_vk'), 'https://t.me/stealth_bot');
});
