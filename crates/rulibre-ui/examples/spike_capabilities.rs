//! Disposable item №2 lab. See docs/UI_SPIKE.md; not part of the shipped UI.
use std::path::PathBuf;
use std::time::Instant;

use iced::widget::{button, column, container, image, row, scrollable, svg, text};
use iced::{Color, Element, Event, Fill, Subscription, Task, Theme, keyboard, window};

const CYRILLIC: &str = "Математика. Утрата определённости";
const POLISH: &str = "Harry Potter i kamień filozoficzny";
const ICON: &[u8] = include_bytes!("assets/book-open.svg");

#[derive(Debug, Clone, Copy)]
enum Effect {
    Off,
    Mica,
    Acrylic,
}

#[derive(Debug, Clone)]
enum Message {
    Frame(Instant),
    Modifiers(keyboard::Modifiers),
    Unfocused,
    Click,
    Effect(Effect),
    Applied(Effect, Result<(), String>),
    Screenshot(window::Screenshot, String),
}

struct State {
    covers: Vec<image::Handle>,
    modifiers: keyboard::Modifiers,
    click: String,
    effect: Effect,
    effect_result: String,
    frame: usize,
    previous: Option<Instant>,
    intervals: Vec<f64>,
    output: Option<PathBuf>,
}

fn main() -> iced::Result {
    env_logger::init();
    iced::application(boot, update, view)
        .title("Rulibre · item 2 capability spike")
        .window(window::Settings {
            size: iced::Size::new(1100.0, 700.0),
            min_size: Some(iced::Size::new(760.0, 520.0)),
            transparent: true,
            ..Default::default()
        })
        .theme(Theme::Dark)
        .style(|_, theme| iced::theme::Style {
            background_color: Color::TRANSPARENT,
            text_color: theme.extended_palette().background.base.text,
        })
        .subscription(subscription)
        .run()
}

fn boot() -> (State, Task<Message>) {
    // --measure DIR runs four phases, saves renderer screenshots, then exits.
    let output = std::env::args().skip(1).collect::<Vec<_>>();
    let output = match output.as_slice() {
        [] => None,
        [flag, path] if flag == "--measure" => Some(PathBuf::from(path)),
        _ => panic!("usage: spike_capabilities [--measure OUTPUT_DIRECTORY]"),
    };
    if let Some(path) = &output {
        std::fs::create_dir_all(path).expect("create measurement directory");
    }
    // Synthetic fixtures, each with distinct pixels. Never touches a Calibre library.
    let fixtures: Vec<_> = (0..200_u32)
        .map(|i| {
            let pixels = ::image::RgbImage::from_fn(240, 360, |x, y| {
                ::image::Rgb([
                    ((x + i * 17) % 256) as u8,
                    ((y + i * 31) % 256) as u8,
                    ((x / 8 + y / 8 + i * 7) % 256) as u8,
                ])
            });
            let mut bytes = Vec::new();
            ::image::codecs::jpeg::JpegEncoder::new_with_quality(&mut bytes, 85)
                .encode_image(&pixels)
                .expect("encode fixture");
            bytes
        })
        .collect();
    let encoded_bytes: usize = fixtures.iter().map(Vec::len).sum();
    let start = Instant::now();
    let covers = fixtures
        .iter()
        .map(|bytes| {
            let decoded = ::image::load_from_memory(bytes).expect("decode JPEG");
            image::Handle::from_rgba(240, 360, decoded.into_rgba8().into_raw())
        })
        .collect();
    println!(
        "JPEG count=200 size=240x360 encoded_bytes={encoded_bytes} rgba_bytes={} decode_and_handles_ms={:.3}",
        200 * 240 * 360 * 4,
        start.elapsed().as_secs_f64() * 1000.0
    );
    (
        State {
            covers,
            modifiers: keyboard::Modifiers::default(),
            click: "Click with Ctrl, Shift, or both".into(),
            effect: Effect::Off,
            effect_result: "Waiting for window handle".into(),
            frame: 0,
            previous: None,
            intervals: Vec::new(),
            output,
        },
        effect_task(Effect::Mica),
    )
}

