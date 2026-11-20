//! Emojis on `AppState`: the open guild's custom emojis and their pictures, the picker (composer,
//! reactions) and the `:sm` autocomplete, recent emojis, reactions on messages (optimistic, with
//! rollback and live events), the "Emojis" settings page (upload, rename, delete) and the emoji
//! form of a channel's icon. Pure state and commands, like the rest of the model.

use std::cell::Cell;
use std::collections::HashMap;
use std::ops::Range;
use std::path::Path;
use std::rc::Rc;

use rune::TextEdit;
use thehideout_api::{ApiError, Channel, ChannelIconRef, Emoji, MAX_EMOJI_BYTES, Message, Permissions, Reaction, ReactionEmoji};
use thehideout_design::components::chat::emoji::{self, EmojiImage, EmojiSet, Pick, RECENT_MAX, ServerEmoji, Suggestion};
use thehideout_design::components::{EmojiPickerState, MAX_REACTIONS, ToastKind};

use crate::backend::{Command, ImagePurpose};
use crate::forms::{DialogKind, Phase, sentence};
use crate::media;
use crate::server_settings::{ImageDraft, error_text};
use crate::state::{AppState, Place};

/// Custom emojis one server may have.
pub const MAX_EMOJIS: usize = 200;
/// The prefs key of the recent emojis: a JSON array of reaction keys, newest first.
pub const RECENTS_KEY: &str = "recent_emojis";

/// Whether `name` is a valid custom emoji name: `^[a-z0-9_]{2,32}$`.
pub fn valid_name(name: &str) -> bool {
    (2..=32).contains(&name.len()) && name.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
}

/// A name for a picked file: its stem, lowercased, anything outside `[a-z0-9_]` turned into `_`,
/// trimmed and cut to 32 characters (`emoji` when nothing usable is left).
pub fn suggest_name(path: &Path) -> String {
    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
    let mut name = String::new();
    for c in stem.chars().flat_map(char::to_lowercase) {
        if c.is_ascii_lowercase() || c.is_ascii_digit() {
            name.push(c);
        } else if !name.ends_with('_') {
            name.push('_');
        }
    }
    let mut name: String = name.trim_matches('_').chars().take(32).collect();
    if name.len() < 2 {
        name = if name.is_empty() { "emoji".into() } else { format!("{name}_") };
    }
    name
}

/// What the picker will do with the emoji chosen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PickerTarget {
    /// Insert at the composer's caret.
    Composer,
    /// React to a message.
    Reaction { channel: u64, message: u64 },
}

/// The open emoji picker.
pub struct PickerUi {
    pub target: PickerTarget,
    /// The key of the element it hangs from.
    pub anchor: String,
    pub state: EmojiPickerState,
}

/// What the `:sm` autocomplete remembers between frames.
#[derive(Default)]
pub struct SuggestUi {
    pub highlight: usize,
    /// The word the user dismissed with Escape: it stays hidden until the word changes.
    pub dismissed: Option<String>,
}

/// A channel icon that is an emoji.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum IconEmoji {
    Unicode(String),
    Custom(u64),
}

impl IconEmoji {
    pub fn to_ref(&self) -> ChannelIconRef {
        match self {
            IconEmoji::Unicode(s) => ChannelIconRef::Unicode(s.clone()),
            IconEmoji::Custom(id) => ChannelIconRef::Custom(*id),
        }
    }
}

/// A new emoji being named before it is added.
pub struct EmojiDraft {
    pub name: TextEdit,
    pub image: ImageDraft,
    pub error: Option<String>,
}

/// An emoji being renamed in place.
pub struct Rename {
    pub emoji: u64,
    pub name: TextEdit,
    pub error: Option<String>,
    pub busy: bool,
}

/// The "Emojis" settings page.
#[derive(Default)]
pub struct EmojiPage {
    pub draft: Option<EmojiDraft>,
    /// A file is being read off-thread.
    pub reading: bool,
    /// The name the file picked last suggests.
    pub hint: Option<String>,
    pub drop_over: bool,
    pub busy: bool,
    pub error: Option<String>,
    pub rename: Option<Rename>,
}

