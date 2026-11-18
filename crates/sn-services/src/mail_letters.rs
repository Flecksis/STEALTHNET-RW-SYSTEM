use crate::Actor;
use crate::error::{ServiceError, ServiceResult};
use crate::mail::{RichLetter, Undelivered};
use crate::services::Services;
use crate::settings::ModuleConfigUpdate;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use sn_domain::bot_content::escape;
use sn_domain::client::{IdentityKind, Locale, normalize_identity};
use sn_domain::ids::AdminId;
use sn_domain::image::ImageKind;
use sn_domain::lang;
use sn_domain::mail_letters::{
    self as catalog, BUTTON_MAX, CATALOG, Field, Group, LetterId, SIGN_MAX, SIGN_VARS, SUBJECT_MAX,
    Spec, TEXT_MAX, Values, Var,
};
use sn_domain::module::ModuleId;
use sn_domain::settings::{ClientDesign, CoreSettings};
use sn_modules::catalog::broadcast::{body_html, body_text};
use sn_modules::catalog::mail::{MailSettings, NAME_MAX};
use sn_store::Db;
use std::collections::BTreeMap;

#[cfg(test)]
mod tests;

pub const LETTER_PREFIX: &str = "mail.letter.";
pub const SIGN_PREFIX: &str = "mail.sign.";

const INK: &str = "#151a22";
const SOFT: &str = "#3e4652";
const MUTED: &str = "#626b78";
const LINE: &str = "#e5e8ed";
const PAPER: &str = "#f2f4f7";
const ACCENT: &str = "#ff4d79";
const ACCENT_INK: &str = "#101114";
const FONT: &str = "-apple-system,BlinkMacSystemFont,'Segoe UI',Roboto,Helvetica,Arial,sans-serif";
const MONO: &str = "ui-monospace,SFMono-Regular,Menlo,Consolas,monospace";
const CODE_OPEN: char = '\u{e000}';
const CODE_CLOSE: char = '\u{e001}';

pub fn letter_key(id: LetterId, locale: Locale) -> String {
    format!("{LETTER_PREFIX}{}.{}", id.as_str(), locale.as_str())
}

