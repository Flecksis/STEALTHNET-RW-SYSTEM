use super::table::{Dump, Table, parse_time};
use serde::de::{DeserializeSeed, Deserializer, IgnoredAny, MapAccess, SeqAccess, Visitor};
use std::fmt;

fn text_of(value: serde_json::Value) -> Option<String> {
    match value {
        serde_json::Value::Null => None,
        serde_json::Value::Bool(true) => Some("t".into()),
        serde_json::Value::Bool(false) => Some("f".into()),
        serde_json::Value::Number(n) => Some(n.to_string()),
        serde_json::Value::String(s) => Some(s),
        other => Some(other.to_string()),
    }
}

struct Rows<'a> {
    table: &'a mut Table,
}

impl<'de> DeserializeSeed<'de> for Rows<'_> {
    type Value = ();

    fn deserialize<D: Deserializer<'de>>(self, deserializer: D) -> Result<(), D::Error> {
        deserializer.deserialize_seq(self)
    }
}

impl<'de> Visitor<'de> for Rows<'_> {
    type Value = ();

    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("список строк таблицы")
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<(), A::Error> {
        while let Some(row) = seq.next_element::<serde_json::Map<String, serde_json::Value>>()? {
            self.table
                .push_named(row.into_iter().map(|(k, v)| (k, text_of(v))));
        }
        Ok(())
    }
}

struct Tables<'a> {
    wanted: &'a dyn Fn(&str) -> bool,
    dump: &'a mut Dump,
}

impl<'de> DeserializeSeed<'de> for Tables<'_> {
    type Value = ();

    fn deserialize<D: Deserializer<'de>>(self, deserializer: D) -> Result<(), D::Error> {
        deserializer.deserialize_map(self)
    }
}

impl<'de> Visitor<'de> for Tables<'_> {
    type Value = ();

    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("таблицы выгрузки")
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<(), A::Error> {
        while let Some(name) = map.next_key::<String>()? {
            if (self.wanted)(&name) {
                let table = self.dump.tables.entry(name).or_default();
                map.next_value_seed(Rows { table })?;
            } else {
                map.next_value::<IgnoredAny>()?;
            }
        }
        Ok(())
    }
}

struct Top<'a> {
    wanted: &'a dyn Fn(&str) -> bool,
    dump: &'a mut Dump,
    tables: usize,
}

impl<'de> Visitor<'de> for &mut Top<'_> {
    type Value = ();

    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("выгрузку в формате JSON")
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<(), A::Error> {
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "metadata" => {
                    let meta: serde_json::Value = map.next_value()?;
                    if self.dump.dumped_at.is_none() {
                        self.dump.dumped_at = meta
                            .get("timestamp")
                            .and_then(serde_json::Value::as_str)
                            .and_then(parse_time);
                    }
                }
                "data" => {
                    self.tables += 1;
                    map.next_value_seed(Tables {
                        wanted: self.wanted,
                        dump: self.dump,
                    })?;
                }
                _ => {
                    map.next_value::<IgnoredAny>()?;
                }
            }
        }
        Ok(())
    }
}

pub fn read(
    reader: impl std::io::Read,
    wanted: &dyn Fn(&str) -> bool,
    dump: &mut Dump,
) -> Result<bool, serde_json::Error> {
    let mut top = Top {
        wanted,
        dump,
        tables: 0,
    };
    let mut de = serde_json::Deserializer::from_reader(reader);
    (&mut de).deserialize_map(&mut top)?;
    Ok(top.tables > 0)
}

pub fn read_metadata(reader: impl std::io::Read, dump: &mut Dump) {
    if let Ok(meta) = serde_json::from_reader::<_, serde_json::Value>(reader)
        && dump.dumped_at.is_none()
    {
        dump.dumped_at = meta
            .get("timestamp")
            .and_then(serde_json::Value::as_str)
            .and_then(parse_time);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn нужные_таблицы_читаются_потоком() {
        let text = r#"{"metadata":{"timestamp":"2026-11-16T10:00:00+00:00","version":"orm-1.0"},
            "data":{"users":[{"id":1,"telegram_id":555,"username":"ann","status":"active","balance_kopeks":15000,"has_had_paid_subscription":true},
                             {"id":2,"telegram_id":null,"email":"b@example.com","extra":"x"}],
                    "monitoring_logs":[{"id":1,"message":"шум"}],
                    "tariffs":[{"id":3,"period_prices":"{\"30\": 19900}","allowed_squads":"[\"a\"]"}]},
            "associations":{"x":[]}}"#;
        let mut dump = Dump::default();
        assert!(read(text.as_bytes(), &|t| t != "monitoring_logs", &mut dump).unwrap());
        let users: Vec<_> = dump.rows("users").collect();
        assert_eq!(users.len(), 2);
        assert_eq!(users[0].i64("telegram_id"), Some(555));
        assert_eq!(users[0].bool("has_had_paid_subscription"), Some(true));
        assert_eq!(users[1].text("telegram_id"), None);
        assert_eq!(users[1].text("email"), Some("b@example.com"));
        assert_eq!(users[1].text("extra"), Some("x"));
        assert!(dump.table("monitoring_logs").is_none());
        let tariff = dump.rows("tariffs").next().unwrap();
        assert_eq!(
            tariff.json("period_prices"),
            Some(serde_json::json!({"30": 19900}))
        );
        assert_eq!(tariff.list("allowed_squads"), ["a"]);
        assert!(dump.dumped_at.is_some());
    }
}
