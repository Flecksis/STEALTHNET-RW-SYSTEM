import { count, NBSP } from '../../lib/format';
import type { ServerLoad } from './model';

const MB = 1024 ** 2;
const GB = 1024 ** 3;
const TB = 1024 ** 4;

function decimal(value: number, digits: number): string {
  return value.toFixed(digits).replace('.', ',').replace(/,0+$/, '');
}

function unitFor(bytes: number): [number, string] {
  if (bytes >= TB) {
    return [TB, 'ТБ'];
  }
  if (bytes >= GB) {
    return [GB, 'ГБ'];
  }
  return [MB, 'МБ'];
}

export function sizeText(bytes: number): string {
  const [unit, word] = unitFor(bytes);
  return `${decimal(bytes / unit, bytes / unit >= 10 ? 0 : 1)}${NBSP}${word}`;
}

function pairText(used: number, total: number): string {
  const [unit, word] = unitFor(total);
  const digits = total / unit >= 10 ? 0 : 1;
  return `${decimal(used / unit, digits)}${NBSP}из${NBSP}${decimal(total / unit, digits)}${NBSP}${word}`;
}

export function uptimeText(seconds: number): string {
  const days = Math.floor(seconds / 86_400);
  if (days >= 1) {
    return count(days, 'день', 'дня', 'дней');
  }
  const hours = Math.floor(seconds / 3600);
  if (hours >= 1) {
    return count(hours, 'час', 'часа', 'часов');
  }
  return count(Math.max(1, Math.floor(seconds / 60)), 'минута', 'минуты', 'минут');
}

function tone(share: number): string {
  if (share >= 0.9) {
    return 'bad';
  }
  if (share >= 0.75) {
    return 'warn';
  }
  return '';
}

type Row = { label: string; value: string; share: number; note?: string };

export function serverRows(load: ServerLoad): Row[] {
  const rows: Row[] = [];
  if (load.cpu_percent !== null) {
    const notes = [load.cores ? count(load.cores, 'ядро', 'ядра', 'ядер') : null, load.load ? `нагрузка ${decimal(load.load[0], 2)}` : null].filter(Boolean);
    rows.push({ label: 'Процессор', value: `${Math.round(load.cpu_percent)}%`, share: load.cpu_percent / 100, note: notes.join(' · ') || undefined });
  }
  if (load.memory) {
    rows.push({ label: 'Память', value: pairText(load.memory.used_bytes, load.memory.total_bytes), share: load.memory.used_bytes / load.memory.total_bytes });
  }
  if (load.disk) {
    rows.push({ label: 'Диск', value: pairText(load.disk.used_bytes, load.disk.total_bytes), share: load.disk.used_bytes / load.disk.total_bytes });
  }
  return rows;
}

function footText(load: ServerLoad): string | null {
  const parts = [
    load.database_bytes !== null ? `база ${sizeText(load.database_bytes)}` : null,
    load.app_memory_bytes !== null ? `панель занимает ${sizeText(load.app_memory_bytes)} памяти` : null,
  ].filter(Boolean);
  if (!parts.length) {
    return null;
  }
  const text = parts.join(' · ');
  return text.charAt(0).toUpperCase() + text.slice(1);
}

export function PanelServer({ load, variant }: { load: ServerLoad | null | undefined; variant: 'side' | 'day' | 'phone' }) {
  if (!load) {
    return null;
  }
  const rows = serverRows(load);
  const foot = footText(load);
  if (!rows.length && !foot) {
    return null;
  }
  const uptime = load.uptime_seconds !== null ? `работает ${uptimeText(load.uptime_seconds)}` : null;
  const meters = (
    <>
      <div className="hm-meters">
        {rows.map((row) => (
          <div key={row.label} className={`hm-meter ${tone(row.share)}`}>
            <span className="hm-meter-head">
              <span>{row.label}</span>
              <span className="num">{row.value}</span>
            </span>
            <span className="hm-meter-bar" aria-hidden="true">
              <span style={{ width: `${Math.max(2, Math.min(100, row.share * 100))}%` }} />
            </span>
            {row.note ? <span className="hm-small muted">{row.note}</span> : null}
          </div>
        ))}
      </div>
      {foot ? <p className="hm-small muted hm-panel-foot">{foot}</p> : null}
    </>
  );
  if (variant === 'phone') {
    return (
      <section aria-labelledby="hp-panel" className="hp-group">
        <h2 id="hp-panel" className="hp-h2">
          Сервер панели
        </h2>
        <div className="hp-card hm-panel phone">
          {uptime ? <span className="hm-small muted">{uptime.charAt(0).toUpperCase() + uptime.slice(1)}</span> : null}
          {meters}
        </div>
      </section>
    );
  }
  return (
    <section aria-labelledby={`panel-h-${variant}`} className={`hm-sec hm-panel ${variant}`}>
      <div className="hm-feedhead">
        <h2 id={`panel-h-${variant}`} className={variant === 'day' ? 'hm-h2b' : 'hm-h2'}>
          Сервер панели
        </h2>
        {uptime ? <span className="hm-small muted">{uptime}</span> : null}
      </div>
      {meters}
    </section>
  );
}
