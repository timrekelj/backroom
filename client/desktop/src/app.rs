//! The app root: state plus backend, the poll that carries replies into state, key handling and
//! the top-level view (auth or main, dialogs and toasts on top).

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use rune::{el, Cx, El, Key, KeyEvent, SpringPair, Transition, Tx, Waker};
use thehideout_design::{kit_for, motion, theme_for};

use crate::apply::apply;
use crate::backend::{Backend, Command};
use crate::fake::{now_unix, FakeBackend};
use crate::format::Tz;
use crate::look::{self, Saver};
use crate::real::{default_server, open_prefs, RealBackend, SharedPrefs};
use crate::state::{AppState, Auth, Screen};
use crate::ui;

/// Rune's `S`: everything handlers can reach.
pub struct App {
    pub state: AppState,
    pub backend: Box<dyn Backend>,
    /// Launch sequence of the network backend: 0 done (or fake), 1 before the first poll, 2 after
    /// it. The saved session is checked on the poll after the first frame has been drawn.
    boot: u8,
    /// Writes look changes to the prefs on a background thread. `None` in scenes and tests.
    saver: Option<Saver>,
}

impl App {
    pub fn new(backend: Box<dyn Backend>, state: AppState) -> App {
        App {
            state,
            backend,
            boot: 0,
            saver: None,
        }
    }

    /// The app as the window starts it: the saved look is read from `prefs` now, before the first
    /// frame, so the window never flashes the default theme; later changes are saved on a thread.
    pub fn start(backend: Box<dyn Backend>, mut state: AppState, prefs: SharedPrefs) -> App {
        state.ui.look = look::load(&prefs);
        let mut app = App::new(backend, state);
        app.saver = Some(Saver::spawn(prefs));
        app
    }

    /// Hands the commands queued on the state to the backend.
    pub fn flush(&mut self) {
        if std::mem::take(&mut self.state.ui.look_dirty)
            && let Some(saver) = &self.saver
        {
            saver.save(self.state.ui.look);
        }
        for cmd in std::mem::take(&mut self.state.outbox) {
            if matches!(cmd, Command::Login { .. } | Command::SignUp { .. }) {
                self.backend.set_server(&self.state.server_url());
            }
            self.backend.submit(cmd);
        }
    }

    /// A handler's way to change state: runs `f`, then sends what it queued.
    pub fn act(&mut self, f: impl FnOnce(&mut AppState)) {
        f(&mut self.state);
        self.flush();
    }

    /// Applies every reply that is due now, until it goes quiet (scenes and tests; the window
    /// does this from `poll`).
    pub fn pump(&mut self) {
        let mut tx = Tx::default();
        loop {
            self.flush();
            let replies = self.backend.poll(self.state.clock);
            if replies.is_empty() {
                break;
            }
            for r in replies {
                apply(&mut self.state, r, &mut tx);
            }
        }
    }
}

/// The pinned clock of the fake world, scenes and tests.
pub fn fake_now() -> SystemTime {
    UNIX_EPOCH + Duration::from_secs(now_unix() as u64)
}

/// The real wall clock, for the real backend.
pub fn real_now() -> SystemTime {
    SystemTime::now()
}

/// Flags of the runs that draw without a window (scenes, snapshots, scripts).
const HEADLESS: &[&str] = &[
    "--scene",
    "--snap",
    "--script",
    "--tree",
    "--html",
    "--video",
    "--frames",
    "--trace",
    "--strip",
    "--list-scenes",
    "--playground",
    "--first-pixels",
    "--cpu-boot",
];

fn headless() -> bool {
    std::env::args().skip(1).any(|a| HEADLESS.contains(&a.as_str()))
}

/// Whether this run uses the deterministic fake: `--fake`, or any headless run (scenes,
/// snapshots, scripts) unless `--real` asks for the network.
fn wants_fake() -> bool {
    let extra = rune::extra_args();
    if extra.iter().any(|a| a == "--fake") {
        return true;
    }
    if extra.iter().any(|a| a == "--real") {
        return false;
    }
    headless()
}

