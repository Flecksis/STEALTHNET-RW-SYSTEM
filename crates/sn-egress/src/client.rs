use crate::address::{Proxy, outward};
use crate::handle::{Egress, current};
use crate::plan::Route;
use std::sync::{Arc, Mutex};

pub struct Profile {
    pub name: &'static str,
    pub build: fn() -> reqwest::ClientBuilder,
}

pub fn routed(builder: reqwest::ClientBuilder, proxy: Option<&Proxy>) -> reqwest::ClientBuilder {
    let builder = builder.no_proxy();
    match proxy {
        None => builder,
        Some(proxy) => {
            let target = proxy.url().clone();
            builder.proxy(reqwest::Proxy::custom(move |url| {
                outward(url).then(|| target.clone())
            }))
        }
    }
}

fn closed() -> reqwest::Client {
    reqwest::Client::builder()
        .no_proxy()
        .proxy(reqwest::Proxy::custom(|_| {
            reqwest::Url::parse("socks5h://0.0.0.0:9").ok()
        }))
        .build()
        .unwrap_or_default()
}

pub fn build(builder: reqwest::ClientBuilder, proxy: Option<&Proxy>) -> reqwest::Client {
    let required = proxy.is_some();
    match routed(builder, proxy).build() {
        Ok(client) => client,
        Err(err) => {
            tracing::error!(error = %err.without_url(), "HTTP-клиент не собран");
            if required {
                closed()
            } else {
                reqwest::Client::builder()
                    .no_proxy()
                    .build()
                    .unwrap_or_default()
            }
        }
    }
}

type Built = Option<(Option<Arc<Proxy>>, reqwest::Client)>;

#[derive(Clone)]
pub struct Http {
    profile: &'static Profile,
    route: Route,
    provider: Option<&'static str>,
    egress: Option<Egress>,
    built: Arc<Mutex<Built>>,
}

impl std::fmt::Debug for Http {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Http")
            .field("profile", &self.profile.name)
            .field("route", &self.route)
            .field("provider", &self.provider)
            .finish()
    }
}

impl Http {
    pub fn new(profile: &'static Profile, route: Route, provider: Option<&'static str>) -> Self {
        Http {
            profile,
            route,
            provider,
            egress: None,
            built: Arc::new(Mutex::new(None)),
        }
    }

    pub fn bound(mut self, egress: &Egress) -> Self {
        self.egress = Some(egress.clone());
        self
    }

    pub fn proxy(&self) -> Option<Arc<Proxy>> {
        current(self.egress.as_ref()).proxy_for(self.route, self.provider)
    }

    pub fn client(&self) -> reqwest::Client {
        let proxy = self.proxy();
        let mut built = match self.built.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        if let Some((via, client)) = built.as_ref()
            && *via == proxy
        {
            return client.clone();
        }
        let client = build((self.profile.build)(), proxy.as_deref());
        *built = Some((proxy, client.clone()));
        client
    }

    pub fn get<U: reqwest::IntoUrl>(&self, url: U) -> reqwest::RequestBuilder {
        self.client().get(url)
    }

    pub fn post<U: reqwest::IntoUrl>(&self, url: U) -> reqwest::RequestBuilder {
        self.client().post(url)
    }

    pub fn put<U: reqwest::IntoUrl>(&self, url: U) -> reqwest::RequestBuilder {
        self.client().put(url)
    }

    pub fn patch<U: reqwest::IntoUrl>(&self, url: U) -> reqwest::RequestBuilder {
        self.client().patch(url)
    }

    pub fn delete<U: reqwest::IntoUrl>(&self, url: U) -> reqwest::RequestBuilder {
        self.client().delete(url)
    }

    pub fn request<U: reqwest::IntoUrl>(
        &self,
        method: reqwest::Method,
        url: U,
    ) -> reqwest::RequestBuilder {
        self.client().request(method, url)
    }
}
