import { msg, t } from './i18n';

export type Author = 'client' | 'support' | 'ai' | 'system';

export type HandoffReason = 'human' | 'money' | 'unsure' | 'groq' | 'client_limit' | 'budget' | 'operator' | 'disabled';

export type TicketAi = { state: 'active' | 'handed_off'; reason?: HandoffReason; handoff_at: string | null };

export type TicketBrief = {
  id: number;
  status: 'open' | 'closed';
  subject: string | null;
  last_author: Author;
  created_at: string;
  updated_at: string;
  closed_at: string | null;
};

export type ChatMessage = { author: Author; body: string; created_at: string };

export type ChatThread = { ticket: TicketBrief; messages: ChatMessage[]; ai?: TicketAi | null };

export const AI_NAME = msg('ИИ-помощник');

export function aiName(): string {
  return t(AI_NAME);
}

export const MAX_BODY = 4000;

const TYPING_MINUTES = 3;

export function authorName(author: Author): string {
  if (author === 'client') {
    return t('Вы');
  }
  return author === 'ai' ? aiName() : t('Поддержка');
}

export function visibleMessages(thread: ChatThread | null): ChatMessage[] {
  return (thread?.messages ?? []).filter((m) => m.author !== 'system');
}

export function aiAnswering(thread: ChatThread | null): boolean {
  return thread?.ai?.state === 'active' && thread.ticket.status === 'open';
}

export function aiTyping(thread: ChatThread | null): boolean {
  if (!aiAnswering(thread)) {
    return false;
  }
  const last = visibleMessages(thread).at(-1);
  if (!last || last.author !== 'client') {
    return false;
  }
  return Date.now() - Date.parse(last.created_at) < TYPING_MINUTES * 60_000;
}

export function handoffAfter(thread: ChatThread | null): number | null {
  const at = thread?.ai?.state === 'handed_off' ? thread.ai.handoff_at : null;
  if (!at) {
    return null;
  }
  const moment = Date.parse(at);
  return visibleMessages(thread).filter((m) => Date.parse(m.created_at) <= moment).length;
}

export function ticketTitle(ticket: TicketBrief): string {
  return ticket.subject?.trim() || t('Обращение №{id}', { id: ticket.id });
}
