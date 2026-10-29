//! `vmmAsyncJob` (`asyncjob.py`): runs a job on a worker thread behind
//! a progress window, pulsing until the job's meter reports a size.
//! [`run_with_cancel`] adds upstream's `cancel_cb`: a Cancel button,
//! the warning row a failed cancel shows, and "Cancelling job...".

use std::cell::Cell;
use std::rc::Rc;
use std::sync::{mpsc, Arc};
use std::time::{Duration, Instant};

use gtk::glib;
use gtk::prelude::*;

/// What the worker sends to the window.
enum Msg {
    Pulse {
        progress: String,
        stage: Option<String>,
    },
    Fraction {
        frac: f64,
        progress: String,
        stage: Option<String>,
    },
    Done,
}

/// `virtinst.progress.Meter.format_number`: metric-like sizes with
/// 1024 steps, `"512 "` or `"1.2 M"`.
pub fn format_number(number: u64) -> String {
    const SYMBOLS: [&str; 9] = ["", "k", "M", "G", "T", "P", "E", "Z", "Y"];
    if number <= 999 {
        return format!("{number} ");
    }
    let mut number = number as f64;
    let mut depth = 0;
    while number > 999.0 && depth < SYMBOLS.len() - 1 {
        depth += 1;
        number /= 1024.0;
    }
    if number < 9.95 {
        format!("{number:.1} {}", SYMBOLS[depth])
    } else {
        format!("{number:.0} {}", SYMBOLS[depth])
    }
}

