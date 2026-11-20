# thehideout design language

`thehideout-design` is the shared design language for the desktop, web and mobile clients: tokens,
a mapping onto `rune_kit::Kit`, and the basic controls. It is platform-neutral (no fs, no windowing).

**Direction.** Cozy and warm, but minimal and crisp, and still tool-like. OpenCode's radii, but warm
greys instead of cool ones and one sage-teal accent. Dark is a dim lamp-lit room (warm near-black, never
`#000`). Light is paper (warm off-white, never `#fff`). Borders separate surfaces; shadows are only
for things that float.

Review it: `client/gallery/render.sh` renders `client/gallery/snapshots/{foundations,controls,icons,themes,chat,chat-dialog}-{dark,light}.png`, or run `cargo run -p thehideout-gallery` for a window with theme, tint and contrast toggles and a Themes page (every accent on every background; press a tile to apply it everywhere).

```rust
let k = thehideout_design::kit(dark);                    // &'static rune_kit::Kit, the default look
let t = thehideout_design::theme(dark);                  // semantic colour tokens (Tone = OKLCH)
Button::new("Send").variant(Variant::Primary).build(k);  // components take the kit

// A user's look: build the kit from a ThemeSpec instead (see Themes below).
let k = thehideout_design::kit_for(&spec, spec.is_dark(system_dark));
let t = thehideout_design::theme_of(k);                  // the theme that kit was built from
```

Apps pass `thehideout_design::{FONTS, FAMILIES}` to `AppSpec` (Instrument Sans + JetBrains Mono, embedded
from `design/fonts/`; see Type).

## Colour (`tokens/color.rs`, `tokens/theme.rs`)

Everything is authored in OKLCH (`Tone { l, c, h, a }`). Components read the semantic names below,
never the ramp.

**Neutral ramp** `N0` (lightest) to `N14`: L = .985 .965 .94 .90 .84 .74 .64 .54 .49 .38 .31 .26 .22 .185 .155,
hue 75 to 55, chroma .006 to .0105.

| Token | Dark | Light | Use |
|---|---|---|---|
| `bg` | `#16120e` | `#f6f3ef` | window |
| `surface` | `#1e1a16` | `#fdfaf6` | panels, cards |
| `surface_raised` | `#28231f` | `#fdfaf6` | inputs, raised controls |
| `surface_overlay` | `#342f2c` | `#fdfaf6` | menus, popovers, tooltips, dialogs |
| `border_subtle` | `#342f2c` | `#e1ddd9` | dividers inside a surface |
| `border` | `#46413e` | `#cecac5` | panel, card, badge edges |
| `border_strong` | `#7b7672` | `#908b87` | input and control edges |
| `text` | `#eeeae6` | `#16120e` | primary text |
| `text_muted` | `#aeaaa6` | `#645f5b` | secondary text |
| `text_faint` | `#908b87` | `#726e6a` | placeholders, hints (still 4.5:1) |

**Accent** (`tokens/accent.rs`; Sage-teal is the default, the other seven are in Themes below). The default's tokens:

| Token | Dark | Light | Use |
|---|---|---|---|
| `accent` | `#68bea8` | `#62b7a1` | fills: primary button, switch/checkbox on, mention pill |
| `accent_fg` | `#051a15` | `#051a15` | ink on `accent` |
| `accent_text` | `#80d0bb` | `#187363` | accent as text/icon on `bg`; also `focus` |
| `accent_subtle` | `#13322a` | `#d1f0e6` | quiet wash behind accent text; the selected state of Home and toggles |

