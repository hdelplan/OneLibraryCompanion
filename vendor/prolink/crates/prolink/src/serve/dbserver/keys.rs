// SPDX-License-Identifier: GPL-3.0-only

//! The Camelot wheel: placing a key name on it, and reading neighbours off it.
//!
//! Two things in the browse surface need this and nothing else does. **KEY has
//! an extra drill level no other category has** — choosing a key does not list
//! its tracks, it offers three widening harmonic tolerances first (F44) — and
//! the tolerances are wheel arithmetic. And the KEY *listing* is the one place
//! this server deliberately differs from the hardware (§8).
//!
//! # Two notations, one wheel
//!
//! rekordbox writes a key in whichever notation a preference selects, so the
//! same library can come back as `Abm` or as `1A`. Both are the same wheel spot
//! and harmonic matching has to work for either, so everything here converts to
//! `(position, letter)` first and the rest of the module never sees a name
//! again. The reference capture that settled the tolerance menu used classical
//! names throughout; `testdata/export.pdb` is Camelot.
//!
//! # Why the ordering differs from a CDJ, on purpose
//!
//! A CDJ sorts key names as text, so a library in Camelot notation comes out
//! `1A 1B 10A 10B 11A 11B 12A 12B 2A 2B` — the wheel positions interleave and
//! two harmonically adjacent keys land eleven screens apart. [`sort_text`]
//! sorts by `(position, letter)` instead. The sort happens entirely here and
//! the deck renders whatever order it is handed, so there is no
//! interoperability cost; everywhere else the goal is to be indistinguishable
//! from a real deck, and here being indistinguishable would mean being wrong.
//!
//! A name that is neither notation — an empty key, or something rekordbox wrote
//! that this table does not know — keeps alphabetical order and sorts *after*
//! every wheel key, because mixing two orderings in one list has no meaningful
//! answer.

/// How many harmonic tolerances a real player offers.
///
/// Read straight off a real reply for `Abm` (1A): level 0 offered `Abm`, level
/// 1 `Abm, B` — the relative major at the same wheel position — and level 2
/// `Abm, B, Dbm, Ebm`, adding the two adjacent positions in the same mode
/// (F44).
pub(super) const TOLERANCES: u32 = 3;

/// Wheel positions, one turn.
const POSITIONS: u8 = 12;

/// A spot on the Camelot wheel.
///
/// `Ord` is derived and is exactly the order the wheel is drawn in:
/// `1A 1B 2A 2B … 12A 12B`.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug, Hash)]
pub(super) struct Wheel {
    /// 1 to 12.
    position: u8,
    /// `false` for the `A` (minor) ring, `true` for `B` (major).
    major: bool,
}

impl Wheel {
    /// Read a key name in either notation, or `None` for one this table does
    /// not place.
    pub(super) fn parse(name: &str) -> Option<Self> {
        let trimmed = name.trim();
        if trimmed.is_empty() {
            return None;
        }
        Self::parse_camelot(trimmed).or_else(|| Self::parse_classical(trimmed))
    }

    /// `12A` — one or two digits and a letter.
    fn parse_camelot(name: &str) -> Option<Self> {
        let (digits, letter) = name.split_at_checked(name.len().checked_sub(1)?)?;
        let major = match letter {
            "A" | "a" => false,
            "B" | "b" => true,
            _ => return None,
        };
        let position: u8 = digits.trim().parse().ok()?;
        (1..=POSITIONS)
            .contains(&position)
            .then_some(Self { position, major })
    }

    /// `Abm`, `F#`, `Gb` — the names rekordbox writes in the other notation.
    fn parse_classical(name: &str) -> Option<Self> {
        let lower = name.to_ascii_lowercase();
        CLASSICAL
            .iter()
            .find(|(text, _, _)| *text == lower)
            .map(|&(_, position, major)| Self { position, major })
    }

    /// The same position on the other ring — the relative major or minor.
    fn relative(self) -> Self {
        Self {
            major: !self.major,
            ..self
        }
    }

    /// The neighbouring position, `step` places around the wheel.
    ///
    /// Wraps, because the wheel is a circle: one below 1A is 12A.
    fn stepped(self, step: i8) -> Self {
        // Arithmetic in 0-based positions so the wrap is a plain modulus, and
        // in `i16` so `-1` cannot underflow before it wraps.
        let zero_based = i16::from(self.position).saturating_sub(1) + i16::from(step);
        let wrapped = zero_based.rem_euclid(i16::from(POSITIONS));
        let position = u8::try_from(wrapped + 1).unwrap_or(self.position);
        Self { position, ..self }
    }
}

/// The wheel spots a `tolerance` widens to, in the order a real player lists
/// them.
///
/// `Abm` at tolerance 2 gives `Abm, B, Dbm, Ebm` — the key itself, its
/// relative, then the position below and the position above (F44). A tolerance
/// beyond the last is clamped rather than refused: the id comes off the wire.
pub(super) fn harmonic_set(key: Wheel, tolerance: u32) -> Vec<Wheel> {
    let mut spots = vec![key];
    if tolerance >= 1 {
        spots.push(key.relative());
    }
    if tolerance >= 2 {
        spots.push(key.stepped(-1));
        spots.push(key.stepped(1));
    }
    spots
}

