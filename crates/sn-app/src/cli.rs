use anyhow::{Result, anyhow, bail};
use sn_services::auth::LoginUnlock;
use std::net::IpAddr;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    Serve,
    Migrate,
    CreateAdmin {
        email: String,
    },
    UnlockLogin {
        email: String,
        addresses: Vec<IpAddr>,
    },
    ImportLegacy {
        dry_run: bool,
        file: Option<String>,
    },
    Remna {
        pull: bool,
        dry_run: bool,
        fix: bool,
    },
    Backup,
    Restore {
        file: String,
        check: bool,
        yes: bool,
        password_stdin: bool,
    },
    GenSecretKey,
    ModuleKeygen {
        file: String,
    },
    ModulePack {
        dir: String,
        key: Option<String>,
        out: Option<String>,
    },
    ModuleUnpack {
        package: String,
        out: String,
        platform: Option<String>,
    },
    Version,
    Help,
}

pub const USAGE: &str = "stealthnet — панель продажи VPN поверх Remnawave

Команды:
  serve                         запустить API, фоновые задачи и бота (по умолчанию)
  migrate                       применить миграции базы и выйти
  create-admin --email <почта>  создать владельца или сбросить ему пароль;
                                пароль читается из SN_ADMIN_PASSWORD или первой строки stdin
  unlock-login --email <почта> [--ip <адрес>]
                                снять блокировку входа администратора после подбора пароля
                                или неверных кодов из приложения; с --ip ваш адрес 30 дней
                                считается привычным и вход с него не закроется, даже пока
                                подбор продолжается; --ip можно повторить
  import-legacy [--dry-run] [файл]
                                перенести клиентов из STEALTHNET 7.x или бота Бедолаги:
                                файл резервной копии старой панели или бота;
                                без файла — база STEALTHNET 7.x из LEGACY_DATABASE_URL;
                                пароль zip-архива — из LEGACY_ARCHIVE_PASSWORD
  remna push [--dry-run] [--fix]
                                перенести клиентов и подписки панели в подключённую
                                Remnawave: создать недостающих пользователей с прежней
                                ссылкой и ключами; с --fix ещё и поправить расхождения
                                (срок только продлевается); ничего не удаляет
  remna pull [--dry-run]        забрать из Remnawave пользователей, которых нет в панели:
                                найти клиента по Telegram ID, почте или имени, иначе
                                создать клиента и подписку; тариф подбирается по сквадам
                                и лимитам; --dry-run только показывает, что будет сделано
  backup                        сделать резервную копию сейчас: база, подписки и снимок
                                Remnawave; пароль копий и отправка — из настроек админки
  restore [--check] [--yes] [--password-stdin] <файл>
                                восстановить базу из резервной копии: сначала проверяет
                                файл и сохраняет копию текущей базы, копию старой версии
                                обновляет до текущей; --check — только проверить файл;
                                пароль зашифрованной копии — из SN_BACKUP_PASSWORD или
                                первой строки stdin с --password-stdin
  gen-secret-key                напечатать новый ключ для SN_SECRET_KEY
  module keygen <файл>          создать ключ подписи архивов модулей; файл не перезаписывается
  module pack <папка> [--key <файл>] [--out <архив.zip>]
                                собрать архив модуля из папки с module.json и образами
                                image-linux-amd64.tar и image-linux-arm64.tar (docker save),
                                проверить его и подписать ключом из module keygen
  version                       показать версию
  help                          эта справка";

fn option<'a>(
    arg: &str,
    name: &str,
    rest: &mut impl Iterator<Item = &'a String>,
) -> Result<Option<String>> {
    if arg == name {
        return rest
            .next()
            .cloned()
            .map(Some)
            .ok_or_else(|| anyhow!("после {name} нужно значение"));
    }
    Ok(arg
        .strip_prefix(name)
        .and_then(|tail| tail.strip_prefix('='))
        .map(str::to_string))
}

fn email(value: Option<String>) -> Result<String> {
    match value.filter(|e| e.contains('@')) {
        Some(email) => Ok(email),
        None => bail!("укажите --email <почта>"),
    }
}

fn address(value: &str) -> Result<IpAddr> {
    value.trim().parse().map_err(|_| {
        anyhow!("«{value}» не похоже на IP-адрес: нужен вид 203.0.113.7 или 2001:db8::7")
    })
}

