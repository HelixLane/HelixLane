//! Native gpui-shell embedding. Each panel owns an isolated VM and a Policy.
use super::{ToolContext, ToolInstance};
use crate::i18n::Language;
use gpui_kit::{App, Entity, Window};
use gpui_shell::{
    HostError, HostModule, HostObject, HostValue, ScriptView, ShellRuntime,
    policy::{self, Policy},
};
use std::{
    path::{Path, PathBuf},
    rc::Rc,
    sync::atomic::{AtomicU64, Ordering},
};

pub const EXAMPLE_ID: &str = "helixlane.resource-overview";
pub const EXAMPLE_SOURCE: &str = include_str!("../../plugins/resource-overview/main.js");

/// 0.8.1 exposes policy-taking construction only inside its crate. Its public
/// load/mount APIs capture the creation default. This guard lends that default
/// synchronously, with no await, event-loop yield, or reentrant host UI calls.
/// Existing views carry their own Rc<Policy>; this never alters their grants.
struct CreationPolicy(Option<Policy>);
impl CreationPolicy {
    fn enter(next: Policy) -> Self {
        let mut saved = None;
        policy::update_default(|previous| {
            saved = Some(previous);
            next
        });
        Self(saved)
    }
}
impl Drop for CreationPolicy {
    fn drop(&mut self) {
        if let Some(previous) = self.0.take() {
            policy::set_default(previous);
        }
    }
}