/// `virtinst.progress.Meter.format_time(seconds, use_hours=True)`.
pub fn format_time(seconds: Option<f64>) -> String {
    match seconds {
        Some(s) if s >= 0.0 && s.is_finite() => {
            let s = s as u64;
            format!("{:02}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60)
        }
        Some(s) if s.is_infinite() => "Infinite".into(),
        _ => "--:--:--".into(),
    }
}

/// `_vmmMeter`: progress reports from the worker thread.
pub struct Meter {
    tx: mpsc::Sender<Msg>,
    text: String,
    size: Option<u64>,
    total_read: u64,
    start: Option<Instant>,
}

impl Meter {
    /// `start`: `size` is `None` for work of unknown length, which
    /// pulses the bar.
    pub fn start(&mut self, text: &str, size: Option<u64>) {
        self.text = text.to_string();
        self.size = size;
        self.total_read = 0;
        self.start = Some(Instant::now());
        self.write();
    }

    pub fn end(&mut self) {
        let _ = self.tx.send(Msg::Done);
    }

    /// `is_started`.
    pub fn is_started(&self) -> bool {
        self.start.is_some()
    }

    fn write(&self) {
        let stage = Some(self.text.clone());
        let Some(size) = self.size else {
            let _ = self.tx.send(Msg::Pulse {
                progress: String::new(),
                stage,
            });
            return;
        };
        let frac = if size == 0 {
            0.0
        } else {
            self.total_read as f64 / size as f64
        };
        // the average rate so far gives the time left
        let elapsed = self.start.map_or(0.0, |s| s.elapsed().as_secs_f64());
        let remaining = (self.total_read > 0).then(|| {
            elapsed / self.total_read as f64 * size.saturating_sub(self.total_read) as f64
        });
        let progress = format!(
            "{:3}% {:>5}B {} ETA",
            (frac * 100.0) as u32,
            format_number(self.total_read),
            format_time(remaining)
        );
        let _ = self.tx.send(Msg::Fraction {
            frac,
            progress,
            stage,
        });
    }
}

/// virtinst's meter interface, for the downloads and uploads it does
/// (`installer.start_install(guest, meter=meter)`).
impl virtinst_core::progress::Meter for Meter {
    fn start(&mut self, text: &str, size: Option<u64>) {
        Meter::start(self, text, size);
    }

    fn update(&mut self, new_total: u64) {
        self.total_read = new_total;
        self.write();
    }

    fn end(&mut self) {
        Meter::end(self);
    }
}

/// The worker's side of the job.
pub struct JobHandle {
    tx: mpsc::Sender<Msg>,
}

impl JobHandle {
    /// `get_meter`.
    pub fn meter(&self) -> Meter {
        Meter {
            tx: self.tx.clone(),
            text: String::new(),
            size: None,
            total_read: 0,
            start: None,
        }
    }
}

/// `(error, details)`, as `set_error` takes them.
pub type JobError = (String, String);

/// `cancel_cb`: what the Cancel button runs, on a worker thread (it
/// makes a blocking libvirt call). `Ok` is `asyncjob.job_canceled =
/// True`; `Err` is the summary for `asyncjob.show_warning`.
pub type CancelCb = Arc<dyn Fn() -> Result<(), String> + Send + Sync>;

struct Window {
    win: gtk::Window,
    stage: gtk::Label,
    pbar: gtk::ProgressBar,
    warning_box: gtk::Box,
    warning_text: gtk::Label,
    cancel: gtk::Button,
}

impl Window {
    fn new(parent: Option<&gtk::Window>, title: &str, text: &str) -> Window {
        let win = gtk::Window::builder()
            .title(title)
            .resizable(false)
            .deletable(false)
            .default_width(300)
            .build();
        win.set_transient_for(parent);

        let image = gtk::Image::from_icon_name("dialog-information");
        image.set_pixel_size(48);
        image.set_valign(gtk::Align::Start);
        image.set_margin_start(3);
        image.set_margin_end(3);
        image.set_margin_top(10);
        image.set_margin_bottom(10);
        let pbar_text = gtk::Label::new(Some(text));
        pbar_text.set_halign(gtk::Align::Start);
        pbar_text.set_valign(gtk::Align::Start);
        pbar_text.set_wrap(true);
        pbar_text.set_max_width_chars(50);
        pbar_text.set_xalign(0.0);
        pbar_text.set_hexpand(true);
        let header = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        header.append(&image);
        header.append(&pbar_text);

        // `warning-box`: hidden until a cancel fails
        let warning_icon = gtk::Image::from_icon_name("dialog-warning");
        warning_icon.set_margin_start(3);
        warning_icon.set_margin_end(3);
        let warning_text = gtk::Label::new(Some("some warning"));
        warning_text.set_width_request(400);
        warning_text.set_wrap(true);
        warning_text.set_max_width_chars(40);
        warning_text.set_hexpand(true);
        let warning_box = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        warning_box.append(&warning_icon);
        warning_box.append(&warning_text);
        warning_box.set_visible(false);

        let stage = gtk::Label::new(Some("Processing..."));
        stage.set_halign(gtk::Align::Start);
        stage.set_margin_start(3);
        stage.set_margin_end(3);
        stage.set_margin_top(3);
        stage.set_margin_bottom(3);
        let pbar = gtk::ProgressBar::new();
        pbar.set_pulse_step(0.1);
        pbar.set_show_text(true);

        // `cancel-async-job`, packed at the end of `hbox1`
        let cancel = gtk::Button::with_mnemonic("_Cancel");
        cancel.set_visible(false);
        let hbox1 = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        hbox1.set_halign(gtk::Align::End);
        hbox1.append(&cancel);

        let vbox = gtk::Box::new(gtk::Orientation::Vertical, 5);
        vbox.set_margin_start(12);
        vbox.set_margin_end(12);
        vbox.set_margin_top(12);
        vbox.set_margin_bottom(12);
        vbox.append(&header);
        vbox.append(&warning_box);
        vbox.append(&stage);
        vbox.append(&pbar);
        vbox.append(&hbox1);
        win.set_child(Some(&vbox));
        Window {
            win,
            stage,
            pbar,
            warning_box,
            warning_text,
            cancel,
        }
    }

    /// `show_warning`.
    fn show_warning(&self, summary: &str) {
        self.warning_box.set_visible(true);
        self.warning_text.set_markup(&format!(
            "<small>{}</small>",
            glib::markup_escape_text(summary)
        ));
    }
}

/// `vmmAsyncJob(callback, ...).run()`: `work` runs on a worker thread;
/// the result is its error, if any. With `show_progress` false nothing
/// is shown (`simple_async_noshow`).
pub async fn run(
    parent: Option<&gtk::Window>,
    title: &str,
    text: &str,
    show_progress: bool,
    work: impl FnOnce(JobHandle) -> Result<(), JobError> + Send + 'static,
) -> Option<JobError> {
    run_with_cancel(parent, title, text, show_progress, None, work).await
}

/// [`run`] with upstream's `cancel_cb`: the window gets a Cancel
/// button. Once the cancel succeeded the job's error isn't reported
/// (`cb_wrapper`: a libvirt error from a cancelled job; every error a
/// cancellable job here returns is libvirt's).
pub async fn run_with_cancel(
    parent: Option<&gtk::Window>,
    title: &str,
    text: &str,
    show_progress: bool,
    cancel_cb: Option<CancelCb>,
    work: impl FnOnce(JobHandle) -> Result<(), JobError> + Send + 'static,
) -> Option<JobError> {
    let (tx, rx) = mpsc::channel();
    let window = show_progress.then(|| Rc::new(Window::new(parent, title, text)));
    let job_canceled = Rc::new(Cell::new(false));
    let finished = Rc::new(Cell::new(false));
    let cancels: PendingCancels = Rc::default();
    if let Some(w) = &window {
        w.cancel.set_visible(cancel_cb.is_some());
        if let Some(cb) = cancel_cb.clone() {
            connect_cancel(w, cb, &job_canceled, &finished, &cancels);
        }
        w.win.present();
        if cancel_cb.is_none() {
            w.win.set_cursor_from_name(Some("progress"));
        }
    }
    let handle = JobHandle { tx };
    let job = gtk::gio::spawn_blocking(move || work(handle));

    // `_exit_if_necessary`'s 100ms timer: pulse, and apply the meter's
    // reports, until the worker is done.
    let mut pulsing = true;
    let mut job = std::pin::pin!(job);
    let res = loop {
        let tick = glib::timeout_future(Duration::from_millis(100));
        match select_tick(&mut job, tick).await {
            Some(res) => break res,
            None => {
                while let Ok(msg) = rx.try_recv() {
                    let Some(w) = &window else {
                        continue;
                    };
                    // `_set_stage_text`: a cancelled job keeps saying so
                    let set_stage = |stage: Option<String>| {
                        if !job_canceled.get() {
                            w.stage
                                .set_text(stage.as_deref().unwrap_or("Processing..."));
                        }
                    };
                    match msg {
                        Msg::Pulse { progress, stage } => {
                            pulsing = true;
                            w.pbar.set_text(Some(&progress));
                            set_stage(stage);
                        }
                        Msg::Fraction {
                            frac,
                            progress,
                            stage,
                        } => {
                            pulsing = false;
                            set_stage(stage);
                            w.pbar.set_text(Some(&progress));
                            w.pbar.set_fraction(frac.clamp(0.0, 1.0));
                        }
                        Msg::Done => pulsing = false,
                    }
                }
                if pulsing {
                    if let Some(w) = &window {
                        w.pbar.pulse();
                    }
                }
            }
        }
    };
    finished.set(true);
    // Upstream's `_on_cancel` runs `cancel_cb` synchronously, so a job
    // failing because it was aborted already sees `job_canceled`: let a
    // cancel still in flight finish before judging the job's error.
    let pending = cancels.take();
    for cancel in pending {
        let _ = cancel.await;
    }
    if let Some(w) = window {
        w.win.destroy();
    }
    match res {
        Ok(Ok(())) => None,
        Ok(Err(_)) if cancel_cb.is_some() && job_canceled.get() => None,
        Ok(Err(e)) => Some(e),
        Err(_) => Some(("Worker thread panicked".into(), String::new())),
    }
}

/// The cancel calls still running when the job ends.
type PendingCancels = Rc<std::cell::RefCell<Vec<glib::JoinHandle<()>>>>;

/// `_on_cancel`: run `cancel_cb`; on success hide the warning and say
/// "Cancelling job...", otherwise show why it failed.
fn connect_cancel(
    w: &Rc<Window>,
    cb: CancelCb,
    job_canceled: &Rc<Cell<bool>>,
    finished: &Rc<Cell<bool>>,
    cancels: &PendingCancels,
) {
    let weak = Rc::downgrade(w);
    let job_canceled = job_canceled.clone();
    let finished = finished.clone();
    let cancels = cancels.clone();
    w.cancel.connect_clicked(move |_| {
        if finished.get() {
            return;
        }
        let (weak, cb, job_canceled) = (weak.clone(), cb.clone(), job_canceled.clone());
        let cancel = glib::spawn_future_local(async move {
            let res = gtk::gio::spawn_blocking(move || cb())
                .await
                .unwrap_or_else(|_| Err("Worker thread panicked".into()));
            if res.is_ok() {
                job_canceled.set(true);
            }
            let Some(w) = weak.upgrade() else {
                return;
            };
            match res {
                Ok(()) => {
                    w.warning_box.set_visible(false);
                    w.stage.set_text("Cancelling job...");
                }
                Err(summary) => w.show_warning(&summary),
            }
        });
        cancels.borrow_mut().push(cancel);
    });
}

/// Waits for whichever finishes first: `job` (`Some`) or `tick`
/// (`None`).
async fn select_tick<T>(
    job: &mut (impl std::future::Future<Output = T> + Unpin),
    tick: impl std::future::Future<Output = ()>,
) -> Option<T> {
    let mut tick = std::pin::pin!(tick);
    std::future::poll_fn(|cx| {
        use std::task::Poll;
        if let Poll::Ready(v) = std::pin::Pin::new(&mut *job).poll(cx) {
            return Poll::Ready(Some(v));
        }
        if tick.as_mut().poll(cx).is_ready() {
            return Poll::Ready(None);
        }
        Poll::Pending
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats() {
        assert_eq!(format_number(512), "512 ");
        assert_eq!(format_number(1536), "1.5 k");
        assert_eq!(format_number(20 * 1024 * 1024), "20 M");
        assert_eq!(format_time(Some(3725.0)), "01:02:05");
        assert_eq!(format_time(None), "--:--:--");
    }
}
