use super::query::{Dates, Range, Step};
use chrono::NaiveDate;
use serde::Serialize;
use sn_domain::money::Currency;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Unit {
    Money,
    Stars,
    Count,
    Percent,
    Seconds,
    Bytes,
    Days,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ListKind {
    Payments,
    Clients,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ListRef {
    pub kind: ListKind,
    pub set: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub slice: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub from: Option<NaiveDate>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub to: Option<NaiveDate>,
}

impl ListRef {
    pub fn payments(set: &str) -> ListRef {
        ListRef {
            kind: ListKind::Payments,
            set: set.into(),
            slice: None,
            from: None,
            to: None,
        }
    }

    pub fn clients(set: &str) -> ListRef {
        ListRef {
            kind: ListKind::Clients,
            set: set.into(),
            slice: None,
            from: None,
            to: None,
        }
    }

    pub fn slice(mut self, slice: impl Into<String>) -> ListRef {
        self.slice = Some(slice.into());
        self
    }

    pub fn dates(mut self, from: NaiveDate, to: NaiveDate) -> ListRef {
        self.from = Some(from);
        self.to = Some(to);
        self
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Point {
    pub at: NaiveDate,
    pub value: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct BRow {
    pub key: String,
    pub label: String,
    pub value: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub count: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prev: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub list: Option<ListRef>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Breakdown {
    pub key: &'static str,
    pub title: &'static str,
    pub rows: Vec<BRow>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Column {
    pub title: String,
    pub unit: Unit,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Row {
    pub label: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sub: Option<String>,
    pub cells: Vec<Option<f64>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub list: Option<ListRef>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub children: Vec<Row>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Bar {
    pub label: String,
    pub value: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub list: Option<ListRef>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CohortRow {
    pub month: NaiveDate,
    pub size: i64,
    pub kept: Vec<Option<i64>>,
    pub money: Vec<Option<i64>>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Detail {
    None,
    Series {
        points: Vec<Point>,
        prev: Vec<Point>,
        breakdowns: Vec<Breakdown>,
        #[serde(skip_serializing_if = "Option::is_none")]
        list: Option<ListRef>,
    },
    Table {
        columns: Vec<Column>,
        rows: Vec<Row>,
        #[serde(skip_serializing_if = "Option::is_none")]
        note: Option<String>,
    },
    Bars {
        bars: Vec<Bar>,
        #[serde(skip_serializing_if = "Option::is_none")]
        note: Option<String>,
    },
    Cohorts {
        columns: usize,
        rows: Vec<CohortRow>,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Metric {
    pub key: &'static str,
    pub title: String,
    pub unit: Unit,
    pub value: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prev: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
    pub sub: bool,
    pub detail: Detail,
}

impl Metric {
    pub fn new(key: &'static str, title: &str, unit: Unit, value: Option<f64>) -> Metric {
        Metric {
            key,
            title: title.into(),
            unit,
            value,
            prev: None,
            note: None,
            hint: None,
            sub: false,
            detail: Detail::None,
        }
    }

    pub fn prev(mut self, prev: Option<f64>) -> Metric {
        self.prev = prev;
        self
    }

    pub fn note(mut self, note: impl Into<String>) -> Metric {
        self.note = Some(note.into());
        self
    }

    pub fn note_opt(mut self, note: Option<String>) -> Metric {
        if note.is_some() {
            self.note = note;
        }
        self
    }

    pub fn hint(mut self, hint: impl Into<String>) -> Metric {
        self.hint = Some(hint.into());
        self
    }

    pub fn sub(mut self) -> Metric {
        self.sub = true;
        self
    }

    pub fn detail(mut self, detail: Detail) -> Metric {
        self.detail = detail;
        self
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Group {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    pub metrics: Vec<Metric>,
}

impl Group {
    pub fn new(title: Option<&str>, metrics: Vec<Metric>) -> Group {
        Group {
            title: title.map(str::to_string),
            metrics,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RangeInfo {
    pub from: NaiveDate,
    pub to: NaiveDate,
    pub step: Step,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prev: Option<Dates>,
    pub today: NaiveDate,
}

impl RangeInfo {
    pub fn of(range: &Range) -> RangeInfo {
        RangeInfo {
            from: range.dates.from,
            to: range.dates.to,
            step: range.step,
            prev: range.prev,
            today: range.today,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TabReport {
    pub tab: &'static str,
    pub currency: Currency,
    pub range: RangeInfo,
    pub groups: Vec<Group>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<String>,
    pub preparing: bool,
}

impl TabReport {
    pub fn metric(&self, key: &str) -> Option<&Metric> {
        self.groups
            .iter()
            .flat_map(|g| g.metrics.iter())
            .find(|m| m.key == key)
    }
}

pub fn points(labels: &[NaiveDate], values: &[f64]) -> Vec<Point> {
    labels
        .iter()
        .zip(values.iter())
        .map(|(at, value)| Point {
            at: *at,
            value: *value,
        })
        .collect()
}

pub fn per_bucket(n: usize, rows: impl IntoIterator<Item = (i32, f64)>) -> Vec<f64> {
    let mut out = vec![0.0; n];
    for (bucket, value) in rows {
        if let Ok(i) = usize::try_from(bucket - 1)
            && i < n
        {
            out[i] += value;
        }
    }
    out
}

pub fn share(part: f64, whole: f64) -> Option<f64> {
    (whole > 0.0).then(|| round1(part / whole * 100.0))
}

pub fn ratio(part: f64, whole: f64) -> Option<f64> {
    (whole > 0.0).then(|| (part / whole).round())
}

pub fn round1(value: f64) -> f64 {
    (value * 10.0).round() / 10.0
}

pub fn num(value: i64) -> f64 {
    value as f64
}

pub fn pct_text(value: f64) -> String {
    let rounded = round1(value);
    let text = if rounded.fract() == 0.0 {
        format!("{rounded:.0}")
    } else {
        format!("{rounded:.1}").replace('.', ",")
    };
    format!("{text}\u{a0}%")
}
