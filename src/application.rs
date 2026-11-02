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
    panels: Panels,
    windows: RefCell<HashMap<WindowId, (AnyWindowHandle, WeakEntity<HelixLane>)>>,
}
impl Global for ApplicationOwner {}
impl ApplicationOwner {
    pub fn install(cx: &mut App) -> Self {
        let mut settings = Settings::load(&Settings::path()).unwrap_or_else(|error| Settings {
            recovery: Some(error),
            ..Settings::default()
        });
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
        self.0.settings.borrow().save(&Settings::path())
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
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
                None,
                size(px(1440.), px(900.)),
                cx,
            ))),
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
        if let Some((handle, view)) = window.and_then(|id| self.window(id)) {
            let _ = handle.update(cx, |_, w, cx| {
                w.activate_window();
                let _ = view.update(cx, |view, cx| view.focus_tool(id, w, cx));
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