fn subscription(state: &State) -> Subscription<Message> {
    let events = iced::event::listen_with(|event, _, _| match event {
        Event::Keyboard(keyboard::Event::ModifiersChanged(value)) => {
            Some(Message::Modifiers(value))
        }
        Event::Window(window::Event::Unfocused) => Some(Message::Unfocused),
        _ => None,
    });
    if state.output.is_some() {
        Subscription::batch([events, window::frames().map(Message::Frame)])
    } else {
        events
    }
}

fn click_label(modifiers: keyboard::Modifiers) -> String {
    format!(
        "Click: Ctrl={} Shift={}",
        modifiers.control(),
        modifiers.shift()
    )
}

fn effect_task(effect: Effect) -> Task<Message> {
    window::oldest().then(move |id| match id {
        Some(id) => window::run(id, move |handle| apply_effect(handle, effect))
            .map(move |result| Message::Applied(effect, result)),
        None => Task::none(),
    })
}

#[cfg(target_os = "windows")]
fn apply_effect(handle: &dyn window::Window, effect: Effect) -> Result<(), String> {
    // Clear both kinds before switching; switching is tested on the same HWND.
    match window_vibrancy::clear_mica(handle) {
        Ok(()) | Err(window_vibrancy::Error::UnsupportedPlatformVersion(_)) => {}
        Err(error) => return Err(error.to_string()),
    }
    window_vibrancy::clear_acrylic(handle).map_err(|e| e.to_string())?;
    match effect {
        Effect::Off => Ok(()),
        Effect::Mica => window_vibrancy::apply_mica(handle, Some(true)),
        Effect::Acrylic => window_vibrancy::apply_acrylic(handle, Some((20, 22, 28, 150))),
    }
    .map_err(|e| e.to_string())
}

#[cfg(target_os = "macos")]
fn apply_effect(handle: &dyn window::Window, effect: Effect) -> Result<(), String> {
    use window_vibrancy::{NSVisualEffectMaterial, NSVisualEffectState};
    match effect {
        Effect::Off => window_vibrancy::clear_vibrancy(handle).map(|_| ()),
        _ => window_vibrancy::apply_vibrancy(
            handle,
            NSVisualEffectMaterial::HudWindow,
            Some(NSVisualEffectState::Active),
            None,
        ),
    }
    .map_err(|e| e.to_string())
}

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
fn apply_effect(_: &dyn window::Window, _: Effect) -> Result<(), String> {
    Err("OS blur unsupported; using flat translucent fill".into())
}

fn update(state: &mut State, message: Message) -> Task<Message> {
    match message {
        Message::Modifiers(value) => state.modifiers = value,
        Message::Unfocused => state.modifiers = keyboard::Modifiers::default(),
        Message::Click => {
            state.click = click_label(state.modifiers);
            println!("{}", state.click);
        }
        Message::Effect(effect) => return effect_task(effect),
        Message::Applied(effect, result) => {
            state.effect = if result.is_ok() { effect } else { Effect::Off };
            state.effect_result = format!("{effect:?}: {result:?}");
            println!("effect {}", state.effect_result);
        }
        Message::Screenshot(shot, name) => {
            let path = state.output.as_ref().expect("measurement mode").join(name);
            ::image::save_buffer(
                &path,
                &shot.rgba,
                shot.size.width,
                shot.size.height,
                ::image::ColorType::Rgba8,
            )
            .expect("save renderer screenshot");
            println!("screenshot={} scale={}", path.display(), shot.scale_factor);
        }
        Message::Frame(now) => {
            state.frame += 1;
            if let Some(previous) = state.previous.replace(now) {
                // Discard the first 30 frames of each phase (upload/screenshot warmup).
                if state.frame % 120 > 30 {
                    state
                        .intervals
                        .push((now - previous).as_secs_f64() * 1000.0);
                }
            }
            if state.frame % 120 == 60 {
                let name = format!("phase-{}-{:?}.png", state.frame / 120, state.effect);
                return window::oldest().then(move |id| match id {
                    Some(id) => {
                        let name = name.clone();
                        window::screenshot(id)
                            .map(move |shot| Message::Screenshot(shot, name.clone()))
                    }
                    None => Task::none(),
                });
            }
            if state.frame.is_multiple_of(120) {
                state.intervals.sort_by(f64::total_cmp);
                let n = state.intervals.len();
                if n > 0 {
                    println!(
                        "redraw_intervals phase={} effect={:?} n={} mean_ms={:.3} p95_ms={:.3}",
                        state.frame / 120,
                        state.effect,
                        n,
                        state.intervals.iter().sum::<f64>() / n as f64,
                        state.intervals[(n - 1) * 95 / 100]
                    );
                }
                state.intervals.clear();
                return match state.frame {
                    120 | 360 => effect_task(Effect::Off),
                    240 => effect_task(Effect::Acrylic),
                    _ => iced::exit(),
                };
            }
            if state.frame.is_multiple_of(5) {
                let y = (state.frame % 120) as f32 / 120.0;
                return iced::widget::operation::snap_to(
                    "covers",
                    iced::widget::scrollable::RelativeOffset { x: 0.0, y },
                );
            }
        }
    }
    Task::none()
}

