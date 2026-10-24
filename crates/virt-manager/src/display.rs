//! What the VNC and SPICE displays share: the drawing widget, the
//! framebuffer geometry, screenshots, and the keyboard routing that
//! stands in for GTK3's keyboard grab.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gtk::gdk;
use gtk::glib;
use gtk::glib::translate::IntoGlib;
use gtk::prelude::*;

/// `get_render_region_info` (gtk-vnc) / `spice_display_get_scaling`:
/// where the framebuffer lands in the widget, as
/// `(offset_x, offset_y, scale_x, scale_y)`.
pub fn render_region(
    win: (f64, f64),
    fb: (f64, f64),
    scaling: bool,
    keep_aspect: bool,
) -> (f64, f64, f64, f64) {
    let (ww, wh) = win;
    let (fw, fh) = fb;
    if fw <= 0.0 || fh <= 0.0 {
        return (0.0, 0.0, 1.0, 1.0);
    }
    let (mut sx, mut sy) = (1.0, 1.0);
    if scaling {
        sx = ww / fw;
        sy = wh / fh;
        if keep_aspect {
            let s = sx.min(sy);
            sx = s;
            sy = s;
        }
    }
    let ox = ((ww - fw * sx) / 2.0).max(0.0);
    let oy = ((wh - fh * sy) / 2.0).max(0.0);
    (ox, oy, sx, sy)
}

/// A copy of the guest screen: 32bpp xRGB in native byte order, what
/// both viewers' framebuffers hold. Screenshots are made from it.
pub struct FrameCopy {
    pub width: i32,
    pub height: i32,
    pub stride: usize,
    pub pixels: Vec<u8>,
}

/// `(press, keyval, hardware keycode) -> consumed`
pub type KeyHandler = Rc<dyn Fn(bool, u32, u32) -> bool>;

/// Routes keys to a display the way GTK3 did for gtk-vnc and
/// spice-gtk. While the display holds the (emulated) keyboard grab,
/// a window-level capture handler sends every key to the guest ahead
/// of the window's shortcuts, as upstream does by removing its accel
/// groups during a grab. Without the grab, a focused display still
/// gets the keys the window's shortcuts leave over.
pub fn connect_keys(area: &DisplayWidget, grabbed: Rc<dyn Fn() -> bool>, on_key: KeyHandler) {
    let keys = gtk::EventControllerKey::new();
    let (g, k) = (grabbed.clone(), on_key.clone());
    keys.connect_key_pressed(move |_, keyval, keycode, _| {
        if !g() && k(true, keyval.into_glib(), keycode) {
            glib::Propagation::Stop
        } else {
            glib::Propagation::Proceed
        }
    });
    let (g, k) = (grabbed.clone(), on_key.clone());
    keys.connect_key_released(move |_, keyval, keycode, _| {
        if !g() {
            k(false, keyval.into_glib(), keycode);
        }
    });
    area.add_controller(keys);

    area.connect_realize(move |area| {
        let Some(root) = area.root() else { return };
        let keys = gtk::EventControllerKey::new();
        keys.set_propagation_phase(gtk::PropagationPhase::Capture);
        let (g, k, a) = (grabbed.clone(), on_key.clone(), area.downgrade());
        keys.connect_key_pressed(move |_, keyval, keycode, _| {
            let focused = a.upgrade().is_some_and(|a| a.has_focus());
            if focused && g() && k(true, keyval.into_glib(), keycode) {
                glib::Propagation::Stop
            } else {
                glib::Propagation::Proceed
            }
        });
        let (g, k, a) = (grabbed.clone(), on_key.clone(), area.downgrade());
        keys.connect_key_released(move |_, keyval, keycode, _| {
            let focused = a.upgrade().is_some_and(|a| a.has_focus());
            if focused && g() {
                k(false, keyval.into_glib(), keycode);
            }
        });
        root.add_controller(keys.clone());
        let weak_ctrl = keys.downgrade();
        area.connect_unrealize(move |area| {
            if let (Some(root), Some(ctrl)) = (area.root(), weak_ctrl.upgrade()) {
                root.remove_controller(&ctrl);
            }
        });
    });
}

/// The first hardware keycode producing the keyval in the current
/// layout (`get_scancode_from_keyval`, before the scancode mapping).
pub fn keycode_from_keyval(widget: &impl IsA<gtk::Widget>, keyval: u32) -> Option<u32> {
    // SAFETY: any u32 is a valid (maybe unnamed) keyval.
    let key: gdk::Key = unsafe { glib::translate::from_glib(keyval) };
    widget
        .display()
        .map_keyval(key)
        .and_then(|keys| keys.first().map(|k| k.keycode()))
}

/// A keyval's name, e.g. `Control_L`.
pub fn keyval_name(keyval: u32) -> String {
    // SAFETY: any u32 is a valid (maybe unnamed) keyval.
    let key: gdk::Key = unsafe { glib::translate::from_glib(keyval) };
    key.name().map(|n| n.to_string()).unwrap_or_default()
}

/// The display's drawing surface: paints the framebuffer as a texture,
/// black around it, an optional overlay (SPICE's guest cursor while
/// the pointer is grabbed) on top, and reports allocation changes.
mod widget_imp {
    use super::*;
    use gtk::graphene;
    use gtk::subclass::prelude::*;

    type TextureProvider = Box<dyn Fn() -> Option<gdk::Texture>>;
    /// The overlay and where its top-left lands, in framebuffer pixels.
    type OverlayProvider = Box<dyn Fn() -> Option<(gdk::Texture, f64, f64)>>;
    type ResizeHandler = Box<dyn Fn(i32, i32)>;