pub fn parse(args: &[String]) -> Result<Command> {
    let mut rest = args.iter().skip(1);
    let Some(command) = rest.next() else {
        return Ok(Command::Serve);
    };
    let command = match command.as_str() {
        "serve" => Command::Serve,
        "migrate" => Command::Migrate,
        "gen-secret-key" => Command::GenSecretKey,
        "version" | "--version" | "-V" => Command::Version,
        "help" | "--help" | "-h" => Command::Help,
        "backup" => Command::Backup,
        "restore" => {
            let mut file = None;
            let (mut check, mut yes, mut password_stdin) = (false, false, false);
            for arg in rest.by_ref() {
                match arg.as_str() {
                    "--check" => check = true,
                    "--yes" | "-y" => yes = true,
                    "--password-stdin" => password_stdin = true,
                    other if other.starts_with('-') => bail!("неизвестный аргумент: {other}"),
                    other if file.is_none() => file = Some(other.to_string()),
                    other => bail!("лишний аргумент: {other}"),
                }
            }
            Command::Restore {
                file: file
                    .ok_or_else(|| anyhow!("укажите файл копии: stealthnet restore <файл>"))?,
                check,
                yes,
                password_stdin,
            }
        }
        "import-legacy" => {
            let mut dry_run = false;
            let mut file = None;
            for arg in rest.by_ref() {
                match arg.as_str() {
                    "--dry-run" => dry_run = true,
                    other if other.starts_with('-') => bail!("неизвестный аргумент: {other}"),
                    other if file.is_none() => file = Some(other.to_string()),
                    other => bail!("лишний аргумент: {other}"),
                }
            }
            Command::ImportLegacy { dry_run, file }
        }
        "remna" => {
            let pull = match rest.next().map(String::as_str) {
                Some("push") => false,
                Some("pull") => true,
                Some(other) => bail!("после remna нужно push или pull, а не {other}"),
                None => bail!("после remna нужно push или pull"),
            };
            let mut dry_run = false;
            let mut fix = false;
            for arg in rest.by_ref() {
                match arg.as_str() {
                    "--dry-run" => dry_run = true,
                    "--fix" if !pull => fix = true,
                    other => bail!("неизвестный аргумент: {other}"),
                }
            }
            Command::Remna { pull, dry_run, fix }
        }
        "create-admin" => {
            let mut found = None;
            while let Some(arg) = rest.next() {
                match option(arg, "--email", &mut rest)? {
                    Some(value) => found = Some(value),
                    None => bail!("неизвестный аргумент: {arg}"),
                }
            }
            Command::CreateAdmin {
                email: email(found)?,
            }
        }
        "unlock-login" => {
            let mut found = None;
            let mut addresses = Vec::new();
            while let Some(arg) = rest.next() {
                if let Some(value) = option(arg, "--email", &mut rest)? {
                    found = Some(value);
                } else if let Some(value) = option(arg, "--ip", &mut rest)? {
                    addresses.push(address(&value)?);
                } else {
                    bail!("неизвестный аргумент: {arg}");
                }
            }
            Command::UnlockLogin {
                email: email(found)?,
                addresses,
            }
        }
        "module" => match rest.next().map(String::as_str) {
            Some("keygen") => {
                let file = rest.next().cloned().ok_or_else(|| {
                    anyhow!("укажите файл ключа: stealthnet module keygen <файл>")
                })?;
                if let Some(extra) = rest.next() {
                    bail!("лишний аргумент: {extra}");
                }
                Command::ModuleKeygen { file }
            }
            Some("pack") => {
                let mut dir = None;
                let mut key = None;
                let mut out = None;
                while let Some(arg) = rest.next() {
                    if let Some(value) = option(arg, "--key", &mut rest)? {
                        key = Some(value);
                    } else if let Some(value) = option(arg, "--out", &mut rest)? {
                        out = Some(value);
                    } else if arg.starts_with('-') {
                        bail!("неизвестный аргумент: {arg}");
                    } else if dir.is_none() {
                        dir = Some(arg.clone());
                    } else {
                        bail!("лишний аргумент: {arg}");
                    }
                }
                Command::ModulePack {
                    dir: dir.ok_or_else(|| {
                        anyhow!("укажите папку модуля: stealthnet module pack <папка>")
                    })?,
                    key,
                    out,
                }
            }
            Some(other) => bail!("после module нужно keygen или pack, а не {other}"),
            None => bail!("после module нужно keygen или pack"),
        },
        "module-unpack" => {
            let mut paths = Vec::new();
            let mut platform = None;
            while let Some(arg) = rest.next() {
                if let Some(value) = option(arg, "--platform", &mut rest)? {
                    platform = Some(value);
                } else if arg.starts_with('-') {
                    bail!("неизвестный аргумент: {arg}");
                } else {
                    paths.push(arg.clone());
                }
            }
            let [package, out]: [String; 2] = paths
                .try_into()
                .map_err(|_| anyhow!("нужно: module-unpack <архив> <папка>"))?;
            Command::ModuleUnpack {
                package,
                out,
                platform,
            }
        }
        other => bail!("неизвестная команда: {other}\n\n{USAGE}"),
    };
    if let Some(extra) = rest.next() {
        bail!("лишний аргумент: {extra}");
    }
    Ok(command)
}

