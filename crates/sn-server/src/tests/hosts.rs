use super::*;
use sn_remnawave::fake::sample_node;
use sn_remnawave::{
    ConfigProfile, Host, HostInbound, HostSquadFilter, Inbound, NodeConfigProfile, NodeDetails,
};

const PROFILE: Uuid = Uuid::from_u128(60);
const REALITY: Uuid = Uuid::from_u128(61);
const SHADOWSOCKS: Uuid = Uuid::from_u128(62);
const GERMANY: Uuid = Uuid::from_u128(63);
const SQUAD: Uuid = Uuid::from_u128(64);

fn inbound(uuid: Uuid, tag: &str, kind: &str, port: i64) -> Inbound {
    Inbound {
        uuid,
        profile_uuid: Some(PROFILE),
        tag: tag.into(),
        kind: kind.into(),
        network: Some("tcp".into()),
        security: None,
        port: Some(port),
    }
}

fn host(n: u128, remark: &str, inbound: Uuid) -> Host {
    Host {
        uuid: Uuid::from_u128(n),
        view_position: n as i64,
        remark: remark.into(),
        address: "203.0.113.41".into(),
        port: 1234,
        inbound: HostInbound {
            config_profile_uuid: Some(PROFILE),
            config_profile_inbound_uuid: Some(inbound),
        },
        ..Host::default()
    }
}

fn panel(h: &Http) {
    h.remna.set_config_profiles(vec![ConfigProfile {
        uuid: PROFILE,
        name: "Default-Profile".into(),
        view_position: 0,
        inbounds: vec![
            inbound(SHADOWSOCKS, "SHADOWSOCKS", "shadowsocks", 1234),
            inbound(REALITY, "VLESS_REALITY", "vless", 443),
        ],
        nodes: Vec::new(),
    }]);
    let mut node = sample_node(GERMANY, "Germany", "DE");
    node.address = "203.0.113.41".into();
    let mut detail = NodeDetails::from(node);
    detail.config_profile = Some(NodeConfigProfile {
        active_config_profile_uuid: Some(PROFILE),
        active_inbounds: vec![inbound(SHADOWSOCKS, "SHADOWSOCKS", "shadowsocks", 1234)],
    });
    h.remna.set_node_details(vec![detail]);
    h.remna.set_squads(vec![InternalSquad {
        uuid: SQUAD,
        name: "Default-Squad".into(),
        view_position: 0,
        info: SquadInfo::default(),
        inbounds: vec![inbound(SHADOWSOCKS, "SHADOWSOCKS", "shadowsocks", 1234)],
    }]);
    let mut reserve = host(72, "Германия, резерв", SHADOWSOCKS);
    reserve.internal_squads = Some(HostSquadFilter::exclude(vec![SQUAD]));
    reserve.is_disabled = true;
    h.remna
        .set_hosts(vec![reserve, host(71, "Германия", SHADOWSOCKS)]);
}

fn remarks(view: &Value) -> Vec<String> {
    view["hosts"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| row["host"]["remark"].as_str().unwrap().to_string())
        .collect()
}

