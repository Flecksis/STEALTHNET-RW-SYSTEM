use super::labels;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

const KNOWN_FIELDS: [(&str, &str); 2] = [
    ("crates/sn-services/src/subscriptions.rs", "change.action"),
    ("crates/sn-services/src/stars_review.rs", "decision.action"),
];

struct Source {
    path: String,
    text: Vec<char>,
}

fn is_code(text: &str) -> bool {
    let parts: Vec<&str> = text.split('.').collect();
    parts.len() >= 2
        && parts.iter().all(|part| {
            let mut chars = part.chars();
            chars.next().is_some_and(|c| c.is_ascii_lowercase())
                && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
        })
}

fn is_ident(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

fn starts(text: &[char], at: usize, pat: &str) -> bool {
    let mut i = at;
    for p in pat.chars() {
        if text.get(i) != Some(&p) {
            return false;
        }
        i += 1;
    }
    true
}

fn raw_string_end(text: &[char], at: usize) -> Option<usize> {
    if text.get(at) != Some(&'r') || (at > 0 && is_ident(text[at - 1])) {
        return None;
    }
    let mut i = at + 1;
    let mut hashes = 0;
    while text.get(i) == Some(&'#') {
        hashes += 1;
        i += 1;
    }
    if text.get(i) != Some(&'"') {
        return None;
    }
    i += 1;
    while i < text.len() {
        if text[i] == '"' && (0..hashes).all(|h| text.get(i + 1 + h) == Some(&'#')) {
            return Some(i + 1 + hashes);
        }
        i += 1;
    }
    Some(text.len())
}

fn skip(text: &[char], at: usize) -> Option<usize> {
    if let Some(end) = raw_string_end(text, at) {
        return Some(end);
    }
    match text[at] {
        '"' => {
            let mut i = at + 1;
            while i < text.len() {
                match text[i] {
                    '\\' => i += 2,
                    '"' => return Some(i + 1),
                    _ => i += 1,
                }
            }
            Some(text.len())
        }
        '\'' => {
            if text.get(at + 1) == Some(&'\\') {
                let mut i = at + 2;
                while i < text.len() && text[i] != '\'' {
                    i += 1;
                }
                Some(i + 1)
            } else if text.get(at + 2) == Some(&'\'') {
                Some(at + 3)
            } else {
                None
            }
        }
        '/' if text.get(at + 1) == Some(&'/') => {
            let mut i = at;
            while i < text.len() && text[i] != '\n' {
                i += 1;
            }
            Some(i)
        }
        '/' if text.get(at + 1) == Some(&'*') => {
            let mut i = at + 2;
            while i + 1 < text.len() && !(text[i] == '*' && text[i + 1] == '/') {
                i += 1;
            }
            Some(i + 2)
        }
        _ => None,
    }
}

fn close_of(text: &[char], open: usize) -> usize {
    let mut depth = 0i32;
    let mut i = open;
    while i < text.len() {
        if let Some(next) = skip(text, i) {
            i = next;
            continue;
        }
        match text[i] {
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' => {
                depth -= 1;
                if depth == 0 {
                    return i;
                }
            }
            _ => {}
        }
        i += 1;
    }
    text.len()
}

fn split_top(text: &[char]) -> Vec<String> {
    let mut parts = Vec::new();
    let mut depth = 0i32;
    let mut current = String::new();
    let mut i = 0;
    while i < text.len() {
        if let Some(next) = skip(text, i) {
            current.extend(&text[i..next.min(text.len())]);
            i = next;
            continue;
        }
        let c = text[i];
        match c {
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' => depth -= 1,
            _ => {}
        }
        if c == ',' && depth == 0 {
            parts.push(current.trim().to_string());
            current.clear();
        } else {
            current.push(c);
        }
        i += 1;
    }
    if !current.trim().is_empty() {
        parts.push(current.trim().to_string());
    }
    parts
}

