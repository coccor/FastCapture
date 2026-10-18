//! Copied from FastPad `src/library/dates.rs`.
//!
//! Dates for daily notes and capture names (FastPad second brain phase 1 spec §9.2, §9.3): a
//! calendar date, the format tokens FastPad knows, and reading a daily note's name back.

/// A calendar date. The weekday is worked out, never stored.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct Date {
    pub year: u16,
    pub month: u8,
    pub day: u8,
}

/// A time of day, to the minute.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Time {
    pub hour: u8,
    pub minute: u8,
}

const MONTHS: [&str; 12] = [
    "January",
    "February",
    "March",
    "April",
    "May",
    "June",
    "July",
    "August",
    "September",
    "October",
    "November",
    "December",
];

const WEEKDAYS: [&str; 7] = [
    "Sunday",
    "Monday",
    "Tuesday",
    "Wednesday",
    "Thursday",
    "Friday",
    "Saturday",
];

/// The daily note format when `notebook.ini` names none.
pub const DEFAULT_DAILY_FORMAT: &str = "YYYY-MM-DD";

/// The heading of a daily note made without a template.
pub const DEFAULT_DAILY_HEADING: &str = "dddd, D MMMM YYYY";

impl Date {
    /// A real date, or `None`.
    pub fn new(year: u16, month: u8, day: u8) -> Option<Self> {
        (year >= 1 && (1..=12).contains(&month) && day >= 1 && day <= days_in(year, month))
            .then_some(Self { year, month, day })
    }

    /// 0 for Sunday through 6 for Saturday (Sakamoto's method).
    pub fn weekday(self) -> usize {
        const OFFSETS: [u32; 12] = [0, 3, 2, 5, 0, 3, 5, 1, 4, 6, 2, 4];
        let month = usize::from(self.month - 1);
        let year = u32::from(self.year) - u32::from(self.month < 3);
        let day = u32::from(self.day);
        ((year + year / 4 - year / 100 + year / 400 + OFFSETS[month] + day) % 7) as usize
    }
}

fn is_leap(year: u16) -> bool {
    (year.is_multiple_of(4) && !year.is_multiple_of(100)) || year.is_multiple_of(400)
}

