//! Dashboard pieces: `StatCard` (a label over a big tabular number that rolls when it changes),
//! `StackedBar` (one bar split by weight, for a total shown by category) and `ActivityChart`
//! (daily bars with a line over them, hover a day for its numbers). Data in, callbacks out; no
//! app types. Cards sit on `bg` against the `surface` panel: a fill step, no border.

use std::rc::Rc;

use rune::{Color, El, Shape, TextStyle, canvas, col, el, row, text};
use rune_kit::{Kit, RollingText};

use crate::theme_of;
use crate::tokens::{radius, type_};

/// `1234567` as "1,234,567".
pub fn group_digits(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// An axis label: `950`, `1.2k`, `15k`, `2.4M`.
pub fn compact(n: u64) -> String {
    let short = |v: f64, unit: &str| {
        if v >= 10.0 {
            format!("{v:.0}{unit}")
        } else {
            format!("{:.1}{unit}", (v * 10.0).round() / 10.0).replace(".0", "")
        }
    };
    match n {
        0..=999 => n.to_string(),
        1_000..=999_999 => short(n as f64 / 1e3, "k"),
        _ => short(n as f64 / 1e6, "M"),
    }
}

/// The smallest 1, 2, 5 x 10^k at or above `max` (an axis top); at least 1.
pub fn nice_max(max: u64) -> u64 {
    let mut step = 1u64;
    loop {
        for m in [1, 2, 5] {
            if step * m >= max {
                return step * m;
            }
        }
        step *= 10;
    }
}

/// Figures that keep their columns: digits of one width.
const BIG: TextStyle = TextStyle {
    size: 24.0,
    line_height: 32.0,
    weight: 650.0,
    tracking: -0.3,
    features: "'tnum' 1",
    ..type_::HEADING
};

// ---- stat card -------------------------------------------------------------------------------

pub struct StatCard {
    key: String,
    label: String,
    value: Option<u64>,
}

impl StatCard {
    /// `key` names the card (and its rolling number); `value` is `None` while loading.
    pub fn new(key: impl Into<String>, label: impl Into<String>, value: Option<u64>) -> StatCard {
        StatCard {
            key: key.into(),
            label: label.into(),
            value,
        }
    }

    pub fn build(self, k: &Kit) -> El {
        let t = theme_of(k);
        let shown = self
            .value
            .map_or_else(|| "\u{2013}".to_owned(), group_digits);
        let mut number = RollingText::new(format!("{}-value", self.key), shown.clone())
            .font(BIG)
            .color(
                if self.value.is_some() {
                    t.text
                } else {
                    t.text_faint
                }
                .color(),
            );
        if let Some(v) = self.value {
            number = number.value(v as f64);
        }
        col()
            .key(self.key)
            .role("group")
            .label(format!("{}: {}", self.label, shown))
            .grow(1.0)
            .shrink()
            .min_w(0.0)
            .padding(16.0)
            .gap(4.0)
            .radius(radius::LG)
            .fill(t.bg.color())
            .child(
                text(self.label)
                    .font(type_::at(k, type_::CONTROL))
                    .color(t.text_muted.color())
                    .max_lines(1)
                    .a11y_hidden(),
            )
            .child(number.build(k))
    }
}

// ---- stacked bar -----------------------------------------------------------------------------

pub struct BarSegment {
    /// Names the segment's element (`<bar key>-<key>`).
    pub key: String,
    pub label: String,
    pub weight: u64,
    pub color: Color,
}

pub struct StackedBar {
    key: String,
    segments: Vec<BarSegment>,
    height: f32,
}

impl StackedBar {
    pub fn new(key: impl Into<String>, segments: Vec<BarSegment>) -> StackedBar {
        StackedBar {
            key: key.into(),
            segments,
            height: 10.0,
        }
    }

    pub fn height(mut self, h: f32) -> Self {
        self.height = h;
        self
    }

    /// Fills its parent's width; each segment's width is its share of the total (a thin sliver
    /// keeps 2pt so it can still be seen). With nothing stored the track is an empty wash.
    pub fn build(self, k: &Kit) -> El {
        let t = theme_of(k);
        let total: u64 = self.segments.iter().map(|s| s.weight).sum();
        let summary = self
            .segments
            .iter()
            .filter(|s| s.weight > 0)
            .map(|s| s.label.clone())
            .collect::<Vec<_>>()
            .join(", ");
        let mut bar = row()
            .key(self.key.clone())
            .role("img")
            .label(summary)
            .w_full()
            .h(self.height)
            .gap(2.0)
            .radius(radius::SM)
            .clip_rounded(radius::SM);
        if total == 0 {
            return bar.fill(t.hover.color());
        }
        for s in self.segments.into_iter().filter(|s| s.weight > 0) {
            bar = bar.child(
                el().key(format!("{}-{}", self.key, s.key))
                    .grow(s.weight as f32)
                    .shrink()
                    .min_w(2.0)
                    .h_full()
                    .fill(s.color),
            );
        }
        bar
    }
}

// ---- activity chart --------------------------------------------------------------------------

/// One day on the chart.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChartDay {
    /// The axis label ("Nov 12").
    pub label: String,
    /// The tooltip title ("Thu, Nov 12").
    pub title: String,
    pub messages: u64,
    pub active_users: u64,
    pub signups: u64,
}