**Status** (each has `x`, `x_bg`, `x_border`; the hues below are Sage-teal's, each accent preset moves the ones that would collide with it): `success` (leaf green h138, livelier than the accent), `warning` (gold h90),
`danger` (red h25, plus `danger_solid` / `on_danger` for destructive buttons), `info` (slate blue h255). Resulting status text: success `#8dcb78` / `#337431`, warning `#e5c057` / `#845a0f`, info `#8ab4ea` / `#446396` (dark / light). The eight avatar and server-badge fills (L .48, same in both appearances): `#8f4244` `#136d5b` `#2c628d` `#456930` `#8b4356` `#1c6878` `#695e1d` `#7a4d67`.
**Overlays**: `hover`, `pressed` (text colour at 5-6% / 9-10%), `accent_hover`, `accent_pressed`
(black 10% / 18%), `scrim`.

Rules: red is for errors only (a mention is accent, not red). Never hard-code a colour in a screen; add a token.
Contrast is enforced by tests (`cargo test -p thehideout-design`): `text`, `text_muted`, `text_faint`,
`accent_text` and status text are at least 4.5:1 on `bg`/`surface`/`surface_raised`; ink on fills at least 4.5:1;
`border` at least 1.3:1, `border_strong` at least 3:1 (non-text) on `bg`, `surface` and `surface_raised` in both themes, `focus` at least 3:1; every token stays in sRGB gamut. The tests run for **every** look (see Themes), including avatar ink, and keep the status hues at least 25 degrees from the accent.

## Themes (`tokens/spec.rs`, `accent.rs`, `background.rs`)

A user's look is one `ThemeSpec`, and nothing else is configurable:

```rust
ThemeSpec {
    accent: AccentPreset,         // 8 presets
    background: BackgroundPreset, // 5 presets
    appearance: Appearance,       // System | Light | Dark, resolved to `dark: bool` with the system's preference
    tint: bool,                   // surfaces and borders pick up a faint accent hue
    contrast: Contrast,           // Standard | High
}
```

`theme_for(&spec, dark)` and `kit_for(&spec, dark)` return a `&'static Theme` / `&'static Kit`, built once per distinct look and cached (at most `ThemeSpec::LOOKS` = 8 x 5 x 2 x 2 x 2 = 320 each, so the cache is bounded; `appearance` only decides `dark`). `theme(dark)` and `kit(dark)` are the default spec's. **Components keep the same call shape**: they take `&Kit` and call `theme_of(k)`, which returns the theme that kit was built from (a lookup by the kit's address, kept in `kit.rs`), so one kit carries the whole look through the tree and nothing is set globally (there is no thread-local palette any more). An app builds the kit once per frame from its spec; `ThemePreview` builds a miniature of any spec for settings tiles and the gallery. `ThemeSpec` is serde-serialisable (kebab-case strings, missing fields default) for apps that save it.

**Presets only.** There is no colour picker, and there will not be: every accent x background x appearance x tint x contrast combination is checked by the contrast tests, and a free colour could not be. Add a preset, and the tests cover it for all the rest.

**Accent presets.** Each defines the fill, its ink, text, subtle wash and (through `accent_text`) the focus colour for dark and light, the status hues that move off it, and eight avatar / server-badge fills. Graphite shares Sage-teal's badges (it has no hue to avoid). Fills (dark / light), Sage-teal first (the default):

| Accent | `accent` dark | `accent` light | `accent_text` dark / light | Status and moved hues (dark) |
|---|---|---|---|---|
| Sage-teal | `#68bea8` | `#62b7a1` | `#80d0bb` / `#187363` | success `#8dcb78` / `#337431` danger `#fb817a` info `#8ab4ea` |
| Moss | `#94ba60` | `#8eb35a` | `#a8cc7a` / `#4e6f24` | success `#70ccb0` / `#0b7462` danger `#fb817a` info `#80b7e7` |
| Petrol | `#52a2b9` | `#4c9bb3` | `#72c1da` / `#07657d` | success `#80cd82` / `#21763c` danger `#fb817a` info `#98afeb` |
| Ocean | `#64aaf0` | `#5da3e9` | `#8cc3fc` / `#215da5` | success `#76cf8a` / `#0b7643` danger `#fb817a` info `#64c2cc` |
| Brass | `#d4b54f` | `#ceae48` | `#e0c262` / `#796006` | success `#76cf8a` / `#0b7643` danger `#fb817a` info `#76bbe2` |
| Ember | `#f28f36` | `#e98a35` | `#f8a052` / `#a84811` | success `#93ca73` / `#3a732c` danger `#fb817a` info `#76bbe2` |
| Coral | `#f08695` | `#ec7e8e` | `#fe9eaa` / `#b2354a` | success `#80cd82` / `#21763c` danger `#f98662` info `#80b7e7` |
| Graphite | `#c7cbd0` | `#2b2e33` | `#c7cbd0` / `#35383d` | success `#80cd82` / `#21763c` danger `#fb817a` info `#85b6e9` |