fn strip_tests(text: Vec<char>) -> Vec<char> {
    let mut out = Vec::with_capacity(text.len());
    let mut i = 0;
    while i < text.len() {
        if starts(&text, i, "#[cfg(test)]") {
            let mut j = i + "#[cfg(test)]".len();
            while j < text.len() && text[j] != '{' && text[j] != ';' {
                j += 1;
            }
            i = if text.get(j) == Some(&'{') {
                close_of(&text, j) + 1
            } else {
                j + 1
            };
            continue;
        }
        if let Some(next) = skip(&text, i) {
            out.extend_from_slice(&text[i..next.min(text.len())]);
            i = next;
            continue;
        }
        out.push(text[i]);
        i += 1;
    }
    out
}

fn collect(dir: &Path, root: &Path, out: &mut Vec<Source>) {
    let mut entries: Vec<PathBuf> = std::fs::read_dir(dir)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect();
    entries.sort();
    for path in entries {
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        if path.is_dir() {
            if !matches!(name.as_str(), "tests" | "scenario_tests" | "repo_tests") {
                collect(&path, root, out);
            }
        } else if name.ends_with(".rs") && name != "tests.rs" && !name.ends_with("_tests.rs") {
            let text: Vec<char> = std::fs::read_to_string(&path).unwrap().chars().collect();
            out.push(Source {
                path: path
                    .strip_prefix(root)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/"),
                text: strip_tests(text),
            });
        }
    }
}

fn sources() -> Vec<Source> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap();
    let mut out = Vec::new();
    let mut crates: Vec<PathBuf> = std::fs::read_dir(root.join("crates"))
        .unwrap()
        .map(|entry| entry.unwrap().path().join("src"))
        .filter(|path| path.is_dir())
        .collect();
    crates.sort();
    for dir in crates {
        collect(&dir, &root, &mut out);
    }
    out
}

fn literals(text: &[char]) -> Vec<String> {
    let mut found = Vec::new();
    let mut i = 0;
    while i < text.len() {
        if text[i] == '"' {
            let end = skip(text, i).unwrap_or(text.len());
            let inner: String = text[i + 1..end.saturating_sub(1).max(i + 1)]
                .iter()
                .collect();
            if is_code(&inner) {
                found.push(inner);
            }
            i = end;
            continue;
        }
        if let Some(next) = skip(text, i) {
            i = next;
            continue;
        }
        i += 1;
    }
    found
}

fn format_patterns(expr: &str) -> Vec<String> {
    let mut found = Vec::new();
    let mut rest = expr;
    while let Some(at) = rest.find("format!(") {
        let after = rest[at + "format!(".len()..].trim_start();
        if let Some(body) = after.strip_prefix('"')
            && let Some(end) = body.find('"')
        {
            let pattern = &body[..end];
            if pattern.contains('{') && is_code(&pattern.replace("{}", "x").replace(['{', '}'], ""))
            {
                found.push(pattern.to_string());
            }
        }
        rest = &rest[at + 1..];
    }
    found
}

fn screaming(expr: &str) -> Vec<String> {
    let chars: Vec<char> = expr.chars().collect();
    let mut found = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        if chars[i].is_ascii_uppercase() && (i == 0 || !is_ident(chars[i - 1])) {
            let start = i;
            while i < chars.len() && is_ident(chars[i]) {
                i += 1;
            }
            let word: String = chars[start..i].iter().collect();
            if word.len() > 2
                && word
                    .chars()
                    .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
            {
                found.push(word);
            }
            continue;
        }
        i += 1;
    }
    found
}

fn calls(expr: &str) -> Vec<String> {
    let chars: Vec<char> = expr.chars().collect();
    let mut found = Vec::new();
    for i in 0..chars.len() {
        if chars[i] == '(' && i > 0 && is_ident(chars[i - 1]) {
            let mut start = i;
            while start > 0 && is_ident(chars[start - 1]) {
                start -= 1;
            }
            found.push(chars[start..i].iter().collect());
        }
    }
    found
}

struct Scan<'s> {
    files: &'s [Source],
    consts: BTreeMap<String, BTreeSet<String>>,
    actions: BTreeMap<String, String>,
    patterns: BTreeMap<String, String>,
    unresolved: Vec<String>,
}

fn find(text: &[char], pat: &str, from: usize) -> Option<usize> {
    let pat: Vec<char> = pat.chars().collect();
    (from..text.len().saturating_sub(pat.len() - 1)).find(|i| text[*i..*i + pat.len()] == pat[..])
}

