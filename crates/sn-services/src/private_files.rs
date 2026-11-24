use std::fs::File;
use std::io;
use std::path::Path;

pub fn private_dir(dir: &Path) -> io::Result<()> {
    let mut builder = std::fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        let meta = std::fs::metadata(dir)?;
        if !meta.is_dir() {
            return Err(io::Error::other(format!("{} — не папка", dir.display())));
        }
        if meta.uid() != rustix::process::geteuid().as_raw() {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                format!(
                    "папка {} принадлежит другому пользователю системы: удалите её или укажите другую папку",
                    dir.display()
                ),
            ));
        }
        if meta.mode() & 0o077 != 0 {
            std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
        }
    }
    Ok(())
}

pub fn private_file(path: &Path) -> io::Result<File> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path)
}

pub fn fresh_private_file(path: &Path) -> io::Result<File> {
    match std::fs::remove_file(path) {
        Err(err) if err.kind() != io::ErrorKind::NotFound => return Err(err),
        _ => {}
    }
    private_file(path)
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::io::Write;
    use std::os::unix::fs::PermissionsExt;

    fn mode(path: &Path) -> u32 {
        std::fs::metadata(path).unwrap().permissions().mode() & 0o777
    }

    #[test]
    fn временные_папки_и_файлы_панели_закрыты_от_других_пользователей() {
        let root =
            std::env::temp_dir().join(format!("sn-private-{}", uuid::Uuid::new_v4().simple()));
        let nested = root.join("a").join("b");
        private_dir(&nested).unwrap();
        assert_eq!(mode(&nested), 0o700);
        std::fs::set_permissions(&nested, std::fs::Permissions::from_mode(0o755)).unwrap();
        private_dir(&nested).unwrap();
        assert_eq!(mode(&nested), 0o700);
        let file = nested.join("x.upload");
        private_file(&file).unwrap().write_all(b"1").unwrap();
        assert_eq!(mode(&file), 0o600);
        assert_eq!(
            private_file(&file).unwrap_err().kind(),
            io::ErrorKind::AlreadyExists
        );
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o644)).unwrap();
        fresh_private_file(&file).unwrap().write_all(b"2").unwrap();
        assert_eq!(mode(&file), 0o600);
        assert_eq!(std::fs::read(&file).unwrap(), b"2");
        let plain = root.join("plain");
        std::fs::write(&plain, b"").unwrap();
        assert!(private_dir(&plain).is_err());
        std::fs::remove_dir_all(&root).unwrap();
    }
}
