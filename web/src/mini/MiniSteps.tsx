import type { CSSProperties, ReactNode } from 'react';
import { AppMark } from '../cabinet/ConnectSteps';
import { GROUP_OF, PLATFORM_NAME, fillLink, local, useConnectApps, usePicked, type Group, type LinkButton, type Step } from '../cabinet/connect';
import { t } from '../lib/i18n';
import { QrCode } from '../lib/qr';
import { openExternal } from './telegram';
import './connect.css';

type Props = {
  url: string | null;
  brand: string;
  copied: boolean;
  onCopy: () => void;
  qr: boolean;
  onQr: () => void;
  reissue: ReactNode;
  waiting: string;
};

function Buttons({ buttons }: { buttons: LinkButton[] }) {
  if (!buttons.length) {
    return null;
  }
  return (
    <span className="mn-step-btns">
      {buttons.map((button) => (
        <button key={`${button.url}-${local(button.label)}`} type="button" className="mn-btn2 press" onClick={() => openExternal(button.url)}>
          {local(button.label)}
        </button>
      ))}
    </span>
  );
}

export function MiniSteps({ url, brand, copied, onCopy, qr, onQr, reissue, waiting }: Props) {
  const apps = useConnectApps(true);
  const picked = usePicked(apps, true);
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

  const card = (step: Step, number: number) => {
    const key = `${picked.platform}-${app.id}-${number}`;
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
            <Buttons buttons={step.buttons} />
          </div>
        );
      }
      case 'add': {
        const link = step.link;
        return (
          <div key={key} className="mn-card mn-step col">
            <span className="mn-step-row">{head(t('Добавьте подписку'), local(step.text))}</span>
            {link ? (
              <button type="button" className="mn-main sm press" onClick={() => url && openExternal(fillLink(link, url, brand))} disabled={!url}>
                {t('Добавить в {app}', { app: app.name })}
              </button>
            ) : null}
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
            <Buttons buttons={step.buttons} />
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