/// A private host-written bundle, never a user-selected entry or an installer.
/// No manifest is accepted, so the runtime cannot fetch dependency repositories.
struct Bundle(PathBuf);
impl Bundle {
    fn new(source: &str) -> std::io::Result<Self> {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "helixlane-plugin-{}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let mut builder = std::fs::DirBuilder::new();
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        builder.create(&root)?;
        let bundle = Self(root);
        std::fs::write(bundle.0.join("main.js"), source)?;
        Ok(bundle)
    }
    fn path(&self) -> &Path {
        &self.0
    }
}
impl Drop for Bundle {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn context_value(context: ToolContext) -> HostValue {
    let resource = context
        .resource
        .map(|resource| {
            HostObject::new()
                .field("id", resource.id as f64)
                .field("name", resource.name)
                .field("host", resource.host)
                .field("port", f64::from(resource.port))
                .into()
        })
        .unwrap_or(HostValue::Null);
    HostObject::new()
        .field("view_id", context.view_id as f64)
        .field("generation", context.generation as f64)
        .field("resource", resource)
        .into()
}

pub fn resource_module(instance: &ToolInstance, language: Language) -> HostModule {
    let context = instance.resource_context();
    let lease = instance.lease();
    let labels_lease = lease.clone();
    let activity = instance.collection_activity();
    HostModule::new("helixlane:resource")
        .function("context", move |_| {
            if !lease.is_active() {
                return Err(HostError::new("Plugin is unloaded"));
            }
            if !activity.get() {
                return Err(HostError::new("Collection paused"));
            }
            context.clone().map(context_value).map_err(HostError::new)
        })
        .function("labels", move |_| {
            if !labels_lease.is_active() {
                return Err(HostError::new("Plugin is unloaded"));
            }
            Ok(HostObject::new()
                .field("title", language.text("plugin_title"))
                .field("empty", language.text("empty"))
                .field("view", language.text("plugin_view"))
                .field("permissions", language.text("plugin_permissions"))
                .into())
        })
}

pub struct NativePlugin {
    // Revoke before dropping ScriptView: old rendered frames may hold its entity.
    instance: ToolInstance,
    view: Option<Entity<ScriptView>>,
    _runtime: Rc<ShellRuntime>,
    _bundle: Bundle,
}
impl NativePlugin {
    pub fn mount(
        instance: ToolInstance,
        language: Language,
        window: &mut Window,
        cx: &mut App,
    ) -> Result<Self, String> {
        Self::mount_source(instance, language, EXAMPLE_SOURCE, window, cx)
    }
    fn mount_source(
        instance: ToolInstance,
        language: Language,
        source: &str,
        window: &mut Window,
        cx: &mut App,
    ) -> Result<Self, String> {
        let policy = Policy::new()
            .with_application(EXAMPLE_ID)
            .with_host_module(resource_module(&instance, language))
            .map_err(|error| error.to_string())?;
        Self::mount_with_policy(instance, policy, source, window, cx)
    }
    fn mount_with_policy(
        instance: ToolInstance,
        policy: Policy,
        source: &str,
        window: &mut Window,
        cx: &mut App,
    ) -> Result<Self, String> {
        // The upstream default environment includes proxy URLs with passwords.
        // Opt out per policy, including when an embedding caller supplied grants.
        let capabilities = policy.capabilities().clone().without_default_env();
        let policy = policy.with_capabilities(capabilities);
        let bundle = Bundle::new(source).map_err(|error| error.to_string())?;
        let runtime = ShellRuntime::new_isolated().map_err(|error| error.to_string())?;
        runtime.set_editor_files(false);
        let view = {
            let _guard = CreationPolicy::enter(policy);
            let application = runtime
                .load_application(bundle.path(), "main.js")
                .map_err(|error| error.to_string())?;
            runtime
                .mount_application(&application, window, cx)
                .map_err(|error| error.to_string())?
        };
        Ok(Self {
            instance,
            view: Some(view),
            _runtime: runtime,
            _bundle: bundle,
        })
    }
    pub fn set_collection_active(&self, active: bool) {
        self.instance.set_collection_active(active);
    }
    pub fn view(&self) -> Option<Entity<ScriptView>> {
        self.view.clone()
    }
    pub fn context(&self) -> &ToolContext {
        self.instance.context()
    }
    pub fn unload(&mut self) {
        self.instance.unload();
        self.view.take();
    }
}
impl Drop for NativePlugin {
    fn drop(&mut self) {
        self.unload();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn creation_policy_restores_on_unwind_and_keeps_captured_grants() {
        let before = policy::default();
        let captured;
        {
            let _guard = CreationPolicy::enter(Policy::new().with_application("first"));
            captured = policy::default();
        }
        assert_eq!(policy::default().application(), before.application());
        let _ = std::panic::catch_unwind(|| {
            let _guard = CreationPolicy::enter(Policy::new().with_application("second"));
            panic!("test failed construction");
        });
        assert_eq!(policy::default().application(), before.application());
        assert_eq!(captured.application(), "first");
        let caps = captured.capabilities();
        assert!(!caps.has_read_access());
        assert!(!caps.has_write_access());
        assert!(!caps.may_run("sh"));
        assert!(!caps.may_reach("example.com"));
        assert!(!caps.has_storage());
        assert!(!caps.may_exit());
    }
    #[test]
    fn module_functions_reject_retained_handles_after_unload() {
        use super::super::{ResourceCapabilities, ToolProvider};
        use crate::workspace::{TargetMode, ToolKind, Workspace};
        let mut workspace = Workspace::default();
        let id = workspace.open(ToolKind::Information, TargetMode::Follow);
        let mut instance = ToolProvider::plugin(EXAMPLE_ID, ResourceCapabilities::metadata())
            .instantiate(id, &workspace)
            .unwrap();
        let module = resource_module(&instance, Language::English);
        let args = gpui_shell::HostArguments::new([]);
        assert!(
            module
                .call("context", &args)
                .unwrap()
                .get("resource")
                .is_some()
        );
        instance.unload();
        assert!(module.call("context", &args).is_err());
        assert!(module.call("labels", &args).is_err());
    }
}

#[cfg(all(test, feature = "shell-tests"))]
mod native_tests {
    use super::*;
    use crate::{
        tools::{ResourceCapabilities, ToolProvider},
        workspace::{TargetMode, ToolKind, Workspace},
    };
    use gpui::{IntoElement, TestAppContext, VisualTestContext};
    use gpui_kit as gpui;
    use std::ops::Deref;

