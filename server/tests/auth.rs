mod common;

use common::TestApp;
use common::fixtures::*;
use connectrpc::ErrorCode;
use thehideout_server::proto::thehideout::v1::*;
use thehideout_server::services::auth::LOGIN_IP_MAX;

async fn get_me(app: &TestApp, token: Option<&str>) -> Result<User, connectrpc::ConnectError> {
    Ok(auth(app, token).get_me(GetMeRequest::default()).await?.into_owned().user.unwrap())
}

#[tokio::test]
async fn sign_up_requires_an_invite() {
    let app = TestApp::start().await;
    let err = auth(&app, None)
        .sign_up(SignUpRequest { username: app.name("alice"), password: PASSWORD.into(), ..Default::default() })
        .await
        .expect_err("sign up is invite-only");
    assert_eq!(err.code, ErrorCode::FailedPrecondition);
    assert_eq!(err.message.as_deref(), Some("invite required"));
    app.cleanup().await;
}

#[tokio::test]
async fn login_by_username_or_email_returns_a_new_working_token() {
    let app = TestApp::start().await;
    let bob = user(&app, "bob").await;
    let client = auth(&app, None);
    for name in [app.name("Bob"), email_of(&app, "bob").to_uppercase()] {
        let login = client.login(login_req(&name, PASSWORD, "laptop")).await.expect("login").into_owned();
        assert!(!login.token.is_empty() && login.token != bob.token);
        assert!(!login.second_factor_required.is_set());
        let user = login.user.unwrap();
        assert_eq!(user.id, bob.id);
        assert_eq!(user.email.as_deref(), Some(email_of(&app, "bob").as_str()));
        let me = get_me(&app, Some(&login.token)).await.expect("get me");
        assert_eq!(me.username, app.name("bob"));
    }
    app.cleanup().await;
}

#[tokio::test]
async fn legacy_accounts_without_email_keep_password_login() {
    let app = TestApp::start().await;
    let old = legacy_user(&app, "old").await;
    let me = get_me(&app, Some(&old.token)).await.unwrap();
    assert_eq!(me.email, None);
    let login = auth(&app, None).login(login_req(&app.name("old"), PASSWORD, "")).await.expect("login").into_owned();
    assert_eq!(login.user.unwrap().id, old.id);
    app.cleanup().await;
}

#[tokio::test]
async fn bad_credentials_are_unauthenticated() {
    let app = TestApp::start().await;
    user(&app, "carol").await;
    let client = auth(&app, None);
    let wrong_password = client.login(login_req(&app.name("carol"), "not the password", "")).await.expect_err("wrong password");
    let unknown_user = client.login(login_req("nobody", PASSWORD, "")).await.expect_err("unknown user");
    let unknown_email = client.login(login_req("nobody@example.test", PASSWORD, "")).await.expect_err("unknown email");
    for err in [&wrong_password, &unknown_user, &unknown_email] {
        assert_eq!(err.code, ErrorCode::Unauthenticated);
        assert_eq!(err.message, wrong_password.message);
    }
    app.cleanup().await;
}

#[tokio::test]
async fn login_is_rate_limited_per_ip() {
    let app = TestApp::start().await;
    let client = auth(&app, None);
    // Distinct usernames keep each per-username counter at one attempt (no delay);
    // the per-IP hard cap is what rejects here.
    for i in 0..LOGIN_IP_MAX {
        let err = client.login(login_req(&format!("user{i}"), "wrong password", "")).await.expect_err("unknown user");
        assert_eq!(err.code, ErrorCode::Unauthenticated, "attempt {i}");
    }
    let err = client.login(login_req("one-more", "wrong password", "")).await.expect_err("over the limit");
    assert_eq!(err.code, ErrorCode::ResourceExhausted);
    app.cleanup().await;
}

#[tokio::test]
async fn get_me_requires_valid_token() {
    let app = TestApp::start().await;
    let err = get_me(&app, None).await.expect_err("no header");
    assert_eq!(err.code, ErrorCode::Unauthenticated);
    let err = get_me(&app, Some("garbage")).await.expect_err("garbage token");
    assert_eq!(err.code, ErrorCode::Unauthenticated);
    app.cleanup().await;
}
