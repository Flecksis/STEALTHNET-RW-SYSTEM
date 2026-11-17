import { Link } from 'react-router';
import { dateNumeric } from '../lib/format';

export type BuildSummary = {
  version: string;
  commit: string | null;
  built_at: string | null;
  update: { commit: string; built_at: string } | null;
};

export const UPDATES_PATH = '/admin/settings/updates';

export function shortCommit(commit: string | null | undefined): string {
  return commit ? commit.slice(0, 7) : '';
}

export function buildText(build: BuildSummary): string {
  const parts = [build.version];
  if (build.commit) {
    parts.push(shortCommit(build.commit));
  }
  const text = parts.join(' · ');
  return build.built_at ? `${text} от ${dateNumeric(build.built_at)}` : text;
}

export function upgradeText(build: BuildSummary): string | null {
  if (!build.update) {
    return null;
  }
  return `${build.version} · ${shortCommit(build.commit) || 'сейчас'} → ${shortCommit(build.update.commit)}`;
}

type LineProps = {
  build: BuildSummary | null;
  prefix: string;
  className: string;
  canOpen: boolean;
  onOpen?: () => void;
};

export function VersionLine({ build, prefix, className, canOpen, onOpen }: LineProps) {
  if (!build) {
    return null;
  }
  const upgrade = upgradeText(build);
  if (upgrade && canOpen) {
    return (
      <Link to={UPDATES_PATH} className={`${className} ver-new`} aria-label={`${prefix} ${upgrade}: вышла новая сборка, открыть обновления`} onClick={onOpen}>
        {prefix} {upgrade}
      </Link>
    );
  }
  return (
    <span className={className}>
      {prefix} {buildText(build)}
    </span>
  );
}
