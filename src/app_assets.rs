//! Kit's default asset subset plus the icon used by the plugin host.
use gpui_kit::{AssetSource, Result, SharedString, assets};
use std::borrow::Cow;

assets::icon_assets!(PluginIcons, [Code]);

pub struct AppAssets;
impl AssetSource for AppAssets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        if let Some(icon) = PluginIcons.load(path)? {
            return Ok(Some(icon));
        }
        assets::Assets.load(path)
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        let mut paths = assets::Assets.list(path)?;
        paths.extend(PluginIcons.list(path)?);
        Ok(paths)
    }
}
