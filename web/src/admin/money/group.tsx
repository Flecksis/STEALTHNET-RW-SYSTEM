import type { ReactNode } from 'react';

export type Tone = 'ok' | 'warn';

export type Outcome = { text: string; tone: Tone };

type FrameProps = {
  id: string;
  title: string;
  total: number;
  lead: string;
  outcome: Outcome | null;
  error: string | null;
  children: ReactNode;
};

export function GroupFrame({ id, title, total, lead, outcome, error, children }: FrameProps) {
  return (
    <section id={id} className="mo-group" aria-labelledby={`${id}-h`}>
      <div className="mo-sub">
        <h3 id={`${id}-h`} className="mo-h3">
          {title}
          {total > 0 ? ` · ${total}` : ''}
        </h3>
        <p className="muted">{lead}</p>
      </div>
      {outcome ? (
        <p className={outcome.tone === 'ok' ? 'mo-done' : 'mo-done warn'} role="status">
          {outcome.text}
        </p>
      ) : null}
      {error ? (
        <p className="err mo-pad" role="alert">
          {error}
        </p>
      ) : null}
      {children}
    </section>
  );
}