#[tokio::test]
async fn хосты_через_http_список_правка_порядок_и_удаление() {
    let Some(h) = http().await else { return };
    let owner = h.owner().await;
    let support = h.admin("support@example.com", AdminRole::Support).await;
    panel(&h);

    let (status, view) = h.get("/api/admin/infra/hosts", &owner).await;
    assert_eq!(status, StatusCode::OK, "{view}");
    assert_eq!(remarks(&view), ["Германия", "Германия, резерв"]);
    let first = &view["hosts"][0];
    assert_eq!(first["host"]["address"], "203.0.113.41");
    assert_eq!(first["host"]["port"], 1234);
    assert_eq!(first["nodes"], json!([GERMANY]));
    assert_eq!(
        first["squads"],
        json!([{ "uuid": SQUAD, "name": "Default-Squad", "shown": true }])
    );
    assert_eq!(view["hosts"][1]["squads"][0]["shown"], false);
    assert_eq!(view["hosts"][1]["host"]["isDisabled"], true);
    assert_eq!(view["nodes"][0]["country_code"], "DE");
    assert_eq!(view["nodes"][0]["state"], "online");
    assert_eq!(view["inbounds"][0]["tag"], "SHADOWSOCKS");
    assert_eq!(view["inbounds"][0]["port"], 1234);
    assert_eq!(view["inbounds"][0]["nodes"], json!([GERMANY]));
    assert_eq!(view["inbounds"][1]["nodes"], json!([]));

    let (status, _) = h.get("/api/admin/infra/hosts", &support).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) = h
        .write(
            Method::POST,
            &format!(
                "/api/admin/infra/hosts/{}/actions/disable",
                Uuid::from_u128(71)
            ),
            &support,
            json!({}),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _, _) = h
        .send(
            Method::DELETE,
            &format!("/api/admin/infra/hosts/{}", Uuid::from_u128(71)),
            Some(&support),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    let (status, body) = h
        .write(
            Method::POST,
            "/api/admin/infra/hosts",
            &owner,
            json!({ "inbound": REALITY, "remark": "Германия", "address": "de.example.com", "port": 70000 }),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(body["error"]["code"], "host.port");
    let (status, body) = h
        .write(
            Method::POST,
            "/api/admin/infra/hosts",
            &owner,
            json!({ "inbound": REALITY, "remark": "Германия", "address": "de.example.com", "port": 443, "alpn": "spdy" }),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(body["error"]["code"], "host.alpn");
    let (status, created) = h
        .write(
            Method::POST,
            "/api/admin/infra/hosts",
            &owner,
            json!({
                "inbound": REALITY, "remark": " Германия Reality ", "address": "de.example.com", "port": 443,
                "sni": "www.example.com", "fingerprint": "chrome", "alpn": "h2", "security_layer": "DEFAULT"
            }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{created}");
    assert_eq!(created["remark"], "Германия Reality");
    assert_eq!(created["sni"], "www.example.com");
    assert_eq!(
        created["inbound"]["configProfileInboundUuid"],
        json!(REALITY)
    );
    let created_uuid = created["uuid"].as_str().unwrap().to_string();

    let (status, updated) = h
        .write(
            Method::PUT,
            &format!("/api/admin/infra/hosts/{created_uuid}"),
            &owner,
            json!({
                "remark": "Германия Reality", "address": "203.0.113.41", "port": 8443, "host": "cdn.example.com",
                "path": "/ws", "sni": "", "fingerprint": "firefox", "alpn": null, "security_layer": "TLS"
            }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{updated}");
    assert_eq!(updated["port"], 8443);
    assert_eq!(updated["sni"], Value::Null);
    assert_eq!(updated["alpn"], Value::Null);
    assert_eq!(updated["host"], "cdn.example.com");
    assert_eq!(updated["securityLayer"], "TLS");
    let (status, body) = h
        .write(
            Method::PUT,
            &format!("/api/admin/infra/hosts/{created_uuid}"),
            &owner,
            json!({ "remark": "", "address": "203.0.113.41", "port": 8443 }),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(body["error"]["code"], "host.remark");
    let (status, _) = h
        .write(
            Method::PUT,
            &format!("/api/admin/infra/hosts/{}", Uuid::from_u128(404)),
            &owner,
            json!({ "remark": "X", "address": "203.0.113.41", "port": 443 }),
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    let (status, off) = h
        .write(
            Method::POST,
            &format!(
                "/api/admin/infra/hosts/{}/actions/disable",
                Uuid::from_u128(71)
            ),
            &owner,
            json!({}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{off}");
    assert_eq!(off["isDisabled"], true);
    let (status, on) = h
        .write(
            Method::POST,
            &format!(
                "/api/admin/infra/hosts/{}/actions/enable",
                Uuid::from_u128(72)
            ),
            &owner,
            json!({}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{on}");
    assert_eq!(on["isDisabled"], false);
    let (status, _) = h
        .write(
            Method::POST,
            &format!(
                "/api/admin/infra/hosts/{}/actions/hide",
                Uuid::from_u128(72)
            ),
            &owner,
            json!({}),
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    let order = json!({ "uuids": [created_uuid, Uuid::from_u128(72), Uuid::from_u128(71)] });
    let (status, body) = h
        .write(Method::PUT, "/api/admin/infra/hosts/order", &owner, order)
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (_, view) = h.get("/api/admin/infra/hosts", &owner).await;
    assert_eq!(
        remarks(&view),
        ["Германия Reality", "Германия, резерв", "Германия"]
    );
    let (status, body) = h
        .write(
            Method::PUT,
            "/api/admin/infra/hosts/order",
            &owner,
            json!({ "uuids": [Uuid::from_u128(71)] }),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(body["error"]["code"], "host.order");

    let (status, _, body) = h
        .send(
            Method::DELETE,
            &format!("/api/admin/infra/hosts/{created_uuid}"),
            Some(&owner),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (_, view) = h.get("/api/admin/infra/hosts", &owner).await;
    assert_eq!(remarks(&view), ["Германия, резерв", "Германия"]);
    let (status, _, _) = h
        .send(
            Method::DELETE,
            &format!("/api/admin/infra/hosts/{created_uuid}"),
            Some(&owner),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let calls = h.remna.calls();
    for method in ["create_host", "update_host", "reorder_hosts", "delete_host"] {
        assert!(calls.iter().any(|c| c == method), "нет вызова {method}");
    }
    h.db.cleanup().await;
}

#[tokio::test]
async fn хосты_без_remnawave_и_при_её_отказе() {
    let Some(h) = http().await else { return };
    let owner = h.owner().await;
    panel(&h);
    h.remna
        .fail_on("hosts", sn_remnawave::fake::Fault::Unavailable);
    let (status, _) = h.get("/api/admin/infra/hosts", &owner).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    h.remna
        .fail_on("config_profiles", sn_remnawave::fake::Fault::ServerError);
    h.remna
        .fail_on("internal_squads", sn_remnawave::fake::Fault::ServerError);
    let (status, view) = h.get("/api/admin/infra/hosts", &owner).await;
    assert_eq!(status, StatusCode::OK, "{view}");
    assert_eq!(view["hosts"].as_array().unwrap().len(), 2);
    assert_eq!(view["hosts"][0]["nodes"], json!([GERMANY]));
    assert_eq!(view["hosts"][0]["squads"], json!([]));
    assert_eq!(view["inbounds"][0]["tag"], "SHADOWSOCKS");
    assert_eq!(view["inbounds"][0]["profile_name"], "");
    h.services().set_remna(None);
    let (status, body) = h.get("/api/admin/infra/hosts", &owner).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{body}");
    h.db.cleanup().await;
}
