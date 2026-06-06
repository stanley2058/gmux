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

        if self.state.sessions().is_empty() {
            crate::persist::clear();
        } else {
            self.state.collapse_to_single_session();
            let snap = crate::persist::capture(
                self.state.sessions(),
                &self.state.terminals,
                &self.terminal_runtimes,
            );
            let history = self.persist_pane_history.then(|| {
                crate::persist::capture_history(self.state.sessions(), &self.terminal_runtimes)
            });
            crate::persist::save(&snap, history.as_ref());
        }

        self.session_save_deadline = None;
    }
}
