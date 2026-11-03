//! A layer in the colour of a cover, drawn by the widget itself. A new colour
//! redraws this layer alone, where a CSS rule reloads the app's stylesheet and
//! restyles the whole window.

use adw::prelude::*;
use adw::subclass::prelude::*;
use gtk::{gdk, glib, graphene, gsk};

use crate::tint;

mod imp {
    use super::*;
    use std::cell::{Cell, RefCell};

    #[derive(Default)]
    pub struct TintLayer {
        /// The cover colour; none draws nothing.
        pub color: Cell<Option<(u8, u8, u8)>>,
        /// Redraws in the colours of the other style (light, dark).
        pub dark_handler: RefCell<Option<glib::SignalHandlerId>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for TintLayer {
        const NAME: &'static str = "VumiTintLayer";
        type Type = super::TintLayer;
        type ParentType = gtk::Widget;
    }

    impl ObjectImpl for TintLayer {
        fn constructed(&self) {
            self.parent_constructed();
            let obj = self.obj();
            obj.set_can_target(false);
            let layer = obj.downgrade();
            let handler = adw::StyleManager::default().connect_dark_notify(move |_| {
                if let Some(layer) = layer.upgrade() {
                    layer.queue_draw();
                }
            });
            self.dark_handler.replace(Some(handler));
        }

        fn dispose(&self) {
            if let Some(handler) = self.dark_handler.take() {
                adw::StyleManager::default().disconnect(handler);
            }
        }
    }

    impl WidgetImpl for TintLayer {
        fn snapshot(&self, snapshot: &gtk::Snapshot) {
            let Some(color) = self.color.get() else {
                return;
            };
            let obj = self.obj();
            let (width, height) = (obj.width() as f32, obj.height() as f32);
            if width <= 0.0 || height <= 0.0 {
                return;
            }
            // The gradient line ends at the last stop, wherever the layer
            // ends: below it the layer stays transparent, as in CSS.
            let stops = tint::page_stops(color, adw::StyleManager::default().is_dark());
            let end = stops[stops.len() - 1].at;
            let color_stops: Vec<gsk::ColorStop> = stops
                .iter()
                .map(|stop| {
                    let [r, g, b, a] = stop.rgba;
                    gsk::ColorStop::new(stop.at / end, gdk::RGBA::new(r, g, b, a))
                })
                .collect();
            snapshot.append_linear_gradient(
                &graphene::Rect::new(0.0, 0.0, width, height),
                &graphene::Point::new(0.0, 0.0),
                &graphene::Point::new(0.0, end),
                &color_stops,
            );
        }
    }
}

glib::wrapper! {
    pub struct TintLayer(ObjectSubclass<imp::TintLayer>)
        @extends gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl TintLayer {
    pub(crate) fn new() -> Self {
        glib::Object::new()
    }

    /// The page gradient in `color` (tint::page_stops); none clears it.
    pub(crate) fn set_page(&self, color: Option<(u8, u8, u8)>) {
        self.imp().color.set(color);
        self.queue_draw();
    }
}
