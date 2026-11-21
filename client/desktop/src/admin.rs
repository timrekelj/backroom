//! The site-admin dashboard on `AppState`: which page is open, each page's data and paging, the
//! optimistic file deletion (rows leave the list at once and come back if the server refuses),
//! the confirmations, and the replies that fill it all in. Pure state and commands, like the rest
//! of the model. Everything here is gated on `User::is_admin`; a server that doesn't have the
//! service answers `Unimplemented`, which the page shows as "Admin service not available".

use std::collections::{HashMap, HashSet, VecDeque};

use rune::TextEdit;
use thehideout_api::{
    ActivityPoint, AdminFileKind, AdminFileRow, AdminGuildRow, AdminUserRow, ApiError, AuditEntry,
    Invite, InviteStatus, NewInvite, Overview, Paged, StorageBreakdown, User,
};
use thehideout_design::components::{ChartDay, ToastKind};

use crate::backend::Command;
use crate::format::{day_label, parse_date, short_date};
use crate::server_settings::error_text;
use crate::state::{AppState, Menu};

/// Rows asked for per page.
pub const PAGE_SIZE: u32 = 40;
/// The activity ranges, in days.
pub const RANGES: [u32; 3] = [7, 30, 90];
/// A picture is only fetched for a row when the file is this small; bigger files show their type.
pub const THUMB_MAX_BYTES: u64 = 2 * 1024 * 1024;
/// Rows of a list whose pictures are fetched at most.
const THUMB_ROWS: usize = 120;

pub const SELF_DEMOTE: &str = "You can't remove your own admin rights.";
pub const LAST_ADMIN: &str = "There must be at least one admin.";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum AdminPage {
    Overview,
    Servers,
    Files,
    Users,
    Invites,
    Audit,
}

/// The statuses the Invites filter offers, after "All".
pub const INVITE_STATUSES: [InviteStatus; 4] =
    [InviteStatus::Pending, InviteStatus::Accepted, InviteStatus::Expired, InviteStatus::Revoked];

/// The admin's "Create invite" dialog: an email, the servers to pre-join and whether the new
/// account is a site admin.
pub struct InviteForm {
    pub email: TextEdit,
    pub guilds: Vec<u64>,
    pub make_admin: bool,
    pub error: Option<String>,
    pub busy: bool,
}

/// One paged list: what is loaded, where the next page starts, and what is in flight. `epoch`
/// changes whenever the list restarts (a search, a filter), so an answer to an older question
/// is dropped.
pub struct Feed<T> {
    pub items: Vec<T>,
    pub next: Option<String>,
    pub loaded: bool,
    pub pending: bool,
    pub error: Option<String>,
    pub epoch: u64,
}

impl<T> Default for Feed<T> {
    fn default() -> Self {
        Feed {
            items: Vec::new(),
            next: None,
            loaded: false,
            pending: false,
            error: None,
            epoch: 0,
        }
    }
}

impl<T> Feed<T> {
    /// Starts the list over (the old answers are now stale).
    pub fn reset(&mut self) {
        self.items.clear();
        self.next = None;
        self.loaded = false;
        self.pending = false;
        self.error = None;
        self.epoch += 1;
    }

    /// The request that is due: `(epoch, page token)`, marked in flight. None while one is out, after a
    /// failure (the page offers a retry), and once the last page is in.
    pub fn due(&mut self) -> Option<(u64, Option<String>)> {
        if self.pending || self.error.is_some() || (self.loaded && self.next.is_none()) {
            return None;
        }
        self.pending = true;
        Some((self.epoch, self.next.clone()))
    }

    /// An answer. `None` when it was for an older epoch; else the error it brought, if any.
    fn accept(
        &mut self,
        epoch: u64,
        result: Result<Paged<T>, ApiError>,
    ) -> Option<Option<ApiError>> {
        if epoch != self.epoch {
            return None;
        }
        self.pending = false;
        Some(match result {
            Ok(page) => {
                self.items.extend(page.items);
                self.next = page.next_page_token;
                self.loaded = true;
                self.error = None;
                None
            }
            Err(e) => {
                self.error = Some(error_text(&e));
                Some(e)
            }
        })
    }
}

/// A question waiting for "Delete" or "Confirm".
pub enum Confirm {
    DeleteFiles(Box<DeleteFiles>),
    SetAdmin { user: u64, make: bool },
}

/// The delete confirmation's payload, boxed out of [`Confirm`]: it holds the reason field.
pub struct DeleteFiles {
    pub keys: Vec<String>,
    pub reason: TextEdit,
}

