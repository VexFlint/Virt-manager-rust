//! Progress reporting for long operations (disk cloning, volume
//! creation) — upstream's `progress.py` plus the text meter it wraps
//! (`_progresspriv.py`, itself copied from python-urlgrabber). The CLI
//! tools print it on stdout; `quiet` keeps every call but prints
//! nothing, same as upstream's `BaseMeter`.

use std::io::Write;
use std::time::Instant;

/// `progress.Meter`: what long operations report to.
pub trait Meter {
    /// `start(text, size)`; `size` is in bytes, `None` when unknown.
    fn start(&mut self, text: &str, size: Option<u64>);
    fn update(&mut self, new_total: u64);
    fn end(&mut self);
}

/// `make_meter(quiet)`.
pub fn make_meter(quiet: bool) -> TextMeter {
    TextMeter::new(quiet)
}

/// `RateEstimator`: a temporal rolling average of the transfer rate.
struct RateEstimator {
    timescale: f64,
    total: Option<u64>,
    start_time: f64,
    last_update_time: f64,
    last_amount_read: u64,
    ave_rate: Option<f64>,
}

impl RateEstimator {
    fn new() -> RateEstimator {
        RateEstimator {
            timescale: 5.0,
            total: None,
            start_time: 0.0,
            last_update_time: 0.0,
            last_amount_read: 0,
            ave_rate: None,
        }
    }

    fn start(&mut self, total: Option<u64>, now: f64) {
        self.total = total;
        self.start_time = now;
        self.last_update_time = now;
        self.last_amount_read = 0;
        self.ave_rate = None;
    }

    fn update(&mut self, amount_read: u64, now: f64) {
        if amount_read == 0 || amount_read < self.last_amount_read {
            // if we just started this file, all bets are off
            self.last_update_time = now;
            self.last_amount_read = amount_read;
            self.ave_rate = None;
            return;
        }
        let time_diff = now - self.last_update_time;
        let read_diff = (amount_read - self.last_amount_read) as f64;
        // First update, on reget is the file size
        if self.last_amount_read != 0 {
            self.last_update_time = now;
            let epsilon = (time_diff / self.timescale).min(1.0);
            let recent = (time_diff != 0.0).then(|| read_diff / time_diff);
            self.ave_rate = match (self.ave_rate, recent) {
                (None, r) => r,
                (Some(last), None) => Some(last),
                (Some(last), Some(r)) => Some(epsilon * r + (1.0 - epsilon) * last),
            };
        }
        self.last_amount_read = amount_read;
    }

    fn elapsed_time(&self) -> f64 {
        self.last_update_time - self.start_time
    }

    fn remaining_time(&self) -> Option<f64> {
        let rate = self.ave_rate.filter(|r| *r != 0.0)?;
        let total = self.total.filter(|t| *t != 0)?;
        Some((total as f64 - self.last_amount_read as f64) / rate)
    }

    fn fraction_read(&self) -> Option<f64> {
        let total = self.total?;
        if total == 0 {
            return Some(1.0);
        }
        Some(self.last_amount_read as f64 / total as f64)
    }
}

/// `format_time`.
pub fn format_time(seconds: Option<f64>, use_hours: bool) -> String {
    match seconds {
        None => if use_hours { "--:--:--" } else { "--:--" }.into(),
        Some(s) if s < 0.0 => if use_hours { "--:--:--" } else { "--:--" }.into(),
        Some(s) if s.is_infinite() => "Infinite".into(),
        Some(s) => {
            let seconds = s as u64;
            let minutes = seconds / 60;
            let seconds = seconds % 60;
            if use_hours {
                format!("{:02}:{:02}:{:02}", minutes / 60, minutes % 60, seconds)
            } else {
                format!("{minutes:02}:{seconds:02}")
            }
        }
    }
}

