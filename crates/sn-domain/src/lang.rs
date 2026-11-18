use crate::client::Locale;
use std::future::Future;

tokio::task_local! {
    static LOCALE: Locale;
}

pub fn current() -> Locale {
    LOCALE.try_with(|l| *l).unwrap_or_default()
}

pub fn hint() -> Option<Locale> {
    LOCALE.try_with(|l| *l).ok()
}

pub async fn scope<F: Future>(locale: Locale, f: F) -> F::Output {
    LOCALE.scope(locale, f).await
}

pub fn sync_scope<R>(locale: Locale, f: impl FnOnce() -> R) -> R {
    LOCALE.sync_scope(locale, f)
}

pub fn pick<'a>(ru: &'a str, en: &'a str) -> &'a str {
    match current() {
        Locale::Ru => ru,
        Locale::En => en,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn вне_запроса_язык_русский() {
        assert_eq!(current(), Locale::Ru);
        assert_eq!(hint(), None);
        assert_eq!(sync_scope(Locale::Ru, hint), Some(Locale::Ru));
        assert_eq!(pick("да", "yes"), "да");
    }

    #[test]
    fn язык_действует_только_внутри_хода() {
        let inside = sync_scope(Locale::En, || (current(), pick("да", "yes")));
        assert_eq!(inside, (Locale::En, "yes"));
        assert_eq!(current(), Locale::Ru);
    }

    #[tokio::test]
    async fn язык_переживает_ожидание_внутри_хода() {
        let seen = scope(Locale::En, async {
            tokio::task::yield_now().await;
            current()
        })
        .await;
        assert_eq!(seen, Locale::En);
    }
}
