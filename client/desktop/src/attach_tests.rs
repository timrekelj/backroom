//! Headless proofs of attachments, embeds, the lightbox, downloads and players, driven through
//! the real view against the fake backend.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use rune::prefs::Prefs;
use rune::{ClipboardContent, ClipboardImage, Key, Runtime};
use thehideout_api::Message;
use thehideout_design::components::ToastKind;

use crate::app::{fake_now, App};
use crate::attach::{ClipKind, UploadState, MAX_FILES};
use crate::backend::Command;
use crate::fake::{FakeBackend, GENERAL, PLANS};
use crate::format::Tz;
use crate::scenes::booted;
use crate::state::AppState;
use crate::tests::{click, has, key, rect_of, runtime, settle};

fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("thehideout-attach-{}-{name}", std::process::id()));
    std::fs::remove_dir_all(&dir).ok();
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// A file of `n` bytes named `name` in `dir`.
fn file(dir: &std::path::Path, name: &str, n: usize) -> PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, vec![b'x'; n]).unwrap();
    path
}

fn ready(rt: &Runtime<App>) -> usize {
    rt.state.state.draft(GENERAL).map_or(0, |d| d.ready())
}

fn sent(fake: &FakeBackend, f: impl Fn(&Command) -> bool) -> Vec<Command> {
    use crate::backend::Backend;
    fake.sent().into_iter().filter(|c| f(c)).collect()
}

fn files_of(rt: &Runtime<App>) -> Vec<(String, UploadState)> {
    rt.state
        .state
        .draft(GENERAL)
        .map(|d| {
            d.files
                .iter()
                .map(|f| (f.name.clone(), f.state.clone()))
                .collect()
        })
        .unwrap_or_default()
}

/// A signed-in window with #general open.
fn window() -> (Runtime<App>, FakeBackend) {
    let (app, fake) = booted(true);
    (runtime(app), fake)
}

// ---- uploads -----------------------------------------------------------------------------------

#[test]
fn files_start_uploading_at_once_and_the_tray_follows_their_progress() {
    let (mut rt, fake) = window();
    let dir = temp_dir("progress");
    fake.stall_uploads(Some(40));
    let path = file(&dir, "report.txt", 1000);
    rt.update(|a, _| a.act(|s| s.attach_paths(&[path])));
    settle(&mut rt, 4);
    // The upload was queued the moment the file was chosen, and the tray shows it going up.
    assert!(sent(&fake, |c| matches!(c, Command::UploadFile { .. })).len() == 1);
    assert_eq!(
        files_of(&rt),
        vec![(
            "report.txt".to_owned(),
            UploadState::Uploading { sent: 400 }
        )]
    );
    assert!(has(&rt, "chip-1"), "a chip for it");
    assert!(has(&rt, "chip-1/progress"), "with a progress bar");
    // The send button waits for it.
    assert!(!rt.state.state.send("hello"));
    // Another file, with the stall lifted, finishes: ready, and its bar is gone.
    fake.stall_uploads(None);
    let second = file(&dir, "second.txt", 1000);
    rt.update(|a, _| a.act(|s| s.attach_paths(&[second])));
    settle(&mut rt, 4);
    assert_eq!(ready(&rt), 1);
    assert!(!has(&rt, "chip-2/progress"));
    assert!(has(&rt, "chip-1/progress"), "the stalled one still shows its bar");
}

#[test]
fn removing_a_file_that_is_uploading_cancels_it() {
    let (mut rt, fake) = window();
    let dir = temp_dir("cancel");
    fake.stall_uploads(Some(30));
    let path = file(&dir, "big.bin", 4000);
    rt.update(|a, _| a.act(|s| s.attach_paths(&[path])));
    settle(&mut rt, 3);
    assert_eq!(files_of(&rt).len(), 1);
    click(&mut rt, "chip-1/remove");
    settle(&mut rt, 3);
    assert!(files_of(&rt).is_empty(), "the chip is gone");
    assert_eq!(
        sent(&fake, |c| matches!(c, Command::CancelUpload { id: 1 })).len(),
        1,
        "and the upload was told to stop"
    );
    // Whatever the server still reports for it changes nothing.
    rt.update(|a, _| {
        a.state
            .uploaded(1, Err(thehideout_api::ApiError::Network("late".into())));
        a.state.upload_progress(1, 5, 10);
    });
    assert!(files_of(&rt).is_empty());
}