type Hover = Rc<dyn Fn(El, usize) -> El>;

pub struct ActivityChart {
    key: String,
    days: Vec<ChartDay>,
    hover: Option<usize>,
    height: f32,
    on_hover: Option<Hover>,
}

const AXIS_W: f32 = 36.0;
/// The shape's own units: one day is `PITCH` wide, the plot `VIEW_H` tall.
const PITCH: f32 = 10.0;
const VIEW_H: f32 = 100.0;
const TIP_W: f32 = 164.0;

impl ActivityChart {
    pub fn new(key: impl Into<String>, days: Vec<ChartDay>) -> ActivityChart {
        ActivityChart {
            key: key.into(),
            days,
            hover: None,
            height: 200.0,
            on_hover: None,
        }
    }

    /// The day under the pointer (its numbers show in a tooltip).
    pub fn hover(mut self, day: Option<usize>) -> Self {
        self.hover = day;
        self
    }

    pub fn height(mut self, h: f32) -> Self {
        self.height = h;
        self
    }

    /// Called as the pointer enters (`true`) or leaves (`false`) a day's column.
    pub fn on_hover<S: 'static>(mut self, f: impl Fn(&mut S, usize, bool) + 'static) -> Self {
        let f = Rc::new(f);
        self.on_hover = Some(Rc::new(move |e, i| {
            let f = f.clone();
            e.on_hover_change(move |s: &mut S, over, _tx| f(s, i, over))
        }));
        self
    }

    /// Fills its parent's width. Bars are messages (left axis), the line is active users (right
    /// axis, in the info colour); an empty list draws only the empty axes.
    pub fn build(self, k: &Kit) -> El {
        let t = theme_of(k);
        let n = self.days.len();
        let msg_top = nice_max(self.days.iter().map(|d| d.messages).max().unwrap_or(0));
        let usr_top = nice_max(self.days.iter().map(|d| d.active_users).max().unwrap_or(0));
        let y = |v: u64, top: u64| VIEW_H - (v as f32 / top as f32) * (VIEW_H - 2.0);

        let mut shape = Shape::new().view_box((n.max(1) as f32) * PITCH, VIEW_H);
        let mut line = Vec::with_capacity(n);
        for (i, d) in self.days.iter().enumerate() {
            let x = i as f32 * PITCH;
            let (a, b) = (x + 1.5, x + PITCH - 1.5);
            let on = self.hover == Some(i);
            let ink = if on {
                t.accent.color()
            } else {
                t.accent.color().alpha(0.55)
            };
            let top = y(d.messages, msg_top);
            shape = shape.fill(
                vec![
                    [a, VIEW_H],
                    [a, top.min(VIEW_H - 1.0)],
                    [b, top.min(VIEW_H - 1.0)],
                    [b, VIEW_H],
                ],
                ink,
            );
            line.push([x + PITCH / 2.0, y(d.active_users, usr_top)]);
        }
        shape = shape.stroke(line, 2.0, t.info.color());
        let plot = canvas(shape)
            .key(format!("{}-plot", self.key))
            .w_full()
            .h_full();

        // Faint guides at the quarter marks: fills, not rules.
        let mut guides = col()
            .w_full()
            .h_full()
            .justify_between()
            .inert()
            .a11y_hidden();
        for i in 0..5 {
            guides = guides.child(
                el().key(format!("guide-{i}"))
                    .w_full()
                    .h(1.0)
                    .fill(if i == 4 { t.border_subtle } else { t.hover }.color()),
            );
        }

        let mut hits = row().w_full().h_full();
        for (i, d) in self.days.iter().enumerate() {
            let mut column = col()
                .key(format!("{}-day-{i}", self.key))
                .role("img")
                .label(format!(
                    "{}: {} messages, {} active users",
                    d.title, d.messages, d.active_users
                ))
                .grow(1.0)
                .shrink()
                .min_w(0.0)
                .h_full();
            if let Some(h) = &self.on_hover {
                column = h(column, i);
            }
            if self.hover == Some(i) {
                column = column.fill(t.hover.color());
                column = column.child(self.tooltip(k, i, n));
            }
            hits = hits.child(column);
        }

        let scale = |top: u64, ink: Color| {
            let mut c = col().h_full().justify_between().a11y_hidden();
            for v in [top, top * 3 / 4, top / 2, top / 4, 0] {
                c = c.child(text(compact(v)).font(type_::at(k, type_::CAPTION)).color(ink));
            }
            c
        };
        let plot_area = el()
            .key(format!("{}-area", self.key))
            .grow(1.0)
            .shrink()
            .min_w(0.0)
            .h_full()
            .child(el().absolute(0.0, 0.0).w_full().h_full().child(guides))
            .child(el().absolute(0.0, 0.0).w_full().h_full().child(plot))
            .child(el().absolute(0.0, 0.0).w_full().h_full().child(hits));
        let body = row()
            .w_full()
            .h(self.height)
            .gap(8.0)
            .child(scale(msg_top, t.text_faint.color()).w(AXIS_W).items_end())
            .child(plot_area)
            .child(scale(usr_top, t.info.color()).w(AXIS_W));

        // Five dates under the plot, not one per bar.
        let mut dates = row()
            .w_full()
            .justify_between()
            .px(AXIS_W + 8.0)
            .a11y_hidden();
        if n > 0 {
            for i in 0..5usize {
                let at = if n == 1 { 0 } else { i * (n - 1) / 4 };
                dates = dates.child(
                    text(self.days[at].label.clone())
                        .font(type_::at(k, type_::CAPTION))
                        .color(t.text_muted.color()),
                );
            }
        }
        col()
            .key(self.key)
            .role("group")
            .label("Activity")
            .w_full()
            .gap(8.0)
            .child(body)
            .child(dates)
    }

    /// The card over a hovered day: its date and the three figures. It hangs from the column
    /// towards the middle of the plot so it never leaves it.
    fn tooltip(&self, k: &Kit, i: usize, n: usize) -> El {
        let t = theme_of(k);
        let d = &self.days[i];
        let line = |label: &str, value: u64, dot: Color| {
            row()
                .w_full()
                .gap(8.0)
                .center_y()
                .child(el().size(8.0, 8.0).radius(radius::FULL).fill(dot))
                .child(
                    text(label.to_owned())
                        .font(type_::at(k, type_::CONTROL))
                        .color(t.text_muted.color())
                        .grow(1.0),
                )
                .child(
                    text(group_digits(value))
                        .font(TextStyle {
                            features: "'tnum' 1",
                            ..type_::CONTROL_STRONG
                        })
                        .color(t.text.color()),
                )
        };
        let card = col()
            .key(format!("{}-tip", self.key))
            .role("tooltip")
            .w(TIP_W)
            .padding(10.0)
            .gap(4.0)
            .radius(radius::MD)
            .fill(t.surface_overlay.color())
            .border(1.0, t.border_subtle.color())
            .raise()
            .inert()
            .child(
                text(d.title.clone())
                    .font(type_::at(k, type_::CONTROL_STRONG))
                    .color(t.text.color()),
            )
            .child(line("Messages", d.messages, t.accent.color()))
            .child(line("Active users", d.active_users, t.info.color()))
            .child(line("Sign-ups", d.signups, t.text_faint.color()));
        if i * 2 < n {
            card.pin(Some(0.0), Some(8.0), None, None)
        } else {
            card.pin(None, Some(8.0), Some(0.0), None)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn digits_group_and_axes_compact() {
        assert_eq!(group_digits(0), "0");
        assert_eq!(group_digits(999), "999");
        assert_eq!(group_digits(1_000), "1,000");
        assert_eq!(group_digits(1_234_567), "1,234,567");
        assert_eq!(compact(950), "950");
        assert_eq!(compact(1_200), "1.2k");
        assert_eq!(compact(2_000), "2k");
        assert_eq!(compact(15_000), "15k");
        assert_eq!(compact(2_400_000), "2.4M");
    }

    #[test]
    fn axis_tops_are_one_two_or_five() {
        assert_eq!(nice_max(0), 1);
        assert_eq!(nice_max(3), 5);
        assert_eq!(nice_max(6), 10);
        assert_eq!(nice_max(1_130), 2_000);
        assert_eq!(nice_max(5_000), 5_000);
        assert_eq!(nice_max(5_001), 10_000);
    }
}
