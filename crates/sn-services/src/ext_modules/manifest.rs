use super::address;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use sn_domain::access::Permission;
use sn_domain::module::ModuleId;

pub const FORMAT: u64 = 1;
pub const MAX_BYTES: usize = 64 * 1024;
const ID_MIN: usize = 2;
const ID_MAX: usize = 40;
const NAME_MAX: usize = 60;
const VERSION_MAX: usize = 30;
const AUTHOR_MAX: usize = 80;
const DESCRIPTION_MAX: usize = 300;
const PAGE_TITLE_MAX: usize = 40;
const FIELDS_MAX: usize = 30;
const FIELD_KEY_MAX: usize = 40;
const LABEL_MAX: usize = 80;
const HINT_MAX: usize = 300;
const OPTIONS_MAX: usize = 50;
pub const TEXT_MAX: usize = 300;
pub const LARGE_TEXT_MAX: usize = 4000;
const RESERVED_IDS: [&str; 3] = ["me", "install", "ext"];
const TOP_FIELDS: [&str; 12] = [
    "format",
    "id",
    "name",
    "version",
    "author",
    "description",
    "site",
    "webhook",
    "events",
    "permissions",
    "page",
    "settings",
];
const PAGE_FIELDS: [&str; 3] = ["url", "title", "permission"];
const SETTING_FIELDS: [&str; 9] = [
    "key", "type", "label", "hint", "required", "default", "large", "options", "labels",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EventInfo {
    pub kind: &'static str,
    pub title: &'static str,
    pub about: &'static str,
    pub needs: Permission,
}

const fn event(
    kind: &'static str,
    title: &'static str,
    about: &'static str,
    needs: Permission,
) -> EventInfo {
    EventInfo {
        kind,
        title,
        about,
        needs,
    }
}

pub const EVENTS: [EventInfo; 13] = [
    event(
        "client_registered",
        "Клиент зарегистрировался",
        "номер клиента, откуда он пришёл и параметр ссылки",
        Permission::ClientsRead,
    ),
    event(
        "client_blocked",
        "Клиент заблокирован",
        "номер клиента и причина",
        Permission::ClientsRead,
    ),
    event(
        "client_unblocked",
        "Клиент разблокирован",
        "номер клиента",
        Permission::ClientsRead,
    ),
    event(
        "payment_paid",
        "Оплата прошла",
        "номер оплаты и заказа, что куплено, сумма и платёжная система",
        Permission::Money,
    ),
    event(
        "payment_refunded",
        "Оплата возвращена",
        "номер оплаты и заказа, сумма возврата",
        Permission::Money,
    ),
    event(
        "order_fulfilled",
        "Покупка выдана",
        "номер заказа, что куплено и номер подписки",
        Permission::ClientsRead,
    ),
    event(
        "subscription_created",
        "Подписка выдана",
        "номер подписки и клиента, вид подписки",
        Permission::ClientsRead,
    ),
    event(
        "subscription_renewed",
        "Подписка продлена",
        "номер подписки и новая дата окончания",
        Permission::ClientsRead,
    ),
    event(
        "tariff_changed",
        "Тариф сменился",
        "номер подписки, прежний и новый тариф",
        Permission::ClientsRead,
    ),
    event(
        "subscription_status_changed",
        "Статус подписки изменился",
        "активна, ограничена, истекла или отключена",
        Permission::ClientsRead,
    ),
    event(
        "balance_changed",
        "Баланс изменился",
        "номер клиента, кошелёк, причина и сумма",
        Permission::Money,
    ),
    event(
        "ticket_opened",
        "Новое обращение в поддержку",
        "номер обращения и клиента",
        Permission::Support,
    ),
    event(
        "ticket_answered",
        "Поддержка ответила клиенту",
        "номер обращения и клиента",
        Permission::Support,
    ),
];

pub fn event_info(kind: &str) -> Option<&'static EventInfo> {
    EVENTS.iter().find(|e| e.kind == kind)
}