fn days_in(year: u16, month: u8) -> u8 {
    match month {
        2 if is_leap(year) => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

/// One piece of a format.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Token<'a> {
    Year,
    MonthName,
    MonthShort,
    Month2,
    Day2,
    Day,
    WeekdayName,
    WeekdayShort,
    Literal(&'a str),
}

/// Splits `format` into tokens, longest first; anything else is literal.
fn tokens(format: &str) -> Vec<Token<'_>> {
    const KNOWN: [(&str, Token<'static>); 8] = [
        ("YYYY", Token::Year),
        ("MMMM", Token::MonthName),
        ("MMM", Token::MonthShort),
        ("MM", Token::Month2),
        ("dddd", Token::WeekdayName),
        ("ddd", Token::WeekdayShort),
        ("DD", Token::Day2),
        ("D", Token::Day),
    ];
    let mut out = Vec::new();
    let mut literal_start = 0;
    let mut at = 0;
    while at < format.len() {
        let rest = &format[at..];
        if let Some((text, token)) = KNOWN.iter().find(|(text, _)| rest.starts_with(text)) {
            if literal_start < at {
                out.push(Token::Literal(&format[literal_start..at]));
            }
            out.push(*token);
            at += text.len();
            literal_start = at;
        } else {
            at += rest.chars().next().map_or(1, char::len_utf8);
        }
    }
    if literal_start < format.len() {
        out.push(Token::Literal(&format[literal_start..]));
    }
    out
}

/// `date` written in `format`: `YYYY`, `MM`, `DD`, `D`, `MMM` (Jan), `MMMM` (January), `ddd`
/// (Mon) and `dddd` (Monday); anything else as it is.
pub fn format(date: Date, format: &str) -> String {
    let mut out = String::new();
    for token in tokens(format) {
        match token {
            Token::Year => out.push_str(&format!("{:04}", date.year)),
            Token::MonthName => out.push_str(MONTHS[usize::from(date.month - 1)]),
            Token::MonthShort => out.push_str(&MONTHS[usize::from(date.month - 1)][..3]),
            Token::Month2 => out.push_str(&format!("{:02}", date.month)),
            Token::Day2 => out.push_str(&format!("{:02}", date.day)),
            Token::Day => out.push_str(&date.day.to_string()),
            Token::WeekdayName => out.push_str(WEEKDAYS[date.weekday()]),
            Token::WeekdayShort => out.push_str(&WEEKDAYS[date.weekday()][..3]),
            Token::Literal(text) => out.push_str(text),
        }
    }
    out
}

/// Whether `format` can name daily notes: it has `YYYY`, `MM` and `DD`, and makes a file name
/// (no path separators or characters Windows refuses).
pub fn is_daily_format(format: &str) -> bool {
    let tokens = tokens(format);
    let has = |wanted: Token<'_>| tokens.contains(&wanted);
    has(Token::Year)
        && has(Token::Month2)
        && has(Token::Day2)
        && !format.trim().is_empty()
        && !format.chars().any(|c| {
            matches!(c, '\\' | '/' | ':' | '*' | '?' | '"' | '<' | '>' | '|') || c.is_control()
        })
}

/// The date a daily note's stem names in `format`, or `None` when it doesn't match. Names are
/// compared without regard to case.
pub fn parse(stem: &str, format: &str) -> Option<Date> {
    let (mut year, mut month, mut day) = (None, None, None);
    let mut rest = stem;
    let take_digits = |rest: &mut &str, count: usize| -> Option<u32> {
        let digits = rest.get(..count)?;
        if !digits.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        *rest = &rest[count..];
        digits.parse().ok()
    };
    let take_name = |rest: &mut &str, names: &[&str], short: bool| -> Option<usize> {
        names.iter().position(|name| {
            let name = if short { &name[..3] } else { name };
            let matched = rest
                .get(..name.len())
                .is_some_and(|head| head.eq_ignore_ascii_case(name));
            if matched {
                *rest = &rest[name.len()..];
            }
            matched
        })
    };
    for token in tokens(format) {
        match token {
            Token::Year => year = Some(take_digits(&mut rest, 4)?),
            Token::Month2 => month = Some(take_digits(&mut rest, 2)?),
            Token::Day2 => day = Some(take_digits(&mut rest, 2)?),
            Token::Day => {
                let count = rest.bytes().take(2).take_while(u8::is_ascii_digit).count();
                day = Some(take_digits(&mut rest, count.max(1))?);
            }
            Token::MonthName => month = Some(take_name(&mut rest, &MONTHS, false)? as u32 + 1),
            Token::MonthShort => month = Some(take_name(&mut rest, &MONTHS, true)? as u32 + 1),
            Token::WeekdayName => {
                take_name(&mut rest, &WEEKDAYS, false)?;
            }
            Token::WeekdayShort => {
                take_name(&mut rest, &WEEKDAYS, true)?;
            }
            Token::Literal(text) => {
                let head = rest.get(..text.len())?;
                if !head.eq_ignore_ascii_case(text) {
                    return None;
                }
                rest = &rest[text.len()..];
            }
        }
    }
    if !rest.is_empty() {
        return None;
    }
    Date::new(
        u16::try_from(year?).ok()?,
        u8::try_from(month?).ok()?,
        u8::try_from(day?).ok()?,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn date(year: u16, month: u8, day: u8) -> Date {
        Date::new(year, month, day).unwrap()
    }

    #[test]
    fn weekdays_follow_the_calendar() {
        assert_eq!(date(2026, 10, 16).weekday(), 5, "a Friday");
        assert_eq!(date(2000, 2, 29).weekday(), 2, "a Tuesday");
        assert_eq!(date(2024, 1, 1).weekday(), 1, "a Monday");
    }

    #[test]
    fn only_real_dates_exist() {
        assert!(Date::new(2024, 2, 29).is_some());
        assert!(Date::new(2023, 2, 29).is_none());
        assert!(Date::new(1900, 2, 29).is_none());
        assert!(Date::new(2026, 4, 31).is_none());
        assert!(Date::new(2026, 13, 1).is_none());
    }

    #[test]
    fn formats_write_every_token_and_keep_the_rest() {
        let day = date(2026, 3, 5);
        assert_eq!(format(day, DEFAULT_DAILY_FORMAT), "2026-03-05");
        assert_eq!(format(day, DEFAULT_DAILY_HEADING), "Thursday, 5 March 2026");
        assert_eq!(format(day, "ddd DD MMM YYYY"), "Thu 05 Mar 2026");
        assert_eq!(format(day, "YYYY/week x"), "2026/week x");
    }

    #[test]
    fn a_daily_format_needs_year_month_and_day_and_a_file_name() {
        assert!(is_daily_format("YYYY-MM-DD"));
        assert!(is_daily_format("YYYY-MM-DD ddd"));
        assert!(!is_daily_format("YYYY-MM"));
        assert!(!is_daily_format("YYYY-M-D"));
        assert!(!is_daily_format("YYYY/MM/DD"));
        assert!(!is_daily_format("YYYY:MM:DD"));
    }

    #[test]
    fn a_daily_name_reads_back_as_its_date() {
        assert_eq!(parse("2026-10-16", "YYYY-MM-DD"), Some(date(2026, 10, 16)));
        assert_eq!(
            parse("2026-10-16 fri", "YYYY-MM-DD ddd"),
            Some(date(2026, 10, 16))
        );
        assert_eq!(
            parse("16 October 2026", "D MMMM YYYY"),
            Some(date(2026, 10, 16))
        );
        assert_eq!(parse("2026-10-16 notes", "YYYY-MM-DD"), None);
        assert_eq!(parse("2026-02-30", "YYYY-MM-DD"), None);
        assert_eq!(parse("Plan", "YYYY-MM-DD"), None);
    }
}