/// `format_number`: human-readable metric-like numbers, 1024 based.
/// `is_int` is whether Python would have seen an `int` (byte counts)
/// rather than a `float` (rates): an int prints without decimals until
/// it gets divided. `None` is `0`, an int.
pub fn format_number(number: Option<f64>, is_int: bool) -> String {
    const SYMBOLS: [&str; 9] = ["", "k", "M", "G", "T", "P", "E", "Z", "Y"];
    let mut is_int = is_int || number.is_none();
    let mut number = number.unwrap_or(0.0);
    let mut depth = 0;
    while number > 999.0 && depth < SYMBOLS.len() - 1 {
        depth += 1;
        number /= 1024.0;
        is_int = false;
    }
    if is_int {
        format!("{} {}", number as i64, SYMBOLS[depth])
    } else if number < 9.95 {
        format!("{number:.1} {}", SYMBOLS[depth])
    } else {
        format!("{number:.0} {}", SYMBOLS[depth])
    }
}

/// `terminal_width`: stdout's columns, 80 when it isn't a terminal.
fn terminal_width() -> usize {
    // SAFETY: TIOCGWINSZ fills a plain winsize struct; on failure it's
    // left zeroed and we fall back to 80.
    unsafe {
        let mut ws: libc::winsize = std::mem::zeroed();
        if libc::ioctl(1, libc::TIOCGWINSZ, &mut ws) == 0 && ws.ws_col > 0 {
            return ws.ws_col as usize;
        }
    }
    80
}

/// `TerminalLine`: hands out the line's width to elements in order,
/// dropping everything after the first one that doesn't fit.
struct TerminalLine {
    min_len: usize,
    llen: usize,
    fin: bool,
}

impl TerminalLine {
    fn new(min_rest: usize, beg_len: Option<usize>) -> TerminalLine {
        let beg_len = beg_len.unwrap_or(min_rest);
        TerminalLine {
            min_len: min_rest,
            llen: terminal_width().max(beg_len),
            fin: false,
        }
    }

    fn len(&self) -> usize {
        self.llen.saturating_sub(self.min_len)
    }

    fn rest_split(&self, fixed: usize, elements: usize) -> usize {
        self.llen.saturating_sub(fixed) / elements
    }

    fn add(&mut self, element: String) -> String {
        if self.len() < element.chars().count() {
            self.fin = true;
        }
        if self.fin {
            return String::new();
        }
        self.llen -= element.chars().count();
        element
    }
}

/// `%-*.*s`: `text` cut or padded to exactly `width` characters.
fn fit(text: &str, width: usize) -> String {
    let cut: String = text.chars().take(width).collect();
    format!("{cut:<width$}")
}

/// `TextMeter` (and `BaseMeter` when `quiet`).
pub struct TextMeter {
    quiet: bool,
    update_period: f64,
    text: String,
    size: Option<u64>,
    epoch: Instant,
    last_amount_read: u64,
    last_update_time: Option<f64>,
    re: RateEstimator,
}

impl TextMeter {
    pub fn new(quiet: bool) -> TextMeter {
        TextMeter {
            quiet,
            update_period: 0.3,
            text: String::new(),
            size: None,
            epoch: Instant::now(),
            last_amount_read: 0,
            last_update_time: None,
            re: RateEstimator::new(),
        }
    }

    fn now(&self) -> f64 {
        self.epoch.elapsed().as_secs_f64()
    }

