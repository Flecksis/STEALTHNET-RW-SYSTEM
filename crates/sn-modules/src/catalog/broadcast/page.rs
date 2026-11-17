use sn_domain::bot_content::escape;

pub struct Form<'a> {
    pub action: &'a str,
    pub button: &'a str,
    pub quiet: bool,
}

pub fn page(brand: &str, title: &str, text: &str, form: Option<Form<'_>>) -> String {
    let form = form
        .map(|form| {
            format!(
                "<form method=\"post\" action=\"{}\"><input type=\"hidden\" name=\"confirm\" value=\"1\">\
                 <button type=\"submit\" class=\"{}\">{}</button></form>",
                escape(form.action),
                if form.quiet { "quiet" } else { "main" },
                escape(form.button)
            )
        })
        .unwrap_or_default();
    format!(
        "<!doctype html><html lang=\"ru\"><head><meta charset=\"utf-8\">\
         <meta name=\"viewport\" content=\"width=device-width,initial-scale=1\">\
         <meta name=\"robots\" content=\"noindex\"><title>{title} — {brand}</title><style>\
         :root{{color-scheme:light dark;--bg:#f2f4f7;--card:#ffffff;--ink:#151a22;--muted:#626b78;--line:#e5e8ed;--btn:#151a22;--btn-ink:#ffffff}}\
         @media (prefers-color-scheme:dark){{:root{{--bg:#0f1013;--card:#17191d;--ink:#edeef0;--muted:#8c929c;--line:#262a30;--btn:#edeef0;--btn-ink:#0f1013}}}}\
         *{{box-sizing:border-box}}\
         body{{margin:0;min-height:100vh;display:grid;place-items:center;padding:24px 16px;background:var(--bg);color:var(--ink);\
         font:16px/1.5 -apple-system,BlinkMacSystemFont,'Segoe UI',Roboto,Helvetica,Arial,sans-serif}}\
         main{{width:100%;max-width:440px;padding:28px 24px;border:1px solid var(--line);border-radius:16px;background:var(--card)}}\
         .brand{{margin:0 0 18px;color:var(--ink);font-weight:800;letter-spacing:-0.01em}}\
         h1{{margin:0 0 8px;font-size:22px;line-height:1.3}}\
         p{{margin:0;color:var(--muted)}}\
         form{{margin-top:20px}}\
         button{{font:inherit;font-weight:700;cursor:pointer;border-radius:10px}}\
         .main{{height:44px;padding:0 20px;border:0;background:var(--btn);color:var(--btn-ink)}}\
         .quiet{{height:40px;padding:0 16px;border:1px solid var(--line);background:transparent;color:var(--ink)}}\
         </style></head><body><main><p class=\"brand\">{brand}</p><h1>{title}</h1><p>{text}</p>{form}</main></body></html>",
        brand = escape(brand),
        title = escape(title),
        text = escape(text),
    )
}