pub struct AdminState {
    pub page: AdminPage,
    /// The server answered `Unimplemented`: every page says so.
    pub unavailable: bool,
    pub overview: Option<Overview>,
    pub overview_pending: bool,
    pub overview_error: Option<String>,
    pub range: u32,
    /// The points shown and the range they are for.
    pub activity: Option<(u32, Vec<ActivityPoint>)>,
    pub activity_pending: Option<u32>,
    pub hover_day: Option<usize>,
    pub servers: Feed<AdminGuildRow>,
    pub server_search: TextEdit,
    pub users: Feed<AdminUserRow>,
    pub user_search: TextEdit,
    pub files: Feed<AdminFileRow>,
    pub file_guild: Option<u64>,
    pub file_kind: Option<AdminFileKind>,
    pub guild_menu: bool,
    pub kind_menu: bool,
    pub selected: HashSet<String>,
    pub audit: Feed<AuditEntry>,
    pub invites: Feed<Invite>,
    /// The Invites page shows only this status (`None`: all). The server lists them all; the
    /// filter is applied to what is loaded.
    pub invite_filter: Option<InviteStatus>,
    pub invite_filter_menu: bool,
    pub invite_form: Option<InviteForm>,
    /// The invite just made or sent again, with its code (shown once above the list).
    pub invite_created: Option<Invite>,
    pub invite_busy: HashSet<u64>,
    pub page_size: u32,
    /// Pages whose data the user changed meanwhile: reloaded when next opened.
    pub stale: HashSet<AdminPage>,
    pub confirm: Option<Confirm>,
    pub user_errors: HashMap<u64, String>,
    pub busy_users: HashSet<u64>,
    /// Every server seen so far (by id), for the Files filter.
    pub known_guilds: Vec<(u64, String)>,
    /// Rows taken out of the files list while their deletion is out: put back if it fails.
    pub removed: HashMap<String, AdminFileRow>,
    /// Deletions not yet sent (one is in flight at a time) with their reason.
    pub queue: VecDeque<(String, String)>,
    pub in_flight: Option<String>,
    pub done: usize,
    pub total: usize,
    pub failed: Vec<(String, String)>,
}

impl AdminState {
    pub fn new() -> AdminState {
        AdminState {
            page: AdminPage::Overview,
            unavailable: false,
            overview: None,
            overview_pending: false,
            overview_error: None,
            range: 30,
            activity: None,
            activity_pending: None,
            hover_day: None,
            servers: Feed::default(),
            server_search: TextEdit::new(""),
            users: Feed::default(),
            user_search: TextEdit::new(""),
            files: Feed::default(),
            file_guild: None,
            file_kind: None,
            guild_menu: false,
            kind_menu: false,
            selected: HashSet::new(),
            audit: Feed::default(),
            invites: Feed::default(),
            invite_filter: None,
            invite_filter_menu: false,
            invite_form: None,
            invite_created: None,
            invite_busy: HashSet::new(),
            page_size: PAGE_SIZE,
            stale: HashSet::new(),
            confirm: None,
            user_errors: HashMap::new(),
            busy_users: HashSet::new(),
            known_guilds: Vec::new(),
            removed: HashMap::new(),
            queue: VecDeque::new(),
            in_flight: None,
            done: 0,
            total: 0,
            failed: Vec::new(),
        }
    }

    /// Deletions finished (or refused) and asked for, as `(answered, total)`; `None` when nothing is being deleted.
    pub fn progress(&self) -> Option<(usize, usize)> {
        (self.total > 0).then_some((self.done + self.failed.len(), self.total))
    }

    fn remember_guild(&mut self, id: u64, name: &str) {
        match self.known_guilds.iter_mut().find(|(g, _)| *g == id) {
            Some(entry) => entry.1 = name.to_owned(),
            None => {
                self.known_guilds.push((id, name.to_owned()));
                self.known_guilds.sort_by_key(|(_, n)| n.to_lowercase());
            }
        }
    }

    pub fn guild_name(&self, id: u64) -> Option<&str> {
        self.known_guilds
            .iter()
            .find(|(g, _)| *g == id)
            .map(|(_, n)| n.as_str())
    }
}

impl Default for AdminState {
    fn default() -> Self {
        AdminState::new()
    }
}

/// What a file row is called: an attachment its file name, anything else what it is.
pub fn file_title(row: &AdminFileRow) -> String {
    match row.kind {
        AdminFileKind::Attachment if !row.filename.is_empty() => row.filename.clone(),
        AdminFileKind::Attachment => "Attachment".to_owned(),
        AdminFileKind::Emoji => "Custom emoji".to_owned(),
        AdminFileKind::GuildIcon => "Server icon".to_owned(),
        AdminFileKind::Avatar => "Profile picture".to_owned(),
        AdminFileKind::EmbedThumbnail => "Link preview image".to_owned(),
        AdminFileKind::Unknown => "File".to_owned(),
    }
}