    struct Empty;
    impl gpui::Render for Empty {
        fn render(&mut self, _: &mut Window, _: &mut gpui::Context<Self>) -> impl IntoElement {
            gpui::div()
        }
    }
    fn instance(allowed: bool) -> ToolInstance {
        let mut workspace = Workspace::default();
        let id = workspace.open(ToolKind::Plugin, TargetMode::Follow);
        ToolProvider::plugin(EXAMPLE_ID, ResourceCapabilities { metadata: allowed })
            .instantiate(id, &workspace)
            .unwrap()
    }
    fn draw(context: &mut VisualTestContext, view: &Entity<ScriptView>) -> String {
        let entity = view.clone();
        context.draw(
            gpui::Point::default(),
            gpui::size(gpui::px(640.), gpui::px(400.)),
            move |_, _| entity.into_any_element(),
        );
        context.update(|_, cx| {
            assert!(
                view.read(cx).build_error().is_none(),
                "{:?}",
                view.read(cx).build_error()
            );
            view.read(cx)
                .snapshot()
                .expect("native snapshot")
                .debug_tree()
        })
    }
    const PROBE: &str = r#"
import { View, div } from "gpui-kit";
import { context } from "helixlane:resource";
import * as fs from "fs/promises";
import process from "process";
export default class Probe extends View {
  init(_props, cx) {
    this.outcome = "pending";
    cx.spawn(async (cx) => {
      await Promise.resolve();
      try { this.outcome = "resource:" + context().resource.id; }
      catch (error) { this.outcome = "denied:" + error.message; }
      try { await fs.readFile("/etc/passwd"); this.outcome += "|UNSAFE"; }
      catch (error) { this.outcome += error.message.includes("capabilit") ? "|fs-denied" : "|unexpected-fs-error:" + error.message; }
      try { await process.run("helixlane-test-forbidden-command"); this.outcome += "|UNSAFE"; }
      catch (error) { this.outcome += error.message.includes("capabilit") ? "|process-denied" : "|unexpected-process-error:" + error.message; }
      try { await fetch("http://127.0.0.1:9/"); this.outcome += "|UNSAFE"; }
      catch (error) { this.outcome += error.message.includes("capabilit") ? "|network-denied" : "|unexpected-network-error:" + error.message; }
      cx.notify();
    });
  }
  render() {
    let now;
    try { now = "current:" + context().resource.id; }
    catch (_) { now = "current:denied"; }
    return div().child(this.outcome + "|" + now);
  }
}
"#;
    #[gpui::test]
    fn coexisting_native_views_keep_policy_after_async_reentry_and_default_swap(
        cx: &mut TestAppContext,
    ) {
        cx.update(gpui_shell::init);
        let window = cx.add_window(|_, _| Empty);
        let mut context = VisualTestContext::from_window(*window.deref(), cx);
        let allowed = context
            .update(|window, cx| {
                NativePlugin::mount_source(instance(true), Language::English, PROBE, window, cx)
            })
            .unwrap();
        let denied = context
            .update(|window, cx| {
                NativePlugin::mount_source(instance(false), Language::English, PROBE, window, cx)
            })
            .unwrap();
        let a = allowed.view().unwrap();
        let b = denied.view().unwrap();
        let _third = CreationPolicy::enter(Policy::new().with_application("unrelated"));
        draw(&mut context, &a);
        draw(&mut context, &b);
        context.run_until_parked();
        let allowed_tree = draw(&mut context, &a);
        let denied_tree = draw(&mut context, &b);
        assert!(
            allowed_tree.contains("resource:1|fs-denied|process-denied|network-denied|current:1"),
            "{allowed_tree}"
        );
        assert!(
            denied_tree
                .contains("Resource metadata permission denied|fs-denied|process-denied|network-denied|current:denied"),
            "{denied_tree}"
        );
        assert!(!allowed_tree.contains("UNSAFE"));
        context.update(|_, cx| {
            assert!(!a.read(cx).policy().capabilities().may_run("sh"));
            assert!(!b.read(cx).policy().capabilities().may_reach("example.com"));
        });
        drop(a);
        drop(b);
        drop(allowed);
        drop(denied);
        context.run_until_parked();
    }