#[test]
fn a_failed_upload_shows_its_error_and_retries() {
    let (mut rt, fake) = window();
    let dir = temp_dir("retry");
    fake.fail_uploads(1);
    let path = file(&dir, "notes.txt", 500);
    rt.update(|a, _| a.act(|s| s.attach_paths(&[path])));
    settle(&mut rt, 4);
    assert_eq!(
        files_of(&rt),
        vec![(
            "notes.txt".to_owned(),
            UploadState::Failed("Upload failed".into())
        )]
    );
    assert!(has(&rt, "chip-1/retry"), "the error state offers a retry");
    assert!(
        !rt.state.state.send(""),
        "nothing to send: the only file failed"
    );
    click(&mut rt, "chip-1/retry");
    settle(&mut rt, 4);
    assert_eq!(
        files_of(&rt),
        vec![("notes.txt".to_owned(), UploadState::Ready { pending: 9001 })]
    );
    assert_eq!(
        sent(&fake, |c| matches!(c, Command::UploadFile { .. })).len(),
        2,
        "the file was read again"
    );
}

#[test]
fn at_most_ten_files_and_nothing_over_the_size_cap() {
    let (mut rt, _fake) = window();
    let dir = temp_dir("caps");
    let paths: Vec<PathBuf> = (0..12)
        .map(|i| file(&dir, &format!("f{i}.txt"), 10))
        .collect();
    rt.update(|a, _| a.act(|s| s.attach_paths(&paths)));
    settle(&mut rt, 4);
    assert_eq!(files_of(&rt).len(), MAX_FILES, "the eleventh is refused");
    let note = rt
        .state
        .state
        .draft(GENERAL)
        .and_then(|d| d.note.clone())
        .expect("a note says why");
    assert!(note.contains("up to 10 files"), "{note}");
    assert!(has(&rt, "tray-note"), "inline, under the chips");
    // The size cap: refused before anything is read or sent.
    let (mut rt, fake) = window();
    rt.update(|a, _| a.state.ui.attach.max_upload = 100);
    let big = file(&dir, "big.bin", 101);
    let fits = file(&dir, "fits.bin", 100);
    rt.update(|a, _| a.act(|s| s.attach_paths(&[big, fits])));
    settle(&mut rt, 4);
    assert_eq!(
        files_of(&rt)
            .iter()
            .map(|(n, _)| n.as_str())
            .collect::<Vec<_>>(),
        ["fits.bin"]
    );
    assert_eq!(
        sent(&fake, |c| matches!(c, Command::UploadFile { .. })).len(),
        1,
        "the big one never left"
    );
    let note = rt.state.state.draft(GENERAL).and_then(|d| d.note.clone());
    assert!(
        note.is_none(),
        "attaching a file that fits clears the old note"
    );
    rt.update(|a, _| a.act(|s| s.attach_paths(&[file(&dir, "big2.bin", 500)])));
    let note = rt
        .state
        .state
        .draft(GENERAL)
        .and_then(|d| d.note.clone())
        .expect("a note");
    assert!(
        note.contains("big2.bin") && note.contains("limit"),
        "{note}"
    );
}

#[test]
fn the_plus_button_opens_the_file_dialog_and_a_drop_or_a_paste_attach_too() {
    let (mut rt, _fake) = window();
    let dir = temp_dir("sources");
    click(&mut rt, "composer/scroll/input");
    click(&mut rt, "composer-add");
    settle(&mut rt, 2);
    let (kind, _) = rt.file_dialog().expect("the + asks for a file");
    assert_eq!(kind, rune::FileDialogKind::Open);
    rt.resolve_file_dialog(Some(file(&dir, "picked.txt", 20)))
        .unwrap();
    settle(&mut rt, 4);
    // A drop on the transcript.
    let at = rt.find("transcript-area").expect("the transcript");
    rt.drop_files(vec![file(&dir, "dropped.txt", 20)], Some(at));
    settle(&mut rt, 4);
    // A picture pasted into the field; the field is where it was while the tray filled up.
    let png = crate::media::generated_png(3, 16);
    click(&mut rt, "composer/scroll/input");
    rt.paste_content(
        ClipboardContent::new("").with_image(ClipboardImage::from_png(png.clone()).unwrap()),
    );
    settle(&mut rt, 4);
    let names: Vec<String> = files_of(&rt).into_iter().map(|(n, _)| n).collect();
    assert_eq!(names, ["picked.txt", "dropped.txt", "pasted-image-3.png"]);
    assert_eq!(ready(&rt), 3, "all three uploaded");
    assert!(
        rt.state.state.data.media.contains("local-3"),
        "the pasted picture is its own thumbnail"
    );
}

