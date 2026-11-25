use super::*;
use crate::auth::{INIT_DATA_AHEAD_SECONDS, INIT_DATA_MAX_AGE_SECONDS, sign_init_data};
use crate::server_clock::{Drift, Evidence, STATE_KEY};

const BOT: &str = "123456:TEST-token";
const OTHER_BOT: &str = "654321:OTHER-token";

async fn mini_env() -> Option<Env> {
    env_with_fallbacks(
        ModuleRegistry::catalog_only(),
        EnvFallbacks {
            bot_token: Some(BOT.into()),
            ..EnvFallbacks::default()
        },
    )
    .await
}

fn encoded(value: &str) -> String {
    value
        .bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() {
                char::from(b).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect()
}

fn signed(telegram_id: i64, opened: DateTime<Utc>, bot: &str) -> String {
    let user = format!(r#"{{"id":{telegram_id},"first_name":"Алексей","language_code":"ru"}}"#);
    let auth_date = opened.timestamp().to_string();
    let hash = sign_init_data(&[("auth_date", &auth_date), ("user", &user)], bot);
    format!("auth_date={auth_date}&user={}&hash={hash}", encoded(&user))
}

impl Env {
    async fn open(&self, telegram_id: i64, opened: DateTime<Utc>) -> Result<(), (String, String)> {
        self.services
            .auth()
            .miniapp_login(
                &signed(telegram_id, opened, BOT),
                &RequestMeta {
                    ip: Some("203.0.113.40".into()),
                    user_agent: None,
                },
            )
            .await
            .map(|_| ())
            .map_err(|err| (err.code().to_string(), err.to_string()))
    }

    fn runs_ahead(&self, seconds: i64) {
        self.clock.set(start() + Duration::seconds(seconds));
    }

    fn telegram_answers(&self) {
        self.services.server_clock().answered(start());
    }

    async fn saved(&self) -> Option<serde_json::Value> {
        let mut conn = self.services.store().conn().await.unwrap();
        conn.bot_state().get(STATE_KEY).await.unwrap()
    }

    async fn notice(&self) -> Option<Drift> {
        self.services.server_clock_ops().notice().await.unwrap()
    }

    async fn report(&self) {
        self.services.server_clock_ops().report().await.unwrap();
    }

    async fn shift_saved(&self, shift: &str) {
        let mut conn = self.services.store().conn().await.unwrap();
        let changed = sqlx::query(
            "UPDATE bot_state SET updated_at = clock_timestamp() + $2::interval WHERE key = $1",
        )
        .bind(STATE_KEY)
        .bind(shift)
        .execute(conn.conn())
        .await
        .unwrap();
        assert_eq!(changed.rows_affected(), 1);
    }
}

fn old_data() -> Result<(), (String, String)> {
    Err((
        "auth.init_data_old".into(),
        "Войти не получилось: данные от Telegram устарели. Закройте мини-приложение и откройте его снова из бота. Если не помогло, напишите в поддержку.".into(),
    ))
}

fn server_clock() -> Result<(), (String, String)> {
    Err((
        "auth.server_clock".into(),
        "Войти сейчас не получается: на сервере неверное время. Это сбой на нашей стороне. Попробуйте позже или напишите в поддержку.".into(),
    ))
}

#[tokio::test]
async fn пороги_входа_в_мини_приложение_прежние() {
    let Some(env) = mini_env().await else { return };
    assert_eq!(
        (INIT_DATA_MAX_AGE_SECONDS, INIT_DATA_AHEAD_SECONDS),
        (3_600, 60)
    );
    for (seconds, passes) in [(3_600, true), (3_601, false), (-60, true), (-61, false)] {
        let opened = start() - Duration::seconds(seconds);
        assert_eq!(
            env.open(1_001, opened).await.is_ok(),
            passes,
            "данным {seconds} с"
        );
    }
    env.db.cleanup().await;
}

#[tokio::test]
async fn клиенту_с_давно_открытым_приложением_советуют_открыть_его_снова() {
    let Some(env) = mini_env().await else { return };
    env.telegram_answers();
    env.telegram_answers();
    assert_eq!(
        env.open(1_001, start() - Duration::hours(2)).await,
        old_data()
    );
    assert_eq!(env.open(1_001, start()).await, Ok(()));
    env.report().await;
    assert_eq!(env.notice().await, None);
    env.db.cleanup().await;
}

#[tokio::test]
async fn при_спешащих_часах_клиент_видит_что_дело_в_сервере_а_оператор_насколько() {
    let Some(env) = mini_env().await else { return };
    env.runs_ahead(7_200);
    env.telegram_answers();
    env.telegram_answers();
    assert_eq!(env.open(1_001, start()).await, server_clock());
    assert_eq!(
        env.notice().await,
        None,
        "запись делает раз в минуту фоновая работа"
    );
    env.report().await;
    assert_eq!(
        env.notice().await,
        Some(Drift {
            seconds: 7_200,
            evidence: Evidence::Telegram
        })
    );
    env.runs_ahead(0);
    env.telegram_answers();
    assert_eq!(env.open(1_001, start()).await, Ok(()));
    env.report().await;
    assert_eq!(env.saved().await, None);
    env.db.cleanup().await;
}

#[tokio::test]
async fn при_отстающих_часах_клиент_видит_что_дело_в_сервере_и_без_ответов_телеграма() {
    let Some(env) = mini_env().await else { return };
    env.runs_ahead(-300);
    assert_eq!(env.open(1_001, start()).await, server_clock());
    env.report().await;
    assert_eq!(
        env.notice().await,
        Some(Drift {
            seconds: -300,
            evidence: Evidence::Login
        })
    );
    env.db.cleanup().await;
}

#[tokio::test]
async fn без_ответов_телеграма_спешащие_часы_видны_по_отказам_подряд() {
    let Some(env) = mini_env().await else { return };
    env.runs_ahead(7_200);
    assert_eq!(env.open(1_001, start()).await, old_data());
    assert_eq!(
        env.open(1_002, start() + Duration::seconds(5)).await,
        old_data()
    );
    env.report().await;
    assert_eq!(env.notice().await, None);
    assert_eq!(env.open(1_003, start()).await, server_clock());
    env.report().await;
    assert_eq!(
        env.notice().await,
        Some(Drift {
            seconds: 7_195,
            evidence: Evidence::Logins
        })
    );
    env.runs_ahead(0);
    assert_eq!(env.open(1_001, start()).await, Ok(()));
    env.report().await;
    assert_eq!(env.notice().await, None);
    env.db.cleanup().await;
}

#[tokio::test]
async fn чужая_подпись_о_часах_ничего_не_говорит_и_клиент_видит_понятный_текст() {
    let Some(env) = mini_env().await else { return };
    let who = RequestMeta::default();
    let auth = env.services.auth();
    let future = start() + Duration::hours(2);
    for data in [
        signed(1_001, future, OTHER_BOT),
        "auth_date=1&user=%7B%7D".to_string(),
        "%".to_string(),
        format!("{}&hash=00", signed(1_001, future, BOT)),
    ] {
        let err = auth.miniapp_login(&data, &who).await.unwrap_err();
        assert_eq!(err.code(), "auth.init_data", "{data}");
        assert_eq!(
            err.to_string(),
            "Войти не получилось: данные от Telegram не подошли. Закройте мини-приложение и откройте его снова из бота. Если не помогло, напишите в поддержку."
        );
    }
    env.report().await;
    assert_eq!(env.saved().await, None);
    env.db.cleanup().await;
}

#[tokio::test]
async fn запись_о_часах_видна_пятнадцать_минут_и_не_застревает_в_будущем() {
    let Some(env) = mini_env().await else { return };
    env.runs_ahead(7_200);
    env.telegram_answers();
    env.telegram_answers();
    env.report().await;
    assert!(env.notice().await.is_some());
    env.shift_saved("-14 minutes").await;
    assert!(env.notice().await.is_some());
    env.shift_saved("-16 minutes").await;
    assert_eq!(env.notice().await, None, "копия давно молчит");
    env.shift_saved("2 hours").await;
    assert_eq!(env.notice().await, None, "часы базы тоже перевели назад");
    env.report().await;
    assert!(env.notice().await.is_some(), "расхождение ещё есть");
    env.db.cleanup().await;
}

#[tokio::test]
async fn исправная_копия_не_прячет_предупреждение_другой_копии() {
    let Some(env) = mini_env().await else { return };
    env.runs_ahead(7_200);
    env.telegram_answers();
    env.telegram_answers();
    env.report().await;
    let exact = Services::new(ServiceDeps {
        store: env.db.store.clone(),
        payments: Arc::new(ProviderRegistry::builtin()),
        modules: Arc::new(ModuleRegistry::catalog_only()),
        secrets: None,
        clock: Arc::new(FixedClock::new(start())),
        env: EnvFallbacks::default(),
    });
    exact.server_clock().answered(start());
    exact.server_clock_ops().report().await.unwrap();
    env.report().await;
    for _ in 0..3 {
        exact.server_clock().answered(start());
        exact.server_clock().admitted();
        exact.server_clock_ops().report().await.unwrap();
    }
    assert_eq!(
        exact.server_clock_ops().notice().await.unwrap(),
        Some(Drift {
            seconds: 7_200,
            evidence: Evidence::Telegram
        }),
        "видно из любой копии"
    );
    env.db.cleanup().await;
}
