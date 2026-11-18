//! Pages and navigation (docs/EDITOR.md E9.1). A page is a [`Route`] with a path; [`Nav`] keeps
//! the history. Pure: no window needed to test it.

use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Route {
    Home,
    NewProject,
    OpenProject,
    /// The project folder to delete. The path is not part of the page path.
    DeleteProject(PathBuf),
    Settings,
    Editor,
}

impl Route {
    pub fn path(&self) -> &'static str {
        match self {
            Route::Home => "/home",
            Route::NewProject => "/project/new",
            Route::OpenProject => "/project/open",
            Route::DeleteProject(_) => "/project/delete",
            Route::Settings => "/settings",
            Route::Editor => "/editor",
        }
    }

    /// Unknown paths give `Home`. `/project/delete` parses with an empty folder; [`resolve`]
    /// sends that back to `Home`.
    pub fn parse(path: &str) -> Route {
        match path {
            "/home" => Route::Home,
            "/project/new" => Route::NewProject,
            "/project/open" => Route::OpenProject,
            "/project/delete" => Route::DeleteProject(PathBuf::new()),
            "/settings" => Route::Settings,
            "/editor" => Route::Editor,
            _ => Route::Home,
        }
    }
}

/// The page that is really shown for `route`: `/editor` needs a project (or a game given on the
/// command line), and the delete page needs a folder.
pub fn resolve(route: Route, has_project: bool) -> Route {
    match route {
        Route::Editor if !has_project => Route::Home,
        Route::DeleteProject(p) if p.as_os_str().is_empty() => Route::Home,
        r => r,
    }
}

/// Browser-style history: a stack with a position.
#[derive(Debug, Clone)]
pub struct Nav {
    stack: Vec<Route>,
    pos: usize,
}

impl Nav {
    pub fn new(start: Route) -> Self {
        Self { stack: vec![start], pos: 0 }
    }

    pub fn current(&self) -> &Route {
        &self.stack[self.pos]
    }

    /// Go to `route` (after the redirect rule). Going to the page already shown adds nothing;
    /// otherwise the forward history is dropped.
    pub fn navigate(&mut self, route: Route, has_project: bool) {
        let route = resolve(route, has_project);
        if *self.current() == route {
            return;
        }
        self.stack.truncate(self.pos + 1);
        self.stack.push(route);
        self.pos += 1;
    }

    pub fn can_back(&self) -> bool {
        self.pos > 0
    }

    pub fn can_forward(&self) -> bool {
        self.pos + 1 < self.stack.len()
    }

    /// One page back; the entry reached is redirected again if it no longer applies.
    pub fn back(&mut self, has_project: bool) -> bool {
        if !self.can_back() {
            return false;
        }
        self.pos -= 1;
        self.fix(has_project);
        true
    }

    pub fn forward(&mut self, has_project: bool) -> bool {
        if !self.can_forward() {
            return false;
        }
        self.pos += 1;
        self.fix(has_project);
        true
    }

    fn fix(&mut self, has_project: bool) {
        let r = resolve(self.stack[self.pos].clone(), has_project);
        self.stack[self.pos] = r;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_path_round_trip() {
        for r in [Route::Home, Route::NewProject, Route::OpenProject, Route::Settings, Route::Editor] {
            assert_eq!(Route::parse(r.path()), r);
        }
        assert_eq!(Route::parse("/project/delete").path(), "/project/delete");
        assert_eq!(Route::DeleteProject("/x".into()).path(), "/project/delete");
        assert_eq!(Route::parse("/nope"), Route::Home);
        assert_eq!(Route::parse(""), Route::Home);
        assert_eq!(Route::parse("/editor/"), Route::Home);
    }

    #[test]
    fn editor_needs_a_project() {
        assert_eq!(resolve(Route::Editor, false), Route::Home);
        assert_eq!(resolve(Route::Editor, true), Route::Editor);
        assert_eq!(resolve(Route::DeleteProject(PathBuf::new()), true), Route::Home);
        assert_eq!(resolve(Route::DeleteProject("/p".into()), false), Route::DeleteProject("/p".into()));
        let mut n = Nav::new(Route::Home);
        n.navigate(Route::Editor, false);
        assert_eq!(*n.current(), Route::Home);
        assert!(!n.can_back());
    }

    #[test]
    fn back_and_forward() {
        let mut n = Nav::new(Route::Home);
        n.navigate(Route::Settings, false);
        n.navigate(Route::NewProject, false);
        assert!(!n.can_forward());
        assert!(n.back(false));
        assert_eq!(*n.current(), Route::Settings);
        assert!(n.forward(false));
        assert_eq!(*n.current(), Route::NewProject);
        assert!(!n.forward(false));
        n.back(false);
        n.back(false);
        assert_eq!(*n.current(), Route::Home);
        assert!(!n.back(false));
        // A new page drops the forward history.
        n.navigate(Route::OpenProject, false);
        assert!(!n.can_forward());
        assert!(n.back(false));
        assert_eq!(*n.current(), Route::Home);
        // The same page twice adds nothing.
        n.navigate(Route::Home, false);
        assert!(n.forward(false));
        assert!(!n.can_forward());
    }

    #[test]
    fn history_entry_is_redirected_when_the_project_is_gone() {
        let mut n = Nav::new(Route::Home);
        n.navigate(Route::Editor, true);
        n.navigate(Route::Settings, true);
        n.back(false);
        assert_eq!(*n.current(), Route::Home);
    }
}
