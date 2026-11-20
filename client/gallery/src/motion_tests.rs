//! Headless proofs of the chat motion: a sent message enters (opacity), the sidebar highlight
//! travels between rows, the dialog opens with scale and opacity. Each drives the real gallery
//! view with pointer input, since entrances only play in a commit an input caused.

use rune::{Env, Runtime, TextEdit};

use super::*;

const VIEWPORT: (f32, f32) = (1200.0, 3200.0);

fn chat_runtime() -> Runtime<G> {
    let mut g = init();
    g.page = Page::Chat;
    let mut rt = Runtime::new(
        g,
        view,
        VIEWPORT,
        Env {
            system_dark: true,
            scale: 1.0,
        },
    );
    settle(&mut rt, 90);
    rt
}

fn settle(rt: &mut Runtime<G>, frames: usize) {
    for _ in 0..frames {
        rt.frame(1.0 / 60.0);
    }
}

fn click(rt: &mut Runtime<G>, key: &str) {
    let (x, y) = rt.find(key).unwrap_or_else(|| panic!("no element {key}"));
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
}

#[test]
fn a_sent_message_fades_in_and_the_pending_body_rests_dimmed() {
    let mut rt = chat_runtime();
    rt.update(|g, _| g.chat.draft = TextEdit::new("Deploying now"));
    settle(&mut rt, 3);
    click(&mut rt, "composer/send");
    settle(&mut rt, 1);
    assert!(
        rt.state.chat.messages.iter().any(|m| m.id == "n1"),
        "send appended n1"
    );
    assert_eq!(
        rt.state.chat.draft.text, "",
        "the field cleared in the same commit"
    );
    settle(&mut rt, 3);
    let (_, mid) = rt.presented("msg-n1").expect("the new row is mounted");
    assert!(mid > 0.0 && mid < 1.0, "mid-frame opacity {mid}");
    settle(&mut rt, 60);
    let (_, rest) = rt.presented("msg-n1").unwrap();
    assert_eq!(rest, 1.0, "the row rests opaque");
}

#[test]
fn a_message_that_starts_a_new_group_enters_too() {
    let mut rt = chat_runtime();
    // Drop Tim's own run so the next send starts a group after Ana's.
    rt.update(|g, _| g.chat.messages.retain(|m| m.id != "m7" && m.id != "m8"));
    settle(&mut rt, 90);
    rt.update(|g, _| g.chat.draft = TextEdit::new("A fresh group"));
    settle(&mut rt, 3);
    click(&mut rt, "composer/send");
    settle(&mut rt, 4);
    let (_, mid) = rt.presented("msg-n1").expect("the new row is mounted");
    assert!(mid > 0.0 && mid < 1.0, "mid-frame opacity {mid}");
}

fn body_opacity(rt: &Runtime<G>, id: &str) -> f32 {
    let row = format!("msg-{id}/");
    rt.elements()
        .into_iter()
        .find(|e| e.key.as_deref() == Some("body") && e.path.contains(&row))
        .unwrap_or_else(|| panic!("no body for {id}"))
        .opacity
}

#[test]
fn pending_to_sent_only_changes_opacity() {
    let mut rt = chat_runtime();
    rt.update(|g, _| g.chat.send("first".into()));
    settle(&mut rt, 90);
    let dim = body_opacity(&rt, "n1");
    assert!(
        (dim - 0.55).abs() < 0.01,
        "a pending body rests at 0.55, got {dim}"
    );
    let before = rt.presented("msg-n1").unwrap().0;
    // The second send acks the first: Pending -> Sent, with no movement of its own.
    rt.update(|g, _| g.chat.send("second".into()));
    settle(&mut rt, 3);
    let mid = body_opacity(&rt, "n1");
    assert!(mid > 0.56 && mid < 0.99, "the ack fades the body up: {mid}");
    settle(&mut rt, 90);
    assert!((body_opacity(&rt, "n1") - 1.0).abs() < 0.01);
    let after = rt.presented("msg-n1").unwrap().0;
    // The stream is stuck to the bottom, so the row may scroll up to make room, but its size holds.
    assert!(
        (before[2] - after[2]).abs() < 0.01 && (before[3] - after[3]).abs() < 0.01,
        "size unchanged: {before:?} -> {after:?}"
    );
}

#[test]
fn the_selection_highlight_travels_between_rows() {
    let mut rt = chat_runtime();
    let (_, start_y) = rt
        .find("channels-highlight")
        .expect("a highlight on the selected row");
    let (_, end_y) = rt.find("channels-row-ops").unwrap();
    assert!(end_y > start_y + 40.0);
    click(&mut rt, "channels-row-ops");
    settle(&mut rt, 4);
    let (r, _) = rt.presented("channels-highlight").unwrap();
    let mid_y = r[1] + r[3] * 0.5;
    assert!(
        mid_y > start_y + 2.0 && mid_y < end_y - 2.0,
        "mid-flight y {mid_y} between {start_y} and {end_y}"
    );
    settle(&mut rt, 90);
    let (r, o) = rt.presented("channels-highlight").unwrap();
    assert!(
        (r[1] + r[3] * 0.5 - end_y).abs() < 0.5,
        "lands on the new row"
    );
    assert_eq!(o, 1.0);
}

#[test]
fn the_dialog_opens_with_scale_and_opacity() {
    let mut rt = chat_runtime();
    assert!(rt.presented("demo-dialog").is_none(), "closed at first");
    click(&mut rt, "open-dialog");
    settle(&mut rt, 3);
    let info = |rt: &Runtime<G>| {
        rt.elements()
            .into_iter()
            .find(|e| e.key.as_deref() == Some("demo-dialog"))
            .expect("the panel is in the tree")
    };
    let mid = info(&rt);
    assert!(
        mid.opacity > 0.0 && mid.opacity < 1.0,
        "mid opacity {}",
        mid.opacity
    );
    assert!(
        mid.scale >= 0.98 && mid.scale < 1.0,
        "mid scale {}",
        mid.scale
    );
    settle(&mut rt, 60);
    let rest = info(&rt);
    assert_eq!(rest.opacity, 1.0);
    assert!((rest.scale - 1.0).abs() < 1e-3, "rest scale {}", rest.scale);
}

#[test]
fn the_scene_the_dialog_scenes_build_shows_it_open() {
    let mut g = init();
    g.page = Page::Chat;
    g.chat.dialog = true;
    let mut rt = Runtime::new(
        g,
        view,
        VIEWPORT,
        Env {
            system_dark: true,
            scale: 1.0,
        },
    );
    settle(&mut rt, 2);
    assert_eq!(
        rt.presented("demo-dialog").unwrap().1,
        1.0,
        "nothing animates on the first frame"
    );
}
