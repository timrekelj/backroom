//! Sidebar structure on `AppState`: folding sections, the optimistic layout edits (steps, moves,
//! drag and drop) and what happens when the server answers them. The pure arithmetic is in
//! `layout.rs`; this is the state around it: permission checks, the undo copy and the commands.

use thehideout_api::{ApiError, GuildLayout};
use thehideout_design::components::ToastKind;

use crate::backend::Command;
use crate::layout::{self, Item};
use crate::forms::ChannelForm;
use crate::state::{AppState, Drag, ROW_PITCH, sentence};

/// A section header's height and the gap under it, and between groups.
const SECTION_HEADER: f32 = 30.0;
const SECTION_GAP: f32 = 8.0;

pub const CONFLICT_TOAST: &str = "Layout changed elsewhere \u{2014} try again";

impl AppState {
    pub fn layout(&self, guild: u64) -> Option<&GuildLayout> {
        self.data.layouts.get(&guild)
    }

    /// Folds or unfolds a section; saved to the prefs by `App::flush`.
    pub fn toggle_section(&mut self, guild: u64, section: u64) {
        if !self.ui.collapsed.remove(&(guild, section)) {
            self.ui.collapsed.insert((guild, section));
        }
        self.ui.collapsed_dirty = true;
    }

    /// The sidebar's rows in order, folded sections as their header alone.
    pub fn sidebar_items(&self, guild: u64) -> Vec<Item> {
        self.layout(guild).map_or_else(Vec::new, |l| layout::items(l, &|s| self.section_open(guild, s)))
    }

    /// Applies `new` at once and sends it. False (nothing changed) when the user may not edit the
    /// structure or an earlier edit is still waiting for its answer.
    pub fn edit_layout(&mut self, guild: u64, new: GuildLayout) -> bool {
        if !self.can_manage_channels(guild) || self.data.layout_undo.contains_key(&guild) {
            return false;
        }
        let Some(old) = self.data.layouts.insert(guild, new.clone()) else { return false };
        self.data.layout_undo.insert(guild, old);
        self.outbox.push(Command::UpdateLayout { guild, layout: new });
        true
    }

    /// Move up (`-1`) or down (`1`) from the channel menu.
    pub fn step_channel_by(&mut self, guild: u64, channel: u64, delta: i32) {
        if let Some(new) = self.layout(guild).and_then(|l| layout::step_channel(l, channel, delta)) {
            self.edit_layout(guild, new);
        }
    }

    /// "Move to section": to the end of `section` (`None`: the top level).
    pub fn move_channel_to_section(&mut self, guild: u64, channel: u64, section: Option<u64>) {
        if let Some(new) = self.layout(guild).and_then(|l| layout::move_channel_to(l, channel, section, usize::MAX)) {
            self.edit_layout(guild, new);
        }
    }

    pub fn step_section_by(&mut self, guild: u64, section: u64, delta: i32) {
        if let Some(new) = self.layout(guild).and_then(|l| layout::step_section(l, section, delta)) {
            self.edit_layout(guild, new);
        }
    }

    // ---- the channel dialogs' pickers --------------------------------------------------------

    pub fn channel_form(&mut self) -> Option<&mut ChannelForm> {
        self.ui.dialog.as_mut().and_then(|d| d.channel.as_mut())
    }

    /// Opens or closes the icon picker; opening puts the cursor in its search field.
    pub fn toggle_picker(&mut self) {
        let Some(d) = self.ui.dialog.as_mut() else { return };
        let Some(f) = d.channel.as_mut() else { return };
        f.picker_open = !f.picker_open;
        f.section_open = false;
        d.input.focused = !f.picker_open;
        f.search.focused = f.picker_open;
        if f.picker_open {
            f.search.replace_text("");
            f.research();
            f.emoji_pick = thehideout_design::components::EmojiPickerState::new();
            f.emoji_pick.search.focused = f.tab != 0;
            f.search.focused = f.tab == 0;
        }
    }

    /// Switches the picker's tab (0 Icons, 1 Emoji, 2 Server emoji) and puts the cursor in its search.
    pub fn set_picker_tab(&mut self, tab: usize) {
        let Some(f) = self.channel_form() else { return };
        f.tab = tab.min(2);
        f.emoji_pick = thehideout_design::components::EmojiPickerState::new();
        f.emoji_pick.search.focused = f.tab != 0;
        f.search.focused = f.tab == 0;
    }

    /// Chooses an icon (`""`: the default `#`) and closes the picker.
    pub fn pick_icon(&mut self, name: &str) {
        if let Some(f) = self.channel_form() {
            f.icon = name.to_owned();
            f.emoji = None;
        }
        if self.channel_form().is_some_and(|f| f.picker_open) {
            self.toggle_picker();
        }
    }

