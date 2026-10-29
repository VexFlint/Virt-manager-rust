//! Upstream's `virtManager/lib/graphwidgets.py`: the sparkline graph in
//! the manager's stats columns. Upstream's `CellRendererSparkline`
//! becomes a small widget drawing in `snapshot()` (ticket 13), with the
//! same cairo drawing code ported line for line.

use std::cell::{Cell, RefCell};

use gtk::gdk;
use gtk::glib;
use gtk::prelude::*;
use gtk::subclass::prelude::*;

/// `_line_helper`.
fn line_helper(
    cr: &gtk::cairo::Context,
    bottom_baseline: f64,
    points: &[(f64, f64)],
    for_fill: bool,
) -> Option<(f64, f64)> {
    let mut last_was_zero = false;
    let mut last_point = None;
    let n = points.len();
    for (index, &(mut x, mut y)) in points.iter().enumerate() {
        // If stats value == 0, we don't want to draw a line
        let is_zero = y == bottom_baseline;

        // If the line is for filling, alter the coords so that fill covers
        // the same area as the parent sparkline: fill is one pixel short
        // to not overwrite the spark line
        if for_fill {
            if index == 0 {
                x -= 1.0;
            } else if index == n - 1 {
                x += 1.0;
            } else if last_was_zero && is_zero {
                y += 1.0;
            }
        }

        if index == 0 || (last_was_zero && is_zero && !for_fill) {
            cr.move_to(x, y);
        } else {
            cr.line_to(x, y);
            last_point = Some((x, y));
        }
        last_was_zero = is_zero;
    }
    last_point
}

/// `draw_line`.
fn draw_line(cr: &gtk::cairo::Context, y: f64, h: f64, points: &[(f64, f64)]) {
    if points.is_empty() {
        return;
    }
    if line_helper(cr, y + h, points, false).is_some() {
        let _ = cr.stroke();
    } else {
        cr.new_path();
    }
}

/// `draw_fill` (taper=False).
fn draw_fill(cr: &gtk::cairo::Context, x: f64, y: f64, h: f64, points: &[(f64, f64)]) {
    if points.is_empty() {
        return;
    }
    line_helper(cr, y + h, points, true);
    let baseline_y = h + y + 1.0;
    let start_x = points[points.len() - 1].0;
    cr.line_to(start_x + 1.0, baseline_y);
    cr.line_to(x - 1.0, baseline_y);
    let _ = cr.fill();
}

/// `draw_fill` with taper=True: the fill is boxed out to the full
/// width instead of the last point.
fn draw_fill_tapered(
    cr: &gtk::cairo::Context,
    x: f64,
    y: f64,
    w: f64,
    h: f64,
    points: &[(f64, f64)],
) {
    if points.is_empty() {
        return;
    }
    line_helper(cr, y + h, points, true);
    let baseline_y = h + y + 1.0;
    let start_x = w + x;
    cr.line_to(start_x + 1.0, baseline_y);
    cr.line_to(x - 1.0, baseline_y);
    let _ = cr.fill();
}

mod imp {
    use super::*;

    #[derive(Default)]
    pub struct Sparkline {
        pub data_array: RefCell<Vec<f64>>,
        pub reversed: Cell<bool>,
        pub xpad: Cell<i32>,
        pub ypad: Cell<i32>,
        /// The object + handler feeding this graph while it's bound to a
        /// list row; dropped on unbind.
        pub binding: RefCell<Option<(glib::Object, glib::SignalHandlerId)>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for Sparkline {
        const NAME: &'static str = "VmmSparkline";
        type Type = super::Sparkline;
        type ParentType = gtk::Widget;
    }

    impl ObjectImpl for Sparkline {}

    impl WidgetImpl for Sparkline {
        /// `do_get_size`: one pixel per data point, 15px tall, plus pads.
        fn measure(&self, orientation: gtk::Orientation, _for_size: i32) -> (i32, i32, i32, i32) {
            let size = match orientation {
                gtk::Orientation::Horizontal => {
                    self.xpad.get() * 2 + self.data_array.borrow().len() as i32
                }
                _ => self.ypad.get() * 2 + 15,
            };
            (size, size, -1, -1)
        }

