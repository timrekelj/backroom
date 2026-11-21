//! Passkeys end to end with a software authenticator (SoftPasskey).

mod common;

use common::TestApp;
use common::fixtures::*;
use connectrpc::{ConnectError, ErrorCode};
use thehideout_server::codes::totp_now;
use thehideout_server::proto::thehideout::v1::*;
use webauthn_authenticator_rs::WebauthnAuthenticator;
use webauthn_authenticator_rs::softpasskey::SoftPasskey;
use webauthn_rs::prelude::{CreationChallengeResponse, RequestChallengeResponse, Url};

fn code<T>(r: Result<T, ConnectError>) -> ErrorCode {
    r.err().expect("expected an error").code
}

fn origin(app: &TestApp) -> Url {
    app.state.public_url.clone()
}

/// Registers a passkey for `who` on `key`; returns it.
async fn register(app: &TestApp, who: &Account, key: &mut SoftPasskey, name: &str) -> Passkey {
    let client = auth(app, Some(&who.token));
    let ceremony = client
        .begin_passkey_registration(BeginPasskeyRegistrationRequest { name: name.into(), ..Default::default() })
        .await
        .unwrap()
        .into_owned()
        .ceremony
        .unwrap();
    assert_eq!(ceremony.ceremony_url, format!("{}/auth/passkey?ceremony={}", app.state.public_base(), ceremony.ceremony_id));
    let options: CreationChallengeResponse = serde_json::from_str(&ceremony.options_json).unwrap();
    let credential = key.do_registration(origin(app), options).expect("soft passkey registers");
    client
        .finish_passkey_registration(FinishPasskeyRegistrationRequest {
            ceremony_id: ceremony.ceremony_id,
            credential_json: serde_json::to_string(&credential).unwrap(),
            ..Default::default()
        })
        .await
        .unwrap()
        .into_owned()
        .passkey
        .unwrap()
}

async fn begin_login(app: &TestApp, username: &str) -> Result<PasskeyCeremony, ConnectError> {
    Ok(auth(app, None)
        .begin_passkey_login(BeginPasskeyLoginRequest { username: username.into(), device_name: "browser".into(), ..Default::default() })
        .await?
        .into_owned()
        .ceremony
        .unwrap())
}

async fn finish_login(app: &TestApp, ceremony_id: &str, json: &str) -> Result<FinishPasskeyLoginResponse, ConnectError> {
    Ok(auth(app, None)
        .finish_passkey_login(FinishPasskeyLoginRequest { ceremony_id: ceremony_id.into(), credential_json: json.into(), ..Default::default() })
        .await?
        .into_owned())
}

#[tokio::test]
async fn register_and_sign_in_with_a_passkey_skipping_totp() {
    let app = TestApp::start().await;
    let lee = user(&app, "lee").await;
    let mut key = SoftPasskey::new(true);
    let added = register(&app, &lee, &mut key, "  YubiKey  ").await;
    assert_eq!(added.name, "YubiKey");
    wait_mail(&app, &email_of(&app, "lee"), 0, "Security change").await;

    // Turn on TOTP: a passkey sign-in still needs nothing else.
    let client = auth(&app, Some(&lee.token));
    let setup = client.begin_totp_setup(BeginTotpSetupRequest::default()).await.unwrap().into_owned();
    let now = chrono::Utc::now().timestamp() as u64;
    client.confirm_totp(ConfirmTotpRequest { code: totp_now(&setup.secret, now), ..Default::default() }).await.unwrap();

    let ceremony = begin_login(&app, &email_of(&app, "lee")).await.unwrap();
    let options: RequestChallengeResponse = serde_json::from_str(&ceremony.options_json).unwrap();
    let credential = key.do_authentication(origin(&app), options).expect("soft passkey signs");
    let json = serde_json::to_string(&credential).unwrap();
    let signed = finish_login(&app, &ceremony.ceremony_id, &json).await.unwrap();
    assert!(!signed.pending && !signed.token.is_empty());
    assert_eq!(signed.user.unwrap().id, lee.id);
    auth(&app, Some(&signed.token)).get_me(GetMeRequest::default()).await.unwrap();
    // The ceremony is single use.
    assert_eq!(code(finish_login(&app, &ceremony.ceremony_id, &json).await), ErrorCode::NotFound);

    let listed = client.list_passkeys(ListPasskeysRequest::default()).await.unwrap().into_owned().passkeys;
    assert_eq!(listed.len(), 1);
    assert!(listed[0].last_used_at.is_set());
    app.cleanup().await;
}

