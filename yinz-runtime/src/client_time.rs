//! Client timezone capture and display. Never consult the server's local timezone.
use base64::{engine::general_purpose::STANDARD, Engine as _};
use jiff::{fmt::temporal, tz::TimeZone, Timestamp};

use crate::errors::prelude::*;

/// Capture timezone rules, rather than today's offset, so archived dates respect DST.
pub fn capture_timezone() -> Option<String> {
    match capture().context("capturing the client timezone") {
        Ok(timezone) => Some(timezone),
        Err(error) => {
            Err::<(), _>(error).non_fatal();
            None
        },
    }
}

fn capture() -> Result<String> {
    let timezone = TimeZone::try_system().context("detecting the client timezone")?;
    let mut name = String::new();
    if temporal::DateTimePrinter::new()
        .print_time_zone(&timezone, &mut name)
        .is_ok()
    {
        return Ok(name);
    }
    // Unix also permits an anonymous TZif file (a copied /etc/localtime or TZ path).
    // Preserve its historical rules instead of substituting a fixed current offset.
    let path = std::env::var("TZ")
        .ok()
        .map(|path| std::path::PathBuf::from(path.strip_prefix(':').unwrap_or(&path)))
        .unwrap_or_else(|| "/etc/localtime".into());
    let bytes = std::fs::read(path).context("reading the client timezone rules")?;
    TimeZone::tzif("client", &bytes).context("validating the client timezone rules")?;
    Ok(format!("tzif:{}", STANDARD.encode(bytes)))
}

#[derive(Clone, Debug, Default)]
pub struct ClientTimeZone(Option<TimeZone>);

impl ClientTimeZone {
    pub fn from_client(value: Option<&str>) -> Self {
        let timezone = value.and_then(|value| {
            let parsed = if let Some(encoded) = value.strip_prefix("tzif:") {
                STANDARD
                    .decode(encoded)
                    .context("decoding the client timezone rules")
                    .and_then(|bytes| {
                        TimeZone::tzif("client", &bytes)
                            .context("parsing the client timezone rules")
                    })
            } else {
                temporal::DateTimeParser::new()
                    .parse_time_zone(value)
                    .context("parsing the client timezone")
            };
            match parsed {
                Ok(timezone) => Some(timezone),
                Err(error) => {
                    Err::<(), _>(error).non_fatal();
                    None
                },
            }
        });
        Self(timezone)
    }

    /// An absent or invalid timezone must not silently use the server's timezone.
    pub fn timestamp(&self, epoch_ms: Option<i64>) -> String {
        let Some((timezone, timestamp)) = self
            .0
            .as_ref()
            // TZif lookups use whole seconds. Floor negative fractions so an instant
            // just before a transition is rendered with the preceding offset.
            .zip(epoch_ms.and_then(|ms| Timestamp::from_second(ms.div_euclid(1000)).ok()))
        else {
            return "Time unavailable".into();
        };
        let date = timestamp.to_zoned(timezone.clone());
        let day = date.day();
        let ordinal = match day {
            11..=13 => "th",
            _ => match day % 10 {
                1 => "st",
                2 => "nd",
                3 => "rd",
                _ => "th",
            },
        };
        let month = [
            "Jan.", "Feb.", "Mar.", "Apr.", "May", "June", "July", "Aug.", "Sept.", "Oct.", "Nov.",
            "Dec.",
        ][date.month() as usize - 1];
        let hour = match date.hour() % 12 {
            0 => 12,
            hour => hour,
        };
        let minutes = if date.minute() == 0 {
            String::new()
        } else {
            format!(":{:02}", date.minute())
        };
        let meridiem = if date.hour() < 12 { "am" } else { "pm" };
        format!(
            "{month} {day}{ordinal}, {}, {hour}{minutes}{meridiem}",
            date.year()
        )
    }
}

#[cfg(test)]
#[path = "unit/client_time.rs"]
mod tests;
