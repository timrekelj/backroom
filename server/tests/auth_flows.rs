mod common;

use common::TestApp;
use common::fixtures::*;
use connectrpc::{ConnectError, ErrorCode};
use thehideout_server::proto::thehideout::v1::*;
use thehideout_server::services::auth::{CODE_REQUEST_EMAIL_MAX, CODE_VERIFY_EMAIL_MAX};

fn code<T>(r: Result<T, ConnectError>) -> ErrorCode {
    r.err().expect("expected an error").code
}

async fn request_code(app: &TestApp, email: &str) -> Result<(), ConnectError> {
    auth(app, None)
        .request_email_code(RequestEmailCodeRequest { email: email.into(), ..Default::default() })
        .await
        .map(|_| ())
}

async fn verify_code(app: &TestApp, email: &str, code: &str, device: &str) -> Result<VerifyEmailCodeResponse, ConnectError> {
    Ok(auth(app, None)
        .verify_email_code(VerifyEmailCodeRequest { email: email.into(), code: code.into(), device_name: device.into(), ..Default::default() })
        .await?
        .into_owned())
}

#[tokio::test]
async fn email_code_sign_in() {
    let app = TestApp::start().await;
    let alice = user(&app, "alice").await;
    let email = email_of(&app, "alice");
    request_code(&app, &email.to_uppercase()).await.unwrap();
    let mail = wait_mail(&app, &email, 0, "sign-in code").await;
    let c = six_digit_code(&mail.text);
    assert!(mail.html.contains(&c) && mail.subject.starts_with(&c));

    // Wrong codes count; the right one signs in once.
    assert_eq!(code(verify_code(&app, &email, "000000", "phone").await), ErrorCode::Unauthenticated);
    let signed = verify_code(&app, &email, &c, "phone").await.unwrap();
    assert_eq!(signed.user.unwrap().id, alice.id);
    assert!(!signed.second_factor_required.is_set());
    assert_eq!(code(verify_code(&app, &email, &c, "phone").await), ErrorCode::Unauthenticated, "single use");

    // A new device triggers a notice.
    let notice = wait_mail(&app, &email, 1, "New sign-in").await;
    assert!(notice.text.contains("phone") && notice.text.contains("email code"));
    app.cleanup().await;
}

#[tokio::test]
async fn email_codes_die_after_five_wrong_guesses() {
    let app = TestApp::start().await;
    user(&app, "bob").await;
    let email = email_of(&app, "bob");
    request_code(&app, &email).await.unwrap();
    let c = six_digit_code(&wait_mail(&app, &email, 0, "sign-in code").await.text);
    let wrong = if c == "111111" { "222222" } else { "111111" };
    for _ in 0..5 {
        assert_eq!(code(verify_code(&app, &email, wrong, "").await), ErrorCode::Unauthenticated);
    }
    assert_eq!(code(verify_code(&app, &email, &c, "").await), ErrorCode::Unauthenticated);
    app.cleanup().await;
}

#[tokio::test]
async fn code_requests_are_enumeration_safe_and_rate_limited() {
    let app = TestApp::start().await;
    user(&app, "carol").await;
    let known = email_of(&app, "carol");
    let unknown = format!("{}@nowhere.test", app.name("ghost"));
    // Same answer for both; only the real account gets mail.
    request_code(&app, &known).await.unwrap();
    request_code(&app, &unknown).await.unwrap();
    auth(&app, None)
        .request_password_reset(RequestPasswordResetRequest { email: unknown.clone(), ..Default::default() })
        .await
        .unwrap();
    wait_mail(&app, &known, 0, "sign-in code").await;
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    assert!(mail(&app, &unknown).is_empty());
    // Verifying against an unknown address looks like a wrong code.
    let unknown_err = verify_code(&app, &unknown, "123456", "").await.unwrap_err();
    let known_err = verify_code(&app, &known, "abc", "").await.unwrap_err();
    assert_eq!((unknown_err.code, &unknown_err.message), (known_err.code, &known_err.message));

    // Per-address request limit applies to every address alike.
    for _ in 1..CODE_REQUEST_EMAIL_MAX {
        request_code(&app, &unknown).await.unwrap();
    }
    assert_eq!(code(request_code(&app, &unknown).await), ErrorCode::ResourceExhausted);
    // Per-address verification limit.
    for _ in 1..CODE_VERIFY_EMAIL_MAX {
        let _ = verify_code(&app, &unknown, "123456", "").await;
    }
    assert_eq!(code(verify_code(&app, &unknown, "123456", "").await), ErrorCode::ResourceExhausted);
    app.cleanup().await;
}

#[tokio::test]
async fn password_reset_sets_the_password_and_signs_out_everywhere() {
    let app = TestApp::start().await;
    let dave = user(&app, "dave").await;
    let email = email_of(&app, "dave");
    auth(&app, None).request_password_reset(RequestPasswordResetRequest { email: email.clone(), ..Default::default() }).await.unwrap();
    let c = six_digit_code(&wait_mail(&app, &email, 0, "Reset").await.text);
    let reset = |code: &str, password: &str| {
        let client = auth(&app, None);
        let req = ResetPasswordRequest { email: email.clone(), code: code.into(), new_password: password.into(), ..Default::default() };
        async move { client.reset_password(req).await.map(|_| ()) }
    };
    assert_eq!(code(reset(&c, "short").await), ErrorCode::InvalidArgument);
    reset(&c, "a brand new password").await.unwrap();
    assert_eq!(code(reset(&c, "another new password").await), ErrorCode::Unauthenticated, "single use");
    assert_eq!(code(auth(&app, Some(&dave.token)).get_me(GetMeRequest::default()).await), ErrorCode::Unauthenticated);
    assert_eq!(code(auth(&app, None).login(login_req(&email, PASSWORD, "")).await), ErrorCode::Unauthenticated);
    auth(&app, None).login(login_req(&email, "a brand new password", "")).await.unwrap();
    wait_mail(&app, &email, 1, "Security change").await;
    app.cleanup().await;
}

