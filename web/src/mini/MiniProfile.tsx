import { useEffect, useState } from 'react';
import { Link } from 'react-router';
import { api } from '../lib/api';
import { money } from '../lib/format';
import { t } from '../lib/i18n';
import { Icon } from '../lib/icons';
import { heldBalance, type Referral } from '../cabinet/model';
import { AccountCard } from './MiniAccount';
import { BackHead, useMini } from './MiniApp';
import { openExternal } from './telegram';
import { siteInviteLink } from '../lib/referral';
import { supportLink, supportTickets } from '../lib/support';

export function MiniProfile() {
  const { me, site } = useMini();
  const held = heldBalance(me);
  const [referral, setReferral] = useState<Referral | null>(null);
  const [copied, setCopied] = useState<string | null>(null);

  useEffect(() => {
    api<Referral>('/api/client/m/referral/me')
      .then(setReferral)
      .catch(() => setReferral(null));
  }, []);

  const link = supportLink(site);
  const chat = supportTickets(site) && site.support_ai?.miniapp === true;
  const ticketsBot = supportTickets(site) && !chat ? site.bot_username : null;

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
      <BackHead title={t('Профиль')} />
      {me.balance_enabled !== false ? (
        <div className="mn-card mn-pad">
          <span className="mn-muted">{t('Баланс')}</span>
          <strong className="mn-big">{money(me.balance)}</strong>
          {held ? <span className="mn-muted small">{t('Удержано до ответа платёжной системы: {amount}', { amount: money(held) })}</span> : null}
          <Link to="/app/topup" className="mn-btn2 press mn-btn-link">
            {t('Пополнить')}
          </Link>
        </div>
      ) : me.balance.minor > 0 || held ? (
        <div className="mn-card mn-pad">
          <span className="mn-muted">{t('Остаток на балансе')}</span>
          <strong className="mn-big">{money(me.balance)}</strong>
          {held ? <span className="mn-muted small">{t('Удержано до ответа платёжной системы: {amount}', { amount: money(held) })}</span> : null}
          <span className="mn-muted small">{t('Пополнить баланс нельзя. Остатком можно оплатить подписку, если его хватает на всю сумму')}</span>
        </div>
      ) : null}
      {referral ? (
        <div className="mn-card mn-pad">
          <strong>{t('Друзья')}</strong>
          <span className="mn-ink2">{t('Пригласите друга и получайте {percent}% от его оплат на баланс.', { percent: referral.levels[0] ?? 0 })}</span>
          {referral.bot_link ? (
            <>
              <span className="mn-muted small">{t('Ссылка на Telegram-бота')}</span>
              <span className="mn-url mono">{referral.bot_link.replace(/^https:\/\//, '')}</span>
              <button type="button" className="mn-btn2 press" onClick={() => void copy(referral.bot_link ?? '', 'bot')}>
                {copied === 'bot' ? t('Скопировано') : t('Скопировать ссылку')}
              </button>
            </>
          ) : null}
          <span className="mn-muted small">{t('Ссылка на сайт')}</span>
          <span className="mn-url mono">{siteInviteLink(referral.code).replace(/^https?:\/\//, '')}</span>
          <button type="button" className="mn-btn2 press" onClick={() => void copy(siteInviteLink(referral.code), 'site')}>
            {copied === 'site' ? t('Скопировано') : t('Скопировать ссылку')}
          </button>
          <span className="mn-muted small">{t('Пришли по ссылке: {count} · заработано {amount}', { count: referral.invited, amount: money(referral.earned) })}</span>
          {referral.withdrawals_enabled && referral.balance && referral.balance.minor > 0 ? (
            <Link to="/app/withdraw" className="mn-btn2 press mn-btn-link">
              {t('Вывести {amount}', { amount: money(referral.balance) })}
            </Link>
          ) : (referral.withdrawals?.length ?? 0) > 0 ? (
            <Link to="/app/withdraw" className="mn-link press">
              {t('Заявки на вывод')}
            </Link>
          ) : null}
        </div>
      ) : null}
      <div className="mn-list">
        <Link to="/app/payments" className="mn-row press">
          <Icon name="receipt" size={21} />
          <span className="mn-row-name">{t('История оплат')}</span>
          {me.payments_count ? <span className="mn-row-val num">{me.payments_count}</span> : null}
          <Icon name="chevron-right" size={16} />
        </Link>
        {chat ? (
          <Link to="/app/support" className="mn-row press">
            <Icon name="chat" size={21} />
            <span className="mn-row-name">{link ? t('Написать в поддержку') : t('Поддержка')}</span>
            <Icon name="chevron-right" size={16} />
          </Link>
        ) : null}
        {ticketsBot ? (
          <button type="button" className="mn-row press" onClick={() => openExternal(`https://t.me/${ticketsBot}?start=support`)}>
            <Icon name="chat" size={21} />
            <span className="mn-row-name">{link ? t('Написать в поддержку') : t('Поддержка')}</span>
            <Icon name="chevron-right" size={16} />
          </button>
        ) : null}
        {link ? (
          <button type="button" className="mn-row press" onClick={() => openExternal(link)}>
            <Icon name="chat" size={21} />
            <span className="mn-row-name">{ticketsBot || chat ? t('Чат поддержки') : t('Поддержка')}</span>
            <Icon name="chevron-right" size={16} />
          </button>
        ) : null}
      </div>
      <AccountCard />
    </main>
  );
}
