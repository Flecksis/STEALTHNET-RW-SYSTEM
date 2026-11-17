use super::fixtures::*;
use crate::testing::test_store;
use crate::*;
use sn_domain::access::AdminRole;
use sn_domain::ids::{AdminId, ClientId, TicketId};
use sn_domain::paging::PageRequest;
use sn_domain::support::{MessageAuthor, TicketChannel, TicketFilter, TicketStatus};

async fn open(db: &mut impl Db, client: ClientId, body: &str) -> TicketId {
    db.tickets()
        .open(&NewTicket {
            client_id: client,
            subscription_id: None,
            channel: TicketChannel::Bot,
            subject: None,
            body: body.into(),
        })
        .await
        .unwrap()
}

fn message(
    ticket: TicketId,
    author: MessageAuthor,
    admin: Option<AdminId>,
    close: bool,
) -> NewReply {
    NewReply {
        ticket_id: ticket,
        author,
        admin_id: admin,
        body: format!("{author:?}"),
        attachments: Vec::new(),
        close,
    }
}

async fn ids(db: &mut impl Db, filter: TicketFilter) -> Vec<TicketId> {
    db.tickets()
        .list(filter, PageRequest::default())
        .await
        .unwrap()
        .items
        .into_iter()
        .map(|t| t.id)
        .collect()
}

#[tokio::test]
async fn обращения_делятся_на_ждущие_отвеченные_и_закрытые() {
    let Some(db) = test_store().await else { return };
    let mut conn = db.store.conn().await.unwrap();
    let support = conn
        .admins()
        .create(&NewAdmin {
            email: "support@example.com".into(),
            password_hash: "hash".into(),
            role: AdminRole::Support,
            must_change_password: false,
        })
        .await
        .unwrap()
        .id;
    let ksenia = client(
        &mut conn,
        &Person {
            telegram: Some(1_001),
            username: Some("ks_smirnova"),
            first_name: Some("Ксения"),
            last_name: Some("Смирнова"),
            email: None,
        },
    )
    .await;
    let igor = client(
        &mut conn,
        &Person {
            email: Some("igor@example.com"),
            ..Person::NOBODY
        },
    )
    .await;
    let first = open(&mut conn, ksenia, "Не подключается на iPhone").await;
    let second = open(&mut conn, igor, "Медленно работает").await;
    let third = open(&mut conn, ksenia, "Не пришёл чек").await;
    assert_eq!(
        ids(&mut conn, TicketFilter::Waiting).await,
        [first, second, third]
    );
    let view = conn
        .tickets()
        .list_view(TicketFilter::Waiting, PageRequest::default(), None)
        .await
        .unwrap();
    assert_eq!(view.items.len(), 3);
    let top = &view.items[0];
    assert_eq!(top.ticket.id, first);
    assert_eq!(top.client_name.as_deref(), Some("Ксения Смирнова"));
    assert_eq!(top.telegram_username.as_deref(), Some("ks_smirnova"));
    assert_eq!(top.preview.as_deref(), Some("Не подключается на iPhone"));
    assert_eq!(top.waiting_since, Some(top.ticket.created_at));
    assert_eq!(view.items[1].email.as_deref(), Some("igor@example.com"));
    assert_eq!(view.items[1].client_name, None);

    conn.tickets()
        .reply(&message(
            second,
            MessageAuthor::Support,
            Some(support),
            false,
        ))
        .await
        .unwrap();
    assert_eq!(ids(&mut conn, TicketFilter::Waiting).await, [first, third]);
    assert_eq!(ids(&mut conn, TicketFilter::Answered).await, [second]);
    let answered = conn
        .tickets()
        .list_view(TicketFilter::Answered, PageRequest::default(), None)
        .await
        .unwrap()
        .items;
    assert_eq!(answered[0].waiting_since, None);
    assert_eq!(answered[0].preview.as_deref(), Some("Support"));
    assert!(answered[0].last_message_at.is_some());
    assert_eq!(
        conn.tickets().get(second).await.unwrap().filter(),
        TicketFilter::Answered
    );

    conn.tickets()
        .reply(&message(second, MessageAuthor::Client, None, false))
        .await
        .unwrap();
    conn.tickets()
        .reply(&message(first, MessageAuthor::Client, None, false))
        .await
        .unwrap();
    assert_eq!(
        ids(&mut conn, TicketFilter::Waiting).await,
        [first, third, second]
    );

    conn.tickets()
        .reply(&message(third, MessageAuthor::Support, Some(support), true))
        .await
        .unwrap();
    let closed = conn.tickets().get(third).await.unwrap();
    assert_eq!(closed.status, TicketStatus::Closed);
    assert_eq!(closed.last_author, MessageAuthor::Support);
    assert!(closed.closed_at.is_some());
    assert_eq!(ids(&mut conn, TicketFilter::Closed).await, [third]);
    assert_eq!(
        conn.tickets().counts().await.unwrap(),
        TicketCounts {
            waiting: 2,
            answered: 0,
            closed: 1
        }
    );
    assert_eq!(conn.tickets().count_open(ksenia).await.unwrap(), 1);

    let thread = conn.tickets().messages(third).await.unwrap();
    assert_eq!(thread.len(), 2);
    assert_eq!(thread[0].author, MessageAuthor::Client);
    assert_eq!(thread[0].body, "Не пришёл чек");
    assert_eq!(thread[1].author, MessageAuthor::Support);
    assert_eq!(thread[1].admin_id, Some(support));
    assert_eq!(conn.tickets().messages(first).await.unwrap().len(), 2);

    conn.tickets()
        .reply(&message(third, MessageAuthor::Client, None, false))
        .await
        .unwrap();
    let reopened = conn.tickets().get(third).await.unwrap();
    assert_eq!(reopened.status, TicketStatus::Open);
    assert_eq!(reopened.closed_at, None);
    assert_eq!(
        ids(&mut conn, TicketFilter::Waiting).await,
        [first, second, third]
    );

    conn.tickets().close(first).await.unwrap();
    assert_eq!(ids(&mut conn, TicketFilter::Closed).await, [first]);
    conn.tickets().reopen(first).await.unwrap();
    assert_eq!(
        ids(&mut conn, TicketFilter::Waiting).await,
        [first, second, third]
    );

    let page = conn
        .tickets()
        .list(TicketFilter::Waiting, PageRequest::new(Some(2), None))
        .await
        .unwrap();
    assert_eq!(
        page.items.iter().map(|t| t.id).collect::<Vec<_>>(),
        [first, second]
    );
    let rest = conn
        .tickets()
        .list(TicketFilter::Waiting, PageRequest::new(Some(2), page.next))
        .await
        .unwrap();
    assert_eq!(rest.items.iter().map(|t| t.id).collect::<Vec<_>>(), [third]);
    assert_eq!(rest.next, None);

    let own = conn
        .tickets()
        .for_client(ksenia, PageRequest::default())
        .await
        .unwrap();
    assert_eq!(
        own.items.iter().map(|t| t.id).collect::<Vec<_>>(),
        [third, first]
    );

    let missing = TicketId(third.0 + 100);
    assert!(
        conn.tickets()
            .get(missing)
            .await
            .unwrap_err()
            .is_not_found()
    );
    assert!(
        conn.tickets()
            .messages(missing)
            .await
            .unwrap_err()
            .is_not_found()
    );
    assert!(
        conn.tickets()
            .reply(&message(missing, MessageAuthor::Support, None, false))
            .await
            .unwrap_err()
            .is_not_found()
    );
    drop(conn);
    db.cleanup().await;
}
