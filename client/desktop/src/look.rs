//! The user's look: the design crate's `ThemeSpec` plus the reduce-motion override, kept in the
//! app's prefs under `appearance` as JSON.
//!
//! The prefs are read once, in `app::init`, before the first frame; changes are written by a
//! background thread (`Saver`), never from a handler, `poll` or `view`: Rune keeps file I/O out
//! of the frame loop. The thread shares the one `Prefs` with the network backend, which keeps
//! the session in the same file, so neither can overwrite the other's keys.

use std::sync::mpsc::{self, Sender};
use std::thread::{self, JoinHandle};

use serde::{Deserialize, Serialize};
use thehideout_design::ThemeSpec;

/// The prefs key.
pub const KEY: &str = "appearance";

pub use crate::real::SharedPrefs;

/// Whether to follow the system's reduce-motion setting or force it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ReduceMotion {
    #[default]
    System,
    On,
    Off,
}

impl ReduceMotion {
    pub const ALL: [ReduceMotion; 3] = [ReduceMotion::System, ReduceMotion::On, ReduceMotion::Off];

    pub fn name(self) -> &'static str {
        match self {
            ReduceMotion::System => "System",
            ReduceMotion::On => "On",
            ReduceMotion::Off => "Off",
        }
    }
}

/// Everything on the Appearance page. Unknown or missing fields fall back to the defaults, so a
/// file from another version still loads.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Look {
    #[serde(flatten)]
    pub spec: ThemeSpec,
    pub reduce_motion: ReduceMotion,
}

/// The saved look, or the default when there is none (or it doesn't parse).
pub fn load(prefs: &SharedPrefs) -> Look {
    prefs.lock().unwrap_or_else(|e| e.into_inner()).get(KEY).unwrap_or_default()
}

/// Writes looks to the prefs on its own thread; a burst of changes is written as its last one.
pub struct Saver {
    tx: Option<Sender<Look>>,
    thread: Option<JoinHandle<()>>,
}

impl Saver {
    pub fn spawn(prefs: SharedPrefs) -> Saver {
        let (tx, rx) = mpsc::channel::<Look>();
        let thread = thread::Builder::new()
            .name("thehideout-prefs".into())
            .spawn(move || {
                while let Ok(mut look) = rx.recv() {
                    while let Ok(newer) = rx.try_recv() {
                        look = newer;
                    }
                    // A look that can't be saved only costs the next launch's choice.
                    prefs.lock().unwrap_or_else(|e| e.into_inner()).set(KEY, &look).ok();
                }
            })
            .ok();
        Saver { tx: Some(tx), thread }
    }

    pub fn save(&self, look: Look) {
        if let Some(tx) = &self.tx {
            tx.send(look).ok();
        }
    }
}

impl Drop for Saver {
    /// Finishes what was queued before the app goes.
    fn drop(&mut self) {
        self.tx.take();
        if let Some(t) = self.thread.take() {
            t.join().ok();
        }
    }
}