/// Whether the window draws with reduced motion; the view sets it each frame, so builders deep in
/// the tree can read it without threading a flag.
#[derive(Default)]
pub struct Reduced(pub Cell<bool>);

impl Reduced {
    pub fn get(&self) -> bool {
        self.0.get()
    }

    pub fn set(&self, on: bool) {
        self.0.set(on);
    }
}

fn server_list(list: &[Emoji]) -> Vec<ServerEmoji> {
    list.iter().map(|e| ServerEmoji { id: e.id, name: e.name.clone(), animated: e.animated }).collect()
}

/// Your part of the reaction on `emoji` set or taken back; false when it already was so.
fn set_mine(m: &mut Message, emoji: &ReactionEmoji, mine: bool) -> bool {
    let at = m.reactions.iter().position(|r| &r.emoji == emoji);
    match (at, mine) {
        (Some(i), true) => {
            if m.reactions[i].me {
                return false;
            }
            m.reactions[i].me = true;
            m.reactions[i].count += 1;
            true
        }
        (None, true) => {
            m.reactions.push(Reaction { emoji: emoji.clone(), count: 1, me: true });
            true
        }
        (Some(i), false) => {
            if !m.reactions[i].me {
                return false;
            }
            m.reactions[i].me = false;
            m.reactions[i].count = m.reactions[i].count.saturating_sub(1);
            if m.reactions[i].count == 0 {
                m.reactions.remove(i);
            }
            true
        }
        (None, false) => false,
    }
}

/// Someone else reacted or took theirs back.
fn set_other(m: &mut Message, emoji: &ReactionEmoji, add: bool) {
    let at = m.reactions.iter().position(|r| &r.emoji == emoji);
    match (at, add) {
        (Some(i), true) => m.reactions[i].count += 1,
        (None, true) => m.reactions.push(Reaction { emoji: emoji.clone(), count: 1, me: false }),
        (Some(i), false) => {
            m.reactions[i].count = m.reactions[i].count.saturating_sub(1);
            if m.reactions[i].count == 0 {
                m.reactions.remove(i);
            }
        }
        (None, false) => {}
    }
}

impl AppState {
    /// The open channel and server id of the message a transcript row (`chat_id`) shows; none for a
    /// message the server hasn't confirmed.
    pub fn message_ref(&self, chat_id: &str) -> Option<(u64, u64)> {
        let channel = self.current_channel()?;
        let me = self.my_id();
        let m = self.log(channel)?.messages.iter().find(|m| crate::state::chat_id_of(m, me) == chat_id)?;
        Some((channel, m.id))
    }

    // ---- the guild's emojis ------------------------------------------------------------------

    /// The guild whose custom emojis a message can use here: the open server, none at Home.
    pub fn emoji_guild(&self) -> Option<u64> {
        match self.ui.place {
            Place::Guild(g) => Some(g),
            Place::Home => None,
        }
    }

    pub fn emojis_of(&self, guild: u64) -> &[Emoji] {
        self.data.emojis.get(&guild).map_or(&[], Vec::as_slice)
    }

    pub fn emoji(&self, guild: u64, id: u64) -> Option<&Emoji> {
        self.emojis_of(guild).iter().find(|e| e.id == id)
    }

    /// The open server's emojis as the picker and the autocomplete want them (none in a DM).
    pub fn server_emojis(&self) -> Vec<ServerEmoji> {
        self.emoji_guild().map(|g| server_list(self.emojis_of(g))).unwrap_or_default()
    }

    /// Asks for a guild's emojis unless they are loaded or already asked for.
    pub fn want_emojis(&mut self, guild: u64) {
        if !self.data.emojis.contains_key(&guild) && self.data.emojis_asked.insert(guild) {
            self.outbox.push(Command::LoadEmojis { guild });
        }
    }