#[tokio::test]
async fn ceremony_page_flow_with_polling() {
    let app = TestApp::start().await;
    let max = user(&app, "max").await;
    let mut key = SoftPasskey::new(true);
    register(&app, &max, &mut key, "").await;

    let ceremony = auth(&app, None)
        .begin_passkey_login(BeginPasskeyLoginRequest { username: app.name("max"), redirect_port: Some(45678), ..Default::default() })
        .await
        .unwrap()
        .into_owned()
        .ceremony
        .unwrap();
    // Before the page posts a result, Finish with no credential is pending.
    assert!(finish_login(&app, &ceremony.ceremony_id, "").await.unwrap().pending);

    // What the page does: fetch options, run WebAuthn, post the credential.
    let http = reqwest::Client::new();
    let page = http.get(ceremony.ceremony_url.replace(&app.state.public_base(), &app.base_url)).send().await.unwrap();
    assert_eq!(page.status(), 200);
    assert!(page.text().await.unwrap().contains("navigator.credentials"));
    let cfg: serde_json::Value = serde_json::from_str(
        &http
            .get(format!("{}/auth/passkey/options?ceremony={}", app.base_url, ceremony.ceremony_id))
            .send()
            .await
            .unwrap()
            .text()
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(cfg["kind"], "login");
    assert_eq!(cfg["redirect"], "http://127.0.0.1:45678/done");
    let options: RequestChallengeResponse = serde_json::from_value(cfg["options"].clone()).unwrap();
    let credential = key.do_authentication(origin(&app), options).unwrap();
    let posted = http
        .post(format!("{}/auth/passkey/result?ceremony={}", app.base_url, ceremony.ceremony_id))
        .body(serde_json::to_string(&credential).unwrap())
        .send()
        .await
        .unwrap();
    assert_eq!(posted.status(), 204);
    let signed = finish_login(&app, &ceremony.ceremony_id, "").await.unwrap();
    assert!(!signed.pending);
    assert_eq!(signed.user.unwrap().id, max.id);
    app.cleanup().await;
}

#[tokio::test]
async fn rename_delete_and_wrong_keys() {
    let app = TestApp::start().await;
    let ned = user(&app, "ned").await;
    let mut key = SoftPasskey::new(true);
    let pk = register(&app, &ned, &mut key, "old").await;
    let client = auth(&app, Some(&ned.token));
    let renamed = client
        .rename_passkey(RenamePasskeyRequest { passkey_id: pk.id, name: "laptop".into(), ..Default::default() })
        .await
        .unwrap()
        .into_owned()
        .passkey
        .unwrap();
    assert_eq!(renamed.name, "laptop");
    assert_eq!(
        code(client.rename_passkey(RenamePasskeyRequest { passkey_id: pk.id, name: " ".into(), ..Default::default() }).await),
        ErrorCode::InvalidArgument
    );
    // Someone else cannot touch it.
    let other = user(&app, "other").await;
    assert_eq!(
        code(auth(&app, Some(&other.token)).delete_passkey(DeletePasskeyRequest { passkey_id: pk.id, ..Default::default() }).await),
        ErrorCode::NotFound
    );
    // A different authenticator cannot sign in.
    let ceremony = begin_login(&app, &app.name("ned")).await.unwrap();
    let options: RequestChallengeResponse = serde_json::from_str(&ceremony.options_json).unwrap();
    let mut stranger = SoftPasskey::new(true);
    assert!(stranger.do_authentication(origin(&app), options).is_err(), "soft key refuses unknown credential ids");

    client.delete_passkey(DeletePasskeyRequest { passkey_id: pk.id, ..Default::default() }).await.unwrap();
    assert!(client.list_passkeys(ListPasskeysRequest::default()).await.unwrap().into_owned().passkeys.is_empty());
    assert_eq!(code(begin_login(&app, &app.name("ned")).await), ErrorCode::FailedPrecondition);
    assert_eq!(code(begin_login(&app, "nobody").await), ErrorCode::FailedPrecondition);
    app.cleanup().await;
}

async fn begin_second_factor(app: &TestApp, challenge_id: &str) -> Result<PasskeyCeremony, ConnectError> {
    Ok(auth(app, None)
        .begin_second_factor_passkey(BeginSecondFactorPasskeyRequest { challenge_id: challenge_id.into(), ..Default::default() })
        .await?
        .into_owned()
        .ceremony
        .unwrap())
}

async fn finish_second_factor(app: &TestApp, ceremony_id: &str, json: &str) -> Result<FinishSecondFactorPasskeyResponse, ConnectError> {
    Ok(auth(app, None)
        .finish_second_factor_passkey(FinishSecondFactorPasskeyRequest {
            ceremony_id: ceremony_id.into(),
            credential_json: json.into(),
            ..Default::default()
        })
        .await?
        .into_owned())
}

/// A password sign-in of `login` that stops at the second factor: the challenge.
async fn second_factor_challenge(app: &TestApp, login: &str) -> SecondFactorChallenge {
    let resp = auth(app, None).login(login_req(login, PASSWORD, "desk")).await.unwrap().into_owned();
    resp.second_factor_required.into_option().expect("second factor required")
}

fn sign(app: &TestApp, key: &mut SoftPasskey, ceremony: &PasskeyCeremony) -> String {
    let options: RequestChallengeResponse = serde_json::from_str(&ceremony.options_json).unwrap();
    serde_json::to_string(&key.do_authentication(origin(app), options).expect("soft passkey signs")).unwrap()
}

#[tokio::test]
async fn a_passkey_answers_the_second_factor_challenge() {
    let app = TestApp::start().await;
    let oli = user(&app, "oli").await;
    let login = app.name("oli");
    let client = auth(&app, Some(&oli.token));
    let setup = client.begin_totp_setup(BeginTotpSetupRequest::default()).await.unwrap().into_owned();
    let now = chrono::Utc::now().timestamp() as u64;
    client.confirm_totp(ConfirmTotpRequest { code: totp_now(&setup.secret, now), ..Default::default() }).await.unwrap();
    let passkey = SecondFactorMethod::SECOND_FACTOR_METHOD_PASSKEY;

    // Without a passkey, it is not offered (and cannot be used).
    let ch = second_factor_challenge(&app, &login).await;
    assert!(!ch.methods.contains(&passkey.into()));
    assert_eq!(code(begin_second_factor(&app, &ch.challenge_id).await), ErrorCode::FailedPrecondition);

    let mut key = SoftPasskey::new(true);
    register(&app, &oli, &mut key, "laptop").await;
    let ch = second_factor_challenge(&app, &login).await;
    assert!(ch.methods.contains(&passkey.into()), "offered once the account has a passkey");
    assert_eq!(
        code_error(begin_second_factor(&app, "no-such-challenge").await),
        (ErrorCode::FailedPrecondition, Some((CodeErrorReason::CODE_ERROR_REASON_EXPIRED, 0)))
    );

    let first = begin_second_factor(&app, &ch.challenge_id).await.unwrap();
    assert!(finish_second_factor(&app, &first.ceremony_id, "").await.unwrap().pending, "the page has no result yet");
    // Not a sign-in ceremony.
    assert_eq!(code(finish_login(&app, &first.ceremony_id, "").await), ErrorCode::NotFound);

    // An answer signed for another ceremony is a wrong answer to the challenge.
    let second = begin_second_factor(&app, &ch.challenge_id).await.unwrap();
    let json = sign(&app, &mut key, &second);
    assert_eq!(
        code_error(finish_second_factor(&app, &first.ceremony_id, &json).await),
        (ErrorCode::InvalidArgument, Some((CodeErrorReason::CODE_ERROR_REASON_WRONG, 4)))
    );
    let signed = finish_second_factor(&app, &second.ceremony_id, &json).await.unwrap();
    assert!(!signed.pending && !signed.token.is_empty());
    assert_eq!(signed.user.unwrap().id, oli.id);
    auth(&app, Some(&signed.token)).get_me(GetMeRequest::default()).await.unwrap();

    // The challenge is used up.
    let again = auth(&app, None)
        .complete_second_factor(CompleteSecondFactorRequest {
            challenge_id: ch.challenge_id.clone(),
            totp_code: totp_now(&setup.secret, now + 30),
            ..Default::default()
        })
        .await;
    assert_eq!(code_error(again), (ErrorCode::FailedPrecondition, Some((CodeErrorReason::CODE_ERROR_REASON_EXPIRED, 0))));
    let security = client.get_security(GetSecurityRequest::default()).await.unwrap().into_owned();
    assert_eq!(security.passkey_count, 1);
    app.cleanup().await;
}
