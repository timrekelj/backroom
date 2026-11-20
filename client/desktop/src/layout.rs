//! Pure edits of a guild's [`GuildLayout`]: the moves the sidebar offers (up, down, to a section,
//! a drag's drop). Each returns the layout to send to `UpdateLayout` (same `version`, so the
//! server can refuse a stale one) or `None` when the move changes nothing. Nothing here knows
//! about the UI; `AppState` applies the result optimistically and sends it.

use thehideout_api::{GuildLayout, LayoutEntry, Section};

/// One row of the sidebar in order: a section header or a channel. Top-level channels come first.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Item {
    Section(u64),
    Channel(u64),
}

/// The layout with one entry per container (top level, then each section in order), so edits
/// never meet a missing one. The server sends them all; a fake or a partial one is completed.
pub fn normalized(layout: &GuildLayout) -> GuildLayout {
    let mut out = layout.clone();
    let mut entries = Vec::with_capacity(layout.sections.len() + 1);
    for id in std::iter::once(None).chain(layout.sections.iter().map(|s| Some(s.id))) {
        let channel_ids = layout
            .entries
            .iter()
            .find(|e| e.section_id == id)
            .map(|e| e.channel_ids.clone())
            .unwrap_or_default();
        entries.push(LayoutEntry {
            section_id: id,
            channel_ids,
        });
    }
    out.entries = entries;
    out
}

/// The channel ids of a container.
pub fn channels_in(layout: &GuildLayout, section: Option<u64>) -> &[u64] {
    layout
        .entries
        .iter()
        .find(|e| e.section_id == section)
        .map_or(&[], |e| e.channel_ids.as_slice())
}

/// Every channel id in sidebar order.
pub fn channel_order(layout: &GuildLayout) -> Vec<u64> {
    items(layout, &|_| true)
        .into_iter()
        .filter_map(|i| {
            if let Item::Channel(c) = i {
                Some(c)
            } else {
                None
            }
        })
        .collect()
}

/// The sidebar's rows in order. A section whose id `open` rejects (collapsed) shows its header only.
pub fn items(layout: &GuildLayout, open: &dyn Fn(u64) -> bool) -> Vec<Item> {
    let mut v: Vec<Item> = channels_in(layout, None)
        .iter()
        .map(|&c| Item::Channel(c))
        .collect();
    for s in &layout.sections {
        v.push(Item::Section(s.id));
        if open(s.id) {
            v.extend(
                channels_in(layout, Some(s.id))
                    .iter()
                    .map(|&c| Item::Channel(c)),
            );
        }
    }
    v
}

/// The section a channel is in (`Some(None)` is the top level), or `None` if it isn't in the layout.
pub fn section_of(layout: &GuildLayout, channel: u64) -> Option<Option<u64>> {
    layout
        .entries
        .iter()
        .find(|e| e.channel_ids.contains(&channel))
        .map(|e| e.section_id)
}

fn entry_mut(layout: &mut GuildLayout, section: Option<u64>) -> Option<&mut Vec<u64>> {
    layout
        .entries
        .iter_mut()
        .find(|e| e.section_id == section)
        .map(|e| &mut e.channel_ids)
}

fn changed(old: &GuildLayout, new: GuildLayout) -> Option<GuildLayout> {
    (normalized(old) != new).then_some(new)
}

/// Puts `channel` into `section` at `index` (clamped), removing it from where it was.
pub fn move_channel_to(
    layout: &GuildLayout,
    channel: u64,
    section: Option<u64>,
    index: usize,
) -> Option<GuildLayout> {
    let mut l = normalized(layout);
    section_of(&l, channel)?;
    if section.is_some_and(|s| !l.sections.iter().any(|x| x.id == s)) {
        return None;
    }
    for e in &mut l.entries {
        e.channel_ids.retain(|&c| c != channel);
    }
    let list = entry_mut(&mut l, section)?;
    list.insert(index.min(list.len()), channel);
    changed(layout, l)
}

/// One step up (`-1`) or down (`1`) among the channels: within its container, and past the
/// container's edge into the end of the previous one or the start of the next.
pub fn step_channel(layout: &GuildLayout, channel: u64, delta: i32) -> Option<GuildLayout> {
    let l = normalized(layout);
    let here = section_of(&l, channel)?;
    let list = channels_in(&l, here);
    let at = list.iter().position(|&c| c == channel)?;
    let to = at as i32 + delta;
    if (0..list.len() as i32).contains(&to) {
        return move_channel_to(layout, channel, here, to as usize);
    }
    let containers: Vec<Option<u64>> = l.entries.iter().map(|e| e.section_id).collect();
    let i = containers.iter().position(|&c| c == here)? as i32 + delta.signum();
    let next = *containers.get(usize::try_from(i).ok()?)?;
    let index = if delta < 0 {
        channels_in(&l, next).len()
    } else {
        0
    };
    move_channel_to(layout, channel, next, index)
}

/// Whether `step_channel` would move `channel` by `delta`.
pub fn can_step_channel(layout: &GuildLayout, channel: u64, delta: i32) -> bool {
    step_channel(layout, channel, delta).is_some()
}