    #[derive(Default)]
    pub struct DisplayWidget {
        pub provider: RefCell<Option<TextureProvider>>,
        pub overlay: RefCell<Option<OverlayProvider>>,
        pub on_resize: RefCell<Option<ResizeHandler>>,
        pub texture: RefCell<Option<gdk::Texture>>,
        pub dirty: Cell<bool>,
        pub scaling: Cell<bool>,
        pub keep_aspect: Cell<bool>,
        /// `((min_w, min_h), (nat_w, nat_h))`
        pub request: Cell<((i32, i32), (i32, i32))>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for DisplayWidget {
        const NAME: &'static str = "VmmDisplayWidget";
        type Type = super::DisplayWidget;
        type ParentType = gtk::Widget;
    }

    impl ObjectImpl for DisplayWidget {}

    impl WidgetImpl for DisplayWidget {
        fn measure(&self, orientation: gtk::Orientation, _for_size: i32) -> (i32, i32, i32, i32) {
            let ((mw, mh), (nw, nh)) = self.request.get();
            match orientation {
                gtk::Orientation::Horizontal => (mw, nw.max(mw), -1, -1),
                _ => (mh, nh.max(mh), -1, -1),
            }
        }

        fn size_allocate(&self, width: i32, height: i32, baseline: i32) {
            self.parent_size_allocate(width, height, baseline);
            if let Some(cb) = &*self.on_resize.borrow() {
                cb(width, height);
            }
        }

        fn snapshot(&self, snapshot: &gtk::Snapshot) {
            let widget = self.obj();
            let (w, h) = (widget.width() as f32, widget.height() as f32);
            snapshot.append_color(&gdk::RGBA::BLACK, &graphene::Rect::new(0.0, 0.0, w, h));
            if self.dirty.replace(false) {
                let tex = self.provider.borrow().as_ref().and_then(|p| p());
                self.texture.replace(tex);
            }
            let Some(tex) = self.texture.borrow().clone() else {
                return;
            };
            let (fw, fh) = (tex.width() as f64, tex.height() as f64);
            let (ox, oy, sx, sy) = render_region(
                (w as f64, h as f64),
                (fw, fh),
                self.scaling.get(),
                self.keep_aspect.get(),
            );
            let rect =
                graphene::Rect::new(ox as f32, oy as f32, (fw * sx) as f32, (fh * sy) as f32);
            snapshot.push_clip(&graphene::Rect::new(0.0, 0.0, w, h));
            if self.scaling.get() {
                snapshot.append_scaled_texture(&tex, gtk::gsk::ScalingFilter::Linear, &rect);
            } else {
                snapshot.append_texture(&tex, &rect);
            }
            if let Some((cur, cx, cy)) = self.overlay.borrow().as_ref().and_then(|p| p()) {
                let rect = graphene::Rect::new(
                    (ox + cx * sx) as f32,
                    (oy + cy * sy) as f32,
                    (cur.width() as f64 * sx) as f32,
                    (cur.height() as f64 * sy) as f32,
                );
                snapshot.append_texture(&cur, &rect);
            }
            snapshot.pop();
        }
    }
}

glib::wrapper! {
    pub struct DisplayWidget(ObjectSubclass<widget_imp::DisplayWidget>)
        @extends gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl Default for DisplayWidget {
    fn default() -> Self {
        let w: DisplayWidget = glib::Object::new();
        w.imp().keep_aspect.set(true);
        w.set_focusable(true);
        w.set_can_focus(true);
        w.set_hexpand(true);
        w.set_vexpand(true);
        w
    }
}

impl DisplayWidget {
    pub fn imp(&self) -> &widget_imp::DisplayWidget {
        gtk::subclass::prelude::ObjectSubclassIsExt::imp(self)
    }

    /// New pixels: rebuild the texture on the next frame.
    pub fn mark_dirty(&self) {
        self.imp().dirty.set(true);
        self.queue_draw();
    }

    pub fn set_scaling(&self, scaling: bool) {
        self.imp().scaling.set(scaling);
        self.queue_draw();
    }

    pub fn set_keep_aspect(&self, keep: bool) {
        self.imp().keep_aspect.set(keep);
        self.queue_draw();
    }

    /// `((min_w, min_h), (nat_w, nat_h))`
    pub fn set_request(&self, min: (i32, i32), natural: (i32, i32)) {
        if self.imp().request.replace((min, natural)) != (min, natural) {
            self.queue_resize();
        }
    }

    /// The natural size last requested.
    pub fn natural_request(&self) -> (i32, i32) {
        self.imp().request.get().1
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_region_modes() {
        // Unscaled, centered in a bigger widget.
        assert_eq!(
            render_region((1000.0, 800.0), (800.0, 600.0), false, true),
            (100.0, 100.0, 1.0, 1.0)
        );
        // Unscaled in a smaller widget: pinned top-left.
        assert_eq!(
            render_region((400.0, 300.0), (800.0, 600.0), false, true),
            (0.0, 0.0, 1.0, 1.0)
        );
        // Scaled keeping aspect: letterboxed.
        assert_eq!(
            render_region((1600.0, 600.0), (800.0, 600.0), true, true),
            (400.0, 0.0, 1.0, 1.0)
        );
        // Scaled, stretched.
        assert_eq!(
            render_region((1600.0, 600.0), (800.0, 600.0), true, false),
            (0.0, 0.0, 2.0, 1.0)
        );
    }
}
