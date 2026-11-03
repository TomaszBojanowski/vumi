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

    /// A new layer under `widget`, and so under everything it shows, its
    /// header bar too: `widget` moves into an overlay over the layer, in its
    /// own place. For a widget in an AdwBreakpointBin or an AdwNavigationPage
    /// (a page's toolbar view); None elsewhere.
    pub(crate) fn under(widget: &impl IsA<gtk::Widget>) -> Option<Self> {
        let widget = widget.upcast_ref::<gtk::Widget>().clone();
        let parent = widget.parent()?;
        let layer = Self::new();
        let overlay = gtk::Overlay::new();
        overlay.set_child(Some(&layer));
        let reparent = |set_child: &dyn Fn(Option<&gtk::Widget>)| {
            set_child(None);
            overlay.add_overlay(&widget);
            overlay.set_measure_overlay(&widget, true);
            set_child(Some(overlay.upcast_ref()));
        };
        if let Some(bin) = parent.downcast_ref::<adw::BreakpointBin>() {
            reparent(&|child| bin.set_child(child));
        } else {
            let page = parent.downcast_ref::<adw::NavigationPage>()?;
            reparent(&|child| page.set_child(child));
        }
        Some(layer)
    }
}

/// Adwaita's corner radius of a card: the colour fills the card's rounded box.
const CARD_RADIUS: f32 = 12.0;

/// Colours as red, green, blue and alpha, 0 to 1.
type Rgba = [f32; 4];

mod card_imp {
    use super::*;
    use std::cell::{Cell, RefCell};

    #[derive(Default)]
    pub struct TintCard {
        /// The cover colour the card shows, if any.
        pub color: Cell<Option<(u8, u8, u8)>>,
        /// What is drawn now, over the card's own background (alpha 0: none).
        pub shown: Cell<Rgba>,
        pub animation: RefCell<Option<adw::TimedAnimation>>,
        pub dark_handler: RefCell<Option<glib::SignalHandlerId>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for TintCard {
        const NAME: &'static str = "VumiTintCard";
        type Type = super::TintCard;
        type ParentType = gtk::Box;
    }

    impl ObjectImpl for TintCard {
        fn constructed(&self) {
            self.parent_constructed();
            let card = self.obj().downgrade();
            // The other style has its own mix (tint::card_color): at once.
            let handler = adw::StyleManager::default().connect_dark_notify(move |_| {
                if let Some(card) = card.upgrade() {
                    let target = card.target();
                    card.imp().shown.set(target);
                    card.queue_draw();
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

    impl WidgetImpl for TintCard {
        /// The colour over the card's background and under its children, on
        /// the card's whole box: its padding too, which width() and height()
        /// leave out.
        fn snapshot(&self, snapshot: &gtk::Snapshot) {
            let [r, g, b, a] = self.shown.get();
            let obj = self.obj();
            if a > 0.0
                && let Some(rect) = obj.compute_bounds(&*obj)
            {
                snapshot.push_rounded_clip(&gsk::RoundedRect::from_rect(rect, CARD_RADIUS));
                snapshot.append_color(&gdk::RGBA::new(r, g, b, a), &rect);
                snapshot.pop();
            }
            self.parent_snapshot(snapshot);
        }
    }

    impl BoxImpl for TintCard {}
}

glib::wrapper! {
    /// A card (gtk::Box) in the colour of a cover, drawn by itself.
    pub struct TintCard(ObjectSubclass<card_imp::TintCard>)
        @extends gtk::Box, gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget, gtk::Orientable;
}

impl TintCard {
    /// The colour the card should show now: none is the shown colour, clear.
    fn target(&self) -> Rgba {
        let dark = adw::StyleManager::default().is_dark();
        match self.imp().color.get() {
            Some(color) => tint::card_color(color, dark),
            None => {
                let [r, g, b, _] = self.imp().shown.get();
                [r, g, b, 0.0]
            }
        }
    }

    /// The card in `color` (tint::card_color), or plain: a quarter of a
    /// second from the colour shown, as a CSS transition of the background
    /// would, and following "Reduce animations" in GNOME.
    pub(crate) fn set_card(&self, color: Option<(u8, u8, u8)>) {
        let imp = self.imp();
        imp.color.set(color);
        // The card's own background stays under the colour while it changes.
        self.remove_css_class("tint-drawn");
        let to = self.target();
        let mut from = imp.shown.get();
        if from[3] == 0.0 {
            // From plain: the colour comes in over the card's background.
            from = [to[0], to[1], to[2], 0.0];
        }
        if let Some(animation) = imp.animation.take() {
            animation.pause();
        }
        let card = self.downgrade();
        let target = adw::CallbackAnimationTarget::new(move |t| {
            if let Some(card) = card.upgrade() {
                let t = t as f32;
                let mix: Rgba = std::array::from_fn(|i| from[i] + (to[i] - from[i]) * t);
                card.imp().shown.set(mix);
                card.queue_draw();
            }
        });
        let animation = adw::TimedAnimation::builder()
            .widget(self)
            .target(&target)
            .value_from(0.0)
            .value_to(1.0)
            .duration(250)
            .easing(adw::Easing::Ease)
            .build();
        // Once in, the colour alone fills the card: its rounded edges are
        // blended once, as a CSS background's are (style.css: .tint-drawn).
        let card = self.downgrade();
        animation.connect_done(move |_| {
            if let Some(card) = card.upgrade()
                && card.imp().color.get().is_some()
            {
                card.add_css_class("tint-drawn");
            }
        });
        animation.play();
        imp.animation.replace(Some(animation));
    }
}