fn static_fn_body(text: &[char], name: &str) -> Option<Vec<char>> {
    let mut from = 0;
    while let Some(at) = find(text, &format!("fn {name}("), from) {
        let open = at + 3 + name.len();
        let close = close_of(text, open);
        let head: String = text[close..(close + 40).min(text.len())].iter().collect();
        if head.contains("&'static str")
            && let Some(brace) = find(text, "{", close)
        {
            let end = close_of(text, brace);
            return Some(text[brace..end].to_vec());
        }
        from = at + 1;
    }
    None
}

fn enclosing_fn(text: &[char], pos: usize) -> Option<(String, Vec<String>, usize)> {
    let mut best = None;
    let mut from = 0;
    while let Some(at) = find(text, "fn ", from) {
        if at >= pos {
            break;
        }
        if at == 0 || !is_ident(text[at - 1]) {
            best = Some(at);
        }
        from = at + 1;
    }
    let at = best?;
    let mut i = at + 3;
    let start = i;
    while i < text.len() && is_ident(text[i]) {
        i += 1;
    }
    let name: String = text[start..i].iter().collect();
    if text.get(i) == Some(&'<') {
        i = close_of(text, i) + 1;
    }
    let open = find(text, "(", i)?;
    let close = close_of(text, open);
    let params = split_top(&text[open + 1..close])
        .into_iter()
        .map(|param| {
            let param = param.trim().trim_start_matches("mut ").to_string();
            if param.ends_with("self") {
                "self".to_string()
            } else {
                param.split(':').next().unwrap_or("").trim().to_string()
            }
        })
        .collect();
    Some((name, params, at))
}

fn error_codes(rhs: &str) -> BTreeSet<String> {
    let mut found = BTreeSet::new();
    for head in ["rejected(", "field_error(", "ApiError::new(", "invalid("] {
        let mut rest = rhs;
        while let Some(at) = rest.find(head) {
            let after = &rest[at + head.len()..];
            let chars: Vec<char> = after.chars().collect();
            if let Some(first) = literals(&chars).first() {
                found.insert(first.clone());
            }
            rest = &rest[at + 1..];
        }
    }
    found
}

impl<'s> Scan<'s> {
    fn new(files: &'s [Source]) -> Self {
        let mut consts: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        for file in files {
            let text: String = file.text.iter().collect();
            let mut rest = text.as_str();
            while let Some(at) = rest.find("const ") {
                let after = &rest[at + "const ".len()..];
                let name: String = after.chars().take_while(|c| is_ident(*c)).collect();
                let tail = &after[name.len()..];
                if let Some(value) = tail
                    .trim_start()
                    .strip_prefix(':')
                    .map(str::trim_start)
                    .and_then(|t| t.strip_prefix("&"))
                    .map(|t| t.trim_start_matches("'static").trim_start())
                    .and_then(|t| t.strip_prefix("str"))
                    .map(str::trim_start)
                    .and_then(|t| t.strip_prefix('='))
                    .map(str::trim_start)
                    .and_then(|t| t.strip_prefix('"'))
                    .and_then(|t| t.split_once('"').map(|(v, _)| v.to_string()))
                {
                    consts.entry(name).or_default().insert(value);
                }
                rest = &rest[at + 1..];
            }
        }
        Scan {
            files,
            consts,
            actions: BTreeMap::new(),
            patterns: BTreeMap::new(),
            unresolved: Vec::new(),
        }
    }

    fn resolve(&self, expr: &str, file: &Source) -> (Vec<String>, Vec<String>) {
        let chars: Vec<char> = expr.chars().collect();
        let mut found = literals(&chars);
        for name in screaming(expr) {
            for value in self.consts.get(&name).into_iter().flatten() {
                if is_code(value) {
                    found.push(value.clone());
                }
            }
        }
        for name in calls(expr) {
            if let Some(body) = static_fn_body(&file.text, &name) {
                found.extend(literals(&body));
            }
        }
        (found, format_patterns(expr))
    }

