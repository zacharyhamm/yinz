//! The bottom-right block: what your agents are doing, and what time it is.
//!
//! This is what the one-line bar shows in the space the `Alt + …` hint used to hold
//! permanently. The hint is still there — it takes the space back while Alt is held (or, on a
//! terminal that cannot say, for a moment after an Alt chord) — so the two are alternatives
//! rendered into the same right-aligned slot by [`render_secondary_info`](crate::one_line_ui).
//!
//! Everything here measures itself: `LinePart.len` is maintained by hand throughout this
//! plugin, and the slot's padding is computed from it, so a segment that lies about its width
//! pushes the line off the end of the terminal.

use unicode_width::UnicodeWidthStr;
use zellij_tile::prelude::*;

use crate::LinePart;

/// How many agents are in each state, folded out of the pane manifest.
///
/// Three counts rather than a list: the bar has room for a number, and the question it answers
/// is "is anything waiting on me", not "which pane". Anyone who wants the list has the agents
/// strip.
#[derive(Default, Debug, Clone, Copy, PartialEq, Eq)]
pub struct AgentCounts {
    pub busy: usize,
    /// Only waits a *person* has to answer — see [`AgentCounts::from_manifest`].
    pub waiting: usize,
    pub idle: usize,
}

impl AgentCounts {
    /// Fold every pane's agent record into the three counts.
    ///
    /// Three decisions worth stating, because each could reasonably have gone the other way:
    ///
    /// * `Exited` counts as nothing. The record outlives the process so the pane can say what
    ///   it was; a finished agent is not one you are running.
    /// * `waiting` is [`AgentInfo::is_waiting_on_a_person`], not bare `Waiting`. A wait the
    ///   permission mode is about to answer itself is not something to colour red at somebody.
    ///   Such a wait falls into `busy`, which is what it looks like from outside.
    /// * A record is counted whatever its [`AgentSource`]. Unlike the agents strip, which only
    ///   believes a state the agent itself reported, a spawned-but-silent agent is still an
    ///   agent you have running — and the spawn path stamps it `Busy`, which is true.
    pub fn from_manifest(manifest: &PaneManifest) -> Self {
        let mut counts = AgentCounts::default();
        for panes in manifest.panes.values() {
            for pane in panes {
                let Some(agent) = pane.agent.as_ref() else {
                    continue;
                };
                match agent.state {
                    AgentState::Exited => {},
                    AgentState::Idle => counts.idle += 1,
                    AgentState::Waiting if agent.is_waiting_on_a_person() => counts.waiting += 1,
                    AgentState::Waiting | AgentState::Busy => counts.busy += 1,
                }
            }
        }
        counts
    }

    pub fn animation_interval_ms(&self) -> Option<i64> {
        if self.busy > 0 {
            Some(125)
        } else if self.waiting > 0 {
            Some(500)
        } else {
            None
        }
    }

    fn is_empty(&self) -> bool {
        self.busy == 0 && self.waiting == 0 && self.idle == 0
    }
}

/// One run of text and how to paint it. The block is a handful of these in a row, because the
/// waiting count has to be able to shout while the rest of the line does not.
struct Segment {
    text: String,
    paint: Paint,
}

#[derive(Clone, Copy)]
enum Paint {
    /// The theme's "something is wrong / needs you" colour.
    Alert,
    /// An emphasis colour — busy agents, and the time itself.
    Emphasis,
    /// Backed off: idle counts and the date.
    Dim,
}

impl Segment {
    fn new(text: impl Into<String>, paint: Paint) -> Self {
        Segment {
            text: text.into(),
            paint,
        }
    }

    fn width(&self) -> usize {
        self.text.width()
    }

    fn serialize(&self, dimmed_session: bool) -> String {
        let text = Text::new(&self.text);
        let text = if dimmed_session {
            text.disabled()
        } else {
            match self.paint {
                Paint::Alert => text.error_color_range(..),
                Paint::Emphasis => text.color_range(1, ..),
                Paint::Dim => text.dim_range(..),
            }
        };
        serialize_text(&text.opaque())
    }
}

/// The glyph for each state, and the ASCII the simplified UI gets instead.
///
/// `arrow_fonts` in [`PluginCapabilities`] means the host was told not to expect a font with
/// anything exotic in it, which is the same flag the superkey prefix already branches on. A
/// braille cell and a lozenge are exactly the kind of thing it is warning about.
fn state_marks(arrow_fonts: bool, now_ms: i64) -> (&'static str, &'static str, &'static str) {
    let frame = now_ms.div_euclid(125).rem_euclid(8) as usize;
    if arrow_fonts {
        (["|", "/", "-", "\\"][frame % 4], "w", "i")
    } else {
        (["⣾", "⣽", "⣻", "⢿", "⡿", "⣟", "⣯", "⣷"][frame], "◆", "·")
    }
}

