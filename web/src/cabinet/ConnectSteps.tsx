import type { CSSProperties, ReactNode } from 'react';
import { t } from '../lib/i18n';
import { QrCode } from '../lib/qr';
import { GROUP_OF, PLATFORM_NAME, fillLink, isWebLink, local, useConnectApps, usePicked, type ClientApp, type ConnectPlatform, type Group, type LinkButton, type Step } from './connect';
import './connect.css';

type Props = {
  url: string | null;
  phone: boolean;
  brand: string;
  copied: boolean;
  onCopy: () => void;
  qr: boolean;
  onQr: () => void;
  reissue: ReactNode;
  waiting: string;
};

export function AppMark({ app, className }: { app: ClientApp; className: string }) {
  return (
    <span className={className} aria-hidden="true">
      {app.icon ? <img src={app.icon} alt="" decoding="async" /> : app.name.slice(0, 1).toUpperCase()}
    </span>
  );
}

function linkProps(href: string) {
  return isWebLink(href) ? { href, target: '_blank', rel: 'noreferrer' } : { href };
}

function Buttons({ buttons, phone }: { buttons: LinkButton[]; phone: boolean }) {
  if (!buttons.length) {
    return null;
  }
  return (
    <span className="cb-step-btns">
      {buttons.map((button) => (
        <a key={`${button.url}-${local(button.label)}`} className={phone ? 'cb-second' : 'cb-second sm'} {...linkProps(button.url)}>
          {local(button.label)}
        </a>
      ))}
    </span>
  );
}

function PcLabel() {
  return (
    <>
      <span className="cb-long">{t('Компьютер')}</span>
      <span className="cb-short">{t('ПК')}</span>
    </>
  );
}