pub fn unlock_report(unlock: &LoginUnlock) -> Vec<String> {
    let mut lines = Vec::new();
    if unlock.password_locked {
        lines.push(format!(
            "Вход для {} с новых адресов был закрыт из-за подбора пароля. Блокировка снята.",
            unlock.email
        ));
    }
    if unlock.totp_locked {
        lines.push(
            "Вход по коду из приложения был закрыт после неверных кодов. Блокировка снята. \
             Коды вводили уже после верного пароля: смените пароль."
                .to_string(),
        );
    }
    if !unlock.password_locked && !unlock.totp_locked {
        lines.push(format!(
            "Вход для {} не был закрыт. Счётчики неверных попыток обнулены.",
            unlock.email
        ));
    }
    for address in &unlock.addresses {
        lines.push(format!(
            "Адрес {address} теперь привычный на 30 дней: подбор с других адресов не закроет вход с него."
        ));
    }
    if unlock.addresses.is_empty() {
        lines.push(
            "Если подбор продолжается, вход с новых адресов снова закроется. \
             Чтобы этого не случилось, повторите команду с --ip <ваш IP-адрес>."
                .to_string(),
        );
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;
    use sn_domain::ids::AdminId;

    fn args(list: &[&str]) -> Vec<String> {
        std::iter::once("stealthnet")
            .chain(list.iter().copied())
            .map(String::from)
            .collect()
    }

    #[test]
    fn команды_синхронизации_с_remnawave_разбираются() {
        assert_eq!(
            parse(&args(&["remna", "push", "--dry-run"])).unwrap(),
            Command::Remna {
                pull: false,
                dry_run: true,
                fix: false
            }
        );
        assert_eq!(
            parse(&args(&["remna", "push", "--fix"])).unwrap(),
            Command::Remna {
                pull: false,
                dry_run: false,
                fix: true
            }
        );
        assert_eq!(
            parse(&args(&["remna", "pull"])).unwrap(),
            Command::Remna {
                pull: true,
                dry_run: false,
                fix: false
            }
        );
        assert!(parse(&args(&["remna", "pull", "--fix"])).is_err());
        assert!(parse(&args(&["remna"])).is_err());
        assert!(parse(&args(&["remna", "sideways"])).is_err());
    }

    #[test]
    fn команды_разбираются() {
        assert_eq!(parse(&args(&[])).unwrap(), Command::Serve);
        assert_eq!(parse(&args(&["migrate"])).unwrap(), Command::Migrate);
        assert_eq!(
            parse(&args(&["create-admin", "--email", "owner@example.com"])).unwrap(),
            Command::CreateAdmin {
                email: "owner@example.com".into()
            }
        );
        assert_eq!(
            parse(&args(&["create-admin", "--email=owner@example.com"])).unwrap(),
            Command::CreateAdmin {
                email: "owner@example.com".into()
            }
        );
        assert!(parse(&args(&["create-admin"])).is_err());
        assert!(parse(&args(&["create-admin", "--email"])).is_err());
        assert!(parse(&args(&["create-admin", "--emailx=owner@example.com"])).is_err());
        assert_eq!(
            parse(&args(&["import-legacy", "--dry-run"])).unwrap(),
            Command::ImportLegacy {
                dry_run: true,
                file: None
            }
        );
        assert_eq!(
            parse(&args(&["import-legacy", "backup.tar.gz"])).unwrap(),
            Command::ImportLegacy {
                dry_run: false,
                file: Some("backup.tar.gz".into())
            }
        );
        assert!(parse(&args(&["import-legacy", "--from", "postgres://old"])).is_err());
        assert!(parse(&args(&["import-legacy", "a.sql", "b.sql"])).is_err());
        assert!(parse(&args(&["serve", "extra"])).is_err());
        assert!(parse(&args(&["deploy"])).is_err());
    }

    #[test]
    fn команды_копий_разбираются() {
        assert_eq!(parse(&args(&["backup"])).unwrap(), Command::Backup);
        assert_eq!(
            parse(&args(&["restore", "/backups/a.tar.gz"])).unwrap(),
            Command::Restore {
                file: "/backups/a.tar.gz".into(),
                check: false,
                yes: false,
                password_stdin: false,
            }
        );
        assert_eq!(
            parse(&args(&[
                "restore",
                "--check",
                "--password-stdin",
                "-y",
                "a.tar.gz.enc"
            ]))
            .unwrap(),
            Command::Restore {
                file: "a.tar.gz.enc".into(),
                check: true,
                yes: true,
                password_stdin: true,
            }
        );
        assert!(parse(&args(&["restore"])).is_err());
        assert!(parse(&args(&["restore", "a", "b"])).is_err());
        assert!(parse(&args(&["restore", "--force", "a"])).is_err());
        assert!(parse(&args(&["backup", "now"])).is_err());
        assert!(USAGE.contains("restore [--check] [--yes] [--password-stdin] <файл>"));
    }

    #[test]
    fn снятие_блокировки_входа_разбирается() {
        assert_eq!(
            parse(&args(&["unlock-login", "--email", "owner@example.com"])).unwrap(),
            Command::UnlockLogin {
                email: "owner@example.com".into(),
                addresses: Vec::new(),
            }
        );
        assert_eq!(
            parse(&args(&[
                "unlock-login",
                "--ip",
                "203.0.113.7",
                "--email=owner@example.com",
                "--ip=2001:db8::7",
            ]))
            .unwrap(),
            Command::UnlockLogin {
                email: "owner@example.com".into(),
                addresses: vec![
                    "203.0.113.7".parse().unwrap(),
                    "2001:db8::7".parse().unwrap()
                ],
            }
        );
        assert!(parse(&args(&["unlock-login"])).is_err());
        assert!(parse(&args(&["unlock-login", "--ip", "203.0.113.7"])).is_err());
        assert!(
            parse(&args(&[
                "unlock-login",
                "--email",
                "owner@example.com",
                "--ip"
            ]))
            .is_err()
        );
        assert_eq!(
            parse(&args(&[
                "unlock-login",
                "--email",
                "owner@example.com",
                "--ip",
                "203.0.113.0/24"
            ]))
            .unwrap_err()
            .to_string(),
            "«203.0.113.0/24» не похоже на IP-адрес: нужен вид 203.0.113.7 или 2001:db8::7"
        );
        assert!(
            parse(&args(&[
                "unlock-login",
                "--email",
                "owner@example.com",
                "--force"
            ]))
            .is_err()
        );
        assert!(USAGE.contains("unlock-login --email <почта> [--ip <адрес>]"));
    }

    #[test]
    fn отчёт_о_снятии_блокировки_говорит_что_было_и_что_делать() {
        let mut unlock = LoginUnlock {
            admin_id: AdminId(1),
            email: "owner@example.com".into(),
            password_failures: 20,
            password_locked: true,
            totp_locked: false,
            addresses: Vec::new(),
        };
        let lines = unlock_report(&unlock);
        assert_eq!(lines.len(), 2, "{lines:?}");
        assert!(lines[0].contains("owner@example.com с новых адресов был закрыт"));
        assert!(lines[1].contains("--ip <ваш IP-адрес>"));
        unlock.password_locked = false;
        unlock.totp_locked = true;
        unlock.addresses = vec!["203.0.113.7".into(), "2001:db8:1:2::/64".into()];
        let lines = unlock_report(&unlock);
        assert_eq!(lines.len(), 3, "{lines:?}");
        assert!(lines[0].contains("смените пароль"));
        assert!(lines[1].starts_with("Адрес 203.0.113.7 теперь привычный"));
        assert!(lines[2].starts_with("Адрес 2001:db8:1:2::/64 теперь привычный"));
        unlock.totp_locked = false;
        let lines = unlock_report(&unlock);
        assert!(lines[0].contains("не был закрыт"), "{lines:?}");
    }
}
