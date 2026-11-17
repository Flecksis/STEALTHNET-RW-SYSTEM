import { Icon } from '../../lib/icons';
import { TelegramText } from '../pages/telegram';
import type { ButtonSpec } from './model';
import '../pages/bot.css';

type Props = {
  text: string;
  image: string | null;
  buttons: ButtonSpec[];
  className?: string;
};

export function Preview({ text, image, buttons, className }: Props) {
  return (
    <div className={`bc-preview ${className ?? ''}`} aria-label="Как увидит клиент">
      <span className="bc-preview-cap">Так увидит клиент</span>
      <div className="bt-preview">
        {image ? <img src={image} alt="" className="bt-preview-img" /> : null}
        {text.trim() ? (
          <div className="bt-bubble">
            <TelegramText html={text} />
          </div>
        ) : (
          <span className="cap">Здесь появится текст сообщения</span>
        )}
      </div>
      {buttons.length ? (
        <div className="bc-keys">
          {buttons.map((button, index) => (
            <span key={index} className="bt-tg-btn bc-key">
              <span>{button.text.trim() || 'Кнопка'}</span>
              {button.url !== undefined && !button.section ? <Icon name="external" size={13} /> : null}
            </span>
          ))}
        </div>
      ) : null}
    </div>
  );
}