    fn let_codes(&self, file: &Source, pos: usize, name: &str) -> Vec<String> {
        let Some((_, _, fn_at)) = enclosing_fn(&file.text, pos) else {
            return Vec::new();
        };
        let region: String = file.text[fn_at..pos].iter().collect();
        let mut best = None;
        let mut rest = 0;
        while let Some(at) = region[rest..].find("let ") {
            let start = rest + at;
            let head: String = region[start + 4..]
                .chars()
                .take_while(|c| *c != '=')
                .collect();
            if head.split(|c: char| !is_ident(c)).any(|word| word == name) {
                best = Some(start + 4 + head.len() + 1);
            }
            rest = start + 1;
        }
        let Some(start) = best else {
            return Vec::new();
        };
        let tail: Vec<char> = region[start..].chars().collect();
        let mut depth = 0i32;
        let mut end = tail.len();
        let mut i = 0;
        while i < tail.len() {
            if let Some(next) = skip(&tail, i) {
                i = next;
                continue;
            }
            match tail[i] {
                '(' | '[' | '{' => depth += 1,
                ')' | ']' | '}' => depth -= 1,
                ';' if depth == 0 => {
                    end = i;
                    break;
                }
                _ => {}
            }
            i += 1;
        }
        let rhs: String = tail[..end].iter().collect();
        let skip_codes = error_codes(&rhs);
        let (found, _) = self.resolve(&rhs, file);
        found
            .into_iter()
            .filter(|code| !skip_codes.contains(code))
            .collect()
    }

    fn add(&mut self, codes: Vec<String>, patterns: Vec<String>, place: &str) {
        for code in codes {
            self.actions
                .entry(code)
                .or_insert_with(|| place.to_string());
        }
        for pattern in patterns {
            self.patterns
                .entry(pattern)
                .or_insert_with(|| place.to_string());
        }
    }

    fn sink(
        &mut self,
        file: &Source,
        pos: usize,
        expr: &str,
        wrappers: &mut Vec<(String, String, usize, bool)>,
    ) {
        let place = format!(
            "{}:{}",
            file.path,
            file.text[..pos].iter().filter(|c| **c == '\n').count() + 1
        );
        let (codes, patterns) = self.resolve(expr, file);
        if !codes.is_empty() || !patterns.is_empty() {
            self.add(codes, patterns, &place);
            return;
        }
        let ident = expr
            .trim()
            .trim_start_matches('&')
            .trim_end_matches(".into()")
            .trim_end_matches(".to_string()")
            .trim_end_matches(".to_owned()")
            .trim()
            .to_string();
        if !ident.is_empty() && ident.chars().all(is_ident) {
            if let Some((name, params, _)) = enclosing_fn(&file.text, pos)
                && let Some(index) = params.iter().position(|p| *p == ident)
            {
                if !(file.path.ends_with("sn-services/src/lib.rs") && name == "audit") {
                    let method = params.first().is_some_and(|p| p == "self");
                    wrappers.push((file.path.clone(), name, index, method));
                }
                return;
            }
            let codes = self.let_codes(file, pos, &ident);
            if !codes.is_empty() {
                self.add(codes, Vec::new(), &place);
                return;
            }
        }
        if KNOWN_FIELDS
            .iter()
            .any(|(path, field)| file.path == *path && expr.contains(field))
        {
            return;
        }
        self.unresolved.push(format!("{place}: {}", expr.trim()));
    }

