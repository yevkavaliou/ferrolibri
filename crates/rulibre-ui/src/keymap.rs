//! Keyboard map: key + modifiers + context → command. Pure, so every row of the
//! table is tested without a window.
//!
//! Letter shortcuts follow the logical key when it is a Latin letter and fall back
//! to the physical key otherwise, so they work on Cyrillic and other layouts.
//! Text fields capture typed characters before this map sees them (the app only
//! forwards events a widget ignored, plus Esc), so no text-focus flag is needed.
use iced::keyboard::{
    Key, Modifiers,
    key::{Code, Named, Physical},
};

/// Which kind of content currently receives navigation keys.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pane {
    /// First run or loading: only Esc is handled.
    Setup,
    Library {
        grid: bool,
    },
    Search,
    /// Jobs and Settings.
    Other,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Context {
    pub pane: Pane,
    /// A modal layer is open (export prompt, reader picker): only Esc works.
    pub overlay: bool,
    /// The scan warning list replaces the book list.
    pub warnings: bool,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Motion {
    Up,
    Down,
    Left,
    Right,
    PageUp,
    PageDown,
    Home,
    End,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Command {
    OpenSearch,
    /// `extend` is Shift: grow the selection instead of replacing it.
    Move {
        motion: Motion,
        extend: bool,
    },
    SelectAll,
    /// Enter: open (and focus) the details of the selection.
    OpenDetails,
    Escape,
    Send,
    ToggleConvert,
    ToggleSidebar,
    ToggleDetails,
    /// Tab / Shift+Tab: move between text fields (iced buttons take no focus).
    Focus {
        backwards: bool,
    },
}
/// Shortcuts shown in tooltips; the modifier is Ctrl, or ⌘ on macOS.
pub fn modifier_label() -> &'static str {
    if cfg!(target_os = "macos") {
        "⌘"
    } else {
        "Ctrl+"
    }
}
pub fn hint(command: Command) -> String {
    match command {
        Command::OpenSearch => "/".into(),
        Command::Send => "T".into(),
        Command::ToggleConvert => "C".into(),
        Command::ToggleSidebar => format!("{}B", modifier_label()),
        Command::ToggleDetails => format!("{}I", modifier_label()),
        Command::SelectAll => format!("{}A", modifier_label()),
        Command::OpenDetails => "Enter".into(),
        Command::Escape => "Esc".into(),
        Command::Move { .. } => "↑ ↓".into(),
        Command::Focus { .. } => "Tab".into(),
    }
}
/// `"Label (hint)"`, the form used by every tooltip.
pub fn tip(label: &str, command: Command) -> String {
    format!("{label} ({})", hint(command))
}
fn letter(key: &Key, physical: &Physical, ch: char, code: Code) -> bool {
    match key {
        Key::Character(s) if s.chars().next().is_some_and(|c| c.is_ascii_alphabetic()) => {
            s.chars().count() == 1 && s.eq_ignore_ascii_case(ch.encode_utf8(&mut [0; 4]))
        }
        _ => *physical == Physical::Code(code),
    }
}
fn slash(key: &Key, physical: &Physical, modifiers: Modifiers) -> bool {
    matches!(key, Key::Character(s) if s.as_str() == "/")
        || (!modifiers.shift() && *physical == Physical::Code(Code::Slash))
}
fn motion(named: Named) -> Option<Motion> {
    Some(match named {
        Named::ArrowUp => Motion::Up,
        Named::ArrowDown => Motion::Down,
        Named::ArrowLeft => Motion::Left,
        Named::ArrowRight => Motion::Right,
        Named::PageUp => Motion::PageUp,
        Named::PageDown => Motion::PageDown,
        Named::Home => Motion::Home,
        Named::End => Motion::End,
        _ => return None,
    })
}
pub fn resolve(
    key: &Key,
    physical: &Physical,
    modifiers: Modifiers,
    context: &Context,
) -> Option<Command> {
    let command_key = modifiers.control() || modifiers.logo();
    let plain = !command_key && !modifiers.alt();
    if matches!(key, Key::Named(Named::Escape)) {
        return (!command_key && !modifiers.alt()).then_some(Command::Escape);
    }
    if matches!(key, Key::Named(Named::Tab)) {
        return (plain).then_some(Command::Focus {
            backwards: modifiers.shift(),
        });
    }
    if context.overlay || context.pane == Pane::Setup {
        return None;
    }
    let lists = matches!(context.pane, Pane::Library { .. } | Pane::Search);
    if command_key && !modifiers.alt() && !modifiers.shift() {
        return if letter(key, physical, 'b', Code::KeyB) {
            Some(Command::ToggleSidebar)
        } else if letter(key, physical, 'i', Code::KeyI) {
            matches!(context.pane, Pane::Library { .. }).then_some(Command::ToggleDetails)
        } else if letter(key, physical, 'a', Code::KeyA) {
            (lists && !context.warnings).then_some(Command::SelectAll)
        } else {
            None
        };
    }
    if !plain {
        return None;
    }
    if slash(key, physical, modifiers) {
        return Some(Command::OpenSearch);
    }
    if context.warnings || !lists {
        return None;
    }
    if let Key::Named(named) = key {
        if *named == Named::Enter {
            return (!modifiers.shift()).then_some(Command::OpenDetails);
        }
        return motion(*named).map(|motion| Command::Move {
            motion,
            extend: modifiers.shift(),
        });
    }
    if modifiers.shift() {
        return None;
    }
    if letter(key, physical, 't', Code::KeyT) {
        Some(Command::Send)
    } else if letter(key, physical, 'c', Code::KeyC) {
        matches!(context.pane, Pane::Library { .. }).then_some(Command::ToggleConvert)
    } else {
        None
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn ch(s: &str) -> Key {
        Key::Character(s.into())
    }
    fn named(n: Named) -> Key {
        Key::Named(n)
    }
    fn code(c: Code) -> Physical {
        Physical::Code(c)
    }
    const LIST: Context = Context {
        pane: Pane::Library { grid: false },
        overlay: false,
        warnings: false,
    };
    const GRID: Context = Context {
        pane: Pane::Library { grid: true },
        overlay: false,
        warnings: false,
    };
    const SEARCH: Context = Context {
        pane: Pane::Search,
        overlay: false,
        warnings: false,
    };
    const OTHER: Context = Context {
        pane: Pane::Other,
        overlay: false,
        warnings: false,
    };
    const SETUP: Context = Context {
        pane: Pane::Setup,
        overlay: false,
        warnings: false,
    };
    fn run(
        key: Key,
        physical: Physical,
        modifiers: Modifiers,
        context: &Context,
    ) -> Option<Command> {
        resolve(&key, &physical, modifiers, context)
    }
    fn mv(motion: Motion, extend: bool) -> Option<Command> {
        Some(Command::Move { motion, extend })
    }
    #[test]
    fn plain_letters_and_slash() {
        let none = Modifiers::empty();
        for ctx in [LIST, GRID, SEARCH, OTHER] {
            assert_eq!(
                run(ch("/"), code(Code::Slash), none, &ctx),
                Some(Command::OpenSearch)
            );
        }
        assert_eq!(
            run(ch("t"), code(Code::KeyT), none, &LIST),
            Some(Command::Send)
        );
        assert_eq!(
            run(ch("T"), code(Code::KeyT), none, &GRID),
            Some(Command::Send)
        );
        assert_eq!(
            run(ch("t"), code(Code::KeyT), none, &SEARCH),
            Some(Command::Send)
        );
        assert_eq!(
            run(ch("c"), code(Code::KeyC), none, &LIST),
            Some(Command::ToggleConvert)
        );
        // Convert lives in the details pane, which Search does not have.
        assert_eq!(run(ch("c"), code(Code::KeyC), none, &SEARCH), None);
        assert_eq!(run(ch("t"), code(Code::KeyT), none, &OTHER), None);
        assert_eq!(run(ch("x"), code(Code::KeyX), none, &LIST), None);
    }
    #[test]
    fn letters_work_on_cyrillic_and_other_layouts() {
        let none = Modifiers::empty();
        // Russian layout: the T key types "е", the C key "с", the B key "и".
        assert_eq!(
            run(ch("е"), code(Code::KeyT), none, &LIST),
            Some(Command::Send)
        );
        assert_eq!(
            run(ch("с"), code(Code::KeyC), none, &LIST),
            Some(Command::ToggleConvert)
        );
        assert_eq!(
            run(ch("и"), code(Code::KeyB), Modifiers::CTRL, &LIST),
            Some(Command::ToggleSidebar)
        );
        assert_eq!(
            run(ch("ф"), code(Code::KeyA), Modifiers::CTRL, &LIST),
            Some(Command::SelectAll)
        );
        // The slash key types "." on a Russian layout.
        assert_eq!(
            run(ch("."), code(Code::Slash), none, &LIST),
            Some(Command::OpenSearch)
        );
        // Dvorak: the physical T key types "y"; the logical Latin letter wins.
        assert_eq!(run(ch("y"), code(Code::KeyT), none, &LIST), None);
        assert_eq!(
            run(ch("t"), code(Code::KeyK), none, &LIST),
            Some(Command::Send)
        );
        // "/" behind Shift on a layout where the physical key differs.
        assert_eq!(
            run(ch("/"), code(Code::Digit7), Modifiers::SHIFT, &LIST),
            Some(Command::OpenSearch)
        );
    }
    #[test]
    fn modified_letters_do_not_fire_plain_shortcuts() {
        for m in [
            Modifiers::CTRL,
            Modifiers::ALT,
            Modifiers::LOGO,
            Modifiers::SHIFT,
        ] {
            assert_eq!(run(ch("t"), code(Code::KeyT), m, &LIST), None, "{m:?}");
            assert_eq!(run(ch("c"), code(Code::KeyC), m, &LIST), None, "{m:?}");
        }
        assert_eq!(run(ch("/"), code(Code::Slash), Modifiers::ALT, &LIST), None);
        assert_eq!(
            run(ch("/"), code(Code::Slash), Modifiers::CTRL, &LIST),
            None
        );
    }
    #[test]
    fn command_shortcuts_with_ctrl_or_cmd() {
        for m in [Modifiers::CTRL, Modifiers::LOGO] {
            assert_eq!(
                run(ch("b"), code(Code::KeyB), m, &LIST),
                Some(Command::ToggleSidebar)
            );
            assert_eq!(
                run(ch("B"), code(Code::KeyB), m, &OTHER),
                Some(Command::ToggleSidebar)
            );
            assert_eq!(
                run(ch("i"), code(Code::KeyI), m, &GRID),
                Some(Command::ToggleDetails)
            );
            assert_eq!(
                run(ch("a"), code(Code::KeyA), m, &LIST),
                Some(Command::SelectAll)
            );
            assert_eq!(
                run(ch("a"), code(Code::KeyA), m, &SEARCH),
                Some(Command::SelectAll)
            );
        }
        // No details pane on Search, Jobs, Settings; nothing to select outside lists.
        assert_eq!(
            run(ch("i"), code(Code::KeyI), Modifiers::CTRL, &SEARCH),
            None
        );
        assert_eq!(
            run(ch("i"), code(Code::KeyI), Modifiers::CTRL, &OTHER),
            None
        );
        assert_eq!(
            run(ch("a"), code(Code::KeyA), Modifiers::CTRL, &OTHER),
            None
        );
        // Ctrl+Shift+B and Ctrl+Alt+B are other chords.
        assert_eq!(
            run(
                ch("b"),
                code(Code::KeyB),
                Modifiers::CTRL | Modifiers::SHIFT,
                &LIST
            ),
            None
        );
        assert_eq!(
            run(
                ch("b"),
                code(Code::KeyB),
                Modifiers::CTRL | Modifiers::ALT,
                &LIST
            ),
            None
        );
    }
    #[test]
    fn navigation_keys_and_shift_extension() {
        let table = [
            (Named::ArrowUp, Motion::Up),
            (Named::ArrowDown, Motion::Down),
            (Named::ArrowLeft, Motion::Left),
            (Named::ArrowRight, Motion::Right),
            (Named::PageUp, Motion::PageUp),
            (Named::PageDown, Motion::PageDown),
            (Named::Home, Motion::Home),
            (Named::End, Motion::End),
        ];
        for (key, motion) in table {
            for ctx in [LIST, GRID, SEARCH] {
                assert_eq!(
                    run(named(key), code(Code::Escape), Modifiers::empty(), &ctx),
                    mv(motion, false)
                );
                assert_eq!(
                    run(named(key), code(Code::Escape), Modifiers::SHIFT, &ctx),
                    mv(motion, true)
                );
                // Ctrl/Alt + arrow stay free for the OS and the text fields.
                assert_eq!(
                    run(named(key), code(Code::Escape), Modifiers::CTRL, &ctx),
                    None
                );
                assert_eq!(
                    run(named(key), code(Code::Escape), Modifiers::ALT, &ctx),
                    None
                );
            }
            assert_eq!(
                run(named(key), code(Code::Escape), Modifiers::empty(), &OTHER),
                None
            );
        }
    }
    #[test]
    fn enter_opens_details_only_in_lists() {
        let enter = || named(Named::Enter);
        assert_eq!(
            run(enter(), code(Code::Enter), Modifiers::empty(), &LIST),
            Some(Command::OpenDetails)
        );
        assert_eq!(
            run(enter(), code(Code::Enter), Modifiers::empty(), &SEARCH),
            Some(Command::OpenDetails)
        );
        assert_eq!(
            run(enter(), code(Code::Enter), Modifiers::empty(), &OTHER),
            None
        );
        assert_eq!(
            run(enter(), code(Code::Enter), Modifiers::SHIFT, &LIST),
            None
        );
    }
    #[test]
    fn escape_always_resolves_except_with_command_keys() {
        for ctx in [LIST, GRID, SEARCH, OTHER, SETUP] {
            assert_eq!(
                run(
                    named(Named::Escape),
                    code(Code::Escape),
                    Modifiers::empty(),
                    &ctx
                ),
                Some(Command::Escape)
            );
        }
        let overlay = Context {
            overlay: true,
            ..LIST
        };
        assert_eq!(
            run(
                named(Named::Escape),
                code(Code::Escape),
                Modifiers::empty(),
                &overlay
            ),
            Some(Command::Escape)
        );
        assert_eq!(
            run(
                named(Named::Escape),
                code(Code::Escape),
                Modifiers::CTRL,
                &LIST
            ),
            None
        );
    }
    #[test]
    fn overlay_setup_and_warning_list_block_other_keys() {
        let overlay = Context {
            overlay: true,
            ..LIST
        };
        let warnings = Context {
            warnings: true,
            ..LIST
        };
        let none = Modifiers::empty();
        for ctx in [overlay, SETUP] {
            assert_eq!(run(ch("/"), code(Code::Slash), none, &ctx), None);
            assert_eq!(run(ch("t"), code(Code::KeyT), none, &ctx), None);
            assert_eq!(run(ch("b"), code(Code::KeyB), Modifiers::CTRL, &ctx), None);
            assert_eq!(
                run(named(Named::ArrowDown), code(Code::ArrowDown), none, &ctx),
                None
            );
        }
        // The warning list hides the books, so book keys do nothing; navigation shortcuts work.
        assert_eq!(run(ch("t"), code(Code::KeyT), none, &warnings), None);
        assert_eq!(
            run(
                named(Named::ArrowDown),
                code(Code::ArrowDown),
                none,
                &warnings
            ),
            None
        );
        assert_eq!(
            run(ch("a"), code(Code::KeyA), Modifiers::CTRL, &warnings),
            None
        );
        assert_eq!(
            run(ch("/"), code(Code::Slash), none, &warnings),
            Some(Command::OpenSearch)
        );
        assert_eq!(
            run(ch("b"), code(Code::KeyB), Modifiers::CTRL, &warnings),
            Some(Command::ToggleSidebar)
        );
    }
    #[test]
    fn tab_moves_between_text_fields_everywhere() {
        let tab = || named(Named::Tab);
        let overlay = Context {
            overlay: true,
            ..LIST
        };
        for ctx in [LIST, GRID, SEARCH, OTHER, SETUP, overlay] {
            assert_eq!(
                run(tab(), code(Code::Tab), Modifiers::empty(), &ctx),
                Some(Command::Focus { backwards: false })
            );
            assert_eq!(
                run(tab(), code(Code::Tab), Modifiers::SHIFT, &ctx),
                Some(Command::Focus { backwards: true })
            );
            assert_eq!(run(tab(), code(Code::Tab), Modifiers::CTRL, &ctx), None);
        }
    }
    #[test]
    fn hints_use_the_platform_modifier() {
        let m = modifier_label();
        assert_eq!(hint(Command::ToggleSidebar), format!("{m}B"));
        assert_eq!(
            tip("Toggle details pane", Command::ToggleDetails),
            format!("Toggle details pane ({m}I)")
        );
        assert_eq!(hint(Command::OpenSearch), "/");
        assert_eq!(hint(Command::Send), "T");
    }
}