    /// The server's answer, or its `emojis_updated` event: the whole list replaces the old one.
    pub fn set_emojis(&mut self, guild: u64, mut list: Vec<Emoji>) {
        list.sort_by(|a, b| a.name.cmp(&b.name).then(a.id.cmp(&b.id)));
        let live: Vec<u64> = list.iter().map(|e| e.id).collect();
        self.data.emojis.insert(guild, list);
        // A rename of an emoji that is gone is dropped, and a channel form's custom icon that
        // is gone falls back to the default.
        if let Some(ss) = self.ui.server_settings.as_mut().filter(|s| s.guild == guild)
            && ss.emojis.rename.as_ref().is_some_and(|r| !live.contains(&r.emoji))
        {
            ss.emojis.rename = None;
        }
        if let Some(f) = self.ui.dialog.as_mut().and_then(|d| d.channel.as_mut())
            && matches!(f.emoji, Some(IconEmoji::Custom(id)) if !live.contains(&id))
        {
            f.emoji = None;
        }
        self.want_media();
    }

    /// Whether media key `key` is the picture of a custom emoji of some loaded guild.
    pub fn is_emoji_image(&self, key: &str) -> bool {
        self.data.emojis.values().any(|l| l.iter().any(|e| e.image == key))
    }

    /// The picture of custom emoji `id` of `guild`, as message text and reactions draw it.
    pub fn emoji_image(&self, guild: u64, id: u64) -> Option<EmojiImage> {
        let e = self.emoji(guild, id)?;
        Some(EmojiImage { name: e.name.clone(), animated: e.animated, live: self.data.media.pixels(&e.image), still: self.data.media.still(&e.image) })
    }

    /// How the open channel's messages find custom emojis: the open server's, or none in a DM.
    pub fn emoji_set(&self) -> EmojiSet {
        let reduced = self.ui.reduced.get();
        let Some(guild) = self.emoji_guild() else { return EmojiSet::none().reduced_motion(reduced) };
        let map: HashMap<String, EmojiImage> = self.emojis_of(guild).iter().filter_map(|e| self.emoji_image(guild, e.id).map(|i| (e.id.to_string(), i))).collect();
        let map = Rc::new(map);
        EmojiSet::new(move |id| map.get(id).cloned()).reduced_motion(reduced)
    }

    // ---- recents -----------------------------------------------------------------------------

    /// `pick` was just used: first in the recents (the last [`RECENT_MAX`] are kept), saved by the next flush.
    pub fn record_recent(&mut self, pick: &Pick) {
        let key = pick.key();
        self.ui.recents.retain(|k| *k != key);
        self.ui.recents.insert(0, key);
        self.ui.recents.truncate(RECENT_MAX);
        self.ui.recents_dirty = true;
    }

    // ---- the picker --------------------------------------------------------------------------

    /// Opens the picker for `target`, hung from the element keyed `anchor`. Pressing the button
    /// that opened it closes it again.
    pub fn open_picker(&mut self, target: PickerTarget, anchor: impl Into<String>) {
        let anchor = anchor.into();
        if self.ui.picker.as_ref().is_some_and(|p| p.target == target && p.anchor == anchor) {
            self.close_picker();
            return;
        }
        self.ui.menu = crate::state::Menu::None;
        if let Some(guild) = self.emoji_guild() {
            self.want_emojis(guild);
        }
        self.ui.picker = Some(PickerUi { target, anchor, state: EmojiPickerState::new() });
    }

    pub fn close_picker(&mut self) {
        if let Some(p) = self.ui.picker.take()
            && p.target == PickerTarget::Composer
        {
            self.composer_mut().focused = true;
        }
    }

    /// The emoji chosen in the open picker: inserted, or reacted with.
    pub fn picker_pick(&mut self, pick: Pick) {
        let Some(target) = self.ui.picker.as_ref().map(|p| p.target) else { return };
        self.record_recent(&pick);
        self.ui.picker = None;
        match target {
            PickerTarget::Composer => self.insert_at_caret(&pick.text()),
            PickerTarget::Reaction { channel, message } => self.react(channel, message, reaction_of(&pick), false),
        }
    }

