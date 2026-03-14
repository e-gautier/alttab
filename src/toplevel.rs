use wayland_client::protocol::wl_output;
use wayland_client::{Connection, Dispatch, QueueHandle};
use wayland_protocols_wlr::foreign_toplevel::v1::client::{
    zwlr_foreign_toplevel_handle_v1::{self, ZwlrForeignToplevelHandleV1},
    zwlr_foreign_toplevel_manager_v1::{self, ZwlrForeignToplevelManagerV1},
};

use crate::AppState;

/// Information about a single toplevel window.
#[derive(Debug, Clone, Default)]
pub struct ToplevelInfo {
    pub title: String,
    pub app_id: String,
    pub is_activated: bool,
    pub is_minimized: bool,
    pub is_fullscreen: bool,
    pub is_maximized: bool,
}

/// Tracks all open toplevel windows with MRU ordering.
#[derive(Debug)]
pub struct ToplevelState {
    /// (handle, pending info, committed info)
    pub toplevels: Vec<(ZwlrForeignToplevelHandleV1, ToplevelInfo, ToplevelInfo)>,
    pub manager: Option<ZwlrForeignToplevelManagerV1>,
    /// MRU order: stores handle IDs from most-recently-activated to least.
    /// The front of the vec is the most recently activated window.
    pub mru_order: Vec<u32>,
    /// Monotonic counter for assigning unique IDs to handles.
    next_id: u32,
    /// Map handle to its assigned ID.
    pub handle_ids: Vec<(ZwlrForeignToplevelHandleV1, u32)>,
    /// Track which outputs each toplevel is on (by handle ID).
    pub handle_outputs: Vec<(u32, Vec<wl_output::WlOutput>)>,
}

impl ToplevelState {
    pub fn new() -> Self {
        Self {
            toplevels: Vec::new(),
            manager: None,
            mru_order: Vec::new(),
            next_id: 0,
            handle_ids: Vec::new(),
            handle_outputs: Vec::new(),
        }
    }

    fn assign_id(&mut self, handle: &ZwlrForeignToplevelHandleV1) -> u32 {
        let id = self.next_id;
        self.next_id += 1;
        self.handle_ids.push((handle.clone(), id));
        self.handle_outputs.push((id, Vec::new()));
        // New windows go to the back of MRU
        self.mru_order.push(id);
        id
    }

    fn get_id(&self, handle: &ZwlrForeignToplevelHandleV1) -> Option<u32> {
        self.handle_ids
            .iter()
            .find(|(h, _)| h == handle)
            .map(|(_, id)| *id)
    }

    /// Move a handle to the front of the MRU order (most recently used).
    pub fn touch_mru_by_id(&mut self, id: u32) {
        self.mru_order.retain(|&x| x != id);
        self.mru_order.insert(0, id);
    }

    fn handle_for_id(&self, id: u32) -> Option<&ZwlrForeignToplevelHandleV1> {
        self.handle_ids
            .iter()
            .find(|(_, i)| *i == id)
            .map(|(h, _)| h)
    }

    /// Get the window list sorted by MRU order.
    /// Returns (handle, committed info) pairs.
    pub fn window_list(&self) -> Vec<(&ZwlrForeignToplevelHandleV1, &ToplevelInfo)> {
        let mut result: Vec<(&ZwlrForeignToplevelHandleV1, &ToplevelInfo)> = Vec::new();

        // Add windows in MRU order
        for &id in &self.mru_order {
            if let Some(handle) = self.handle_for_id(id) {
                if let Some((_, _, committed)) = self.toplevels.iter().find(|(h, _, _)| h == handle)
                {
                    if !committed.title.is_empty() {
                        result.push((handle, committed));
                    }
                }
            }
        }

        result
    }

    fn find_pending_mut(
        &mut self,
        handle: &ZwlrForeignToplevelHandleV1,
    ) -> Option<&mut ToplevelInfo> {
        self.toplevels
            .iter_mut()
            .find(|(h, _, _)| h == handle)
            .map(|(_, pending, _)| pending)
    }

    fn commit(&mut self, handle: &ZwlrForeignToplevelHandleV1) {
        // Check if this window just became activated - if so, move to front of MRU
        let activated_id = self
            .toplevels
            .iter()
            .find(|(h, _, _)| h == handle)
            .and_then(|(h, pending, committed)| {
                if pending.is_activated && !committed.is_activated {
                    self.get_id(h)
                } else {
                    None
                }
            });

        if let Some((_, pending, committed)) =
            self.toplevels.iter_mut().find(|(h, _, _)| h == handle)
        {
            *committed = pending.clone();
        }

        if let Some(id) = activated_id {
            log::debug!("Window activated, moving to front of MRU: id={}", id);
            self.touch_mru_by_id(id);
        }
    }

    fn remove(&mut self, handle: &ZwlrForeignToplevelHandleV1) {
        if let Some(id) = self.get_id(handle) {
            self.mru_order.retain(|&x| x != id);
            self.handle_outputs.retain(|(i, _)| *i != id);
        }
        self.handle_ids.retain(|(h, _)| h != handle);
        self.toplevels.retain(|(h, _, _)| h != handle);
    }

    /// Call after the initial roundtrip to ensure the currently-activated window
    /// is at the front of the MRU list.
    pub fn sort_initial_mru(&mut self) {
        let activated_id = self
            .toplevels
            .iter()
            .find(|(_, _, committed)| committed.is_activated)
            .and_then(|(handle, _, _)| self.get_id(handle));

        if let Some(id) = activated_id {
            self.touch_mru_by_id(id);
        }
    }