Ocean is a clear mid blue (h250) with info moved to cyan; Coral is a warm pink-red (h12) with danger moved to a brick-orange red (h38) so errors never read as the accent; Graphite is grey (chroma under .012): dark gets a light-grey fill with dark ink, light a charcoal fill with light ink, and `accent_hover` lightens instead of darkening to match. No preset is purple.

**Background presets.** Each is a 15-step neutral ramp (`Ramp`: lightness per step, chroma and hue drifting down it) for dark and for light; surfaces, borders and text are picked from it by step, exactly as the warm table above. Warm is the default and the original palette.

| Background | Dark | Light |
|---|---|---|
| Warm | bg `#16120e` surface `#1e1a16` raised `#28231f` border `#46413e` | bg `#f6f3ef` surface `#fdfaf6` border `#cecac5` |
| Neutral | bg `#131313` surface `#1b1b1b` raised `#242424` border `#424242` | bg `#f3f3f3` surface `#fafafa` border `#cacaca` |
| Cool | bg `#0f1318` surface `#171b20` raised `#20242a` border `#3e4348` | bg `#eff4f9` surface `#f6fbff` border `#c6cbd1` |
| Deep | bg `#0d0b09` surface `#171413` raised `#201e1c` border `#413f3d` | bg `#f6f3ef` surface `#fdfaf6` border `#cecac5` |
| Paper | bg `#16120e` surface `#1e1a16` raised `#28231f` border `#46413e` | bg `#f4f0e6` surface `#fdf9ef` border `#ccc7bd` |

Neutral is true grey (chroma 0). Cool is slate, hue 250 to 255, chroma .008 to .013. Deep sinks dark to near `#0b0b0b` with a warm hint (light is Warm's). Paper is cream, hue 88 to 70 (dark is Warm's).

**Tint.** With `tint` on, `bg`, the surfaces, the borders and the inset layers move toward the accent hue by .006 of OKLab chroma (borders a little less), lightness untouched. It scales with the accent's own chroma, so Graphite stays grey. A test caps the shift.

**Contrast.** `Contrast::High` raises text and edges against `bg`/`surface`/`surface_raised`: `text` 10:1, `text_muted` 8:1, `text_faint` 7:1, `accent_text` and status text 7:1, `border_subtle` 1.8:1, `border` 3:1, `border_strong` 4.5:1 (Standard: 7, 4.5, 4.5, 4.5, 4.5, 1.1, 1.3, 3). Tokens are authored for the default look; a `fit` step moves only a token whose contrast falls short on the chosen background, tint or level, by the least lightness that clears the bar (and eases chroma back into gamut), so the presets read as designed wherever they already pass and every combination passes by construction.

**Tests** (`tokens/tests.rs`) build all 320 looks once (8 accents x 5 backgrounds x dark/light x tint on/off x Standard/High; `every_combination_is_covered` asserts the count and that `look_index` is a bijection) and check every rule above on each, the preset characters (Neutral is grey, Cool is blue-grey, Deep is under L .16, nothing is purple, Ocean is blue, Coral is pink-red, Graphite is grey), the tint cap, the serde round trip and the caches. They take about 30 ms.

