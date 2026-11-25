import { useCallback, useEffect, useLayoutEffect, useRef, useState, type CSSProperties } from 'react';
import { api, ApiError, fetchBlob, upload } from './api';
import { lang, t } from './i18n';
import { Icon } from './icons';

export type Attachment = {
  id: number;
  name: string;
  mime: string;
  size: number;
  kind: 'image' | 'file';
  width?: number;
  height?: number;
  removed?: boolean;
};

export type FileLimits = { max_bytes: number; max_files: number };

export const FILE_ACCEPT = '.jpg,.jpeg,.png,.webp,.gif,.pdf,.txt,.log';

const EXTENSIONS = new Set(['jpg', 'jpeg', 'png', 'webp', 'gif', 'pdf', 'txt', 'log']);
const TYPES = new Set(['image/jpeg', 'image/png', 'image/webp', 'image/gif', 'application/pdf', 'text/plain']);

export function fileSize(bytes: number): string {
  if (bytes < 1024 * 1024) {
    return t('{n} КБ', { n: Math.max(1, Math.round(bytes / 1024)) });
  }
  const mb = new Intl.NumberFormat(lang() === 'en' ? 'en-US' : 'ru-RU', { maximumFractionDigits: 1 }).format(bytes / (1024 * 1024));
  return t('{n} МБ', { n: mb });
}

function extension(name: string): string {
  const dot = name.lastIndexOf('.');
  return dot > 0 ? name.slice(dot + 1).toLowerCase() : '';
}

export function fileProblem(file: File, limits: FileLimits): string | null {
  const ext = extension(file.name);
  if (ext ? !EXTENSIONS.has(ext) : !TYPES.has(file.type)) {
    return t('«{name}» не подойдёт: можно фото JPG, PNG, WebP, GIF и файлы PDF, TXT, LOG', { name: file.name });
  }
  if (file.size === 0) {
    return t('«{name}» пустой', { name: file.name });
  }
  if (file.size > limits.max_bytes) {
    return t('«{name}» больше {size}', { name: file.name, size: fileSize(limits.max_bytes) });
  }
  return null;
}

type Draft = { key: number; name: string; size: number; state: 'uploading' | 'ready' | 'failed'; id: number | null; error: string | null };

export type Attachments = ReturnType<typeof useAttachments>;

export function useAttachments(base: string, limits: FileLimits | null) {
  const [drafts, setDrafts] = useState<Draft[]>([]);
  const [problem, setProblem] = useState<string | null>(null);
  const current = useRef<Draft[]>([]);
  const seq = useRef(0);

  const update = useCallback((change: (list: Draft[]) => Draft[]) => {
    current.current = change(current.current);
    setDrafts(current.current);
  }, []);

  const discard = useCallback((id: number) => {
    void api(`${base}/${id}`, { method: 'DELETE' }).catch(() => undefined);
  }, [base]);

  const add = useCallback(
    (list: FileList | File[] | null) => {
      const files = Array.from(list ?? []);
      if (!limits || files.length === 0) {
        return;
      }
      let room = limits.max_files - current.current.length;
      const notes: string[] = [];
      for (const file of files) {
        const issue = fileProblem(file, limits);
        if (issue) {
          notes.push(issue);
          continue;
        }
        if (room <= 0) {
          notes.push(t('В одном сообщении не больше {n} файлов', { n: limits.max_files }));
          break;
        }
        room -= 1;
        seq.current += 1;
        const key = seq.current;
        update((items) => [...items, { key, name: file.name, size: file.size, state: 'uploading', id: null, error: null }]);
        upload<Attachment>(`${base}?name=${encodeURIComponent(file.name)}`, file)
          .then((saved) => {
            if (!current.current.some((item) => item.key === key)) {
              discard(saved.id);
              return;
            }
            update((items) => items.map((item) => (item.key === key ? { ...item, state: 'ready', id: saved.id, name: saved.name, size: saved.size } : item)));
          })
          .catch((err: unknown) => {
            const error = err instanceof ApiError ? err.message : t('Файл не загрузился');
            update((items) => items.map((item) => (item.key === key ? { ...item, state: 'failed', error } : item)));
          });
      }
      setProblem(notes.length ? notes.join(' ') : null);
    },
    [base, limits, update, discard],
  );

  const remove = useCallback(
    (key: number) => {
      const draft = current.current.find((item) => item.key === key);
      if (draft?.id) {
        discard(draft.id);
      }
      update((items) => items.filter((item) => item.key !== key));
      setProblem(null);
    },
    [update, discard],
  );

  const reset = useCallback(() => {
    update(() => []);
    setProblem(null);
  }, [update]);

  const ids = drafts.flatMap((item) => (item.state === 'ready' && item.id !== null ? [item.id] : []));
  const uploading = drafts.some((item) => item.state === 'uploading');
  const failed = drafts.some((item) => item.state === 'failed');
  return {
    enabled: limits !== null,
    drafts,
    problem: problem ?? (failed ? t('Уберите файл, который не загрузился') : null),
    ids,
    uploading,
    ready: !uploading && !failed,
    full: limits ? drafts.length >= limits.max_files : true,
    add,
    remove,
    reset,
  };
}