    #[gpui::test]
    fn bundled_example_materializes_resource_and_unload_revokes_retained_frame(
        cx: &mut TestAppContext,
    ) {
        cx.update(gpui_shell::init);
        let window = cx.add_window(|_, _| Empty);
        let mut context = VisualTestContext::from_window(*window.deref(), cx);
        let mut plugin = context
            .update(|window, cx| NativePlugin::mount(instance(true), Language::English, window, cx))
            .unwrap();
        let view = plugin.view().unwrap();
        let tree = draw(&mut context, &view);
        assert!(tree.contains("host-1.example.invalid:22"), "{tree}");
        assert!(tree.contains("JavaScript panel"), "{tree}");
        let lease = plugin.instance.lease();
        plugin.unload();
        assert!(!lease.is_active());
        assert!(plugin.view().is_none());
        // A GPUI frame may temporarily retain the old entity; its host lease is
        // already revoked even before GPUI drops it and cancels shell tasks.
        assert!(plugin.instance.resource_context().is_err());
        drop(view);
        drop(plugin);
        context.run_until_parked();
    }
    #[gpui::test]
    fn unload_cancels_runtime_timers_and_sleeping_async_tasks(cx: &mut TestAppContext) {
        use std::cell::Cell;
        cx.update(gpui_shell::init);
        let window = cx.add_window(|_, _| Empty);
        let mut context = VisualTestContext::from_window(*window.deref(), cx);
        let count = Rc::new(Cell::new(0));
        let observed = count.clone();
        let module = HostModule::new("test:lifecycle").function("pulse", move |_| {
            observed.set(observed.get() + 1);
            Ok(HostValue::Null)
        });
        let policy = Policy::new()
            .with_application("lifecycle")
            .with_host_module(module)
            .unwrap();
        let source = r#"
import { View, div } from "gpui-kit";
import { pulse } from "test:lifecycle";
export default class Probe extends View {
  init(_props, cx) {
    this.timer = cx.timer.every(10, () => pulse());
    cx.spawn(async (cx) => { await cx.sleep(100); pulse(); });
  }
  render() { return div().child("timer active"); }
}
"#;
        let mut plugin = context
            .update(|window, cx| {
                NativePlugin::mount_with_policy(instance(true), policy, source, window, cx)
            })
            .unwrap();
        let view = plugin.view().unwrap();
        draw(&mut context, &view);
        context.run_until_parked();
        context
            .executor()
            .advance_clock(std::time::Duration::from_millis(11));
        context.run_until_parked();
        assert!(count.get() > 0, "timer actually ran before unload");
        plugin.unload();
        drop(view);
        drop(plugin);
        for _ in 0..2 {
            context.draw(
                gpui::Point::default(),
                gpui::size(gpui::px(640.), gpui::px(400.)),
                |_, _| gpui::div().into_any_element(),
            );
        }
        context.run_until_parked();
        let after = count.get();
        context
            .executor()
            .advance_clock(std::time::Duration::from_secs(1));
        context.run_until_parked();
        assert_eq!(
            count.get(),
            after,
            "unload cancels repeating subscriptions and pending async continuations"
        );
        assert_eq!(
            Rc::strong_count(&count),
            1,
            "host module closures are released"
        );
    }
    #[gpui::test]
    fn failed_script_mount_restores_authority_and_a_fresh_instance_can_load(
        cx: &mut TestAppContext,
    ) {
        cx.update(gpui_shell::init);
        let window = cx.add_window(|_, _| Empty);
        let mut context = VisualTestContext::from_window(*window.deref(), cx);
        let previous = policy::default();
        let instance = instance(true);
        let lease = instance.lease();
        let failed = context.update(|window, cx| NativePlugin::mount_source(instance, Language::English,
            r#"import { View } from "gpui-kit"; export default class Failure extends View { init() { throw new Error("fixture mount failed"); } }"#, window, cx));
        let error = match failed {
            Ok(_) => panic!("broken script must fail"),
            Err(error) => error,
        };
        assert!(error.contains("fixture mount failed"), "{error}");
        assert!(!lease.is_active());
        assert_eq!(policy::default().application(), previous.application());
        let fresh = context
            .update(|window, cx| {
                NativePlugin::mount(self::instance(true), Language::Chinese, window, cx)
            })
            .unwrap();
        let view = fresh.view().unwrap();
        assert!(draw(&mut context, &view).contains("资源概览"));
        drop(view);
        drop(fresh);
        context.run_until_parked();
    }
    #[gpui::test]
    fn plugin_environment_excludes_proxy_credentials_after_async_reentry(cx: &mut TestAppContext) {
        const CHILD: &str = "HELIXLANE_ENV_PROBE_CHILD";
        if std::env::var_os(CHILD).is_none() {
            let output = std::process::Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "tools::shell::native_tests::plugin_environment_excludes_proxy_credentials_after_async_reentry", "--nocapture", "--test-threads=1"])
                .env(CHILD, "1")
                .env("HTTP_PROXY", "http://test-user:synthetic-secret@127.0.0.1:9")
                .env("HTTPS_PROXY", "http://test-user:synthetic-secret@127.0.0.1:9")
                .env("ALL_PROXY", "http://test-user:synthetic-secret@127.0.0.1:9")
                .env("http_proxy", "http://test-user:synthetic-secret@127.0.0.1:9")
                .env("HELIXLANE_PRIVATE_TEST_TOKEN", "synthetic-private-token")
                .env("HELIXLANE_PUBLIC_TEST_LABEL", "public-label")
                .output().unwrap();
            assert!(
                output.status.success(),
                "environment isolation subprocess failed:\n{}\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            return;
        }
        assert!(
            std::env::var("HTTP_PROXY")
                .unwrap()
                .contains("synthetic-secret")
        );
        cx.update(gpui_shell::init);
        let window = cx.add_window(|_, _| Empty);
        let mut context = VisualTestContext::from_window(*window.deref(), cx);
        let source = r#"
import { View, div } from "gpui-kit";
import process from "process";
export default class Probe extends View {
  init(_props, cx) {
    this.count = "pending";
    cx.spawn(async (cx) => {
      await cx.sleep(1);
      this.count = `${Object.keys(process.env).length}|${process.env.HTTP_PROXY === undefined}|${process.env.HELIXLANE_PRIVATE_TEST_TOKEN === undefined}|${process.env.HELIXLANE_PUBLIC_TEST_LABEL || "none"}`;
      cx.notify();
    });
  }
  render() { return div().child("environment:" + this.count); }
}
"#;
        let plugin = context
            .update(|window, cx| {
                NativePlugin::mount_source(instance(true), Language::English, source, window, cx)
            })
            .unwrap();
        let view = plugin.view().unwrap();
        draw(&mut context, &view);
        context.run_until_parked();
        context
            .executor()
            .advance_clock(std::time::Duration::from_millis(2));
        context.run_until_parked();
        let tree = draw(&mut context, &view);
        assert!(
            tree.contains("environment:0|true|true|none"),
            "environment key counts and absence flags: {tree}"
        );
        let granted = Policy::new()
            .with_application("public-label-only")
            .with_capabilities(
                gpui_shell::Capabilities::new()
                    .env_variables(["HELIXLANE_PUBLIC_TEST_LABEL".to_string()]),
            );
        let second = context
            .update(|window, cx| {
                NativePlugin::mount_with_policy(instance(true), granted, source, window, cx)
            })
            .unwrap();
        let second_view = second.view().unwrap();
        draw(&mut context, &second_view);
        context.run_until_parked();
        context
            .executor()
            .advance_clock(std::time::Duration::from_millis(2));
        context.run_until_parked();
        let granted_tree = draw(&mut context, &second_view);
        assert!(
            granted_tree.contains("environment:1|true|true|public-label"),
            "{granted_tree}"
        );
        assert!(draw(&mut context, &view).contains("environment:0|true|true|none"));
        drop(second_view);
        drop(second);
        drop(view);
        drop(plugin);
        context.run_until_parked();
    }
}
