use sn_remnawave::{RemnaError, RemnaResult, RemnaUser, RemnawaveApi, UsersStreamQuery};
use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Pace {
    pub page: u32,
    pub batch: usize,
    pub pause: Duration,
    pub attempts: u32,
    pub backoff: Duration,
}

impl Default for Pace {
    fn default() -> Self {
        Pace {
            page: 500,
            batch: 25,
            pause: Duration::from_millis(300),
            attempts: 5,
            backoff: Duration::from_secs(1),
        }
    }
}

impl Pace {
    pub const fn quick() -> Self {
        Pace {
            page: 2,
            batch: 2,
            pause: Duration::ZERO,
            attempts: 3,
            backoff: Duration::ZERO,
        }
    }
}

const BACKOFF_MAX: Duration = Duration::from_secs(30);

pub(crate) async fn retry<T, F, Fut>(pace: &Pace, mut call: F) -> RemnaResult<T>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = RemnaResult<T>>,
{
    let mut wait = pace.backoff;
    let mut attempt = 1;
    loop {
        match call().await {
            Err(err) if err.is_retryable() && attempt < pace.attempts.max(1) => {
                if !wait.is_zero() {
                    tokio::time::sleep(wait).await;
                }
                wait = (wait * 2).min(BACKOFF_MAX);
                attempt += 1;
            }
            other => return other,
        }
    }
}

pub(crate) fn panel_down(err: &RemnaError) -> bool {
    err.is_retryable()
        || matches!(
            err,
            RemnaError::Unauthorized(_) | RemnaError::Redirected(_) | RemnaError::NotConfigured(_)
        )
}

#[derive(Default)]
pub(crate) struct RemnaIndex {
    users: Vec<RemnaUser>,
    by_id: HashMap<i64, usize>,
    by_short: HashMap<String, usize>,
    by_name: HashMap<String, usize>,
}

impl RemnaIndex {
    pub async fn load(remna: &dyn RemnawaveApi, pace: &Pace) -> RemnaResult<Self> {
        let mut index = RemnaIndex::default();
        let mut cursor: Option<String> = None;
        let mut seen: HashSet<String> = HashSet::new();
        loop {
            let query = UsersStreamQuery {
                cursor: cursor.clone(),
                size: Some(pace.page.clamp(1, 1000)),
                ..UsersStreamQuery::default()
            };
            let page = retry(pace, || remna.stream_users(&query)).await?;
            for user in page.users {
                index.put(user);
            }
            match page.next_cursor {
                Some(next) if page.has_more && seen.insert(next.clone()) => cursor = Some(next),
                _ => break,
            }
        }
        Ok(index)
    }

    pub fn put(&mut self, user: RemnaUser) {
        if let Some(&at) = self.by_id.get(&user.id.0) {
            let old = &self.users[at];
            if self.by_short.get(&old.short_uuid) == Some(&at) {
                self.by_short.remove(&old.short_uuid);
            }
            if self.by_name.get(&old.username) == Some(&at) {
                self.by_name.remove(&old.username);
            }
            self.by_short.insert(user.short_uuid.clone(), at);
            self.by_name.insert(user.username.clone(), at);
            self.users[at] = user;
            return;
        }
        let at = self.users.len();
        self.by_id.insert(user.id.0, at);
        self.by_short.insert(user.short_uuid.clone(), at);
        self.by_name.insert(user.username.clone(), at);
        self.users.push(user);
    }

    pub fn len(&self) -> usize {
        self.users.len()
    }

    pub fn users(&self) -> &[RemnaUser] {
        &self.users
    }

    pub fn by_id(&self, id: i64) -> Option<&RemnaUser> {
        self.by_id.get(&id).map(|&at| &self.users[at])
    }

    pub fn by_short(&self, short: &str) -> Option<&RemnaUser> {
        if short.is_empty() {
            return None;
        }
        self.by_short.get(short).map(|&at| &self.users[at])
    }

    pub fn by_name(&self, name: &str) -> Option<&RemnaUser> {
        self.by_name.get(name).map(|&at| &self.users[at])
    }
}
