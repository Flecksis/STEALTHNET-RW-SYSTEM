use super::ButtonSpec;
use crate::sender::MailSite;
use sn_domain::bot_content::escape;
use uuid::Uuid;

const INK: &str = "#151a22";
const MUTED: &str = "#626b78";
const LINE: &str = "#d0d5dd";
const LINK: &str = "#2b5bf0";
const FONT: &str = "-apple-system,BlinkMacSystemFont,'Segoe UI',Roboto,Helvetica,Arial,sans-serif";
const MONO: &str = "ui-monospace,SFMono-Regular,Menlo,Consolas,monospace";

pub fn unsubscribe_url(site: &MailSite, token: Uuid) -> String {
    format!(
        "{}/api/public/m/broadcast/unsubscribe/{token}",
        site.base_url
    )
}

pub fn image_url(site: &MailSite, key: Uuid) -> String {
    format!("{}/api/public/m/broadcast/images/{key}", site.base_url)
}

fn decode(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find('&') {
        out.push_str(&rest[..at]);
        let tail = &rest[at..];
        let end = tail.find(';').filter(|e| *e <= 10);
        let entity = end.map(|e| &tail[1..e]);
        let decoded = match entity {
            Some("amp") => Some('&'),
            Some("lt") => Some('<'),
            Some("gt") => Some('>'),
            Some("quot") => Some('"'),
            Some(name) if name.starts_with("#x") || name.starts_with("#X") => {
                u32::from_str_radix(&name[2..], 16)
                    .ok()
                    .and_then(char::from_u32)
            }
            Some(name) if name.starts_with('#') => {
                name[1..].parse::<u32>().ok().and_then(char::from_u32)
            }
            _ => None,
        };
        match (decoded, end) {
            (Some(ch), Some(e)) => {
                out.push(ch);
                rest = &tail[e + 1..];
            }
            _ => {
                out.push('&');
                rest = &tail[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

fn attribute<'a>(tag: &'a str, name: &str) -> Option<&'a str> {
    let at = tag.find(&format!("{name}=\""))?;
    let value = &tag[at + name.len() + 2..];
    value.find('"').map(|end| &value[..end])
}

fn tag_name(tag: &str) -> String {
    tag.trim_start_matches('<')
        .trim_start_matches('/')
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric() || *c == '-')
        .collect::<String>()
        .to_ascii_lowercase()
}

pub fn body_html(telegram: &str) -> String {
    let mut out = String::with_capacity(telegram.len() + 64);
    let mut pre = 0usize;
    let mut rest = telegram;
    loop {
        let next = rest.find(['<', '\n']);
        let Some(at) = next else {
            out.push_str(rest);
            break;
        };
        out.push_str(&rest[..at]);
        let tail = &rest[at..];
        if let Some(after) = tail.strip_prefix('\n') {
            out.push_str(if pre > 0 { "\n" } else { "<br>" });
            rest = after;
            continue;
        }
        let Some(close) = tail.find('>') else {
            out.push_str("&lt;");
            rest = &tail[1..];
            continue;
        };
        let tag = &tail[..=close];
        let closing = tag.starts_with("</");
        let name = tag_name(tag);
        let mapped = match (name.as_str(), closing) {
            ("tg-emoji", _) => String::new(),
            ("tg-spoiler" | "span", false) => "<span>".into(),
            ("tg-spoiler" | "span", true) => "</span>".into(),
            ("a", false) => format!(
                "<a href=\"{}\" style=\"color:{LINK};text-decoration:underline;\">",
                attribute(tag, "href").unwrap_or("#")
            ),
            ("code", false) => {
                format!("<code style=\"font-family:{MONO};font-size:14px;\">")
            }
            ("pre", false) => {
                pre += 1;
                format!(
                    "<pre style=\"margin:8px 0;white-space:pre-wrap;font-family:{MONO};font-size:14px;\">"
                )
            }
            ("pre", true) => {
                pre = pre.saturating_sub(1);
                "</pre>".into()
            }
            ("blockquote", false) => format!(
                "<blockquote style=\"margin:8px 0;padding:0 0 0 12px;border-left:3px solid {LINE};color:#3e4652;\">"
            ),
            (other, true) => format!("</{other}>"),
            (other, false) => format!("<{other}>"),
        };
        out.push_str(&mapped);
        rest = &tail[close + 1..];
    }
    out
}

pub fn body_text(telegram: &str) -> String {
    let mut out = String::with_capacity(telegram.len());
    let mut links: Vec<Option<String>> = Vec::new();
    let mut rest = telegram;
    while let Some(at) = rest.find('<') {
        out.push_str(&decode(&rest[..at]));
        let tail = &rest[at..];
        let Some(close) = tail.find('>') else {
            out.push_str(&decode(tail));
            rest = "";
            break;
        };
        let tag = &tail[..=close];
        let name = tag_name(tag);
        if name == "a" {
            if tag.starts_with("</") {
                if let Some(Some(href)) = links.pop()
                    && !out.ends_with(&href)
                {
                    out.push_str(&format!(" ({href})"));
                }
            } else {
                links.push(attribute(tag, "href").map(decode));
            }
        }
        rest = &tail[close + 1..];
    }
    out.push_str(&decode(rest));
    out
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Letter {
    pub subject: String,
    brand: String,
    html: String,
    text: String,
    image: Option<String>,
    buttons: Vec<(String, String)>,
}

impl Letter {
    pub fn new(
        subject: &str,
        telegram: &str,
        image: Option<String>,
        buttons: &[ButtonSpec],
        site: &MailSite,
    ) -> Self {
        let buttons = buttons
            .iter()
            .map(|button| {
                let href = match (&button.url, button.section) {
                    (Some(url), _) => url.clone(),
                    (None, Some(section)) => format!("{}{}", site.base_url, section.web_path()),
                    (None, None) => site.base_url.clone(),
                };
                (button.text.clone(), href)
            })
            .collect();
        Letter {
            subject: subject.to_string(),
            brand: site.brand.clone(),
            html: body_html(telegram),
            text: body_text(telegram),
            image,
            buttons,
        }
    }

    pub fn html(&self, unsubscribe: &str) -> String {
        let mut out = String::with_capacity(self.html.len() + 2048);
        out.push_str(&format!(
            "<!doctype html><html lang=\"ru\"><head><meta charset=\"utf-8\">\
             <meta name=\"viewport\" content=\"width=device-width,initial-scale=1\">\
             <title>{}</title></head>\
             <body style=\"margin:0;padding:0;background:#f2f4f7;\">\
             <table role=\"presentation\" width=\"100%\" cellpadding=\"0\" cellspacing=\"0\" style=\"background:#f2f4f7;\">\
             <tr><td align=\"center\" style=\"padding:24px 12px;\">\
             <table role=\"presentation\" width=\"100%\" cellpadding=\"0\" cellspacing=\"0\" \
             style=\"max-width:560px;background:#ffffff;border-radius:14px;overflow:hidden;font-family:{FONT};color:{INK};\">",
            escape(&self.subject)
        ));
        if let Some(image) = &self.image {
            out.push_str(&format!(
                "<tr><td style=\"padding:0;\"><img src=\"{}\" width=\"560\" alt=\"\" \
                 style=\"display:block;width:100%;max-width:560px;height:auto;border:0;\"></td></tr>",
                escape(image)
            ));
        }
        out.push_str(&format!(
            "<tr><td style=\"padding:24px 24px {};font-size:16px;line-height:1.55;\">{}</td></tr>",
            if self.buttons.is_empty() {
                "28px"
            } else {
                "8px"
            },
            self.html
        ));
        if !self.buttons.is_empty() {
            out.push_str("<tr><td style=\"padding:8px 24px 24px;\">");
            for (label, href) in &self.buttons {
                out.push_str(&format!(
                    "<a href=\"{}\" style=\"display:block;margin:8px 0 0;padding:12px 16px;border-radius:10px;\
                     background:{INK};color:#ffffff;text-decoration:none;text-align:center;font-weight:600;font-size:15px;\">{}</a>",
                    escape(href),
                    escape(label)
                ));
            }
            out.push_str("</td></tr>");
        }
        out.push_str(&format!(
            "</table>\
             <table role=\"presentation\" width=\"100%\" cellpadding=\"0\" cellspacing=\"0\" style=\"max-width:560px;\">\
             <tr><td style=\"padding:16px 24px;font-family:{FONT};font-size:12px;line-height:1.5;color:{MUTED};text-align:center;\">\
             Это письмо пришло, потому что у вас есть аккаунт в {}. \
             <a href=\"{}\" style=\"color:{MUTED};text-decoration:underline;white-space:nowrap;\">Отписаться от рассылок</a>\
             </td></tr></table>\
             </td></tr></table></body></html>",
            escape(&self.brand),
            escape(unsubscribe)
        ));
        out
    }

    pub fn text(&self, unsubscribe: &str) -> String {
        let mut out = self.text.trim().to_string();
        if !self.buttons.is_empty() {
            out.push('\n');
            for (label, href) in &self.buttons {
                out.push_str(&format!("\n{label}: {href}"));
            }
        }
        out.push_str(&format!(
            "\n\n—\nЭто письмо пришло, потому что у вас есть аккаунт в {}.\nОтписаться от рассылок: {unsubscribe}\n",
            self.brand
        ));
        out
    }
}