fn view(state: &State) -> Element<'_, Message> {
    let icon = svg(svg::Handle::from_memory(ICON))
        .width(28)
        .height(28)
        .style(|theme: &Theme, _| svg::Style {
            color: Some(theme.extended_palette().background.base.text),
        });
    let controls = row![
        icon,
        button("Mica / vibrancy").on_press(Message::Effect(Effect::Mica)),
        button("Acrylic / vibrancy").on_press(Message::Effect(Effect::Acrylic)),
        button("Off").on_press(Message::Effect(Effect::Off)),
        button("Test Ctrl / Shift click").on_press(Message::Click),
    ]
    .spacing(12);
    let mut grid = column![].spacing(12);
    for covers in state.covers.chunks(10) {
        let mut strip = row![].spacing(8);
        for cover in covers {
            strip = strip.push(image(cover.clone()).width(88).height(132));
        }
        grid = grid.push(strip);
    }
    container(
        column![
            controls,
            text(&state.effect_result),
            text(&state.click),
            text(CYRILLIC).size(23).shaping(text::Shaping::Advanced),
            text(POLISH).size(23).shaping(text::Shaping::Advanced),
            text("Mixed: Łódź · Жизнь · 2026 · 日本語 · 😀").shaping(text::Shaping::Advanced),
            row![
                text("Clipped, no built-in ellipsis:"),
                container(text(CYRILLIC).wrapping(text::Wrapping::None)).width(150),
                text("Explicit shortened label:"),
                text("Математика. Утрата…"),
            ]
            .spacing(12),
            scrollable(grid).id("covers").height(Fill),
        ]
        .spacing(12),
    )
    .padding(20)
    .width(Fill)
    .height(Fill)
    .style(|_| container::Style {
        background: Some(Color::from_rgba(0.06, 0.07, 0.09, 0.72).into()),
        ..Default::default()
    })
    .into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn click_reads_current_combination_and_focus_loss_clears_it() {
        for (modifiers, expected) in [
            (
                keyboard::Modifiers::default(),
                "Click: Ctrl=false Shift=false",
            ),
            (keyboard::Modifiers::CTRL, "Click: Ctrl=true Shift=false"),
            (keyboard::Modifiers::SHIFT, "Click: Ctrl=false Shift=true"),
            (
                keyboard::Modifiers::CTRL | keyboard::Modifiers::SHIFT,
                "Click: Ctrl=true Shift=true",
            ),
        ] {
            assert_eq!(click_label(modifiers), expected);
        }
        let mut state = State {
            covers: Vec::new(),
            modifiers: keyboard::Modifiers::CTRL,
            click: String::new(),
            effect: Effect::Off,
            effect_result: String::new(),
            frame: 0,
            previous: None,
            intervals: Vec::new(),
            output: None,
        };
        let _ = update(&mut state, Message::Unfocused);
        let _ = update(&mut state, Message::Click);
        assert_eq!(state.click, "Click: Ctrl=false Shift=false");
    }
}
