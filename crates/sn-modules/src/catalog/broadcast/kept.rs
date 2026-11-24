use axum::body::Bytes;
use std::collections::HashMap;
use std::sync::{LazyLock, Mutex, MutexGuard};
use std::time::{Duration, Instant};
use uuid::Uuid;

pub const TTL: Duration = Duration::from_secs(600);
pub const BUDGET: usize = 32 * 1024 * 1024;

#[derive(Clone)]
struct Entry {
    at: Instant,
    content_type: String,
    bytes: Bytes,
}

#[derive(Default)]
pub struct KeptImages {
    entries: Mutex<HashMap<Uuid, Entry>>,
}

fn guard<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    match mutex.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

impl KeptImages {
    pub fn get(&self, key: Uuid, now: Instant) -> Option<(String, Bytes)> {
        let entries = guard(&self.entries);
        let entry = entries.get(&key)?;
        (now.saturating_duration_since(entry.at) < TTL)
            .then(|| (entry.content_type.clone(), entry.bytes.clone()))
    }

    pub fn put(&self, key: Uuid, content_type: &str, bytes: Bytes, now: Instant) {
        if bytes.len() > BUDGET {
            return;
        }
        let mut entries = guard(&self.entries);
        entries.retain(|_, entry| now.saturating_duration_since(entry.at) < TTL);
        entries.insert(
            key,
            Entry {
                at: now,
                content_type: content_type.to_string(),
                bytes,
            },
        );
        let mut total: usize = entries.values().map(|entry| entry.bytes.len()).sum();
        while total > BUDGET {
            let Some(oldest) = entries
                .iter()
                .filter(|(kept, _)| **kept != key)
                .min_by_key(|(_, entry)| entry.at)
                .map(|(kept, _)| *kept)
            else {
                break;
            };
            if let Some(entry) = entries.remove(&oldest) {
                total -= entry.bytes.len();
            }
        }
    }
}

pub static PUBLIC: LazyLock<KeptImages> = LazyLock::new(KeptImages::default);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn картинка_письма_живёт_в_памяти_до_срока_и_в_пределах_объёма() {
        let kept = KeptImages::default();
        let start = Instant::now();
        let first = Uuid::from_u128(1);
        kept.put(first, "image/png", Bytes::from_static(b"png"), start);
        let (kind, bytes) = kept.get(first, start + Duration::from_secs(60)).unwrap();
        assert_eq!(kind, "image/png");
        assert_eq!(&bytes[..], b"png");
        assert!(kept.get(first, start + TTL).is_none());
        assert!(kept.get(Uuid::from_u128(2), start).is_none());
        let big = Bytes::from(vec![0u8; BUDGET / 2 + 1]);
        kept.put(Uuid::from_u128(3), "image/jpeg", big.clone(), start);
        kept.put(
            Uuid::from_u128(4),
            "image/jpeg",
            big,
            start + Duration::from_secs(1),
        );
        let later = start + Duration::from_secs(2);
        assert!(kept.get(Uuid::from_u128(3), later).is_none());
        assert!(kept.get(Uuid::from_u128(4), later).is_some());
        kept.put(
            Uuid::from_u128(5),
            "image/jpeg",
            Bytes::from(vec![0u8; BUDGET + 1]),
            later,
        );
        assert!(kept.get(Uuid::from_u128(5), later).is_none());
    }
}