pub fn kind_label(kind: AdminFileKind) -> &'static str {
    match kind {
        AdminFileKind::Attachment => "Attachment",
        AdminFileKind::Emoji => "Emoji",
        AdminFileKind::GuildIcon => "Server icon",
        AdminFileKind::Avatar => "Avatar",
        AdminFileKind::EmbedThumbnail => "Link preview",
        AdminFileKind::Unknown => "Other",
    }
}

/// The kinds the Files filter offers, after "All kinds".
pub const FILE_KINDS: [AdminFileKind; 5] = [
    AdminFileKind::Attachment,
    AdminFileKind::Emoji,
    AdminFileKind::GuildIcon,
    AdminFileKind::Avatar,
    AdminFileKind::EmbedThumbnail,
];

/// Whether a row is a picture the list may fetch and show.
pub fn has_thumbnail(row: &AdminFileRow) -> bool {
    let image = match row.kind {
        AdminFileKind::Attachment => row.content_type.starts_with("image/"),
        AdminFileKind::Unknown => false,
        _ => true,
    };
    image && row.bytes <= THUMB_MAX_BYTES
}

/// The five storage categories in the order the bar and legend use: key, label, bytes.
pub fn storage_parts(s: &StorageBreakdown) -> [(&'static str, &'static str, u64); 5] {
    [
        ("attachments", "Attachments", s.attachments),
        ("emojis", "Emojis", s.emojis),
        ("icons", "Server icons", s.guild_icons),
        ("avatars", "Avatars", s.avatars),
        ("embeds", "Link previews", s.embeds),
    ]
}

impl AppState {
    pub fn is_admin(&self) -> bool {
        self.me().is_some_and(|u| u.is_admin)
    }

    pub fn admin(&self) -> Option<&AdminState> {
        self.ui.admin.as_ref()
    }

    // ---- open and close ----------------------------------------------------------------------

    /// "Admin" in the account menu, or Cmd/Ctrl+Shift+A. Nothing for a user who isn't an admin.
    pub fn open_admin(&mut self) {
        self.ui.menu = Menu::None;
        if !self.is_admin() || self.screen != crate::state::Screen::Main {
            return;
        }
        if self.ui.admin.is_some() {
            return;
        }
        self.ui.settings = None;
        self.ui.server_settings = None;
        self.ui.admin = Some(AdminState::new());
        self.want_admin();
    }

    pub fn close_admin(&mut self) {
        self.ui.admin = None;
    }

    pub fn admin_page(&mut self, page: AdminPage) {
        let Some(ad) = self.ui.admin.as_mut() else {
            return;
        };
        ad.page = page;
        ad.guild_menu = false;
        ad.kind_menu = false;
        // Invites are accepted in other people's inboxes, not here: the tab always asks again.
        if page == AdminPage::Invites {
            ad.stale.insert(AdminPage::Invites);
        }
        self.invalidate_stale(page);
        self.want_admin();
    }

    /// A member joined a server (maybe through an invite): the Invites tab asks again, now if it
    /// is open, else when it is next opened.
    pub(crate) fn admin_invites_changed(&mut self) {
        let Some(ad) = self.ui.admin.as_mut() else {
            return;
        };
        if ad.page == AdminPage::Invites {
            ad.invites.reset();
            self.want_admin();
        } else {
            ad.stale.insert(AdminPage::Invites);
        }
    }

    /// A page changed by the user's own actions starts over when it is opened.
    fn invalidate_stale(&mut self, page: AdminPage) {
        let Some(ad) = self.ui.admin.as_mut() else {
            return;
        };
        if !ad.stale.remove(&page) {
            return;
        }
        match page {
            AdminPage::Overview => {
                ad.overview = None;
                ad.activity = None;
            }
            AdminPage::Servers => ad.servers.reset(),
            AdminPage::Files => ad.files.reset(),
            AdminPage::Users => ad.users.reset(),
            AdminPage::Invites => ad.invites.reset(),
            AdminPage::Audit => ad.audit.reset(),
        }
    }

