//! `xmleditor.py` + `ui/xmleditor.ui`: the Details/XML notebook that
//! wraps a details panel, with a GtkSourceView XML editor on the second
//! tab. Editing is read-only unless 'Enable XML editing' is on in
//! Preferences.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gtk::glib;
use gtk::prelude::*;
use sourceview5::prelude::*;

use crate::engine;

const PAGE_DETAILS: u32 = 0;
const PAGE_XML: u32 = 1;

type Callback = RefCell<Option<Box<dyn Fn()>>>;

struct Inner {
    notebook: gtk::Notebook,
    warning_box: gtk::Box,
    srcview: sourceview5::View,
    srcbuff: sourceview5::Buffer,
    curpage: Cell<u32>,
    srcxml: RefCell<String>,
    /// Set by `set_xml`, so loading XML isn't an edit.
    setting_xml: Cell<bool>,
    /// `details_changed`: the details page has unapplied changes.
    details_changed: Cell<bool>,
    changed_cb: Callback,
    xml_requested_cb: Callback,
    xml_reset_cb: Callback,
}

/// `vmmXMLEditor`.
#[derive(Clone)]
pub struct XmlEditor(Rc<Inner>);

impl XmlEditor {
    /// Wraps `details_widget` as the notebook's Details page; the
    /// caller packs [`Self::widget`] where `details_widget` was.
    pub fn new(details_widget: &impl IsA<gtk::Widget>) -> XmlEditor {
        let details_box = gtk::Box::new(gtk::Orientation::Vertical, 0);
        details_box.set_margin_start(6);
        details_box.set_margin_top(6);
        details_box.append(details_widget);

        let srcview = sourceview5::View::new();
        let srcbuff = srcview
            .buffer()
            .downcast::<sourceview5::Buffer>()
            .expect("a GtkSourceView has a GtkSourceBuffer");
        srcview.set_auto_indent(true);
        let lang = sourceview5::LanguageManager::default().language("xml");
        srcbuff.set_language(lang.as_ref());
        srcbuff.set_highlight_syntax(true);
        srcview.set_monospace(true);
        srcview
            .upcast_ref::<gtk::Widget>()
            .update_property(&[gtk::accessible::Property::Label("XML editor")]);
        let scroll = gtk::ScrolledWindow::builder()
            .child(&srcview)
            .has_frame(true)
            .hexpand(true)
            .vexpand(true)
            .build();

        let warning_box = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        warning_box.append(&gtk::Image::from_icon_name("dialog-warning"));
        let warning = gtk::Label::new(None);
        warning.set_markup(
            "<small>XML editing is disabled in 'Preferences'. Only enable it if you know \
             what you are doing.</small>",
        );
        warning.set_wrap(true);
        warning_box.append(&warning);

        let xml_grid = gtk::Grid::new();
        xml_grid.set_margin_start(6);
        xml_grid.set_margin_top(6);
        xml_grid.set_row_spacing(6);
        xml_grid.attach(&warning_box, 0, 0, 1, 1);
        xml_grid.attach(&scroll, 0, 1, 1, 1);

        let notebook = gtk::Notebook::new();
        notebook.set_show_border(false);
        let details_tab = gtk::Label::with_mnemonic("_Details");
        details_tab.update_property(&[gtk::accessible::Property::Label("Details tab")]);
        notebook.append_page(&details_box, Some(&details_tab));
        let xml_tab = gtk::Label::with_mnemonic("_XML");
        xml_tab.set_tooltip_text(Some("Edit libvirt XML"));
        xml_tab.update_property(&[gtk::accessible::Property::Label("XML tab")]);
        notebook.append_page(&xml_grid, Some(&xml_tab));

        let this = XmlEditor(Rc::new(Inner {
            notebook,
            warning_box,
            srcview,
            srcbuff,
            curpage: Cell::new(PAGE_DETAILS),
            srcxml: RefCell::default(),
            setting_xml: Cell::new(false),
            details_changed: Cell::new(false),
            changed_cb: RefCell::default(),
            xml_requested_cb: RefCell::default(),
            xml_reset_cb: RefCell::default(),
        }));
        this.init_ui();
        this
    }

    fn init_ui(&self) {
        let i = &self.0;
        let weak = Rc::downgrade(&self.0);
        i.srcbuff.connect_changed(move |_| {
            if let Some(inner) = weak.upgrade() {
                if !inner.setting_xml.get() {
                    XmlEditor(inner).emit(|i| &i.changed_cb);
                }
            }
        });
        let weak = Rc::downgrade(&self.0);
        i.notebook.connect_switch_page(move |_, _, pagenum| {
            if let Some(inner) = weak.upgrade() {
                XmlEditor(inner).before_page_changed(pagenum);
            }
        });
        let weak = Rc::downgrade(&self.0);
        i.notebook.connect_page_notify(move |nb| {
            if let Some(inner) = weak.upgrade() {
                if let Some(page) = nb.current_page() {
                    inner.curpage.set(page);
                }
            }
        });
        let weak = Rc::downgrade(&self.0);
        engine::get()
            .config()
            .on_xmleditor_enabled_changed(move || {
                if let Some(inner) = weak.upgrade() {
                    XmlEditor(inner).set_xmleditor_enabled_from_config();
                }
            });
        self.set_xmleditor_enabled_from_config();
    }

    fn set_xmleditor_enabled_from_config(&self) {
        let enabled = engine::get().config().get_xmleditor_enabled();
        self.0.srcview.set_editable(enabled);
        self.0.warning_box.set_visible(!enabled);
    }

