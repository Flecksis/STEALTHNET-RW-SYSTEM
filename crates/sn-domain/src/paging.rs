use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PageRequest {
    pub limit: u32,
    pub after: Option<i64>,
}

impl PageRequest {
    pub const DEFAULT_LIMIT: u32 = 50;
    pub const MAX_LIMIT: u32 = 200;

    pub fn new(limit: Option<u32>, after: Option<i64>) -> Self {
        PageRequest {
            limit: limit
                .unwrap_or(Self::DEFAULT_LIMIT)
                .clamp(1, Self::MAX_LIMIT),
            after,
        }
    }

    pub const fn fetch_limit(&self) -> i64 {
        self.limit as i64 + 1
    }
}

impl Default for PageRequest {
    fn default() -> Self {
        PageRequest::new(None, None)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Page<T> {
    pub items: Vec<T>,
    pub next: Option<i64>,
}

impl<T> Page<T> {
    pub fn from_overfetch(
        mut items: Vec<T>,
        request: PageRequest,
        key: impl Fn(&T) -> i64,
    ) -> Self {
        let next = if items.len() > request.limit as usize {
            items.truncate(request.limit as usize);
            items.last().map(&key)
        } else {
            None
        };
        Page { items, next }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn страница_с_курсором() {
        let request = PageRequest::new(Some(2), None);
        let page = Page::from_overfetch(vec![10_i64, 9, 8], request, |v| *v);
        assert_eq!(page.items, [10, 9]);
        assert_eq!(page.next, Some(9));
        let last = Page::from_overfetch(vec![7_i64], request, |v| *v);
        assert_eq!(last.next, None);
        assert_eq!(
            PageRequest::new(Some(10_000), None).limit,
            PageRequest::MAX_LIMIT
        );
    }
}
