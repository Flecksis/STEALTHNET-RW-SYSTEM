import { useEffect, useState } from 'react';
import { api } from '../lib/api';
import { money } from '../lib/format';
import { Icon } from '../lib/icons';
import type { Referral } from '../cabinet/model';
import { BackHead, useMini } from './MiniApp';
import { openExternal } from './telegram';
import { siteInviteLink } from '../lib/referral';

export function MiniProfile() {
  const { me, site } = useMini();
  const [referral, setReferral] = useState<Referral | null>(null);
  const [copied, setCopied] = useState<string | null>(null);

  useEffect(() => {
    api<Referral>('/api/client/m/referral/me')
      .then(setReferral)
      .catch(() => setReferral(null));
  }, []);

  const copy = async (text: string, key: string) => {
    try {
      await navigator.clipboard.writeText(text);
      setCopied(key);
      window.setTimeout(() => setCopied(null), 2000);
    } catch {
      return;
    }
  };

  return (
    <main className="mn-page profile">
      <BackHead title="Профиль" />
      {me.balance_enabled !== false ? (
        <div className="mn-card mn-pad">
          <span className="mn-muted">Баланс</span>
          <strong className="mn-big">{money(me.balance)}</strong>
        </div>
      ) : me.balance.minor > 0 ? (
        <div className="mn-card mn-pad">
          <span className="mn-muted">Остаток на балансе</span>
          <strong className="mn-big">{money(me.balance)}</strong>
          <span className="mn-muted small">Пополнить баланс нельзя. Остатком можно оплатить подписку, если его хватает на всю сумму</span>
        </div>
      ) : null}
      {referral ? (
        <div className="mn-card mn-pad">
          <strong>Друзья</strong>
          <span className="mn-ink2">Пригласите друга и получайте {referral.levels[0] ?? 0}% от его оплат на баланс.</span>
          {referral.bot_link ? (
            <>
              <span className="mn-muted small">Ссылка на Telegram-бота</span>
              <span className="mn-url mono">{referral.bot_link.replace(/^https:\/\//, '')}</span>
              <button type="button" className="mn-btn2 press" onClick={() => void copy(referral.bot_link ?? '', 'bot')}>
                {copied === 'bot' ? 'Скопировано' : 'Скопировать ссылку'}
              </button>
            </>
          ) : null}
          <span className="mn-muted small">Ссылка на сайт</span>
          <span className="mn-url mono">{siteInviteLink(referral.code).replace(/^https?:\/\//, '')}</span>
          <button type="button" className="mn-btn2 press" onClick={() => void copy(siteInviteLink(referral.code), 'site')}>
            {copied === 'site' ? 'Скопировано' : 'Скопировать ссылку'}
          </button>
          <span className="mn-muted small">
            Пришли по ссылке: {referral.invited} · заработано {money(referral.earned)}
          </span>
        </div>
      ) : null}
      {site.support_url ? (
        <div className="mn-list">
          <button type="button" className="mn-row press" onClick={() => openExternal(site.support_url ?? '')}>
            <Icon name="chat" size={21} />
            <span className="mn-row-name">Поддержка</span>
            <Icon name="chevron-right" size={16} />
          </button>
        </div>
      ) : null}
    </main>
  );
}