    fn run(&mut self) {
        let files = self.files;
        let mut wrappers = Vec::new();
        for file in files {
            let text = &file.text;
            let mut from = 0;
            while let Some(at) = find(text, ".audit(", from) {
                let open = at + ".audit".len();
                let args = split_top(&text[open + 1..close_of(text, open)]);
                if let Some(first) = args.first() {
                    self.sink(file, at, first, &mut wrappers);
                }
                from = at + 1;
            }
            let mut from = 0;
            while let Some(at) = find(text, "AuditEntry {", from) {
                from = at + 1;
                let before: String = text[at.saturating_sub(11)..at].iter().collect();
                if before.ends_with("struct ") {
                    continue;
                }
                let open = at + "AuditEntry ".len();
                let fields = split_top(&text[open + 1..close_of(text, open)]);
                let expr = fields.iter().find_map(|f| {
                    let f = f.trim();
                    if f == "action" {
                        Some("action".to_string())
                    } else {
                        f.strip_prefix("action:").map(|e| e.trim().to_string())
                    }
                });
                if let Some(expr) = expr {
                    self.sink(file, at, &expr, &mut wrappers);
                }
            }
        }
        let mut done = BTreeSet::new();
        while let Some((path, name, index, method)) = wrappers.pop() {
            if !done.insert((path.clone(), name.clone())) {
                continue;
            }
            let dir = path.rsplit_once('/').map_or("", |(d, _)| d).to_string();
            let stem = path
                .rsplit_once('/')
                .map_or(path.as_str(), |(_, f)| f)
                .trim_end_matches(".rs")
                .to_string();
            for file in files
                .iter()
                .filter(|f| f.path.rsplit_once('/').map_or("", |(d, _)| d) == dir)
            {
                let same = file.path == path;
                let text: String = file.text.iter().collect();
                let imported = text.contains("use super::")
                    && text
                        .lines()
                        .filter(|line| line.trim_start().starts_with("use super::"))
                        .any(|line| line.split(|c: char| !is_ident(c)).any(|w| w == name));
                let chars = &file.text;
                let pat = format!("{name}(");
                let mut from = 0;
                while let Some(at) = find(chars, &pat, from) {
                    from = at + 1;
                    if at > 0 && is_ident(chars[at - 1]) {
                        continue;
                    }
                    let head: String = chars[at.saturating_sub(8)..at].iter().collect();
                    if head.ends_with("fn ") {
                        continue;
                    }
                    let dotted = head.ends_with('.');
                    let qualified = head.ends_with("::");
                    let self_call = head.ends_with("Self::");
                    let allowed = if method {
                        same && (dotted || self_call)
                    } else if dotted {
                        false
                    } else if same {
                        true
                    } else {
                        (qualified
                            && (head.ends_with(&format!("{stem}::")) || head.ends_with("super::")))
                            || (!qualified && imported)
                    };
                    if !allowed {
                        continue;
                    }
                    let open = at + name.len();
                    let args = split_top(&chars[open + 1..close_of(chars, open)]);
                    let k = if method && !self_call {
                        index.checked_sub(1)
                    } else {
                        Some(index)
                    };
                    let Some(arg) = k.and_then(|k| args.get(k)) else {
                        continue;
                    };
                    self.sink(file, at, arg, &mut wrappers);
                }
            }
        }
        for file in files {
            let text: String = file.text.iter().collect();
            if let Some(at) = text.find("mod actions {") {
                let chars: Vec<char> = text[at..].chars().collect();
                let open = chars.iter().position(|c| *c == '{').unwrap();
                let body: String = chars[open..close_of(&chars, open)].iter().collect();
                let place = format!("{}: mod actions", file.path);
                let codes = literals(&body.chars().collect::<Vec<_>>());
                self.add(codes, Vec::new(), &place);
            }
        }
        let named: Vec<(String, String)> = self
            .consts
            .iter()
            .filter(|(name, _)| {
                name.ends_with("_AUDIT") || name.ends_with("_ACTION") || name.starts_with("AUDIT_")
            })
            .flat_map(|(name, values)| {
                values
                    .iter()
                    .filter(|v| is_code(v))
                    .map(move |v| (name.clone(), v.clone()))
            })
            .collect();
        for (name, value) in named {
            self.add(vec![value], Vec::new(), &format!("const {name}"));
        }
    }
}

fn scan() -> Scan<'static> {
    let files: &'static [Source] = Box::leak(sources().into_boxed_slice());
    let mut scan = Scan::new(files);
    scan.run();
    scan
}

fn pattern_matches(pattern: &str, code: &str) -> bool {
    let parts: Vec<&str> = pattern.split("{}").collect();
    if parts.len() < 2 {
        return pattern == code;
    }
    let mut rest = code;
    for (i, part) in parts.iter().enumerate() {
        if i == 0 {
            let Some(r) = rest.strip_prefix(part) else {
                return false;
            };
            rest = r;
        } else if i == parts.len() - 1 {
            return rest.len() > part.len() && rest.ends_with(part);
        } else {
            let Some(at) = rest.find(part) else {
                return false;
            };
            rest = &rest[at + part.len()..];
        }
    }
    true
}

