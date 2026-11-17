use chrono::{DateTime, NaiveDate, NaiveDateTime, TimeZone, Utc};
use std::collections::{BTreeMap, HashMap};

#[derive(Debug, Default)]
pub struct Table {
    columns: Vec<String>,
    index: HashMap<String, usize>,
    rows: Vec<Vec<Option<Box<str>>>>,
}

impl Table {
    pub fn new(columns: Vec<String>) -> Self {
        let index = columns
            .iter()
            .enumerate()
            .map(|(i, c)| (c.to_ascii_lowercase(), i))
            .collect();
        Table {
            columns,
            index,
            rows: Vec::new(),
        }
    }

    pub fn columns(&self) -> &[String] {
        &self.columns
    }

    pub fn has(&self, column: &str) -> bool {
        self.index.contains_key(column)
    }

    pub fn push(&mut self, values: Vec<Option<Box<str>>>) {
        self.rows.push(values);
    }

    pub fn push_named(&mut self, values: impl IntoIterator<Item = (String, Option<String>)>) {
        let mut row = vec![None; self.columns.len()];
        for (name, value) in values {
            let key = name.to_ascii_lowercase();
            let at = match self.index.get(&key) {
                Some(at) => *at,
                None => {
                    self.columns.push(name);
                    self.index.insert(key, self.columns.len() - 1);
                    for old in &mut self.rows {
                        old.push(None);
                    }
                    row.push(None);
                    self.columns.len() - 1
                }
            };
            row[at] = value.map(String::into_boxed_str);
        }
        self.rows.push(row);
    }

    pub fn len(&self) -> usize {
        self.rows.len()
    }

    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    pub fn rows(&self) -> impl Iterator<Item = Row<'_>> {
        self.rows.iter().map(move |values| Row {
            table: self,
            values,
        })
    }
}

#[derive(Debug, Default)]
pub struct Dump {
    pub tables: BTreeMap<String, Table>,
    pub dumped_at: Option<DateTime<Utc>>,
    pub format: &'static str,
}

impl Dump {
    pub fn table(&self, name: &str) -> Option<&Table> {
        self.tables.get(name)
    }

    pub fn rows(&self, name: &str) -> impl Iterator<Item = Row<'_>> {
        self.tables.get(name).into_iter().flat_map(Table::rows)
    }

    pub fn count(&self, name: &str) -> usize {
        self.tables.get(name).map_or(0, Table::len)
    }
}

#[derive(Clone, Copy)]
pub struct Row<'a> {
    table: &'a Table,
    values: &'a [Option<Box<str>>],
}

impl<'a> Row<'a> {
    pub fn raw(&self, column: &str) -> Option<&'a str> {
        let at = match self.table.index.get(column) {
            Some(at) => *at,
            None => *self.table.index.get(&column.to_ascii_lowercase())?,
        };
        self.values.get(at)?.as_deref()
    }

    pub fn text(&self, column: &str) -> Option<&'a str> {
        self.raw(column).map(str::trim).filter(|v| !v.is_empty())
    }

    pub fn string(&self, column: &str) -> Option<String> {
        self.text(column).map(str::to_string)
    }

    pub fn i64(&self, column: &str) -> Option<i64> {
        let text = self.text(column)?;
        text.parse::<i64>().ok().or_else(|| {
            text.parse::<f64>()
                .ok()
                .filter(|v| v.is_finite() && v.fract() == 0.0 && v.abs() < 9.0e15)
                .map(|v| v as i64)
        })
    }

    pub fn f64(&self, column: &str) -> Option<f64> {
        self.text(column)?
            .parse::<f64>()
            .ok()
            .filter(|v| v.is_finite())
    }

    pub fn bool(&self, column: &str) -> Option<bool> {
        match self.text(column)?.to_ascii_lowercase().as_str() {
            "t" | "true" | "1" | "yes" => Some(true),
            "f" | "false" | "0" | "no" => Some(false),
            _ => None,
        }
    }

    pub fn flag(&self, column: &str) -> bool {
        self.bool(column).unwrap_or(false)
    }

    pub fn time(&self, column: &str) -> Option<DateTime<Utc>> {
        parse_time(self.text(column)?)
    }

    pub fn list(&self, column: &str) -> Vec<String> {
        self.text(column).map(parse_list).unwrap_or_default()
    }

    pub fn json(&self, column: &str) -> Option<serde_json::Value> {
        parse_json(self.text(column)?)
    }
}

pub fn minor_from_float(value: f64) -> Option<i64> {
    let scaled = (value * 100.0).round();
    (scaled.is_finite() && scaled.abs() < 9.0e15).then_some(scaled as i64)
}

pub fn parse_json(text: &str) -> Option<serde_json::Value> {
    let value: serde_json::Value = serde_json::from_str(text).ok()?;
    match &value {
        serde_json::Value::String(inner) if inner.trim_start().starts_with(['{', '[']) => {
            serde_json::from_str(inner).ok().or(Some(value))
        }
        _ => Some(value),
    }
}