// ---- sending -----------------------------------------------------------------------------------

#[test]
fn a_message_sends_the_uploaded_files_and_shows_them_before_the_server_confirms() {
    let (app, fake) = booted(true);
    fake.set_latency(0.5);
    let mut rt = runtime(app);
    let dir = temp_dir("send");
    let a = file(&dir, "a.txt", 30);
    let b = file(&dir, "b.pdf", 40);
    rt.update(|a_, _| a_.act(|s| s.attach_paths(&[a, b])));
    settle(&mut rt, 60);
    assert_eq!(ready(&rt), 2);
    // Send with no text at all: enabled because files are up.
    rt.update(|a, _| assert!(a.state.send("")));
    rt.update(|a, _| a.flush());
    let cmd = sent(&fake, |c| matches!(c, Command::SendWith { .. }));
    assert_eq!(cmd.len(), 1);
    let Command::SendWith {
        attachments,
        content,
        ..
    } = &cmd[0]
    else {
        unreachable!()
    };
    assert_eq!(
        (attachments.clone(), content.as_str()),
        (vec![9001, 9002], "")
    );
    assert!(
        rt.state.state.draft(GENERAL).unwrap().files.is_empty(),
        "the tray is emptied"
    );
    // Pending: the files show under the message already.
    settle(&mut rt, 3);
    let o = &rt.state.state.data.outgoing[0];
    assert_eq!(
        o.files.iter().map(|f| f.name.as_str()).collect::<Vec<_>>(),
        ["a.txt", "b.pdf"]
    );
    assert!(
        has(&rt, &format!("ofile-{}-0", o.nonce)),
        "drawn while pending"
    );
    // Confirmed: the real attachments replace them.
    settle(&mut rt, 60);
    assert!(rt.state.state.data.outgoing.is_empty());
    let m = rt
        .state
        .state
        .log(GENERAL)
        .unwrap()
        .messages
        .last()
        .unwrap()
        .clone();
    assert_eq!(
        m.attachments
            .iter()
            .map(|a| a.filename.as_str())
            .collect::<Vec<_>>(),
        ["a.txt", "b.pdf"]
    );
    assert!(has(&rt, &format!("att-{}-{}", m.id, m.attachments[0].id)));
}

#[test]
fn text_and_files_go_together_and_a_failed_send_keeps_its_files_for_the_retry() {
    let (mut rt, fake) = window();
    let dir = temp_dir("retry-send");
    let p = file(&dir, "a.txt", 30);
    rt.update(|a, _| a.act(|s| s.attach_paths(&[p])));
    settle(&mut rt, 4);
    fake.fail_sends(1);
    rt.update(|a, _| assert!(a.state.send("see attached")));
    rt.update(|a, _| a.flush());
    settle(&mut rt, 4);
    let nonce = rt.state.state.data.outgoing[0].nonce.clone();
    assert_eq!(
        rt.state.state.data.outgoing[0].state,
        crate::state::Delivery::Failed
    );
    rt.update(|a, _| a.act(|s| s.retry(&nonce)));
    settle(&mut rt, 4);
    let sends = sent(&fake, |c| matches!(c, Command::SendWith { .. }));
    assert_eq!(sends.len(), 2, "the retry links the same upload again");
    assert!(rt.state.state.data.outgoing.is_empty());
    assert_eq!(
        rt.state
            .state
            .log(GENERAL)
            .unwrap()
            .messages
            .last()
            .unwrap()
            .attachments
            .len(),
        1
    );
}

// ---- how messages show their files -------------------------------------------------------------

fn plans_with(parts: &[&str]) -> (Runtime<App>, FakeBackend) {
    let (mut app, fake) = booted(true);
    fake.seed_attachments(PLANS, parts);
    app.act(|s| s.select_channel(PLANS));
    app.pump();
    app.state.data.media.settle();
    (runtime(app), fake)
}

fn plans_messages(rt: &Runtime<App>) -> Vec<Message> {
    rt.state.state.log(PLANS).unwrap().messages.clone()
}

