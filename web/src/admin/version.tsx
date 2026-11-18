import { Link } from 'react-router';
import { dateNumeric, dayMonthNumeric } from '../lib/format';

export type BuildSummary = {
  version: string;
  label: string | null;
  commit: string | null;
  built_at: string | null;
  update: { label: string | null; commit: string; built_at: string } | null;
};

export const UPDATES_PATH = '/admin/settings/updates';
export const BACKUPS_PATH = '/admin/settings/backups';

export function shortCommit(commit: string | null | undefined): string {
  return commit ? commit.slice(0, 7) : '';
}

export function buildName(label: string | null | undefined, commit: string | null | undefined, builtAt?: string | null): string {
  if (label) {
    return label;
  }
  if (builtAt) {
    return `сборка от ${dateNumeric(builtAt)}`;
  }
  return shortCommit(commit) || 'сборка без номера';
}

export function shortName(label: string | null | undefined, commit: string | null | undefined, builtAt?: string | null): string {
  if (label) {
    return label;
  }
  if (builtAt) {
    return dateNumeric(builtAt);
  }
  return shortCommit(commit) || '—';
}

export function buildText(build: BuildSummary): string {
  const date = build.built_at ? dateNumeric(build.built_at) : '';
  if (build.label) {
    return date ? `${build.label} от ${date}` : build.label;
  }
  return date ? `сборка от ${date}` : 'собрана вручную';
}

export function upgradeText(build: BuildSummary): string | null {
  if (!build.update) {
    return null;
  }
  return `${shortName(build.label, build.commit, build.built_at)} → ${shortName(build.update.label, build.update.commit, build.update.built_at)}`;
}

type LineProps = {
  build: BuildSummary | null;
  prefix: string;
  className: string;
  canOpen: boolean;
  onOpen?: () => void;
};

function withPrefix(prefix: string, build: BuildSummary): string {
  return build.label ? `${prefix} ${buildText(build)}` : `${prefix}: ${buildText(build)}`;
}

export function VersionLine({ build, prefix, className, canOpen, onOpen }: LineProps) {
  if (!build) {
    return null;
  }
  const upgrade = upgradeText(build);
  if (upgrade && canOpen) {
    return (
      <Link to={UPDATES_PATH} className={`${className} ver-new`} aria-label={`${prefix} ${upgrade}: вышла новая версия, открыть обновления`} onClick={onOpen}>
        {prefix} {upgrade}
      </Link>
    );
  }
  return <span className={className}>{withPrefix(prefix, build)}</span>;
}

export function versionLine(prefix: string, build: BuildSummary): string {
  return withPrefix(prefix, build);
}

function compactName(label: string | null | undefined, commit: string | null | undefined, builtAt?: string | null): string {
  if (label) {
    return label;
  }
  if (builtAt) {
    return dayMonthNumeric(builtAt);
  }
  return shortCommit(commit) || '—';
}

function railNote(build: BuildSummary, next: string | null): string {
  if (next) {
    return `→ ${next}`;
  }
  if (!build.label) {
    return build.built_at ? 'сборка без номера' : 'собрана вручную';
  }
  return build.built_at ? `от ${dateNumeric(build.built_at)}` : '';
}

type ChipProps = {
  build: BuildSummary | null;
  canOpen: boolean;
  variant: 'top' | 'rail';
};

export function BuildChip({ build, canOpen, variant }: ChipProps) {
  if (!build) {
    return null;
  }
  const fresh = build.update;
  const current = compactName(build.label, build.commit, build.built_at);
  const next = fresh ? compactName(fresh.label, fresh.commit, fresh.built_at) : null;
  const title = fresh
    ? `STEALTHNET ${buildText(build)}. Вышла версия ${buildName(fresh.label, fresh.commit, fresh.built_at)}`
    : `STEALTHNET ${buildText(build)}${build.commit ? ` · ${shortCommit(build.commit)}` : ''}`;
  const base = variant === 'top' ? 'top-build' : 'rail-build';
  const className = `${base}${fresh ? ' ver-new' : ''}${canOpen ? ' hov' : ''}`;
  const content =
    variant === 'top' ? (
      <>
        {fresh ? <span className="build-dot" aria-hidden="true" /> : null}
        <span>
          {build.label || fresh ? <span className="top-build-word">версия </span> : null}
          {fresh ? `${current} → ${next}` : build.label ? current : buildText(build)}
        </span>
      </>
    ) : (
      <>
        <span className="rail-build-short">
          <span className="rail-build-num">
            {current}
            {fresh ? <span className="build-dot" aria-hidden="true" /> : null}
          </span>
        </span>
        <span className="lbl">{railNote(build, next)}</span>
      </>
    );
  if (canOpen) {
    return (
      <Link to={UPDATES_PATH} className={className} title={title} aria-label={`${title}. Открыть обновления`}>
        {content}
      </Link>
    );
  }
  return (
    <span className={className} title={title}>
      {content}
    </span>
  );
}