    /// Asks for whatever the open page lacks (first pages, the overview, the activity). Returns
    /// whether anything was queued.
    pub fn want_admin(&mut self) -> bool {
        let Some(ad) = self.ui.admin.as_mut() else {
            return false;
        };
        if ad.unavailable {
            return false;
        }
        let mut queued = false;
        let size = ad.page_size;
        match ad.page {
            AdminPage::Overview => {
                if ad.overview.is_none() && !ad.overview_pending && ad.overview_error.is_none() {
                    ad.overview_pending = true;
                    self.outbox.push(Command::AdminOverview);
                    queued = true;
                }
                let days = ad.range;
                if ad.activity.as_ref().map(|(d, _)| *d) != Some(days)
                    && ad.activity_pending != Some(days)
                {
                    ad.activity_pending = Some(days);
                    self.outbox.push(Command::AdminActivity { days });
                    queued = true;
                }
            }
            AdminPage::Servers => {
                if let Some((epoch, page_token)) = ad.servers.due() {
                    self.outbox.push(Command::AdminGuilds {
                        epoch,
                        query: ad.server_search.text.trim().to_owned(),
                        page_token,
                        limit: size,
                    });
                    queued = true;
                }
            }
            AdminPage::Files => {
                if let Some((epoch, page_token)) = ad.files.due() {
                    self.outbox.push(Command::AdminFiles {
                        epoch,
                        guild: ad.file_guild,
                        kind: ad.file_kind,
                        page_token,
                        limit: size,
                    });
                    queued = true;
                }
                // The server filter lists every server: keep their pages coming until all are known.
                if let Some((epoch, page_token)) = ad.servers.due() {
                    self.outbox.push(Command::AdminGuilds {
                        epoch,
                        query: ad.server_search.text.trim().to_owned(),
                        page_token,
                        limit: size,
                    });
                    queued = true;
                }
            }
            AdminPage::Users => {
                if let Some((epoch, page_token)) = ad.users.due() {
                    self.outbox.push(Command::AdminUsers {
                        epoch,
                        query: ad.user_search.text.trim().to_owned(),
                        page_token,
                        limit: size,
                    });
                    queued = true;
                }
            }
            AdminPage::Invites => {
                if let Some((epoch, page_token)) = ad.invites.due() {
                    self.outbox.push(Command::ListInvites {
                        epoch,
                        guild: None,
                        page_token,
                        limit: size,
                    });
                    queued = true;
                }
                // Server names for the rows and the create dialog's choices.
                if let Some((epoch, page_token)) = ad.servers.due() {
                    self.outbox.push(Command::AdminGuilds {
                        epoch,
                        query: String::new(),
                        page_token,
                        limit: size,
                    });
                    queued = true;
                }
            }
            AdminPage::Audit => {
                if let Some((epoch, page_token)) = ad.audit.due() {
                    self.outbox.push(Command::AdminAudit {
                        epoch,
                        page_token,
                        limit: size,
                    });
                    queued = true;
                }
            }
        }
        queued
    }

    /// "Try again" on a failed or unavailable page.
    pub fn retry_admin(&mut self) {
        let Some(ad) = self.ui.admin.as_mut() else {
            return;
        };
        ad.unavailable = false;
        match ad.page {
            AdminPage::Overview => {
                ad.overview_error = None;
                ad.overview = None;
                ad.activity = None;
                ad.activity_pending = None;
                ad.overview_pending = false;
            }
            AdminPage::Servers => ad.servers.reset(),
            AdminPage::Files => ad.files.reset(),
            AdminPage::Users => ad.users.reset(),
            AdminPage::Invites => ad.invites.reset(),
            AdminPage::Audit => ad.audit.reset(),
        }
        self.want_admin();
    }

    /// The end of a list came into view: its next page. Returns whether anything was queued.
    pub fn admin_more(&mut self) -> bool {
        self.want_admin()
    }

    pub fn set_admin_range(&mut self, days: u32) {
        let Some(ad) = self.ui.admin.as_mut() else {
            return;
        };
        if !RANGES.contains(&days) {
            return;
        }
        ad.range = days;
        ad.hover_day = None;
        self.want_admin();
    }

    pub fn hover_admin_day(&mut self, day: usize, over: bool) {
        let Some(ad) = self.ui.admin.as_mut() else {
            return;
        };
        if over {
            ad.hover_day = Some(day);
        } else if ad.hover_day == Some(day) {
            ad.hover_day = None;
        }
    }

    // ---- searching and filtering -------------------------------------------------------------

    /// The Servers or Users search box changed: the list starts over for the new text.
    pub fn admin_search_changed(&mut self, page: AdminPage) {
        let Some(ad) = self.ui.admin.as_mut() else {
            return;
        };
        match page {
            AdminPage::Servers => ad.servers.reset(),
            AdminPage::Users => ad.users.reset(),
            _ => return,
        }
        self.want_admin();
    }

    /// A server row was pressed: the Files page, for that server only.
    pub fn admin_files_of(&mut self, guild: u64) {
        let Some(ad) = self.ui.admin.as_mut() else {
            return;
        };
        ad.file_guild = Some(guild);
        ad.files.reset();
        ad.selected.clear();
        ad.stale.remove(&AdminPage::Files);
        self.admin_page(AdminPage::Files);
    }