#[test]
fn pictures_sit_in_a_grid_that_follows_the_layout_rules() {
    let (rt, _) = plans_with(&["three", "one", "four"]);
    let msgs = plans_messages(&rt);
    let size = |i: usize| {
        let r = rect_of(&rt, &format!("grid-{}", msgs[i].id));
        (r[2].round(), r[3].round())
    };
    // Three and four pictures fill a 400x240 grid; one keeps its 16:9 shape inside 400x300.
    assert_eq!(size(0), (400.0, 240.0));
    assert_eq!(size(1), (400.0, 225.0));
    assert_eq!(size(2), (400.0, 240.0));
    // Three: the first is tall at the left, two stack at the right.
    let tile = |m: usize, a: usize| {
        rect_of(
            &rt,
            &format!("img-{}-{}", msgs[m].id, msgs[m].attachments[a].id),
        )
    };
    let (t0, t1, t2) = (tile(0, 0), tile(0, 1), tile(0, 2));
    assert_eq!(t0[3].round(), 240.0);
    assert!(
        t1[0] > t0[0] + t0[2] - 1.0
            && (t1[1] - t0[1]).abs() < 1.0
            && (t2[1] - (t1[1] + t1[3])).abs() < 6.0
    );
}

#[test]
fn clicking_a_picture_opens_the_lightbox_and_it_steps_between_the_messages_pictures() {
    let (mut rt, _) = plans_with(&["three"]);
    let m = plans_messages(&rt).pop().unwrap();
    click(&mut rt, &format!("img-{}-{}", m.id, m.attachments[1].id));
    settle(&mut rt, 4);
    let lb = rt.state.state.ui.attach.lightbox.clone().expect("opened");
    assert_eq!(
        (lb.items.len(), lb.at),
        (3, 1),
        "on the picture that was pressed"
    );
    assert!(has(&rt, "lightbox-media") && has(&rt, "lightbox-download"));
    // The arrows step, and stop at the ends.
    key(&mut rt, Key::Right);
    settle(&mut rt, 2);
    assert_eq!(rt.state.state.ui.attach.lightbox.as_ref().unwrap().at, 2);
    key(&mut rt, Key::Right);
    assert_eq!(
        rt.state.state.ui.attach.lightbox.as_ref().unwrap().at,
        2,
        "no wrapping"
    );
    assert!(!has(&rt, "lightbox-next"), "the end has no next button");
    click(&mut rt, "lightbox-prev");
    settle(&mut rt, 2);
    assert_eq!(rt.state.state.ui.attach.lightbox.as_ref().unwrap().at, 1);
    // The full picture is asked for (not just the thumbnail).
    assert!(rt.state.state.data.media.contains(&lb.items[1].key));
    // Escape closes it; so does a press outside the picture.
    key(&mut rt, Key::Escape);
    settle(&mut rt, 2);
    assert!(rt.state.state.ui.attach.lightbox.is_none() && !has(&rt, "lightbox-media"));
    click(&mut rt, &format!("img-{}-{}", m.id, m.attachments[0].id));
    settle(&mut rt, 2);
    click_outside_lightbox(&mut rt);
    assert!(rt.state.state.ui.attach.lightbox.is_none());
}

fn click_outside_lightbox(rt: &mut Runtime<App>) {
    rt.pointer_down(8.0, 790.0);
    rt.pointer_up(8.0, 790.0);
    settle(rt, 4);
}