pub const fn permission_title(permission: Permission) -> &'static str {
    match permission {
        Permission::ClientsRead => "Клиенты: просмотр",
        Permission::ClientsWrite => "Клиенты: изменения",
        Permission::Money => "Деньги",
        Permission::Refunds => "Возвраты",
        Permission::Tariffs => "Тарифы",
        Permission::Infrastructure => "Инфраструктура",
        Permission::Support => "Поддержка",
        Permission::Marketing => "Маркетинг",
        Permission::Modules => "Модули",
        Permission::Settings => "Настройки",
        Permission::Admins => "Администраторы",
        Permission::ClientsLogin => "Вход за клиента",
        Permission::RemnaSync => "Синхронизация с Remnawave",
    }
}

pub const fn permission_about(permission: Permission) -> &'static str {
    match permission {
        Permission::ClientsRead => "видеть клиентов, их подписки и историю",
        Permission::ClientsWrite => {
            "менять подписки и баланс клиентов, писать им, блокировать и удалять"
        }
        Permission::Money => "видеть оплаты, заказы и выручку, разбирать спорные оплаты",
        Permission::Refunds => "возвращать клиентам деньги",
        Permission::Tariffs => "менять тарифы и цены",
        Permission::Infrastructure => "управлять нодами, сквадами и хостами Remnawave",
        Permission::Support => "читать обращения и отвечать клиентам",
        Permission::Marketing => "промокоды, рассылки, рефералы, конкурсы",
        Permission::Modules => "включать и настраивать встроенные модули",
        Permission::Settings => {
            "менять настройки панели, в том числе ключи платёжных систем и бота"
        }
        Permission::Admins => "управлять администраторами",
        Permission::ClientsLogin => {
            "выдавать клиенту пароль для сайта и снимать ему вход по коду из приложения"
        }
        Permission::RemnaSync => "переносить клиентов и подписки между панелью и Remnawave",
    }
}

