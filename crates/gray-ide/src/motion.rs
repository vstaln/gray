//! Motion tokens and small animated pieces for the IDE.
//!
//! Everything here goes through gpui's `with_animation`, which already honors
//! `App::reduce_motion()`: one-shot enters jump to their end state and loops
//! hold their first frame, so callers never need their own flag.

use gpui_kit::base::animation::cubic_bezier;
use gpui_kit::component::shimmer::ShimmerText;
use gpui_kit::component::{Theme, h_flex};
use gpui_kit::*;
use std::f32::consts::TAU;
use std::time::Duration;

/// Enter curve, monocode `--motion-ease-out`.
pub fn ease_out() -> impl Fn(f32) -> f32 {
    cubic_bezier(0.22, 1.0, 0.36, 1.0)
}

/// Snappier curve for popovers and modals.
pub fn ease_pop() -> impl Fn(f32) -> f32 {
    cubic_bezier(0.16, 1.0, 0.3, 1.0)
}

pub const BACKDROP_MS: u64 = 160;
pub const POPOVER_MS: u64 = 170;
pub const MODAL_MS: u64 = 200;
pub const CARD_MS: u64 = 180;
pub const WELCOME_MS: u64 = 600;
/// Message rise-in, monocode `prompt-turn-reveal`.
pub const RISE_MS: u64 = 320;
/// In-app toast auto-dismiss.
pub const TOAST_MS: u64 = 4000;
/// Composer dock-slide on the first send (monocode `useComposerDockMotion`).
pub const DOCK_MS: u64 = 480;
/// Only prompt bubbles this fresh get the celebration burst (monocode FRESH_MS).
pub const FRESH_MS: i64 = 4_000;
/// Loops are capped so a running turn doesn't re-render the window at 60 Hz.
const LOOP_FPS: f32 = 30.;

pub fn ms(n: u64) -> Duration {
    Duration::from_millis(n)
}

/// Raised cosine in `0..=1` that peaks once per cycle at `phase`.
fn wave(x: f32, phase: f32) -> f32 {
    let p = (x - phase).rem_euclid(1.0);
    0.5 + 0.5 * (p * TAU).cos()
}

/// Fade in and slide `travel` px (positive = from above) into place.
pub fn enter<E: Styled + IntoElement + 'static>(
    el: E,
    id: impl Into<ElementId>,
    dur: u64,
    travel: f32,
    easing: impl Fn(f32) -> f32 + 'static,
) -> AnimationElement<E> {
    el.with_animation(
        id,
        Animation::new(ms(dur)).with_easing(easing),
        move |el, t| el.opacity(t).top(px(-travel * (1. - t))),
    )
}

/// Plain opacity fade for backdrops.
pub fn fade_in<E: Styled + IntoElement + 'static>(
    el: E,
    id: impl Into<ElementId>,
    dur: u64,
) -> AnimationElement<E> {
    el.with_animation(
        id,
        Animation::new(ms(dur)).with_easing(ease_out()),
        |el, t| el.opacity(t),
    )
}

/// Eight accent squares with a comet of light running through them.
pub fn churner(t: &Theme) -> impl IntoElement {
    let on = t.primary;
    h_flex().gap_0p5().children((0..8usize).map(move |i| {
        div().size(px(4.)).bg(on).with_animation(
            ("churn", i),
            Animation::new(ms(1100))
                .repeat_synced()
                .with_max_fps(LOOP_FPS),
            move |d, x| {
                let b = wave(x, i as f32 / 8.).powi(3);
                d.opacity(0.18 + 0.82 * b)
            },
        )
    }))
}

/// Three dots bobbing in sequence: the agent is thinking, nothing streamed yet.
pub fn typing_dots(t: &Theme) -> impl IntoElement {
    let c = t.muted_foreground;
    h_flex()
        .h(px(14.))
        .items_center()
        .gap_1()
        .children((0..3usize).map(move |i| {
            div().relative().size(px(5.)).bg(c).with_animation(
                ("typing-dot", i),
                Animation::new(ms(1100))
                    .repeat_synced()
                    .with_max_fps(LOOP_FPS),
                move |d, x| {
                    let b = wave(x, 0.3 + i as f32 * 0.15).powi(2);
                    d.opacity(0.35 + 0.65 * b).top(px(-3. * b))
                },
            )
        }))
}

/// Status word with a light sweep across it.
pub fn shimmer_label(id: &'static str, text: impl Into<SharedString>, t: &Theme) -> ShimmerText {
    ShimmerText::new(text)
        .id(id)
        .duration(Duration::from_secs(2))
        .highlight_color(t.primary)
}

