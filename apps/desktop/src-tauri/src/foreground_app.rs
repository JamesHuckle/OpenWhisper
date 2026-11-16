//! Remembers the name of the program the user was last in, by process id.
//!
//! Windows reports every change of the focused window. Looking the program's
//! name up opens its process, so the name is looked up once per process and
//! the settings menu is told only when the name really changes. This file has
//! no dependencies so it can also be tested on its own with `rustc --test`.

#[derive(Debug, Default)]
pub struct ForegroundApp {
    /// The process whose name is remembered, and that name.
    looked_up: Option<(u32, Option<String>)>,
    /// The last name sent to the menu; `None` before the first one.
    reported: Option<Option<String>>,
}

impl ForegroundApp {
    pub const fn new() -> Self {
        Self {
            looked_up: None,
            reported: None,
        }
    }

    /// The program name for `pid`, calling `look_up` only for a new process.
    pub fn name_for(
        &mut self,
        pid: u32,
        look_up: impl FnOnce(u32) -> Option<String>,
    ) -> Option<String> {
        if pid == 0 {
            return None;
        }
        match &self.looked_up {
            Some((known, name)) if *known == pid => name.clone(),
            _ => {
                let name = look_up(pid);
                self.looked_up = Some((pid, name.clone()));
                name
            }
        }
    }

    /// `Some(name)` when `name` differs from the last one sent to the menu.
    pub fn report(&mut self, name: Option<String>) -> Option<Option<String>> {
        if self.reported.as_ref() == Some(&name) {
            return None;
        }
        self.reported = Some(name.clone());
        Some(name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    #[test]
    fn looks_a_process_up_once_until_another_process_has_focus() {
        let lookups = Cell::new(0);
        let look_up = |pid: u32| {
            lookups.set(lookups.get() + 1);
            Some(format!("app{pid}"))
        };
        let mut app = ForegroundApp::default();

        assert_eq!(app.name_for(10, look_up), Some("app10".into()));
        assert_eq!(app.name_for(10, look_up), Some("app10".into()));
        assert_eq!(lookups.get(), 1);

        assert_eq!(app.name_for(20, look_up), Some("app20".into()));
        assert_eq!(app.name_for(10, look_up), Some("app10".into()));
        assert_eq!(lookups.get(), 3);
    }

    #[test]
    fn remembers_a_process_whose_name_cannot_be_read() {
        let lookups = Cell::new(0);
        let look_up = |_pid: u32| {
            lookups.set(lookups.get() + 1);
            None
        };
        let mut app = ForegroundApp::default();

        assert_eq!(app.name_for(7, look_up), None);
        assert_eq!(app.name_for(7, look_up), None);
        assert_eq!(lookups.get(), 1);
    }

    #[test]
    fn no_process_has_no_name_and_is_not_looked_up() {
        let mut app = ForegroundApp::default();
        assert_eq!(app.name_for(0, |_| panic!("looked up pid 0")), None);
    }

    #[test]
    fn tells_the_menu_only_when_the_name_changes() {
        let mut app = ForegroundApp::default();

        assert_eq!(app.report(None), Some(None));
        assert_eq!(app.report(None), None);
        assert_eq!(app.report(Some("Code".into())), Some(Some("Code".into())));
        assert_eq!(app.report(Some("Code".into())), None);
        assert_eq!(
            app.report(Some("chrome".into())),
            Some(Some("chrome".into()))
        );
        assert_eq!(app.report(Some("Code".into())), Some(Some("Code".into())));
    }
}