/// A sort key that orders wheel notation by position rather than as text.
///
/// `1A` becomes `01A` and `10A` becomes `10A`, so plain string ordering puts
/// them in wheel order. Anything this table cannot place is prefixed past every
/// wheel spot and keeps its alphabetical order among the others.
pub(super) fn sort_text(name: &str) -> String {
    match Wheel::parse(name) {
        Some(wheel) => format!(
            "{:02}{}",
            wheel.position,
            if wheel.major { 'B' } else { 'A' }
        ),
        None => format!("~{}", name.to_lowercase()),
    }
}

/// Classical key names to their wheel spot, as `(lowercased name, position,
/// major)`.
///
/// Both spellings of every accidental, because rekordbox writes either.
const CLASSICAL: [(&str, u8, bool); 34] = [
    ("abm", 1, false),
    ("g#m", 1, false),
    ("b", 1, true),
    ("ebm", 2, false),
    ("d#m", 2, false),
    ("f#", 2, true),
    ("gb", 2, true),
    ("bbm", 3, false),
    ("a#m", 3, false),
    ("db", 3, true),
    ("c#", 3, true),
    ("fm", 4, false),
    ("ab", 4, true),
    ("g#", 4, true),
    ("cm", 5, false),
    ("eb", 5, true),
    ("d#", 5, true),
    ("gm", 6, false),
    ("bb", 6, true),
    ("a#", 6, true),
    ("dm", 7, false),
    ("f", 7, true),
    ("am", 8, false),
    ("c", 8, true),
    ("em", 9, false),
    ("g", 9, true),
    ("bm", 10, false),
    ("d", 10, true),
    ("f#m", 11, false),
    ("gbm", 11, false),
    ("a", 11, true),
    ("dbm", 12, false),
    ("c#m", 12, false),
    ("e", 12, true),
];

#[cfg(test)]
mod tests {
    use super::*;

    fn wheel(name: &str) -> Wheel {
        Wheel::parse(name).expect("a key this table places")
    }

    #[test]
    fn both_notations_name_the_same_wheel_spot() {
        // rekordbox writes either depending on a preference, and the harmonic
        // menu has to work for both.
        assert_eq!(wheel("1A"), wheel("Abm"));
        assert_eq!(wheel("1B"), wheel("B"));
        assert_eq!(wheel("12A"), wheel("Dbm"));
        assert_eq!(wheel("12A"), wheel("C#m"));
        assert_eq!(wheel("2B"), wheel("Gb"));
        assert_eq!(wheel("2B"), wheel("F#"));
    }

    #[test]
    fn a_name_off_the_wheel_is_not_placed() {
        assert_eq!(Wheel::parse(""), None);
        assert_eq!(Wheel::parse("13A"), None);
        assert_eq!(Wheel::parse("0A"), None);
        assert_eq!(Wheel::parse("A#dim"), None);
        assert_eq!(Wheel::parse("Unknown"), None);
    }

    #[test]
    fn the_tolerances_widen_the_way_a_real_reply_did() {
        // The reference reply for Abm (1A): 'Abm', then 'Abm, B', then
        // 'Abm, B, Dbm, Ebm' — self, relative, position below, position above
        // (F44). The order is the deck's, not ours.
        let key = wheel("1A");
        assert_eq!(harmonic_set(key, 0), vec![wheel("1A")]);
        assert_eq!(harmonic_set(key, 1), vec![wheel("1A"), wheel("1B")]);
        assert_eq!(
            harmonic_set(key, 2),
            vec![wheel("1A"), wheel("1B"), wheel("12A"), wheel("2A")],
            "one below 1A is 12A: the wheel is a circle"
        );
    }

    #[test]
    fn the_widest_tolerance_wraps_at_both_ends() {
        assert_eq!(
            harmonic_set(wheel("12B"), 2),
            vec![wheel("12B"), wheel("12A"), wheel("11B"), wheel("1B")]
        );
    }

    #[test]
    fn a_tolerance_past_the_last_is_clamped_not_refused() {
        // The tolerance arrives as a filter id off the wire.
        assert_eq!(harmonic_set(wheel("5A"), 99), harmonic_set(wheel("5A"), 2));
    }

    #[test]
    fn camelot_keys_sort_by_position_and_not_as_text() {
        // A CDJ sorts these as text and gets 1A 1B 10A 10B 11A … 2A, which puts
        // two harmonically adjacent keys eleven screens apart (§8).
        let mut names = ["10A", "1A", "2B", "12A", "1B", "2A"];
        names.sort_by_key(|name| sort_text(name));
        assert_eq!(names, ["1A", "1B", "2A", "2B", "10A", "12A"]);
    }

    #[test]
    fn a_key_off_the_wheel_sorts_after_every_key_on_it() {
        // Mixing two orderings in one list has no meaningful answer, so the
        // ones we can place come first and the rest keep alphabetical order.
        let mut names = ["zzz", "12B", "", "Unknown", "1A"];
        names.sort_by_key(|name| sort_text(name));
        assert_eq!(names, ["1A", "12B", "", "Unknown", "zzz"]);
    }

    #[test]
    fn every_classical_name_places_on_the_wheel_it_claims() {
        for (name, position, major) in CLASSICAL {
            let placed = wheel(name);
            assert_eq!(
                placed,
                Wheel { position, major },
                "{name} should be {position}{}",
                if major { 'B' } else { 'A' }
            );
        }
    }

    #[test]
    fn the_two_notations_cover_the_same_twenty_four_spots() {
        let mut classical: Vec<Wheel> = CLASSICAL
            .iter()
            .map(|&(_, position, major)| Wheel { position, major })
            .collect();
        classical.sort_unstable();
        classical.dedup();
        assert_eq!(classical.len(), 24, "twelve positions, two rings");
    }
}