/// Slow opacity breathing (0.35 ↔ 0.9 over 1.8 s) for "waiting" labels.
pub fn breathe<E: Styled + IntoElement + 'static>(
    el: E,
    id: impl Into<ElementId>,
) -> AnimationElement<E> {
    el.with_animation(
        id,
        Animation::new(ms(1800))
            .repeat_synced()
            .with_max_fps(LOOP_FPS),
        |el, x| el.opacity(0.35 + 0.55 * (0.5 - 0.5 * (x * TAU).cos())),
    )
}

/// Three bars that light up in turn while a terminal command runs.
pub fn running_bars(t: &Theme) -> impl IntoElement {
    let c = t.primary;
    h_flex()
        .gap(px(2.))
        .items_end()
        .children((0..3usize).map(move |i| {
            div().w(px(3.)).h(px(8.)).bg(c).with_animation(
                ("term-bar", i),
                Animation::new(ms(1200))
                    .repeat_synced()
                    .with_max_fps(LOOP_FPS),
                move |d, x| {
                    let lit = x >= 0.25 * (i as f32 + 1.);
                    d.opacity(if lit { 0.9 } else { 0.3 })
                },
            )
        }))
}

/// Braille terminal spinner (monocode `TerminalSpinner`): one frame per 80 ms.
pub fn braille_spinner(id: impl Into<ElementId>, t: &Theme) -> impl IntoElement {
    const FRAMES: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
    div()
        .text_size(px(16.))
        .line_height(px(16.))
        .text_color(t.primary)
        .with_animation(
            id,
            Animation::new(ms(800)).repeat().with_max_fps(12.5),
            |d, x| d.child(FRAMES[((x * 10.) as usize).min(9)]),
        )
}

/// Fade in and slide along x (`travel` px; positive = from the right).
pub fn enter_x<E: Styled + IntoElement + 'static>(
    el: E,
    id: impl Into<ElementId>,
    dur: u64,
    travel: f32,
    easing: impl Fn(f32) -> f32 + 'static,
) -> AnimationElement<E> {
    el.with_animation(
        id,
        Animation::new(ms(dur)).with_easing(easing),
        move |el, t| el.opacity(t).left(px(travel * (1. - t))),
    )
}

/// Word-by-word fade for streaming markdown (monocode `wordFade`): each
/// burst's appended text fades in over ~320ms, words ~24ms apart.
pub fn word_fade() -> gpui_kit::base::TextViewMotion {
    use gpui_kit::base::motion::Easing;
    gpui_kit::base::TextViewMotion::default()
        .with_stream_fade(ms(320))
        .with_stream_fade_stagger(ms(24))
        .with_stream_fade_easing(
            Easing::cubic_bezier(0.33, 0.53, 0.67, 1.0).unwrap_or(Easing::EaseOut),
        )
}

/// Halo ring + sheen sweep + ember glow on a just-submitted prompt bubble
/// (monocode turn-mode-halo / turn-mode-sheen / monocode-ember-glow).
/// One-shot overlays keyed by the message id, hugging the bubble not the row.
pub fn celebrate<E: IntoElement + 'static>(bubble: E, id: &str, t: &Theme) -> impl IntoElement {
    let glow = t.primary;
    div()
        .relative()
        .child(bubble)
        .child(div().absolute().inset_0().with_animation(
            SharedString::from(format!("halo-{id}")),
            Animation::new(ms(2400)).with_easing(ease_out()),
            move |d, x| {
                let a = if x < 0.25 {
                    x / 0.25
                } else {
                    1. - (x - 0.25) / 0.75
                };
                d.border_1()
                    .border_color(glow.opacity(0.6 * a))
                    .bg(glow.opacity(0.08 * a))
            },
        ))
        .child(div().absolute().inset_0().overflow_hidden().with_animation(
            SharedString::from(format!("sheen-{id}")),
            Animation::new(ms(1400)),
            move |d, x| {
                let u = ((x * 1400. - 300.) / 1100.).clamp(0., 1.);
                d.child(
                    div()
                        .absolute()
                        .top_0()
                        .bottom_0()
                        .w(relative(0.45))
                        .left(relative(-0.45 + 1.6 * u))
                        .bg(linear_gradient(
                            105.,
                            linear_color_stop(glow.opacity(0.), 0.),
                            linear_color_stop(glow.opacity(0.30), 1.),
                        )),
                )
            },
        ))
        .child(div().absolute().inset_0().with_animation(
            SharedString::from(format!("ember-{id}")),
            Animation::new(ms(2200)).with_easing(ease_out()),
            move |d, x| {
                let u = ((x * 2200. - 100.) / 2100.).clamp(0., 1.);
                let a = if u < 0.3 {
                    u / 0.3
                } else {
                    1. - (u - 0.3) / 0.7
                };
                d.bg(linear_gradient(
                    180.,
                    linear_color_stop(glow.opacity(0.), 0.),
                    linear_color_stop(glow.opacity(0.35 * a), 1.),
                ))
            },
        ))
}