## Type (`tokens/type_.rs`)

Instrument Sans for UI, JetBrains Mono for code (ligatures off). Size / line height, weight.

| Style | Size / LH | Weight | Use |
|---|---|---|---|
| `CAPTION` | 11 / 16 | 450 | timestamps, counts, hints |
| `LABEL` | 12 / 16 | 500 | section labels, chips |
| `CONTROL` / `CONTROL_STRONG` | 13 / 20 | 440 / 530 | text inside controls |
| `BODY` | 14 / 21 | 400 | messages, reading |
| `BODY_STRONG` | 14 / 21 | 600 | names, emphasis |
| `TITLE` | 16 / 24 | 600 | panel and dialog titles |
| `HEADING` | 20 / 28 | 650 | page headings |
| `DISPLAY` | 28 / 36 | 700 | rare, large moments |
| `CODE` | 12.5 / 19 | 400 mono | code, identifiers |

**Fonts.** `Family::Sans` is **Instrument Sans** (variable, weight and width axes) and `Family::Mono` is
**JetBrains Mono** (Regular + Bold). Both are SIL Open Font License 1.1, with the licence texts beside the files:
`client/design/fonts/instrument-sans/` and `client/design/fonts/jetbrains-mono/`. They are embedded with
`include_bytes!` and exposed as `thehideout_design::{FONTS, FAMILIES}`. Code text never uses ligatures
(Rune turns `liga`, `clig`, `calt` and `dlig` off for every mono style, so `->` stays two characters).

## Layout (`components/chat/mod.rs`)

The shell insets the rail, sidebar and panel 8pt from the window's top and bottom, then lines the hairlines up on one y:
`HEADER_H` = 48 for the sidebar title, the channel header and the member panel title; `FOOTER_H` = 64 for the sidebar's
user strip and the composer zone (a multi-line composer grows its zone upward; at rest they match). Every divider uses
`border` (in both themes), so dark and light read equally. Widths: the sidebar is 240 from 1100pt up and 208 below;
the member panel is only offered from 1000pt. In the transcript a group's rows sit 2pt apart and groups ~16pt apart.

## Spacing, radius, elevation, motion

- **Space** (`space.rs`): `S0_5`=2 (hairline only), `S1`=4, `S2`=8, `S3`=12, `S4`=16, `S5`=20, `S6`=24, `S8`=32, `S10`=40, `S12`=48, `S16`=64.
- **Radius** (`radius.rs`), OpenCode's scale exactly: `XS`=2, `SM`=4, `MD`=6, `LG`=8, `XL`=10. `FULL` is for dots and count pills only. Nothing rounder than `XL` otherwise.
- **Elevation** (`elevation.rs`): `Flat` (no shadow, a border), `Popover` (menus, tooltips), `Modal` (dialogs). Two warm-umber shadow levels, subtle in light and a little stronger in dark.
- **Motion** (`motion.rs`), Rune's springs (duration, bounce): `PRESS` .10/0, `CONTROL` .20/0, `OVERLAY` .24/0, `DISCLOSURE` .30/0, `REFLOW` .40/0, `POP` .25/.15 (sparingly).

## Components (`components/`)

Every control takes `&Kit`, and shows `State::{Rest, Hover, Pressed, Focus, Disabled}` through `.state(..)` (forced, for the gallery and tests; at rest the runtime drives hover/press).