    fn do_update(&mut self, amount_read: u64) {
        let etime = self.re.elapsed_time();
        let fread = format_number(Some(amount_read as f64), true);
        let ave_dl = format_number(self.re.ave_rate, false);

        // Include text + ui_rate in minimal
        let mut tl = TerminalLine::new(8, Some(8 + 1 + 8));
        // For big screens, make it more readable.
        let use_hours = tl.llen > 80;
        let ui_size = tl.add(format!(" | {fread:>5}B"));
        let out = match self.size {
            None => {
                let ui_time = tl.add(format!("  {}", format_time(Some(etime), use_hours)));
                let ui_end = tl.add(" ".repeat(5));
                let ui_rate = tl.add(format!(" {ave_dl:>5}B/s"));
                format!(
                    "{}{ui_rate}{ui_size}{ui_time}{ui_end}\r",
                    fit(&self.text, tl.llen)
                )
            }
            Some(_) => {
                let frtime = format_time(self.re.remaining_time(), use_hours);
                let frac = self.re.fraction_read().unwrap_or(0.0);
                let ui_time = tl.add(format!("  {frtime}"));
                let ui_end = tl.add(" ETA ".into());
                let ui_pc = tl.add(format!(" {:2}%", (frac * 100.0) as i64));
                let ui_rate = tl.add(format!(" {ave_dl:>5}B/s"));
                // Make text grow a bit before we start growing the bar too
                let blen = 4 + tl.rest_split(8 + 8 + 4, 2);
                let bar_len = blen as f64 * frac;
                let ibar_len = bar_len as usize;
                let mut bar = "=".repeat(ibar_len);
                if bar_len - ibar_len as f64 >= 0.5 {
                    bar.push('-');
                }
                let ui_bar = tl.add(format!(" [{}]", fit(&bar, blen)));
                format!(
                    "\r{}{ui_pc}{ui_bar}{ui_rate}{ui_size}{ui_time}{ui_end}\r",
                    fit(&self.text, tl.llen)
                )
            }
        };
        let mut stdout = std::io::stdout();
        let _ = stdout.write_all(out.as_bytes());
        let _ = stdout.flush();
    }

    fn do_end(&mut self) {
        let amount_read = self.last_amount_read;
        let total_size = format_number(Some(amount_read as f64), true);

        let mut tl = TerminalLine::new(8, None);
        // For big screens, make it more readable.
        let use_hours = tl.llen > 80;
        let ui_time = tl.add(format!(
            "  {}",
            format_time(Some(self.re.elapsed_time()), use_hours)
        ));
        // `_term_add_end`
        let (ui_end, not_done) = match self.size.filter(|s| *s > 0) {
            Some(osize) if amount_read > osize => (tl.add(" !!! ".into()), true),
            Some(osize) if amount_read != osize => (tl.add(" ... ".into()), true),
            _ => (tl.add(" ".repeat(5)), false),
        };
        let ui_size = if !not_done && amount_read == 0 {
            // Doesn't need to print total_size
            tl.add(format!(" | {:>5} ", " "))
        } else {
            tl.add(format!(" | {total_size:>5}B"))
        };
        let out = format!("\r{}{ui_size}{ui_time}{ui_end}\n", fit(&self.text, tl.llen));
        let mut stdout = std::io::stdout();
        let _ = stdout.write_all(out.as_bytes());
        let _ = stdout.flush();
    }
}

impl Meter for TextMeter {
    fn start(&mut self, text: &str, size: Option<u64>) {
        self.text = text.to_string();
        self.size = size;
        let now = self.now();
        self.re.start(size, now);
        self.last_amount_read = 0;
        self.last_update_time = Some(now);
    }

    fn update(&mut self, amount_read: u64) {
        let now = self.now();
        self.last_amount_read = amount_read;
        self.re.update(amount_read, now);
        let due = match self.last_update_time {
            Some(last) => now >= last + self.update_period,
            None => true,
        };
        if due {
            self.last_update_time = Some(now);
            if !self.quiet {
                self.do_update(amount_read);
            }
        }
    }

    fn end(&mut self) {
        if !self.quiet {
            self.do_end();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_number_matches_urlgrabber() {
        assert_eq!(format_number(None, false), "0 ");
        assert_eq!(format_number(Some(999.0), true), "999 ");
        assert_eq!(format_number(Some(5.0), false), "5.0 ");
        assert_eq!(format_number(Some(1000.0), true), "1.0 k");
        assert_eq!(format_number(Some(10.0 * 1024.0 * 1024.0), true), "10 M");
        assert_eq!(
            format_number(Some(1.5 * 1024.0 * 1024.0 * 1024.0), true),
            "1.5 G"
        );
    }

    #[test]
    fn format_time_matches_urlgrabber() {
        assert_eq!(format_time(None, false), "--:--");
        assert_eq!(format_time(None, true), "--:--:--");
        assert_eq!(format_time(Some(75.9), false), "01:15");
        assert_eq!(format_time(Some(3725.0), true), "01:02:05");
    }
}