    fn emit(&self, cb: impl Fn(&Inner) -> &Callback) {
        if let Some(f) = cb(&self.0).borrow().as_ref() {
            f();
        }
    }

    fn parent(&self) -> Option<gtk::Window> {
        self.0.notebook.root().and_downcast::<gtk::Window>()
    }

    // Internal helpers

    fn reselect_page(&self, pagenum: u32) {
        // Setting curpage first will shortcircuit our page changed callback
        self.0.curpage.set(pagenum);
        self.0.notebook.set_current_page(Some(pagenum));
    }

    fn reset_xml(&self) {
        self.set_xml("");
        self.emit(|i| &i.xml_reset_cb);
    }

    fn reset_cursor(&self) {
        // Put cursor at the start of the second line. Starting on the
        // first means XML open/close tags are highlighted which is weird
        // starting visual
        let mut startiter = self.0.srcbuff.start_iter();
        startiter.forward_line();
        self.0.srcbuff.place_cursor(&startiter);
    }

    fn xml_changed(&self) -> bool {
        *self.0.srcxml.borrow() != self.get_xml()
    }

    /// `_before_page_changed_cb`. The questions are asynchronous here:
    /// the old page is selected again while they are up, and the new
    /// one only once the user agrees to leave.
    fn before_page_changed(&self, pagenum: u32) {
        if self.0.curpage.get() == pagenum {
            return;
        }
        let prevpage = self.0.curpage.get();
        let to_xml = pagenum == PAGE_XML;
        let unapplied = if to_xml {
            self.0.details_changed.get()
        } else {
            self.xml_changed()
        };
        if !unapplied {
            self.0.curpage.set(pagenum);
            if to_xml {
                // If the XML page is clicked, emit xml-requested signal which
                // expects the user to call set_xml/set_libvirtobject. This saves
                // having to fetch inactive XML up front, and gives users like
                // a hook to actually serialize the final XML to return
                self.emit(|i| &i.xml_requested_cb);
            } else {
                self.reset_xml();
            }
            return;
        }

        // The notebook can't be kept on the current page from here, so
        // switch back from an idle callback.
        let this = self.clone();
        glib::idle_add_local_once(move || this.reselect_page(prevpage));
        let this = self.clone();
        glib::spawn_future_local(async move {
            let text2 = if to_xml {
                "Your changes will be lost if you leave this tab. Really leave this tab?"
            } else {
                "Your XML changes will be lost if you leave this tab. Really leave this tab?"
            };
            let leave = crate::error::yes_no(
                this.parent().as_ref(),
                "There are unapplied changes.",
                Some(text2),
            )
            .await;
            if !leave {
                return;
            }
            this.reselect_page(pagenum);
            if to_xml {
                this.0.details_changed.set(false);
                this.emit(|i| &i.xml_requested_cb);
            } else {
                this.reset_xml();
            }
        });
    }

    // Public API

    /// The notebook, to pack where the details widget was.
    pub fn widget(&self) -> &gtk::Notebook {
        &self.0.notebook
    }

    pub fn connect_changed(&self, f: impl Fn() + 'static) {
        self.0.changed_cb.replace(Some(Box::new(f)));
    }

    pub fn connect_xml_requested(&self, f: impl Fn() + 'static) {
        self.0.xml_requested_cb.replace(Some(Box::new(f)));
    }

    pub fn connect_xml_reset(&self, f: impl Fn() + 'static) {
        self.0.xml_reset_cb.replace(Some(Box::new(f)));
    }

    pub fn set_details_changed(&self, changed: bool) {
        self.0.details_changed.set(changed);
    }

    /// `get_xml`: the XML from the editor UI.
    pub fn get_xml(&self) -> String {
        let b = &self.0.srcbuff;
        b.text(&b.start_iter(), &b.end_iter(), true).to_string()
    }

    /// `set_xml`: set the editor UI XML to the passed string.
    pub fn set_xml(&self, xml: &str) {
        self.0.setting_xml.set(true);
        self.0.srcxml.replace(xml.to_string());
        self.0.srcbuff.set_text(xml);
        self.reset_cursor();
        self.0.setting_xml.set(false);
    }

    /// `is_xml_selected`.
    pub fn is_xml_selected(&self) -> bool {
        self.0.curpage.get() == PAGE_XML
    }

    /// `reset_state`: clear XML and select the details page. Used when
    /// callers do their own reset_state.
    pub fn reset_state(&self) {
        self.reset_xml();
        // Setting curpage first will shortcircuit our page changed callback
        self.reselect_page(PAGE_DETAILS);
    }
}

/// `xmlutil.unindent_device_xml`: strip the indentation of the first
/// line from every line that starts with it followed by a tag.
pub fn unindent_device_xml(xml: &str) -> String {
    let mut lines = xml.lines().peekable();
    let Some(first) = lines.peek() else {
        return xml.to_string();
    };
    let unindent = first.chars().take_while(|c| *c == ' ').count();
    let prefix = " ".repeat(unindent);
    let mut ret = String::new();
    for line in lines {
        let line = match line.strip_prefix(&prefix) {
            Some(rest) if rest.trim_start_matches(' ').starts_with('<') => rest,
            _ => line,
        };
        ret.push_str(line);
        ret.push('\n');
    }
    ret
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unindent_device_xml_strips_the_first_lines_indent() {
        let xml = "    <disk type=\"file\">\n      <source file=\"/a\"/>\n    </disk>";
        assert_eq!(
            unindent_device_xml(xml),
            "<disk type=\"file\">\n  <source file=\"/a\"/>\n</disk>\n"
        );
        assert_eq!(unindent_device_xml(""), "");
    }
}