    pub fn filter_files_guild(&mut self, guild: Option<u64>) {
        let Some(ad) = self.ui.admin.as_mut() else {
            return;
        };
        ad.guild_menu = false;
        if ad.file_guild == guild {
            return;
        }
        ad.file_guild = guild;
        ad.files.reset();
        ad.selected.clear();
        self.want_admin();
    }

    pub fn filter_files_kind(&mut self, kind: Option<AdminFileKind>) {
        let Some(ad) = self.ui.admin.as_mut() else {
            return;
        };
        ad.kind_menu = false;
        if ad.file_kind == kind {
            return;
        }
        ad.file_kind = kind;
        ad.files.reset();
        ad.selected.clear();
        self.want_admin();
    }

    // ---- deleting files ----------------------------------------------------------------------

    pub fn toggle_file_selected(&mut self, key: &str) {
        let Some(ad) = self.ui.admin.as_mut() else {
            return;
        };
        if !ad.selected.remove(key) && ad.files.items.iter().any(|r| r.key == key) {
            ad.selected.insert(key.to_owned());
        }
    }

    /// The header checkbox: everything loaded, or nothing when everything already is.
    pub fn toggle_all_files(&mut self) {
        let Some(ad) = self.ui.admin.as_mut() else {
            return;
        };
        if !ad.files.items.is_empty() && ad.files.items.iter().all(|r| ad.selected.contains(&r.key))
        {
            ad.selected.clear();
        } else {
            ad.selected = ad.files.items.iter().map(|r| r.key.clone()).collect();
        }
    }

    /// "Delete" on a row, or on the selection: opens the confirmation (nothing is sent yet).
    pub fn ask_delete_files(&mut self, keys: Vec<String>) {
        let Some(ad) = self.ui.admin.as_mut() else {
            return;
        };
        let keys: Vec<String> = keys
            .into_iter()
            .filter(|k| ad.files.items.iter().any(|r| &r.key == k))
            .collect();
        if keys.is_empty() {
            return;
        }
        let mut reason = TextEdit::new("");
        reason.focused = true;
        ad.confirm = Some(Confirm::DeleteFiles(Box::new(DeleteFiles { keys, reason })));
    }

    pub fn cancel_admin_confirm(&mut self) {
        if let Some(ad) = self.ui.admin.as_mut() {
            ad.confirm = None;
        }
    }

    /// "Delete" in the confirmation: the rows leave the list now, the deletions go out one after the
    /// other, and a refused one puts its row back.
    pub fn confirm_delete_files(&mut self) {
        let Some(ad) = self.ui.admin.as_mut() else {
            return;
        };
        let Some(Confirm::DeleteFiles(d)) = ad.confirm.take() else {
            return;
        };
        let reason = d.reason.text.trim().to_owned();
        let keys = d.keys;
        if ad.queue.is_empty() && ad.in_flight.is_none() {
            ad.done = 0;
            ad.total = 0;
            ad.failed.clear();
        }
        for key in keys {
            if let Some(at) = ad.files.items.iter().position(|r| r.key == key) {
                let row = ad.files.items.remove(at);
                ad.removed.insert(key.clone(), row);
                ad.selected.remove(&key);
                ad.queue.push_back((key, reason.clone()));
                ad.total += 1;
            }
        }
        self.pump_deletes();
    }

    fn pump_deletes(&mut self) {
        let Some(ad) = self.ui.admin.as_mut() else {
            return;
        };
        if ad.in_flight.is_some() {
            return;
        }
        if let Some((key, reason)) = ad.queue.pop_front() {
            ad.in_flight = Some(key.clone());
            self.outbox.push(Command::AdminDeleteFile { key, reason });
        }
    }

    /// The answer to one deletion.
    pub fn admin_file_deleted(&mut self, key: &str, result: Result<(), ApiError>) {
        let Some(ad) = self.ui.admin.as_mut() else {
            return;
        };
        if ad.in_flight.as_deref() == Some(key) {
            ad.in_flight = None;
        }
        let row = ad.removed.remove(key);
        match result {
            Ok(()) => ad.done += 1,
            Err(e) => {
                let why = if matches!(e, ApiError::Unimplemented) {
                    "The admin service isn't available.".to_owned()
                } else {
                    error_text(&e)
                };
                if let Some(row) = row {
                    // The list is sorted by size, largest first: the row goes back where it was.
                    let at = ad.files.items.partition_point(|r| r.bytes >= row.bytes);
                    let name = file_title(&row);
                    ad.files.items.insert(at, row);
                    ad.failed.push((name, why));
                } else {
                    ad.failed.push((key.to_owned(), why));
                }
            }
        }
        self.pump_deletes();
        self.finish_deletes();
    }