pub fn parse_list(text: &str) -> Vec<String> {
    let text = text.trim();
    if text.starts_with('[') || text.starts_with('"') {
        return match parse_json(text) {
            Some(serde_json::Value::Array(items)) => items
                .into_iter()
                .filter_map(|item| match item {
                    serde_json::Value::String(s) => Some(s),
                    serde_json::Value::Null => None,
                    other => Some(other.to_string()),
                })
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect(),
            _ => Vec::new(),
        };
    }
    let Some(inner) = text.strip_prefix('{').and_then(|t| t.strip_suffix('}')) else {
        return Vec::new();
    };
    let mut items = Vec::new();
    let mut current = String::new();
    let mut quoted = false;
    let mut was_quoted = false;
    let mut chars = inner.chars();
    while let Some(c) = chars.next() {
        match c {
            '"' => {
                quoted = !quoted;
                was_quoted = true;
            }
            '\\' if quoted => {
                if let Some(next) = chars.next() {
                    current.push(next);
                }
            }
            ',' if !quoted => {
                push_item(&mut items, &mut current, was_quoted);
                was_quoted = false;
            }
            _ => current.push(c),
        }
    }
    push_item(&mut items, &mut current, was_quoted);
    items
}

fn push_item(items: &mut Vec<String>, current: &mut String, was_quoted: bool) {
    let item = current.trim().to_string();
    current.clear();
    if item.is_empty() || (!was_quoted && item.eq_ignore_ascii_case("NULL")) {
        return;
    }
    items.push(item);
}

pub fn parse_time(text: &str) -> Option<DateTime<Utc>> {
    let text = text.trim();
    if let Ok(at) = DateTime::parse_from_rfc3339(text) {
        return Some(at.with_timezone(&Utc));
    }
    let normalized = text.replacen(' ', "T", 1);
    if let Ok(at) = DateTime::parse_from_rfc3339(&normalized) {
        return Some(at.with_timezone(&Utc));
    }
    for (format, with_zone) in [
        ("%Y-%m-%dT%H:%M:%S%.f%#z", true),
        ("%Y-%m-%dT%H:%M:%S%#z", true),
        ("%Y-%m-%dT%H:%M:%S%.f", false),
        ("%Y-%m-%dT%H:%M:%S", false),
        ("%Y-%m-%dT%H:%M", false),
    ] {
        if with_zone {
            if let Ok(at) = DateTime::parse_from_str(&normalized, format) {
                return Some(at.with_timezone(&Utc));
            }
        } else if let Ok(naive) = NaiveDateTime::parse_from_str(&normalized, format) {
            return Some(Utc.from_utc_datetime(&naive));
        }
    }
    NaiveDate::parse_from_str(text, "%Y-%m-%d")
        .ok()
        .and_then(|d| d.and_hms_opt(0, 0, 0))
        .map(|naive| Utc.from_utc_datetime(&naive))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn table() -> Table {
        let mut table = Table::new(vec!["a".into(), "b".into(), "c".into()]);
        table.push(vec![
            Some("t".into()),
            Some("{abc,\"d,e\",NULL}".into()),
            None,
        ]);
        table.push_named([("d".to_string(), Some("[\"x\",\"y\"]".to_string()))]);
        table
    }

    #[test]
    fn значения_читаются_из_текста_выгрузки() {
        let table = table();
        let rows: Vec<Row<'_>> = table.rows().collect();
        assert_eq!(rows[0].bool("a"), Some(true));
        assert_eq!(rows[0].list("b"), ["abc", "d,e"]);
        assert_eq!(rows[0].text("c"), None);
        assert_eq!(rows[1].list("d"), ["x", "y"]);
        assert_eq!(rows[0].text("d"), None);
    }

    #[test]
    fn время_читается_во_всех_видах() {
        let expect = Utc.with_ymd_and_hms(2026, 5, 13, 4, 0, 0).unwrap();
        for text in [
            "2026-05-13 04:00:00",
            "2026-05-13 04:00:00.000",
            "2026-05-13 07:00:00+03",
            "2026-05-13 07:00:00.000+03:00",
            "2026-05-13T04:00:00Z",
            "2026-05-13T04:00:00.000000+00:00",
            "2026-05-13T04:00:00",
        ] {
            assert_eq!(parse_time(text), Some(expect), "{text}");
        }
        assert_eq!(parse_time("вчера"), None);
    }

    #[test]
    fn деньги_округляются_до_копейки() {
        assert_eq!(minor_from_float(6.99), Some(699));
        assert_eq!(minor_from_float(0.105), Some(11));
        assert_eq!(minor_from_float(1204330.5), Some(120_433_050));
        assert_eq!(minor_from_float(f64::NAN), None);
    }

    #[test]
    fn json_в_строке_разворачивается() {
        assert_eq!(
            parse_json("\"[\\\"a\\\"]\""),
            Some(serde_json::json!(["a"]))
        );
        assert_eq!(
            parse_json("{\"30\": 100}"),
            Some(serde_json::json!({"30": 100}))
        );
    }
}
