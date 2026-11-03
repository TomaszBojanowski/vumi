//! Grids that get their items in parts. A GtkGridView makes a card for every
//! item within about 32 rows of what it shows, all in one frame: with a few
//! hundred artists that is one frame of well over 100 ms. Here a grid gets its
//! items once it is first on screen: the first screen at once, then a few rows
//! a frame.

use std::cell::RefCell;
use std::rc::Rc;

use gtk::prelude::*;
use gtk::{gio, glib};

/// Rows of cards a GridView keeps around what it shows: 30, and one above and
/// one below (GTK_GRID_VIEW_MAX_VISIBLE_ROWS in gtkgridview.c), with room to
/// spare. Past that many items it makes no more cards, so the rest goes at once.
const KEPT_ROWS: u32 = 33;

/// How many of `total` items a grid shows after `shown`: `first` to begin
/// with, then `step` more; all of them (`u32::MAX`, the fill is over) once
/// that reaches `limit` or `total`. Nothing while there is nothing.
pub(crate) fn next_size(shown: u32, total: u32, first: u32, step: u32, limit: u32) -> u32 {
    if total == 0 {
        return shown;
    }
    let next = if shown == 0 {
        first
    } else {
        shown.saturating_add(step)
    };
    if next >= total.min(limit) {
        u32::MAX
    } else {
        next
    }
}

struct Fill {
    store: gio::ListStore,
    /// What the grid shows: the first items of `store`.
    slice: gtk::SliceListModel,
    first: u32,
    step: u32,
    tick: RefCell<Option<gtk::TickCallbackId>>,
}

impl Fill {
    fn done(&self) -> bool {
        self.slice.size() == u32::MAX
    }

    fn grow(&self, grid: &gtk::GridView) {
        let limit = grid.max_columns().saturating_mul(KEPT_ROWS);
        let size = next_size(
            self.slice.size(),
            self.store.n_items(),
            self.first,
            self.step,
            limit,
        );
        self.slice.set_size(size);
    }

    /// The first part now, then one a frame until all are there.
    fn start(self: &Rc<Self>, grid: &gtk::GridView) {
        if self.done() || self.store.n_items() == 0 || self.tick.borrow().is_some() {
            return;
        }
        self.grow(grid);
        if self.done() {
            return;
        }
        let fill = Rc::downgrade(self);
        let id = grid.add_tick_callback(move |grid, _| {
            let Some(fill) = fill.upgrade() else {
                return glib::ControlFlow::Break;
            };
            if !fill.done() {
                fill.grow(grid);
            }
            if fill.done() {
                // Ends here: GTK drops the callback.
                fill.tick.take();
                glib::ControlFlow::Break
            } else {
                glib::ControlFlow::Continue
            }
        });
        self.tick.replace(Some(id));
    }

    /// Waits while the grid is hidden: no frames for nothing.
    fn stop(&self) {
        if let Some(id) = self.tick.take() {
            id.remove();
        }
    }
}

/// Gives `grid` the items of `store` once it is first on screen: `first` of
/// them at once (about a screen), then `step` more a frame while it stays on
/// screen. An emptied store (logging out) starts over.
pub(crate) fn attach(grid: &gtk::GridView, store: &gio::ListStore, first: u32, step: u32) {
    let fill = Rc::new(Fill {
        store: store.clone(),
        slice: gtk::SliceListModel::new(Some(store.clone()), 0, 0),
        first,
        step,
        tick: RefCell::default(),
    });

    let f = fill.clone();
    grid.connect_map(move |grid| {
        if grid.model().is_none() {
            grid.set_model(Some(&gtk::NoSelection::new(Some(f.slice.clone()))));
        }
        f.start(grid);
    });
    let f = fill.clone();
    grid.connect_unmap(move |_| f.stop());

    let f = Rc::downgrade(&fill);
    let g = grid.downgrade();
    store.connect_items_changed(move |store, _, _, _| {
        let (Some(fill), Some(grid)) = (f.upgrade(), g.upgrade()) else {
            return;
        };
        if store.n_items() == 0 {
            fill.stop();
            fill.slice.set_size(0);
        } else if grid.is_mapped() {
            fill.start(&grid);
        }
    });
}

/// Shows all of a grid's items at once (a jump to a letter needs them).
/// True if some were still to come: they are laid out in the next frame.
pub(crate) fn finish(grid: &gtk::GridView) -> bool {
    let slice = grid
        .model()
        .and_downcast::<gtk::NoSelection>()
        .and_then(|selection| selection.model())
        .and_downcast::<gtk::SliceListModel>();
    match slice {
        Some(slice) if slice.size() != u32::MAX => {
            slice.set_size(u32::MAX);
            true
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_first_part_then_a_step_a_frame_until_all_are_there() {
        // 266 artists, 36 first, 7 a frame, cards for 396 at most.
        assert_eq!(next_size(0, 266, 36, 7, 396), 36);
        assert_eq!(next_size(36, 266, 36, 7, 396), 43);
        assert_eq!(next_size(252, 266, 36, 7, 396), 259);
        assert_eq!(next_size(259, 266, 36, 7, 396), u32::MAX);
    }

    #[test]
    fn past_the_cards_the_grid_keeps_the_rest_comes_at_once() {
        // 651 albums: once 396 are there no more cards are made.
        assert_eq!(next_size(366, 651, 60, 30, 396), u32::MAX);
        assert_eq!(next_size(336, 651, 60, 30, 396), 366);
    }

    #[test]
    fn a_short_list_comes_whole_and_an_empty_one_waits() {
        assert_eq!(next_size(0, 20, 36, 7, 396), u32::MAX);
        assert_eq!(next_size(0, 36, 36, 7, 396), u32::MAX);
        assert_eq!(next_size(0, 0, 36, 7, 396), 0);
    }
}
