use anyhow::{Result, bail};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    Serve,
    Migrate,
    CreateAdmin { email: String },
    ImportLegacy { dry_run: bool, file: Option<String> },
    GenSecretKey,
    Version,
    Help,
}

pub const USAGE: &str = "stealthnet — панель продажи VPN поверх Remnawave

Команды:
  serve                         запустить API, фоновые задачи и бота (по умолчанию)
  migrate                       применить миграции базы и выйти
  create-admin --email <почта>  создать владельца или сбросить ему пароль;
                                пароль читается из SN_ADMIN_PASSWORD или первой строки stdin
  import-legacy [--dry-run] [файл]
                                перенести клиентов из STEALTHNET 7.x или бота Бедолаги:
                                файл резервной копии старой панели или бота;
                                без файла — база STEALTHNET 7.x из LEGACY_DATABASE_URL;
                                пароль zip-архива — из LEGACY_ARCHIVE_PASSWORD
  gen-secret-key                напечатать новый ключ для SN_SECRET_KEY
  version                       показать версию
  help                          эта справка";

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
        "create-admin" => {
            let mut email = None;
            while let Some(arg) = rest.next() {
                match arg.as_str() {
                    "--email" => email = rest.next().cloned(),
                    other => match other.strip_prefix("--email=") {
                        Some(value) => email = Some(value.to_string()),
                        None => bail!("неизвестный аргумент: {other}"),
                    },
                }
            }
            match email.filter(|e| e.contains('@')) {
                Some(email) => Command::CreateAdmin { email },
                None => bail!("укажите --email <почта>"),
            }
        }
        other => bail!("неизвестная команда: {other}\n\n{USAGE}"),
    };
    if let Some(extra) = rest.next() {
        bail!("лишний аргумент: {extra}");
    }
    Ok(command)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        std::iter::once("stealthnet")
            .chain(list.iter().copied())
            .map(String::from)
            .collect()
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
}