    /// Once the last deletion is answered: one toast for the lot, and the pages it changed go stale.
    fn finish_deletes(&mut self) {
        let Some(ad) = self.ui.admin.as_mut() else {
            return;
        };
        if !ad.queue.is_empty() || ad.in_flight.is_some() || ad.total == 0 {
            return;
        }
        let (done, failed, total) = (ad.done, std::mem::take(&mut ad.failed), ad.total);
        ad.total = 0;
        ad.done = 0;
        if done > 0 {
            ad.stale.extend([
                AdminPage::Overview,
                AdminPage::Servers,
                AdminPage::Users,
                AdminPage::Audit,
            ]);
        }
        let success = match (done, total) {
            (0, _) => None,
            (1, 1) => Some("File deleted".to_owned()),
            (n, _) => Some(format!("Deleted {n} files")),
        };
        let failure = match failed.as_slice() {
            [] => None,
            [(name, why)] => Some(format!("Couldn't delete {name}. {why}")),
            many => Some(format!(
                "{} files couldn't be deleted. {}",
                many.len(),
                many[0].1
            )),
        };
        if let Some(m) = success {
            self.toast(ToastKind::Success, m);
        }
        if let Some(m) = failure {
            self.toast(ToastKind::Danger, m);
        }
    }

    // ---- admin rights ------------------------------------------------------------------------

    /// The switch on a user row: opens the confirmation (nothing is sent yet).
    pub fn ask_set_admin(&mut self, user: u64, make: bool) {
        let Some(ad) = self.ui.admin.as_mut() else {
            return;
        };
        if ad.busy_users.contains(&user) || !ad.users.items.iter().any(|r| r.user.id == user) {
            return;
        }
        ad.user_errors.remove(&user);
        ad.confirm = Some(Confirm::SetAdmin { user, make });
    }

    pub fn confirm_set_admin(&mut self) {
        let Some(ad) = self.ui.admin.as_mut() else {
            return;
        };
        let Some(Confirm::SetAdmin { user, make }) = ad.confirm.take() else {
            return;
        };
        ad.busy_users.insert(user);
        self.outbox.push(Command::AdminSetAdmin {
            user,
            is_admin: make,
        });
    }

    pub fn admin_set_replied(&mut self, user: u64, is_admin: bool, result: Result<User, ApiError>) {
        let me = self.my_id();
        let Some(ad) = self.ui.admin.as_mut() else {
            return;
        };
        ad.busy_users.remove(&user);
        match result {
            Ok(u) => {
                let name = u.username.clone();
                if let Some(row) = ad.users.items.iter_mut().find(|r| r.user.id == u.id) {
                    row.user = u;
                }
                ad.user_errors.remove(&user);
                ad.stale.insert(AdminPage::Audit);
                self.toast(
                    ToastKind::Success,
                    if is_admin {
                        format!("{name} is now an admin")
                    } else {
                        format!("{name} is no longer an admin")
                    },
                );
            }
            Err(e) => {
                let message = match e {
                    // The server doesn't say which rule it applied: demoting yourself is the first.
                    ApiError::Precondition(_) if user == me => SELF_DEMOTE.to_owned(),
                    ApiError::Precondition(_) => LAST_ADMIN.to_owned(),
                    ApiError::Unimplemented => {
                        ad.unavailable = true;
                        return;
                    }
                    other => error_text(&other),
                };
                ad.user_errors.insert(user, message);
            }
        }
    }

    // ---- replies -----------------------------------------------------------------------------

    pub fn admin_overview_replied(&mut self, result: Result<Overview, ApiError>) {
        let Some(ad) = self.ui.admin.as_mut() else {
            return;
        };
        ad.overview_pending = false;
        match result {
            Ok(o) => {
                ad.overview = Some(o);
                ad.overview_error = None;
            }
            Err(ApiError::Unimplemented) => ad.unavailable = true,
            Err(e) => ad.overview_error = Some(error_text(&e)),
        }
    }

    pub fn admin_activity_replied(
        &mut self,
        days: u32,
        result: Result<Vec<ActivityPoint>, ApiError>,
    ) {
        let Some(ad) = self.ui.admin.as_mut() else {
            return;
        };
        if ad.activity_pending == Some(days) {
            ad.activity_pending = None;
        }
        match result {
            Ok(points) => {
                // Only the range asked for last is shown: a slow answer to an older pick is kept out.
                if days == ad.range {
                    ad.activity = Some((days, points));
                }
            }
            Err(ApiError::Unimplemented) => ad.unavailable = true,
            Err(_) => {}
        }
    }

