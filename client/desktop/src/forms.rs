//! Form state shared by the auth screen and the dialogs: which mode, what is typed, what went
//! wrong. Pure data; `AppState` owns the instances and the transitions that submit them.

use std::rc::Rc;

use rune::TextEdit;
use thehideout_api::{ApiError, Channel};
use thehideout_design::icons::catalog::{self, CatalogIcon};

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
    CreateSection { guild: u64 },
    RenameSection { guild: u64, section: u64 },
    /// A confirmation: no field, only "Delete section".
    DeleteSection { guild: u64, section: u64 },
    ChannelSettings { guild: u64, channel: u64 },
}

/// The look and place fields of the channel dialogs (create and settings): where the channel
/// goes, its icon and colour, topic, and the icon picker's search.
pub struct ChannelForm {
    /// `None` = top level. Only the create dialog offers the choice.
    pub section: Option<u64>,
    pub section_open: bool,
    /// Phosphor name, `""` = the default `#`.
    pub icon: String,
    /// Palette key, `""` = none.
    pub color: &'static str,
    pub topic: TextEdit,
    pub picker_open: bool,
    /// The picker tab; only 0 (Icons) is populated so far.
    pub tab: usize,
    pub search: TextEdit,
    /// The keyboard-chosen result in `results`.
    pub highlight: Option<usize>,
    pub results: Rc<Vec<&'static CatalogIcon>>,
    /// What the channel had when the dialog opened (settings), to send only what changed.
    pub original: Option<Channel>,
}

impl ChannelForm {
    pub fn new(section: Option<u64>) -> ChannelForm {
        ChannelForm {
            section,
            section_open: false,
            icon: String::new(),
            color: "",
            topic: TextEdit::new(""),
            picker_open: false,
            tab: 0,
            search: TextEdit::new(""),
            highlight: None,
            results: Rc::new(catalog::search("")),
            original: None,
        }
    }

    pub fn of(c: &Channel) -> ChannelForm {
        let mut f = ChannelForm::new(c.section_id);
        f.icon = c.icon.0.clone();
        f.color = thehideout_design::PALETTE_KEYS.iter().copied().find(|k| *k == c.color.as_key()).unwrap_or("");
        f.topic = TextEdit::new(&c.topic);
        f.original = Some(c.clone());
        f
    }

    /// Recomputes `results` after the search text changed, and puts the highlight on the best one.
    pub fn research(&mut self) {
        self.results = Rc::new(catalog::search(&self.search.text));
        self.highlight = (!self.results.is_empty()).then_some(0);
    }

    /// Arrow keys in the grid: `dx` columns, `dy` rows. Starts at the first result.
    pub fn step_highlight(&mut self, dx: i32, dy: i32) {
        use thehideout_design::components::chat::pickers::GRID_COLS;
        let n = self.results.len() as i32;
        if n == 0 {
            return;
        }
        let at = self.highlight.map_or(-dx - dy * GRID_COLS as i32, |h| h as i32) + dx + dy * GRID_COLS as i32;
        self.highlight = Some(at.clamp(0, n - 1) as usize);
    }
}

pub struct DialogState {
    pub kind: DialogKind,
    pub input: TextEdit,
    /// Present for the channel dialogs.
    pub channel: Option<ChannelForm>,
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