| Control | Source | Notes |
|---|---|---|
| `Button` | own | `Variant::{Primary, Secondary, Ghost, Danger}`, `Size::{Sm 24 / radius-sm, Md 32 / radius-md}`, optional `.icon(..)`. One Primary per view. |
| `IconButton` | own | Same variants/sizes, square (24 / 32), a label is required. Ghost by default; `.selected(true)` on a Ghost toggle is the quiet `accent_subtle` fill with an `accent_text` icon (never a bordered box). |
| `Segmented` | own | One-of-N switch: one bordered container on `bg`, 32pt, the selected segment a filled `surface_raised` tile with no border of its own. |
| `TextInput` | `rune_kit` | 28pt (`.large()` = 32pt, the Md control height, used in dialogs and the auth card), `radius-md`, 1px `border_strong` ring; focus swaps it for `focus`; invalid uses `danger`. |
| `switch(on)` | `rune_kit::Switch` `.web()` | 24x16, 4pt corners, accent when on. |
| `Checkbox` | own | 16x16, `radius-sm`, off: `border_strong`; on / mixed: accent fill with `accent_fg` mark. |
| `Avatar` | own | Sizes 20/24/32/40, rounded squares (radius sm/sm/md/lg), initials on one of eight warm fills chosen by FNV-1a of the user id (all take the same ink at 4.5:1+), `Presence::{None, Online, Idle, Offline}` dot (offline is hollow). |
| `Badge` | own | 18pt, `radius-xs`, `Neutral`, `Accent`, `Success`, `Warning`, `Danger`, `Info`. |
| `UnreadDot` / `MentionPill` | own | Unread: quiet 6pt dot in `text`. Mention: accent pill, tabular count, `99+` cap. |
| `Kbd` | own | 18pt keycaps, `radius-xs`, raised fill, 1px border. |
| `tooltip(..)` | `rune_kit::Tooltip` | Standard appearance: overlay surface, 1px border, `radius-sm`, popover shadow, optional `Kbd`. |
| chat components | `components/chat/` | See "Chat components" below. |

## Chat components (`components/chat/`)

Data in, callbacks out: no app state types, every one takes the `Kit` like the controls above. The gallery's
**Chat** page shows each in its forced states, then an interactive assembled preview
(`chat-{dark,light}`, and `chat-dialog-{dark,light}` with the live dialog open).

| Component | For | States |
|---|---|---|
| `GuildBadge`, `RailTile` | The server icon, sizes 24/32/40, avatar radii, deterministic warm fill; `RailTile` is the same tile with an icon on a neutral face (Home, and the outlined "+"). A left rail holds the indicator. | rest, hover (short bar), selected (tall accent bar; `RailTile` adds an `accent_subtle` face), unread (4pt dot), mention count on the corner (overhangs the face 2pt; the rail gap is 12) |
| `NavRow`, `ChannelRow` | Sidebar rows, 32pt (`ROW_H`), 2pt apart, 8pt padding, 16pt icon, `radius-md`. Nav rows take an icon + its fill twin (fill only when selected); `NavRow::person` shows the user's 20pt `Avatar` instead; channels show `#` or their own Phosphor icon (`.icon`, always regular), tinted by a palette colour (`.tint`; the selected highlight takes a 12% wash of it), and a `.trailing` "more" button that fades in on hover. | selected (highlight + strong text), unread (brighter, heavier, dot), mentions (pill), muted (faint), hover |
| `SidebarSection` | A collapsible group header (28pt) with a chevron, a hover "+" and an optional hover "more" button (`.more`); `.decorate_header` lets the app add a context menu or a drag to the title. | open, closed, add shown |
| `MessageGroup`, `ChatMessage` | Avatar, author, time, one or more bodies. `MessageGroup::runs` splits a transcript: same author, at most 5 minutes (`GROUP_WINDOW_SECS`) apart. Follow-ups show their time in the gutter on hover. Bodies are markup (see "Message markup"). `links::<S>()` makes links and spoilers pressable. | `Sent`, `Pending` (body at 0.55, no spinner), `Failed` ("Failed to send · Retry"), mentions you (`accent_wash` + 2pt accent bar), hover (row highlight + floating reply/copy/more) |
| `DateDivider`, `NewMessagesDivider` | A hairline with the date; an accent line with a "New" tag. | static |
| `Composer` | The message box: "+", a field that grows to 8 lines then scrolls, smiley, Send. `bind` wires the field and send; Enter sends, Shift+Enter breaks the line; whitespace never sends. | empty (Send ghost, disabled), text (Send primary), focused (focus ring), disabled with a reason |
| `ChannelHeader` | `#` + name + topic + trailing IconButtons; `breadcrumb` gives "Guild / # channel". | static |
| `MemberRow` | Avatar with presence, name, crown for the owner. | online, idle, offline (faint), hover |
| `EmptyState` | Icon tile, title, body, optional action `Button`. | static |
| `PickerTabs`, `IconGrid`, `ColorSwatches` | The channel look pickers: tabs (Icons, Emoji and Server emoji disabled with "Soon"), the Phosphor catalog as a virtualized 10-column grid (only rows near the viewport are built; a `highlight` for keyboard choice, the search field keeps focus), and the 12 palette swatches plus "none" (selected: ring and check). | rest, hover, selected, highlighted |
| `Dialog` | Modal: scrim, panel (24 padding; title to body 16, label to field 6, field to field 16, body to footer 24), title, body, right-aligned footer. Rune's modal stack gives the focus trap, Esc, scrim click and focus restore. `preview()` draws the panel inline. | closed, open |
| `Toast` | Bottom-centre notice, info / success / danger. `Toast::host` is its inert overlay layer; the caller dismisses. | kinds |
| `ConnectionPill` | Connected, Connecting…, Reconnecting in Ns, Offline. | status |
| `dialog_field` | A labelled field that spans the dialog body, with an inline error under it. | valid, error |

