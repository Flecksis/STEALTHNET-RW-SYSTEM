use super::labels::Names;
use super::metric::{Point, points};
use super::query::{Filters, Range, Sql, grid, spread};
use crate::context::PaymentTitle;
use chrono::{DateTime, NaiveDate, Utc};
use sn_domain::money::Currency;
use sn_store::{Store, StoreResult};

pub struct Ctx<'a> {
    pub store: &'a Store,
    pub range: Range,
    pub filters: Filters,
    pub currency: Currency,
    pub titles: Vec<PaymentTitle>,
    pub names: Names,
}

#[derive(Debug, Clone)]
pub struct Window {
    pub bounds: Vec<DateTime<Utc>>,
    pub labels: Vec<NaiveDate>,
}

impl Window {
    pub fn start(&self) -> DateTime<Utc> {
        self.bounds[0]
    }

    pub fn end(&self) -> DateTime<Utc> {
        *self.bounds.last().unwrap_or(&self.bounds[0])
    }

    pub fn len(&self) -> usize {
        self.bounds.len().saturating_sub(1)
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl Ctx<'_> {
    pub fn now(&self) -> Window {
        Window {
            bounds: self.range.bounds(),
            labels: self.range.labels(),
        }
    }

    pub fn before(&self) -> Option<Window> {
        Some(Window {
            bounds: self.range.prev_bounds()?,
            labels: self.range.prev_labels()?,
        })
    }

    pub async fn series(
        &self,
        w: &Window,
        table: &str,
        time: &str,
        cond: impl Fn(&mut Sql) -> String,
        values: &[&str],
    ) -> StoreResult<Series> {
        let mut sql = Sql::default();
        let b = sql.times(w.bounds.clone());
        let s = sql.time(w.start());
        let e = sql.time(w.end());
        let c = cond(&mut sql);
        let cols = values
            .iter()
            .map(|v| format!("COALESCE({v}, 0)::float8"))
            .collect::<Vec<_>>()
            .join(", ");
        let text = format!(
            "SELECT width_bucket({time}, {b}) AS b, {cols} FROM {table} \
             WHERE {time} >= {s} AND {time} < {e} AND {c} GROUP BY 1"
        );
        let mut conn = self.store.conn().await?;
        let rows = grid(&mut conn, &sql, &text, values.len()).await?;
        Ok(Series::from_rows(w, &rows, values.len()))
    }
}

#[derive(Debug, Clone, Default)]
pub struct Series {
    pub columns: Vec<Vec<f64>>,
    pub labels: Vec<NaiveDate>,
}

impl Series {
    pub fn from_rows(w: &Window, rows: &[(i32, Vec<f64>)], values: usize) -> Series {
        Series {
            columns: (0..values).map(|i| spread(w.len(), rows, i)).collect(),
            labels: w.labels.clone(),
        }
    }

    pub fn total(&self, i: usize) -> f64 {
        self.columns.get(i).map_or(0.0, |c| c.iter().sum())
    }

    pub fn points(&self, i: usize) -> Vec<Point> {
        self.columns
            .get(i)
            .map_or_else(Vec::new, |c| points(&self.labels, c))
    }
}

pub async fn both(
    c: &Ctx<'_>,
    table: &str,
    time: &str,
    cond: impl Fn(&mut Sql) -> String + Copy,
    values: &[&str],
) -> StoreResult<(Series, Option<Series>)> {
    let current = c.now();
    let now = c.series(&current, table, time, cond, values);
    match c.before() {
        Some(w) => {
            let (a, b) = tokio::try_join!(now, c.series(&w, table, time, cond, values))?;
            Ok((a, Some(b)))
        }
        None => Ok((now.await?, None)),
    }
}

pub fn prev_total(prev: &Option<Series>, i: usize) -> Option<f64> {
    prev.as_ref().map(|p| p.total(i))
}

pub fn prev_points(prev: &Option<Series>, i: usize) -> Vec<Point> {
    prev.as_ref().map_or_else(Vec::new, |p| p.points(i))
}
