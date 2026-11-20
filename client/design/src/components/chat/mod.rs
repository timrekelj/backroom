//! Chat building blocks: data in, callbacks out, no app state types. Each takes the `Kit`
//! like the basic controls do, and every motion comes from `tokens::motion`. See the README's
//! "Chat components" for the motion table and what never animates.
//!
//! | Component | Source |
//! |---|---|
//! | [`GuildBadge`], [`RailTile`] | `guild_badge` |
//! | [`Segmented`] | `segmented` |
//! | [`NavRow`], [`ChannelRow`], [`SidebarSection`] | `nav_row`, `section` |
//! | [`MessageGroup`], [`ChatMessage`] | `message`, `body`, `markdown` |
//! | [`DateDivider`], [`NewMessagesDivider`] | `dividers` |
//! | [`Composer`] | `composer` |
//! | [`EmojiPicker`], [`EmojiSuggest`], [`EmojiSet`] | `emoji_picker`, `emoji` |
//! | [`ChannelHeader`], [`MemberRow`], [`EmptyState`] | `header`, `member_row`, `empty_state` |
//! | [`Dialog`], [`Toast`], [`ConnectionStatus`] | `dialog`, `toast`, `connection` |

/// Height of every panel header (sidebar title, channel header, member panel title). One value, so
/// the hairlines under them sit on one y across the window.
pub const HEADER_H: f32 = 48.0;
/// Height of the zone under the transcript and of the sidebar's user strip: the hairlines above
/// them line up for the same reason.
pub const FOOTER_H: f32 = 64.0;
/// Height of a sidebar row (nav, channel, DM) and of a member row.
pub const ROW_H: f32 = 32.0;

pub mod body;
pub mod composer;
pub mod connection;
pub mod dialog;
pub mod dividers;
pub mod emoji;
pub mod emoji_picker;
pub mod empty_state;
pub mod guild_badge;
pub mod header;
pub mod markdown;
pub mod member_row;
pub mod message;
pub mod segmented;
pub mod nav_row;
pub mod pickers;
pub mod section;
pub mod toast;

pub use composer::Composer;
pub use connection::{ConnectionPill, ConnectionStatus};
pub use dialog::{Dialog, dialog_field};
pub use dividers::{DateDivider, NewMessagesDivider};
pub use emoji::{EmojiImage, EmojiSet, Pick, ServerEmoji, Suggestion};
pub use emoji_picker::{EmojiPicker, EmojiPickerState, EmojiSuggest, Scope as EmojiScope};
pub use empty_state::EmptyState;
pub use guild_badge::{GuildBadge, RailTile};
pub use header::ChannelHeader;
pub use member_row::MemberRow;
pub use message::{ChatMessage, DeliveryState, GROUP_WINDOW_SECS, MAX_REACTIONS, MessageGroup, ReactionChip, ReactionGlyph, collapses_into};
pub use nav_row::{ChannelRow, NavRow};
pub use pickers::{ColorSwatches, IconGrid, PickerTabs};
pub use segmented::Segmented;
pub use section::SidebarSection;
pub use toast::{Toast, ToastKind};
