mod common;

use common::TestApp;
use common::fixtures::*;
use connectrpc::{ConnectError, ErrorCode};
use thehideout_server::codes::{TOTP_STEP_SECS, totp_now};
use thehideout_server::proto::thehideout::v1::*;

fn code<T>(r: Result<T, ConnectError>) -> ErrorCode {
    r.err().expect("expected an error").code
}

fn now() -> u64 {
    chrono::Utc::now().timestamp() as u64
}

/// Turns TOTP on for `who`; returns the secret and the recovery codes.
async fn enable(app: &TestApp, who: &Account) -> (String, Vec<String>) {
    let client = auth(app, Some(&who.token));
    let setup = client.begin_totp_setup(BeginTotpSetupRequest::default()).await.unwrap().into_owned();
    assert!(setup.otpauth_uri.starts_with("otpauth://totp/thehideout:") && setup.otpauth_uri.contains(&setup.secret));
    assert_eq!(
        code(client.confirm_totp(ConfirmTotpRequest { code: "000000".into(), ..Default::default() }).await),
        ErrorCode::Unauthenticated
    );
    // Use the previous step so the current one stays free for the test.
    let codes = client
        .confirm_totp(ConfirmTotpRequest { code: totp_now(&setup.secret, now() - TOTP_STEP_SECS), ..Default::default() })
        .await
        .unwrap()
        .into_owned()
        .recovery_codes;
    assert_eq!(codes.len(), 10);
    (setup.secret, codes)
}

async fn challenge(app: &TestApp, login: &str) -> String {
    let resp = auth(app, None).login(login_req(login, PASSWORD, "laptop")).await.unwrap().into_owned();
    assert!(resp.token.is_empty() && !resp.user.is_set(), "no session before the second factor");
    let ch = resp.second_factor_required.into_option().expect("second factor required");
    assert_eq!(ch.methods.len(), 2);
    ch.challenge_id
}

async fn complete(app: &TestApp, challenge_id: &str, totp: &str, recovery: &str) -> Result<CompleteSecondFactorResponse, ConnectError> {
    Ok(auth(app, None)
        .complete_second_factor(CompleteSecondFactorRequest {
            challenge_id: challenge_id.into(),
            totp_code: totp.into(),
            recovery_code: recovery.into(),
            ..Default::default()
        })
        .await?
        .into_owned())
}

#[tokio::test]
async fn totp_is_required_and_codes_are_single_use() {
    let app = TestApp::start().await;
    let ivy = user(&app, "ivy").await;
    let (secret, recovery) = enable(&app, &ivy).await;
    wait_mail(&app, &email_of(&app, "ivy"), 0, "Security change").await;
    let login = app.name("ivy");

    let ch = challenge(&app, &login).await;
    assert_eq!(code(complete(&app, &ch, "123", "").await), ErrorCode::Unauthenticated);
    assert_eq!(code(complete(&app, &ch, "1", "x").await), ErrorCode::InvalidArgument);
    let current = totp_now(&secret, now());
    let done = complete(&app, &ch, &current, "").await.unwrap();
    assert_eq!(done.user.unwrap().id, ivy.id);
    assert_eq!(code(complete(&app, &ch, &current, "").await), ErrorCode::Unauthenticated, "challenge used");

    // Replay: the same code is refused on a new challenge; a later step works.
    let ch = challenge(&app, &login).await;
    assert_eq!(code(complete(&app, &ch, &current, "").await), ErrorCode::Unauthenticated);
    complete(&app, &ch, &totp_now(&secret, now() + TOTP_STEP_SECS), "").await.unwrap();

    // Email codes also need the second factor.
    let email = email_of(&app, "ivy");
    auth(&app, None).request_email_code(RequestEmailCodeRequest { email: email.clone(), ..Default::default() }).await.unwrap();
    let c = six_digit_code(&wait_mail(&app, &email, 0, "sign-in code").await.text);
    let resp = auth(&app, None)
        .verify_email_code(VerifyEmailCodeRequest { email: email.clone(), code: c, ..Default::default() })
        .await
        .unwrap()
        .into_owned();
    let ch = resp.second_factor_required.into_option().expect("2fa").challenge_id;

    // Recovery codes work once, in any case.
    let rc = recovery[0].to_uppercase();
    complete(&app, &ch, "", &rc).await.unwrap();
    let ch = challenge(&app, &login).await;
    assert_eq!(code(complete(&app, &ch, "", &rc).await), ErrorCode::Unauthenticated);
    complete(&app, &ch, "", &recovery[1]).await.unwrap();
    app.cleanup().await;
}

#[tokio::test]
async fn challenges_die_after_five_wrong_answers() {
    let app = TestApp::start().await;
    let jo = user(&app, "jo").await;
    let (secret, _) = enable(&app, &jo).await;
    let ch = challenge(&app, &app.name("jo")).await;
    for _ in 0..5 {
        assert_eq!(code(complete(&app, &ch, "000000", "").await), ErrorCode::Unauthenticated);
    }
    assert_eq!(code(complete(&app, &ch, &totp_now(&secret, now()), "").await), ErrorCode::Unauthenticated);
    app.cleanup().await;
}

#[tokio::test]
async fn regenerate_and_disable() {
    let app = TestApp::start().await;
    let kim = user(&app, "kim").await;
    let client = auth(&app, Some(&kim.token));
    let (secret, old) = enable(&app, &kim).await;
    assert_eq!(code(client.begin_totp_setup(BeginTotpSetupRequest::default()).await), ErrorCode::FailedPrecondition);
    let fresh = client
        .regenerate_recovery_codes(RegenerateRecoveryCodesRequest { code: totp_now(&secret, now()), ..Default::default() })
        .await
        .unwrap()
        .into_owned()
        .recovery_codes;
    assert_eq!(fresh.len(), 10);
    let ch = challenge(&app, &app.name("kim")).await;
    assert_eq!(code(complete(&app, &ch, "", &old[0]).await), ErrorCode::Unauthenticated, "old codes are gone");

    assert_eq!(
        code(client.disable_totp(DisableTotpRequest { code: "000000".into(), ..Default::default() }).await),
        ErrorCode::Unauthenticated
    );
    client.disable_totp(DisableTotpRequest { code: fresh[0].clone(), ..Default::default() }).await.unwrap();
    let resp = auth(&app, None).login(login_req(&app.name("kim"), PASSWORD, "")).await.unwrap().into_owned();
    assert!(!resp.token.is_empty() && !resp.second_factor_required.is_set());
    app.cleanup().await;
}
