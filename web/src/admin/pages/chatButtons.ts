export type MenuButton = { type: 'default' } | { type: 'commands' } | { type: 'web_app'; text: string; url: string };

export type ButtonsView = {
  username: string | null;
  wanted: MenuButton;
  menu: MenuButton;
  same: boolean;
  applied: boolean;
  main_app: boolean;
};

export type MenuNote = { warn: boolean; text: string; action: string | null };

export type MainApp = { warn: boolean; text: string };

export function placeOf(button: MenuButton): string {
  if (button.type !== 'web_app') {
    return 'обычное «Меню» с командами';
  }
  const label = button.text.trim();
  const target = button.url.trim() || 'неизвестный адрес';
  return label ? `кнопка «${label}», ведёт на ${target}` : `кнопка, ведёт на ${target}`;
}

export function menuNote(view: ButtonsView): MenuNote {
  const now = `Сейчас в Telegram: ${placeOf(view.menu)}.`;
  if (view.same) {
    return { warn: false, text: now, action: null };
  }
  const puts = view.wanted.type === 'web_app';
  if (!view.applied) {
    return {
      warn: true,
      text: `${now} Бот ${puts ? 'поставит новую' : 'уберёт её'} сам при первой возможности.`,
      action: puts ? 'Поставить сейчас' : 'Убрать сейчас',
    };
  }
  const auto = view.wanted.type === 'web_app' ? ` Вход без пароля работает только на ${view.wanted.url}` : '';
  return {
    warn: true,
    text: `${now} Её поставили не из панели: через @BotFather или прежним ботом.${auto}`,
    action: puts ? 'Поставить заново' : 'Убрать',
  };
}

export function mainApp(view: ButtonsView, miniapp: boolean, address: string | null): MainApp | null {
  const bot = view.username ? `@${view.username}` : 'ваш бот';
  const path = `/mybots → ${bot} → Bot Settings → Configure Mini App`;
  if (!miniapp || address === null) {
    return view.main_app ? { warn: true, text: `Включена, хотя приложение не работает. Уберите её в @BotFather: ${path}.` } : null;
  }
  const rule = `Боту Telegram менять её не даёт, только владельцу в @BotFather: ${path}.`;
  return {
    warn: false,
    text: view.main_app ? `Включена. ${rule} Проверьте там адрес: вход без пароля работает только на ${address}` : `Выключена. ${rule} Адрес для включения: ${address}`,
  };
}
