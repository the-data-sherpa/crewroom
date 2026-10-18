//! Source-date compatibility with the retained JavaScript provider boundary.
//!
//! The scanner/composers follow V8's DateParser at e3ddb93205db9db66565447a4eee2143dfe72710.
//! Copyright 2011 the V8 project authors. All rights reserved.
//! Adapted under the BSD license in ../licenses/V8-LICENSE. Tests compare the
//! installed Node parser; the service has no JavaScript runtime dependency.

#[derive(Clone, Copy, PartialEq)]
enum Word {
    Unknown,
    Month,
    Zone,
    Separator,
    Meridian,
}
#[derive(Clone, Copy, PartialEq)]
enum Token {
    Number(i64, usize),
    Symbol(char),
    Word(Word, i64, usize),
    Space,
    Unknown,
    End,
    Invalid,
}
impl Token {
    fn number(self) -> Option<i64> {
        if let Self::Number(n, _) = self {
            Some(n)
        } else {
            None
        }
    }
    fn fixed(self, length: usize, min: i64, max: i64) -> bool {
        matches!(self,Self::Number(n,len) if len==length && (min..=max).contains(&n))
    }
    fn sign(self) -> Option<i64> {
        match self {
            Self::Symbol('+') => Some(1),
            Self::Symbol('-') => Some(-1),
            _ => None,
        }
    }
    fn z(self) -> bool {
        self == Self::Word(Word::Zone, 0, 1)
    }
    fn milliseconds(self) -> i64 {
        let Self::Number(n, length) = self else {
            unreachable!()
        };
        if length < 3 {
            n * 10_i64.pow((3 - length) as u32)
        } else {
            n / 10_i64.pow((length.min(9) - 3) as u32)
        }
    }
}
fn space(c: char) -> bool {
    matches!(c,'\u{0009}'..='\u{000d}'|'\u{0020}'|'\u{00a0}'|'\u{1680}'|'\u{2000}'..='\u{200a}'|'\u{2028}'|'\u{2029}'|'\u{202f}'|'\u{205f}'|'\u{3000}'|'\u{feff}')
}
fn word_space(c: char) -> bool {
    space(c) && !matches!(c, '\n' | '\r' | '\u{2028}' | '\u{2029}')
}
struct Scanner {
    tokens: Vec<Token>,
    at: usize,
}
impl Scanner {
    fn new(text: &str) -> Self {
        let chars: Vec<_> = text.chars().take_while(|c| *c != '\0').collect();
        let mut tokens = Vec::new();
        let mut i = 0;
        while i < chars.len() {
            let c = chars[i];
            let begin = i;
            i += 1;
            let token = if c.is_ascii_digit() {
                while i < chars.len() && chars[i].is_ascii_digit() {
                    i += 1;
                }
                let n = chars[begin..i]
                    .iter()
                    .skip_while(|c| **c == '0')
                    .take(9)
                    .fold(0, |n, c| n * 10 + (*c as i64 - '0' as i64));
                Token::Number(n, i - begin)
            } else if matches!(c, ':' | '-' | '+' | '.' | ')') {
                Token::Symbol(c)
            } else if c >= 'A' && !word_space(c) {
                while i < chars.len() && chars[i] >= 'A' && !word_space(chars[i]) {
                    i += 1;
                }
                let len = i - begin;
                let prefix: String = chars[begin..i]
                    .iter()
                    .take(3)
                    .map(char::to_ascii_lowercase)
                    .collect();
                let month = [
                    "jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov",
                    "dec",
                ]
                .iter()
                .position(|m| *m == prefix);
                if let Some(month) = month {
                    Token::Word(Word::Month, month as i64 + 1, len)
                } else {
                    let (kind, value) = match prefix.as_str() {
                        "am" if len == 2 => (Word::Meridian, 0),
                        "pm" if len == 2 => (Word::Meridian, 12),
                        "ut" if len == 2 => (Word::Zone, 0),
                        "utc" | "gmt" if len == 3 => (Word::Zone, 0),
                        "z" if len == 1 => (Word::Zone, 0),
                        "cdt" | "est" if len == 3 => (Word::Zone, -5),
                        "cst" | "mdt" if len == 3 => (Word::Zone, -6),
                        "edt" if len == 3 => (Word::Zone, -4),
                        "mst" | "pdt" if len == 3 => (Word::Zone, -7),
                        "pst" if len == 3 => (Word::Zone, -8),
                        "t" if len == 1 => (Word::Separator, 0),
                        _ => (Word::Unknown, 0),
                    };
                    Token::Word(kind, value, len)
                }
            } else if space(c) {
                Token::Space
            } else if c == '(' {
                let mut depth = 1;
                while i < chars.len() && depth > 0 {
                    match chars[i] {
                        '(' => depth += 1,
                        ')' => depth -= 1,
                        _ => (),
                    }
                    i += 1;
                }
                Token::Unknown
            } else {
                Token::Unknown
            };
            tokens.push(token);
        }
        tokens.push(Token::End);
        Self { tokens, at: 0 }
    }
    fn peek(&self) -> Token {
        self.tokens[self.at]
    }
    fn next(&mut self) -> Token {
        let t = self.peek();
        if t != Token::End {
            self.at += 1;
        }
        t
    }
    fn skip(&mut self, c: char) -> bool {
        if self.peek() == Token::Symbol(c) {
            self.next();
            true
        } else {
            false
        }
    }
}
#[derive(Default)]
struct Parts {
    day: Vec<i64>,
    time: Vec<i64>,
    month: Option<i64>,
    iso: bool,
    meridian: Option<i64>,
    zone_sign: Option<i64>,
    zone_hour: Option<i64>,
    zone_minute: Option<i64>,
}
impl Parts {
    fn day(&mut self, n: i64) -> bool {
        if self.day.len() == 3 {
            false
        } else {
            self.day.push(n);
            true
        }
    }
    fn time(&mut self, n: i64) -> bool {
        if self.time.len() == 4 {
            false
        } else {
            self.time.push(n);
            true
        }
    }
    fn final_time(&mut self, n: i64) {
        if self.time(n) {
            self.time.resize(4, 0);
        }
    }
    fn expects_time(&self, n: i64) -> bool {
        match self.time.len() {
            1 | 2 => (0..60).contains(&n),
            3 => (0..1000).contains(&n),
            _ => false,
        }
    }
    fn zone(&mut self, hour: i64) {
        self.zone_sign = Some(if hour < 0 { -1 } else { 1 });
        self.zone_hour = Some(hour.abs());
        self.zone_minute = Some(0);
    }
    fn iso(&mut self, s: &mut Scanner) -> Token {
        if let Some(sign) = s.peek().sign() {
            let sign_token = s.next();
            if !s.peek().fixed(6, 0, 999999) {
                return sign_token;
            }
            let year = s.next().number().unwrap();
            if sign < 0 && year == 0 {
                return sign_token;
            }
            self.day(sign * year);
        } else if s.peek().fixed(4, 0, 9999) {
            self.day(s.next().number().unwrap());
        } else {
            return s.next();
        }
        if s.skip('-') {
            if !s.peek().fixed(2, 1, 12) {
                return s.next();
            }
            self.day(s.next().number().unwrap());
            if s.skip('-') {
                if !s.peek().fixed(2, 1, 31) {
                    return s.next();
                }
                self.day(s.next().number().unwrap());
            }
        }
        if !matches!(s.peek(), Token::Word(Word::Separator, _, _)) {
            if s.peek() != Token::End {
                return s.next();
            }
        } else {
            s.next();
            if !s.peek().fixed(2, 0, 24) {
                return Token::Invalid;
            }
            let h = s.next().number().unwrap();
            self.time(h);
            if !s.skip(':') || !s.peek().fixed(2, 0, if h == 24 { 0 } else { 59 }) {
                return Token::Invalid;
            }
            self.time(s.next().number().unwrap());
            if s.skip(':') {
                if !s.peek().fixed(2, 0, if h == 24 { 0 } else { 59 }) {
                    return Token::Invalid;
                }
                self.time(s.next().number().unwrap());
                if s.skip('.') {
                    let Some(n) = s.peek().number() else {
                        return Token::Invalid;
                    };
                    if h == 24 && n > 0 {
                        return Token::Invalid;
                    }
                    self.time(s.next().milliseconds());
                }
            }
            if s.peek().z() {
                s.next();
                self.zone(0);
            } else if let Some(sign) = s.peek().sign() {
                s.next();
                self.zone_sign = Some(sign);
                if s.peek().fixed(4, 0, 9999) {
                    let n = s.next().number().unwrap();
                    if n / 100 > 23 || n % 100 > 59 {
                        return Token::Invalid;
                    }
                    self.zone_hour = Some(n / 100);
                    self.zone_minute = Some(n % 100);
                } else {
                    if !s.peek().fixed(2, 0, 23) {
                        return Token::Invalid;
                    }
                    self.zone_hour = s.next().number();
                    if !s.skip(':') || !s.peek().fixed(2, 0, 59) {
                        return Token::Invalid;
                    }
                    self.zone_minute = s.next().number();
                }
            }
            if s.peek() != Token::End {
                return Token::Invalid;
            }
        }
        if self.zone_hour.is_none() && self.time.is_empty() {
            self.zone(0);
        }
        self.iso = true;
        Token::End
    }
    fn legacy(&mut self, s: &mut Scanner, mut token: Token) -> Option<()> {
        let mut number_seen = !self.day.is_empty();
        while token != Token::End {
            match token {
                Token::Number(n, _) => {
                    number_seen = true;
                    if s.skip(':') {
                        if s.skip(':') {
                            if !self.time.is_empty() {
                                return None;
                            }
                            self.time(n);
                            self.time(0);
                        } else {
                            if !self.time(n) {
                                return None;
                            }
                            s.skip('.');
                        }
                    } else if s.skip('.') && self.expects_time(n) {
                        self.time(n);
                        s.peek().number()?;
                        self.final_time(s.next().milliseconds());
                    } else if self.zone_hour.is_some()
                        && self.zone_minute.is_none()
                        && (0..60).contains(&n)
                    {
                        self.zone_minute = Some(n);
                    } else if self.expects_time(n) {
                        self.final_time(n);
                        let next = s.peek();
                        if next != Token::End
                            && next != Token::Space
                            && !next.z()
                            && next.sign().is_none()
                        {
                            return None;
                        }
                    } else {
                        if !self.day(n) {
                            return None;
                        }
                        s.skip('-');
                    }
                }
                Token::Word(kind, value, _) => match kind {
                    Word::Meridian if !self.time.is_empty() => self.meridian = Some(value),
                    Word::Month => {
                        self.month = Some(value);
                        s.skip('-');
                    }
                    Word::Zone if number_seen => self.zone(value),
                    _ => {
                        if number_seen || s.peek().number().is_some() {
                            return None;
                        }
                    }
                },
                _ if token.sign().is_some()
                    && ((self.zone_hour == Some(0) && self.zone_minute == Some(0))
                        || !self.time.is_empty()) =>
                {
                    self.zone_sign = token.sign();
                    let (n, len) = match s.peek() {
                        Token::Number(n, len) => {
                            s.next();
                            (n, len)
                        }
                        _ => (0, 0),
                    };
                    number_seen = true;
                    if s.peek() == Token::Symbol(':') {
                        self.zone_hour = Some(n);
                        self.zone_minute = None;
                    } else if len == 1 || len == 2 {
                        self.zone_hour = Some(n);
                        self.zone_minute = Some(0);
                    } else if len == 3 || len == 4 {
                        self.zone_hour = Some(n / 100);
                        self.zone_minute = Some(n % 100);
                    } else {
                        return None;
                    }
                }
                _ if (token.sign().is_some() || token == Token::Symbol(')')) && number_seen => {
                    return None;
                }
                _ => (),
            }
            token = s.next();
        }
        Some(())
    }
    fn finish(mut self) -> Option<i64> {
        if self.day.is_empty() {
            return None;
        }
        self.day.resize(3, 1);
        let (mut year, month, day) = if let Some(month) = self.month {
            if !(1..=31).contains(&self.day[0]) {
                (self.day[0], month, self.day[1])
            } else {
                (self.day[1], month, self.day[0])
            }
        } else if self.iso || !(1..=31).contains(&self.day[0]) {
            (self.day[0], self.day[1], self.day[2])
        } else {
            (self.day[2], self.day[0], self.day[1])
        };
        if !self.iso {
            if (0..=49).contains(&year) {
                year += 2000;
            } else if (50..=99).contains(&year) {
                year += 1900;
            }
        }
        if year.abs() > 1_000_000 || !(1..=12).contains(&month) || !(1..=31).contains(&day) {
            return None;
        }
        self.time.resize(4, 0);
        let mut hour = self.time[0];
        let (minute, second, ms) = (self.time[1], self.time[2], self.time[3]);
        if let Some(offset) = self.meridian {
            if !(0..=12).contains(&hour) {
                return None;
            }
            hour = hour % 12 + offset;
        }
        let ordinary = (0..24).contains(&hour)
            && (0..60).contains(&minute)
            && (0..60).contains(&second)
            && (0..1000).contains(&ms);
        if !ordinary && (hour != 24 || minute != 0 || second != 0 || ms != 0) {
            return None;
        }
        let naive = days_from_civil(year, month, day) * 86_400_000
            + hour * 3_600_000
            + minute * 60_000
            + second * 1000
            + ms;
        let result = if let Some(sign) = self.zone_sign {
            let seconds = (self.zone_hour.unwrap_or(0) as u32)
                .wrapping_mul(3600)
                .wrapping_add((self.zone_minute.unwrap_or(0) as u32).wrapping_mul(60));
            if seconds > i32::MAX as u32 {
                return None;
            }
            naive - sign * i64::from(seconds) * 1000
        } else {
            local_to_utc(naive)?
        };
        (result.abs() <= 8_640_000_000_000_000).then_some(result)
    }
}
// Proleptic Gregorian arithmetic also handles JS's expanded-year range and
// accepted day overflow (for example February 31), without chrono's year limit.
fn days_from_civil(mut year: i64, month: i64, day: i64) -> i64 {
    year -= i64::from(month <= 2);
    let era = year.div_euclid(400);
    let y = year - era * 400;
    let m = month + if month > 2 { -3 } else { 9 };
    let d = (153 * m + 2) / 5 + day - 1;
    era * 146097 + y * 365 + y / 4 - y / 100 + d - 719468
}
fn local_offset(seconds: i64) -> Option<i64> {
    let timestamp: libc::time_t = seconds;
    // SAFETY: localtime_r writes only to this initialized, exclusively borrowed tm.
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    if unsafe { libc::localtime_r(&timestamp, &mut tm) }.is_null() {
        return None;
    }
    Some(
        days_from_civil(
            i64::from(tm.tm_year) + 1900,
            i64::from(tm.tm_mon) + 1,
            i64::from(tm.tm_mday),
        ) * 86_400
            + i64::from(tm.tm_hour) * 3600
            + i64::from(tm.tm_min) * 60
            + i64::from(tm.tm_sec)
            - seconds,
    )
}
fn local_to_utc(naive: i64) -> Option<i64> {
    // Probe offsets on both sides of a local transition, including skipped days.
    // localtime_r supports expanded years where macOS mktime fails. Each
    // candidate must round-trip; repeated times choose the earlier instant,
    // nonexistent times advance by the gap as in the retained Date parser.
    let seconds = naive.div_euclid(1000);
    let mut exact = Vec::new();
    let mut gaps = Vec::new();
    for probe in [-172_800, -86_400, 0, 86_400, 172_800] {
        let offset = local_offset(seconds + probe)?;
        let candidate = seconds - offset;
        let local = candidate + local_offset(candidate)?;
        let result = candidate * 1000 + naive.rem_euclid(1000);
        if local == seconds {
            exact.push(result);
        } else if local > seconds {
            gaps.push((local - seconds, result));
        }
    }
    exact.into_iter().min().or_else(|| {
        gaps.into_iter()
            .min_by_key(|(gap, _)| *gap)
            .map(|(_, result)| result)
    })
}
/// Milliseconds since the Unix epoch, or `None` for a rejected source date.
pub fn parse(value: &str) -> Option<i64> {
    let mut scanner = Scanner::new(value);
    let mut parts = Parts::default();
    let token = parts.iso(&mut scanner);
    if token == Token::Invalid {
        return None;
    }
    parts.legacy(&mut scanner, token)?;
    parts.finish()
}
