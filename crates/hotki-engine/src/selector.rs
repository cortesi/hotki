//! Runtime selector state and fuzzy matching helpers.

use std::sync::Arc;

use config::dynamic::{SelectorConfig, SelectorItem};
use mac_keycode::{Chord, Key, Modifier};
use nucleo::{
    Config as NucleoConfig, Matcher as NucleoMatcher, Nucleo, Status, Utf32Str,
    pattern::{CaseMatching, Normalization},
};

/// Event emitted by the selector state machine when handling input.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SelectorEvent {
    /// State changed; the UI should be updated.
    Update,
    /// Confirm the current selection.
    Select,
    /// Cancel the selector.
    Cancel,
    /// No-op input (ignored).
    None,
}

/// A single match candidate stored in the matcher.
#[derive(Clone)]
pub(crate) struct SelectorCandidate {
    /// Stable identity for this candidate.
    pub(crate) id: u64,
    /// User-facing item data.
    pub(crate) item: SelectorItem,
}

impl std::fmt::Debug for SelectorCandidate {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SelectorCandidate")
            .field("id", &self.id)
            .field("label", &self.item.label)
            .finish()
    }
}

/// Wrapper around the nucleo matcher for selector items.
pub(crate) struct SelectorMatcher {
    nucleo: Nucleo<SelectorCandidate>,
    highlight_matcher: NucleoMatcher,
    last_query: String,
    indices: Vec<u32>,
}

impl std::fmt::Debug for SelectorMatcher {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SelectorMatcher")
            .field("last_query", &self.last_query)
            .finish_non_exhaustive()
    }
}

impl SelectorMatcher {
    /// Create a new matcher for a static item list.
    pub(crate) fn new(items: Vec<SelectorItem>, notify: Arc<dyn Fn() + Send + Sync>) -> Self {
        let nucleo = Nucleo::new(NucleoConfig::DEFAULT, notify, None, 1);
        let injector = nucleo.injector();

        for (i, item) in items.into_iter().enumerate() {
            let candidate = SelectorCandidate {
                id: (i as u64) + 1,
                item,
            };
            injector.push(candidate, |cand, cols| {
                cols[0] = cand.item.label.as_str().into();
            });
        }

        Self {
            nucleo,
            highlight_matcher: NucleoMatcher::new(NucleoConfig::DEFAULT),
            last_query: String::new(),
            indices: Vec::new(),
        }
    }

    /// Update the query pattern used for matching.
    pub(crate) fn update_pattern(&mut self, query: &str) {
        let append = query.len() >= self.last_query.len() && query.starts_with(&self.last_query);
        self.nucleo
            .pattern
            .reparse(0, query, CaseMatching::Smart, Normalization::Smart, append);
        self.last_query.clear();
        self.last_query.push_str(query);
    }

    /// Tick the matcher worker, allowing it to update internal snapshots.
    pub(crate) fn tick(&mut self) -> Status {
        self.nucleo.tick(10)
    }

    /// Return the number of items matched by the current snapshot.
    pub(crate) fn matched_count(&self) -> u32 {
        self.nucleo.snapshot().matched_item_count()
    }

    /// Return the nth matched candidate, if it exists.
    pub(crate) fn matched_candidate(&self, index: u32) -> Option<&SelectorCandidate> {
        self.nucleo
            .snapshot()
            .get_matched_item(index)
            .map(|i| i.data)
    }

    /// Return matched items for a windowed range, including highlight indices.
    pub(crate) fn matched_window(&mut self, start: u32, end: u32) -> Vec<(SelectorItem, Vec<u32>)> {
        let snapshot = self.nucleo.snapshot();
        let pattern = snapshot.pattern().column_pattern(0);
        let mut out = Vec::new();
        for matched in snapshot.matched_items(start..end) {
            self.indices.clear();
            let haystack: Utf32Str<'_> = matched.matcher_columns[0].slice(..);
            let _score_ignored =
                pattern.indices(haystack, &mut self.highlight_matcher, &mut self.indices);
            self.indices.sort_unstable();
            self.indices.dedup();
            out.push((matched.data.item.clone(), self.indices.clone()));
        }
        out
    }
}