export function AttachButton({ files, className, disabled, label }: { files: Attachments; className: string; disabled?: boolean; label?: string }) {
  const input = useRef<HTMLInputElement>(null);
  if (!files.enabled) {
    return null;
  }
  return (
    <>
      <input
        ref={input}
        type="file"
        multiple
        accept={FILE_ACCEPT}
        hidden
        onChange={(e) => {
          files.add(e.target.files);
          e.target.value = '';
        }}
      />
      <button
        type="button"
        className={className}
        onClick={() => input.current?.click()}
        disabled={disabled || files.full}
        aria-label={label ? undefined : t('Прикрепить файл')}
        title={label ? undefined : t('Прикрепить файл')}
      >
        <Icon name="paperclip" size={19} />
        {label ? <span>{label}</span> : null}
      </button>
    </>
  );
}

export function DraftList({ files, prefix, disabled }: { files: Attachments; prefix: string; disabled?: boolean }) {
  if (files.drafts.length === 0 && !files.problem) {
    return null;
  }
  return (
    <div className={`${prefix}-drafts`}>
      {files.drafts.length ? (
        <ul className={`${prefix}-draftlist`} aria-label={t('Прикреплённые файлы')}>
          {files.drafts.map((draft) => (
            <li key={draft.key} className={draft.state === 'failed' ? `${prefix}-draft bad` : `${prefix}-draft`}>
              <Icon name={/\.(jpe?g|png|webp|gif)$/i.test(draft.name) ? 'image' : 'file'} size={16} />
              <span className={`${prefix}-draft-name`}>{draft.name}</span>
              <span className={`${prefix}-draft-meta`}>{draft.state === 'uploading' ? t('загружаем…') : draft.state === 'failed' ? (draft.error ?? t('не загрузился')) : fileSize(draft.size)}</span>
              <button type="button" className={`${prefix}-draft-x`} onClick={() => files.remove(draft.key)} disabled={disabled} aria-label={t('Убрать {name}', { name: draft.name })}>
                <Icon name="close" size={15} />
              </button>
            </li>
          ))}
        </ul>
      ) : null}
      {files.problem ? (
        <p className={`${prefix}-files-note`} role="alert">
          {files.problem}
        </p>
      ) : null}
    </div>
  );
}

const blobs = new Map<string, Promise<string>>();

function blobUrl(path: string): Promise<string> {
  let found = blobs.get(path);
  if (!found) {
    found = fetchBlob(path).then((blob) => URL.createObjectURL(blob));
    found.catch(() => blobs.delete(path));
    blobs.set(path, found);
  }
  return found;
}

function useSource(path: string, viaFetch: boolean): string | null {
  const [src, setSrc] = useState<string | null>(viaFetch ? null : path);
  useEffect(() => {
    if (!viaFetch) {
      setSrc(path);
      return;
    }
    let alive = true;
    blobUrl(path)
      .then((url) => {
        if (alive) {
          setSrc(url);
        }
      })
      .catch(() => undefined);
    return () => {
      alive = false;
    };
  }, [path, viaFetch]);
  return src;
}