    /// Puts `text` into the composer at the caret (replacing a selection) and focuses it.
    pub fn insert_at_caret(&mut self, text: &str) {
        let edit = self.composer_mut();
        edit.insert(text);
        edit.focused = true;
    }

    // ---- autocomplete ------------------------------------------------------------------------

    /// The `:partial` before the caret and its best matches, while the composer is focused and
    /// the user hasn't dismissed this word.
    pub fn suggestions(&self) -> Option<(Range<usize>, Vec<Suggestion>)> {
        if self.ui.picker.is_some() || self.composer_block().is_some() {
            return None;
        }
        let edit = self.composer();
        if !edit.focused || edit.anchor != edit.focus {
            return None;
        }
        let (range, word) = emoji::trigger_at(&edit.text, edit.focus)?;
        if self.ui.suggest.dismissed.as_deref() == Some(word) {
            return None;
        }
        let list = emoji::suggest(word, &self.server_emojis());
        (!list.is_empty()).then_some((range, list))
    }

    /// Up and Down in the list.
    pub fn suggest_step(&mut self, delta: i32) {
        let Some((_, list)) = self.suggestions() else { return };
        let n = list.len() as i32;
        let at = self.ui.suggest.highlight as i32;
        self.ui.suggest.highlight = (at + delta).rem_euclid(n) as usize;
    }

    /// Tab or Enter (or a press): the emoji at `index` replaces the `:partial`.
    pub fn suggest_accept(&mut self, index: usize) -> bool {
        let Some((range, list)) = self.suggestions() else { return false };
        let Some(s) = list.get(index) else { return false };
        let (text, pick) = (s.pick.text(), s.pick.clone());
        self.record_recent(&pick);
        let edit = self.composer_mut();
        edit.set_selection(range.start, range.end);
        edit.insert(&text);
        edit.focused = true;
        self.ui.suggest = SuggestUi::default();
        true
    }

    /// Escape: hides the list for this word.
    pub fn suggest_dismiss(&mut self) {
        let word = self.suggestions().and_then(|(r, _)| self.composer().text.get(r).map(|w| w.trim_start_matches(':').to_owned()));
        if let Some(word) = word {
            self.ui.suggest.dismissed = Some(word);
        }
    }

    // ---- reactions ---------------------------------------------------------------------------

    fn message_mut(&mut self, channel: u64, id: u64) -> Option<&mut Message> {
        self.data.logs.get_mut(&channel)?.messages.iter_mut().find(|m| m.id == id)
    }

    /// A chip was pressed: your reaction on `emoji` flips.
    pub fn toggle_reaction(&mut self, channel: u64, message: u64, emoji: ReactionEmoji) {
        self.react(channel, message, emoji, true);
    }

    /// Adds (or with `toggle`, flips) your reaction at once and tells the server; a refusal rolls
    /// it back ([`reaction_saved`](Self::reaction_saved)). A message already holding
    /// [`MAX_REACTIONS`] different emojis takes no new one.
    pub fn react(&mut self, channel: u64, message: u64, emoji: ReactionEmoji, toggle: bool) {
        let Some(m) = self.message_mut(channel, message) else { return };
        let existing = m.reactions.iter().find(|r| r.emoji == emoji);
        let mine = existing.is_some_and(|r| r.me);
        let add = !(toggle && mine);
        if add && mine {
            return;
        }
        if add && existing.is_none() && m.reactions.len() >= MAX_REACTIONS {
            self.toast(ToastKind::Danger, format!("A message can have up to {MAX_REACTIONS} different reactions"));
            return;
        }
        set_mine(m, &emoji, add);
        self.outbox.push(if add { Command::AddReaction { channel, message, emoji } } else { Command::RemoveReaction { channel, message, emoji } });
    }