#[test]
fn у_каждого_действия_из_кода_есть_подпись() {
    let scan = scan();
    assert!(
        scan.unresolved.is_empty(),
        "не удалось понять код действия журнала — передайте его строкой или константой: {:#?}",
        scan.unresolved
    );
    assert!(
        scan.actions.len() > 150,
        "сканер нашёл слишком мало действий: {}",
        scan.actions.len()
    );
    for code in [
        "client.block",
        "subscription.add_days",
        "settings.core",
        "remna.transfer.started",
        "broadcast.started",
    ] {
        assert!(scan.actions.contains_key(code), "сканер не нашёл {code}");
    }
    let missing: Vec<String> = scan
        .actions
        .iter()
        .filter(|(code, _)| labels::label(code).is_none())
        .map(|(code, place)| format!("{code} ({place})"))
        .collect();
    assert!(
        missing.is_empty(),
        "добавьте подписи в audit_journal/labels.rs: {missing:#?}"
    );
    let unmatched: Vec<String> = scan
        .patterns
        .iter()
        .filter(|(pattern, _)| {
            !labels::ACTIONS
                .iter()
                .any(|label| pattern_matches(pattern, label.code))
        })
        .map(|(pattern, place)| format!("{pattern} ({place})"))
        .collect();
    assert!(
        unmatched.is_empty(),
        "для действий, собранных через format!, нет подписей: {unmatched:#?}"
    );
}

#[test]
fn сканер_понимает_обёртки_переменные_и_константы() {
    let file = Source {
        path: "crates/x/src/a.rs".into(),
        text: strip_tests(
            r#"
const DONE_AUDIT: &str = "demo.done";
fn audit(action: &str, admin: i64) -> AuditEntry { AuditEntry { action: action.into(), actor_id: Some(admin) } }
fn kind_action(kind: u8) -> &'static str { match kind { 0 => "demo.first", _ => "demo.second" } }
async fn go() {
    tx.audit().record(&audit("demo.wrapped", 1)).await;
    let action = match x { 1 => "demo.left", _ => return Err(rejected("demo.error", "нет")) };
    tx.audit().record(&audit(action, 2)).await;
    tx.audit().record(&Actor::System.audit(DONE_AUDIT, "x", None, json!({}))).await;
    tx.audit().record(&Actor::System.audit(kind_action(1), "x", None, json!({ "y": "demo.detail" }))).await;
    tx.audit().record(&AuditEntry { action: format!("demo.{}_paid", key), actor_id: None }).await;
    tx.audit().record(&Actor::System.audit(mystery.field, "x", None, json!({}))).await;
}
#[cfg(test)]
mod tests { fn t() { tx.audit().record(&audit("demo.test_only", 1)); } }
"#
            .chars()
            .collect(),
        ),
    };
    let files: &'static [Source] = Box::leak(vec![file].into_boxed_slice());
    let mut scan = Scan::new(files);
    scan.run();
    let codes: Vec<&str> = scan.actions.keys().map(String::as_str).collect();
    assert_eq!(
        codes,
        [
            "demo.done",
            "demo.first",
            "demo.left",
            "demo.second",
            "demo.wrapped"
        ]
    );
    assert_eq!(scan.patterns.keys().collect::<Vec<_>>(), ["demo.{}_paid"]);
    assert_eq!(scan.unresolved.len(), 1, "{:?}", scan.unresolved);
    assert!(scan.unresolved[0].contains("mystery.field"));
    assert!(pattern_matches(
        "payment.{}_review",
        "payment.underpaid_review"
    ));
    assert!(!pattern_matches("payment.{}_review", "payment._review"));
}

#[test]
fn подписи_без_повторов_и_с_разделом() {
    let mut seen = BTreeSet::new();
    for label in labels::ACTIONS {
        assert!(is_code(label.code), "плохой код {}", label.code);
        assert!(seen.insert(label.code), "повтор {}", label.code);
        assert!(!label.title.trim().is_empty());
        assert_eq!(labels::area_of(label.code), Some(label.area));
    }
    assert_eq!(
        labels::title_of("client.something_new"),
        "Действие с клиентом"
    );
    assert_eq!(labels::title_of("zzz.unknown"), "Действие в панели");
}
