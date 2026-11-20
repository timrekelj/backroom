//! Shared helpers of the live tests.
#![allow(dead_code)]

use thehideout_api::{Client, User};
use thehideout_server::AppState;
use thehideout_server::accounts::{NewUser, create_user};

pub const PASSWORD: &str = "correct horse battery";

/// The email [`sign_up`] gives `username`.
pub fn email(username: &str) -> String {
    format!("{username}@example.test")
}

/// Creates the account `username` directly on the in-process server
/// (accounts are invite-only) and signs `client` in as it.
pub async fn sign_up(state: &AppState, client: &Client, username: &str) -> (String, User) {
    let email = email(username);
    let account = create_user(state, NewUser { username, password: Some(PASSWORD), email: Some(&email) })
        .await
        .expect("create user");
    let token = thehideout_server::auth::create_session(state, account.id, "test").await.expect("create session");
    client.set_token(Some(token.clone()));
    let me = client.get_me().await.expect("get me");
    (token, me)
}

/// The newest message to `to` in the server's in-memory outbox, waiting up to 5 s
/// for one whose subject contains `subject`.
pub async fn mail(state: &AppState, to: &str, subject: &str) -> thehideout_server::email::Email {
    let outbox = state.mailer.outbox.clone().expect("in-memory mailer");
    for _ in 0..250 {
        if let Some(m) = outbox.to(to).into_iter().rev().find(|m| m.subject.contains(subject)) {
            return m;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    panic!("no mail {subject:?} to {to}");
}