    /// The server's answer to a reaction call: a refusal undoes the optimistic change.
    pub fn reaction_saved(&mut self, channel: u64, message: u64, emoji: &ReactionEmoji, add: bool, result: Result<(), ApiError>) {
        let Err(e) = result else { return };
        if let Some(m) = self.message_mut(channel, message) {
            set_mine(m, emoji, !add);
        }
        let what = if add { "add the reaction" } else { "remove the reaction" };
        self.toast(ToastKind::Danger, format!("Couldn't {what}. {}", sentence(&error_text(&e))));
    }

    /// A `reaction_added` / `reaction_removed` event. Your own come back too (the optimistic
    /// change already made them, so they change nothing); everyone else's move the count.
    pub fn reaction_event(&mut self, channel: u64, message: u64, emoji: &ReactionEmoji, user: u64, add: bool) {
        let me = self.my_id();
        let Some(m) = self.message_mut(channel, message) else { return };
        if user == me {
            set_mine(m, emoji, add);
        } else {
            set_other(m, emoji, add);
        }
    }

    // ---- the emojis page ---------------------------------------------------------------------

    pub fn can_manage_emojis(&self, guild: u64) -> bool {
        self.perms(guild).contains(Permissions::MANAGE_EMOJIS)
    }

    fn emoji_page(&mut self) -> Option<&mut EmojiPage> {
        self.ui.server_settings.as_mut().map(|s| &mut s.emojis)
    }

    /// The file chooser's (or a drop's) answer: checks the type and has the backend read it. Also
    /// remembers the file's name to suggest one.
    pub fn pick_emoji_file(&mut self, path: &Path) {
        let Some(guild) = self.ui.server_settings.as_ref().map(|s| s.guild) else { return };
        if !self.can_manage_emojis(guild) {
            return;
        }
        let full = self.emojis_of(guild).len() >= MAX_EMOJIS;
        let Some(page) = self.emoji_page() else { return };
        if full {
            page.error = Some(format!("This server has {MAX_EMOJIS} emojis, the most it can have."));
            return;
        }
        if page.reading || page.busy {
            return;
        }
        if media::image_type(path).is_none() {
            page.error = Some("Choose a PNG, JPEG, WebP or GIF image.".into());
            return;
        }
        page.reading = true;
        page.error = None;
        page.hint = Some(suggest_name(path));
        self.outbox.push(Command::ReadImage { path: path.to_path_buf(), purpose: ImagePurpose::Emoji { guild } });
    }

    /// The backend read the file: a draft with the suggested name, or why it can't be used.
    pub fn emoji_file_read(&mut self, guild: u64, result: Result<(Vec<u8>, String), ApiError>) {
        let outcome = match result {
            Ok((bytes, _)) if bytes.len() > MAX_EMOJI_BYTES => Err("That image is over 2 MiB.".to_owned()),
            Ok((bytes, content_type)) => Ok((self.data.media.preview(bytes.clone()), bytes, content_type)),
            Err(e) => Err(error_text(&e)),
        };
        let Some(page) = self.ui.server_settings.as_mut().filter(|s| s.guild == guild).map(|s| &mut s.emojis) else { return };
        page.reading = false;
        match outcome {
            Ok((preview, bytes, content_type)) => {
                let mut name = TextEdit::new(page.hint.as_deref().unwrap_or("emoji"));
                name.focused = true;
                page.draft = Some(EmojiDraft { name, image: ImageDraft { bytes, content_type, preview }, error: None });
                page.error = None;
            }
            Err(e) => page.error = Some(e),
        }
    }

    pub fn discard_emoji_draft(&mut self) {
        if let Some(p) = self.emoji_page().filter(|p| !p.busy) {
            p.draft = None;
        }
    }

