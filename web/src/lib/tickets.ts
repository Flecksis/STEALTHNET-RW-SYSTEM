import type { TicketBrief, TicketPreview } from './supportChat';

export type TicketMark = 'fresh' | 'answered' | 'waiting' | 'closed';

export type TicketGroups = { open: TicketBrief[]; closed: TicketBrief[] };

export function isUnread(ticket: TicketBrief): boolean {
  return (ticket.unread ?? 0) > 0;
}

export function unreadCount(list: TicketBrief[]): number {
  return list.filter(isUnread).length;
}

export function ticketMark(ticket: TicketBrief): TicketMark {
  if (isUnread(ticket)) {
    return 'fresh';
  }
  if (ticket.status === 'closed') {
    return 'closed';
  }
  return ticket.last_author === 'client' ? 'waiting' : 'answered';
}

export function ticketGroups(list: TicketBrief[]): TicketGroups {
  const group = (status: TicketBrief['status']) =>
    list
      .filter((ticket) => ticket.status === status)
      .sort((a, b) => Number(isUnread(b)) - Number(isUnread(a)) || Date.parse(ticketWhen(b)) - Date.parse(ticketWhen(a)));
  return { open: group('open'), closed: group('closed') };
}

export function freshFirst(list: TicketBrief[]): TicketBrief[] {
  return [...list].sort((a, b) => Number(isUnread(b)) - Number(isUnread(a)));
}

export function waitingTicket(list: TicketBrief[]): TicketBrief | null {
  return list.find((ticket) => ticket.status === 'open' && ticket.last_author === 'client') ?? null;
}

export function previewText(preview: TicketPreview | null | undefined): string {
  return preview?.text.replace(/\s+/g, ' ').trim() ?? '';
}

export function ticketWhen(ticket: TicketBrief): string {
  return ticket.preview?.at ?? ticket.updated_at;
}

export function ticketId(raw: string | undefined): number | null {
  const id = /^\d{1,15}$/.test(raw ?? '') ? Number(raw) : 0;
  return id > 0 ? id : null;
}