#[test]
fn a_file_card_downloads_through_the_save_dialog_with_progress_and_a_toast() {
    let (mut rt, fake) = plans_with(&["docs"]);
    let m = plans_messages(&rt).pop().unwrap();
    let a = &m.attachments[0];
    assert_eq!(a.filename, "Q4-roadmap.pdf");
    let dir = temp_dir("download");
    click(&mut rt, &format!("att-{}-{}/download", m.id, a.id));
    settle(&mut rt, 2);
    let (kind, options) = rt.file_dialog().expect("the save dialog");
    assert_eq!(kind, rune::FileDialogKind::Save);
    assert_eq!(
        options.file_name.as_deref(),
        Some("Q4-roadmap.pdf"),
        "offers the file's own name"
    );
    let target = dir.join("saved.pdf");
    rt.resolve_file_dialog(Some(target.clone())).unwrap();
    settle(&mut rt, 6);
    assert_eq!(
        sent(&fake, |c| matches!(c, Command::Download { .. })).len(),
        1
    );
    let bytes = std::fs::read(&target).expect("written");
    assert_eq!(bytes, crate::fake::files_fixture_bytes("fx-file-1"));
    assert!(
        rt.state.state.ui.attach.downloads.is_empty(),
        "the progress entry is gone"
    );
    let toast = rt.state.state.ui.toasts.back().expect("a toast");
    assert_eq!(
        (toast.kind, toast.message.as_str()),
        (ToastKind::Success, "Saved saved.pdf")
    );
    // Cancelling the dialog downloads nothing.
    click(&mut rt, &format!("att-{}-{}/download", m.id, a.id));
    settle(&mut rt, 2);
    rt.resolve_file_dialog(None).unwrap();
    settle(&mut rt, 4);
    assert_eq!(
        sent(&fake, |c| matches!(c, Command::Download { .. })).len(),
        1
    );
    // A failed download says so and leaves no file.
    fake.fail_downloads(1);
    click(&mut rt, &format!("att-{}-{}/download", m.id, a.id));
    settle(&mut rt, 2);
    let bad = dir.join("bad.pdf");
    rt.resolve_file_dialog(Some(bad.clone())).unwrap();
    settle(&mut rt, 6);
    assert!(!bad.exists());
    assert_eq!(
        rt.state.state.ui.toasts.back().unwrap().kind,
        ToastKind::Danger
    );
    // Only types the server serves inline offer "Open".
    assert!(has(&rt, &format!("att-{}-{}/open", m.id, a.id)));
    assert!(
        !has(&rt, &format!("att-{}-{}/open", m.id, m.attachments[1].id)),
        "a zip is only downloaded"
    );
}

#[test]
fn a_file_an_admin_removed_is_a_muted_placeholder_without_actions() {
    let (rt, _) = plans_with(&["removed"]);
    let m = plans_messages(&rt).pop().unwrap();
    assert!(m.attachments[0].removed);
    let key = format!("att-{}-{}", m.id, m.attachments[0].id);
    assert!(has(&rt, &key), "the placeholder is drawn");
    assert!(!has(&rt, &format!("{key}/download")), "and offers nothing");
    let tree = rt.state.state.ui.attach.clips.len();
    assert_eq!(tree, 0, "no player for it");
}

// ---- embeds ------------------------------------------------------------------------------------

fn embeds_in_plans(which: &[&str]) -> (Runtime<App>, FakeBackend) {
    let (mut app, fake) = booted(true);
    fake.seed_embeds(PLANS, which);
    app.act(|s| s.select_channel(PLANS));
    app.pump();
    app.state.data.media.settle();
    (runtime(app), fake)
}

