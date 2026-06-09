use std::time::Instant;

use super::{App, SESSION_SAVE_DEBOUNCE};

impl App {
    pub(super) fn schedule_session_save(&mut self) {
        if !self.no_session {
            self.session_save_deadline = Some(Instant::now() + SESSION_SAVE_DEBOUNCE);
        }
    }

    pub(crate) fn sync_session_save_schedule(&mut self) {
        if self.state.session_dirty {
            self.state.session_dirty = false;
            self.schedule_session_save();
        }
    }

    pub(crate) fn save_session_now(&mut self) {
        if self.no_session {
            self.session_save_deadline = None;
            return;
        }

        self.state.collapse_to_single_session();
        if self.state.session().is_none() {
            crate::persist::clear();
        } else {
            let snap = crate::persist::capture(
                self.state.session(),
                &self.state.terminals,
                &self.terminal_runtimes,
                self.state.restore_processes,
            );
            let history = self.persist_pane_history.then(|| {
                crate::persist::capture_history(self.state.session(), &self.terminal_runtimes)
            });
            crate::persist::save(&snap, history.as_ref());
        }

        self.session_save_deadline = None;
    }
}