/// Interactive selector runtime state.
pub(crate) struct SelectorState {
    pub(crate) config: SelectorConfig,
    pub(crate) session: SelectorSession,
    pub(crate) prev_hud_visible: bool,
}

impl std::fmt::Debug for SelectorState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SelectorState")
            .field("title", &self.config.title)
            .field("query", &self.session.query)
            .field("selected", &self.session.selected)
            .finish_non_exhaustive()
    }
}

impl SelectorState {
    /// Create a new selector state for a resolved item list.
    pub(crate) fn new(
        config: SelectorConfig,
        items: Vec<SelectorItem>,
        notify: Arc<dyn Fn() + Send + Sync>,
        prev_hud_visible: bool,
    ) -> Self {
        let session = SelectorSession::new(items, notify);
        Self {
            config,
            session,
            prev_hud_visible,
        }
    }

    /// Tick the matcher worker. Returns true when the snapshot changed.
    pub(crate) fn tick(&mut self) -> bool {
        self.session.tick()
    }

    /// Handle a key-down event routed to the selector.
    pub(crate) fn handle_key_down(&mut self, chord: &Chord) -> SelectorEvent {
        self.session.handle_key_down(chord)
    }
}

/// Selector session state: query text, selection cursor, and match snapshot.
pub(crate) struct SelectorSession {
    pub(crate) matcher: SelectorMatcher,
    pub(crate) query: String,
    pub(crate) selected: u32,
}

impl std::fmt::Debug for SelectorSession {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SelectorSession")
            .field("query", &self.query)
            .field("selected", &self.selected)
            .finish_non_exhaustive()
    }
}

impl SelectorSession {
    /// Create a new selector session for a resolved item list.
    pub(crate) fn new(items: Vec<SelectorItem>, notify: Arc<dyn Fn() + Send + Sync>) -> Self {
        let mut matcher = SelectorMatcher::new(items, notify);
        matcher.update_pattern("");
        Self {
            matcher,
            query: String::new(),
            selected: 0,
        }
    }

    /// Tick the matcher worker. Returns true when the snapshot changed.
    pub(crate) fn tick(&mut self) -> bool {
        self.matcher.tick().changed
    }

    /// Handle a key-down event routed to the selector.
    pub(crate) fn handle_key_down(&mut self, chord: &Chord) -> SelectorEvent {
        match selector_action_for_chord(chord) {
            SelectorAction::None => SelectorEvent::None,
            SelectorAction::Cancel => SelectorEvent::Cancel,
            SelectorAction::Select => {
                if self.matcher.matched_count() == 0 {
                    SelectorEvent::None
                } else {
                    SelectorEvent::Select
                }
            }
            SelectorAction::MoveUp => {
                if self.selected > 0 {
                    self.selected -= 1;
                    SelectorEvent::Update
                } else {
                    SelectorEvent::None
                }
            }
            SelectorAction::MoveDown => {
                let max = self.matcher.matched_count().saturating_sub(1);
                if self.selected < max {
                    self.selected += 1;
                    SelectorEvent::Update
                } else {
                    SelectorEvent::None
                }
            }
            SelectorAction::Backspace => {
                if self.query.pop().is_some() {
                    self.selected = 0;
                    self.matcher.update_pattern(&self.query);
                    SelectorEvent::Update
                } else {
                    SelectorEvent::None
                }
            }
            SelectorAction::Clear => {
                if !self.query.is_empty() {
                    self.query.clear();
                    self.selected = 0;
                    self.matcher.update_pattern(&self.query);
                    SelectorEvent::Update
                } else {
                    SelectorEvent::None
                }
            }
            SelectorAction::Append(ch) => {
                self.query.push(ch);
                self.selected = 0;
                self.matcher.update_pattern(&self.query);
                SelectorEvent::Update
            }
        }
    }
}

