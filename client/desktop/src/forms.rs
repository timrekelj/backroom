//! Form state shared by the auth screen and the dialogs: which mode, what is typed, what went
//! wrong. Pure data; `AppState` owns the instances and the transitions that submit them.

use rune::TextEdit;
use thehideout_api::ApiError;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Phase {
    #[default]
    Idle,
    Busy,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AuthMode {
    Login,
    SignUp,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AuthField {
    Username,
    Password,
    /// Under the button: the problem isn't one field's.
    Form,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AuthError {
    pub field: AuthField,
    pub message: String,
}

pub struct Auth {
    pub mode: AuthMode,
    pub username: TextEdit,
    pub password: TextEdit,
    pub phase: Phase,
    pub error: Option<AuthError>,
    pub server: TextEdit,
    pub server_open: bool,
}

impl Auth {
    pub fn new(server: &str) -> Auth {
        Auth {
            mode: AuthMode::Login,
            username: { let mut e = TextEdit::new(""); e.focused = true; e },
            password: TextEdit::new("").secure(),
            phase: Phase::Idle,
            error: None,
            server: TextEdit::new(server),
            server_open: false,
        }
    }
}

/// What an `ApiError` means on the auth form.
pub fn auth_error(e: &ApiError) -> AuthError {
    let (field, message) = match e {
        ApiError::Unauthenticated => (AuthField::Form, "Wrong username or password.".to_owned()),
        ApiError::AlreadyExists => (AuthField::Username, "That username is taken.".to_owned()),
        ApiError::InvalidArgument(m) => (AuthField::Form, sentence(m)),
        ApiError::RateLimited => (AuthField::Form, "Too many attempts. Try again in a moment.".to_owned()),
        ApiError::Network(_) => (AuthField::Form, "Can't reach the server.".to_owned()),
        other => (AuthField::Form, sentence(&other.to_string())),
    };
    AuthError { field, message }
}

/// Capitalised, ending in a full stop.
pub fn sentence(m: &str) -> String {
    let m = m.trim();
    let mut c = m.chars();
    let mut s: String = c.next().into_iter().flat_map(char::to_uppercase).chain(c).collect();
    if !s.is_empty() && !s.ends_with(['.', '!', '?']) {
        s.push('.');
    }
    s
}


#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DialogKind {
    CreateGuild,
    JoinGuild,
    CreateChannel { guild: u64 },
    Invite { guild: u64 },
    NewDm,
}

pub struct DialogState {
    pub kind: DialogKind,
    pub input: TextEdit,
    pub error: Option<String>,
    pub phase: Phase,
}

pub fn check_name(name: &str, min: usize, max: usize, empty: &str) -> Result<(), String> {
    let n = name.chars().count();
    if n == 0 {
        Err(empty.into())
    } else if n < min || n > max {
        Err(format!("Use {min} to {max} characters"))
    } else {
        Ok(())
    }
}