pub fn sign_key(locale: Locale) -> String {
    format!("{SIGN_PREFIX}{}", locale.as_str())
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Saved {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subject: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub button: Option<String>,
}

impl Saved {
    pub fn is_empty(&self) -> bool {
        self.subject.is_none() && self.text.is_none() && self.button.is_none()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Look {
    pub brand: String,
    pub base: Option<String>,
    pub logo: Option<String>,
    pub accent: bool,
    pub support: Option<String>,
}

impl Look {
    pub fn of(settings: &CoreSettings) -> Look {
        let base = settings
            .public_url
            .as_deref()
            .map(|u| u.trim().trim_end_matches('/').to_string())
            .filter(|u| u.starts_with("https://") || u.starts_with("http://"));
        let logo = match (&base, &settings.logo) {
            (Some(base), Some(logo)) if matches!(logo.kind, ImageKind::Png | ImageKind::Jpeg) => {
                settings.logo_url().map(|path| format!("{base}{path}"))
            }
            _ => None,
        };
        Look {
            brand: settings.brand_name.clone(),
            base,
            logo,
            accent: settings.appearance.client == ClientDesign::Accent,
            support: settings.support_link().map(str::to_string),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Composed {
    pub subject: String,
    pub text: String,
    pub html: String,
}

fn mark_code(spec: &Spec, template: &str) -> String {
    if !spec.required.contains(&"code") {
        return template.to_string();
    }
    template
        .split('\n')
        .map(|line| {
            if line.trim() == "{code}" {
                format!("{CODE_OPEN}{{code}}{CODE_CLOSE}")
            } else {
                line.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn big_code(html: &str) -> String {
    html.replace(
        CODE_OPEN,
        &format!(
            "<span style=\"display:inline-block;margin:4px 0;font-family:{MONO};font-size:30px;font-weight:700;letter-spacing:0.08em;line-height:1.3;color:{INK};\">"
        ),
    )
    .replace(CODE_CLOSE, "</span>")
}

fn footer(brand: &str, locale: Locale) -> String {
    match locale {
        Locale::Ru => format!("Это письмо о вашем аккаунте в {brand}."),
        Locale::En => format!("This email is about your account at {brand}."),
    }
}

fn sign_values(look: &Look) -> Values {
    Values::new()
        .with("brand", look.brand.clone())
        .maybe("support", look.support.clone())
        .maybe("site", look.base.clone())
}

fn brand_row(look: &Look) -> String {
    let mark = match &look.logo {
        Some(url) => format!(
            "<img src=\"{}\" width=\"30\" height=\"30\" alt=\"\" style=\"display:block;width:30px;height:30px;border:0;border-radius:8px;object-fit:contain;\">",
            escape(url)
        ),
        None => format!(
            "<span style=\"display:block;width:30px;height:30px;border-radius:8px;background:#0b0b0c;color:#ffffff;font-size:16px;font-weight:800;line-height:30px;text-align:center;\">{}</span>",
            escape(
                &look
                    .brand
                    .chars()
                    .next()
                    .map(|c| c.to_uppercase().collect::<String>())
                    .unwrap_or_default()
            )
        ),
    };
    format!(
        "<table role=\"presentation\" cellpadding=\"0\" cellspacing=\"0\"><tr>\
         <td style=\"width:30px;\">{mark}</td>\
         <td style=\"padding-left:10px;font-size:15px;font-weight:700;letter-spacing:0.02em;\">{}</td>\
         </tr></table>",
        escape(&look.brand)
    )
}

fn frame(
    locale: Locale,
    subject: &str,
    body: &str,
    button: Option<(&str, &str)>,
    sign: &str,
    look: &Look,
) -> String {
    let (fill, ink) = if look.accent {
        (ACCENT, ACCENT_INK)
    } else {
        (INK, "#ffffff")
    };
    let mut out = String::with_capacity(body.len() + 3072);
    out.push_str(&format!(
        "<!doctype html><html lang=\"{}\"><head><meta charset=\"utf-8\">\
         <meta name=\"viewport\" content=\"width=device-width,initial-scale=1\">\
         <title>{}</title></head>\
         <body style=\"margin:0;padding:0;background:{PAPER};\">\
         <table role=\"presentation\" width=\"100%\" cellpadding=\"0\" cellspacing=\"0\" style=\"background:{PAPER};\">\
         <tr><td align=\"center\" style=\"padding:24px 12px;\">\
         <table role=\"presentation\" width=\"100%\" cellpadding=\"0\" cellspacing=\"0\" \
         style=\"max-width:560px;background:#ffffff;border-radius:14px;font-family:{FONT};color:{INK};\">\
         <tr><td style=\"padding:22px 24px 0;\">{}</td></tr>\
         <tr><td style=\"padding:18px 24px 4px;font-size:16px;line-height:1.55;\">{body}</td></tr>",
        locale.as_str(),
        escape(subject),
        brand_row(look),
    ));
    if let Some((label, href)) = button {
        out.push_str(&format!(
            "<tr><td style=\"padding:14px 24px 0;\">\
             <a href=\"{}\" style=\"display:block;padding:12px 16px;border-radius:10px;background:{fill};color:{ink};\
             text-decoration:none;text-align:center;font-weight:600;font-size:15px;\">{}</a></td></tr>",
            escape(href),
            escape(label)
        ));
    }
    if sign.trim().is_empty() {
        out.push_str("<tr><td style=\"padding:0 0 24px;\"></td></tr>");
    } else {
        out.push_str(&format!(
            "<tr><td style=\"padding:20px 24px 24px;\">\
             <div style=\"padding-top:16px;border-top:1px solid {LINE};font-size:14px;line-height:1.5;color:{SOFT};\">{sign}</div>\
             </td></tr>"
        ));
    }
    out.push_str(&format!(
        "</table>\
         <table role=\"presentation\" width=\"100%\" cellpadding=\"0\" cellspacing=\"0\" style=\"max-width:560px;\">\
         <tr><td style=\"padding:14px 24px;font-family:{FONT};font-size:12px;line-height:1.5;color:{MUTED};text-align:center;\">{}</td></tr>\
         </table></td></tr></table></body></html>",
        escape(&footer(&look.brand, locale))
    ));
    out
}

pub fn compose(
    spec: &Spec,
    locale: Locale,
    saved: &Saved,
    sign: Option<&str>,
    values: &Values,
    look: &Look,
) -> Option<Composed> {
    let texts = spec.texts(locale);
    let subject = catalog::first_plain(
        saved
            .subject
            .as_deref()
            .into_iter()
            .chain(texts.subject.iter().copied()),
        values,
    )
    .map(|s| catalog::one_line(&s))?;
    let candidates: Vec<String> = saved
        .text
        .as_deref()
        .into_iter()
        .chain(texts.text.iter().copied())
        .map(|t| mark_code(spec, t))
        .collect();
    let body = catalog::first_html(candidates.iter().map(String::as_str), values)?;
    let button = if spec.has_button() {
        let label = catalog::first_plain(
            saved.button.as_deref().into_iter().chain([texts.button]),
            values,
        )
        .map(|l| catalog::one_line(&l));
        let href = look.base.as_ref().and_then(|base| {
            catalog::first_plain(spec.path.iter().copied(), values).map(|p| format!("{base}{p}"))
        });
        label.zip(href)
    } else {
        None
    };
    let sign = catalog::first_html(
        sign.into_iter().chain([catalog::sign_default(locale)]),
        &sign_values(look),
    )
    .unwrap_or_default();
    let html = frame(
        locale,
        &subject,
        &big_code(&body_html(&body)),
        button.as_ref().map(|(l, h)| (l.as_str(), h.as_str())),
        &body_html(&sign),
        look,
    );
    let mut text = body_text(&body)
        .replace([CODE_OPEN, CODE_CLOSE], "")
        .trim()
        .to_string();
    if let Some((label, href)) = &button {
        text.push_str(&format!("\n\n{label}: {href}"));
    }
    let sign_text = body_text(&sign).trim().to_string();
    if !sign_text.is_empty() {
        text.push_str(&format!("\n\n{sign_text}"));
    }
    text.push_str(&format!("\n\n—\n{}\n", footer(&look.brand, locale)));
    Some(Composed {
        subject,
        text,
        html,
    })
}

fn rejected(code: &'static str, message: impl Into<String>) -> ServiceError {
    ServiceError::rejected(code, message)
}

fn invalid(message: impl Into<String>) -> ServiceError {
    rejected("mail_letters.invalid", message)
}

fn lang_title(locale: Locale) -> &'static str {
    match locale {
        Locale::Ru => "",
        Locale::En => ", английский",
    }
}

fn mail_error(err: Undelivered) -> ServiceError {
    match err {
        Undelivered::Address(reason) => rejected("mail.address", reason),
        Undelivered::Limited(_) => rejected(
            "mail.failed",
            "Почтовый сервис просит подождать, попробуйте через минуту",
        ),
        Undelivered::Off(reason) | Undelivered::Failed(reason) => {
            rejected("mail.failed", format!("Письмо не отправилось: {reason}"))
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct VarView {
    pub name: &'static str,
    pub hint: &'static str,
    pub always: bool,
}

impl VarView {
    fn of(var: &Var) -> VarView {
        VarView {
            name: var.name,
            hint: var.hint,
            always: var.always,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ByLang<T> {
    pub ru: T,
    pub en: T,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TextsView {
    pub subject: &'static str,
    pub text: &'static str,
    pub button: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LetterView {
    pub id: LetterId,
    pub group: Group,
    pub title: &'static str,
    pub when: &'static str,
    pub vars: Vec<VarView>,
    pub required: &'static [&'static str],
    pub button: bool,
    pub path: Option<&'static str>,
    pub default: ByLang<TextsView>,
    pub saved: ByLang<Option<Saved>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct GroupView {
    pub key: Group,
    pub title: &'static str,
    pub switch: bool,
    pub on: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SignView {
    pub saved: ByLang<Option<String>>,
    pub default: ByLang<&'static str>,
    pub vars: Vec<VarView>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Limits {
    pub subject: usize,
    pub text: usize,
    pub button: usize,
    pub sign: usize,
    pub sender: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LettersView {
    pub groups: Vec<GroupView>,
    pub letters: Vec<LetterView>,
    pub sign: SignView,
    pub sender: Option<String>,
    pub brand: String,
    pub from_email: Option<String>,
    pub logo_url: Option<String>,
    pub accent: bool,
    pub sends: bool,
    pub site: bool,
    pub english: bool,
    pub limits: Limits,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct DraftTexts {
    #[serde(default)]
    pub subject: String,
    #[serde(default)]
    pub text: String,
    #[serde(default)]
    pub button: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct LangDrafts {
    #[serde(default)]
    pub ru: Option<DraftTexts>,
    #[serde(default)]
    pub en: Option<DraftTexts>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct LangSigns {
    #[serde(default)]
    pub ru: Option<String>,
    #[serde(default)]
    pub en: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct LettersUpdate {
    #[serde(default)]
    pub letters: BTreeMap<String, LangDrafts>,
    #[serde(default)]
    pub sign: LangSigns,
    #[serde(default)]
    pub sender: Option<String>,
    #[serde(default)]
    pub groups: BTreeMap<Group, bool>,
}

impl LettersUpdate {
    pub fn touches_module(&self) -> bool {
        self.sender.is_some() || !self.groups.is_empty()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct DraftLetter {
    pub letter: String,
    #[serde(default)]
    pub lang: Locale,
    #[serde(default)]
    pub subject: String,
    #[serde(default)]
    pub text: String,
    #[serde(default)]
    pub button: String,
    #[serde(default)]
    pub sign: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PreviewView {
    pub from: String,
    pub subject: String,
    pub html: String,
    pub text: String,
}

fn spec_of(raw: &str) -> ServiceResult<&'static Spec> {
    LetterId::parse(raw)
        .map(LetterId::spec)
        .ok_or(ServiceError::NotFound("письмо"))
}

fn cleaned(spec: &Spec, locale: Locale, draft: &DraftTexts) -> ServiceResult<Saved> {
    let texts = spec.texts(locale);
    let mut saved = Saved::default();
    for (field, raw, standard) in [
        (
            Field::Subject,
            &draft.subject,
            texts.subject.first().copied(),
        ),
        (Field::Text, &draft.text, texts.text.first().copied()),
        (Field::Button, &draft.button, Some(texts.button)),
    ] {
        let value = catalog::clean(spec, field, raw).map_err(|p| {
            invalid(format!(
                "«{}»{}, {}: {}",
                spec.title,
                lang_title(locale),
                p.field.title(),
                p.message
            ))
        })?;
        let value = value.filter(|v| Some(v.as_str()) != standard);
        match field {
            Field::Subject => saved.subject = value,
            Field::Text => saved.text = value,
            Field::Button => saved.button = value,
            Field::Sign => {}
        }
    }
    Ok(saved)
}

fn cleaned_sign(locale: Locale, raw: &str) -> ServiceResult<Option<String>> {
    let value = catalog::clean_sign(raw)
        .map_err(|p| invalid(format!("Подпись{}: {}", lang_title(locale), p.message)))?;
    Ok(value.filter(|v| v != catalog::sign_default(locale)))
}

async fn read_saved(db: &mut impl Db, id: LetterId, locale: Locale) -> ServiceResult<Saved> {
    Ok(db
        .settings()
        .get(&letter_key(id, locale))
        .await?
        .and_then(|v| serde_json::from_value::<Saved>(v).ok())
        .unwrap_or_default())
}

async fn read_sign(db: &mut impl Db, locale: Locale) -> ServiceResult<Option<String>> {
    Ok(db
        .settings()
        .get(&sign_key(locale))
        .await?
        .and_then(|v| v.as_str().map(str::to_string))
        .filter(|s| !s.trim().is_empty()))
}

pub struct LettersOps<'a> {
    services: &'a Services,
}

impl<'a> LettersOps<'a> {
    pub fn new(services: &'a Services) -> Self {
        LettersOps { services }
    }

    fn mail_settings(&self) -> Option<MailSettings> {
        MailSettings::parse(&self.services.modules().config(ModuleId::Mail)).ok()
    }

    pub async fn render(
        &self,
        id: LetterId,
        locale: Locale,
        values: &Values,
    ) -> ServiceResult<Option<Composed>> {
        let (saved, sign) = {
            let mut conn = self.services.store().conn().await?;
            (
                read_saved(&mut conn, id, locale).await?,
                read_sign(&mut conn, locale).await?,
            )
        };
        let look = Look::of(&self.services.settings());
        Ok(compose(
            id.spec(),
            locale,
            &saved,
            sign.as_deref(),
            values,
            &look,
        ))
    }

    pub async fn letter(
        &self,
        id: LetterId,
        to: &str,
        mut values: Values,
    ) -> ServiceResult<Option<RichLetter>> {
        values.set("brand", self.services.settings().brand_name.clone());
        values.set("email", to);
        Ok(self
            .render(id, lang::current(), &values)
            .await?
            .map(|composed| RichLetter {
                to: to.to_string(),
                subject: composed.subject,
                text: composed.text,
                html: composed.html,
                unsubscribe: None,
            }))
    }

    pub async fn send(&self, id: LetterId, to: &str, values: Values) -> ServiceResult<()> {
        let mail = self.services.mail();
        if !mail.available() {
            return Err(rejected(
                "mail.unavailable",
                "Письма не отправляются: в «Почте» не выбран способ отправки",
            ));
        }
        let letter = self.letter(id, to, values).await?.ok_or_else(|| {
            ServiceError::Internal(format!("письмо «{}» без нужных данных", id.spec().title))
        })?;
        mail.deliver(&letter).await.map_err(mail_error)
    }

    pub async fn view(&self) -> ServiceResult<LettersView> {
        let settings = self.services.settings();
        let mail = self.mail_settings();
        let enabled = self.services.modules().is_enabled(ModuleId::Mail);
        let mut saved: BTreeMap<String, Saved> = BTreeMap::new();
        let mut signs: BTreeMap<String, String> = BTreeMap::new();
        {
            let mut conn = self.services.store().conn().await?;
            for (key, value) in conn.settings().with_prefix(LETTER_PREFIX).await? {
                if let Ok(entry) = serde_json::from_value::<Saved>(value)
                    && !entry.is_empty()
                {
                    saved.insert(key, entry);
                }
            }
            for (key, value) in conn.settings().with_prefix(SIGN_PREFIX).await? {
                if let Some(text) = value.as_str().filter(|s| !s.trim().is_empty()) {
                    signs.insert(key, text.to_string());
                }
            }
        }
        let letters_on = mail.as_ref().map(|m| m.letters);
        let groups = Group::ALL
            .into_iter()
            .map(|group| GroupView {
                key: group,
                title: group.title(),
                switch: group.switch().is_some(),
                on: letters_on.is_none_or(|l| l.allows(group)),
            })
            .collect();
        let default_texts = |spec: &Spec, locale: Locale| {
            let texts = spec.texts(locale);
            TextsView {
                subject: texts.subject.first().copied().unwrap_or_default(),
                text: texts.text.first().copied().unwrap_or_default(),
                button: texts.button,
            }
        };
        let letters = CATALOG
            .iter()
            .map(|spec| LetterView {
                id: spec.id,
                group: spec.group,
                title: spec.title,
                when: spec.when,
                vars: spec.all_vars().map(VarView::of).collect(),
                required: spec.required,
                button: spec.has_button(),
                path: spec.path.first().copied(),
                default: ByLang {
                    ru: default_texts(spec, Locale::Ru),
                    en: default_texts(spec, Locale::En),
                },
                saved: ByLang {
                    ru: saved.get(&letter_key(spec.id, Locale::Ru)).cloned(),
                    en: saved.get(&letter_key(spec.id, Locale::En)).cloned(),
                },
            })
            .collect();
        let look = Look::of(&settings);
        Ok(LettersView {
            groups,
            letters,
            sign: SignView {
                saved: ByLang {
                    ru: signs.get(&sign_key(Locale::Ru)).cloned(),
                    en: signs.get(&sign_key(Locale::En)).cloned(),
                },
                default: ByLang {
                    ru: catalog::sign_default(Locale::Ru),
                    en: catalog::sign_default(Locale::En),
                },
                vars: SIGN_VARS.iter().map(VarView::of).collect(),
            },
            sender: mail.as_ref().and_then(|m| m.from_name.clone()),
            brand: settings.brand_name.clone(),
            from_email: mail
                .as_ref()
                .map(|m| m.from_email.clone())
                .filter(|e| !e.is_empty()),
            logo_url: settings.logo_url(),
            accent: look.accent,
            sends: enabled && mail.as_ref().is_some_and(MailSettings::sends),
            site: look.base.is_some(),
            english: settings.languages.english,
            limits: Limits {
                subject: SUBJECT_MAX,
                text: TEXT_MAX,
                button: BUTTON_MAX,
                sign: SIGN_MAX,
                sender: NAME_MAX,
            },
        })
    }

    pub async fn update(
        &self,
        update: &LettersUpdate,
        admin: AdminId,
    ) -> ServiceResult<LettersView> {
        let mut writes: Vec<(String, Option<Value>)> = Vec::new();
        for (raw, drafts) in &update.letters {
            let spec = spec_of(raw)?;
            for (locale, draft) in [(Locale::Ru, &drafts.ru), (Locale::En, &drafts.en)] {
                let Some(draft) = draft else {
                    continue;
                };
                let saved = cleaned(spec, locale, draft)?;
                let value = if saved.is_empty() {
                    None
                } else {
                    Some(serde_json::to_value(&saved).map_err(|e| {
                        ServiceError::Internal(format!("шаблон письма не записался: {e}"))
                    })?)
                };
                writes.push((letter_key(spec.id, locale), value));
            }
        }
        for (locale, raw) in [(Locale::Ru, &update.sign.ru), (Locale::En, &update.sign.en)] {
            let Some(raw) = raw else {
                continue;
            };
            let value = cleaned_sign(locale, raw)?.map(Value::String);
            writes.push((sign_key(locale), value));
        }
        if update.touches_module() {
            let mut config = Map::new();
            if let Some(sender) = &update.sender {
                let sender = sender.trim();
                config.insert(
                    "from_name".into(),
                    if sender.is_empty() || sender == self.services.settings().brand_name {
                        Value::Null
                    } else {
                        Value::String(sender.to_string())
                    },
                );
            }
            for (group, on) in &update.groups {
                if let Some(key) = group.switch() {
                    config.insert(key.into(), Value::Bool(*on));
                }
            }
            self.services
                .settings_ops()
                .update_module_config(
                    ModuleId::Mail,
                    &ModuleConfigUpdate {
                        config,
                        clear: Vec::new(),
                    },
                    admin,
                )
                .await?;
        }
        if !writes.is_empty() {
            let mut tx = self.services.store().begin().await?;
            for (key, value) in &writes {
                match value {
                    Some(value) => tx.settings().set(key, value, Some(admin)).await?,
                    None => {
                        tx.settings().delete(key).await?;
                    }
                }
            }
            tx.audit()
                .record(&Actor::Admin(admin).audit(
                    "mail.letters",
                    "settings",
                    None,
                    serde_json::json!({
                        "keys": writes.iter().map(|(k, _)| k.as_str()).collect::<Vec<_>>(),
                    }),
                ))
                .await?;
            tx.commit().await?;
        }
        self.view().await
    }

    fn sender_line(&self) -> String {
        let mail = self.mail_settings();
        let name = mail
            .as_ref()
            .and_then(|m| m.from_name.clone())
            .unwrap_or_else(|| self.services.settings().brand_name.clone());
        match mail.map(|m| m.from_email).filter(|e| !e.is_empty()) {
            Some(email) => format!("{name} <{email}>"),
            None => name,
        }
    }

    async fn compose_draft(&self, draft: &DraftLetter) -> ServiceResult<Composed> {
        let spec = spec_of(&draft.letter)?;
        let locale = draft.lang;
        let saved = cleaned(
            spec,
            locale,
            &DraftTexts {
                subject: draft.subject.clone(),
                text: draft.text.clone(),
                button: draft.button.clone(),
            },
        )?;
        let sign = match &draft.sign {
            Some(raw) => cleaned_sign(locale, raw)?,
            None => {
                let mut conn = self.services.store().conn().await?;
                read_sign(&mut conn, locale).await?
            }
        };
        let settings = self.services.settings();
        let values = catalog::samples(spec, locale, settings.currency, &settings.brand_name);
        compose(
            spec,
            locale,
            &saved,
            sign.as_deref(),
            &values,
            &Look::of(&settings),
        )
        .ok_or_else(|| invalid("Письмо не собралось на выдуманных данных"))
    }

    pub async fn preview(&self, draft: &DraftLetter) -> ServiceResult<PreviewView> {
        let composed = self.compose_draft(draft).await?;
        Ok(PreviewView {
            from: self.sender_line(),
            subject: composed.subject,
            html: composed.html,
            text: composed.text,
        })
    }

    pub async fn test(&self, draft: &DraftLetter, to: &str) -> ServiceResult<()> {
        let to = normalize_identity(IdentityKind::Email, to)
            .map_err(|_| rejected("mail.address", "Адрес для проверки записан неверно"))?;
        if !self.services.mail().available() {
            return Err(rejected(
                "mail.unavailable",
                "Письма не отправляются: во вкладке «Отправка» не выбран способ отправки",
            ));
        }
        let composed = self.compose_draft(draft).await?;
        self.services
            .mail()
            .deliver(&RichLetter {
                to,
                subject: composed.subject,
                text: composed.text,
                html: composed.html,
                unsubscribe: None,
            })
            .await
            .map_err(mail_error)
    }
}
