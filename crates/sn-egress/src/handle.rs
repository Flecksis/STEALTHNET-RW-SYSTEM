use crate::address::Proxy;
use crate::plan::{Plan, Route};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock, RwLock};

struct Shared {
    plan: RwLock<Arc<Plan>>,
    down: AtomicBool,
}

#[derive(Clone)]
pub struct Egress {
    shared: Arc<Shared>,
}

impl Default for Egress {
    fn default() -> Self {
        Egress::new()
    }
}

impl std::fmt::Debug for Egress {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Egress")
            .field("plan", &self.plan())
            .field("down", &self.is_down())
            .finish()
    }
}

static GLOBAL: OnceLock<Egress> = OnceLock::new();

impl Egress {
    pub fn new() -> Self {
        Egress {
            shared: Arc::new(Shared {
                plan: RwLock::new(Arc::new(Plan::direct())),
                down: AtomicBool::new(false),
            }),
        }
    }

    pub fn install(&self) -> bool {
        GLOBAL.set(self.clone()).is_ok()
    }

    pub fn global() -> Option<&'static Egress> {
        GLOBAL.get()
    }

    pub fn plan(&self) -> Arc<Plan> {
        match self.shared.plan.read() {
            Ok(guard) => guard.clone(),
            Err(poisoned) => poisoned.into_inner().clone(),
        }
    }

    pub fn set_plan(&self, plan: Plan) -> bool {
        let next = Arc::new(plan);
        let changed = {
            let mut guard = match self.shared.plan.write() {
                Ok(guard) => guard,
                Err(poisoned) => poisoned.into_inner(),
            };
            let changed = **guard != *next;
            *guard = next;
            changed
        };
        if changed {
            self.set_down(false);
        }
        changed
    }

    pub fn proxy_for(&self, route: Route, provider: Option<&str>) -> Option<Arc<Proxy>> {
        current(Some(self)).proxy_for(route, provider)
    }

    pub fn is_down(&self) -> bool {
        self.shared.down.load(Ordering::Relaxed)
    }

    pub fn set_down(&self, down: bool) {
        self.shared.down.store(down, Ordering::Relaxed);
    }

    pub fn down_for(&self, route: Route) -> bool {
        self.is_down() && self.plan().uses(route)
    }
}

pub fn current(bound: Option<&Egress>) -> Arc<Plan> {
    match bound {
        Some(egress) => egress.plan(),
        None => Egress::global().map_or_else(|| Arc::new(Plan::direct()), Egress::plan),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plan::{ProxySettings, Source};

    fn plan(server: &str) -> Plan {
        Plan::from_panel(
            &ProxySettings {
                enabled: true,
                server: Some(server.into()),
                routes: [Route::Telegram].into_iter().collect(),
                ..ProxySettings::default()
            },
            None,
        )
    }

    #[test]
    fn план_меняется_только_при_отличии() {
        let egress = Egress::new();
        assert!(egress.set_plan(plan("socks5://203.0.113.5:1080")));
        assert!(!egress.set_plan(plan("socks5://203.0.113.5:1080")));
        assert_eq!(
            egress.proxy_for(Route::Telegram, None).unwrap().to_string(),
            "socks5://203.0.113.5:1080"
        );
        assert_eq!(egress.plan().source, Source::Panel);
        assert!(Egress::global().is_none());
    }

    #[test]
    fn сбой_сбрасывается_при_смене_плана() {
        let egress = Egress::new();
        egress.set_plan(plan("socks5://203.0.113.5:1080"));
        egress.set_down(true);
        assert!(egress.down_for(Route::Telegram));
        assert!(!egress.down_for(Route::Payments));
        egress.set_plan(plan("socks5://203.0.113.6:1080"));
        assert!(!egress.is_down());
    }
}
