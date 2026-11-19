use sn_remnawave::{RemnawaveApi, SquadAccessibleNodes};
use std::collections::HashMap;
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant};
use uuid::Uuid;

pub const TTL: Duration = Duration::from_secs(600);

#[derive(Default)]
pub struct SquadCountries {
    known: Mutex<HashMap<Uuid, (Instant, Vec<String>)>>,
}

fn guard<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    match mutex.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

fn codes_of(nodes: SquadAccessibleNodes) -> Vec<String> {
    let mut codes: Vec<String> = Vec::new();
    for node in nodes.accessible_nodes {
        let code = node.country_code.to_uppercase();
        if !code.is_empty() && code != "XX" && !codes.contains(&code) {
            codes.push(code);
        }
    }
    codes
}

pub fn union(squads: &[Uuid], map: &HashMap<Uuid, Vec<String>>) -> Vec<String> {
    let mut codes: Vec<String> = Vec::new();
    for squad in squads {
        for code in map.get(squad).into_iter().flatten() {
            if !codes.contains(code) {
                codes.push(code.clone());
            }
        }
    }
    codes
}

impl SquadCountries {
    pub async fn of(
        &self,
        remna: &dyn RemnawaveApi,
        squads: &[Uuid],
        now: Instant,
    ) -> HashMap<Uuid, Vec<String>> {
        let mut found = HashMap::new();
        let mut missing: Vec<Uuid> = Vec::new();
        {
            let known = guard(&self.known);
            for squad in squads {
                match known.get(squad) {
                    Some((at, codes)) if now.saturating_duration_since(*at) < TTL => {
                        found.insert(*squad, codes.clone());
                    }
                    _ if !missing.contains(squad) => missing.push(*squad),
                    _ => {}
                }
            }
        }
        for squad in missing {
            match remna.squad_accessible_nodes(squad).await {
                Ok(nodes) => {
                    let codes = codes_of(nodes);
                    guard(&self.known).insert(squad, (now, codes.clone()));
                    found.insert(squad, codes);
                }
                Err(err) => {
                    tracing::warn!(squad = %squad, error = %err, "страны сквада не получены");
                    found.insert(squad, Vec::new());
                }
            }
        }
        found
    }

    pub fn forget(&self) {
        guard(&self.known).clear();
    }
}
