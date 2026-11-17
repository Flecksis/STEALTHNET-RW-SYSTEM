use sn_domain::module::ModuleId;
use sn_store::StoreError;

#[derive(Debug, thiserror::Error)]
pub enum ModuleError {
    #[error("модуль «{0}» выключен")]
    Disabled(ModuleId),
    #[error("модуль «{module}» нельзя включить: {reason}")]
    Unavailable { module: ModuleId, reason: String },
    #[error("модуль «{0}» зарегистрирован дважды")]
    Duplicate(ModuleId),
    #[error("неверные настройки модуля: {0}")]
    Invalid(String),
    #[error("не найдено: {0}")]
    NotFound(&'static str),
    #[error("ядро отказало: {0}")]
    Core(String),
    #[error("функция модуля ещё не реализована: {0}")]
    NotImplemented(&'static str),
    #[error(transparent)]
    Store(#[from] StoreError),
}
