use super::*;
use sn_domain::ledger::{LedgerKind, Wallet};
use sn_domain::money::Money;
use sn_domain::payment::providers;
use sn_domain::settings::keys;
use sn_modules::{BalanceChange, CoreApi};

async fn balance_off(services: &Services) {
    {
        let mut conn = services.store().conn().await.unwrap();
        conn.settings()
            .set(
                &keys::provider(providers::BALANCE),
                &serde_json::json!({"enabled": false}),
                None,
            )
            .await
            .unwrap();
    }
    services.reload().await.unwrap();
}

#[tokio::test]
async fn выключенный_баланс_прячет_кнопку_и_пополнение_в_боте() {
    let Some((db, services, server, ctx)) = setup().await else {
        return;
    };
    balance_off(&services).await;
    handle_update(&ctx, text_update(1, 611, "/start")).await;
    let buttons = last_buttons(&server).await;
    assert!(!buttons.contains(&"bal".to_string()), "{buttons:?}");
    assert!(buttons.contains(&"buy".to_string()), "{buttons:?}");
    for (update, data) in [(2, "top:1000"), (3, "topx"), (4, "tp:cryptopay")] {
        handle_update(&ctx, callback_update(update, 611, data)).await;
        let text = last_text(&server).await;
        assert!(
            text.contains("Пополнить баланс сейчас нельзя"),
            "{data}: {text}"
        );
        let buttons = last_buttons(&server).await;
        assert!(
            buttons.iter().all(|b| !b.starts_with("top")),
            "{data}: {buttons:?}"
        );
    }
    let client = client_of(&services, 611).await.unwrap();
    services
        .change_balance(BalanceChange {
            client_id: client.id,
            wallet: Wallet::Main,
            amount: Money::new(300, services.settings().currency),
            kind: LedgerKind::TopUp,
            comment: None,
            idempotency_key: "bot-balance-off".into(),
            order_id: None,
            payment_id: None,
        })
        .await
        .unwrap();
    handle_update(&ctx, text_update(5, 611, "/start")).await;
    assert!(last_buttons(&server).await.contains(&"bal".to_string()));
    handle_update(&ctx, callback_update(6, 611, "bal")).await;
    let text = last_text(&server).await;
    assert!(text.contains("На балансе"), "{text}");
    assert!(text.contains("Пополнить баланс сейчас нельзя"), "{text}");
    db.cleanup().await;
}

#[test]
fn напоминание_при_выключенном_балансе_зовёт_продлить_а_не_пополнить() {
    let utc = chrono::FixedOffset::east_opt(0).unwrap();
    let content = sn_domain::bot_content::Content::default();
    let params = serde_json::json!({
        "amount": {"minor": 699, "currency": "USD"},
        "missing": {"minor": 399, "currency": "USD"},
        "expire_at": "2027-02-14T10:00:00Z",
        "balance_off": true
    });
    for template in [
        notify::templates::AUTORENEW_REMINDER,
        notify::templates::AUTORENEW_CHARGE_FAILED,
    ] {
        let rendered = notify::render(&content, template, &params, utc).unwrap();
        assert!(
            rendered
                .text
                .contains("Продлите её до 14\u{a0}февраля\u{a0}2027"),
            "{}",
            rendered.text
        );
        assert!(!rendered.text.contains("Пополните"), "{}", rendered.text);
        let keyboard = serde_json::to_value(&rendered.keyboard).unwrap();
        assert!(keyboard.to_string().contains("\"buy\""), "{keyboard}");
    }
    let usual = notify::render(
        &content,
        notify::templates::AUTORENEW_REMINDER,
        &serde_json::json!({"missing": {"minor": 399, "currency": "USD"}}),
        utc,
    )
    .unwrap();
    assert!(usual.text.contains("Пополните баланс"), "{}", usual.text);
}
