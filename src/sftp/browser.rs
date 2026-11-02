use super::*;
use std::collections::BTreeSet;
#[derive(Clone, Copy, Default, Debug, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub enum Sort {
    #[default]
    Name,
    Size,
    Modified,
}
pub struct Browser {
    pub path: String,
    pub entries: Vec<Entry>,
    pub selected: BTreeSet<String>,
    pub hidden: bool,
    pub sort: Sort,
    revision: u64,
    posix: bool,
    back: Vec<String>,
    forward: Vec<String>,
}
impl Browser {
    pub fn new(path: String) -> Self {
        Self {
            path,
            entries: vec![],
            selected: BTreeSet::new(),
            hidden: false,
            sort: Sort::Name,
            revision: 0,
            posix: false,
            back: vec![],
            forward: vec![],
        }
    }
    pub fn remote(path: String) -> Self {
        let mut b = Self::new(path);
        b.posix = true;
        b
    }
    pub fn refresh_ticket(&mut self) -> u64 {
        self.revision += 1;
        self.revision
    }
    pub fn accepts(&self, ticket: u64) -> bool {
        self.revision == ticket
    }
    pub fn navigate(&mut self, path: String) {
        if path != self.path {
            self.revision += 1;
            self.back.push(std::mem::replace(&mut self.path, path));
            self.forward.clear();
            self.entries.clear();
            self.selected.clear();
        }
    }
    pub fn back(&mut self) {
        if let Some(p) = self.back.pop() {
            self.revision += 1;
            self.entries.clear();
            self.forward.push(std::mem::replace(&mut self.path, p));
            self.selected.clear();
        }
    }
    pub fn forward(&mut self) {
        if let Some(p) = self.forward.pop() {
            self.revision += 1;
            self.entries.clear();
            self.back.push(std::mem::replace(&mut self.path, p));
            self.selected.clear();
        }
    }
    pub fn up(&mut self) {
        if self.posix {
            let path = self.path.trim_end_matches('/');
            let parent = path.rsplit_once('/').map(|(p, _)| p).unwrap_or("/");
            let parent = if parent.is_empty() { "/" } else { parent }.to_string();
            self.navigate(parent);
            return;
        }
        let p = std::path::Path::new(&self.path)
            .parent()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "/".into());
        self.navigate(if p.is_empty() { "/".into() } else { p });
    }
    pub fn visible(&self) -> Vec<Entry> {
        let mut e: Vec<_> = self
            .entries
            .iter()
            .filter(|e| self.hidden || !e.name.starts_with('.'))
            .cloned()
            .collect();
        e.sort_by(|a, b| {
            let dirs = (a.kind != Kind::Directory).cmp(&(b.kind != Kind::Directory));
            dirs.then_with(|| match self.sort {
                Sort::Name => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
                Sort::Size => a.size.cmp(&b.size),
                Sort::Modified => a.modified.cmp(&b.modified),
            })
        });
        e
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn revisiting_path_rejects_old_refresh() {
        let mut browser = Browser::new("/a".into());
        let ticket = browser.refresh_ticket();
        browser.navigate("/b".into());
        browser.navigate("/a".into());
        assert!(!browser.accepts(ticket));
    }
    #[test]
    fn latest_refresh_wins_even_at_same_path() {
        let mut browser = Browser::new("/a".into());
        let old = browser.refresh_ticket();
        let new = browser.refresh_ticket();
        assert!(!browser.accepts(old));
        assert!(browser.accepts(new));
    }
    #[test]
    fn four_fixed_host_browsers_keep_independent_history_and_selection() {
        let mut w = crate::workspace::Workspace::demo_fixture();
        let mut browsers = vec![];
        for id in 1..=4 {
            let view = w.open(
                crate::workspace::ToolKind::Sftp,
                crate::workspace::TargetMode::Fixed(id),
            );
            browsers.push((view, Browser::new(format!("/host{id}"))));
        }
        browsers[0].1.navigate("/different".into());
        browsers[0].1.selected.insert("file".into());
        w.activate_resource(4);
        assert_eq!(browsers[1].1.path, "/host2");
        assert!(browsers[1].1.selected.is_empty());
        for (i, (id, _)) in browsers.iter().enumerate() {
            assert_eq!(w.target(*id), Some(i as u64 + 1));
        }
    }
}