        fn snapshot(&self, snapshot: &gtk::Snapshot) {
            let widget = self.obj();
            let (w, h) = (widget.width(), widget.height());
            let bounds = gtk::graphene::Rect::new(0.0, 0.0, w as f32, h as f32);
            let cr = snapshot.append_cairo(&bounds);
            self.render(
                &cr,
                self.xpad.get() as f64,
                self.ypad.get() as f64,
                (w - 2 * self.xpad.get()) as f64,
                (h - 2 * self.ypad.get()) as f64,
                base_color(&*widget),
            );
        }
    }

    impl Sparkline {
        /// `CellRendererSparkline.do_render`, cell_area = (x, y, width,
        /// height); xalign is GTK's default 0.5.
        fn render(
            &self,
            cr: &gtk::cairo::Context,
            mut cell_x: f64,
            cell_y: f64,
            cell_w: f64,
            cell_h: f64,
            base: gdk::RGBA,
        ) {
            // Indent of the gray border around the graph
            const BORDER_PADDING: f64 = 2.0;
            // Indent of graph from border
            const GRAPH_INDENT: f64 = 2.0;
            const GRAPH_PAD: f64 = BORDER_PADDING + GRAPH_INDENT;
            let xalign = 0.5;

            let data = self.data_array.borrow();
            let segments = data.len().saturating_sub(1).max(1) as f64;

            let mut graph_x = cell_x + GRAPH_PAD;
            let graph_y = cell_y + GRAPH_PAD;
            let graph_width = cell_w - GRAPH_PAD * 2.0;
            let graph_height = cell_h - GRAPH_PAD * 2.0;

            let pixels_per_point = (graph_width / segments).floor();
            // Graph width needs to be some multiple of the amount of data
            // points we have
            let graph_width = pixels_per_point * segments;
            // Recalculate border width based on the amount we are graphing
            let border_width = graph_width + GRAPH_INDENT * 2.0;

            // Align the widget
            let empty_space = cell_w - border_width - BORDER_PADDING * 2.0;
            if empty_space != 0.0 {
                let xalign_space = (empty_space * xalign).trunc();
                cell_x += xalign_space;
                graph_x += xalign_space;
            }

            cr.set_line_width(3.0);
            cr.set_line_cap(gtk::cairo::LineCap::Round);

            // Draw gray graph border
            cr.set_source_rgb(0.8828125, 0.8671875, 0.8671875);
            cr.rectangle(
                cell_x + BORDER_PADDING,
                cell_y + BORDER_PADDING,
                border_width,
                cell_h - BORDER_PADDING * 2.0,
            );
            let _ = cr.stroke();

            // Fill in basecolor box inside graph outline
            cr.set_source_rgb(base.red() as f64, base.green() as f64, base.blue() as f64);
            cr.rectangle(
                cell_x + BORDER_PADDING,
                cell_y + BORDER_PADDING,
                border_width,
                cell_h - BORDER_PADDING * 2.0,
            );
            let _ = cr.fill();

            let reversed = self.reversed.get();
            let get_y = |index: usize| {
                let baseline_y = graph_y + graph_height;
                let n = if reversed {
                    data.len() - index - 1
                } else {
                    index
                };
                let y = baseline_y - graph_height * data[n];
                y.max(graph_y).min(graph_y + graph_height)
            };
            let points: Vec<(f64, f64)> = (0..data.len())
                .map(|i| {
                    (
                        (i as f64 * pixels_per_point + graph_x).trunc(),
                        get_y(i).trunc(),
                    )
                })
                .collect();

            // Set color to dark blue for the actual sparkline
            cr.set_line_width(2.0);
            cr.set_source_rgb(0.421875, 0.640625, 0.73046875);
            draw_line(cr, graph_y, graph_height, &points);

            // Set color to light blue for the fill
            cr.set_source_rgba(0.71484375, 0.84765625, 0.89453125, 0.5);
            draw_fill(cr, graph_x, graph_y, graph_height, &points);
        }
    }
}

/// `BASECOLOR = lookup_color("theme_base_color")`. GTK4 only exposes the
/// named theme colors through the deprecated style context.
#[allow(deprecated)]
fn base_color(widget: &impl IsA<gtk::Widget>) -> gdk::RGBA {
    widget
        .style_context()
        .lookup_color("theme_base_color")
        .unwrap_or(gdk::RGBA::WHITE)
}

glib::wrapper! {
    pub struct Sparkline(ObjectSubclass<imp::Sparkline>)
        @extends gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl Default for Sparkline {
    fn default() -> Self {
        glib::Object::new()
    }
}

impl Sparkline {
    /// The manager's column setup: xpad 6, ypad 12, reversed.
    pub fn new_for_vmlist() -> Sparkline {
        let s = Sparkline::default();
        s.imp().xpad.set(6);
        s.imp().ypad.set(12);
        s.imp().reversed.set(true);
        s
    }

    pub fn set_binding(&self, obj: glib::Object, id: glib::SignalHandlerId) {
        self.clear_binding();
        self.imp().binding.replace(Some((obj, id)));
    }

    pub fn clear_binding(&self) {
        if let Some((obj, id)) = self.imp().binding.take() {
            obj.disconnect(id);
        }
    }

    pub fn set_data_array(&self, data: Vec<f64>) {
        let len_changed = self.imp().data_array.borrow().len() != data.len();
        self.imp().data_array.replace(data);
        if len_changed {
            self.queue_resize();
        }
        self.queue_draw();
    }
}

mod graph_imp {
    use super::*;

    pub struct SparklineGraph {
        pub data_array: RefCell<Vec<f64>>,
        pub num_sets: Cell<usize>,
        pub filled: Cell<bool>,
        pub reversed: Cell<bool>,
        pub rgb: RefCell<Vec<f64>>,
    }

    /// `Sparkline.__init__`'s defaults.
    impl Default for SparklineGraph {
        fn default() -> Self {
            SparklineGraph {
                data_array: RefCell::default(),
                num_sets: Cell::new(1),
                filled: Cell::new(true),
                reversed: Cell::new(false),
                rgb: RefCell::default(),
            }
        }
    }

    #[glib::object_subclass]
    impl ObjectSubclass for SparklineGraph {
        const NAME: &'static str = "VmmSparklineGraph";
        type Type = super::SparklineGraph;
        type ParentType = gtk::Widget;

        /// `ctxt.add_class(Gtk.STYLE_CLASS_ENTRY)`: drawn as an entry,
        /// whose CSS background and frame GTK4 renders for the node.
        fn class_init(klass: &mut Self::Class) {
            klass.set_css_name("entry");
        }
    }

    impl ObjectImpl for SparklineGraph {}

    impl WidgetImpl for SparklineGraph {
        /// `Gtk.DrawingArea` requests no size of its own.
        fn measure(&self, _orientation: gtk::Orientation, _for_size: i32) -> (i32, i32, i32, i32) {
            (0, 0, -1, -1)
        }

        fn snapshot(&self, snapshot: &gtk::Snapshot) {
            let widget = self.obj();
            let (w, h) = (widget.width(), widget.height());
            let bounds = gtk::graphene::Rect::new(0.0, 0.0, w as f32, h as f32);
            let cr = snapshot.append_cairo(&bounds);
            self.draw(&cr, w as f64, h as f64, widget.color());
        }
    }

    impl SparklineGraph {
        /// `Sparkline.do_draw`. Without a full `rgb` the lines and fills
        /// keep cairo's default source.
        fn draw(&self, cr: &gtk::cairo::Context, w: f64, h: f64, fg: gdk::RGBA) {
            let data = self.data_array.borrow();
            let num_sets = self.num_sets.get().max(1);
            let points_per_set = data.len() / num_sets;
            let pixels_per_point = w / (points_per_set.saturating_sub(1).max(1) as f64);

            // This draws the marker ticks (`Gtk.render_line`: a 1px line
            // in the foreground color)
            let max_ticks = 4;
            cr.save().ok();
            cr.set_line_width(1.0);
            cr.set_source_rgba(
                fg.red() as f64,
                fg.green() as f64,
                fg.blue() as f64,
                fg.alpha() as f64,
            );
            for index in 1..max_ticks {
                let y = ((h as i64 / max_ticks) * index) as f64 + 0.5;
                cr.move_to(1.5, y);
                cr.line_to(w - 2.0 + 0.5, y);
                let _ = cr.stroke();
            }
            cr.restore().ok();

            // Draw the actual sparkline
            let reversed = self.reversed.get();
            let get_y = |dataset: usize, index: usize| {
                let mut n = dataset * points_per_set;
                if reversed {
                    n += points_per_set - index - 1;
                } else {
                    n += index;
                }
                h - ((h - 1.0) * data[n])
            };
            cr.set_line_width(2.0);
            let rgb = self.rgb.borrow();
            for dataset in 0..num_sets {
                if rgb.len() == num_sets * 3 {
                    // Upstream's blue index is `(dataset * 1) + 2`, so a
                    // second set takes its blue from its own red slot.
                    cr.set_source_rgb(rgb[dataset * 3], rgb[dataset * 3 + 1], rgb[dataset + 2]);
                }
                let mut points: Vec<(f64, f64)> = (0..points_per_set)
                    .map(|i| {
                        let x = i as f64 * pixels_per_point;
                        (x.trunc(), get_y(dataset, i).trunc())
                    })
                    .collect();
                draw_line(cr, 0.0, h, &points);
                if self.filled.get() {
                    // Fixes a fully filled graph from having an oddly
                    // tapered in end (bug 560913).
                    points.insert(0, (0.0, h));
                    draw_fill_tapered(cr, 0.0, 0.0, w, h, &points);
                }
            }
        }
    }
}

glib::wrapper! {
    /// Upstream's `Sparkline` (the `Gtk.DrawingArea` one, not the cell
    /// renderer above): the Connection Details CPU and memory graphs and
    /// the VM details Performance graphs.
    pub struct SparklineGraph(ObjectSubclass<graph_imp::SparklineGraph>)
        @extends gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

/// GTK3 drew the graph over the whole entry, frame included; GTK4 puts
/// a widget's drawing inside its CSS padding, which entries have.
const GRAPH_CSS: &str = "entry.vmm-sparkline-graph { padding: 0; }";

impl Default for SparklineGraph {
    fn default() -> Self {
        thread_local! {
            static CSS: std::cell::OnceCell<()> = const { std::cell::OnceCell::new() };
        }
        CSS.with(|c| {
            c.get_or_init(|| {
                if let Some(display) = gdk::Display::default() {
                    let provider = gtk::CssProvider::new();
                    provider.load_from_string(GRAPH_CSS);
                    gtk::style_context_add_provider_for_display(
                        &display,
                        &provider,
                        gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
                    );
                }
            });
        });
        let graph: SparklineGraph = glib::Object::new();
        graph.add_css_class("vmm-sparkline-graph");
        graph
    }
}

impl SparklineGraph {
    /// `set_property("data_array", ...)`.
    pub fn set_data_array(&self, data: Vec<f64>) {
        self.imp().data_array.replace(data);
        self.queue_draw();
    }

    /// `set_property("reversed", ...)`.
    pub fn set_reversed(&self, reversed: bool) {
        self.imp().reversed.set(reversed);
        self.queue_draw();
    }

    /// `set_property("filled", ...)`.
    pub fn set_filled(&self, filled: bool) {
        self.imp().filled.set(filled);
        self.queue_draw();
    }

    /// `set_property("num_sets", ...)`: 1 or 2 data sets, laid end to
    /// end in `data_array`.
    pub fn set_num_sets(&self, num_sets: usize) {
        self.imp().num_sets.set(num_sets.clamp(1, 2));
        self.queue_draw();
    }

    /// `set_property("rgb", ...)`: three 0..1 components per data set.
    pub fn set_rgb(&self, rgb: Vec<f64>) {
        self.imp().rgb.replace(rgb);
        self.queue_draw();
    }
}