export function ConnectSteps({ url, phone, brand, copied, onCopy, qr, onQr, reissue, waiting }: Props) {
  const apps = useConnectApps(true);
  const picked = usePicked(apps, phone);

  const copyButton = (main: boolean) => (
    <button type="button" className={main ? 'cb-main sm' : 'cb-second sm'} onClick={onCopy} disabled={!url}>
      {copied ? t('Скопировано') : t('Скопировать')}
    </button>
  );
  const qrButton = phone ? null : (
    <button type="button" className="cb-second sm" aria-expanded={qr} onClick={onQr} disabled={!url}>
      {t('QR-код')}
    </button>
  );
  const qrCode = qr && url ? <QrCode text={url} size={200} label={t('QR-код ссылки подписки')} /> : null;

  if (apps === undefined) {
    return <p className="cb-muted">{t('Загружаем приложения…')}</p>;
  }
  const app = picked.app;
  if (apps === null || !app || !picked.platform) {
    return (
      <>
        <p className="cb-ink2">{apps === null ? t('Список приложений не загрузился. Скопируйте ссылку и добавьте её в приложение для VPN.') : t('Скопируйте ссылку и добавьте её в приложение для VPN.')}</p>
        <div className={phone ? 'cb-linkrow col' : 'cb-linkrow'}>
          <span className="cb-link mono">{url ?? waiting}</span>
          {copyButton(false)}
          {qrButton}
        </div>
        {qrCode}
        {reissue}
      </>
    );
  }

  const inGroup = (group: Group) => picked.platforms.filter((platform) => GROUP_OF[platform] === group);
  const groups: Array<{ key: string; label: ReactNode; pressed: boolean; go: () => void }> = [];
  if (phone) {
    for (const [group, label] of [
      ['ios', 'iPhone'],
      ['android', 'Android'],
      ['pc', <PcLabel key="pc" />],
      ['tv', t('ТВ')],
    ] as Array<[Group, ReactNode]>) {
      if (inGroup(group).length) {
        groups.push({ key: group, label, pressed: picked.group === group, go: () => picked.setGroup(group) });
      }
    }
  } else {
    for (const platform of ['ios', 'android', 'windows', 'macos', 'linux'] as ConnectPlatform[]) {
      if (picked.platforms.includes(platform)) {
        groups.push({ key: platform, label: PLATFORM_NAME[platform], pressed: picked.platform === platform, go: () => picked.setPlatform(platform) });
      }
    }
    if (inGroup('tv').length) {
      groups.push({ key: 'tv', label: t('ТВ'), pressed: picked.group === 'tv', go: () => picked.setGroup('tv') });
    }
  }
  const siblings = picked.group && (phone ? picked.group === 'pc' || picked.group === 'tv' : picked.group === 'tv') ? inGroup(picked.group) : [];
  const segStyle: CSSProperties | undefined = phone && groups.length !== 3 ? { gridTemplateColumns: groups.length === 4 ? 'repeat(4, auto)' : `repeat(${groups.length}, minmax(0, 1fr))` } : undefined;

  const many = picked.apps.length > 1;
  const main = picked.apps[0] ?? app;
  const stepBody = (step: Step) => {
    switch (step.kind) {
      case 'install': {
        const text = local(step.text);
        if (!many && step.buttons.length <= 1) {
          return (
            <span className="cb-step">
              <span className="cb-step-row">
                <span className="cb-step-text">
                  <strong>{t('Установите {app}', { app: app.name })}</strong>
                  {text ? <span className="cb-muted cb-pre">{text}</span> : null}
                </span>
                {step.buttons.map((button) => (
                  <a key={button.url} className="cb-second sm" {...linkProps(button.url)}>
                    {local(button.label)}
                  </a>
                ))}
              </span>
            </span>
          );
        }
        return (
          <span className="cb-step">
            <span className="cb-step-text">
              <strong>{many ? t('Установите приложение') : t('Установите {app}', { app: app.name })}</strong>
              <span className="cb-muted cb-pre">{many ? t('Советуем {app}, подойдут и другие', { app: main.name }) : text}</span>
            </span>
            {many ? (
              <span role="group" aria-label={t('Приложение')} className="cb-apps">
                {picked.apps.map((item) => (
                  <button key={item.id} type="button" aria-pressed={item.id === app.id} onClick={() => picked.setApp(item.id)}>
                    <AppMark app={item} className="cb-app-ico" />
                    {item.name}
                  </button>
                ))}
              </span>
            ) : null}
            {many && text ? <span className="cb-muted cb-pre">{text}</span> : null}
            <Buttons buttons={step.buttons} phone={phone} />
          </span>
        );
      }
      case 'add': {
        const text = local(step.text);
        const href = step.link && url ? fillLink(step.link, url, brand) : null;
        return (
          <span className="cb-step">
            <span className="cb-step-text">
              <strong>{t('Добавьте подписку')}</strong>
              {text ? <span className="cb-muted cb-pre">{text}</span> : null}
            </span>
            <span className={phone ? 'cb-btns col' : 'cb-btns'}>
              {step.link ? (
                <a className={href ? 'cb-main sm' : 'cb-main sm off'} {...(href ? linkProps(href) : {})} aria-disabled={href ? undefined : true}>
                  {t('Добавить в {app}', { app: app.name })}
                </a>
              ) : null}
              {copyButton(!step.link)}
              {qrButton}
            </span>
            {qrCode}
            {phone ? reissue : null}
          </span>
        );
      }
      case 'connect': {
        const text = local(step.text);
        return (
          <span className="cb-step-text">
            <strong>{t('Включите VPN')}</strong>
            {text ? <span className="cb-muted cb-pre">{text}</span> : null}
          </span>
        );
      }
      default: {
        const text = local(step.text);
        return (
          <span className="cb-step">
            <span className="cb-step-text">
              <strong>{local(step.title)}</strong>
              {text ? <span className="cb-muted cb-pre">{text}</span> : null}
            </span>
            <Buttons buttons={step.buttons} phone={phone} />
          </span>
        );
      }
    }
  };

  return (
    <>
      <div role="group" aria-label={t('Устройство')} className={phone ? (groups.length === 4 ? 'cb-plats seg four' : 'cb-plats seg') : 'cb-plats'} style={segStyle}>
        {groups.map((group) => (
          <button key={group.key} type="button" aria-pressed={group.pressed} onClick={group.go}>
            {group.label}
          </button>
        ))}
      </div>
      {siblings.length > 1 ? (
        <div role="group" aria-label={t('Система')} className="cb-sub-plats">
          {siblings.map((platform) => (
            <button key={platform} type="button" aria-pressed={picked.platform === platform} onClick={() => picked.setPlatform(platform)}>
              {PLATFORM_NAME[platform]}
            </button>
          ))}
        </div>
      ) : null}
      <ol className="cb-steps">
        {app.steps.map((step, index) => (
          <li key={`${picked.platform}-${app.id}-${index}`}>
            <span className="cb-num">{index + 1}</span>
            {stepBody(step)}
          </li>
        ))}
      </ol>
      {phone ? null : reissue}
    </>
  );
}
