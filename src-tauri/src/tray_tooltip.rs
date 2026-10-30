use tauri::{Runtime, tray::TrayIcon};

/// Applies the already-composed tray tooltip to every native hover target.
///
/// Tauri assigns the tooltip to the status-item button, while tray-icon places
/// a pointer-receiving subview over that button. Both paths are best-effort
/// presentation updates and never affect the committed application state.
pub(crate) fn set_status_item_tooltip<R: Runtime>(tray: &TrayIcon<R>, tooltip: &str) {
    let _ = tray.set_tooltip(Some(tooltip));
    #[cfg(target_os = "macos")]
    mirror_tooltip_to_button_subviews(tray, tooltip);
}

#[cfg(target_os = "macos")]
fn mirror_tooltip_to_button_subviews<R: Runtime>(tray: &TrayIcon<R>, tooltip: &str) {
    let tooltip = tooltip.to_owned();
    let _ = tray.with_inner_tray_icon(move |inner| {
        let Some(main_thread) = objc2::MainThreadMarker::new() else {
            return;
        };
        let Some(status_item) = inner.ns_status_item() else {
            return;
        };
        let Some(button) = status_item.button(main_thread) else {
            return;
        };
        let tooltip = objc2_foundation::NSString::from_str(&tooltip);
        let subviews = button.subviews();
        for index in 0..subviews.count() {
            subviews.objectAtIndex(index).setToolTip(Some(&tooltip));
        }
    });
}
