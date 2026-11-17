use sn_domain::DomainError;

pub type StoreResult<T> = Result<T, StoreError>;

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("не найдено: {0}")]
    NotFound(&'static str),
    #[error("конфликт: {message}")]
    Conflict {
        constraint: Option<String>,
        message: String,
    },
    #[error("на балансе не хватает средств")]
    InsufficientFunds,
    #[error("недопустимое значение: {0}")]
    Invalid(String),
    #[error("данные в базе не разобраны: {0}")]
    Decode(String),
    #[error("ошибка шифрования: {0}")]
    Crypto(String),
    #[error("ошибка миграции: {0}")]
    Migrate(String),
    #[error("метод хранилища ещё не реализован: {0}")]
    NotImplemented(&'static str),
    #[error(transparent)]
    Db(sqlx::Error),
}

impl StoreError {
    pub fn constraint(&self) -> Option<&str> {
        match self {
            StoreError::Conflict { constraint, .. } => constraint.as_deref(),
            _ => None,
        }
    }

    pub fn field(&self) -> Option<&'static str> {
        self.constraint().and_then(crate::constraints::field_for)
    }

    pub const fn is_not_found(&self) -> bool {
        matches!(self, StoreError::NotFound(_))
    }

    pub const fn is_conflict(&self) -> bool {
        matches!(self, StoreError::Conflict { .. })
    }
}

impl From<sqlx::Error> for StoreError {
    fn from(err: sqlx::Error) -> Self {
        match &err {
            sqlx::Error::RowNotFound => StoreError::NotFound("запись"),
            sqlx::Error::Database(db) if db.is_unique_violation() => StoreError::Conflict {
                constraint: db.constraint().map(str::to_string),
                message: match db.constraint().and_then(crate::constraints::field_for) {
                    Some(field) => format!("значение поля «{field}» уже занято"),
                    None => "запись с таким значением уже существует".into(),
                },
            },
            sqlx::Error::Database(db) if db.is_foreign_key_violation() => StoreError::Conflict {
                constraint: db.constraint().map(str::to_string),
                message: "связанная запись отсутствует или используется".into(),
            },
            sqlx::Error::Database(db) if db.is_check_violation() => StoreError::Invalid(format!(
                "нарушено ограничение {}",
                db.constraint().unwrap_or("?")
            )),
            sqlx::Error::Database(db)
                if matches!(
                    db.code().as_deref(),
                    Some("22001" | "22003" | "22P02" | "23502")
                ) =>
            {
                StoreError::Invalid("проверьте формат и диапазон значений".into())
            }
            sqlx::Error::ColumnDecode { .. } | sqlx::Error::Decode(_) => {
                StoreError::Decode(err.to_string())
            }
            _ => StoreError::Db(err),
        }
    }
}

impl From<DomainError> for StoreError {
    fn from(err: DomainError) -> Self {
        StoreError::Invalid(err.to_string())
    }
}

impl From<serde_json::Error> for StoreError {
    fn from(err: serde_json::Error) -> Self {
        StoreError::Decode(err.to_string())
    }
}

impl From<sqlx::migrate::MigrateError> for StoreError {
    fn from(err: sqlx::migrate::MigrateError) -> Self {
        StoreError::Migrate(err.to_string())
    }
}
