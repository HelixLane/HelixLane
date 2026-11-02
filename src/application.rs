//! Process-owned presentation registry and launch service. No window owns the IPC endpoint.
use crate::app::{HelixLane, Panels, Shared};
use gpui_kit::{
    component::{TitleBar, dock::*},
    *,
};
use helixlane::workspace::WindowId;
use helixlane::{
    launch::{OpenMode, OpenRequest},
    settings::Settings,
    workspace::*,
};
use std::{cell::RefCell, collections::HashMap, rc::Rc};

#[derive(Clone)]
pub struct ApplicationOwner(Rc<Owner>);
struct Owner {
    settings: Shared,
    path: RefCell<Option<std::path::PathBuf>>,
    panels: Panels,
    windows: RefCell<HashMap<WindowId, (AnyWindowHandle, WeakEntity<HelixLane>)>>,
}
impl Global for ApplicationOwner {}
impl ApplicationOwner {
    pub fn install(cx: &mut App) -> Self {
        let settings = Settings::load(&Settings::path()).unwrap_or_else(|error| Settings {
            recovery: Some(error),
            ..Settings::default()
        });
        let owner = Self::install_settings(settings, cx);
        owner.0.path.replace(Some(Settings::path()));
        owner
    }
    fn install_settings(mut settings: Settings, cx: &mut App) -> Self {
        settings.workspace.ensure_windows();
        if let Some(primary) = settings.workspace.windows.iter_mut().find(|w| w.id == 1) {
            if primary.dock_layout.is_none() {
                primary.dock_layout = settings.dock_layout.take();
            }
            if primary.top_layout.is_none() {
                primary.top_layout = settings.top_layout.take();
            }
        }
        let owner = Self(Rc::new(Owner {
            settings: Rc::new(RefCell::new(settings)),
            path: RefCell::new(None),
            panels: Default::default(),
            windows: Default::default(),
        }));
        cx.set_global(owner.clone());
        let factory = owner.clone();
        register_panel(cx, "helixlane.tool", move |context, _, cx| {
            let id = match context.info() {
                PanelInfo::Panel(v) => v.as_u64().unwrap_or_default(),
                _ => 0,
            };
            if let Some(panel) = factory.0.panels.borrow().get(&id).cloned() {
                return panel_handle(panel);
            }
            let window = factory.settings().borrow().workspace.owner(id).unwrap_or(1);
            let owning = factory
                .0
                .windows
                .borrow()
                .get(&window)
                .map(|(_, w)| w.clone())
                .expect("window registered before dock restoration");
            let panel = cx.new(|cx| crate::app::ToolPanel::new(id, factory.settings(), owning, cx));
            factory.0.panels.borrow_mut().insert(id, panel.clone());
            panel_handle(panel)
        });
        owner
    }
    pub fn settings(&self) -> Shared {
        self.0.settings.clone()
    }
    pub fn panels(&self) -> Panels {
        self.0.panels.clone()
    }
    pub fn save(&self) -> Result<(), String> {
        match self.0.path.borrow().as_ref() {
            Some(path) => self.0.settings.borrow().save(path),
            None => Ok(()),
        }
    }
    pub fn register(&self, id: WindowId, handle: AnyWindowHandle, view: WeakEntity<HelixLane>) {
        self.0.windows.borrow_mut().insert(id, (handle, view));
    }
    pub fn window(&self, id: WindowId) -> Option<(AnyWindowHandle, WeakEntity<HelixLane>)> {
        self.0.windows.borrow().get(&id).cloned()
    }
    pub fn windows(&self) -> Vec<WindowId> {
        let mut ids = self.0.windows.borrow().keys().copied().collect::<Vec<_>>();
        ids.sort_unstable();
        ids
    }
    pub fn closed(&self, id: WindowId, cx: &mut App) {
        self.0.windows.borrow_mut().remove(&id);
        self.0.settings.borrow_mut().workspace.close_window(id);
        let _ = self.save();
        cx.refresh_windows();
    }
    pub fn open(&self, id: WindowId, cx: &mut App) -> Result<(), String> {
        if self.window(id).is_some() {
            return Ok(());
        }
        let mut titlebar = TitleBar::title_bar_options();
        if let Some(position) = titlebar.traffic_light_position.as_mut() {
            position.y +=
                (px(crate::theme::TOOLBAR_HEIGHT) - gpui_kit::component::TITLE_BAR_HEIGHT) * 0.5;
        }
        let bounds = self
            .settings()
            .borrow()
            .workspace
            .window(id)
            .and_then(|w| w.bounds)
            .filter(|b| b.iter().all(|n| n.is_finite()) && b[2] >= 960. && b[3] >= 640.)
            .map(|b| Bounds::new(point(px(b[0]), px(b[1])), size(px(b[2]), px(b[3]))))
            .unwrap_or_else(|| Bounds::centered(None, size(px(1440.), px(900.)), cx));
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            window_min_size: Some(size(px(960.), px(640.))),
            titlebar: Some(TitlebarOptions {
                title: Some(format!("HelixLane · {id}").into()),
                ..titlebar
            }),
            ..TitleBar::window_options()
        };
        let owner = self.clone();
        open_window(options, cx, move |window, cx| {
            cx.new(|cx| HelixLane::new(id, owner, window, cx))
        })
        .map_err(|e| e.to_string())?;
        Ok(())
    }
    pub fn create(&self, source: Option<WindowId>, cx: &mut App) -> Result<WindowId, String> {
        let id = self.0.settings.borrow_mut().workspace.create_window(source);
        self.open(id, cx)?;
        Ok(id)
    }
    pub fn focus(&self, id: ViewId, cx: &mut App) {
        let window = self.0.settings.borrow().workspace.owner(id);
        if window.and_then(|id| self.window(id)).is_none() {
            let destination = self
                .windows()
                .first()
                .copied()
                .or_else(|| self.create(None, cx).ok());
            if let Some(destination) = destination {
                self.relocate(id, destination, Location::Center, cx);
            }
            return;
        }
        if let Some((handle, view)) = window.and_then(|id| self.window(id)) {
            let _ = handle.update(cx, |_, w, cx| {
                w.activate_window();
                let _ = view.update(cx, |view, cx| view.focus_tool(id, w, cx));
            });
        }
    }
    pub fn relocate(&self, id: ViewId, destination: WindowId, location: Location, cx: &mut App) {
        let source = self.settings().borrow().workspace.owner(id);
        if let Some((handle, view)) = source.and_then(|id| self.window(id)) {
            let _ = handle.update(cx, |_, w, cx| {
                let _ = view.update(cx, |view, cx| {
                    if source != Some(destination) {
                        view.move_window(id, Some(destination), w, cx);
                    } else {
                        view.move_tool(id, location, w, cx);
                    }
                });
            });
        } else {
            self.settings()
                .borrow_mut()
                .workspace
                .move_to_window(id, destination, location);
        }
        if let Some((handle, view)) = self.window(destination) {
            let _ = handle.update(cx, |_, w, cx| {
                let _ = view.update(cx, |view, cx| view.receive_view(id, location, w, cx));
                w.activate_window();
            });
        }
    }
    pub fn close_active(&self, cx: &mut App) {
        let active = cx.active_window();
        let selected = self
            .0
            .windows
            .borrow()
            .values()
            .find(|(h, _)| Some(*h) == active)
            .cloned();
        if let Some((handle, view)) = selected {
            let _ = handle.update(cx, |_, w, cx| {
                let _ = view.update(cx, |v, cx| v.request_close(None, w, cx));
            });
        }
    }
    pub fn dispatch(&self, mut request: OpenRequest, cx: &mut App) {
        let id = if request.mode == OpenMode::NewWindow {
            match self.create(None, cx) {
                Ok(id) => {
                    request.mode = OpenMode::NewView;
                    id
                }
                Err(_) => return,
            }
        } else {
            let active = cx.active_window();
            let existing = self
                .0
                .windows
                .borrow()
                .iter()
                .find(|(_, (h, _))| Some(*h) == active)
                .map(|(id, _)| *id)
                .or_else(|| self.windows().first().copied());
            match existing {
                Some(id) => id,
                None => match self.create(None, cx) {
                    Ok(id) => id,
                    Err(_) => return,
                },
            }
        };
        if let Some((handle, view)) = self.window(id) {
            let _ = handle.update(cx, |_, w, cx| {
                let _ = view.update(cx, |v, cx| v.dispatch_launch(request, w, cx));
            });
        }
    }
    pub fn start(
        &self,
        server: helixlane::launch::ipc::Server,
        request: Option<OpenRequest>,
        cx: &mut App,
    ) {
        let cold_new = request
            .as_ref()
            .is_some_and(|r| r.mode == OpenMode::NewWindow);
        if !cold_new {
            let ids = self
                .0
                .settings
                .borrow()
                .workspace
                .windows
                .iter()
                .map(|w| w.id)
                .collect::<Vec<_>>();
            for id in ids {
                let _ = self.open(id, cx);
            }
        }
        if let Some(request) = request {
            self.dispatch(request, cx);
        }
        if self.windows().is_empty() {
            let _ = self.create(None, cx);
        }
        let owner = self.clone();
        cx.spawn(async move |cx| {
            loop {
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(50))
                    .await;
                cx.update(|cx| {
                    for request in server
                        .requests
                        .try_iter()
                        .take(helixlane::launch::ipc::QUEUE_CAPACITY)
                    {
                        owner.dispatch(request, cx);
                    }
                });
            }
        })
        .detach();
        cx.activate(true);
    }
}