/// Logical selector actions derived from a key chord.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SelectorAction {
    None,
    Cancel,
    Select,
    MoveUp,
    MoveDown,
    Backspace,
    Clear,
    Append(char),
}

/// Map a chord into a selector action according to the selector key spec.
fn selector_action_for_chord(chord: &Chord) -> SelectorAction {
    // Escape and enter always win.
    match chord.key {
        Key::Escape => return SelectorAction::Cancel,
        Key::Return | Key::KeypadEnter => return SelectorAction::Select,
        Key::UpArrow => return SelectorAction::MoveUp,
        Key::DownArrow => return SelectorAction::MoveDown,
        Key::Delete | Key::ForwardDelete => return SelectorAction::Backspace,
        _ => {}
    }

    let ctrl = chord.modifiers.contains(&Modifier::Control);
    if ctrl {
        return match chord.key {
            Key::P => SelectorAction::MoveUp,
            Key::N => SelectorAction::MoveDown,
            Key::U => SelectorAction::Clear,
            _ => SelectorAction::None,
        };
    }

    // Only accept printable input with no modifiers or just Shift.
    if (chord.modifiers.is_empty()
        || (chord.modifiers.len() == 1 && chord.modifiers.contains(&Modifier::Shift)))
        && let Some(ch) =
            printable_char_for_key(chord.key, chord.modifiers.contains(&Modifier::Shift))
    {
        return SelectorAction::Append(ch);
    }

    SelectorAction::None
}

/// Convert a keypress to a printable character when possible.
fn printable_char_for_key(key: Key, shift: bool) -> Option<char> {
    let spec = key.to_spec();
    let mut chars = spec.chars();
    let ch = chars.next()?;
    if chars.next().is_some() {
        return None;
    }

    if shift {
        if ch.is_ascii_alphabetic() {
            return Some(ch.to_ascii_uppercase());
        }
        let shifted = match ch {
            '1' => '!',
            '2' => '@',
            '3' => '#',
            '4' => '$',
            '5' => '%',
            '6' => '^',
            '7' => '&',
            '8' => '*',
            '9' => '(',
            '0' => ')',
            '-' => '_',
            '=' => '+',
            '[' => '{',
            ']' => '}',
            '\\' => '|',
            ';' => ':',
            '\'' => '"',
            ',' => '<',
            '.' => '>',
            '/' => '?',
            '`' => '~',
            _ => ch,
        };
        return Some(shifted);
    }
    Some(ch)
}