#[tokio::test]
async fn change_password_keeps_only_the_current_session() {
    let app = TestApp::start().await;
    let erin = user(&app, "erin").await;
    let other = auth(&app, None).login(login_req(&app.name("erin"), PASSWORD, "tablet")).await.unwrap().into_owned().token;
    let change = |current: &str, new: &str| {
        let client = auth(&app, Some(&erin.token));
        let req = ChangePasswordRequest { current_password: current.into(), new_password: new.into(), ..Default::default() };
        async move { client.change_password(req).await.map(|_| ()) }
    };
    assert_eq!(code(change("wrong password", "new password 1").await), ErrorCode::PermissionDenied);
    change(PASSWORD, "new password 1").await.unwrap();
    auth(&app, Some(&erin.token)).get_me(GetMeRequest::default()).await.expect("current session survives");
    assert_eq!(code(auth(&app, Some(&other)).get_me(GetMeRequest::default()).await), ErrorCode::Unauthenticated);
    auth(&app, None).login(login_req(&app.name("erin"), "new password 1", "")).await.unwrap();
    app.cleanup().await;
}

#[tokio::test]
async fn change_email_verifies_the_new_address_and_legacy_accounts_can_add_one() {
    let app = TestApp::start().await;
    let frank = user(&app, "frank").await;
    let old = email_of(&app, "frank");
    let new = format!("{}@new.test", app.name("frank"));
    let client = auth(&app, Some(&frank.token));
    let start = |email: &str, password: &str| {
        let req = ChangeEmailRequest { new_email: email.into(), current_password: password.into(), ..Default::default() };
        let client = client.clone();
        async move { client.change_email(req).await.map(|_| ()) }
    };
    assert_eq!(code(start(&new, "wrong").await), ErrorCode::PermissionDenied);
    assert_eq!(code(start("nope", PASSWORD).await), ErrorCode::InvalidArgument);
    user(&app, "gina").await;
    assert_eq!(code(start(&email_of(&app, "gina"), PASSWORD).await), ErrorCode::AlreadyExists);
    let confirm = |code: &str| {
        let req = ConfirmEmailChangeRequest { code: code.into(), ..Default::default() };
        let client = client.clone();
        async move { client.confirm_email_change(req).await.map(|_| ()) }
    };
    let expired = Some((CodeErrorReason::CODE_ERROR_REASON_EXPIRED, 0));
    assert_eq!(code_error(confirm("123456").await), (ErrorCode::FailedPrecondition, expired), "nothing was started");
    start(&new, PASSWORD).await.unwrap();
    let c = six_digit_code(&wait_mail(&app, &new, 0, "Confirm").await.text);
    let wrong = if c == "000000" { "111111" } else { "000000" };
    assert_eq!(
        code_error(confirm(wrong).await),
        (ErrorCode::InvalidArgument, Some((CodeErrorReason::CODE_ERROR_REASON_WRONG, 4))),
        "a wrong code is not UNAUTHENTICATED"
    );
    let confirmed = client
        .confirm_email_change(ConfirmEmailChangeRequest { code: c, ..Default::default() })
        .await
        .unwrap()
        .into_owned()
        .user
        .unwrap();
    assert_eq!(confirmed.email.as_deref(), Some(new.as_str()));
    wait_mail(&app, &old, 0, "email was changed").await;
    // The old address is released, the new one signs in.
    auth(&app, None).login(login_req(&new, PASSWORD, "")).await.unwrap();
    assert_eq!(code(auth(&app, None).login(login_req(&old, PASSWORD, "")).await), ErrorCode::Unauthenticated);

    // Legacy account without email adds one.
    let legacy = legacy_user(&app, "legacy").await;
    let added = format!("{}@added.test", app.name("legacy"));
    let lc = auth(&app, Some(&legacy.token));
    lc.change_email(ChangeEmailRequest { new_email: added.clone(), current_password: PASSWORD.into(), ..Default::default() })
        .await
        .unwrap();
    let c = six_digit_code(&wait_mail(&app, &added, 0, "Confirm").await.text);
    lc.confirm_email_change(ConfirmEmailChangeRequest { code: c, ..Default::default() }).await.unwrap();
    let me = lc.get_me(GetMeRequest::default()).await.unwrap().into_owned().user.unwrap();
    assert_eq!(me.email.as_deref(), Some(added.as_str()));
    app.cleanup().await;
}

#[tokio::test]
async fn known_devices_get_no_new_sign_in_notice() {
    let app = TestApp::start().await;
    user(&app, "hal").await;
    let email = email_of(&app, "hal");
    let client = auth(&app, None);
    client.login(login_req(&email, PASSWORD, "desk")).await.unwrap();
    wait_mail(&app, &email, 0, "New sign-in").await;
    client.login(login_req(&email, PASSWORD, "desk")).await.unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    assert_eq!(mail(&app, &email).iter().filter(|m| m.subject.contains("New sign-in")).count(), 1);
    app.cleanup().await;
}
