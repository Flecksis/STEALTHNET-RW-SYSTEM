use super::money::scene;
use super::*;
use sn_store::testing::test_store;

#[tokio::test]
async fn обращения_скорость_ответа_и_доля_ии() {
    let Some(db) = test_store().await else {
        return;
    };
    let s = scene(db.store.clone()).await;
    s.seed
        .exec(&format!(
            "INSERT INTO tickets (id, client_id, channel, status, created_at, closed_at) OVERRIDING SYSTEM VALUE VALUES \
               (901, {a}, 'bot', 'closed', '2026-03-05 10:00+00', '2026-03-05 11:00+00'), \
               (902, {b}, 'cabinet', 'open', '2026-03-06 10:00+00', NULL); \
             INSERT INTO ticket_messages (ticket_id, author, body, created_at) VALUES \
               (901, 'client', 'не работает', '2026-03-05 10:00+00'), \
               (901, 'support', 'проверим', '2026-03-05 10:30+00'), \
               (902, 'client', 'как продлить', '2026-03-06 10:00+00'), \
               (902, 'ai', 'в разделе подписки', '2026-03-06 10:01+00'); \
             INSERT INTO ai_support_log (ticket_id, client_id, outcome, prompt_tokens, completion_tokens, created_at) \
               VALUES (902, {b}, 'answered', 100, 50, '2026-03-06 10:01+00')",
            a = s.a,
            b = s.b
        ))
        .await;
    let r = ctx_report(&db.store, "support", month(3, Compare::None)).await;
    assert_eq!(value(&r, "tickets"), 2.0);
    let mut channels = breakdown(&r, "tickets", "channel");
    channels.sort_by(|a, b| a.0.cmp(&b.0));
    assert_eq!(channels, vec![("bot".to_string(), 1.0), ("cabinet".to_string(), 1.0)]);
    assert_eq!(value(&r, "first_reply"), 930.0);
    assert_eq!(value(&r, "close_time"), 3_600.0);
    assert_eq!(value(&r, "ai_share"), 50.0);
    assert_eq!(value(&r, "ai_tokens"), 150.0);
    assert_eq!(value(&r, "per_1000"), 500.0);
}
