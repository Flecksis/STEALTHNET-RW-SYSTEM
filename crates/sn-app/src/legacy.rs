use anyhow::{Result, bail};
use std::collections::HashMap;

pub const LEGACY_URL: &str = "LEGACY_DATABASE_URL";

pub fn import(vars: &HashMap<String, String>, dry_run: bool) -> Result<()> {
    let Some(url) = vars
        .get(LEGACY_URL)
        .map(|v| v.trim())
        .filter(|v| !v.is_empty())
    else {
        bail!("задайте {LEGACY_URL} — строку подключения к базе STEALTHNET 7.x");
    };
    if !(url.starts_with("postgres://") || url.starts_with("postgresql://")) {
        bail!("{LEGACY_URL} должна начинаться с postgres://");
    }
    let mode = if dry_run {
        "проверка без записи"
    } else {
        "перенос"
    };
    bail!(
        "import-legacy ({mode}): перенос из STEALTHNET 7.x ещё не написан. \
         Порядок переноса описан в docs/ARCHITECTURE.md, раздел «Переход со старой версии 7.x»"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vars(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn перенос_без_адреса_старой_базы_не_запускается() {
        let err = import(&vars(&[]), false).unwrap_err().to_string();
        assert!(err.contains(LEGACY_URL));
        let err = import(&vars(&[(LEGACY_URL, "mysql://old")]), false)
            .unwrap_err()
            .to_string();
        assert!(err.contains("postgres://"));
        let err = import(&vars(&[(LEGACY_URL, "postgres://old/db")]), true)
            .unwrap_err()
            .to_string();
        assert!(err.contains("ещё не написан"));
    }
}
