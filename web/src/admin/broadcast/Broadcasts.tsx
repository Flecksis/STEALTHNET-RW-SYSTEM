import { useCallback, useEffect, useRef, useState } from 'react';
import { Link, Route, Routes, useNavigate, useParams } from 'react-router';
import { api, ApiError } from '../../lib/api';
import { Icon } from '../../lib/icons';
import { usePhone } from '../../lib/media';
import { PhoneHeader } from '../phone';
import { AutoList } from './AutoList';
import { RuleDetail } from './AutoRule';
import { Composer } from './Composer';
import { BotWarning, List } from './List';
import { BASE, ROOT, failText, type Broadcast, type Options } from './model';
import { Report } from './Report';
import '../form.css';
import './broadcast.css';

export function Broadcasts() {
  return (
    <Routes>
      <Route index element={<List />} />
      <Route path="auto" element={<AutoList />} />
      <Route path="auto/:id" element={<RuleDetail />} />
      <Route path=":id" element={<Detail />} />
    </Routes>
  );
}

type Failure = { text: string; disabled: boolean };

function failure(err: unknown, fallback: string): Failure {
  return {
    text: failText(err, fallback),
    disabled: err instanceof ApiError && err.code === 'module.disabled',
  };
}

function Detail() {
  const { id = 'new' } = useParams();
  const navigate = useNavigate();
  const phone = usePhone();
  const [options, setOptions] = useState<Options | null>(null);
  const [current, setCurrent] = useState<Broadcast | null>(null);
  const [version, setVersion] = useState(0);
  const [loading, setLoading] = useState(id !== 'new');
  const [error, setError] = useState<Failure | null>(null);
  const latest = useRef<Broadcast | null>(null);
  latest.current = current;

  useEffect(() => {
    api<Options>(`${BASE}/options`)
      .then(setOptions)
      .catch((err) => setError(failure(err, 'Раздел не загрузился')));
  }, []);

  useEffect(() => {
    if (id === 'new') {
      setCurrent(null);
      setVersion((v) => v + 1);
      setLoading(false);
      return;
    }
    if (latest.current && String(latest.current.id) === id) {
      setLoading(false);
      return;
    }
    let alive = true;
    setLoading(true);
    api<Broadcast>(`${BASE}/broadcasts/${encodeURIComponent(id)}`)
      .then((broadcast) => {
        if (alive) {
          setCurrent(broadcast);
          setVersion((v) => v + 1);
          setError(null);
        }
      })
      .catch((err) => {
        if (alive) {
          setError(failure(err, 'Рассылка не загрузилась'));
        }
      })
      .finally(() => {
        if (alive) {
          setLoading(false);
        }
      });
    return () => {
      alive = false;
    };
  }, [id]);

  const saved = useCallback(
    (broadcast: Broadcast) => {
      setCurrent(broadcast);
      if (String(broadcast.id) !== id) {
        navigate(`${ROOT}/${broadcast.id}`, { replace: true });
      }
    },
    [id, navigate],
  );

  const copied = useCallback(
    (broadcast: Broadcast) => {
      setCurrent(broadcast);
      setVersion((v) => v + 1);
      navigate(`${ROOT}/${broadcast.id}`);
    },
    [navigate],
  );

  const title = current ? `Рассылка №${current.id}` : id === 'new' ? 'Новая рассылка' : 'Рассылка';

  let body;
  if (error) {
    body = (
      <section className="card card-pad" role="alert">
        <p className="err" style={{ fontSize: 15 }}>
          {error.text}
        </p>
        {error.disabled ? (
          <p className="muted">
            Включить модуль можно в разделе <Link to="/admin/modules/broadcast">«Модули»</Link>.
          </p>
        ) : null}
      </section>
    );
  } else if (!options || loading) {
    body = <p className="muted">Загружаем…</p>;
  } else if (!current || current.status === 'draft') {
    body = <Composer key={version} options={options} initial={current} onSaved={saved} onDeleted={() => navigate(ROOT)} />;
  } else {
    body = <Report broadcast={current} onChange={setCurrent} onCopy={copied} />;
  }

  if (phone) {
    return (
      <div className="hp bc-phone">
        <PhoneHeader title={title} back={{ to: ROOT, label: 'Все рассылки' }} />
        <BotWarning options={options} />
        {body}
      </div>
    );
  }

  return (
    <>
      <Link to={ROOT} className="bc-back hov">
        <Icon name="chevron-left" size={18} />
        Все рассылки
      </Link>
      <h1 className="page-title tight">{title}</h1>
      <BotWarning options={options} />
      {body}
    </>
  );
}
