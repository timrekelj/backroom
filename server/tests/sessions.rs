mod common;

use common::TestApp;
use common::fixtures::*;
use connectrpc::ErrorCode;
use thehideout_server::proto::thehideout::v1::*;

async fn list(app: &TestApp, token: &str) -> Vec<Session> {
    auth(app, Some(token)).list_sessions(ListSessionsRequest::default()).await.unwrap().into_owned().sessions
}

#[tokio::test]
async fn sessions_are_listed_and_revocable() {
    let app = TestApp::start().await;
    let olga = user(&app, "olga").await;
    let login = |device: &'static str| {
        let client = auth(&app, None);
        let name = app.name("olga");
        async move { client.login(login_req(&name, PASSWORD, device)).await.unwrap().into_owned().token }
    };
    let phone = login("Phone").await;
    let laptop = login("  Laptop\n").await;

    let sessions = list(&app, &laptop).await;
    assert_eq!(sessions.iter().map(|s| s.device_name.as_str()).collect::<Vec<_>>(), ["Laptop", "Phone", "test"]);
    assert_eq!(sessions.iter().filter(|s| s.current).count(), 1);
    assert!(sessions[0].current && sessions[0].created_at.is_set() && sessions[0].last_seen_at.is_set());

    // Revoke one session by id (another user's id is not found).
    let phone_id = sessions[1].id;
    let other = user(&app, "other").await;
    let err = auth(&app, Some(&other.token))
        .revoke_session(RevokeSessionRequest { session_id: phone_id, ..Default::default() })
        .await
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::NotFound);
    auth(&app, Some(&laptop)).revoke_session(RevokeSessionRequest { session_id: phone_id, ..Default::default() }).await.unwrap();
    let err = auth(&app, Some(&phone)).get_me(GetMeRequest::default()).await.unwrap_err();
    assert_eq!(err.code, ErrorCode::Unauthenticated);

    // Revoke the others: only the current one survives.
    let revoked = auth(&app, Some(&laptop))
        .revoke_other_sessions(RevokeOtherSessionsRequest::default())
        .await
        .unwrap()
        .into_owned()
        .revoked;
    assert_eq!(revoked, 1);
    assert!(auth(&app, Some(&olga.token)).get_me(GetMeRequest::default()).await.is_err());
    assert_eq!(list(&app, &laptop).await.len(), 1);
    app.cleanup().await;
}

#[tokio::test]
async fn unnamed_devices_get_a_default_name() {
    let app = TestApp::start().await;
    user(&app, "pat").await;
    let token = auth(&app, None).login(login_req(&app.name("pat"), PASSWORD, "")).await.unwrap().into_owned().token;
    assert!(list(&app, &token).await.iter().any(|s| s.current && s.device_name == "Unknown device"));
    app.cleanup().await;
}
