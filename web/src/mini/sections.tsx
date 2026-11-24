import { useEffect, useState } from 'react';
import { Link } from 'react-router';
import { api } from '../lib/api';
import { count, plural } from '../lib/format';
import { t } from '../lib/i18n';
import { Icon, type IconName } from '../lib/icons';
import { supportLink, supportTickets } from '../lib/support';
import type { Site } from '../lib/types';
import type { Referral } from '../cabinet/model';
import { loadTrial } from '../cabinet/trial';
import { friendsPath, isSection, linkIcon, linkUrl, pick, say, SECTION_ICONS, sectionPath, shownTo, type Audience, type Item, type SectionKey } from './config';
import { actionText, rowText, tabText } from './labels';
import { useMini } from './MiniApp';
import { openExternal } from './telegram';

export type Extras = { referral: Referral | null; gifts: boolean; spins: number | null; trial: boolean };

export type Target = { to?: string; url?: string };

export function tabLabel(key: SectionKey): string {
  return t(tabText(key));
}

export function rowLabel(key: SectionKey): string {
  return t(rowText(key));
}

export function actionLabel(key: SectionKey): string {
  return t(actionText(key));
}

export function supportTarget(site: Site): Target | null {
  if (supportTickets(site) && site.support_ai?.miniapp) {
    return { to: sectionPath('support') };
  }
  const url = supportUrl(site);
  return url ? { url } : null;
}

export function supportUrl(site: Site): string | null {
  return supportTickets(site) && site.bot_username ? `https://t.me/${site.bot_username}?start=support` : supportLink(site);
}

export function useExtras(wanted = true): Extras {
  const { demo, site, view } = useMini();
  const [referral, setReferral] = useState<Referral | null>(null);
  const [gifts, setGifts] = useState(false);
  const [spins, setSpins] = useState<number | null>(null);
  const [trial, setTrial] = useState(false);
  const load = wanted && !demo;
  const noSub = !view;
  const trialOn = !!site.trial?.enabled;

  useEffect(() => {
    if (!load) {
      return;
    }
    api<Referral>('/api/client/m/referral/me')
      .then(setReferral)
      .catch(() => setReferral(null));
    api('/api/client/m/gifts/offers')
      .then(() => setGifts(true))
      .catch(() => setGifts(false));
    api<{ ready: boolean; spins: { total: number } }>('/api/client/m/roulette/state')
      .then((s) => setSpins(s.ready ? s.spins.total : null))
      .catch(() => setSpins(null));
  }, [load]);

  useEffect(() => {
    if (!load || !noSub || !trialOn) {
      setTrial(false);
      return;
    }
    let live = true;
    loadTrial()
      .then((offer) => live && setTrial(offer.available))
      .catch(() => live && setTrial(false));
    return () => {
      live = false;
    };
  }, [load, noSub, trialOn]);

  return demo ?? { referral, gifts, spins, trial };
}

export function useTargets(extras: Extras): (key: SectionKey) => Target | null {
  const { me, site, view, config } = useMini();
  const blocked = me.client.status === 'blocked' || view?.status === 'blocked';
  return (key) => {
    switch (key) {
      case 'friends':
        return extras.referral ? { to: friendsPath(config) } : null;
      case 'roulette':
        return extras.spins !== null ? { to: sectionPath(key) } : null;
      case 'addons':
        return me.options !== false && view && view.status !== 'expired' && !blocked && !view.subscription.ended_at ? { to: sectionPath(key) } : null;
      case 'gifts':
        return extras.gifts ? { to: sectionPath(key) } : null;
      case 'promo':
        return me.promo_codes === true ? { to: sectionPath(key) } : null;
      case 'topup':
        return me.balance_enabled !== false ? { to: sectionPath(key) } : null;
      case 'trial':
        return extras.trial ? { to: sectionPath(key) } : null;
      case 'support':
        return supportTarget(site);
      case 'renew':
      case 'devices':
        return view ? { to: sectionPath(key) } : null;
      default:
        return { to: sectionPath(key) };
    }
  };
}

export function useLinkTarget(extras: Extras): (to: string | null | undefined) => Target | null {
  const target = useTargets(extras);
  return (to) => {
    if (isSection(to)) {
      return target(to);
    }
    const url = linkUrl(to);
    return url ? { url } : null;
  };
}

export function needsExtras(items: Array<{ to?: string | null }>): boolean {
  return items.some((item) => isSection(item.to));
}

export type RowView = { key: string; label: string; icon: IconName; value?: string; target: Target; away: boolean };

export function useRows(extras: Extras, state: Audience): (items: Item[]) => RowView[] {
  const { me, lang } = useMini();
  const target = useTargets(extras);
  const link = useLinkTarget(extras);
  return (items) =>
    items.flatMap((item, index): RowView[] => {
      if (item.on === false) {
        return [];
      }
      if (item.std) {
        const at = target(item.std);
        return at ? [{ key: item.std, label: pick(item.label, lang, rowLabel(item.std)), icon: SECTION_ICONS[item.std], value: rowValue(item.std, extras, me.payments_count ?? 0), target: at, away: false }] : [];
      }
      const at = shownTo(item.for, state) ? link(item.to) : null;
      const label = say(item.label, lang);
      return at && label ? [{ key: `own-${index}`, label, icon: linkIcon(item), target: at, away: !!at.url && !isSection(item.to) }] : [];
    });
}

function rowValue(key: SectionKey, extras: Extras, payments: number): string | undefined {
  if (key === 'friends' && extras.referral?.invited) {
    const n = extras.referral.invited;
    return `${n} ${plural(n, 'пришёл', 'пришли', 'пришли')}`;
  }
  if (key === 'roulette' && extras.spins) {
    return count(extras.spins, 'вращение', 'вращения', 'вращений');
  }
  return key === 'payments' && payments ? String(payments) : undefined;
}

export function Row({ row }: { row: RowView }) {
  const inner = (
    <>
      <Icon name={row.icon} size={21} />
      <span className="mn-row-name">{row.label}</span>
      {row.value ? <span className="mn-row-val num">{row.value}</span> : null}
      <Icon name={row.away ? 'external' : 'chevron-right'} size={16} />
    </>
  );
  return row.target.to ? (
    <Link to={row.target.to} className="mn-row press">
      {inner}
    </Link>
  ) : (
    <button type="button" className="mn-row press" onClick={() => row.target.url && openExternal(row.target.url)}>
      {inner}
    </button>
  );
}