    /// "Add": validates the name against the rules the server applies, then uploads.
    pub fn create_emoji(&mut self) {
        let Some(guild) = self.ui.server_settings.as_ref().map(|s| s.guild) else { return };
        let Some(name) = self.ui.server_settings.as_ref().and_then(|s| s.emojis.draft.as_ref()).map(|d| d.name.text.trim().to_owned()) else { return };
        let allowed = self.can_manage_emojis(guild);
        let count = self.emojis_of(guild).len();
        let taken = self.emojis_of(guild).iter().any(|e| e.name == name);
        let problem = if !allowed {
            Some("You don't have permission to do that.".to_owned())
        } else if count >= MAX_EMOJIS {
            Some(format!("This server has {MAX_EMOJIS} emojis, the most it can have."))
        } else if !valid_name(&name) {
            Some("Use 2 to 32 lowercase letters, digits or underscores.".to_owned())
        } else if taken {
            Some("An emoji with that name already exists.".to_owned())
        } else {
            None
        };
        let Some(page) = self.emoji_page() else { return };
        if page.busy {
            return;
        }
        let Some(d) = page.draft.as_mut() else { return };
        if let Some(p) = problem {
            d.error = Some(p);
            return;
        }
        d.error = None;
        let cmd = Command::CreateEmoji { guild, name, bytes: d.image.bytes.clone(), content_type: d.image.content_type.clone() };
        page.busy = true;
        self.outbox.push(cmd);
    }

    pub fn start_rename(&mut self, emoji: u64) {
        let Some(guild) = self.ui.server_settings.as_ref().map(|s| s.guild) else { return };
        let Some(name) = self.emoji(guild, emoji).map(|e| e.name.clone()) else { return };
        if !self.can_manage_emojis(guild) {
            return;
        }
        let mut edit = TextEdit::new(&name);
        edit.focused = true;
        if let Some(p) = self.emoji_page() {
            p.rename = Some(Rename { emoji, name: edit, error: None, busy: false });
        }
    }

    pub fn cancel_rename(&mut self) {
        if let Some(p) = self.emoji_page() {
            p.rename = None;
        }
    }

    /// Enter in the rename field: validates, then renames (nothing sent when unchanged).
    pub fn commit_rename(&mut self) {
        let Some(guild) = self.ui.server_settings.as_ref().map(|s| s.guild) else { return };
        let Some(r) = self.ui.server_settings.as_ref().and_then(|s| s.emojis.rename.as_ref()) else { return };
        if r.busy {
            return;
        }
        let (emoji, name) = (r.emoji, r.name.text.trim().to_owned());
        let current = self.emoji(guild, emoji).map(|e| e.name.clone());
        let clash = self.emojis_of(guild).iter().any(|e| e.id != emoji && e.name == name);
        let problem = if current.as_deref() == Some(name.as_str()) {
            self.cancel_rename();
            return;
        } else if !valid_name(&name) {
            Some("Use 2 to 32 lowercase letters, digits or underscores.".to_owned())
        } else if clash {
            Some("An emoji with that name already exists.".to_owned())
        } else {
            None
        };
        let Some(r) = self.emoji_page().and_then(|p| p.rename.as_mut()) else { return };
        if let Some(p) = problem {
            r.error = Some(p);
            return;
        }
        r.error = None;
        r.busy = true;
        self.outbox.push(Command::RenameEmoji { guild, emoji, name });
    }

    /// The reply to `CreateEmoji` or `RenameEmoji`.
    pub fn emoji_saved(&mut self, guild: u64, result: Result<Emoji, ApiError>) {
        match result {
            Ok(e) => {
                let mut list = self.emojis_of(guild).to_vec();
                match list.iter_mut().find(|x| x.id == e.id) {
                    Some(slot) => *slot = e.clone(),
                    None => list.push(e.clone()),
                }
                let created = self.ui.server_settings.as_ref().is_some_and(|s| s.guild == guild && s.emojis.draft.is_some() && s.emojis.busy);
                self.set_emojis(guild, list);
                if let Some(p) = self.ui.server_settings.as_mut().filter(|s| s.guild == guild).map(|s| &mut s.emojis) {
                    if created {
                        p.draft = None;
                        p.busy = false;
                    }
                    p.rename = None;
                }
                self.toast(ToastKind::Success, if created { format!("Added :{}:", e.name) } else { format!("Renamed to :{}:", e.name) });
            }
            Err(err) => {
                let text = error_text(&err);
                match self.ui.server_settings.as_mut().filter(|s| s.guild == guild).map(|s| &mut s.emojis) {
                    Some(p) => {
                        if let Some(r) = p.rename.as_mut().filter(|r| r.busy) {
                            r.busy = false;
                            r.error = Some(text);
                        } else if p.busy {
                            p.busy = false;
                            if let Some(d) = p.draft.as_mut() {
                                d.error = Some(text);
                            }
                        }
                    }
                    None => self.toast(ToastKind::Danger, text),
                }
            }
        }
    }