pub const fn sensitive(permission: Permission) -> bool {
    !matches!(
        permission,
        Permission::ClientsRead | Permission::Support | Permission::Marketing
    )
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Page {
    pub url: String,
    pub title: String,
    pub permission: Permission,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum FieldKind {
    Text,
    Secret,
    Number,
    Flag,
    Url,
    Select { options: Vec<String> },
}

impl FieldKind {
    pub const fn name(&self) -> &'static str {
        match self {
            FieldKind::Text => "text",
            FieldKind::Secret => "secret",
            FieldKind::Number => "number",
            FieldKind::Flag => "flag",
            FieldKind::Url => "url",
            FieldKind::Select { .. } => "select",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Field {
    pub key: String,
    pub label: String,
    pub hint: String,
    pub kind: FieldKind,
    pub required: bool,
    pub default: Option<String>,
    pub large: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub labels: Option<Vec<String>>,
}

impl Field {
    pub const fn is_secret(&self) -> bool {
        matches!(self.kind, FieldKind::Secret)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Manifest {
    pub format: u64,
    pub id: String,
    pub name: String,
    pub version: String,
    pub author: String,
    pub description: Option<String>,
    pub site: Option<String>,
    pub webhook: Option<String>,
    pub events: Vec<String>,
    pub permissions: Vec<Permission>,
    pub page: Option<Page>,
    pub settings: Vec<Field>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Problem {
    pub path: String,
    pub message: String,
}

#[derive(Default)]
struct Problems(Vec<Problem>);

impl Problems {
    fn add(&mut self, path: impl Into<String>, message: impl Into<String>) {
        self.0.push(Problem {
            path: path.into(),
            message: message.into(),
        });
    }
}

fn chars(value: &str) -> usize {
    value.chars().count()
}

fn clean(value: &str) -> bool {
    !value.chars().any(|c| c.is_control() && c != '\n')
}

fn text(
    map: &Map<String, Value>,
    key: &str,
    path: &str,
    max: usize,
    required: bool,
    problems: &mut Problems,
) -> Option<String> {
    match map.get(key) {
        None | Some(Value::Null) => {
            if required {
                problems.add(path, "поле обязательно");
            }
            None
        }
        Some(Value::String(raw)) => {
            let value = raw.trim();
            if value.is_empty() {
                if required {
                    problems.add(path, "поле не может быть пустым");
                }
                return None;
            }
            if chars(value) > max {
                problems.add(path, format!("не длиннее {max} символов"));
                return None;
            }
            if !clean(value) {
                problems.add(path, "в тексте есть служебные символы");
                return None;
            }
            Some(value.to_string())
        }
        Some(_) => {
            problems.add(path, "ожидается строка в кавычках");
            None
        }
    }
}

fn flag(map: &Map<String, Value>, key: &str, path: &str, problems: &mut Problems) -> bool {
    match map.get(key) {
        None | Some(Value::Null) => false,
        Some(Value::Bool(value)) => *value,
        Some(_) => {
            problems.add(path, "ожидается true или false");
            false
        }
    }
}

fn unknown(map: &Map<String, Value>, known: &[&str], prefix: &str, problems: &mut Problems) {
    for key in map.keys() {
        if !known.contains(&key.as_str()) {
            let path = if prefix.is_empty() {
                key.clone()
            } else {
                format!("{prefix}.{key}")
            };
            problems.add(path, format!("неизвестное поле «{key}»"));
        }
    }
}

pub fn id_problem(id: &str) -> Option<String> {
    let length = id.len();
    let shape = id.starts_with(|c: char| c.is_ascii_lowercase())
        && id
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        && (ID_MIN..=ID_MAX).contains(&length);
    if !shape {
        return Some(format!(
            "от {ID_MIN} до {ID_MAX} символов: латинские буквы в нижнем регистре, цифры и дефис, первая — буква"
        ));
    }
    if ModuleId::parse(id).is_ok() {
        return Some(format!("«{id}» — имя встроенного модуля, выберите другое"));
    }
    if RESERVED_IDS.contains(&id) {
        return Some(format!("«{id}» занято панелью, выберите другое"));
    }
    None
}

fn format_version(map: &Map<String, Value>, problems: &mut Problems) -> u64 {
    match map.get("format") {
        None | Some(Value::Null) => {
            problems.add("format", "укажите версию формата: \"format\": 1");
            0
        }
        Some(Value::Number(n)) => match n.as_u64() {
            Some(FORMAT) => FORMAT,
            Some(other) => {
                problems.add(
                    "format",
                    format!(
                        "версия формата {other} этой панели неизвестна, поддерживается только 1"
                    ),
                );
                other
            }
            None => {
                problems.add("format", "ожидается число 1");
                0
            }
        },
        Some(_) => {
            problems.add("format", "ожидается число 1");
            0
        }
    }
}

fn permission(raw: &str) -> Result<Permission, String> {
    match Permission::parse(raw) {
        Ok(Permission::Admins) => Err("право «Администраторы» модулю дать нельзя".into()),
        Ok(Permission::ClientsLogin) => Err("право «Вход за клиента» модулю дать нельзя".into()),
        Ok(Permission::RemnaSync) => {
            Err("право «Синхронизация с Remnawave» модулю дать нельзя".into())
        }
        Ok(permission) => Ok(permission),
        Err(_) => Err(format!(
            "неизвестное право «{raw}». Есть: {}",
            Permission::ALL
                .iter()
                .filter(|p| {
                    !matches!(
                        p,
                        Permission::Admins | Permission::ClientsLogin | Permission::RemnaSync
                    )
                })
                .map(|p| p.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        )),
    }
}

fn string_list(
    map: &Map<String, Value>,
    key: &str,
    problems: &mut Problems,
) -> Vec<(String, String)> {
    match map.get(key) {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::Array(items)) => items
            .iter()
            .enumerate()
            .filter_map(|(i, item)| match item {
                Value::String(s) => Some((format!("{key}[{i}]"), s.trim().to_string())),
                _ => {
                    problems.add(format!("{key}[{i}]"), "ожидается строка в кавычках");
                    None
                }
            })
            .collect(),
        Some(_) => {
            problems.add(key, "ожидается список в квадратных скобках [ … ]");
            Vec::new()
        }
    }
}

fn permissions(map: &Map<String, Value>, problems: &mut Problems) -> Vec<Permission> {
    let mut out: Vec<Permission> = Vec::new();
    for (path, raw) in string_list(map, "permissions", problems) {
        match permission(&raw) {
            Ok(p) if !out.contains(&p) => out.push(p),
            Ok(_) => {}
            Err(message) => problems.add(path, message),
        }
    }
    out.sort();
    out
}

fn events(
    map: &Map<String, Value>,
    granted: &[Permission],
    problems: &mut Problems,
) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for (path, raw) in string_list(map, "events", problems) {
        let Some(info) = event_info(&raw) else {
            problems.add(
                path,
                format!(
                    "неизвестное событие «{raw}». Есть: {}",
                    EVENTS.iter().map(|e| e.kind).collect::<Vec<_>>().join(", ")
                ),
            );
            continue;
        };
        if !granted.contains(&info.needs) {
            problems.add(
                path,
                format!(
                    "событие «{}» приходит только с правом «{}»: добавьте \"{}\" в permissions",
                    info.title,
                    permission_title(info.needs),
                    info.needs.as_str()
                ),
            );
        }
        if !out.iter().any(|e| e == info.kind) {
            out.push(info.kind.to_string());
        }
    }
    out.sort_by_key(|kind| EVENTS.iter().position(|e| e.kind == kind));
    out
}

fn url(
    map: &Map<String, Value>,
    key: &str,
    path: &str,
    required: bool,
    problems: &mut Problems,
) -> Option<String> {
    let raw = text(map, key, path, address::URL_MAX, required, problems)?;
    match address::check(&raw) {
        Ok(url) => Some(url.to_string()),
        Err(message) => {
            problems.add(path, message);
            None
        }
    }
}

fn page(map: &Map<String, Value>, name: &str, problems: &mut Problems) -> Option<Page> {
    let value = map.get("page")?;
    if value.is_null() {
        return None;
    }
    let Value::Object(page) = value else {
        problems.add(
            "page",
            "ожидается объект { \"url\": …, \"title\": … } в фигурных скобках",
        );
        return None;
    };
    unknown(page, &PAGE_FIELDS, "page", problems);
    let url = url(page, "url", "page.url", true, problems);
    let title = text(page, "title", "page.title", PAGE_TITLE_MAX, false, problems)
        .unwrap_or_else(|| name.chars().take(PAGE_TITLE_MAX).collect());
    let permission = match page.get("permission") {
        None | Some(Value::Null) => Some(Permission::Modules),
        Some(Value::String(raw)) => match self::permission(raw.trim()) {
            Ok(p) => Some(p),
            Err(message) => {
                problems.add("page.permission", message);
                None
            }
        },
        Some(_) => {
            problems.add("page.permission", "ожидается строка в кавычках");
            None
        }
    };
    Some(Page {
        url: url?,
        title,
        permission: permission?,
    })
}

fn default_value(
    field: &Map<String, Value>,
    kind: &FieldKind,
    path: &str,
    problems: &mut Problems,
) -> Option<String> {
    let value = field.get("default")?;
    let path = format!("{path}.default");
    let rendered = match (kind, value) {
        (_, Value::Null) => return None,
        (FieldKind::Secret, _) => {
            problems.add(path, "у секретного поля не бывает значения по умолчанию");
            return None;
        }
        (FieldKind::Flag, Value::Bool(b)) => b.to_string(),
        (FieldKind::Flag, _) => {
            problems.add(path, "ожидается true или false");
            return None;
        }
        (FieldKind::Number, Value::Number(n)) => n.to_string(),
        (FieldKind::Number, Value::String(s)) if number(s).is_some() => s.trim().to_string(),
        (FieldKind::Number, _) => {
            problems.add(path, "ожидается число");
            return None;
        }
        (_, Value::String(s)) => s.trim().to_string(),
        _ => {
            problems.add(path, "ожидается строка в кавычках");
            return None;
        }
    };
    match kind {
        FieldKind::Select { options } if !options.contains(&rendered) => {
            problems.add(path, "значение по умолчанию должно быть из options");
            None
        }
        FieldKind::Url if !rendered.is_empty() && !address::setting_url(&rendered) => {
            problems.add(path, "ожидается адрес https://…");
            None
        }
        FieldKind::Text if chars(&rendered) > LARGE_TEXT_MAX => {
            problems.add(path, format!("не длиннее {LARGE_TEXT_MAX} символов"));
            None
        }
        _ => Some(rendered),
    }
}

pub fn number(raw: &str) -> Option<serde_json::Number> {
    let raw = raw.trim();
    if let Ok(whole) = raw.parse::<i64>() {
        return Some(whole.into());
    }
    raw.parse::<f64>()
        .ok()
        .filter(|n| n.is_finite())
        .and_then(serde_json::Number::from_f64)
}

fn field(item: &Value, path: &str, problems: &mut Problems) -> Option<Field> {
    let Value::Object(field) = item else {
        problems.add(path, "ожидается объект { \"key\": …, \"type\": … }");
        return None;
    };
    unknown(field, &SETTING_FIELDS, path, problems);
    let key = text(
        field,
        "key",
        &format!("{path}.key"),
        FIELD_KEY_MAX,
        true,
        problems,
    );
    let key = key.filter(|key| {
        let ok = key.starts_with(|c: char| c.is_ascii_lowercase())
            && key
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_');
        if !ok {
            problems.add(
                format!("{path}.key"),
                "латинские буквы в нижнем регистре, цифры и _, первая — буква",
            );
        }
        ok
    });
    let label = text(
        field,
        "label",
        &format!("{path}.label"),
        LABEL_MAX,
        true,
        problems,
    );
    let hint = text(
        field,
        "hint",
        &format!("{path}.hint"),
        HINT_MAX,
        false,
        problems,
    )
    .unwrap_or_default();
    let required = flag(field, "required", &format!("{path}.required"), problems);
    let large = flag(field, "large", &format!("{path}.large"), problems);
    let kind = match field.get("type") {
        Some(Value::String(t)) => match t.trim() {
            "text" => Some(FieldKind::Text),
            "secret" => Some(FieldKind::Secret),
            "number" => Some(FieldKind::Number),
            "flag" => Some(FieldKind::Flag),
            "url" => Some(FieldKind::Url),
            "select" => {
                let options = string_list(field, "options", problems);
                if options.is_empty() || options.len() > OPTIONS_MAX {
                    problems.add(
                        format!("{path}.options"),
                        format!("у списка выбора от 1 до {OPTIONS_MAX} вариантов"),
                    );
                }
                let mut values: Vec<String> = Vec::new();
                for (_, option) in options {
                    if option.is_empty() || chars(&option) > LABEL_MAX || !clean(&option) {
                        problems.add(
                            format!("{path}.options"),
                            format!("вариант — непустая строка до {LABEL_MAX} символов"),
                        );
                    } else if values.contains(&option) {
                        problems.add(
                            format!("{path}.options"),
                            format!("вариант «{option}» повторяется"),
                        );
                    } else {
                        values.push(option);
                    }
                }
                Some(FieldKind::Select { options: values })
            }
            other => {
                problems.add(
                    format!("{path}.type"),
                    format!(
                        "неизвестный тип «{other}». Есть: text, secret, number, flag, url, select"
                    ),
                );
                None
            }
        },
        None | Some(Value::Null) => {
            problems.add(format!("{path}.type"), "поле обязательно");
            None
        }
        Some(_) => {
            problems.add(format!("{path}.type"), "ожидается строка в кавычках");
            None
        }
    };
    let kind = kind?;
    if large && kind != FieldKind::Text {
        problems.add(
            format!("{path}.large"),
            "большое поле бывает только у type: text",
        );
    }
    let labels = match (&kind, field.get("labels")) {
        (_, None | Some(Value::Null)) => None,
        (FieldKind::Select { options }, Some(_)) => {
            let labels: Vec<String> = string_list(field, "labels", problems)
                .into_iter()
                .map(|(_, l)| l)
                .collect();
            if labels.len() != options.len()
                || labels
                    .iter()
                    .any(|l| l.is_empty() || chars(l) > LABEL_MAX || !clean(l))
            {
                problems.add(
                    format!("{path}.labels"),
                    "подписей должно быть столько же, сколько вариантов, каждая до 80 символов",
                );
                None
            } else {
                Some(labels)
            }
        }
        (_, Some(_)) => {
            problems.add(
                format!("{path}.labels"),
                "подписи вариантов бывают только у type: select",
            );
            None
        }
    };
    let default = default_value(field, &kind, path, problems);
    Some(Field {
        key: key?,
        label: label?,
        hint,
        required: required && !matches!(kind, FieldKind::Flag),
        large: large && kind == FieldKind::Text,
        kind,
        default,
        labels,
    })
}

fn settings(map: &Map<String, Value>, problems: &mut Problems) -> Vec<Field> {
    let items = match map.get("settings") {
        None | Some(Value::Null) => return Vec::new(),
        Some(Value::Array(items)) => items,
        Some(_) => {
            problems.add(
                "settings",
                "ожидается список полей в квадратных скобках [ … ]",
            );
            return Vec::new();
        }
    };
    if items.len() > FIELDS_MAX {
        problems.add("settings", format!("не больше {FIELDS_MAX} полей"));
        return Vec::new();
    }
    let mut out: Vec<Field> = Vec::new();
    for (i, item) in items.iter().enumerate() {
        let path = format!("settings[{i}]");
        if let Some(field) = field(item, &path, problems) {
            if out.iter().any(|f| f.key == field.key) {
                problems.add(
                    format!("{path}.key"),
                    format!("поле «{}» уже есть", field.key),
                );
            } else {
                out.push(field);
            }
        }
    }
    out
}

pub fn parse(source: &str) -> Result<Manifest, Vec<Problem>> {
    let mut problems = Problems::default();
    let source = source.trim_start_matches('\u{feff}');
    if source.trim().is_empty() {
        problems.add("", "файл пустой");
        return Err(problems.0);
    }
    if source.len() > MAX_BYTES {
        problems.add("", "файл больше 64 КБ — это не похоже на описание модуля");
        return Err(problems.0);
    }
    let value: Value = match serde_json::from_str(source) {
        Ok(value) => value,
        Err(err) => {
            problems.add(
                "",
                format!(
                    "это не JSON: ошибка в строке {}, столбце {}",
                    err.line(),
                    err.column()
                ),
            );
            return Err(problems.0);
        }
    };
    let Value::Object(map) = value else {
        problems.add("", "описание модуля — это объект в фигурных скобках { … }");
        return Err(problems.0);
    };
    unknown(&map, &TOP_FIELDS, "", &mut problems);
    let format = format_version(&map, &mut problems);
    if format != FORMAT {
        return Err(problems.0);
    }
    let id =
        text(&map, "id", "id", ID_MAX + 10, true, &mut problems).filter(|id| {
            match id_problem(id) {
                Some(message) => {
                    problems.add("id", message);
                    false
                }
                None => true,
            }
        });
    let name = text(&map, "name", "name", NAME_MAX, true, &mut problems);
    let version = text(&map, "version", "version", VERSION_MAX, true, &mut problems);
    let author = text(&map, "author", "author", AUTHOR_MAX, true, &mut problems);
    let description = text(
        &map,
        "description",
        "description",
        DESCRIPTION_MAX,
        false,
        &mut problems,
    );
    let site = url(&map, "site", "site", false, &mut problems);
    let permissions = permissions(&map, &mut problems);
    let events = events(&map, &permissions, &mut problems);
    let webhook = url(&map, "webhook", "webhook", false, &mut problems);
    if webhook.is_none() && !events.is_empty() && !map.contains_key("webhook") {
        problems.add(
            "webhook",
            "укажите адрес, куда слать события: модуль подписан на события",
        );
    }
    let page = page(&map, name.as_deref().unwrap_or_default(), &mut problems);
    let settings = settings(&map, &mut problems);
    if !problems.0.is_empty() {
        return Err(problems.0);
    }
    match (id, name, version, author) {
        (Some(id), Some(name), Some(version), Some(author)) => Ok(Manifest {
            format,
            id,
            name,
            version,
            author,
            description,
            site,
            webhook,
            events,
            permissions,
            page,
            settings,
        }),
        _ => {
            problems.add("", "описание модуля не разобрано");
            Err(problems.0)
        }
    }
}