/// Moves a section one place up or down among the sections.
pub fn step_section(layout: &GuildLayout, section: u64, delta: i32) -> Option<GuildLayout> {
    let at = layout.sections.iter().position(|s| s.id == section)?;
    let to = usize::try_from(at as i32 + delta)
        .ok()
        .filter(|&t| t < layout.sections.len())?;
    let mut l = normalized(layout);
    let s: Section = l.sections.remove(at);
    l.sections.insert(to, s);
    changed(layout, l)
}

/// Moves a section to `index` among the sections (clamped).
pub fn move_section_to(layout: &GuildLayout, section: u64, index: usize) -> Option<GuildLayout> {
    let at = layout.sections.iter().position(|s| s.id == section)?;
    let mut l = normalized(layout);
    let s = l.sections.remove(at);
    l.sections.insert(index.min(l.sections.len()), s);
    changed(layout, l)
}

/// Drops the dragged item at slot `slot` of the sidebar's `items` (the row index it would take
/// among the rows as they are now). A channel dropped under a header joins that section, at its
/// top when the header is directly above; a section moves among sections only. `open` is the
/// same predicate `items` was given.
pub fn drop_item(
    layout: &GuildLayout,
    open: &dyn Fn(u64) -> bool,
    dragged: Item,
    slot: usize,
) -> Option<GuildLayout> {
    let rows = items(layout, open);
    let from = rows.iter().position(|&i| i == dragged)?;
    let to = slot.min(rows.len().saturating_sub(1));
    if from == to {
        return None;
    }
    match dragged {
        Item::Section(id) => {
            // The index among the sections is the count of headers above the target row.
            let mut rest = rows.clone();
            rest.remove(from);
            rest.insert(to, dragged);
            let index = rest
                .iter()
                .take_while(|&&i| i != dragged)
                .filter(|i| matches!(i, Item::Section(_)))
                .count();
            let at = layout.sections.iter().position(|s| s.id == id)?;
            let mut l = normalized(layout);
            let s = l.sections.remove(at);
            l.sections.insert(index.min(l.sections.len()), s);
            changed(layout, l)
        }
        Item::Channel(id) => {
            let mut rest = rows.clone();
            rest.remove(from);
            rest.insert(to, dragged);
            let pos = rest.iter().position(|&i| i == dragged)?;
            // The container is the nearest header above; the index counts channels above it there.
            let section = rest[..pos].iter().rev().find_map(|i| {
                if let Item::Section(s) = i {
                    Some(*s)
                } else {
                    None
                }
            });
            let before = rest[..pos]
                .iter()
                .rev()
                .take_while(|i| matches!(i, Item::Channel(_)))
                .count();
            // Collapsed sections hide their channels, so count against the real list instead.
            let hidden: Vec<u64> = channels_in(layout, section)
                .iter()
                .copied()
                .filter(|&c| c != id)
                .collect();
            let visible_before: Vec<u64> = rest[..pos]
                .iter()
                .rev()
                .take_while(|i| matches!(i, Item::Channel(_)))
                .filter_map(|i| {
                    if let Item::Channel(c) = i {
                        Some(*c)
                    } else {
                        None
                    }
                })
                .collect();
            let index = match visible_before.first() {
                Some(prev) => hidden
                    .iter()
                    .position(|c| c == prev)
                    .map_or(before, |p| p + 1),
                None => 0,
            };
            move_channel_to(layout, id, section, index)
        }
    }
}

/// Adds `channel` to the end of `section` unless the layout already lists it.
pub fn with_channel(layout: &GuildLayout, channel: u64, section: Option<u64>) -> GuildLayout {
    let mut l = normalized(layout);
    if section_of(&l, channel).is_none() {
        let target = if section.is_some_and(|s| l.sections.iter().any(|x| x.id == s)) {
            section
        } else {
            None
        };
        if let Some(list) = entry_mut(&mut l, target) {
            list.push(channel);
        }
    }
    l
}

#[cfg(test)]
mod tests {
    use super::*;

    fn layout() -> GuildLayout {
        let s = |id: u64, name: &str, position: i32| Section {
            id,
            guild_id: 1,
            name: name.into(),
            position,
        };
        GuildLayout {
            version: 4,
            sections: vec![s(100, "Build", 0), s(200, "Fun", 1)],
            entries: vec![
                LayoutEntry {
                    section_id: None,
                    channel_ids: vec![1, 2],
                },
                LayoutEntry {
                    section_id: Some(100),
                    channel_ids: vec![3, 4],
                },
                LayoutEntry {
                    section_id: Some(200),
                    channel_ids: vec![5],
                },
            ],
        }
    }

    fn ids(l: &GuildLayout, s: Option<u64>) -> Vec<u64> {
        channels_in(l, s).to_vec()
    }

    #[test]
    fn items_list_top_level_first_and_hide_collapsed_channels() {
        let all = items(&layout(), &|_| true);
        assert_eq!(
            all,
            [
                Item::Channel(1),
                Item::Channel(2),
                Item::Section(100),
                Item::Channel(3),
                Item::Channel(4),
                Item::Section(200),
                Item::Channel(5)
            ]
        );
        let shut = items(&layout(), &|s| s != 100);
        assert_eq!(
            shut,
            [
                Item::Channel(1),
                Item::Channel(2),
                Item::Section(100),
                Item::Section(200),
                Item::Channel(5)
            ]
        );
        assert_eq!(channel_order(&layout()), [1, 2, 3, 4, 5]);
    }