function Viewer({ src, name, prefix, zoom, onClose }: { src: string; name: string; prefix: string; zoom: boolean; onClose: () => void }) {
  const dialog = useRef<HTMLDialogElement>(null);
  const [spot, setSpot] = useState<{ x: number; y: number } | null>(null);
  useEffect(() => {
    dialog.current?.showModal();
  }, []);
  useLayoutEffect(() => {
    const box = dialog.current;
    const image = box?.querySelector('img');
    if (box && image && spot) {
      box.scrollTo(image.offsetLeft + image.offsetWidth * spot.x - box.clientWidth / 2, image.offsetTop + image.offsetHeight * spot.y - box.clientHeight / 2);
    }
  }, [spot]);
  const toggle = (x = 0.5, y = 0.5) => setSpot((now) => (now ? null : { x, y }));
  return (
    <dialog
      ref={dialog}
      className={spot ? `${prefix}-viewer big` : `${prefix}-viewer`}
      aria-label={name}
      onClose={onClose}
      onClick={(e) => {
        if (e.target === dialog.current) {
          dialog.current.close();
        }
      }}
    >
      <img
        src={src}
        alt={name}
        onClick={
          zoom
            ? (e) => {
                const box = e.currentTarget.getBoundingClientRect();
                toggle((e.clientX - box.left) / box.width, (e.clientY - box.top) / box.height);
              }
            : undefined
        }
      />
      <div className={`${prefix}-viewer-bar`}>
        <span>{name}</span>
        {zoom ? (
          <button type="button" onClick={() => toggle()}>
            {spot ? t('Уменьшить') : t('Увеличить')}
          </button>
        ) : null}
        <button type="button" onClick={() => dialog.current?.close()}>
          {t('Закрыть')}
        </button>
      </div>
    </dialog>
  );
}

function thumbSize(file: Attachment, box: [number, number]): CSSProperties | undefined {
  if (!file.width || !file.height) {
    return undefined;
  }
  const scale = Math.min(1, box[0] / file.width, box[1] / file.height);
  return { width: Math.max(24, Math.round(file.width * scale)), aspectRatio: `${file.width} / ${file.height}` };
}

function Thumb({ file, path, prefix, viaFetch, box }: { file: Attachment; path: string; prefix: string; viaFetch: boolean; box: [number, number] }) {
  const src = useSource(path, viaFetch);
  const [open, setOpen] = useState(false);
  const size = thumbSize(file, box);
  return (
    <>
      <button type="button" className={`${prefix}-thumb`} onClick={() => setOpen(true)} disabled={!src} aria-label={t('Открыть {name}', { name: file.name })}>
        {src ? <img src={src} alt={file.name} className={size ? undefined : 'free'} style={size} /> : <span className={`${prefix}-thumb-wait`} style={size} />}
      </button>
      {open && src ? <Viewer src={src} name={file.name} prefix={prefix} zoom={prefix === 'mn'} onClose={() => setOpen(false)} /> : null}
    </>
  );
}

type FilesProps = {
  files: Attachment[] | undefined;
  base: string;
  prefix: string;
  box?: [number, number];
  viaFetch?: boolean;
  onFile?: (file: Attachment) => void;
};

export function MessageFiles({ files, base, prefix, box = [260, 220], viaFetch = false, onFile }: FilesProps) {
  if (!files || files.length === 0) {
    return null;
  }
  return (
    <div className={`${prefix}-files`}>
      {files.map((file) => {
        if (file.removed) {
          return (
            <span key={file.id} className={`${prefix}-file removed`}>
              <Icon name="file" size={18} />
              <span className={`${prefix}-file-name`}>{file.name}</span>
              <span className={`${prefix}-file-meta`}>{t('удалён по сроку хранения')}</span>
            </span>
          );
        }
        if (file.kind === 'image') {
          return <Thumb key={file.id} file={file} path={`${base}/${file.id}`} prefix={prefix} viaFetch={viaFetch} box={box} />;
        }
        const body = (
          <>
            <Icon name="file" size={18} />
            <span className={`${prefix}-file-name`}>{file.name}</span>
            <span className={`${prefix}-file-meta`}>{fileSize(file.size)}</span>
          </>
        );
        return onFile ? (
          <button key={file.id} type="button" className={`${prefix}-file`} onClick={() => onFile(file)}>
            {body}
          </button>
        ) : (
          <a key={file.id} className={`${prefix}-file`} href={`${base}/${file.id}`} download={file.name}>
            {body}
          </a>
        );
      })}
    </div>
  );
}
