import { useState, type CSSProperties, type ReactNode } from 'react';
import { AppMark } from '../cabinet/ConnectSteps';
import { GROUP_OF, PLATFORM_NAME, fillLink, local, useConnectApps, usePicked, type Group, type LinkButton, type Step } from '../cabinet/connect';
import { ApiError } from '../lib/api';
import { t } from '../lib/i18n';
import { QrCode } from '../lib/qr';
import { jump, needsJump, type Spot } from './jump';
import { openExternal } from './telegram';
import './connect.css';

type Props = {
  subscription: number;
  url: string | null;
  brand: string;
  copied: boolean;
  onCopy: () => void;
  qr: boolean;
  onQr: () => void;
  reissue: ReactNode;
  waiting: string;
};

type Jump = { at: string; state: 'busy' | 'sent' | 'failed'; text: string };

function Buttons({ buttons, busy, onOpen }: { buttons: LinkButton[]; busy?: boolean; onOpen: (url: string, index: number) => void }) {
  if (!buttons.length) {
    return null;
  }
  return (
    <span className="mn-step-btns">
      {buttons.map((button, index) => (
        <button key={`${button.url}-${local(button.label)}`} type="button" className="mn-btn2 press" onClick={() => onOpen(button.url, index)} disabled={busy}>
          {local(button.label)}
        </button>
      ))}
    </span>
  );
}