/// `⣾2 ◆1 ·3`, or nothing at all when no pane holds an agent.
///
/// `with_idle` drops the idle count, which is the first thing to go when the line is tight:
/// idle agents are the ones not asking for anything.
fn agent_segments(
    counts: &AgentCounts,
    arrow_fonts: bool,
    with_idle: bool,
    now_ms: i64,
) -> Vec<Segment> {
    if counts.is_empty() {
        return vec![];
    }
    let (busy, waiting, idle) = state_marks(arrow_fonts, now_ms);
    // In the simplified UI the mark follows the number (`2|`), because a bare letter in front
    // of a digit reads as a word; with the glyphs it leads, because that is what you scan for.
    let mark = |glyph: &str, count: usize| match arrow_fonts {
        true => format!("{count}{glyph}"),
        false => format!("{glyph}{count}"),
    };
    let mut segments = vec![];
    // Waiting first and always: it is the only one of the three that is a request.
    if counts.waiting > 0 {
        let paint = if now_ms.div_euclid(500).rem_euclid(2) == 0 {
            Paint::Alert
        } else {
            Paint::Dim
        };
        segments.push(Segment::new(mark(waiting, counts.waiting), paint));
    }
    if counts.busy > 0 {
        segments.push(Segment::new(mark(busy, counts.busy), Paint::Emphasis));
    }
    if with_idle && counts.idle > 0 {
        segments.push(Segment::new(mark(idle, counts.idle), Paint::Dim));
    }
    // A single space between counts, which is enough separation given they are coloured.
    let mut spaced = vec![];
    for (i, segment) in segments.into_iter().enumerate() {
        if i > 0 {
            spaced.push(Segment::new(" ", Paint::Dim));
        }
        spaced.push(segment);
    }
    spaced
}

/// `14:32`, and `Fri 5 Sep` beside it when `with_date`.
fn clock_segments(clock: &Clock, with_date: bool) -> Vec<Segment> {
    let mut segments = vec![Segment::new(clock.time.clone(), Paint::Emphasis)];
    if with_date {
        segments.push(Segment::new("  ", Paint::Dim));
        segments.push(Segment::new(clock.date.clone(), Paint::Dim));
    }
    segments
}

/// The clock as the bar draws it, already resolved into the session's local time.
///
/// Split into the two halves because they are dropped separately: the date is the first thing
/// to go when the line is tight, and the time is the last.
#[derive(Default, Debug, Clone, PartialEq, Eq)]
pub struct Clock {
    /// `14:32`
    pub time: String,
    /// `Fri 5 Sep`
    pub date: String,
}

impl Clock {
    /// The local wall clock at `epoch_ms`, `utc_offset_secs` seconds east of UTC.
    ///
    /// The offset only the host can say — `get_local_utc_offset` — and `0` renders as UTC,
    /// which is the honest answer for a caller that could not ask rather than a wrong one.
    pub fn at(epoch_ms: i64, utc_offset_secs: i32) -> Self {
        let at = civil_from_epoch_ms(
            zellij_tile::prelude::SignedUnixMillis::new(epoch_ms),
            utc_offset_secs,
        );
        let month = MONTH_NAMES
            .get(at.month as usize - 1)
            .copied()
            .unwrap_or("???");
        Clock {
            time: format!("{:02}:{:02}", at.hour, at.minute),
            date: format!("{} {} {}", WEEKDAY_NAMES[at.weekday], at.day, month),
        }
    }
}