    pub fn admin_guilds_replied(
        &mut self,
        epoch: u64,
        result: Result<Paged<AdminGuildRow>, ApiError>,
    ) {
        let Some(ad) = self.ui.admin.as_mut() else {
            return;
        };
        let before = ad.servers.items.len();
        match ad.servers.accept(epoch, result) {
            // An answer to an older list.
            None => return,
            Some(Some(err)) => {
                if matches!(err, ApiError::Unimplemented) {
                    ad.unavailable = true;
                }
                return;
            }
            Some(None) => {}
        }
        let fresh: Vec<(u64, String)> = ad.servers.items[before.min(ad.servers.items.len())..]
            .iter()
            .map(|r| (r.guild.id, r.guild.name.clone()))
            .collect();
        for (id, name) in fresh {
            ad.remember_guild(id, &name);
        }
        // On the Files and Invites pages the whole server list is fetched (filter, choices).
        if matches!(ad.page, AdminPage::Files | AdminPage::Invites)
            && let Some((epoch, page_token)) = ad.servers.due()
        {
            let (limit, query) = (ad.page_size, ad.server_search.text.trim().to_owned());
            self.outbox.push(Command::AdminGuilds { epoch, query, page_token, limit });
        }
    }

    pub fn admin_users_replied(
        &mut self,
        epoch: u64,
        result: Result<Paged<AdminUserRow>, ApiError>,
    ) {
        let Some(ad) = self.ui.admin.as_mut() else {
            return;
        };
        if let Some(Some(ApiError::Unimplemented)) = ad.users.accept(epoch, result) {
            ad.unavailable = true;
        }
    }

    pub fn admin_files_replied(
        &mut self,
        epoch: u64,
        result: Result<Paged<AdminFileRow>, ApiError>,
    ) {
        let Some(ad) = self.ui.admin.as_mut() else {
            return;
        };
        let before = ad.files.items.len();
        if let Some(err) = ad.files.accept(epoch, result) {
            if matches!(err, Some(ApiError::Unimplemented)) {
                ad.unavailable = true;
            }
            return;
        }
        // A row whose deletion is out must not come back from a page fetched meanwhile.
        if !ad.removed.is_empty() {
            let tail = ad.files.items.split_off(before.min(ad.files.items.len()));
            let removed = &ad.removed;
            ad.files
                .items
                .extend(tail.into_iter().filter(|r| !removed.contains_key(&r.key)));
        }
        let names: Vec<(u64, String)> = ad.files.items[before.min(ad.files.items.len())..]
            .iter()
            .filter_map(|r| r.guild.as_ref())
            .map(|g| (g.id, g.name.clone()))
            .collect();
        for (id, name) in names {
            if ad.guild_name(id).is_none() {
                ad.remember_guild(id, &name);
            }
        }
    }

    pub fn admin_audit_replied(&mut self, epoch: u64, result: Result<Paged<AuditEntry>, ApiError>) {
        let Some(ad) = self.ui.admin.as_mut() else {
            return;
        };
        if let Some(Some(ApiError::Unimplemented)) = ad.audit.accept(epoch, result) {
            ad.unavailable = true;
        }
    }

    // ---- invites -----------------------------------------------------------------------------

    pub fn admin_invites_replied(&mut self, epoch: u64, result: Result<Paged<Invite>, ApiError>) {
        let Some(ad) = self.ui.admin.as_mut() else {
            return;
        };
        if let Some(Some(ApiError::Unimplemented)) = ad.invites.accept(epoch, result) {
            ad.unavailable = true;
        }
    }

    /// The invites the page shows: the loaded ones that pass the status filter.
    pub fn admin_invite_rows(&self) -> Vec<&Invite> {
        self.admin().map_or_else(Vec::new, |ad| {
            ad.invites.items.iter().filter(|i| ad.invite_filter.is_none_or(|f| i.status == f)).collect()
        })
    }

    pub fn filter_invites(&mut self, status: Option<InviteStatus>) {
        if let Some(ad) = self.ui.admin.as_mut() {
            ad.invite_filter = status;
            ad.invite_filter_menu = false;
        }
    }

    pub fn open_invite_form(&mut self) {
        if let Some(ad) = self.ui.admin.as_mut() {
            let mut email = TextEdit::new("");
            email.focused = true;
            ad.invite_form = Some(InviteForm { email, guilds: Vec::new(), make_admin: false, error: None, busy: false });
        }
    }

    pub fn close_invite_form(&mut self) {
        if let Some(ad) = self.ui.admin.as_mut() {
            ad.invite_form = None;
        }
    }

    pub fn toggle_invite_guild(&mut self, guild: u64) {
        if let Some(f) = self.ui.admin.as_mut().and_then(|ad| ad.invite_form.as_mut()) {
            match f.guilds.iter().position(|g| *g == guild) {
                Some(i) => {
                    f.guilds.remove(i);
                }
                None => f.guilds.push(guild),
            }
        }
    }

