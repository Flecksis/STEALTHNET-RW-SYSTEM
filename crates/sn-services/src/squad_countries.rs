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

#[cfg(test)]
mod tests {
    use super::*;
    use sn_remnawave::fake::{FakeRemnawave, Fault, sample_node};
    use sn_remnawave::{Inbound, InternalSquad, NodeConfigProfile, NodeDetails, SquadInfo};

    fn fake(country: &str) -> (FakeRemnawave, Uuid) {
        let squad = Uuid::from_u128(0x5a);
        let inbound = Inbound {
            uuid: Uuid::from_u128(0x1b),
            profile_uuid: None,
            tag: "IN".into(),
            kind: "vless".into(),
            network: Some("tcp".into()),
            security: Some("reality".into()),
            port: Some(443),
        };
        let remna = FakeRemnawave::new();
        remna.set_squads(vec![InternalSquad {
            uuid: squad,
            name: "Main".into(),
            view_position: 0,
            info: SquadInfo::default(),
            inbounds: vec![inbound.clone()],
        }]);
        set_country(&remna, &inbound, country);
        (remna, squad)
    }

    fn set_country(remna: &FakeRemnawave, inbound: &Inbound, country: &str) {
        let mut node = NodeDetails::from(sample_node(Uuid::from_u128(0x2c), "N-1", country));
        node.config_profile = Some(NodeConfigProfile {
            active_config_profile_uuid: None,
            active_inbounds: vec![inbound.clone()],
        });
        remna.set_node_details(vec![node]);
    }

    fn asked(remna: &FakeRemnawave) -> usize {
        remna.calls_of("squad_accessible_nodes").len()
    }

    #[tokio::test]
    async fn страны_сквада_берутся_из_памяти_до_срока_или_сброса() {
        let (remna, squad) = fake("nl");
        let countries = SquadCountries::default();
        let start = Instant::now();
        let first = countries.of(&remna, &[squad], start).await;
        assert_eq!(first[&squad], vec!["NL".to_string()]);
        assert_eq!(asked(&remna), 1);
        let inbound = remna.squads()[0].inbounds[0].clone();
        set_country(&remna, &inbound, "DE");
        let cached = countries
            .of(&remna, &[squad], start + Duration::from_secs(60))
            .await;
        assert_eq!(cached[&squad], vec!["NL".to_string()]);
        assert_eq!(asked(&remna), 1);
        let expired = countries.of(&remna, &[squad], start + TTL).await;
        assert_eq!(expired[&squad], vec!["DE".to_string()]);
        assert_eq!(asked(&remna), 2);
        set_country(&remna, &inbound, "FI");
        countries.forget();
        let after_forget = countries.of(&remna, &[squad], start + TTL).await;
        assert_eq!(after_forget[&squad], vec!["FI".to_string()]);
        assert_eq!(asked(&remna), 3);
    }

    #[tokio::test]
    async fn ошибка_remnawave_не_остаётся_в_памяти() {
        let (remna, squad) = fake("nl");
        let countries = SquadCountries::default();
        let now = Instant::now();
        remna.fail_next(Fault::Unavailable);
        let failed = countries.of(&remna, &[squad], now).await;
        assert!(failed[&squad].is_empty());
        let retried = countries.of(&remna, &[squad], now).await;
        assert_eq!(retried[&squad], vec!["NL".to_string()]);
        assert_eq!(asked(&remna), 2);
        assert_eq!(union(&[squad, squad], &retried), vec!["NL".to_string()]);
    }
}