The one new token for these is `accent_wash` (accent at 7% dark / 8% light) behind a message that mentions you;
a test keeps `text`, `text_muted` and `accent_text` at 4.5:1 on it over `bg` and `surface`. Timestamps inside a washed
message therefore use `text_muted`, not `text_faint`.

### Message markup (`chat/markdown.rs`, drawn by `chat/body.rs`)

A Discord-flavoured CommonMark subset, parsed by our own small parser rather than `rune_kit::Markdown`: that is plain
CommonMark (no spoilers, mentions or emoji tokens, `__x__` means bold where Discord underlines), builds whole documents
and cannot be composed with the mention chips, so bending it costs more than a small linear parser with no extra
dependency. Everything is drawn as spans on one wrapping Rune text per paragraph. The composer sends the raw text
unchanged (no live preview). Selecting message text copies it as drawn (markers stripped; a hidden spoiler is left out).

| Write | Get |
|---|---|
| `**bold**`, `*italic*` or `_italic_`, `***both***`, `~~strike~~` | weight 600, italic, both, struck through |
| `__underline__` | underline (Discord's meaning; `___x___` is underline + italic) |
| `` `code` ``, ``` ``a ` b`` ``` | inline mono chip; nothing inside is parsed (no mentions, links or markup) |
| ```` ```rust ```` fence ```` ``` ````, or ` ```one line``` ` | mono block, `radius-md`; syntax-coloured when Rune knows the language (`rune_kit::syntax`), plain otherwise; an unclosed fence runs to the end |
| `> quote`, `>>> quote to the end` | 3pt `border_strong` bar, content indented; holds any block |
| `- item`, `* item`, `1. item`; two-space indent nests | bullets (`•`, then `◦`) or numbers (the first number sets the start) |
| `#`, `##`, `###` + space at line start | 16/15/14pt, semibold; `####` and a mid-line `#` stay text |
| `\|\|spoiler\|\|` | a block in `border` the text colour hides; click reveals it (state per message, local to the UI, not app state); a link inside stays inert until revealed |
| `[text](https://url)`, `https://url`, `<https://url>` | `accent_text`, underline on hover, hand cursor; click opens the default browser. A masked link shows its URL in a tooltip. Trailing punctuation, `*`, `_`, `~`, `\|` and an unbalanced `)` are not part of a bare URL |
| `@name` | the mention chip (`accent_subtle`); the row's `accent_wash` still comes from `mentions_me`. Not parsed in code; works inside bold, quotes, lists and spoilers |
| `<:name:id>`, `<a:name:id>` | tokenised as an emoji mark; drawn as `:name:` text until emoji render |
| `\*`, `\@`, `\<`, any `\` + ASCII punctuation | the character itself |

Markers pair only when they flank text (`2 * 3 * 4` is text; `snake__case__word` is text) and an unmatched marker
stays literal. No HTML, no images, no tables, no setext headings.

**Safety.** Only `http://`, `https://` and `mailto:` become links, matched case-insensitively. Anything else
(`javascript:`, `file:`, `data:`, `//host`) is not a link and the whole `[label](url)` shows as typed, so a label can
never hide a scheme. Destinations and bare URLs are at most 2048 bytes with no whitespace or control characters; Rune's
`Tx::open_url` re-checks the same list before opening anything.

**Limits.** Parsing is linear: block nesting (quotes, lists) stops at 4 levels (deeper lines are plain text), inline
formatting at 16 open markers, at most 4096 marks per text, and only the first 64 KiB of a message is parsed (the rest
is one plain paragraph). Tests drive 60,000-character pathological inputs.

`body::parse` is the flat view the app uses to find mentions (`Block::Text` / `Block::Code`, mention and code marks only).

### Motion

Rules: an action that changes something plays one short, non-bouncing motion; the thing that moved is the thing that
changed. Springs come from `motion::*`; only the mention count's entrance uses `POP` (a mention is the one thing that asks for you; reserve it otherwise for reactions). Nothing animates on the
first frame, and a commit that no input caused (a poll, data arriving) lands at once: Rune's own rule, so a transcript
that loads or reconnects never plays an entrance. Reduced motion is Rune's: travel snaps and opacity/colour take a
0.15s fade.

| Action | Property | Token |
|---|---|---|
| New message appended | opacity 0 to 1 and 6pt rise (that row only) | `motion-control` |
| Pending to sent | body opacity 0.55 to 1 (nothing else) | `motion-control` |
| Selection moves between sidebar rows | the highlight travels (matched element: position, size) | `motion-reflow` |
| Row hover, press | overlay fill | Rune's hover/press tracks |
| Row text/icon changes (selected, unread, muted) | colour | `motion-control` |
| Section collapses or expands | chevron rotates a quarter | `motion-disclosure` |
| | rows fade, what is below moves to make room | `motion-disclosure` |
| Mention count appears (sidebar row, guild badge) | opacity and scale 0.6 to 1, with a small bounce | `motion-pop` |
| Hover a section header, a message, a guild badge | "+", action bar, gutter time, rail bar fade in | hover track |
| Guild badge state | rail bar height and colour | `motion-control` |
| Send becomes available / unavailable | fill colour (primary to ghost) | `motion-control` |
| Send pressed | scale to 0.96 and back, press response | `motion-press` (Rune's press spring) |
| Message sent | the field clears in the same commit, no animation | none |
| Dialog opens or closes | scrim opacity; panel opacity and scale 0.98 to 1 | `motion-overlay` |
| Toast shown or dismissed | opacity and 8pt rise; fade out | `motion-overlay` in, `motion-control` out |
| Connection status changes | pill fill, border, dot and label colour | `motion-control` |

Never animates: date and "New" dividers, channel header, empty states, member rows, avatars, the `Failed` line,
anything inside an existing message when it is edited, the order of a list, whole lists staggering in, scroll position
(the transcript sticks to the bottom). No spinners, flying bubbles or shake on failure: the state is the text.

Two Rune facts worth knowing when you compose these. A child that mounts inside a parent that is also mounting rides
the parent's entrance and plays none of its own, so put a transition on the outermost element that appears (the
`Dialog` puts its scale on the layer, not the panel; message groups carry `Transition::none()` and their rows carry
the entrance). And keep a message's `id` stable from pending to sent (a client nonce), or the row remounts and plays
the entrance again.

## Icons (`icons/`, `tokens/icon.rs`)

**Phosphor Icons** (`@phosphor-icons/core` 2.1.1, MIT, Copyright (c) 2023 Phosphor Icons; licence text in
`src/icons/LICENSES/phosphor-MIT.txt`). Generated Rust, no runtime dependency.

- **Weights.** Regular is the default: `icons::HASH`. Fill is only the active/selected state of nav-style icons
  (house, bell, chat, users, gear and the like): `icons::fill::HOUSE`. Never use fill for `hash`; Phosphor's
  hash-fill is a solid square. A selected channel is shown by the row highlight and heavier text, with the regular hash.
- **Sizes** (`thehideout_design::icon`): `XS`=14 (inside 24pt controls, 0.875pt stroke), `SM`=16 (default, 1pt stroke),
  `MD`=20 (standalone, toolbars). Button and IconButton use `XS` at Sm and `SM` at Md. Icons take the ink of their
  label, or `icon_muted` in a Ghost IconButton.
- **Add an icon.** Append `("our-name", "phosphor-file-stem")` to the `ICONS` list at the top of `tools/gen_icons.py`
  and rerun it from `client/design` (the package is read as data, never executed):

  ```sh
  python3 -I tools/gen_icons.py --phosphor /path/to/@phosphor-icons/core --out src/icons
  ```

  It writes `src/icons/regular.rs` and `src/icons/fill.rs` (both weights for every entry, each with an `ALL` list).
  Names are kebab-case in `Icon::name` and `SCREAMING_SNAKE` as the const (`chevron-up-down` is `CHEVRON_UP_DOWN`).
  The gallery's Icons page lists the catalog from `icons::ALL`. The same run also writes the full catalog
  (`src/icons/catalog/part_N.rs`, tags read from the package's `dist/index.mjs` as data); run `rustfmt --edition 2024`
  over `src/icons` afterwards.

### Channel palette and the full catalog

**Palette** (`tokens/palette.rs`): `palette_color(key, theme) -> Option<Tone>` maps the 12 wire keys
(`PALETTE_KEYS`: slate red orange amber yellow lime green teal cyan blue indigo pink; `""` = none) to icon ink.
One lightness per appearance (dark L .76, light L .53) and a chroma and hue per key, so the twelve read as a
family and contrast is a property of L: every tone has at least **3:1** (non-text) on `bg`, `surface`,
`surface_raised` and under the selected-row wash in **all 320 looks**, stays in gamut, and is distinct from
the others (`palette_tests`). Use it only for icon ink and swatches, never for text.

**Full catalog** (`icons::catalog`): all 1512 Phosphor regular icons with their search tags, generated into
`src/icons/catalog/part_{0..3}.rs` (sorted by name; ~860 KB of source; the release desktop binary grew from 37.06 MB to 40.40 MB, +3.3 MB, for the SVG paths and tags).
`by_name(&str) -> Option<&'static Icon>` is what a channel's stored icon name resolves through (unknown name:
fall back to `#`); `entry`, `all()` and `search(query)` (every word must match the name or a tag; exact name,
name prefix, name substring, then tag-only) serve the picker. The curated `icons::*` constants stay the app's own
chrome.

## Kit mapping (`kit.rs`)

`kit_for(&spec, dark)` (`kit(dark)` for the default) starts from `Kit::from_palette_focus`, then sets every colour, type, radius and motion field
from the tokens, flattens button/switch/knob elevation, gives inputs the strong ring, and sets the floating
shadows to `Popover`/`Modal`. Any rune-kit component (Select, Menu, Dialog, ...) added later inherits the look.

## Known limits

- Kit `Kit::blend_over` is set for Rune's default sRGB blending; a window that opts into linear-light blending would need the kit built through `rune::theme::with_blend`.
- `light.surface == surface_raised == surface_overlay` on purpose: in paper mode, rank comes from border and shadow, not tone.
