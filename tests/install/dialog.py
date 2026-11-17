import os
import pty
import re
import select
import shutil
import sys
import tempfile
import time

HERE = os.path.dirname(os.path.abspath(__file__))
SRC = os.path.abspath(os.path.join(HERE, '..', '..'))
TMP_ROOT = tempfile.mkdtemp(prefix='sn-install-dialog.')
ANSI = re.compile(r'\x1b\[[0-9;]*m')
PROMPTS = ('Домен:', 'Почта:', '[д/Н]')


def fresh():
    work = tempfile.mkdtemp(prefix='work.', dir=TMP_ROOT)
    os.makedirs(work + '/repo/deploy')
    os.makedirs(work + '/state')
    os.makedirs(work + '/bin')
    shutil.copy(SRC + '/install.sh', work + '/repo/')
    for name in ['.env.example', 'docker-compose.yml', 'compose.remnawave.yml']:
        shutil.copy(SRC + '/deploy/' + name, work + '/repo/deploy/')
    return work


def drive(shell, answers, typed_early='', typed_on=None, extra_env=None):
    work = fresh()
    env = dict(os.environ, FAKE_STATE=work + '/state', SN_BIN_DIR=work + '/bin', SN_TEST_BUSY_PORTS='', PATH=HERE + '/bin:' + os.environ['PATH'], TERM='xterm')
    for key in ['SN_OWNER_EMAIL', 'SN_DOMAIN', 'NO_COLOR', 'SN_BUILD']:
        env.pop(key, None)
    env.update(extra_env or {})
    pid, fd = pty.fork()
    if pid == 0:
        os.execvpe(shell, [shell, work + '/repo/install.sh'], env)
    if typed_early:
        os.write(fd, typed_early.encode())
    pending = dict(typed_on or {})
    out = b''
    seen = 0
    queue = list(answers)
    deadline = time.time() + 40
    while time.time() < deadline:
        ready, _, _ = select.select([fd], [], [], 0.5)
        if fd not in ready:
            continue
        try:
            chunk = os.read(fd, 4096)
        except OSError:
            break
        if not chunk:
            break
        out += chunk
        plain = ANSI.sub('', out.decode('utf-8', 'replace'))
        for trigger in list(pending):
            if trigger in plain:
                os.write(fd, pending.pop(trigger).encode())
        tail = plain[seen:].rstrip()
        if queue and tail.endswith(PROMPTS):
            os.write(fd, (queue.pop(0) + '\n').encode())
            seen = len(plain)
    finished, status = 0, 0
    for _ in range(50):
        finished, status = os.waitpid(pid, os.WNOHANG)
        if finished != 0:
            break
        time.sleep(0.1)
    text = ANSI.sub('', out.decode('utf-8', 'replace')).replace('\r', '')
    if finished == 0:
        os.kill(pid, 9)
        os.waitpid(pid, 0)
        return work, text + '\n<<зависло>>', -9
    return work, text, os.waitstatus_to_exitcode(status)


def read(path):
    return open(path, encoding='utf-8').read() if os.path.exists(path) else ''


def report(title, checks, text, failed):
    print('=== ' + title)
    bad = False
    for name, good in checks:
        print('  ПРОВЕРКА:' if good else '  ОШИБКА:', name)
        bad = bad or not good
    if bad:
        print(re.sub(r'(Пароль владельца:\s+)\S+', r'\1<скрыт>', text))
    return failed or bad


failed = False
shells = sys.argv[1:] or ['sh']
for shell in shells:
    work, text, code = drive(shell, ['bad domain', 'http://panel.test/x', 'nope', 'boss@example.com'])
    env = read(work + '/repo/deploy/.env')
    password = read(work + '/state/password.txt')
    shown = re.search(r'Пароль владельца:\s+(\S+)', text)
    failed = report(f'{shell}: неверные ответы переспрашиваются', [
        ('код выхода 0', code == 0),
        ('неверный домен: подсказка и повтор вопроса', 'Не похоже на домен' in text and text.count('Домен:') == 2),
        ('домен очищен до panel.test', 'SN_PUBLIC_URL=https://panel.test\n' in env),
        ('неверная почта: подсказка и повтор вопроса', 'Не похоже на почту' in text and text.count('Почта:') == 2),
        ('владелец создан с введённой почтой', read(work + '/state/owner.txt') == 'boss@example.com'),
        ('показанный пароль совпадает с переданным', bool(shown) and shown.group(1) == password and len(password) == 20),
    ], text, failed)

    work, text, code = drive(
        shell,
        ['primetap.test', 'admin@test.ru'],
        typed_early='\n',
        typed_on={'[fake] building image': 'primetap.pr'},
        extra_env={'FAKE_UP_SLEEP': '2'},
    )
    env = read(work + '/repo/deploy/.env')
    failed = report(f'{shell}: случайный Enter и набранное заранее не портят ответы', [
        ('код выхода 0', code == 0),
        ('случайный Enter не пропустил вопрос про домен', 'SN_PUBLIC_URL=https://primetap.test\n' in env),
        ('вопрос про домен задан один раз', text.count('Домен:') == 1),
        ('набранное во время сборки не попало в почту', read(work + '/state/owner.txt') == 'admin@test.ru'),
        ('в итоге показана правильная почта', re.search(r'Почта владельца:\s+admin@test\.ru\n', text) is not None),
        ('в итоге показан адрес панели', 'Адрес панели:      https://primetap.test' in text),
    ], text, failed)

    work, text, code = drive(shell, ['', '', '', 'д', 'owner@example.com'])
    env = read(work + '/repo/deploy/.env')
    failed = report(f'{shell}: домен пропускается только после подтверждения', [
        ('код выхода 0', code == 0),
        ('пустой ответ вызывает вопрос о подтверждении', text.count('Продолжить без него?') == 2),
        ('после отказа домен спрашивается снова', text.count('Домен:') == 2),
        ('после подтверждения домен пуст', 'SN_PUBLIC_URL=\n' in env),
        ('в итоге сказано, как указать домен', 'Адрес панели:      не задан' in text and '/bin/stealthnet domain' in text),
        ('владелец создан', read(work + '/state/owner.txt') == 'owner@example.com'),
    ], text, failed)

print('ИТОГ диалога:', 'есть ошибки' if failed else 'все проверки прошли')
sys.exit(1 if failed else 0)
