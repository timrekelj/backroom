mod common;

use backroom_server::proto::backroom::v1::{
    AuthServiceClient, GetMeRequest, LoginRequest, SignUpRequest, User,
};
use backroom_server::services::auth::LOGIN_IP_MAX;
use common::TestApp;
use connectrpc::ErrorCode;

const PASSWORD: &str = "correct horse battery";

fn sign_up_req(username: &str, password: &str) -> SignUpRequest {
    SignUpRequest {
        username: username.into(),
        password: password.into(),
        ..Default::default()
    }
}

fn login_req(username: &str, password: &str) -> LoginRequest {
    LoginRequest {
        username: username.into(),
        password: password.into(),
        ..Default::default()
    }
}

async fn get_me(app: &TestApp, token: Option<&str>) -> Result<User, connectrpc::ConnectError> {
    let client = app.client(AuthServiceClient::new, token);
    let resp = client.get_me(GetMeRequest::default()).await?;
    Ok(resp.into_owned().user.unwrap())
}

#[tokio::test]
async fn sign_up_then_get_me() {
    let app = TestApp::start().await;
    let client = app.client(AuthServiceClient::new, None);

    let resp = client
        .sign_up(sign_up_req("  Alice  ", PASSWORD))
        .await
        .expect("sign up")
        .into_owned();
    let user = resp.user.unwrap();
    assert!(!resp.token.is_empty());
    assert_ne!(user.id, 0);
    assert_eq!(user.username, "alice");
    assert!(user.created_at.is_set());

    let me = get_me(&app, Some(&resp.token)).await.expect("get me");
    assert_eq!(me, user);

    app.cleanup().await;
}

#[tokio::test]
async fn duplicate_username_is_rejected_case_insensitively() {
    let app = TestApp::start().await;
    let client = app.client(AuthServiceClient::new, None);

    client.sign_up(sign_up_req("alice", PASSWORD)).await.expect("first sign up");
    for name in ["alice", "Alice", "ALICE "] {
        let err = client
            .sign_up(sign_up_req(name, PASSWORD))
            .await
            .expect_err("duplicate must fail");
        // Generic error: a distinct "username taken" code would be an account-existence oracle.
        assert_eq!(err.code, ErrorCode::FailedPrecondition, "username {name:?}");
        assert!(!err.message.as_deref().unwrap_or("").contains("taken"), "message must not confirm existence");
    }

    app.cleanup().await;
}

#[tokio::test]
async fn sign_up_validates_input() {
    let app = TestApp::start().await;
    let client = app.client(AuthServiceClient::new, None);

    let too_long = "a".repeat(33);
    let long_password = "p".repeat(129);
    let cases = [
        ("", PASSWORD),
        ("a", PASSWORD),
        (too_long.as_str(), PASSWORD),
        ("bad name", PASSWORD),
        ("bad-name", PASSWORD),
        ("émile", PASSWORD),
        ("alice", "short"),
        ("alice", ""),
        ("alice", long_password.as_str()),
    ];
    for (username, password) in cases {
        let err = client
            .sign_up(sign_up_req(username, password))
            .await
            .expect_err("invalid input must fail");
        assert_eq!(
            err.code,
            ErrorCode::InvalidArgument,
            "username {username:?}, password length {}",
            password.len()
        );
    }

    // Boundary values are accepted.
    client
        .sign_up(sign_up_req(&"a".repeat(32), &"p".repeat(128)))
        .await
        .expect("max length");
    client.sign_up(sign_up_req("a.", "12345678")).await.expect("min length");

    app.cleanup().await;
}

#[tokio::test]
async fn login_returns_new_working_token() {
    let app = TestApp::start().await;
    let client = app.client(AuthServiceClient::new, None);

    let signed_up = client
        .sign_up(sign_up_req("bob", PASSWORD))
        .await
        .expect("sign up")
        .into_owned();
    let login = client
        .login(login_req("Bob", PASSWORD))
        .await
        .expect("login")
        .into_owned();
    assert!(!login.token.is_empty());
    assert_ne!(login.token, signed_up.token);
    assert_eq!(login.user.unwrap(), signed_up.user.unwrap());

    let me = get_me(&app, Some(&login.token)).await.expect("get me");
    assert_eq!(me.username, "bob");

    app.cleanup().await;
}

#[tokio::test]
async fn bad_credentials_are_unauthenticated() {
    let app = TestApp::start().await;
    let client = app.client(AuthServiceClient::new, None);
    client.sign_up(sign_up_req("carol", PASSWORD)).await.expect("sign up");

    let wrong_password = client
        .login(login_req("carol", "not the password"))
        .await
        .expect_err("wrong password");
    let unknown_user = client
        .login(login_req("nobody", PASSWORD))
        .await
        .expect_err("unknown user");
    assert_eq!(wrong_password.code, ErrorCode::Unauthenticated);
    assert_eq!(unknown_user.code, ErrorCode::Unauthenticated);
    assert_eq!(wrong_password.message, unknown_user.message);

    app.cleanup().await;
}

#[tokio::test]
async fn login_is_rate_limited_per_ip() {
    let app = TestApp::start().await;
    let client = app.client(AuthServiceClient::new, None);

    // Distinct usernames keep each per-username counter at one attempt (no delay);
    // the per-IP hard cap is what rejects here.
    for i in 0..LOGIN_IP_MAX {
        let err = client
            .login(login_req(&format!("user{i}"), "wrong password"))
            .await
            .expect_err("unknown user");
        assert_eq!(err.code, ErrorCode::Unauthenticated, "attempt {i}");
    }
    let err = client
        .login(login_req("one-more", "wrong password"))
        .await
        .expect_err("over the limit");
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
