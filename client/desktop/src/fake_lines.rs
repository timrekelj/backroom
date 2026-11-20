//! What the fake people say. Pools are longer than a screen so the same line doesn't show twice
//! in one view.

use crate::fake::{ANA, BRUNO, TIM};

/// The newest #general messages, one of each kind of markup (see the design README).
pub const MARKDOWN_LINES: &[&str] = &[
    "Release notes are up: **v0.0.3** adds a live gateway, ~~polling~~ and *much* better reconnects.",
    "Draft is in [the wiki](https://example.com/wiki/release), the changelog is at https://example.com/changelog.",
    "> staging disk is at 80%\nThat is the first thing to fix. ||it was the old snapshots||",
    "Plan for Friday:\n- cut the release\n- update the `README`\n- tell @cvetka",
    "# Release checklist\n1. tag it\n2. ship it",
    "```sh\ncargo build --release\n```",
    "__underlined__, `inline code` and _italic_ words, all in one line.",
];

/// The `i`th of the 150 seeded #general lines: the pool, except the newest few are markup.
pub fn general_line(i: usize) -> &'static str {
    const TOTAL: usize = 150;
    match i.checked_sub(TOTAL - MARKDOWN_LINES.len()) {
        Some(k) => MARKDOWN_LINES[k],
        None => GENERAL_LINES[(i * 17 + i / 60) % GENERAL_LINES.len()],
    }
}

pub const GENERAL_LINES: &[&str] = &[
    "Morning all.",
    "Staging is back up after the migration.",
    "Pushed the fix for the reconnect loop, review welcome.",
    "On it. Is `backoff_ms` capped?",
    "Yep, capped at 30s. @tim can you check the gateway side?",
    "Here is what I read:\n```rust\nlet delay = (base << attempt).min(30_000);\n```",
    "Looks right to me.",
    "Lunch? The new ramen place on Elm opens at noon.",
    "@ana you coming on Friday?",
    "Count me in, I'll bring the speaker.",
    "Did anyone see the build fail on main?",
    "That was me, sorry. Fixed in `a41c9e2`.",
    "No worries.",
    "Reminder: standup moved to 10:15 tomorrow.",
    "Thanks @bruno, noted.",
    "Anyone tried the new release of the router crate?",
    "Not yet. Changelog says it drops `tokio` 0.2 support, which is fine by us.",
    "```sh\ncargo update -p router\ncargo test --workspace\n```",
    "All green here.",
    "Nice. Merging.",
    "I'm out for an hour, dentist.",
    "Good luck!",
    "The staging database is at 80% disk, I'll clean it up tonight.",
    "Please do. `du -sh /var/lib/scylla/*` showed the old snapshots.",
    "Will do @cvetka.",
    "Movie night Saturday? I can host.",
    "Yes please.",
    "Which one though? Last time we argued for an hour.",
    "Vote in #plans when it exists.",
    "Ha. Fair.",
    "Quick one: does the invite code expire?",
    "No, but the owner can rotate it.",
    "Makes sense.",
    "The deploy script now prints the commit it ships. Small thing but it saved me twice.",
    "Love that.",
    "Coffee machine is broken again, don't trust it.",
    "Finally got the Windows build to link. The trick was a missing `#[link]` attribute.",
    "Wow. Write it down somewhere.",
    "It's in the wiki now, under \"Windows, again\".",
    "Does anyone have the router password for the studio?",
    "It's on the fridge, the yellow note.",
    "Of course it is.",
    "I'm bringing snacks Saturday. Dietary restrictions, speak now.",
    "No pork for me, thanks.",
    "Noted. Vegetarian chilli it is then.",
    "Also, we need a name for the group show, voting opens tonight.",
    "Suggestions go in the thread, no jokes over three words.",
    "That rules out most of mine.",
    "Pushed a tiny fix for the sidebar flicker, nothing to see.",
    "```rust\nlet selected = channels.iter().position(|c| c.id == open);\n```",
    "Neat, that reads better than the loop.",
    "Is the staging box still on the old kernel?",
    "Until Thursday, then I'll reboot it. Heads up.",
    "Thanks for the warning @bruno.",
    "Who's got the camera for the shoot on Sunday?",
    "I do, and two spare batteries.",
    "Perfect. Meet at the pier at ten?",
    "Ten works. I'll bring thermoses.",
    "Photos from last week are uploaded, link in the usual place.",
];

pub const DEV_LINES: &[&str] = &[
    "Gateway backoff now resets on a clean connect.",
    "Can you add a test for the silence timeout?",
    "```rust\nassert_eq!(machine.step(Input::Silence, 0.0), vec![Action::Connect]);\n```",
    "Added, plus one for jitter bounds.",
    "Heartbeat interval is 30s, so the dead-line is 75s.",
    "@tim the `Page<T>` cursor is exclusive, right?",
    "Yes: `before` returns strictly older ids.",
    "Cool, then the client can keep `oldest_id` as is.",
    "Unread counts are client-side for now.",
    "Server-side read state can come later.",
];

pub const DM_ANA_LINES: &[(u64, &str)] = &[
    (ANA, "hey, got a minute?"),
    (TIM, "sure, what's up"),
    (
        ANA,
        "I'm stuck on the invite flow, can you look at it tonight?",
    ),
    (TIM, "yep. I'll ping you when it's pushed"),
    (ANA, "perfect, thank you!"),
];

pub const DM_BRUNO_LINES: &[(u64, &str)] = &[
    (BRUNO, "are we still on for Saturday?"),
    (TIM, "yes, my place at 7"),
    (BRUNO, "I'll bring the good bread"),
];