    /// Record that a toplevel entered an output.
    pub fn output_enter(
        &mut self,
        handle: &ZwlrForeignToplevelHandleV1,
        output: wl_output::WlOutput,
    ) {
        if let Some(id) = self.get_id(handle) {
            if let Some((_, outputs)) = self.handle_outputs.iter_mut().find(|(i, _)| *i == id) {
                if !outputs.iter().any(|o| o == &output) {
                    outputs.push(output);
                }
            }
        }
    }

    /// Record that a toplevel left an output.
    pub fn output_leave(
        &mut self,
        handle: &ZwlrForeignToplevelHandleV1,
        output: &wl_output::WlOutput,
    ) {
        if let Some(id) = self.get_id(handle) {
            if let Some((_, outputs)) = self.handle_outputs.iter_mut().find(|(i, _)| *i == id) {
                outputs.retain(|o| o != output);
            }
        }
    }

    /// Get the output of the currently focused (MRU-front) window, if any.
    /// Returns the first output the focused window is on.
    pub fn focused_output(&self) -> Option<&wl_output::WlOutput> {
        let front_id = self.mru_order.first()?;
        let (_, outputs) = self.handle_outputs.iter().find(|(id, _)| id == front_id)?;
        outputs.first()
    }
}

fn parse_states(raw: &[u8]) -> (bool, bool, bool, bool) {
    let mut maximized = false;
    let mut minimized = false;
    let mut activated = false;
    let mut fullscreen = false;
    for chunk in raw.chunks_exact(4) {
        let val = u32::from_ne_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
        match val {
            0 => maximized = true,
            1 => minimized = true,
            2 => activated = true,
            3 => fullscreen = true,
            _ => {}
        }
    }
    (maximized, minimized, activated, fullscreen)
}

// Dispatch for the manager - receives new toplevel handles
impl Dispatch<ZwlrForeignToplevelManagerV1, ()> for AppState {
    fn event(
        state: &mut Self,
        _proxy: &ZwlrForeignToplevelManagerV1,
        event: zwlr_foreign_toplevel_manager_v1::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        match event {
            zwlr_foreign_toplevel_manager_v1::Event::Toplevel { toplevel } => {
                log::debug!("New toplevel handle created");
                state.toplevel_state.assign_id(&toplevel);
                state.toplevel_state.toplevels.push((
                    toplevel,
                    ToplevelInfo::default(),
                    ToplevelInfo::default(),
                ));
            }
            zwlr_foreign_toplevel_manager_v1::Event::Finished => {
                log::warn!("Foreign toplevel manager finished");
            }
            _ => {}
        }
    }

    wayland_client::event_created_child!(AppState, ZwlrForeignToplevelManagerV1, [
        zwlr_foreign_toplevel_manager_v1::EVT_TOPLEVEL_OPCODE => (ZwlrForeignToplevelHandleV1, ()),
    ]);
}

// Dispatch for individual toplevel handles - receives property updates
impl Dispatch<ZwlrForeignToplevelHandleV1, ()> for AppState {
    fn event(
        state: &mut Self,
        proxy: &ZwlrForeignToplevelHandleV1,
        event: zwlr_foreign_toplevel_handle_v1::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        match event {
            zwlr_foreign_toplevel_handle_v1::Event::Title { title } => {
                if let Some(info) = state.toplevel_state.find_pending_mut(proxy) {
                    info.title = title;
                }
            }
            zwlr_foreign_toplevel_handle_v1::Event::AppId { app_id } => {
                if let Some(info) = state.toplevel_state.find_pending_mut(proxy) {
                    info.app_id = app_id;
                }
            }
            zwlr_foreign_toplevel_handle_v1::Event::State { state: raw_state } => {
                if let Some(info) = state.toplevel_state.find_pending_mut(proxy) {
                    let (maximized, minimized, activated, fullscreen) = parse_states(&raw_state);
                    info.is_maximized = maximized;
                    info.is_minimized = minimized;
                    info.is_activated = activated;
                    info.is_fullscreen = fullscreen;
                }
            }
            zwlr_foreign_toplevel_handle_v1::Event::Done => {
                state.toplevel_state.commit(proxy);
            }
            zwlr_foreign_toplevel_handle_v1::Event::Closed => {
                log::debug!("Toplevel closed");
                state.toplevel_state.remove(proxy);
                proxy.destroy();
                // If overlay is showing, we may need to adjust selection
                if state.overlay_visible {
                    let count = state.toplevel_state.window_list().len();
                    if count < 2 {
                        // Not enough windows to switch, close overlay
                        state.close_overlay(false);
                    } else {
                        if state.selected_index >= count {
                            state.selected_index = count - 1;
                        }
                        state.needs_redraw = true;
                    }
                }
            }
            zwlr_foreign_toplevel_handle_v1::Event::OutputEnter { output } => {
                state.toplevel_state.output_enter(proxy, output);
            }
            zwlr_foreign_toplevel_handle_v1::Event::OutputLeave { output } => {
                state.toplevel_state.output_leave(proxy, &output);
            }
            zwlr_foreign_toplevel_handle_v1::Event::Parent { .. } => {}
            _ => {}
        }
    }
}
