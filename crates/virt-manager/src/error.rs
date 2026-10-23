//! Upstream's `virtManager/error.py` (`vmmErrorDialog` + `_errorDialog`):
//! the error/warning/info/yes-no dialogs every window uses, with the
//! optional "Details" expander and "Don't ask me again" checkbox.
//!
//! GTK4 dropped `Gtk.Dialog.run()` and deprecated `GtkMessageDialog`, so
//! this builds the same layout (icon, bold primary text, secondary text,
//! details expander, check box, button row) on a plain modal
//! `GtkWindow`, and every call is `async` instead of blocking.

use futures_channel::oneshot;
use gtk::glib;
use gtk::prelude::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MessageType {
    Question,
    Warning,
    Error,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Buttons {
    Ok,
    Close,
    YesNo,
}

/// `_launch_dialog.fix_text`: truncate at 512 chars, wrap at 80 columns.
pub fn fix_text(t: &str) -> String {
    let mut t = t.to_string();
    if t.chars().count() > 512 {
        t = t.chars().take(512).collect::<String>() + "...";
    }
    let mut out = Vec::new();
    for line in t.lines() {
        if line.is_empty() {
            out.push(String::new());
        } else {
            out.extend(wrap(line, 80));
        }
    }
    out.join("\n")
}

/// Python's `textwrap.wrap` for the plain-words case: greedy fill,
/// whitespace collapsed, over-long words split.
fn wrap(line: &str, width: usize) -> Vec<String> {
    let mut lines = Vec::new();
    let mut cur = String::new();
    for word in line.split_whitespace() {
        let mut word = word.to_string();
        loop {
            let need = if cur.is_empty() { 0 } else { 1 } + word.chars().count();
            if cur.chars().count() + need <= width {
                if !cur.is_empty() {
                    cur.push(' ');
                }
                cur.push_str(&word);
                break;
            }
            if !cur.is_empty() {
                lines.push(std::mem::take(&mut cur));
                continue;
            }
            let head: String = word.chars().take(width).collect();
            word = word.chars().skip(width).collect();
            lines.push(head);
            if word.is_empty() {
                break;
            }
        }
    }
    if !cur.is_empty() {
        lines.push(cur);
    }
    lines
}

pub struct DialogSpec<'a> {
    pub kind: MessageType,
    pub buttons: Buttons,
    pub text1: &'a str,
    pub text2: Option<&'a str>,
    pub title: &'a str,
    pub details: Option<&'a str>,
    pub chktext: Option<&'a str>,
}

/// Shows the dialog; resolves to (affirmative response, checkbox state).
pub async fn run_dialog(parent: Option<&gtk::Window>, spec: DialogSpec<'_>) -> (bool, bool) {
    let win = gtk::Window::builder()
        .modal(true)
        .resizable(false)
        .title(spec.title)
        .build();
    win.set_transient_for(parent);
    win.set_destroy_with_parent(true);

    let outer = gtk::Box::new(gtk::Orientation::Vertical, 12);
    outer.set_margin_top(18);
    outer.set_margin_bottom(12);
    outer.set_margin_start(18);
    outer.set_margin_end(18);

    let top = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    let icon = gtk::Image::from_icon_name(match spec.kind {
        MessageType::Question => "dialog-question",
        MessageType::Warning => "dialog-warning",
        MessageType::Error => "dialog-error",
    });
    icon.set_pixel_size(48);
    icon.set_valign(gtk::Align::Start);
    top.append(&icon);

    let texts = gtk::Box::new(gtk::Orientation::Vertical, 6);
    let primary = gtk::Label::new(None);
    primary.set_markup(&format!(
        "<b>{}</b>",
        glib::markup_escape_text(&fix_text(spec.text1))
    ));
    primary.set_xalign(0.0);
    primary.set_wrap(true);
    primary.set_max_width_chars(40);
    texts.append(&primary);
    if let Some(t2) = spec.text2.filter(|t| !t.is_empty()) {
        let secondary = gtk::Label::new(Some(&fix_text(t2)));
        secondary.set_xalign(0.0);
        secondary.set_wrap(true);
        secondary.set_max_width_chars(40);
        texts.append(&secondary);
    }
    top.append(&texts);
    outer.append(&top);

    let chk = spec.chktext.map(gtk::CheckButton::with_label);
    if let Some(chk) = &chk {
        outer.append(chk);
    }

    if let Some(details) = spec.details.filter(|d| *d != spec.text1) {
        let expander = gtk::Expander::new(Some("Details"));
        let buffer = gtk::TextBuffer::new(None);
        buffer.set_text(details);
        let view = gtk::TextView::with_buffer(&buffer);
        view.set_editable(false);
        view.set_wrap_mode(gtk::WrapMode::Word);
        let sw = gtk::ScrolledWindow::new();
        sw.set_has_frame(true);
        sw.set_size_request(400, 240);
        sw.set_child(Some(&view));
        expander.set_child(Some(&sw));
        outer.append(&expander);
    }

    let (tx, rx) = oneshot::channel::<bool>();
    let tx = std::rc::Rc::new(std::cell::RefCell::new(Some(tx)));
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    row.set_halign(gtk::Align::End);
    let labels: &[(&str, bool)] = match spec.buttons {
        Buttons::Ok => &[("_OK", true)],
        Buttons::Close => &[("_Close", false)],
        Buttons::YesNo => &[("_No", false), ("_Yes", true)],
    };
    let mut last = None;
    for (label, affirmative) in labels.iter().copied() {
        let btn = gtk::Button::with_mnemonic(label);
        let tx = tx.clone();
        let win2 = win.clone();
        btn.connect_clicked(move |_| {
            if let Some(tx) = tx.borrow_mut().take() {
                let _ = tx.send(affirmative);
            }
            win2.close();
        });
        row.append(&btn);
        last = Some(btn);
    }
    outer.append(&row);
    {
        let tx = tx.clone();
        win.connect_close_request(move |_| {
            if let Some(tx) = tx.borrow_mut().take() {
                let _ = tx.send(false);
            }
            glib::Propagation::Proceed
        });
    }

    win.set_child(Some(&outer));
    win.present();
    if let Some(btn) = last {
        btn.grab_focus();
    }
    let response = rx.await.unwrap_or(false);
    let checked = chk.map(|c| c.is_active()).unwrap_or(false);
    (response, checked)
}

