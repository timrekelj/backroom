//! Text + HTML bodies of every email the server sends.

use html_escape::encode_text;

use super::Email;

/// Wraps paragraphs into a minimal, client-safe HTML document.
fn html(title: &str, paragraphs: &[String]) -> String {
    let body: String = paragraphs
        .iter()
        .map(|p| format!("<p style=\"margin:0 0 16px\">{p}</p>"))
        .collect();
    format!(
        "<!doctype html><html><body style=\"font-family:-apple-system,Segoe UI,sans-serif;color:#1b1b1f;max-width:520px;margin:24px auto;padding:0 16px\">\
         <h2 style=\"font-size:18px;margin:0 0 16px\">{}</h2>{body}\
         <p style=\"color:#6b6b76;font-size:12px;margin-top:32px\">thehideout</p></body></html>",
        encode_text(title)
    )
}

fn code_html(code: &str) -> String {
    format!(
        "<span style=\"font-family:ui-monospace,Menlo,monospace;font-size:24px;letter-spacing:4px;font-weight:600\">{}</span>",
        encode_text(code)
    )
}

fn email(to: &str, key: String, subject: &str, text: String, html: String) -> Email {
    Email {
        to: to.to_owned(),
        subject: subject.to_owned(),
        text,
        html,
        idempotency_key: key,
    }
}

/// Invitation with the link and the short code.
pub fn invite(
    to: &str,
    key: String,
    inviter: Option<&str>,
    guilds: &[String],
    link: &str,
    code: &str,
) -> Email {
    let who = inviter.map_or_else(|| "You have".to_owned(), |name| format!("{name} has"));
    let into = if guilds.is_empty() {
        String::new()
    } else {
        format!(" and to join {}", guilds.join(", "))
    };
    let text = format!(
        "{who} invited you to create an account on thehideout{into}.\n\n\
         Accept the invite: {link}\n\nOr enter this code in the app: {code}\n\n\
         The invite expires in 7 days. If you weren't expecting it, ignore this email."
    );
    let html = html(
        "You're invited to thehideout",
        &[
            format!(
                "{} invited you to create an account on thehideout{}.",
                encode_text(&who),
                encode_text(&into)
            ),
            format!(
                "<a href=\"{0}\">Accept the invite</a> &middot; {0}",
                encode_text(link)
            ),
            format!("Or enter this code in the app: {}", code_html(code)),
            "The invite expires in 7 days. If you weren't expecting it, ignore this email.".into(),
        ],
    );
    email(to, key, "You're invited to thehideout", text, html)
}

/// Sign-in code.
pub fn sign_in_code(to: &str, key: String, code: &str) -> Email {
    let text = format!(
        "Your thehideout sign-in code is {code}\n\nIt expires in 10 minutes. If you didn't try to sign in, ignore this email."
    );
    let html = html(
        "Your sign-in code",
        &[
            code_html(code),
            "It expires in 10 minutes. If you didn't try to sign in, ignore this email.".into(),
        ],
    );
    email(
        to,
        key,
        &format!("{code} is your thehideout sign-in code"),
        text,
        html,
    )
}

/// Password reset code.
pub fn password_reset(to: &str, key: String, code: &str) -> Email {
    let text = format!(
        "Your thehideout password reset code is {code}\n\nIt expires in 10 minutes. If you didn't ask to reset your password, ignore this email."
    );
    let html = html(
        "Reset your password",
        &[
            code_html(code),
            "It expires in 10 minutes. If you didn't ask to reset your password, ignore this email.".into(),
        ],
    );
    email(to, key, "Reset your thehideout password", text, html)
}

/// Code confirming a new address.
pub fn email_change_code(to: &str, key: String, code: &str) -> Email {
    let text = format!(
        "Enter {code} in thehideout to use this address for your account.\n\nIt expires in 10 minutes. If you didn't ask for this, ignore this email."
    );
    let html = html(
        "Confirm your email",
        &[
            format!(
                "Enter {} in thehideout to use this address for your account.",
                code_html(code)
            ),
            "It expires in 10 minutes. If you didn't ask for this, ignore this email.".into(),
        ],
    );
    email(to, key, "Confirm your thehideout email", text, html)
}

/// Sent to the old address after an email change.
pub fn email_changed(to: &str, key: String, new_email: &str) -> Email {
    let masked = crate::accounts::mask_email(new_email);
    let text = format!(
        "The email of your thehideout account was changed to {masked}.\n\nIf this wasn't you, reset your password and contact an admin."
    );
    let html = html(
        "Your email was changed",
        &[
            format!(
                "The email of your thehideout account was changed to {}.",
                encode_text(&masked)
            ),
            "If this wasn't you, reset your password and contact an admin.".into(),
        ],
    );
    email(to, key, "Your thehideout email was changed", text, html)
}

/// A sign-in from a device the account has not used before.
pub fn new_device(to: &str, key: String, device: &str, method: &str) -> Email {
    let text = format!(
        "Your thehideout account was signed in on a new device: {device} (with {method}).\n\n\
         If this wasn't you, sign out that session in Settings, change your password and turn on two-factor authentication."
    );
    let html = html(
        "New sign-in",
        &[
            format!(
                "Your thehideout account was signed in on a new device: <b>{}</b> (with {}).",
                encode_text(device),
                encode_text(method)
            ),
            "If this wasn't you, sign out that session in Settings, change your password and turn on two-factor authentication."
                .into(),
        ],
    );
    email(to, key, "New sign-in to thehideout", text, html)
}

/// A security setting changed (password, two-factor, passkeys).
pub fn security_change(to: &str, key: String, what: &str) -> Email {
    let text = format!("Security change on your thehideout account: {what}.\n\nIf this wasn't you, reset your password and contact an admin.");
    let html = html(
        "Security change",
        &[
            format!(
                "Security change on your thehideout account: <b>{}</b>.",
                encode_text(what)
            ),
            "If this wasn't you, reset your password and contact an admin.".into(),
        ],
    );
    email(
        to,
        key,
        "Security change on your thehideout account",
        text,
        html,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invite_has_link_and_code_in_both_parts_and_escapes() {
        let e = invite(
            "a@x.org",
            "k".into(),
            Some("<b>"),
            &["g".into()],
            "https://h.example/invite/T",
            "ABCD-EFGH",
        );
        for part in [&e.text, &e.html] {
            assert!(part.contains("https://h.example/invite/T"));
            assert!(part.contains("ABCD-EFGH"));
        }
        assert!(e.html.contains("&lt;b&gt;") && !e.html.contains("<b> has"));
    }
}