export function MiniSteps({ subscription, url, brand, copied, onCopy, qr, onQr, reissue, waiting }: Props) {
  const apps = useConnectApps(true);
  const picked = usePicked(apps, true);
  const [last, setLast] = useState<Jump | null>(null);
  const busy = last?.state === 'busy';

  const openApp = async (link: string, spot: Spot) => {
    if (!needsJump(link)) {
      openExternal(link);
      return;
    }
    const at = `${subscription}-${spot.platform}-${spot.app}-${spot.step}`;
    setLast({ at, state: 'busy', text: '' });
    try {
      await jump(subscription, spot);
      setLast({ at, state: 'sent', text: '' });
    } catch (err) {
      setLast({ at, state: 'failed', text: err instanceof ApiError ? err.message : t('Telegram не открыл браузер. Попробуйте ещё раз') });
    }
  };

  const twoButtons = (
    <span className="mn-two">
      <button type="button" className="mn-btn2 press" onClick={onCopy} disabled={!url}>
        {copied ? t('Скопировано') : t('Скопировать')}
      </button>
      <button type="button" className="mn-btn2 press" aria-expanded={qr} onClick={onQr} disabled={!url}>
        {t('QR-код')}
      </button>
    </span>
  );
  const qrCode = qr && url ? <QrCode text={url} size={200} label={t('QR-код ссылки подписки')} /> : null;

  if (apps === undefined) {
    return <p className="mn-empty">{t('Загружаем приложения…')}</p>;
  }
  const app = picked.app;
  if (apps === null || !app || !picked.platform) {
    return (
      <div className="mn-card mn-remna">
        <span className="mn-ink2">{apps === null ? t('Список приложений не загрузился. Скопируйте ссылку и добавьте её в приложение для VPN.') : t('Скопируйте ссылку и добавьте её в приложение для VPN.')}</span>
        <span className="mn-url mono">{url ?? waiting}</span>
        {twoButtons}
        {qrCode}
        {reissue}
      </div>
    );
  }

  const inGroup = (group: Group) => picked.platforms.filter((platform) => GROUP_OF[platform] === group);
  const groups = (
    [
      ['ios', 'iPhone'],
      ['android', 'Android'],
      ['pc', t('Компьютер')],
      ['tv', t('ТВ')],
    ] as Array<[Group, string]>
  ).filter(([group]) => inGroup(group).length > 0);
  const siblings = picked.group === 'pc' || picked.group === 'tv' ? inGroup(picked.group) : [];
  const segStyle: CSSProperties | undefined = groups.length === 3 ? undefined : { gridTemplateColumns: groups.length === 4 ? 'repeat(4, auto)' : `repeat(${groups.length}, minmax(0, 1fr))` };
  const many = picked.apps.length > 1;
  const main = picked.apps[0] ?? app;

  const platform = picked.platform;
  const jumped = (index: number, text: string, hint: string | null) => {
    if (last?.at !== `${subscription}-${platform}-${app.id}-${index}` || last.state === 'busy') {
      return null;
    }
    return last.state === 'sent' ? (
      <span role="status" className="mn-hint">
        {text}
      </span>
    ) : (
      <span role="alert" className="mn-jump-failed">
        <span className="mn-error">{last.text}</span>
        {hint ? <span className="mn-hint">{hint}</span> : null}
      </span>
    );
  };

  const card = (step: Step, number: number) => {
    const key = `${picked.platform}-${app.id}-${number}`;
    const spot: Spot = { platform, app: app.id, step: number - 1 };
    const head = (title: string, text: string) => (
      <>
        <span className="mn-num">{number}</span>
        <span className="mn-step-text">
          <strong>{title}</strong>
          {text ? <span className="mn-pre">{text}</span> : null}
        </span>
      </>
    );
    switch (step.kind) {
      case 'install': {
        const text = local(step.text);
        if (!many && step.buttons.length <= 1) {
          return (
            <div key={key} className="mn-card mn-step">
              {head(t('Установите {app}', { app: app.name }), text)}
              {step.buttons.map((button) => (
                <button key={button.url} type="button" className="mn-btn2 press" onClick={() => openExternal(button.url)}>
                  {local(button.label)}
                </button>
              ))}
            </div>
          );
        }
        return (
          <div key={key} className="mn-card mn-step col">
            <span className="mn-step-row">{head(many ? t('Установите приложение') : t('Установите {app}', { app: app.name }), many ? t('Советуем {app}, подойдут и другие', { app: main.name }) : text)}</span>
            {many ? (
              <span role="group" aria-label={t('Приложение')} className="mn-apps">
                {picked.apps.map((item) => (
                  <button key={item.id} type="button" aria-pressed={item.id === app.id} onClick={() => picked.setApp(item.id)}>
                    <AppMark app={item} className="mn-app-ico" />
                    {item.name}
                  </button>
                ))}
              </span>
            ) : null}
            {many && text ? <span className="mn-hint mn-pre">{text}</span> : null}
            <Buttons buttons={step.buttons} onOpen={(link) => openExternal(link)} />
          </div>
        );
      }
      case 'add': {
        const link = step.link;
        return (
          <div key={key} className="mn-card mn-step col">
            <span className="mn-step-row">{head(t('Добавьте подписку'), local(step.text))}</span>
            {link ? (
              <button type="button" className="mn-main sm press" onClick={() => url && void openApp(fillLink(link, url, brand), spot)} disabled={!url || busy} aria-busy={busy}>
                {t('Добавить в {app}', { app: app.name })}
              </button>
            ) : null}
            {jumped(
              number - 1,
              t('Открываем {app} через браузер. Если не получилось, нажмите ещё раз или скопируйте ссылку.', { app: app.name }),
              t('Или скопируйте ссылку и добавьте её в {app} вручную.', { app: app.name }),
            )}
            {twoButtons}
            {qrCode}
            {reissue}
          </div>
        );
      }
      case 'connect':
        return (
          <div key={key} className="mn-card mn-step">
            {head(t('Включите VPN'), local(step.text))}
          </div>
        );
      default:
        return step.buttons.length ? (
          <div key={key} className="mn-card mn-step col">
            <span className="mn-step-row">{head(local(step.title), local(step.text))}</span>
            <Buttons buttons={step.buttons} busy={busy} onOpen={(link, button) => void openApp(link, { ...spot, button })} />
            {jumped(number - 1, t('Открываем {app} через браузер. Если не получилось, нажмите ещё раз.', { app: app.name }), null)}
          </div>
        ) : (
          <div key={key} className="mn-card mn-step">
            {head(local(step.title), local(step.text))}
          </div>
        );
    }
  };

  return (
    <div className="mn-steps">
      <div role="group" aria-label={t('Устройство')} className={groups.length === 4 ? 'mn-seg four' : groups.length === 3 ? 'mn-seg three' : 'mn-seg'} style={segStyle}>
        {groups.map(([group, label]) => (
          <button key={group} type="button" aria-pressed={picked.group === group} onClick={() => picked.setGroup(group)}>
            {group === 'pc' ? (
              <>
                <span className="mn-long">{label}</span>
                <span className="mn-short">{t('ПК')}</span>
              </>
            ) : (
              label
            )}
          </button>
        ))}
      </div>
      {siblings.length > 1 ? (
        <div role="group" aria-label={t('Система')} className="mn-seg small" style={{ gridTemplateColumns: `repeat(${siblings.length}, minmax(0, 1fr))` }}>
          {siblings.map((platform) => (
            <button key={platform} type="button" aria-pressed={picked.platform === platform} onClick={() => picked.setPlatform(platform)}>
              {PLATFORM_NAME[platform]}
            </button>
          ))}
        </div>
      ) : null}
      {app.steps.map((step, index) => card(step, index + 1))}
    </div>
  );
}