/// Return the set of chords that must be bound while a selector is active.
pub(crate) fn selector_capture_chords() -> Vec<Chord> {
    let mut out = Vec::new();

    // Printable: a-z, 0-9, punctuation, and space.
    let printable_keys = [
        Key::A,
        Key::B,
        Key::C,
        Key::D,
        Key::E,
        Key::F,
        Key::G,
        Key::H,
        Key::I,
        Key::J,
        Key::K,
        Key::L,
        Key::M,
        Key::N,
        Key::O,
        Key::P,
        Key::Q,
        Key::R,
        Key::S,
        Key::T,
        Key::U,
        Key::V,
        Key::W,
        Key::X,
        Key::Y,
        Key::Z,
        Key::Digit0,
        Key::Digit1,
        Key::Digit2,
        Key::Digit3,
        Key::Digit4,
        Key::Digit5,
        Key::Digit6,
        Key::Digit7,
        Key::Digit8,
        Key::Digit9,
        Key::Space,
        Key::Minus,
        Key::Equal,
        Key::LeftBracket,
        Key::RightBracket,
        Key::Backslash,
        Key::Semicolon,
        Key::Quote,
        Key::Comma,
        Key::Period,
        Key::Slash,
        Key::Grave,
    ];
    for key in printable_keys {
        out.push(Chord {
            key,
            modifiers: Default::default(),
        });
    }

    // Shift variants for printable keys (upper-case and symbol variants).
    for key in printable_keys {
        out.push(Chord {
            key,
            modifiers: [Modifier::Shift].into_iter().collect(),
        });
    }

    // Navigation and control keys.
    out.push(Chord {
        key: Key::UpArrow,
        modifiers: Default::default(),
    });
    out.push(Chord {
        key: Key::DownArrow,
        modifiers: Default::default(),
    });
    out.push(Chord {
        key: Key::Return,
        modifiers: Default::default(),
    });
    out.push(Chord {
        key: Key::KeypadEnter,
        modifiers: Default::default(),
    });
    out.push(Chord {
        key: Key::Escape,
        modifiers: Default::default(),
    });
    out.push(Chord {
        key: Key::Delete,
        modifiers: Default::default(),
    });
    out.push(Chord {
        key: Key::ForwardDelete,
        modifiers: Default::default(),
    });

    for key in [Key::N, Key::P, Key::U] {
        out.push(Chord {
            key,
            modifiers: [Modifier::Control].into_iter().collect(),
        });
    }

    // Stable ordering for deterministic rebind snapshots.
    out.sort_by_cached_key(|ch| ch.to_string());
    out.dedup_by(|a, b| a.key == b.key && a.modifiers == b.modifiers);
    out
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;

    fn tick_until_settled(m: &mut SelectorMatcher) {
        for _ in 0..64 {
            let status = m.tick();
            if !status.running {
                return;
            }
        }
        panic!("matcher did not settle");
    }

    fn mk_item(label: &str) -> SelectorItem {
        SelectorItem {
            label: label.to_string(),
            sublabel: None,
            data: label.to_string().into(),
        }
    }

    fn test_matcher(items: Vec<&str>) -> SelectorMatcher {
        let items = items.into_iter().map(mk_item).collect::<Vec<_>>();
        SelectorMatcher::new(items, Arc::new(|| {}))
    }

    #[test]
    fn empty_query_matches_all_in_injection_order() {
        let mut m = test_matcher(vec!["Safari", "Chrome", "Notes"]);
        m.update_pattern("");
        tick_until_settled(&mut m);
        let labels = m
            .matched_window(0, 3)
            .into_iter()
            .map(|(i, _)| i.label)
            .collect::<Vec<_>>();
        assert_eq!(labels, vec!["Safari", "Chrome", "Notes"]);
    }

    #[test]
    fn lowercase_query_matches_uppercase_label() {
        let mut m = test_matcher(vec!["Safari", "Chrome"]);
        m.update_pattern("sa");
        tick_until_settled(&mut m);
        let first = m
            .matched_candidate(0)
            .expect("at least one match")
            .item
            .label
            .clone();
        assert_eq!(first, "Safari");
    }

    #[test]
    fn uppercase_query_is_case_sensitive_with_smart_case() {
        let mut m = test_matcher(vec!["safari", "Safari"]);
        m.update_pattern("Sa");
        tick_until_settled(&mut m);
        assert_eq!(m.matched_count(), 1);
        let first = m
            .matched_candidate(0)
            .expect("at least one match")
            .item
            .label
            .clone();
        assert_eq!(first, "Safari");
    }

    #[test]
    fn prefix_match_ranks_above_substring_match() {
        let mut m = test_matcher(vec!["Zabc", "Abcdef"]);
        m.update_pattern("abc");
        tick_until_settled(&mut m);
        let first = m
            .matched_candidate(0)
            .expect("at least one match")
            .item
            .label
            .clone();
        assert_eq!(first, "Abcdef");
    }

    #[test]
    fn substring_match_in_label_is_found() {
        let mut m = test_matcher(vec!["Safari", "Chrome"]);
        m.update_pattern("rom");
        tick_until_settled(&mut m);
        let first = m
            .matched_candidate(0)
            .expect("at least one match")
            .item
            .label
            .clone();
        assert_eq!(first, "Chrome");
    }

    #[test]
    fn ctrl_u_clears_query() {
        let notify = Arc::new(|| {});
        let mut s = SelectorSession::new(vec![mk_item("Safari")], notify);
        s.query = "abc".to_string();
        s.matcher.update_pattern(&s.query);
        tick_until_settled(&mut s.matcher);
        let ev = s.handle_key_down(&Chord::parse("ctrl+u").unwrap());
        assert_eq!(ev, SelectorEvent::Update);
        assert_eq!(s.query, "");
    }

    #[test]
    fn down_arrow_moves_selection() {
        let notify = Arc::new(|| {});
        let mut s = SelectorSession::new(vec![mk_item("Safari"), mk_item("Chrome")], notify);
        tick_until_settled(&mut s.matcher);
        assert_eq!(s.selected, 0);
        let ev = s.handle_key_down(&Chord::parse("down").unwrap());
        assert_eq!(ev, SelectorEvent::Update);
        assert_eq!(s.selected, 1);
        let ev = s.handle_key_down(&Chord::parse("down").unwrap());
        assert_eq!(ev, SelectorEvent::None);
        assert_eq!(s.selected, 1);
    }

    #[test]
    fn ctrl_p_and_ctrl_n_move_selection() {
        let notify = Arc::new(|| {});
        let mut s = SelectorSession::new(vec![mk_item("Safari"), mk_item("Chrome")], notify);
        tick_until_settled(&mut s.matcher);
        let ev = s.handle_key_down(&Chord::parse("ctrl+n").unwrap());
        assert_eq!(ev, SelectorEvent::Update);
        assert_eq!(s.selected, 1);
        let ev = s.handle_key_down(&Chord::parse("ctrl+p").unwrap());
        assert_eq!(ev, SelectorEvent::Update);
        assert_eq!(s.selected, 0);
    }

    #[test]
    fn backspace_deletes_and_resets_selection() {
        let notify = Arc::new(|| {});
        let mut s = SelectorSession::new(vec![mk_item("Safari"), mk_item("Chrome")], notify);
        tick_until_settled(&mut s.matcher);
        let _ = s.handle_key_down(&Chord::parse("down").unwrap());
        assert_eq!(s.selected, 1);

        s.query = "ab".to_string();
        s.matcher.update_pattern(&s.query);
        tick_until_settled(&mut s.matcher);

        let ev = s.handle_key_down(&Chord::parse("backspace").unwrap());
        assert_eq!(ev, SelectorEvent::Update);
        assert_eq!(s.query, "a");
        assert_eq!(s.selected, 0);
    }

    #[test]
    fn shift_digit_appends_symbol() {
        let notify = Arc::new(|| {});
        let mut s = SelectorSession::new(vec![mk_item("!")], notify);
        tick_until_settled(&mut s.matcher);
        let ev = s.handle_key_down(&Chord::parse("shift+1").unwrap());
        assert_eq!(ev, SelectorEvent::Update);
        assert_eq!(s.query, "!");
    }

    #[test]
    fn enter_selects_when_matches_exist() {
        let notify = Arc::new(|| {});
        let mut s = SelectorSession::new(vec![mk_item("Safari")], notify);
        s.matcher.update_pattern("");
        tick_until_settled(&mut s.matcher);
        let ev = s.handle_key_down(&Chord::parse("enter").unwrap());
        assert_eq!(ev, SelectorEvent::Select);
    }

    #[test]
    fn escape_cancels() {
        let notify = Arc::new(|| {});
        let mut s = SelectorSession::new(vec![mk_item("Safari")], notify);
        let ev = s.handle_key_down(&Chord::parse("esc").unwrap());
        assert_eq!(ev, SelectorEvent::Cancel);
    }
}