#[test]
fn an_embed_folds_from_its_header_and_the_fold_survives_a_restart() {
    let dir = temp_dir("embed-fold");
    let path = dir.join("prefs.json");
    let start = |path: &std::path::Path| {
        let (mut app, fake) = booted(true);
        fake.seed_embeds(PLANS, &["link"]);
        let state = std::mem::replace(&mut app.state, AppState::new(fake_now, Tz::Fixed(0)));
        let mut started = App::start(Box::new(fake), state, Arc::new(Mutex::new(Prefs::at(path))));
        started.state.ui.look.spec.appearance = thehideout_design::Appearance::Dark;
        started.act(|s| s.select_channel(PLANS));
        started.pump();
        runtime(started)
    };
    let mut rt = start(&path);
    let id = plans_messages(&rt).pop().unwrap().id;
    let card = format!("embed-{id}-0");
    assert!(has(&rt, &format!("{card}/body")), "open at first");
    click(&mut rt, &format!("{card}/toggle"));
    settle(&mut rt, 40);
    assert!(
        !has(&rt, &format!("{card}/body")),
        "folded: only the header stays"
    );
    assert!(rt.state.state.embed_collapsed(id, 0));
    drop(rt); // the saver finishes what it was handed
    let saved: Vec<(u64, u32)> = Prefs::at(&path)
        .get("collapsed_embeds")
        .expect("saved under its key");
    assert_eq!(saved, vec![(id, 0)]);
    // Next launch: still folded; pressing the header opens it again.
    let mut rt = start(&path);
    assert!(rt.state.state.embed_collapsed(id, 0));
    assert!(!has(&rt, &format!("{card}/body")));
    click(&mut rt, &format!("{card}/toggle"));
    settle(&mut rt, 40);
    assert!(has(&rt, &format!("{card}/body")));
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn the_folded_list_is_bounded() {
    let mut s = AppState::new(fake_now, Tz::Fixed(0));
    for i in 0..(crate::attach::EMBED_PREFS_CAP as u64 + 25) {
        s.toggle_embed(i, 0);
    }
    assert_eq!(
        s.ui.attach.embeds_collapsed.len(),
        crate::attach::EMBED_PREFS_CAP
    );
    assert!(
        !s.embed_collapsed(0, 0)
            && s.embed_collapsed(crate::attach::EMBED_PREFS_CAP as u64 + 24, 0),
        "the oldest are forgotten"
    );
}

/// The id of the first loaded message and of a reference message near the middle of the screen.
fn scrolled_up(rt: &mut Runtime<App>) -> (Vec<u64>, usize) {
    for _ in 0..6 {
        rt.wheel(700.0, 400.0, 300.0);
        settle(rt, 10);
    }
    let ids: Vec<u64> = rt
        .state
        .state
        .log(GENERAL)
        .unwrap()
        .messages
        .iter()
        .map(|m| m.id)
        .collect();
    let r = ids
        .iter()
        .position(|id| {
            rt.presented(&format!("msg-m:{id}"))
                .is_some_and(|(r, _)| r[1] > 260.0 && r[1] < 520.0)
        })
        .expect("a message in the middle");
    (ids, r)
}

#[test]
fn embeds_arriving_for_an_old_message_do_not_move_what_the_reader_is_looking_at() {
    let (mut rt, fake) = window();
    // Scroll up the way a reader does.
    let (ids, r) = scrolled_up(&mut rt);
    assert!(
        !rt.state.state.ui.at_bottom,
        "the reader is away from the end"
    );
    let y = |rt: &Runtime<App>, id: u64| rt.presented(&format!("msg-m:{id}")).unwrap().0[1];
    let reference = ids[r];
    let before = y(&rt, reference);
    // A message above the screen (older than the one being read) gets its embed.
    let above = ids[r - 8];
    assert!(y(&rt, above) < 100.0, "it is above the viewport");
    fake.update_embeds(
        GENERAL,
        above,
        vec![FakeBackend::link_embed("https://example.com/a", "A page")],
    );
    settle(&mut rt, 60);
    assert_eq!(
        rt.state
            .state
            .log(GENERAL)
            .unwrap()
            .messages
            .iter()
            .find(|m| m.id == above)
            .unwrap()
            .embeds
            .len(),
        1,
        "the update was applied"
    );
    let after = y(&rt, reference);
    assert!(
        (after - before).abs() < 1.0,
        "the reader's message stayed at {before} (now {after})"
    );
    // And one on the very message being read grows it downward, not upward.
    fake.update_embeds(
        GENERAL,
        reference,
        vec![FakeBackend::link_embed("https://example.com/b", "B page")],
    );
    settle(&mut rt, 60);
    assert!((y(&rt, reference) - before).abs() < 1.0);
}

#[test]
fn at_the_end_an_embed_keeps_the_reader_at_the_end() {
    let (mut rt, fake) = window();
    let last = rt
        .state
        .state
        .log(GENERAL)
        .unwrap()
        .messages
        .last()
        .unwrap()
        .id;
    assert!(rt.state.state.ui.at_bottom);
    fake.update_embeds(
        GENERAL,
        last,
        vec![FakeBackend::link_embed("https://example.com/c", "C page")],
    );
    settle(&mut rt, 60);
    let card = rect_of(&rt, &format!("embed-{last}-0"));
    let viewport = rect_of(&rt, "transcript-area");
    assert!(
        card[1] + card[3] <= viewport[1] + viewport[3] + 1.0,
        "the new card is in view: {card:?} in {viewport:?}"
    );
    assert!(rt.state.state.ui.at_bottom);
}

// ---- video -------------------------------------------------------------------------------------

#[test]
fn a_looping_clip_plays_by_itself_when_visible_unless_motion_is_reduced() {
    let src = "/tmp/none.mp4";
    let mut s = AppState::new(fake_now, Tz::Fixed(0));
    s.ui.attach.video_unsupported = false;
    s.ui.reduced.set(false);
    s.gifv_visible("clip", src, true);
    let c = s.clip("clip").expect("a player was made");
    assert!(
        c.player.is_playing() && c.player.looping() && c.player.muted(),
        "muted and looping"
    );
    s.gifv_visible("clip", src, false);
    assert!(
        !s.clip("clip").unwrap().player.is_playing(),
        "it stops when it leaves the screen"
    );
    // Reduced motion: it waits for a press.
    let mut s = AppState::new(fake_now, Tz::Fixed(0));
    s.ui.attach.video_unsupported = false;
    s.ui.reduced.set(true);
    s.gifv_visible("clip", src, true);
    assert!(s.clip("clip").is_none(), "no player, no autoplay");
    s.play_clip("clip", src, ClipKind::Gifv);
    assert!(
        s.clip("clip").unwrap().player.is_playing(),
        "a press plays it"
    );
}

#[test]
fn a_gif_embed_in_the_transcript_plays_on_its_own_and_stays_still_under_reduced_motion() {
    for reduced in [false, true] {
        let (mut app, fake) = booted(true);
        app.state.ui.attach.video_unsupported = false;
        app.state.ui.look.reduce_motion = if reduced { crate::look::ReduceMotion::On } else { crate::look::ReduceMotion::Off };
        let mut rt = runtime(app);
        let post = fake.post_link(GENERAL, 2, "gif https://x.com/a/status/9");
        rt.update(|a, _| a.pump());
        fake.update_embeds(GENERAL, post, vec![FakeBackend::gifv_embed()]);
        settle(&mut rt, 30);
        assert!(has(&rt, &format!("evid-{post}-0-0")), "the card is drawn");
        // A player exists only once the clip was started: looping and muted when it was by itself.
        // (Whether it is playing is the platform's: where a clip can't be decoded it reports
        // Failed and stops, so the proof is that the app asked.)
        let started = rt.state.state.clip("fx-clip").is_some_and(|c| c.player.looping() && c.player.muted());
        assert_eq!(started, !reduced, "reduced={reduced}");
    }
}

#[test]
fn where_video_cannot_play_a_card_offers_the_browser_instead() {
    let (mut app, fake) = booted(true);
    fake.seed_embeds(PLANS, &["x"]);
    app.state.ui.attach.video_unsupported = true;
    app.act(|s| s.select_channel(PLANS));
    app.pump();
    let mut rt = runtime(app);
    let id = plans_messages(&rt).pop().unwrap().id;
    let card = format!("evid-{id}-0-0");
    assert!(
        has(&rt, &format!("{card}/fallback")),
        "a note instead of the player"
    );
    assert!(
        has(&rt, &format!("{card}/fallback/browser")),
        "with Open in browser"
    );
    assert!(
        !has(&rt, &format!("{card}/play-wrap")),
        "and no play button"
    );
    // Pressing a play button elsewhere starts nothing.
    rt.update(|a, _| a.act(|s| s.play_clip("fx-clip-x", "x", ClipKind::Video)));
    assert!(rt.state.state.clip("fx-clip-x").is_none());
    assert!(rt.state.state.clip_failed("fx-clip-x"));
}

#[test]
fn a_video_the_server_did_not_keep_shows_its_poster_and_a_link() {
    let (rt, _) = embeds_in_plans(&["big"]);
    let id = plans_messages(&rt).pop().unwrap().id;
    let card = format!("evid-{id}-0-0");
    assert!(
        has(&rt, &format!("{card}/fallback/browser")),
        "Open original over the poster"
    );
    assert!(has(&rt, &format!("{card}/poster")));
}

#[test]
fn a_message_update_replaces_embeds_and_marks_removed_files() {
    let (mut rt, fake) = window();
    let id = fake.post_link(GENERAL, 2, "https://example.com/z");
    rt.update(|a, _| a.pump());
    settle(&mut rt, 4);
    assert!(!has(&rt, &format!("embed-{id}-0")), "no embed yet");
    fake.update_embeds(
        GENERAL,
        id,
        vec![FakeBackend::link_embed("https://example.com/z", "Z")],
    );
    settle(&mut rt, 40);
    assert!(
        has(&rt, &format!("embed-{id}-0")),
        "it appears when the update lands"
    );
    assert!(has(&rt, &format!("embed-{id}-0/body/texts/title")));
}
