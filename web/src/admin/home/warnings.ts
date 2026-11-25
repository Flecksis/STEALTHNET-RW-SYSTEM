export type ConfigWarning = {
  kind: string;
  networks?: string[];
  seconds?: number;
  span?: string;
  evidence?: 'telegram' | 'login' | 'logins';
  miniapp?: 'off' | 'open' | 'closed';
};

export type WarningText = { title: string; text: string };

const CLOCK_FIX = 'Включите на сервере синхронизацию времени: sudo timedatectl set-ntp true. Перезапускать панель не нужно, предупреждение исчезнет само через пару минут.';

function clockTitle(warning: ConfigWarning, span: string): string {
  if (warning.evidence === 'login') {
    return `Часы сервера отстают не меньше чем на ${span}`;
  }
  if (warning.evidence === 'logins') {
    return `Похоже, часы сервера спешат на ${span}`;
  }
  return `Часы сервера ${(warning.seconds ?? 0) > 0 ? 'спешат' : 'отстают'} на ${span}`;
}

function clockSource(warning: ConfigWarning): string {
  if (warning.evidence === 'login') {
    return 'Это видно по данным, которые Telegram передаёт при входе в мини-приложение.';
  }
  if (warning.evidence === 'logins') {
    return 'Несколько клиентов подряд не смогли войти в мини-приложение: по часам сервера данным от Telegram больше часа.';
  }
  return 'Панель сравнила своё время со временем Telegram.';
}

function clockEffect(warning: ConfigWarning): string {
  if (warning.miniapp === 'closed') {
    return 'Пока время неверное, мини-приложение никого не пускает.';
  }
  if (warning.miniapp === 'open') {
    return 'Мини-приложение пока работает, но перестанет пускать клиентов, если часы уйдут вперёд больше чем на час. Коды входа из приложения уже могут не подходить.';
  }
  return 'Из-за этого могут не подходить коды входа из приложения и отклоняться уведомления некоторых платёжек.';
}

export function warningText(warning: ConfigWarning): WarningText | null {
  if (warning.kind === 'trusted_proxies_everyone') {
    const listed = (warning.networks ?? []).join(' и ') || '0.0.0.0/0';
    return {
      title: 'Любой посетитель может подменить свой адрес',
      text: `В файле .env в SN_TRUSTED_PROXIES стоит ${listed}: панель верит адресу из заголовка X-Forwarded-For от кого угодно. Так обходят ограничения попыток входа и промокодов по адресу, а в журнале остаются чужие адреса. Оставьте в SN_TRUSTED_PROXIES только адреса своих прокси и перезапустите систему.`,
    };
  }
  if (warning.kind === 'server_clock' && warning.span) {
    return {
      title: clockTitle(warning, warning.span),
      text: [clockSource(warning), clockEffect(warning), CLOCK_FIX].join(' '),
    };
  }
  return null;
}