/// The window's first state: the network backend with a loading screen while the saved session
/// is checked, or signed out on the fake backend. The look is read from the prefs here, once,
/// before any frame; a headless run never touches the user's prefs, so snapshots stay the same
/// on every machine.
pub fn init() -> App {
    if wants_fake() {
        let state = AppState::new(fake_now, Tz::Local);
        let backend = Box::new(FakeBackend::new().with_latency(0.25));
        if headless() {
            return App::new(backend, state);
        }
        return App::start(backend, state, open_prefs());
    }
    let prefs = open_prefs();
    let mut state = AppState::new(real_now, Tz::Local);
    state.auth = Auth::new(&default_server());
    state.screen = Screen::Loading;
    let mut app = App::start(Box::new(RealBackend::with_shared(prefs.clone())), state, prefs);
    app.boot = 1;
    app
}

pub fn setup(a: &mut App, waker: Waker) {
    a.backend.set_waker(waker);
}

/// Before every frame: replies in, time-driven state forward. True when something visible changed.
pub fn poll(a: &mut App, clock: f64, tx: &mut Tx) -> bool {
    match a.boot {
        // The first poll comes before the first frame: wait for the next.
        1 => {
            a.boot = 2;
            tx.wake_after(Duration::from_millis(1));
        }
        2 => {
            a.boot = 0;
            a.state.outbox.push(Command::Resume);
        }
        _ => {}
    }
    let mut changed = false;
    loop {
        a.flush();
        let replies = a.backend.poll(clock);
        if replies.is_empty() {
            break;
        }
        changed = true;
        for r in replies {
            apply(&mut a.state, r, tx);
        }
    }
    if let Some(text) = a.state.ui.copy_request.take() {
        tx.copy(text);
    }
    let (ticked, wake) = a.state.tick(clock);
    if let Some(due) = [wake, a.backend.next_due()]
        .into_iter()
        .flatten()
        .reduce(f64::min)
    {
        tx.wake_at(due);
    }
    changed || ticked
}

/// Keys no focused control took: Cmd/Ctrl+W closes the window, Cmd/Ctrl+Q quits, Alt+Up and
/// Alt+Down move between channels, Escape leaves the settings. (Escape belongs to an open modal
/// or menu first.)
pub fn on_key(a: &mut App, ev: &KeyEvent, tx: &mut Tx) -> bool {
    match ev.key {
        Key::Command('w') => tx.close_window(),
        Key::Command('q') => tx.quit(),
        Key::Escape if a.state.ui.settings.is_some() => a.act(|s| s.close_settings()),
        Key::Up if ev.mods.alt && a.state.screen == Screen::Main => a.act(|s| s.step_channel(-1)),
        Key::Down if ev.mods.alt && a.state.screen == Screen::Main => a.act(|s| s.step_channel(1)),
        _ => return false,
    }
    true
}

pub fn view(a: &App, cx: &mut Cx<App>) -> El {
    let look = a.state.ui.look;
    let dark = look.spec.is_dark(cx.system_dark());
    let (k, t) = (kit_for(&look.spec, dark), theme_for(&look.spec, dark));
    // Reduce motion follows the system unless the user forced it (the override lasts one commit,
    // so it is restated every time the view runs).
    match look.reduce_motion {
        look::ReduceMotion::System => {}
        look::ReduceMotion::On => cx.reduced_motion(true),
        look::ReduceMotion::Off => cx.reduced_motion(false),
    }
    let lay = cx
        .layout_with("stage", |r| ui::shell::Layout::of(r[2]))
        .unwrap_or_default();
    let mut layer = Vec::new();
    // Auth and the shell share the window; each fades on `motion-overlay` and nothing else moves.
    let fade = Transition::opacity().springs(SpringPair::new(motion::OVERLAY, motion::OVERLAY));
    let screen = match a.state.screen {
        Screen::Loading => el().key("screen-loading"),
        Screen::Auth => el().key("screen-auth").child(ui::auth::build(a, k)),
        Screen::Main => el().key("screen-main").child(ui::shell::build(
            a,
            k,
            &mut layer,
            cx.chrome_insets().height,
            lay,
        )),
    };
    let screen = screen.absolute(0.0, 0.0).w_full().h_full().transition(fade);
    ui::dialogs::build(a, k, &mut layer);
    layer.push(ui::toasts::build(a, k));
    // Where the window's own controls sit over the content: a strip that drags the window.
    let mut stage = el()
        .key("stage")
        .w_full()
        .h_full()
        .fill(t.bg.color())
        .child(screen);
    let bar = cx.chrome_insets().height;
    if bar > 0.0 {
        stage = stage.child(
            el().key("titlebar")
                .absolute(0.0, 0.0)
                .w_full()
                .h(bar)
                .window_drag(),
        );
    }
    rune_kit::root(stage, layer)
}