    #[test]
    fn stepping_moves_within_a_container_and_across_its_edge() {
        let up = step_channel(&layout(), 2, -1).unwrap();
        assert_eq!(ids(&up, None), [2, 1]);
        assert_eq!(up.version, 4, "the version the edit is based on");
        // Down past the end of the top level lands at the top of the first section.
        let down = step_channel(&layout(), 2, 1).unwrap();
        assert_eq!(
            (ids(&down, None), ids(&down, Some(100))),
            (vec![1], vec![2, 3, 4])
        );
        // Up past the top of a section lands at the end of the one above (here the top level).
        let back = step_channel(&layout(), 3, -1).unwrap();
        assert_eq!(
            (ids(&back, None), ids(&back, Some(100))),
            (vec![1, 2, 3], vec![4])
        );
        // The first channel can't go up and the last can't go down.
        assert!(step_channel(&layout(), 1, -1).is_none());
        assert!(step_channel(&layout(), 5, 1).is_none());
        assert!(!can_step_channel(&layout(), 1, -1) && can_step_channel(&layout(), 1, 1));
    }

    #[test]
    fn moving_to_a_section_appends_or_inserts() {
        let l = move_channel_to(&layout(), 1, Some(200), usize::MAX).unwrap();
        assert_eq!((ids(&l, None), ids(&l, Some(200))), (vec![2], vec![5, 1]));
        let l = move_channel_to(&layout(), 5, None, 0).unwrap();
        assert_eq!((ids(&l, None), ids(&l, Some(200))), (vec![5, 1, 2], vec![]));
        assert!(
            move_channel_to(&layout(), 1, None, 0).is_none(),
            "already there"
        );
        assert!(
            move_channel_to(&layout(), 1, Some(999), 0).is_none(),
            "no such section"
        );
        assert!(
            move_channel_to(&layout(), 42, None, 0).is_none(),
            "no such channel"
        );
    }

    #[test]
    fn sections_step_among_sections() {
        let l = step_section(&layout(), 200, -1).unwrap();
        assert_eq!(
            l.sections.iter().map(|s| s.id).collect::<Vec<_>>(),
            [200, 100]
        );
        assert!(step_section(&layout(), 100, -1).is_none());
        assert!(step_section(&layout(), 200, 1).is_none());
    }

    #[test]
    fn a_section_moves_to_an_index() {
        let l = move_section_to(&layout(), 100, 1).unwrap();
        assert_eq!(l.sections.iter().map(|s| s.id).collect::<Vec<_>>(), [200, 100]);
        assert!(move_section_to(&layout(), 100, 0).is_none());
        assert!(move_section_to(&layout(), 999, 0).is_none());
    }

    #[test]
    fn a_drop_joins_the_container_it_lands_in() {
        let open = |_: u64| true;
        // Rows: 1 2 [Build] 3 4 [Fun] 5. Drag channel 1 onto slot 4 (after channel 3's row, where 4 is).
        let l = drop_item(&layout(), &open, Item::Channel(1), 4).unwrap();
        assert_eq!(
            (ids(&l, None), ids(&l, Some(100))),
            (vec![2], vec![3, 4, 1])
        );
        // Directly under the header: the top of that section.
        let l = drop_item(&layout(), &open, Item::Channel(5), 3).unwrap();
        assert_eq!(ids(&l, Some(100)), [5, 3, 4]);
        // Dragging a section past the other reorders sections.
        let l = drop_item(&layout(), &open, Item::Section(100), 5).unwrap();
        assert_eq!(
            l.sections.iter().map(|s| s.id).collect::<Vec<_>>(),
            [200, 100]
        );
        // Dropping where it already is changes nothing.
        assert!(drop_item(&layout(), &open, Item::Channel(2), 1).is_none());
    }

    #[test]
    fn dropping_into_a_collapsed_section_goes_after_its_header() {
        let shut = |s: u64| s != 100;
        // Rows: 1 2 [Build] [Fun] 5. Channel 5 dropped right under the collapsed Build header.
        let l = drop_item(&layout(), &shut, Item::Channel(5), 3).unwrap();
        assert_eq!(ids(&l, Some(100)), [5, 3, 4]);
    }

    #[test]
    fn a_missing_entry_is_completed_and_new_channels_are_appended_once() {
        let mut sparse = layout();
        sparse.entries.retain(|e| e.section_id != Some(200));
        assert_eq!(normalized(&sparse).entries.len(), 3);
        let l = with_channel(&layout(), 9, Some(200));
        assert_eq!(ids(&l, Some(200)), [5, 9]);
        assert_eq!(
            ids(&with_channel(&l, 9, None), Some(200)),
            [5, 9],
            "already listed"
        );
        assert_eq!(ids(&with_channel(&layout(), 9, Some(404)), None), [1, 2, 9]);
    }
}