/// The whole right-hand block, drawn to fit `max_len`.
///
/// Degrades in one direction, dropping what is least worth the columns first: the date, then
/// the idle count, then the agent counts entirely, then the clock. An empty `LinePart` is a
/// legitimate answer for a terminal too narrow to say anything.
pub fn status_block(
    help: &ModeInfo,
    counts: &AgentCounts,
    clock: &Clock,
    max_len: usize,
    now_ms: i64,
) -> LinePart {
    let arrow_fonts = help.capabilities.arrow_fonts;
    let dimmed = help.session_dimmed.unwrap_or(false);
    // Widest first; the first one that fits is what gets drawn.
    let candidates = [
        (true, true, true),
        (true, true, false),
        (true, false, false),
        (false, false, false),
    ];
    for (with_agents, with_idle, with_date) in candidates {
        let mut segments = vec![];
        if with_agents {
            segments.extend(agent_segments(counts, arrow_fonts, with_idle, now_ms));
        }
        if !segments.is_empty() {
            // Wider than the gap between counts, so the two halves read as two things.
            segments.push(Segment::new("   ", Paint::Dim));
        }
        segments.extend(clock_segments(clock, with_date));
        let len: usize = segments.iter().map(|segment| segment.width()).sum();
        if len <= max_len {
            let part = segments
                .iter()
                .map(|segment| segment.serialize(dimmed))
                .collect::<String>();
            return LinePart { part, len };
        }
    }
    LinePart::default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn agent(state: AgentState, wait_needs_a_person: bool) -> AgentInfo {
        AgentInfo {
            kind: "claude".to_owned(),
            state,
            source: AgentSource::Reported,
            detail: None,
            waiting_on: None,
            wait_needs_a_person,
            cwd: None,
            session_id: None,
            run_id: None,
        }
    }

    fn manifest(agents: Vec<Option<AgentInfo>>) -> PaneManifest {
        let panes = agents
            .into_iter()
            .enumerate()
            .map(|(i, agent)| PaneInfo {
                id: zellij_tile::prelude::PaneId::from_legacy(i as u32, false),
                agent,
                ..Default::default()
            })
            .collect();
        PaneManifest {
            panes: [(zellij_tile::prelude::TabPosition::new(0), panes)]
                .into_iter()
                .collect(),
        }
    }

    fn mode_info() -> ModeInfo {
        ModeInfo::default()
    }

    #[test]
    fn a_pane_with_no_agent_counts_as_nothing() {
        let counts = AgentCounts::from_manifest(&manifest(vec![None, None]));
        assert_eq!(counts, AgentCounts::default());
    }

    #[test]
    fn an_exited_agent_counts_as_nothing() {
        let counts =
            AgentCounts::from_manifest(&manifest(vec![Some(agent(AgentState::Exited, false))]));
        assert_eq!(counts, AgentCounts::default());
    }

    #[test]
    fn only_a_wait_that_needs_a_person_is_waiting() {
        let counts = AgentCounts::from_manifest(&manifest(vec![
            Some(agent(AgentState::Waiting, true)),
            Some(agent(AgentState::Waiting, false)),
        ]));
        // The suppressed wait is drawn as busy, which is what it looks like from outside.
        assert_eq!(
            counts,
            AgentCounts {
                busy: 1,
                waiting: 1,
                idle: 0
            }
        );
    }

    #[test]
    fn two_silent_idle_records_remain_counted_until_they_end() {
        let mut panes = manifest(vec![Some(agent(AgentState::Idle, false)); 2]);
        for _ in 0..3 {
            assert_eq!(
                AgentCounts::from_manifest(&panes),
                AgentCounts {
                    idle: 2,
                    ..AgentCounts::default()
                }
            );
        }
        let records = panes.panes.values_mut().next().unwrap();
        records[0].agent.as_mut().unwrap().state = AgentState::Busy;
        assert_eq!(
            AgentCounts::from_manifest(&panes),
            AgentCounts {
                busy: 1,
                idle: 1,
                waiting: 0
            }
        );
        let records = panes.panes.values_mut().next().unwrap();
        records[0].agent = None;
        records[1].agent.as_mut().unwrap().state = AgentState::Exited;
        assert_eq!(AgentCounts::from_manifest(&panes), AgentCounts::default());
    }

    #[test]
    fn every_state_is_counted_whatever_reported_it() {
        let mut spawned = agent(AgentState::Busy, false);
        spawned.source = AgentSource::Spawned;
        let mut registered = agent(AgentState::Idle, false);
        registered.source = AgentSource::Registered;
        let counts = AgentCounts::from_manifest(&manifest(vec![Some(spawned), Some(registered)]));
        assert_eq!(
            counts,
            AgentCounts {
                busy: 1,
                waiting: 0,
                idle: 1
            }
        );
    }

    #[test]
    fn animation_wraps_without_changing_text_width_or_session_dimming() {
        let counts = AgentCounts {
            busy: 12,
            waiting: 34,
            idle: 5,
        };
        let clock = Clock::at(0, 0);
        for ascii in [false, true] {
            let mut help = mode_info();
            help.capabilities.arrow_fonts = ascii;
            for (frame, glyph) in ["⣾", "⣽", "⣻", "⢿", "⡿", "⣟", "⣯", "⣷"]
                .iter()
                .cycle()
                .take(17)
                .enumerate()
            {
                let now = frame as i64 * 125;
                let segments = agent_segments(&counts, ascii, true, now);
                assert_eq!(segments[0].text, if ascii { "34w" } else { "◆34" });
                assert_eq!(
                    segments[2].text,
                    if ascii {
                        format!("12{}", ["|", "/", "-", "\\"][frame % 4])
                    } else {
                        format!("{glyph}12")
                    }
                );
                let expected = if frame / 4 % 2 == 0 {
                    Text::new(&segments[0].text).error_color_range(..)
                } else {
                    Text::new(&segments[0].text).dim_range(..)
                };
                assert_eq!(
                    segments[0].serialize(false),
                    serialize_text(&expected.opaque())
                );
                assert_eq!(
                    segments[0].serialize(true),
                    serialize_text(&Text::new(&segments[0].text).disabled().opaque())
                );
                for width in 0..80 {
                    let block = status_block(&help, &counts, &clock, width, now);
                    assert_eq!(
                        block.len,
                        status_block(&help, &counts, &clock, width, 0).len
                    );
                    let len = block.len;
                    assert_eq!(crate::test_support::plain_text(block).width(), len);
                }
            }
        }
        assert_eq!(state_marks(false, 999).0, "⣷");
        assert_eq!(state_marks(false, 1000).0, "⣾");
    }

    #[test]
    fn the_block_never_claims_more_columns_than_it_was_given() {
        let help = mode_info();
        let counts = AgentCounts {
            busy: 12,
            waiting: 3,
            idle: 40,
        };
        let clock = Clock::at(1_709_208_000_000, 0);
        for max_len in 0..80 {
            let part = status_block(&help, &counts, &clock, max_len, 0);
            assert!(
                part.len <= max_len,
                "claimed {} columns of {}",
                part.len,
                max_len
            );
        }
    }

    #[test]
    fn it_sheds_the_date_then_the_idle_count_then_the_agents() {
        let help = mode_info();
        let counts = AgentCounts {
            busy: 2,
            waiting: 1,
            idle: 3,
        };
        let clock = Clock::at(1_709_208_000_000, 0);
        // Widths, in the order the ladder gives them up.
        let full = status_block(&help, &counts, &clock, 200, 0).len;
        let no_date = status_block(&help, &counts, &clock, full - 1, 0).len;
        let no_idle = status_block(&help, &counts, &clock, no_date - 1, 0).len;
        let clock_only = status_block(&help, &counts, &clock, no_idle - 1, 0).len;
        assert!(full > no_date && no_date > no_idle && no_idle > clock_only);
        // The last rung standing is the time, and nothing narrower than it draws at all.
        assert_eq!(clock_only, "14:32".width());
        assert_eq!(
            status_block(&help, &counts, &clock, clock_only - 1, 0).len,
            0
        );
    }

    #[test]
    fn with_no_agents_it_is_just_the_clock() {
        let help = mode_info();
        let clock = Clock::at(1_709_208_000_000, 0);
        let part = status_block(&help, &AgentCounts::default(), &clock, 200, 0);
        assert_eq!(part.len, "14:32  Fri 29 Feb".width());
    }

    #[test]
    fn the_simplified_ui_gets_ascii() {
        let mut help = mode_info();
        help.capabilities.arrow_fonts = true;
        let counts = AgentCounts {
            busy: 2,
            waiting: 1,
            idle: 3,
        };
        let segments = agent_segments(&counts, true, true, 0);
        let text: String = segments
            .iter()
            .map(|segment| segment.text.clone())
            .collect();
        assert_eq!(text, "1w 2| 3i");
        assert!(text.is_ascii());
    }

    #[test]
    fn a_waiting_agent_leads_the_counts() {
        let counts = AgentCounts {
            busy: 2,
            waiting: 1,
            idle: 3,
        };
        let segments = agent_segments(&counts, false, true, 0);
        let text: String = segments
            .iter()
            .map(|segment| segment.text.clone())
            .collect();
        assert_eq!(text, "\u{25c6}1 \u{28fe}2 \u{00b7}3");
    }

    #[test]
    fn a_state_with_no_agents_in_it_is_not_drawn() {
        let counts = AgentCounts {
            busy: 0,
            waiting: 0,
            idle: 2,
        };
        let segments = agent_segments(&counts, false, true, 0);
        let text: String = segments
            .iter()
            .map(|segment| segment.text.clone())
            .collect();
        assert_eq!(text, "\u{00b7}2");
    }
}
