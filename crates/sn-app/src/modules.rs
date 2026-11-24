use sn_services::ext_modules::package;
use std::path::Path;
use std::process::ExitCode;

pub fn keygen(file: &str) -> ExitCode {
    match package::keygen(Path::new(file)) {
        Ok((public, fingerprint)) => {
            println!("Ключ подписи записан: {file}");
            println!("Открытый ключ: {public}");
            println!("Отпечаток:     {fingerprint}");
            println!("Сохраните файл ключа в менеджере паролей: без него архивы не подписать.");
            ExitCode::SUCCESS
        }
        Err(err) => {
            eprintln!("ошибка: {err}");
            ExitCode::FAILURE
        }
    }
}

pub fn pack(dir: &str, key: Option<&str>, out: Option<&str>) -> ExitCode {
    match package::pack(Path::new(dir), key.map(Path::new), out.map(Path::new)) {
        Ok(packed) => {
            println!("Архив модуля: {}", packed.path.display());
            println!("Размер:       {} байт", packed.bytes);
            println!(
                "Процессоры:   {}",
                packed
                    .platforms
                    .iter()
                    .map(|p| package::processor(p))
                    .collect::<Vec<_>>()
                    .join(", ")
            );
            match packed.fingerprint {
                Some(fingerprint) => println!("Подпись:      ключ {fingerprint}"),
                None => println!("Подпись:      нет, архив ставится только с галочкой доверия"),
            }
            ExitCode::SUCCESS
        }
        Err(err) => {
            eprintln!("ошибка: {err}");
            ExitCode::FAILURE
        }
    }
}

pub fn unpack(package_path: &str, out: &str, platform: Option<&str>) -> ExitCode {
    let out_dir = Path::new(out);
    let platform = match platform {
        Some(platform) => Some(platform),
        None => package::host_platform(),
    };
    let result = match platform {
        Some(platform) => package::unpack(Path::new(package_path), out_dir, platform),
        None => Err("процессор этого сервера не поддерживается".to_string()),
    };
    match result {
        Ok(spec) => {
            println!("{}", spec.image);
            ExitCode::SUCCESS
        }
        Err(err) => {
            let text: String = err
                .chars()
                .map(|c| if c.is_control() { ' ' } else { c })
                .take(300)
                .collect();
            let encoded = serde_json::to_string(&text).unwrap_or_else(|_| "\"\"".into());
            let _ = std::fs::write(out_dir.join("error.json"), format!("{encoded}\n"));
            eprintln!("ошибка: {text}");
            ExitCode::FAILURE
        }
    }
}