    pub fn pick_color(&mut self, key: &'static str) {
        if let Some(f) = self.channel_form() {
            f.color = key;
        }
    }

    /// Enter in the search field: the highlighted icon.
    pub fn pick_highlighted(&mut self) {
        let name = self.channel_form().and_then(|f| f.highlight.and_then(|h| f.results.get(h)).map(|c| c.icon.name));
        if let Some(name) = name {
            self.pick_icon(name);
        }
    }

    // ---- drag and drop -----------------------------------------------------------------------

    /// A row was pressed and moved: starts (or continues) the drag, `dy` from where it was pressed.
    pub fn drag_item(&mut self, guild: u64, item: Item, dy: f32) {
        if !self.can_manage_channels(guild) || self.data.layout_undo.contains_key(&guild) {
            return;
        }
        self.ui.drag = Some(Drag { guild, item, dy });
    }

    /// The row slot the dragged item would take if dropped now.
    pub fn drop_slot(&self) -> Option<usize> {
        let d = self.ui.drag?;
        let rows = self.sidebar_items(d.guild);
        let from = rows.iter().position(|&i| i == d.item)?;
        Some((from as i32 + (d.dy / ROW_PITCH).round() as i32).clamp(0, rows.len() as i32 - 1) as usize)
    }

    /// While a section is dragged: its place among the sections, where it would land, and how far
    /// its neighbours step aside (its own height and the gap).
    pub fn section_drag(&self) -> Option<(usize, usize, f32)> {
        let d = self.ui.drag?;
        let Item::Section(id) = d.item else { return None };
        let l = self.layout(d.guild)?;
        let heights: Vec<f32> = l
            .sections
            .iter()
            .map(|s| {
                let rows = if self.section_open(d.guild, s.id) { layout::channels_in(l, Some(s.id)).len() as f32 } else { 0.0 };
                SECTION_HEADER + rows * ROW_PITCH + SECTION_GAP
            })
            .collect();
        let from = l.sections.iter().position(|s| s.id == id)?;
        let mut to = from;
        let mut travelled = 0.0;
        if d.dy > 0.0 {
            for (k, h) in heights.iter().enumerate().skip(from + 1) {
                if d.dy > travelled + h / 2.0 {
                    to = k;
                    travelled += h;
                } else {
                    break;
                }
            }
        } else {
            for (k, h) in heights.iter().enumerate().take(from).rev() {
                if -d.dy > travelled + h / 2.0 {
                    to = k;
                    travelled += h;
                } else {
                    break;
                }
            }
        }
        Some((from, to, heights[from]))
    }

    /// The pointer was released (or the drag cancelled): a drop that moves something is an edit.
    pub fn end_drag(&mut self, cancelled: bool) {
        let slot = self.drop_slot();
        let section = self.section_drag();
        let Some(d) = self.ui.drag.take() else { return };
        if cancelled {
            return;
        }
        let moved = match (d.item, section) {
            (Item::Section(id), Some((_, to, _))) => self.layout(d.guild).and_then(|l| layout::move_section_to(l, id, to)),
            (Item::Section(_), None) => None,
            (item, _) => {
                let Some(slot) = slot else { return };
                self.layout(d.guild).and_then(|l| layout::drop_item(l, &|s| self.section_open(d.guild, s), item, slot))
            }
        };
        if let Some(new) = moved {
            self.edit_layout(d.guild, new);
        }
    }

    // ---- the server's answers ----------------------------------------------------------------

    /// Takes the server's layout unless it is older than what is held.
    pub fn set_layout(&mut self, guild: u64, l: GuildLayout) {
        if self.data.layouts.get(&guild).is_none_or(|cur| l.version >= cur.version) {
            self.data.layouts.insert(guild, layout::normalized(&l));
        }
    }

    /// The reply to `UpdateLayout`. Success: the server's layout. Failure: the edit comes back out
    /// (unless a newer layout already arrived on the gateway), and a stale version refetches.
    pub fn layout_replied(&mut self, guild: u64, result: Result<GuildLayout, ApiError>) {
        let undo = self.data.layout_undo.remove(&guild);
        match result {
            Ok(l) => self.set_layout(guild, l),
            Err(e) => {
                if let Some(u) = undo
                    && self.layout(guild).is_some_and(|cur| cur.version == u.version)
                {
                    self.data.layouts.insert(guild, u);
                }
                if e == ApiError::Conflict {
                    self.outbox.push(Command::LoadChannels { guild });
                    self.toast(ToastKind::Info, CONFLICT_TOAST);
                } else {
                    self.toast(ToastKind::Danger, format!("Couldn't change the layout. {}", sentence(&e.to_string())));
                }
            }
        }
    }
}