#[cfg(all(test, feature = "shell-tests"))]
mod tests {
    use super::{ApplicationOwner, Settings, TargetMode, ToolKind};
    use gpui_kit as gpui;
    #[gpui::test]
    fn registry_keeps_one_entity_when_moving_between_native_window_roots(
        cx: &mut gpui::TestAppContext,
    ) {
        cx.update(|cx| {
            gpui_shell::init(cx);
            gpui_kit::init(cx);
            let mut settings = Settings::default();
            settings.workspace.ensure_windows();
            let id = settings
                .workspace
                .open(ToolKind::Sftp, TargetMode::Fixed(1));
            let other = settings.workspace.create_window(None);
            let owner = ApplicationOwner::install_settings(settings, cx);
            owner.open(1, cx).unwrap();
            owner.open(other, cx).unwrap();
            let before = owner.panels().borrow().get(&id).unwrap().entity_id();
            let (handle, view) = owner.window(1).unwrap();
            handle
                .update(cx, |_, window, cx| {
                    view.update(cx, |view, cx| view.move_window(id, Some(other), window, cx))
                })
                .unwrap()
                .unwrap();
            assert_eq!(
                owner.panels().borrow().get(&id).unwrap().entity_id(),
                before
            );
            assert_eq!(owner.settings().borrow().workspace.owner(id), Some(other));
            assert_eq!(owner.windows().len(), 2);
        });
    }
}
