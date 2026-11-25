import { test } from 'node:test';
import assert from 'node:assert/strict';
import { isUnread, previewText, ticketGroups, ticketId, ticketMark, ticketWhen, unreadCount, waitingTicket } from './tickets.ts';

const ticket = (id, over = {}) => ({
  id,
  status: 'open',
  subject: null,
  last_author: 'client',
  created_at: '2026-11-20T10:00:00Z',
  updated_at: '2026-11-20T10:00:00Z',
  closed_at: null,
  ...over,
});

test('состояние обращения: новый ответ важнее остального', () => {
  assert.equal(ticketMark(ticket(1)), 'waiting');
  assert.equal(ticketMark(ticket(1, { last_author: 'support' })), 'answered');
  assert.equal(ticketMark(ticket(1, { last_author: 'ai' })), 'answered');
  assert.equal(ticketMark(ticket(1, { last_author: 'support', unread: 2 })), 'fresh');
  assert.equal(ticketMark(ticket(1, { status: 'closed', last_author: 'support' })), 'closed');
  assert.equal(ticketMark(ticket(1, { status: 'closed', last_author: 'support', unread: 1 })), 'fresh');
  assert.equal(ticketMark(ticket(1, { unread: 0 })), 'waiting');
});

test('в списке сначала обращения с новым ответом, дальше по времени последнего сообщения', () => {
  const said = (day) => ({ preview: { author: 'client', text: 'а', at: `2026-11-${day}T10:00:00Z` } });
  const list = [
    ticket(5, said(20)),
    ticket(4, { last_author: 'support', unread: 1, ...said(18) }),
    ticket(3, { last_author: 'support', ...said(22) }),
    ticket(2, { status: 'closed', ...said(10) }),
    ticket(1, { status: 'closed', unread: 3, ...said(5) }),
    ticket(6, { status: 'closed', ...said(12) }),
  ];
  const groups = ticketGroups(list);
  assert.deepEqual(groups.open.map((t) => t.id), [4, 3, 5]);
  assert.deepEqual(groups.closed.map((t) => t.id), [1, 6, 2]);
  assert.equal(unreadCount(list), 2);
  assert.equal(unreadCount([]), 0);
  assert.deepEqual(list.map(isUnread), [false, true, false, false, true, false]);
  assert.deepEqual(ticketGroups([ticket(9), ticket(8)]).open.map((t) => t.id), [9, 8]);
});

test('в строке списка последнее сообщение одной строкой и его время', () => {
  assert.equal(previewText({ author: 'support', text: 'Обновите\n\n  подписку ', at: 'x' }), 'Обновите подписку');
  assert.equal(previewText({ author: 'client', text: '', file: 'экран.png', at: 'x' }), '');
  assert.equal(previewText(null), '');
  assert.equal(previewText(undefined), '');
  assert.equal(ticketWhen(ticket(1)), '2026-11-20T10:00:00Z');
  assert.equal(ticketWhen(ticket(1, { preview: { author: 'client', text: 'а', at: '2026-11-21T10:00:00Z' } })), '2026-11-21T10:00:00Z');
});

test('номер обращения из адреса принимается только целым положительным числом', () => {
  assert.equal(ticketId('12'), 12);
  for (const raw of [undefined, '', '0', '-1', '1.5', '12abc', 'new', '1e3', ' 7', '9999999999999999', '../1', '%31']) {
    assert.equal(ticketId(raw), null, String(raw));
  }
});

test('подсказка о дополнении — про открытое обращение, которое ждёт ответа', () => {
  assert.equal(waitingTicket([]), null);
  assert.equal(waitingTicket([ticket(1, { last_author: 'support' }), ticket(2, { status: 'closed' })]), null);
  assert.equal(waitingTicket([ticket(3, { last_author: 'ai' }), ticket(4), ticket(5)])?.id, 4);
});