/// `vmmErrorDialog.show_err`.
pub async fn show_err(
    parent: Option<&gtk::Window>,
    summary: &str,
    details: Option<&str>,
    title: &str,
) {
    let details = details.unwrap_or(summary);
    let details = if details.contains(summary) {
        details.to_string()
    } else {
        format!("{summary}\n\n{details}")
    };
    eprintln!("error dialog message:\nsummary={summary}");
    run_dialog(
        parent,
        DialogSpec {
            kind: MessageType::Error,
            buttons: Buttons::Close,
            text1: summary,
            text2: None,
            title,
            details: Some(&details),
            chktext: None,
        },
    )
    .await;
}

/// `vmmErrorDialog.val_err`: an "Input Error" box for form validation.
pub async fn val_err(parent: Option<&gtk::Window>, text1: &str) {
    run_dialog(
        parent,
        DialogSpec {
            kind: MessageType::Error,
            buttons: Buttons::Ok,
            text1,
            text2: None,
            title: "Input Error",
            details: None,
            chktext: None,
        },
    )
    .await;
}

/// `vmmErrorDialog.yes_no`.
pub async fn yes_no(parent: Option<&gtk::Window>, text1: &str, text2: Option<&str>) -> bool {
    run_dialog(
        parent,
        DialogSpec {
            kind: MessageType::Warning,
            buttons: Buttons::YesNo,
            text1,
            text2,
            title: "",
            details: None,
            chktext: None,
        },
    )
    .await
    .0
}

/// `vmmErrorDialog.chkbox_helper`: skip the prompt (returning `true`)
/// when the config says not to ask; otherwise ask, and store the
/// "Don't ask me again" answer back.
pub async fn chkbox_helper(
    parent: Option<&gtk::Window>,
    getcb: impl Fn() -> bool,
    setcb: impl Fn(bool),
    text1: &str,
    text2: Option<&str>,
) -> bool {
    if !getcb() {
        return true;
    }
    let (response, skip_prompt) = run_dialog(
        parent,
        DialogSpec {
            kind: MessageType::Warning,
            buttons: Buttons::YesNo,
            text1,
            text2,
            title: "",
            details: None,
            chktext: Some("Don't ask me again"),
        },
    )
    .await;
    setcb(!skip_prompt);
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fix_text_wraps_and_truncates_like_upstream() {
        let long = "word ".repeat(30);
        let fixed = fix_text(long.trim());
        assert!(fixed.lines().all(|l| l.len() <= 80));
        assert_eq!(fixed.lines().count(), 2);
        let huge = "x".repeat(600);
        let fixed = fix_text(&huge);
        assert!(fixed.ends_with("..."));
        assert_eq!(fixed.replace('\n', "").len(), 515);
        assert_eq!(fix_text("a\n\nb"), "a\n\nb");
    }
}