    pub fn toggle_invite_admin(&mut self) {
        if let Some(f) = self.ui.admin.as_mut().and_then(|ad| ad.invite_form.as_mut()) {
            f.make_admin = !f.make_admin;
        }
    }

    pub fn submit_invite_form(&mut self) {
        let Some(f) = self.ui.admin.as_mut().and_then(|ad| ad.invite_form.as_mut()) else {
            return;
        };
        if f.busy {
            return;
        }
        let email = f.email.text.trim().to_owned();
        if !crate::account::looks_like_email(&email) {
            f.error = Some("Enter their email address".into());
            return;
        }
        f.error = None;
        f.busy = true;
        let invite = NewInvite { email, guild_id: None, guild_ids: f.guilds.clone(), make_admin: f.make_admin };
        self.outbox.push(Command::CreateInvite { invite });
    }

    /// Whether the admin's create dialog is waiting for `CreateInvite`.
    pub fn admin_invite_busy(&self) -> bool {
        self.admin().and_then(|ad| ad.invite_form.as_ref()).is_some_and(|f| f.busy)
    }

    pub fn admin_invite_created(&mut self, result: Result<Invite, ApiError>) {
        let Some(ad) = self.ui.admin.as_mut() else {
            return;
        };
        match result {
            Ok(invite) => {
                ad.invite_form = None;
                ad.invites.items.retain(|i| i.id != invite.id);
                ad.invites.items.insert(0, Invite { code: None, link: None, ..invite.clone() });
                ad.stale.insert(AdminPage::Audit);
                let email = invite.email.clone();
                ad.invite_created = Some(invite);
                self.toast(ToastKind::Success, format!("Invited {email}"));
            }
            Err(e) => {
                if let Some(f) = ad.invite_form.as_mut() {
                    f.busy = false;
                    f.error = Some(match e {
                        ApiError::AlreadyExists => "Someone with that email already has an account".into(),
                        other => error_text(&other),
                    });
                }
            }
        }
    }

    pub fn admin_revoke_invite(&mut self, id: u64) {
        if let Some(ad) = self.ui.admin.as_mut()
            && ad.invite_busy.insert(id)
        {
            self.outbox.push(Command::RevokeInvite { id });
        }
    }

    pub fn admin_resend_invite(&mut self, id: u64) {
        if let Some(ad) = self.ui.admin.as_mut()
            && ad.invite_busy.insert(id)
        {
            self.outbox.push(Command::ResendInvite { id });
        }
    }

    pub fn dismiss_invite_created(&mut self) {
        if let Some(ad) = self.ui.admin.as_mut() {
            ad.invite_created = None;
        }
    }

    // ---- derived -----------------------------------------------------------------------------

    /// The activity points as chart days: short date on the axis, the date as the tooltip title.
    pub fn chart_days(&self) -> Vec<ChartDay> {
        let Some((_, points)) = self.admin().and_then(|a| a.activity.as_ref()) else {
            return Vec::new();
        };
        points
            .iter()
            .map(|p| {
                let day = parse_date(&p.date);
                ChartDay {
                    label: day.map_or_else(|| p.date.clone(), short_date),
                    title: day.map_or_else(|| p.date.clone(), |d| day_label(d, self.today)),
                    messages: p.messages,
                    active_users: p.active_users,
                    signups: p.signups,
                }
            })
            .collect()
    }

    /// Media keys the open admin list shows as pictures: avatars, server icons and small image
    /// files of the first rows.
    pub fn admin_media_keys(&self) -> Vec<String> {
        let Some(ad) = self.admin() else {
            return Vec::new();
        };
        let mut keys = Vec::new();
        match ad.page {
            AdminPage::Servers => keys.extend(
                ad.servers
                    .items
                    .iter()
                    .take(THUMB_ROWS)
                    .filter_map(|r| r.guild.icon.clone()),
            ),
            AdminPage::Users => keys.extend(
                ad.users
                    .items
                    .iter()
                    .take(THUMB_ROWS)
                    .filter_map(|r| r.user.avatar.clone()),
            ),
            AdminPage::Files => keys.extend(
                ad.files
                    .items
                    .iter()
                    .take(THUMB_ROWS)
                    .filter(|r| has_thumbnail(r))
                    .map(|r| r.key.clone()),
            ),
            AdminPage::Audit => keys.extend(
                ad.audit
                    .items
                    .iter()
                    .take(THUMB_ROWS)
                    .filter_map(|e| e.actor.as_ref()?.avatar.clone()),
            ),
            AdminPage::Overview | AdminPage::Invites => {}
        }
        keys
    }
}