    pub fn emoji_deleted(&mut self, guild: u64, emoji: u64, result: Result<(), ApiError>) {
        match result {
            Ok(()) => {
                let list: Vec<Emoji> = self.emojis_of(guild).iter().filter(|e| e.id != emoji).cloned().collect();
                self.set_emojis(guild, list);
                if matches!(self.ui.dialog.as_ref().map(|d| d.kind), Some(DialogKind::DeleteEmoji { emoji: e, .. }) if e == emoji) {
                    self.close_dialog();
                }
                self.toast(ToastKind::Success, "Emoji deleted");
            }
            Err(e) => match self.ui.dialog.as_mut() {
                Some(d) => {
                    d.phase = Phase::Idle;
                    d.error = Some(sentence(&error_text(&e)));
                }
                None => self.toast(ToastKind::Danger, error_text(&e)),
            },
        }
    }

    // ---- channel icons -----------------------------------------------------------------------

    /// The emoji icon `c` carries, if it has one (Unicode, or a custom emoji still in its guild).
    pub fn channel_emoji(&self, c: &Channel) -> Option<IconEmoji> {
        match &c.icon_ref {
            ChannelIconRef::Unicode(s) => Some(IconEmoji::Unicode(s.clone())),
            ChannelIconRef::Custom(id) => self.emoji(c.guild_id, *id).map(|_| IconEmoji::Custom(*id)),
            _ => None,
        }
    }

    /// An emoji chosen in the channel dialog's picker tabs: it becomes the icon and the picker closes.
    pub fn pick_icon_emoji(&mut self, pick: Pick) {
        let icon = match &pick {
            Pick::Unicode(s) => IconEmoji::Unicode((*s).to_owned()),
            Pick::Custom(e) => IconEmoji::Custom(e.id),
        };
        if let Some(f) = self.channel_form() {
            f.emoji = Some(icon);
            f.icon.clear();
        }
        if self.channel_form().is_some_and(|f| f.picker_open) {
            self.toggle_picker();
        }
    }
}

/// The reaction an emoji pick stands for.
pub fn reaction_of(pick: &Pick) -> ReactionEmoji {
    match pick {
        Pick::Unicode(s) => ReactionEmoji::Unicode((*s).to_owned()),
        Pick::Custom(e) => ReactionEmoji::Custom(e.id),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_follow_the_servers_rule() {
        for ok in ["ab", "party_parrot", "a1_", &"x".repeat(32)] {
            assert!(valid_name(ok), "{ok}");
        }
        for bad in ["a", "", "Party", "has space", "dash-ed", "ünï", &"x".repeat(33)] {
            assert!(!valid_name(bad), "{bad}");
        }
    }

    #[test]
    fn a_files_name_is_made_valid() {
        let p = |s: &str| suggest_name(Path::new(s));
        assert_eq!(p("/x/Party Parrot.GIF"), "party_parrot");
        assert_eq!(p("a.png"), "a_");
        assert_eq!(p("---.png"), "emoji");
        assert_eq!(p("Smile!!(1).png"), "smile_1");
        assert_eq!(p(&format!("{}.png", "n".repeat(50))).len(), 32);
        assert!(valid_name(&p("ÄÖ.png")) && valid_name(&p(".hidden")));
    }
}
