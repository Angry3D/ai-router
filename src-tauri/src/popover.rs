use std::cmp::Ordering;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::Serialize;
use tauri::{
    AppHandle, Emitter, LogicalPosition, LogicalSize, Manager, PhysicalPosition, PhysicalSize,
    Runtime, State, WindowEvent,
    tray::{MouseButton, MouseButtonState, TrayIconEvent},
};
use tauri_plugin_positioner::{Position, WindowExt};
use unicode_segmentation::UnicodeSegmentation;

#[cfg(target_os = "macos")]
use tauri_nspanel::{
    ManagerExt as _, NSPoint as PanelPoint, NSRect as PanelRect, NSSize as PanelSize, StyleMask,
    WebviewWindowExt as _, panel::NSAutoresizingMaskOptions, tauri_panel,
};

use crate::runtime::DesktopLifecycleServices;
use router_core::{
    balance::{BalanceDisplaySnapshot, BalanceDisplayStatus, BalanceResult},
    domain::ProxyRuntimeStatus,
    state::BootstrapSnapshotDto,
};

const MENU_LABEL: &str = "menu";
const MENU_CARD_WIDTH: f64 = 360.0;
const MENU_HORIZONTAL_FRAME: f64 = 24.0;
const MENU_VERTICAL_FRAME: f64 = 34.0;
const MENU_WINDOW_WIDTH: f64 = MENU_CARD_WIDTH + MENU_HORIZONTAL_FRAME;
const MENU_PREVIEW_WIDTH: f64 = 384.0;
const MENU_PREVIEW_GAP: f64 = 8.0;
const MENU_PREVIEW_HEIGHT: f64 = 480.0;
const MENU_MIN_HEIGHT: f64 = 188.0;
const MENU_MAX_HEIGHT: f64 = 640.0;
const MENU_ARROW_MIN_X: f64 = MENU_HORIZONTAL_FRAME / 2.0 + 20.0;
const MENU_ARROW_MAX_X: f64 = MENU_WINDOW_WIDTH - MENU_ARROW_MIN_X;
const BLUR_HIDE_DELAY: Duration = Duration::from_millis(120);
const TRAY_ROUTE_NAME_GRAPHEMES: usize = 12;
const TRAY_ROUTE_NAME_PREFIX_GRAPHEMES: usize = 6;
const TRAY_ROUTE_NAME_SUFFIX_GRAPHEMES: usize =
    TRAY_ROUTE_NAME_GRAPHEMES - TRAY_ROUTE_NAME_PREFIX_GRAPHEMES - 1;
const TRAY_BALANCE_GRAPHEMES: usize = 16;

#[cfg(target_os = "macos")]
tauri_panel! {
    panel!(MenuPanel {
        config: {
            can_become_key_window: true,
            can_become_main_window: false,
        }
    })
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum TrayVisualState {
    Ready,
    Active,
}

#[derive(Debug, Eq, PartialEq)]
pub(crate) struct TrayPresentation {
    pub(crate) title: String,
    pub(crate) tooltip: String,
    pub(crate) visual_state: TrayVisualState,
}

#[derive(Default)]
struct VisibilityState {
    generation: u64,
    logically_visible: bool,
    tray_anchor: Option<TrayAnchor>,
    base_menu_size: Option<LogicalSize<f64>>,
    preview_side: PreviewSide,
    preview_width: f64,
    preview_height: f64,
    preview_revision: u64,
    preview_open: bool,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum PreviewSide {
    Left,
    #[default]
    Right,
}

#[cfg(any(not(target_os = "macos"), test))]
#[derive(Clone, Copy, Debug, PartialEq)]
struct PreviewGeometryBatch {
    size: LogicalSize<f64>,
    position: Option<PhysicalPosition<i32>>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct PreviewFrame {
    origin: LogicalPosition<f64>,
    size: LogicalSize<f64>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct TrayAnchor {
    event_position: PhysicalPosition<f64>,
    center_x: f64,
    height: f64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct MonitorBounds {
    position: PhysicalPosition<i32>,
    size: PhysicalSize<u32>,
    work_area_position: PhysicalPosition<i32>,
    work_area_size: PhysicalSize<u32>,
    scale_factor: f64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct MenuPlacement {
    position: PhysicalPosition<i32>,
    tray_center_x: f64,
    scale_factor: f64,
}

impl MenuPlacement {
    fn arrow_offset(self) -> f64 {
        ((self.tray_center_x - f64::from(self.position.x)) / self.scale_factor)
            .clamp(MENU_ARROW_MIN_X, MENU_ARROW_MAX_X)
    }
}

pub struct MenuPopoverController {
    geometry_gate: Mutex<()>,
    state: Mutex<VisibilityState>,
}

impl MenuPopoverController {
    #[must_use]
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            geometry_gate: Mutex::new(()),
            state: Mutex::new(VisibilityState::default()),
        })
    }

    fn toggle(&self) -> ToggleDecision {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.generation = state.generation.wrapping_add(1);
        state.logically_visible = !state.logically_visible;
        if state.logically_visible {
            state.base_menu_size = None;
            state.preview_open = false;
            state.preview_revision = 0;
            ToggleDecision::Prepare(state.generation)
        } else {
            ToggleDecision::Hide
        }
    }

    fn request_show(&self) -> u64 {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.generation = state.generation.wrapping_add(1);
        state.logically_visible = true;
        state.base_menu_size = None;
        state.preview_open = false;
        state.preview_revision = 0;
        state.generation
    }

    fn hide(&self) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.generation = state.generation.wrapping_add(1);
        state.logically_visible = false;
        state.preview_open = false;
        state.preview_revision = 0;
    }

    fn is_current_show(&self, generation: u64) -> bool {
        let state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.logically_visible && state.generation == generation
    }

    fn is_current_hide(&self, generation: u64) -> bool {
        let state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        !state.logically_visible && state.generation == generation
    }

    fn present_if_current(&self, generation: u64, present: impl FnOnce()) -> bool {
        let state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !state.logically_visible || state.generation != generation {
            return false;
        }
        present();
        true
    }

    fn hide_if_current(&self, generation: u64) -> bool {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !state.logically_visible || state.generation != generation {
            return false;
        }
        state.generation = state.generation.wrapping_add(1);
        state.logically_visible = false;
        true
    }

    fn generation(&self) -> u64 {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .generation
    }

    fn record_tray_anchor(&self, event: &TrayIconEvent) {
        let (event_position, rect) = match event {
            TrayIconEvent::Click { position, rect, .. }
            | TrayIconEvent::DoubleClick { position, rect, .. }
            | TrayIconEvent::Enter { position, rect, .. }
            | TrayIconEvent::Move { position, rect, .. }
            | TrayIconEvent::Leave { position, rect, .. } => (*position, rect),
            _ => return,
        };
        let position = rect.position.to_physical::<f64>(1.0);
        let size = rect.size.to_physical::<f64>(1.0);
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .tray_anchor = Some(TrayAnchor {
            event_position,
            center_x: position.x + size.width / 2.0,
            height: size.height,
        });
    }

    fn tray_anchor(&self) -> Option<TrayAnchor> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .tray_anchor
    }

    fn record_base_menu_size(
        &self,
        generation: u64,
        size: LogicalSize<f64>,
        preview_side: PreviewSide,
        preview_width: f64,
        preview_height: f64,
    ) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if state.generation == generation {
            state.base_menu_size = Some(size);
            state.preview_side = preview_side;
            state.preview_width = preview_width;
            state.preview_height = preview_height;
            state.preview_revision = 0;
            state.preview_open = false;
        }
    }

    fn set_preview(&self, generation: u64, revision: u64, open: bool) -> bool {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !state.logically_visible
            || state.generation != generation
            || state.base_menu_size.is_none()
            || revision <= state.preview_revision
        {
            return false;
        }
        state.preview_revision = revision;
        state.preview_open = open;
        true
    }

    fn preview_state(&self) -> (u64, bool) {
        let state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        (state.preview_revision, state.preview_open)
    }

    fn rollback_preview(&self, generation: u64, failed_revision: u64, revision: u64, open: bool) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if state.generation == generation && state.preview_revision == failed_revision {
            state.preview_revision = revision;
            state.preview_open = open;
        }
    }

    fn preview_geometry(&self) -> Option<(LogicalSize<f64>, PreviewSide, f64)> {
        let state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let base = state.base_menu_size?;
        let size = if state.preview_open {
            expanded_preview_size(base, state.preview_width, state.preview_height)
        } else {
            base
        };
        Some((size, state.preview_side, base.width))
    }

    fn base_geometry(&self) -> Option<(LogicalSize<f64>, PreviewSide, f64)> {
        let state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let base = state.base_menu_size?;
        Some((base, state.preview_side, base.width))
    }

    fn preview_backing_geometry(
        &self,
    ) -> Option<(LogicalSize<f64>, LogicalSize<f64>, PreviewSide, bool)> {
        let state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let base = state.base_menu_size?;
        Some((
            base,
            expanded_preview_size(base, state.preview_width, state.preview_height),
            state.preview_side,
            state.preview_open,
        ))
    }

    fn reset_preview(&self) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.preview_open = false;
        state.preview_revision = 0;
    }
}

enum ToggleDecision {
    Prepare(u64),
    Hide,
}

#[cfg(target_os = "macos")]
pub fn initialize_menu_panel<R: Runtime>(
    app: &AppHandle<R>,
) -> Result<(), Box<dyn std::error::Error>> {
    let window = app.get_webview_window(MENU_LABEL).ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::NotFound, "menu window is unavailable")
    })?;
    let panel = window.to_panel::<MenuPanel<R>>()?;
    panel.set_style_mask(StyleMask::empty().borderless().nonactivating_panel().into());
    Ok(())
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct MenuPrepareEvent {
    generation: u64,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct MenuPositionedEvent {
    generation: u64,
    arrow_offset_x: f64,
    preview_side: &'static str,
    preview_width: f64,
    preview_height: f64,
}

#[expect(
    clippy::needless_pass_by_value,
    reason = "Tauri's global tray event callback owns TrayIconEvent"
)]
pub fn handle_tray_event<R: Runtime>(app: &AppHandle<R>, event: TrayIconEvent) {
    tauri_plugin_positioner::on_tray_event(app, &event);
    let controller = app.state::<Arc<MenuPopoverController>>();
    controller.record_tray_anchor(&event);
    if !matches!(
        event,
        TrayIconEvent::Click {
            button: MouseButton::Left,
            button_state: MouseButtonState::Up,
            ..
        }
    ) {
        return;
    }
    match controller.toggle() {
        ToggleDecision::Prepare(generation) => emit_prepare(app, generation),
        ToggleDecision::Hide => hide_window(app, &controller),
    }
}

pub fn handle_window_event<R: Runtime>(window: &tauri::Window<R>, event: &WindowEvent) {
    if window.label() == MENU_LABEL {
        match event {
            WindowEvent::Focused(false) => schedule_blur_hide(window.app_handle()),
            WindowEvent::CloseRequested { api, .. } => {
                api.prevent_close();
                let controller = window.state::<Arc<MenuPopoverController>>();
                hide_window(window.app_handle(), &controller);
            }
            _ => {}
        }
    } else if window.label() == "settings"
        && let WindowEvent::CloseRequested { api, .. } = event
    {
        api.prevent_close();
        let _ = window.emit("settings-close-requested", ());
    }
}

pub fn request_menu_show<R: Runtime>(app: &AppHandle<R>) {
    let controller = app.state::<Arc<MenuPopoverController>>();
    let _geometry_guard = controller
        .geometry_gate
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let previous = controller.preview_geometry();
    let base = controller.base_geometry();
    let generation = controller.request_show();
    if let Some(window) = app.get_webview_window(MENU_LABEL)
        && let (Some(previous), Some(base)) = (previous, base)
        && previous != base
    {
        let _ = apply_preview_geometry_atomically(app, &window, previous, base);
    }
    emit_prepare(app, generation);
}

pub fn hide_menu_window<R: Runtime>(app: &AppHandle<R>) {
    let controller = app.state::<Arc<MenuPopoverController>>();
    hide_window(app, &controller);
}

pub(crate) fn tray_presentation(
    app_name: &str,
    snapshot: &BootstrapSnapshotDto,
    balance: Option<&BalanceDisplaySnapshot>,
    isolated: bool,
    active: bool,
) -> TrayPresentation {
    let active_route = snapshot.active_route_id.as_ref().and_then(|active_id| {
        snapshot
            .routes
            .iter()
            .find(|route| &route.route_id == active_id)
    });
    let balance = active_route
        .and_then(|route| balance.filter(|candidate| candidate.route_id == route.route_id));
    let route_name = active_route.map_or("无中转", |route| route.name.as_str());
    let amount = balance
        .and_then(|snapshot| snapshot.value.as_ref())
        .map_or_else(|| "--".to_owned(), compact_balance_value);
    let compact_name = compact_route_name(route_name);
    let title = with_qa_prefix(&format!("{compact_name}({amount})"), isolated);
    let visual_state = tray_visual_state(active);
    let state_label = tray_status_label(snapshot, active);
    let tooltip = active_route.map_or_else(
        || format!("{app_name} {state_label} · 无中转"),
        |route| {
            format!(
                "{app_name} {state_label} · {} · {}",
                route.name,
                balance_tooltip_detail(balance)
            )
        },
    );

    TrayPresentation {
        title,
        tooltip,
        visual_state,
    }
}

#[must_use]
pub fn initial_tray_title(isolated: bool) -> String {
    with_qa_prefix("无中转(--)", isolated)
}

fn with_qa_prefix(title: &str, isolated: bool) -> String {
    if isolated {
        format!("QA · {title}")
    } else {
        title.to_owned()
    }
}

fn compact_route_name(name: &str) -> String {
    let graphemes = UnicodeSegmentation::graphemes(name, true).collect::<Vec<_>>();
    if graphemes.len() <= TRAY_ROUTE_NAME_GRAPHEMES {
        return name.to_owned();
    }
    let prefix = graphemes[..TRAY_ROUTE_NAME_PREFIX_GRAPHEMES]
        .concat()
        .trim_end()
        .to_owned();
    let suffix = graphemes[graphemes.len() - TRAY_ROUTE_NAME_SUFFIX_GRAPHEMES..]
        .concat()
        .trim_start()
        .to_owned();
    format!("{prefix}…{suffix}")
}

fn format_balance_value(value: &BalanceResult) -> String {
    let Some(remaining) = value.remaining else {
        return "余额可用".to_owned();
    };
    let unit = value.unit.as_deref().unwrap_or("");
    match unit {
        "$" | "¥" | "€" | "£" => format!("{unit}{remaining:.2}"),
        "USD" => format!("${remaining:.2}"),
        "" => format!("{remaining:.2}"),
        _ => format!("{remaining:.2} {unit}"),
    }
}

fn compact_balance_value(value: &BalanceResult) -> String {
    let amount = format_balance_value(value);
    let graphemes = UnicodeSegmentation::graphemes(amount.as_str(), true).collect::<Vec<_>>();
    if graphemes.len() <= TRAY_BALANCE_GRAPHEMES {
        return amount;
    }
    format!("{}…", graphemes[..TRAY_BALANCE_GRAPHEMES - 1].concat())
}

fn balance_tooltip_detail(balance: Option<&BalanceDisplaySnapshot>) -> String {
    let Some(balance) = balance else {
        return "尚无余额".to_owned();
    };
    let amount = balance.value.as_ref().map(format_balance_value);
    match balance.status {
        BalanceDisplayStatus::Unavailable => "尚无余额".to_owned(),
        BalanceDisplayStatus::Refreshing => amount.map_or_else(
            || "正在刷新余额".to_owned(),
            |amount| format!("余额 {amount}，正在刷新"),
        ),
        BalanceDisplayStatus::Fresh => {
            amount.map_or_else(|| "尚无余额".to_owned(), |amount| format!("余额 {amount}"))
        }
        BalanceDisplayStatus::Stale => amount.map_or_else(
            || "尚无余额".to_owned(),
            |amount| format!("余额 {amount}，已过期"),
        ),
        BalanceDisplayStatus::LastGood => amount.map_or_else(
            || "尚无余额".to_owned(),
            |amount| format!("上次余额 {amount}"),
        ),
        BalanceDisplayStatus::Failed => "余额查询失败".to_owned(),
    }
}

fn tray_visual_state(active: bool) -> TrayVisualState {
    if active {
        TrayVisualState::Active
    } else {
        TrayVisualState::Ready
    }
}

fn tray_status_label(snapshot: &BootstrapSnapshotDto, active: bool) -> &'static str {
    match snapshot.proxy_status {
        ProxyRuntimeStatus::PortConflict
        | ProxyRuntimeStatus::Error
        | ProxyRuntimeStatus::DatabaseError => "代理故障",
        ProxyRuntimeStatus::Running if active => "处理中",
        ProxyRuntimeStatus::Running if snapshot.active_route_id.is_some() => "正常",
        ProxyRuntimeStatus::Running
        | ProxyRuntimeStatus::Starting
        | ProxyRuntimeStatus::Stopped
        | ProxyRuntimeStatus::ShuttingDown => "需要处理",
    }
}

fn emit_prepare<R: Runtime>(app: &AppHandle<R>, generation: u64) {
    if let Some(window) = app.get_webview_window(MENU_LABEL) {
        let _ = position_menu_window(app, &window);
        let _ = window.show();
        let _ = window.emit("menu-prepare-show", MenuPrepareEvent { generation });
    }
}

fn position_menu_window<R: Runtime>(
    app: &AppHandle<R>,
    window: &tauri::WebviewWindow<R>,
) -> Result<Option<MenuPlacement>, router_core::state::IpcErrorDto> {
    let controller = app.state::<Arc<MenuPopoverController>>();
    if let (Some(tray), Ok(monitors), Ok(window_size), Ok(window_scale_factor)) = (
        controller.tray_anchor(),
        app.available_monitors(),
        window.outer_size(),
        window.scale_factor(),
    ) {
        let monitors = monitors.iter().map(monitor_bounds).collect::<Vec<_>>();
        if let Some(placement) = window_logical_size(window_size, window_scale_factor)
            .and_then(|window_size| menu_placement_for_tray(tray, &monitors, window_size))
        {
            window
                .set_position(placement.position)
                .map_err(|_| shell_error("menu_position_failed", "菜单定位失败。", true))?;
            return Ok(Some(placement));
        }
    }
    window
        .move_window(Position::TopRight)
        .map_err(|_| shell_error("menu_position_failed", "菜单定位失败。", true))?;
    Ok(None)
}

pub fn position_hidden_settings_window<R: Runtime>(
    app: &AppHandle<R>,
    window: &tauri::WebviewWindow<R>,
) {
    if window.is_visible().unwrap_or(false) {
        return;
    }
    let controller = app.state::<Arc<MenuPopoverController>>();
    let (Some(tray), Ok(monitors), Ok(window_size), Ok(window_scale_factor)) = (
        controller.tray_anchor(),
        app.available_monitors(),
        window.outer_size(),
        window.scale_factor(),
    ) else {
        return;
    };
    if !window_scale_factor.is_finite() || window_scale_factor <= 0.0 {
        return;
    }
    let monitors = monitors.iter().map(monitor_bounds).collect::<Vec<_>>();
    let logical_window_size = window_size.to_logical::<f64>(window_scale_factor);
    let Some(position) = settings_position_for_tray(tray, &monitors, logical_window_size) else {
        return;
    };
    let _ = window.set_position(position);
}

fn monitor_bounds(monitor: &tauri::Monitor) -> MonitorBounds {
    MonitorBounds {
        position: *monitor.position(),
        size: *monitor.size(),
        work_area_position: monitor.work_area().position,
        work_area_size: monitor.work_area().size,
        scale_factor: monitor.scale_factor(),
    }
}

fn monitor_for_tray(tray: TrayAnchor, monitors: &[MonitorBounds]) -> Option<MonitorBounds> {
    let mut best = None;
    let mut ambiguous = false;

    for monitor in monitors.iter().copied() {
        let left = f64::from(monitor.position.x);
        let top = f64::from(monitor.position.y);
        let right = left + f64::from(monitor.size.width);
        let bottom = top + f64::from(monitor.size.height);
        if tray.event_position.x < left
            || tray.event_position.x >= right
            || tray.event_position.y < top
            || tray.event_position.y >= bottom
        {
            continue;
        }

        let top_distance = tray.event_position.y - top;
        match best {
            None => {
                best = Some((top_distance, monitor));
                ambiguous = false;
            }
            Some((best_distance, _)) => match top_distance.total_cmp(&best_distance) {
                Ordering::Less => {
                    best = Some((top_distance, monitor));
                    ambiguous = false;
                }
                Ordering::Equal => ambiguous = true,
                Ordering::Greater => {}
            },
        }
    }

    if ambiguous {
        None
    } else {
        best.map(|(_, monitor)| monitor)
    }
}

#[cfg(test)]
fn popover_position_for_tray(
    tray: TrayAnchor,
    monitors: &[MonitorBounds],
    window_size: LogicalSize<f64>,
) -> Option<PhysicalPosition<i32>> {
    menu_placement_for_tray(tray, monitors, window_size).map(|placement| placement.position)
}

fn menu_placement_for_tray(
    tray: TrayAnchor,
    monitors: &[MonitorBounds],
    window_size: LogicalSize<f64>,
) -> Option<MenuPlacement> {
    let monitor = monitor_for_tray(tray, monitors)?;
    if !monitor.scale_factor.is_finite()
        || monitor.scale_factor <= 0.0
        || !window_size.width.is_finite()
        || !window_size.height.is_finite()
    {
        return None;
    }
    let target_window_size = window_size.to_physical::<f64>(monitor.scale_factor);

    let left = f64::from(monitor.position.x);
    let right = left + f64::from(monitor.size.width);
    let max_x = (right - target_window_size.width).max(left);
    let x = (tray.center_x - target_window_size.width / 2.0).clamp(left, max_x);

    let top = f64::from(monitor.position.y);
    let bottom = top + f64::from(monitor.size.height);
    let max_y = (bottom - target_window_size.height).max(top);
    let work_area_top = f64::from(monitor.work_area_position.y);
    let y = work_area_top.max(top + tray.height).clamp(top, max_y);

    Some(MenuPlacement {
        position: PhysicalPosition::new(physical_coordinate(x)?, physical_coordinate(y)?),
        tray_center_x: tray.center_x,
        scale_factor: monitor.scale_factor,
    })
}

fn preview_layout_for_tray(
    tray: TrayAnchor,
    monitors: &[MonitorBounds],
    placement: MenuPlacement,
) -> (PreviewSide, f64, f64) {
    let Some(monitor) = monitor_for_tray(tray, monitors) else {
        return (PreviewSide::Left, MENU_PREVIEW_WIDTH, MENU_PREVIEW_HEIGHT);
    };
    let menu_left = f64::from(placement.position.x) / monitor.scale_factor;
    let monitor_left = f64::from(monitor.work_area_position.x) / monitor.scale_factor;
    let monitor_right = f64::from(monitor.work_area_position.x) / monitor.scale_factor
        + f64::from(monitor.work_area_size.width) / monitor.scale_factor;
    let left = (menu_left - monitor_left - MENU_PREVIEW_GAP).max(0.0);
    let right = (monitor_right - (menu_left + MENU_WINDOW_WIDTH) - MENU_PREVIEW_GAP).max(0.0);
    let preview_height = (f64::from(monitor.work_area_size.height) / monitor.scale_factor
        - MENU_VERTICAL_FRAME)
        .clamp(0.0, MENU_PREVIEW_HEIGHT);
    let (side, width) = if left >= MENU_PREVIEW_WIDTH {
        (PreviewSide::Left, MENU_PREVIEW_WIDTH)
    } else if right >= MENU_PREVIEW_WIDTH {
        (PreviewSide::Right, MENU_PREVIEW_WIDTH)
    } else if left >= right {
        (PreviewSide::Left, left.min(MENU_PREVIEW_WIDTH))
    } else {
        (PreviewSide::Right, right.min(MENU_PREVIEW_WIDTH))
    };
    (side, width, preview_height)
}

fn window_logical_size(
    window_size: PhysicalSize<u32>,
    scale_factor: f64,
) -> Option<LogicalSize<f64>> {
    if !scale_factor.is_finite() || scale_factor <= 0.0 {
        return None;
    }
    Some(window_size.to_logical::<f64>(scale_factor))
}

fn settings_position_for_tray(
    tray: TrayAnchor,
    monitors: &[MonitorBounds],
    window_size: LogicalSize<f64>,
) -> Option<LogicalPosition<f64>> {
    let monitor = monitor_for_tray(tray, monitors)?;
    if !monitor.scale_factor.is_finite()
        || monitor.scale_factor <= 0.0
        || !window_size.width.is_finite()
        || !window_size.height.is_finite()
    {
        return None;
    }
    let left = f64::from(monitor.work_area_position.x) / monitor.scale_factor;
    let top = f64::from(monitor.work_area_position.y) / monitor.scale_factor;
    let width = f64::from(monitor.work_area_size.width) / monitor.scale_factor;
    let height = f64::from(monitor.work_area_size.height) / monitor.scale_factor;
    let max_x = (left + width - window_size.width).max(left);
    let max_y = (top + height - window_size.height).max(top);
    Some(LogicalPosition::new(
        (left + (width - window_size.width) / 2.0).clamp(left, max_x),
        (top + (height - window_size.height) / 2.0).clamp(top, max_y),
    ))
}

fn physical_coordinate(value: f64) -> Option<i32> {
    let rounded = value.round();
    if !(f64::from(i32::MIN)..=f64::from(i32::MAX)).contains(&rounded) {
        return None;
    }
    #[expect(
        clippy::cast_possible_truncation,
        reason = "the inclusive i32 range check above makes this coordinate conversion safe"
    )]
    let coordinate = rounded as i32;
    Some(coordinate)
}

fn hide_window<R: Runtime>(app: &AppHandle<R>, controller: &MenuPopoverController) {
    let _geometry_guard = controller
        .geometry_gate
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let previous = controller.preview_geometry();
    controller.reset_preview();
    let next = controller.preview_geometry();
    controller.hide();
    if let Some(window) = app.get_webview_window(MENU_LABEL) {
        if let (Some(previous), Some(next)) = (previous, next) {
            let _ = apply_preview_geometry_atomically(app, &window, previous, next);
        }
        let _ = window.hide();
    }
}

fn schedule_blur_hide<R: Runtime>(app: &AppHandle<R>) {
    let app = app.clone();
    let generation = app.state::<Arc<MenuPopoverController>>().generation();
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(BLUR_HIDE_DELAY).await;
        let controller = app.state::<Arc<MenuPopoverController>>();
        if controller.hide_if_current(generation) {
            let hidden_generation = controller.generation();
            let controller = controller.inner().clone();
            let main_thread_app = app.clone();
            let _ = app.run_on_main_thread(move || {
                let _geometry_guard = controller
                    .geometry_gate
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                if !controller.is_current_hide(hidden_generation) {
                    return;
                }
                let previous = controller.preview_geometry();
                controller.reset_preview();
                let next = controller.preview_geometry();
                if let Some(window) = main_thread_app.get_webview_window(MENU_LABEL) {
                    if let (Some(previous), Some(next)) = (previous, next) {
                        let _ = apply_preview_geometry_atomically(
                            &main_thread_app,
                            &window,
                            previous,
                            next,
                        );
                    }
                    if controller.is_current_hide(hidden_generation) {
                        let _ = window.hide();
                    }
                }
            });
        }
    });
}

#[tauri::command]
pub async fn menu_frontend_ready(
    app: AppHandle,
    services: State<'_, Arc<DesktopLifecycleServices>>,
) -> Result<(), router_core::state::IpcErrorDto> {
    for _ in 0..50 {
        match services.first_run_pending().await {
            Ok(true) => {
                request_menu_show(&app);
                return Ok(());
            }
            Ok(false) => return Ok(()),
            Err(_) => tokio::time::sleep(Duration::from_millis(100)).await,
        }
    }
    request_menu_show(&app);
    Ok(())
}

#[tauri::command]
pub async fn complete_menu_show(
    app: AppHandle,
    controller: State<'_, Arc<MenuPopoverController>>,
    services: State<'_, Arc<DesktopLifecycleServices>>,
    generation: u64,
    height: f64,
) -> Result<(), router_core::state::IpcErrorDto> {
    if !controller.is_current_show(generation) {
        return Ok(());
    }
    let window = app
        .get_webview_window(MENU_LABEL)
        .ok_or_else(|| shell_error("menu_window_unavailable", "菜单窗口不可用。", true))?;
    let geometry_guard = controller
        .geometry_gate
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if !controller.is_current_show(generation) {
        return Ok(());
    }
    let base_size = menu_window_size(height);
    window
        .set_size(base_size)
        .map_err(|_| shell_error("menu_resize_failed", "菜单尺寸调整失败。", true))?;
    let placement = position_menu_window(&app, &window)?;
    if !controller.is_current_show(generation) {
        return Ok(());
    }
    let (preview_side, preview_width, preview_height) = placement.map_or(
        (PreviewSide::Left, MENU_PREVIEW_WIDTH, MENU_PREVIEW_HEIGHT),
        |placement| {
            let monitors = app
                .available_monitors()
                .unwrap_or_default()
                .iter()
                .map(monitor_bounds)
                .collect::<Vec<_>>();
            controller.tray_anchor().map_or(
                (PreviewSide::Left, MENU_PREVIEW_WIDTH, MENU_PREVIEW_HEIGHT),
                |tray| preview_layout_for_tray(tray, &monitors, placement),
            )
        },
    );
    controller.record_base_menu_size(
        generation,
        base_size,
        preview_side,
        preview_width,
        preview_height,
    );
    let arrow_offset_x = placement.map_or(MENU_WINDOW_WIDTH / 2.0, MenuPlacement::arrow_offset);
    let _ = window.emit(
        "menu-positioned",
        MenuPositionedEvent {
            generation,
            arrow_offset_x,
            preview_side: match preview_side {
                PreviewSide::Left => "left",
                PreviewSide::Right => "right",
            },
            preview_width,
            preview_height,
        },
    );
    drop(geometry_guard);
    if !show_menu_panel(&app, controller.inner().clone(), generation).await? {
        return Ok(());
    }
    services.trigger_menu_open_balance_refresh();
    services.mark_first_run_presented().await
}

#[cfg(not(target_os = "macos"))]
fn apply_preview_geometry<R: Runtime>(
    window: &tauri::WebviewWindow<R>,
    previous: (LogicalSize<f64>, PreviewSide, f64),
    next: (LogicalSize<f64>, PreviewSide, f64),
) -> Result<(), router_core::state::IpcErrorDto> {
    let current_position = window
        .outer_position()
        .map_err(|_| shell_error("menu_position_failed", "菜单定位失败。", true))?;
    let scale = window
        .scale_factor()
        .map_err(|_| shell_error("menu_position_failed", "菜单定位失败。", true))?;
    let batch = preview_geometry_batch(previous, next, current_position, scale)?;
    let result = apply_preview_geometry_batch(window, batch);
    if result.is_err() {
        let _ = window.set_size(previous.0);
        if batch.position.is_some() {
            let _ = window.set_position(current_position);
        }
    }
    result
}

#[cfg(any(not(target_os = "macos"), test))]
fn preview_geometry_batch(
    previous: (LogicalSize<f64>, PreviewSide, f64),
    next: (LogicalSize<f64>, PreviewSide, f64),
    current_position: PhysicalPosition<i32>,
    scale: f64,
) -> Result<PreviewGeometryBatch, router_core::state::IpcErrorDto> {
    let position = if next.1 == PreviewSide::Left {
        let delta = (next.0.width - previous.0.width) * scale;
        let x = f64::from(current_position.x) - delta;
        Some(PhysicalPosition::new(
            physical_coordinate(x)
                .ok_or_else(|| shell_error("menu_position_failed", "菜单定位失败。", true))?,
            current_position.y,
        ))
    } else {
        None
    };
    Ok(PreviewGeometryBatch {
        size: next.0,
        position,
    })
}

#[cfg(not(target_os = "macos"))]
fn apply_preview_geometry_batch<R: Runtime>(
    window: &tauri::WebviewWindow<R>,
    batch: PreviewGeometryBatch,
) -> Result<(), router_core::state::IpcErrorDto> {
    window
        .set_size(batch.size)
        .map_err(|_| shell_error("menu_resize_failed", "菜单尺寸调整失败。", true))?;
    if let Some(position) = batch.position {
        window
            .set_position(position)
            .map_err(|_| shell_error("menu_position_failed", "菜单定位失败。", true))?;
    }
    Ok(())
}

fn expanded_preview_size(
    base_size: LogicalSize<f64>,
    preview_width: f64,
    preview_height: f64,
) -> LogicalSize<f64> {
    LogicalSize::new(
        base_size.width + MENU_PREVIEW_GAP + preview_width,
        base_size.height.max(preview_height + MENU_VERTICAL_FRAME),
    )
}

fn preview_visible_frame(
    current: PreviewFrame,
    next_size: LogicalSize<f64>,
    preserve_right_edge: bool,
) -> PreviewFrame {
    let x = if preserve_right_edge {
        current.origin.x + current.size.width - next_size.width
    } else {
        current.origin.x
    };
    let y = current.origin.y + current.size.height - next_size.height;
    PreviewFrame {
        origin: LogicalPosition::new(x, y),
        size: next_size,
    }
}

fn preview_backing_frame(
    base_size: LogicalSize<f64>,
    expanded_size: LogicalSize<f64>,
    side: PreviewSide,
    open: bool,
) -> PreviewFrame {
    let origin = if open {
        LogicalPosition::new(0.0, 0.0)
    } else {
        LogicalPosition::new(
            if side == PreviewSide::Left {
                base_size.width - expanded_size.width
            } else {
                0.0
            },
            base_size.height - expanded_size.height,
        )
    };
    PreviewFrame {
        origin,
        size: expanded_size,
    }
}

#[cfg(target_os = "macos")]
fn preview_panel_frame(
    current: PanelRect,
    next_size: LogicalSize<f64>,
    preserve_right_edge: bool,
) -> PanelRect {
    let next = preview_visible_frame(
        PreviewFrame {
            origin: LogicalPosition::new(current.origin.x, current.origin.y),
            size: LogicalSize::new(current.size.width, current.size.height),
        },
        next_size,
        preserve_right_edge,
    );
    PanelRect::new(
        PanelPoint::new(next.origin.x, next.origin.y),
        PanelSize::new(next.size.width, next.size.height),
    )
}

#[cfg(target_os = "macos")]
fn configure_preview_backing<R: Runtime>(
    panel: &tauri_nspanel::PanelHandle<R>,
    frame: PreviewFrame,
) {
    let subviews = panel.content_view().subviews();
    for index in 0..subviews.count() {
        let view = subviews.objectAtIndex(index);
        view.setAutoresizingMask(NSAutoresizingMaskOptions::ViewNotSizable);
        view.setFrame(PanelRect::new(
            PanelPoint::new(frame.origin.x, frame.origin.y),
            PanelSize::new(frame.size.width, frame.size.height),
        ));
    }
}

#[cfg(target_os = "macos")]
fn set_preview_backing_origin<R: Runtime>(
    panel: &tauri_nspanel::PanelHandle<R>,
    origin: LogicalPosition<f64>,
) {
    let subviews = panel.content_view().subviews();
    for index in 0..subviews.count() {
        subviews
            .objectAtIndex(index)
            .setFrameOrigin(PanelPoint::new(origin.x, origin.y));
    }
}

#[cfg(target_os = "macos")]
fn apply_preview_geometry_atomically<R: Runtime>(
    app: &AppHandle<R>,
    window: &tauri::WebviewWindow<R>,
    previous: (LogicalSize<f64>, PreviewSide, f64),
    next: (LogicalSize<f64>, PreviewSide, f64),
) -> Result<(), router_core::state::IpcErrorDto> {
    if previous == next {
        return Ok(());
    }
    let _ = window;
    let panel = app
        .get_webview_panel(MENU_LABEL)
        .map_err(|_| shell_error("menu_window_unavailable", "菜单窗口不可用。", true))?;
    let base_size = if previous.0.width < next.0.width {
        previous.0
    } else {
        next.0
    };
    let expanded_size = if previous.0.width > next.0.width {
        previous.0
    } else {
        next.0
    };
    let open = next.0.width > base_size.width;
    let backing = preview_backing_frame(base_size, expanded_size, next.1, open);
    let next_frame = preview_panel_frame(
        panel.as_panel().frame(),
        next.0,
        next.1 == PreviewSide::Left,
    );
    // The fixed expanded WebView is clipped by the visible panel. Hover
    // transitions move only its origin, avoiding a WebKit backing resize.
    set_preview_backing_origin(&panel, backing.origin);
    panel.as_panel().setFrame_display(next_frame, false);
    Ok(())
}

#[cfg(not(target_os = "macos"))]
fn apply_preview_geometry_atomically<R: Runtime>(
    _app: &AppHandle<R>,
    window: &tauri::WebviewWindow<R>,
    previous: (LogicalSize<f64>, PreviewSide, f64),
    next: (LogicalSize<f64>, PreviewSide, f64),
) -> Result<(), router_core::state::IpcErrorDto> {
    if previous == next {
        return Ok(());
    }
    apply_preview_geometry(window, previous, next)
}

fn apply_menu_usage_preview_transition<R: Runtime>(
    app: &AppHandle<R>,
    controller: &MenuPopoverController,
    generation: u64,
    revision: u64,
    open: bool,
) -> Result<(), router_core::state::IpcErrorDto> {
    let _geometry_guard = controller
        .geometry_gate
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let previous = controller.preview_geometry();
    let previous_state = controller.preview_state();
    if !controller.set_preview(generation, revision, open) {
        return Ok(());
    }
    let Some(previous) = previous else {
        controller.rollback_preview(generation, revision, previous_state.0, previous_state.1);
        return Ok(());
    };
    let Some(next) = controller.preview_geometry() else {
        controller.rollback_preview(generation, revision, previous_state.0, previous_state.1);
        return Ok(());
    };
    if previous == next {
        return Ok(());
    }
    let Some(window) = app.get_webview_window(MENU_LABEL) else {
        controller.rollback_preview(generation, revision, previous_state.0, previous_state.1);
        return Err(shell_error(
            "menu_window_unavailable",
            "菜单窗口不可用。",
            true,
        ));
    };
    if let Err(error) = apply_preview_geometry_atomically(app, &window, previous, next) {
        controller.rollback_preview(generation, revision, previous_state.0, previous_state.1);
        return Err(error);
    }
    Ok(())
}

#[tauri::command]
pub async fn set_menu_usage_preview(
    app: AppHandle,
    controller: State<'_, Arc<MenuPopoverController>>,
    generation: u64,
    revision: u64,
    open: bool,
) -> Result<(), router_core::state::IpcErrorDto> {
    let controller = controller.inner().clone();
    let main_thread_app = app.clone();
    let (completion_sender, completion_receiver) = tokio::sync::oneshot::channel();
    app.run_on_main_thread(move || {
        let result = apply_menu_usage_preview_transition(
            &main_thread_app,
            &controller,
            generation,
            revision,
            open,
        );
        let _ = completion_sender.send(result);
    })
    .map_err(|_| shell_error("menu_resize_failed", "菜单尺寸调整失败。", true))?;
    completion_receiver
        .await
        .map_err(|_| shell_error("menu_resize_failed", "菜单尺寸调整失败。", true))?
}

#[cfg(target_os = "macos")]
async fn show_menu_panel(
    app: &AppHandle,
    controller: Arc<MenuPopoverController>,
    generation: u64,
) -> Result<bool, router_core::state::IpcErrorDto> {
    let panel = app
        .get_webview_panel(MENU_LABEL)
        .map_err(|_| shell_error("menu_show_failed", "菜单无法显示。", true))?;
    let (completion_sender, completion_receiver) = tokio::sync::oneshot::channel();
    app.run_on_main_thread(move || {
        let _geometry_guard = controller
            .geometry_gate
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let backing = controller
            .preview_backing_geometry()
            .map(|(base, expanded, side, open)| preview_backing_frame(base, expanded, side, open));
        let presented = controller.present_if_current(generation, || {
            if let Some(backing) = backing {
                configure_preview_backing(&panel, backing);
            }
            panel.show_and_make_key();
        });
        let _ = completion_sender.send(presented);
    })
    .map_err(|_| shell_error("menu_show_failed", "菜单无法显示。", true))?;
    completion_receiver
        .await
        .map_err(|_| shell_error("menu_show_failed", "菜单无法显示。", true))
}

#[cfg(not(target_os = "macos"))]
async fn show_menu_panel(
    app: &AppHandle,
    controller: Arc<MenuPopoverController>,
    generation: u64,
) -> Result<bool, router_core::state::IpcErrorDto> {
    if !controller.is_current_show(generation) {
        return Ok(false);
    }
    let window = app
        .get_webview_window(MENU_LABEL)
        .ok_or_else(|| shell_error("menu_show_failed", "菜单无法显示。", true))?;
    window
        .show()
        .and_then(|()| window.set_focus())
        .map(|()| true)
        .map_err(|_| shell_error("menu_show_failed", "菜单无法显示。", true))
}

fn menu_window_size(height: f64) -> LogicalSize<f64> {
    LogicalSize::new(
        MENU_WINDOW_WIDTH,
        height.clamp(MENU_MIN_HEIGHT, MENU_MAX_HEIGHT) + MENU_VERTICAL_FRAME,
    )
}

#[tauri::command]
#[expect(
    clippy::needless_pass_by_value,
    reason = "Tauri command handle injection requires AppHandle by value"
)]
pub fn hide_menu(app: AppHandle) {
    hide_menu_window(&app);
}

#[tauri::command]
#[expect(
    clippy::needless_pass_by_value,
    reason = "Tauri command handle injection requires AppHandle by value"
)]
pub fn hide_settings_window(app: AppHandle) {
    if let Some(window) = app.get_webview_window("settings") {
        let _ = window.hide();
    }
}

fn shell_error(code: &str, message: &str, retryable: bool) -> router_core::state::IpcErrorDto {
    router_core::state::IpcErrorDto {
        code: code.to_owned(),
        message: message.to_owned(),
        retryable,
        field: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use router_core::domain::{InferenceStatus, InferenceStatusKind, RouteId};

    fn tray_snapshot(
        route: Option<(RouteId, &str)>,
        proxy_status: ProxyRuntimeStatus,
    ) -> BootstrapSnapshotDto {
        let active_route_id = route.as_ref().map(|(route_id, _)| route_id.clone());
        let routes = route.map_or_else(Vec::new, |(route_id, name)| {
            vec![router_core::state::RouteSummaryDto {
                route_id,
                name: name.to_owned(),
                base_url_host: "example.com".to_owned(),
                inference_status: InferenceStatus {
                    kind: InferenceStatusKind::Unverified,
                    last_outcome: None,
                    failure_reason: None,
                    observed_at_ms: None,
                },
            }]
        });
        BootstrapSnapshotDto {
            revision: 1,
            routes,
            active_route_id,
            fallback: router_core::state::FallbackStateDto::default(),
            proxy_status,
            lifecycle: router_core::lifecycle::AppLifecycleSnapshot::default(),
            appearance_preference: router_core::domain::AppearancePreference::System,
        }
    }

    fn tray_balance(
        route_id: RouteId,
        status: BalanceDisplayStatus,
        remaining: Option<f64>,
        unit: Option<&str>,
    ) -> BalanceDisplaySnapshot {
        BalanceDisplaySnapshot {
            route_id,
            value: Some(BalanceResult {
                is_valid: true,
                remaining,
                used: None,
                total: None,
                unit: unit.map(str::to_owned),
                plan_name: None,
                invalid_message: None,
                extra: None,
            }),
            status,
            last_success_at_ms: Some(1),
            last_completion_at_ms: Some(1),
            next_due_at_ms: None,
            error: None,
        }
    }

    #[test]
    fn stale_blur_generation_cannot_hide_a_reopened_menu() {
        let controller = MenuPopoverController::new();
        let ToggleDecision::Prepare(first) = controller.toggle() else {
            panic!("first toggle opens");
        };
        controller.hide();
        let ToggleDecision::Prepare(second) = controller.toggle() else {
            panic!("second toggle opens");
        };

        assert!(!controller.hide_if_current(first));
        assert!(controller.is_current_show(second));
    }

    #[test]
    fn queued_blur_hide_cannot_hide_a_newer_show_generation() {
        let controller = MenuPopoverController::new();
        let first = controller.request_show();
        assert!(controller.hide_if_current(first));
        let hidden_generation = controller.generation();

        let second = controller.request_show();

        assert!(!controller.is_current_hide(hidden_generation));
        assert!(controller.is_current_show(second));
    }

    #[test]
    fn stale_generation_cannot_run_queued_presentation() {
        let controller = MenuPopoverController::new();
        let first = controller.request_show();
        let second = controller.request_show();
        let mut presentation_count = 0;

        assert!(!controller.present_if_current(first, || presentation_count += 1));
        assert_eq!(presentation_count, 0);
        assert!(controller.present_if_current(second, || presentation_count += 1));
        assert_eq!(presentation_count, 1);
    }

    #[test]
    fn arrow_offset_is_scaled_and_clamped_inside_the_popover() {
        let placement = MenuPlacement {
            position: PhysicalPosition::new(800, 0),
            tray_center_x: 1_000.0,
            scale_factor: 2.0,
        };
        assert!((placement.arrow_offset() - 100.0).abs() < f64::EPSILON);

        let clamped = MenuPlacement {
            position: PhysicalPosition::new(1_200, 0),
            ..placement
        };
        assert!((clamped.arrow_offset() - MENU_ARROW_MIN_X).abs() < f64::EPSILON);
    }

    #[test]
    fn mixed_scale_placement_discards_the_previous_displays_physical_window_size() {
        let monitors = [
            MonitorBounds {
                position: PhysicalPosition::new(0, 0),
                size: PhysicalSize::new(1920, 1080),
                work_area_position: PhysicalPosition::new(0, 30),
                work_area_size: PhysicalSize::new(1920, 1050),
                scale_factor: 1.0,
            },
            MonitorBounds {
                position: PhysicalPosition::new(1920, 0),
                size: PhysicalSize::new(3024, 1964),
                work_area_position: PhysicalPosition::new(1920, 50),
                work_area_size: PhysicalSize::new(3024, 1914),
                scale_factor: 2.0,
            },
        ];
        let external_logical_size =
            window_logical_size(PhysicalSize::new(384, 274), 1.0).expect("external logical size");
        let built_in_logical_size =
            window_logical_size(PhysicalSize::new(768, 548), 2.0).expect("built-in logical size");
        assert_eq!(external_logical_size, built_in_logical_size);

        let external_to_built_in = menu_placement_for_tray(
            TrayAnchor {
                event_position: PhysicalPosition::new(4_000.0, 25.0),
                center_x: 4_000.0,
                height: 50.0,
            },
            &monitors,
            external_logical_size,
        )
        .expect("built-in placement from external window state");
        let built_in_to_external = menu_placement_for_tray(
            TrayAnchor {
                event_position: PhysicalPosition::new(1_700.0, 15.0),
                center_x: 1_700.0,
                height: 30.0,
            },
            &monitors,
            built_in_logical_size,
        )
        .expect("external placement from built-in window state");

        assert_eq!(external_to_built_in.position.x, 3_616);
        assert_eq!(built_in_to_external.position.x, 1_508);
        assert!((external_to_built_in.arrow_offset() - 192.0).abs() < f64::EPSILON);
        assert!((built_in_to_external.arrow_offset() - 192.0).abs() < f64::EPSILON);
    }

    #[test]
    fn menu_window_size_uses_the_compact_minimum_and_reserves_the_transparent_frame() {
        assert_eq!(menu_window_size(188.0), LogicalSize::new(384.0, 222.0));
        assert_eq!(menu_window_size(1.0), LogicalSize::new(384.0, 222.0));
        assert_eq!(menu_window_size(1_000.0), LogicalSize::new(384.0, 674.0));
    }

    #[test]
    fn usage_preview_revision_ordering_restores_the_base_geometry() {
        let controller = MenuPopoverController::new();
        let generation = controller.request_show();
        let base = menu_window_size(320.0);
        controller.record_base_menu_size(
            generation,
            base,
            PreviewSide::Left,
            MENU_PREVIEW_WIDTH,
            MENU_PREVIEW_HEIGHT,
        );

        assert!(controller.set_preview(generation, 2, true));
        assert_eq!(
            controller.preview_geometry(),
            Some((
                LogicalSize::new(
                    MENU_WINDOW_WIDTH + MENU_PREVIEW_GAP + MENU_PREVIEW_WIDTH,
                    MENU_PREVIEW_HEIGHT + MENU_VERTICAL_FRAME,
                ),
                PreviewSide::Left,
                MENU_WINDOW_WIDTH,
            )),
        );
        assert!(!controller.set_preview(generation, 2, false));
        assert!(!controller.set_preview(generation, 1, false));
        assert!(controller.set_preview(generation, 3, false));
        assert_eq!(
            controller.preview_geometry(),
            Some((base, PreviewSide::Left, MENU_WINDOW_WIDTH)),
        );
        let newer_generation = controller.request_show();
        assert_ne!(newer_generation, generation);
        assert_eq!(controller.base_geometry(), None);
        assert!(!controller.set_preview(generation, 4, true));
    }

    #[test]
    fn left_preview_open_and_close_are_complete_geometry_batches() {
        let base = menu_window_size(320.0);
        let expanded = LogicalSize::new(
            MENU_WINDOW_WIDTH + MENU_PREVIEW_GAP + MENU_PREVIEW_WIDTH,
            MENU_PREVIEW_HEIGHT + MENU_VERTICAL_FRAME,
        );
        let base_geometry = (base, PreviewSide::Left, MENU_WINDOW_WIDTH);
        let expanded_geometry = (expanded, PreviewSide::Left, MENU_WINDOW_WIDTH);

        let open = preview_geometry_batch(
            base_geometry,
            expanded_geometry,
            PhysicalPosition::new(904, 30),
            2.0,
        )
        .expect("left preview open batch");
        assert_eq!(
            open,
            PreviewGeometryBatch {
                size: expanded,
                position: Some(PhysicalPosition::new(120, 30)),
            }
        );

        let close = preview_geometry_batch(
            expanded_geometry,
            base_geometry,
            PhysicalPosition::new(120, 30),
            2.0,
        )
        .expect("left preview close batch");
        assert_eq!(
            close,
            PreviewGeometryBatch {
                size: base,
                position: Some(PhysicalPosition::new(904, 30)),
            }
        );
    }

    #[test]
    fn right_preview_geometry_batch_never_moves_the_menu_origin() {
        let base = menu_window_size(320.0);
        let expanded = LogicalSize::new(
            MENU_WINDOW_WIDTH + MENU_PREVIEW_GAP + MENU_PREVIEW_WIDTH,
            MENU_PREVIEW_HEIGHT + MENU_VERTICAL_FRAME,
        );
        let close = preview_geometry_batch(
            (expanded, PreviewSide::Right, MENU_WINDOW_WIDTH),
            (base, PreviewSide::Right, MENU_WINDOW_WIDTH),
            PhysicalPosition::new(120, 30),
            2.0,
        )
        .expect("right preview close batch");

        assert_eq!(
            close,
            PreviewGeometryBatch {
                size: base,
                position: None,
            }
        );
    }

    #[test]
    fn preview_backing_keeps_expanded_size_and_uses_side_aware_collapsed_origins() {
        let base = menu_window_size(320.0);
        let expanded = expanded_preview_size(base, MENU_PREVIEW_WIDTH, MENU_PREVIEW_HEIGHT);

        let right_open = preview_backing_frame(base, expanded, PreviewSide::Right, true);
        let right_closed = preview_backing_frame(base, expanded, PreviewSide::Right, false);
        let left_open = preview_backing_frame(base, expanded, PreviewSide::Left, true);
        let left_closed = preview_backing_frame(base, expanded, PreviewSide::Left, false);

        assert_eq!(right_open.origin, LogicalPosition::new(0.0, 0.0));
        assert_eq!(left_open.origin, LogicalPosition::new(0.0, 0.0));
        assert_eq!(right_closed.origin, LogicalPosition::new(0.0, -160.0));
        assert_eq!(left_closed.origin, LogicalPosition::new(-392.0, -160.0));
        assert_eq!(right_open.size, expanded);
        assert_eq!(right_closed.size, expanded);
        assert_eq!(left_open.size, expanded);
        assert_eq!(left_closed.size, expanded);
    }

    #[test]
    fn fixed_backing_preserves_the_menu_screen_rectangle_on_both_preview_sides() {
        fn menu_screen_frame(
            panel: PreviewFrame,
            backing: PreviewFrame,
            base: LogicalSize<f64>,
            expanded: LogicalSize<f64>,
            side: PreviewSide,
        ) -> PreviewFrame {
            let menu_origin = LogicalPosition::new(
                if side == PreviewSide::Left {
                    expanded.width - base.width
                } else {
                    0.0
                },
                expanded.height - base.height,
            );
            PreviewFrame {
                origin: LogicalPosition::new(
                    panel.origin.x + backing.origin.x + menu_origin.x,
                    panel.origin.y + backing.origin.y + menu_origin.y,
                ),
                size: base,
            }
        }

        let base = menu_window_size(320.0);
        let expanded = expanded_preview_size(base, MENU_PREVIEW_WIDTH, MENU_PREVIEW_HEIGHT);
        for (side, collapsed_x) in [(PreviewSide::Left, 904.0), (PreviewSide::Right, 120.0)] {
            let collapsed_panel = PreviewFrame {
                origin: LogicalPosition::new(collapsed_x, 400.0),
                size: base,
            };
            let expanded_panel =
                preview_visible_frame(collapsed_panel, expanded, side == PreviewSide::Left);
            let collapsed_menu = menu_screen_frame(
                collapsed_panel,
                preview_backing_frame(base, expanded, side, false),
                base,
                expanded,
                side,
            );
            let expanded_menu = menu_screen_frame(
                expanded_panel,
                preview_backing_frame(base, expanded, side, true),
                base,
                expanded,
                side,
            );

            assert_eq!(collapsed_menu, expanded_menu);
            assert_eq!(
                collapsed_menu.origin,
                LogicalPosition::new(collapsed_x, 400.0)
            );
            assert_eq!(collapsed_menu.size, base);
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn left_preview_appkit_frame_preserves_top_and_right_edges_in_one_rect() {
        let base_size = menu_window_size(320.0);
        let expanded_size = LogicalSize::new(
            MENU_WINDOW_WIDTH + MENU_PREVIEW_GAP + MENU_PREVIEW_WIDTH,
            MENU_PREVIEW_HEIGHT + MENU_VERTICAL_FRAME,
        );
        let base_frame = PanelRect::new(
            PanelPoint::new(904.0, 400.0),
            PanelSize::new(base_size.width, base_size.height),
        );

        let expanded_frame = preview_panel_frame(base_frame, expanded_size, true);
        assert_eq!(expanded_frame.origin, PanelPoint::new(512.0, 240.0));
        assert_eq!(expanded_frame.size, PanelSize::new(776.0, 514.0));
        assert!(
            ((expanded_frame.origin.x + expanded_frame.size.width)
                - (base_frame.origin.x + base_frame.size.width))
                .abs()
                < f64::EPSILON,
        );
        assert!(
            ((expanded_frame.origin.y + expanded_frame.size.height)
                - (base_frame.origin.y + base_frame.size.height))
                .abs()
                < f64::EPSILON,
        );

        assert_eq!(
            preview_panel_frame(expanded_frame, base_size, true),
            base_frame,
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn right_preview_appkit_frame_preserves_top_and_left_edges() {
        let base_size = menu_window_size(320.0);
        let expanded_size = LogicalSize::new(
            MENU_WINDOW_WIDTH + MENU_PREVIEW_GAP + MENU_PREVIEW_WIDTH,
            MENU_PREVIEW_HEIGHT + MENU_VERTICAL_FRAME,
        );
        let base_frame = PanelRect::new(
            PanelPoint::new(120.0, 400.0),
            PanelSize::new(base_size.width, base_size.height),
        );

        let expanded_frame = preview_panel_frame(base_frame, expanded_size, false);
        assert_eq!(expanded_frame.origin, PanelPoint::new(120.0, 240.0));
        assert_eq!(expanded_frame.size, PanelSize::new(776.0, 514.0));
        assert!(
            ((expanded_frame.origin.y + expanded_frame.size.height)
                - (base_frame.origin.y + base_frame.size.height))
                .abs()
                < f64::EPSILON,
        );
    }

    #[test]
    fn tray_toggle_keeps_expanded_geometry_available_for_hide_restoration() {
        let controller = MenuPopoverController::new();
        let ToggleDecision::Prepare(generation) = controller.toggle() else {
            panic!("first toggle opens");
        };
        controller.record_base_menu_size(
            generation,
            menu_window_size(320.0),
            PreviewSide::Left,
            MENU_PREVIEW_WIDTH,
            MENU_PREVIEW_HEIGHT,
        );
        assert!(controller.set_preview(generation, 1, true));

        assert!(matches!(controller.toggle(), ToggleDecision::Hide));
        assert_eq!(
            controller
                .preview_geometry()
                .map(|geometry| geometry.0.width),
            Some(MENU_WINDOW_WIDTH + MENU_PREVIEW_GAP + MENU_PREVIEW_WIDTH),
        );
        controller.reset_preview();
        assert_eq!(
            controller
                .preview_geometry()
                .map(|geometry| geometry.0.width),
            Some(MENU_WINDOW_WIDTH),
        );
    }

    #[test]
    fn usage_preview_layout_prefers_left_then_right_and_bounds_the_larger_side() {
        let monitor = MonitorBounds {
            position: PhysicalPosition::new(0, 0),
            size: PhysicalSize::new(1200, 800),
            work_area_position: PhysicalPosition::new(0, 30),
            work_area_size: PhysicalSize::new(1200, 770),
            scale_factor: 1.0,
        };
        let placement = |x| MenuPlacement {
            position: PhysicalPosition::new(x, 30),
            tray_center_x: f64::from(x) + MENU_WINDOW_WIDTH / 2.0,
            scale_factor: 1.0,
        };
        let tray = |x| TrayAnchor {
            event_position: PhysicalPosition::new(x, 15.0),
            center_x: x,
            height: 30.0,
        };

        assert_eq!(
            preview_layout_for_tray(tray(800.0), &[monitor], placement(700)),
            (PreviewSide::Left, MENU_PREVIEW_WIDTH, MENU_PREVIEW_HEIGHT),
        );
        assert_eq!(
            preview_layout_for_tray(tray(200.0), &[monitor], placement(100)),
            (PreviewSide::Right, MENU_PREVIEW_WIDTH, MENU_PREVIEW_HEIGHT),
        );

        let narrow = MonitorBounds {
            size: PhysicalSize::new(700, 800),
            work_area_size: PhysicalSize::new(700, 770),
            ..monitor
        };
        assert_eq!(
            preview_layout_for_tray(tray(350.0), &[narrow], placement(158)),
            (PreviewSide::Left, 150.0, MENU_PREVIEW_HEIGHT),
        );

        let short = MonitorBounds {
            size: PhysicalSize::new(1_200, 420),
            work_area_size: PhysicalSize::new(1_200, 390),
            ..monitor
        };
        assert_eq!(
            preview_layout_for_tray(tray(800.0), &[short], placement(700)),
            (PreviewSide::Left, MENU_PREVIEW_WIDTH, 356.0),
        );
    }

    #[test]
    fn vertically_stacked_tray_uses_the_matching_monitor_virtual_y() {
        let monitors = [
            MonitorBounds {
                position: PhysicalPosition::new(0, 0),
                size: PhysicalSize::new(1512, 982),
                work_area_position: PhysicalPosition::new(0, 30),
                work_area_size: PhysicalSize::new(1512, 952),
                scale_factor: 1.0,
            },
            MonitorBounds {
                position: PhysicalPosition::new(1258, -1080),
                size: PhysicalSize::new(1920, 1080),
                work_area_position: PhysicalPosition::new(1258, -1050),
                work_area_size: PhysicalSize::new(1920, 1050),
                scale_factor: 1.0,
            },
        ];
        let position = popover_position_for_tray(
            TrayAnchor {
                event_position: PhysicalPosition::new(2130.0, -1065.0),
                center_x: 2130.0,
                height: 30.0,
            },
            &monitors,
            LogicalSize::new(384.0, 274.0),
        );

        assert_eq!(position, Some(PhysicalPosition::new(1938, -1050)));
    }

    #[test]
    fn tray_popover_horizontal_position_is_clamped_to_monitor_edges() {
        let monitor = MonitorBounds {
            position: PhysicalPosition::new(0, 0),
            size: PhysicalSize::new(800, 600),
            work_area_position: PhysicalPosition::new(0, 30),
            work_area_size: PhysicalSize::new(800, 570),
            scale_factor: 1.0,
        };
        let position = popover_position_for_tray(
            TrayAnchor {
                event_position: PhysicalPosition::new(795.0, 15.0),
                center_x: 795.0,
                height: 30.0,
            },
            &[monitor],
            LogicalSize::new(384.0, 274.0),
        );

        assert_eq!(position, Some(PhysicalPosition::new(416, 30)));
    }

    #[test]
    fn overlapping_scaled_bounds_choose_the_nearest_menu_bar_independent_of_order() {
        let built_in = MonitorBounds {
            position: PhysicalPosition::new(0, 0),
            size: PhysicalSize::new(3024, 1964),
            work_area_position: PhysicalPosition::new(0, 50),
            work_area_size: PhysicalSize::new(3024, 1814),
            scale_factor: 2.0,
        };
        let external = MonitorBounds {
            position: PhysicalPosition::new(1512, 680),
            size: PhysicalSize::new(1920, 1080),
            work_area_position: PhysicalPosition::new(1512, 710),
            work_area_size: PhysicalSize::new(1920, 1050),
            scale_factor: 1.0,
        };
        let tray = TrayAnchor {
            event_position: PhysicalPosition::new(2000.0, 700.0),
            center_x: 2000.0,
            height: 30.0,
        };
        let expected = Some(PhysicalPosition::new(1808, 710));

        assert_eq!(
            popover_position_for_tray(tray, &[built_in, external], LogicalSize::new(384.0, 274.0),),
            expected
        );
        assert_eq!(
            popover_position_for_tray(tray, &[external, built_in], LogicalSize::new(384.0, 274.0),),
            expected
        );
    }

    #[test]
    fn ambiguous_monitor_match_uses_the_existing_fallback_path() {
        let monitors = [
            MonitorBounds {
                position: PhysicalPosition::new(0, 0),
                size: PhysicalSize::new(1600, 900),
                work_area_position: PhysicalPosition::new(0, 30),
                work_area_size: PhysicalSize::new(1600, 870),
                scale_factor: 1.0,
            },
            MonitorBounds {
                position: PhysicalPosition::new(800, 0),
                size: PhysicalSize::new(1600, 900),
                work_area_position: PhysicalPosition::new(800, 30),
                work_area_size: PhysicalSize::new(1600, 870),
                scale_factor: 1.0,
            },
        ];
        let tray = TrayAnchor {
            event_position: PhysicalPosition::new(1000.0, 15.0),
            center_x: 1000.0,
            height: 30.0,
        };

        assert_eq!(
            popover_position_for_tray(tray, &monitors, LogicalSize::new(384.0, 274.0)),
            None
        );
    }

    #[test]
    fn settings_position_centres_in_the_selected_monitor_logical_work_area() {
        let monitors = [
            MonitorBounds {
                position: PhysicalPosition::new(0, 0),
                size: PhysicalSize::new(3024, 1964),
                work_area_position: PhysicalPosition::new(0, 50),
                work_area_size: PhysicalSize::new(3024, 1814),
                scale_factor: 2.0,
            },
            MonitorBounds {
                position: PhysicalPosition::new(1512, 680),
                size: PhysicalSize::new(1920, 1080),
                work_area_position: PhysicalPosition::new(1512, 710),
                work_area_size: PhysicalSize::new(1920, 1050),
                scale_factor: 1.0,
            },
        ];
        let tray = TrayAnchor {
            event_position: PhysicalPosition::new(2000.0, 700.0),
            center_x: 2000.0,
            height: 30.0,
        };

        assert_eq!(
            settings_position_for_tray(tray, &monitors, LogicalSize::new(920.0, 680.0)),
            Some(LogicalPosition::new(2012.0, 895.0))
        );
    }

    #[test]
    fn tray_visual_depends_only_on_logical_request_activity() {
        let snapshot = |proxy_status, active_route_id| BootstrapSnapshotDto {
            revision: 0,
            routes: Vec::new(),
            active_route_id,
            fallback: router_core::state::FallbackStateDto::default(),
            proxy_status,
            lifecycle: router_core::lifecycle::AppLifecycleSnapshot::default(),
            appearance_preference: router_core::domain::AppearancePreference::System,
        };
        for state in [
            snapshot(ProxyRuntimeStatus::Running, Some(RouteId::new())),
            snapshot(ProxyRuntimeStatus::Running, None),
            snapshot(ProxyRuntimeStatus::PortConflict, None),
        ] {
            assert_eq!(tray_visual_state(false), TrayVisualState::Ready);
            assert_eq!(tray_visual_state(true), TrayVisualState::Active);
            assert_ne!(tray_status_label(&state, false), "处理中");
        }
        assert_eq!(
            tray_status_label(
                &snapshot(ProxyRuntimeStatus::Running, Some(RouteId::new())),
                true,
            ),
            "处理中"
        );
        assert_eq!(
            tray_status_label(&snapshot(ProxyRuntimeStatus::PortConflict, None), true),
            "代理故障"
        );
    }

    #[test]
    fn tray_title_projects_route_balance_and_qa_identity() {
        let route_id = RouteId::new();
        let snapshot = tray_snapshot(
            Some((route_id.clone(), "INPUT")),
            ProxyRuntimeStatus::Running,
        );
        let balance = tray_balance(
            route_id,
            BalanceDisplayStatus::Fresh,
            Some(24.8),
            Some("USD"),
        );

        let production = tray_presentation("AI Router", &snapshot, Some(&balance), false, false);
        let isolated = tray_presentation("AI Router QA", &snapshot, Some(&balance), true, false);

        assert_eq!(production.title, "INPUT($24.80)");
        assert_eq!(isolated.title, "QA · INPUT($24.80)");
        assert!(production.tooltip.contains("正常 · INPUT · 余额 $24.80"));

        let active = tray_presentation("AI Router", &snapshot, Some(&balance), false, true);
        assert_eq!(active.title, production.title);
        assert!(active.tooltip.contains("处理中 · INPUT · 余额 $24.80"));
        assert_eq!(active.visual_state, TrayVisualState::Active);
    }

    #[test]
    fn tray_title_keeps_cached_value_while_refreshing_and_uses_fallback_without_one() {
        let route_id = RouteId::new();
        let snapshot = tray_snapshot(
            Some((route_id.clone(), "INPUT")),
            ProxyRuntimeStatus::Running,
        );
        let balance = tray_balance(
            route_id,
            BalanceDisplayStatus::Refreshing,
            Some(9.5),
            Some("$"),
        );

        let refreshing = tray_presentation("AI Router", &snapshot, Some(&balance), false, false);
        let unavailable = tray_presentation("AI Router", &snapshot, None, false, false);

        assert_eq!(refreshing.title, "INPUT($9.50)");
        assert!(refreshing.tooltip.contains("正在刷新"));
        assert_eq!(unavailable.title, "INPUT(--)");
    }

    #[test]
    fn tray_title_covers_stale_last_good_and_failed_balance_states() {
        let route_id = RouteId::new();
        let snapshot = tray_snapshot(
            Some((route_id.clone(), "INPUT")),
            ProxyRuntimeStatus::Running,
        );

        for (status, expected_tooltip) in [
            (BalanceDisplayStatus::Stale, "已过期"),
            (BalanceDisplayStatus::LastGood, "上次余额"),
        ] {
            let balance = tray_balance(route_id.clone(), status, Some(12.5), Some("$"));
            let presentation =
                tray_presentation("AI Router", &snapshot, Some(&balance), false, false);

            assert_eq!(presentation.title, "INPUT($12.50)");
            assert!(presentation.tooltip.contains(expected_tooltip));
        }

        let mut failed = tray_balance(
            route_id,
            BalanceDisplayStatus::Failed,
            Some(12.5),
            Some("$"),
        );
        failed.value = None;
        let presentation = tray_presentation("AI Router", &snapshot, Some(&failed), false, false);

        assert_eq!(presentation.title, "INPUT(--)");
        assert!(presentation.tooltip.contains("余额查询失败"));
    }

    #[test]
    fn tray_title_uses_no_route_fallback_and_keeps_proxy_failure_in_tooltip() {
        let empty = tray_snapshot(None, ProxyRuntimeStatus::Stopped);
        assert_eq!(
            tray_presentation("AI Router", &empty, None, false, false).title,
            "无中转(--)"
        );

        let route_id = RouteId::new();
        let failed = tray_snapshot(Some((route_id, "INPUT")), ProxyRuntimeStatus::PortConflict);
        let presentation = tray_presentation("AI Router", &failed, None, false, true);
        assert_eq!(presentation.title, "INPUT(--)");
        assert!(presentation.tooltip.contains("代理故障"));
    }

    #[test]
    fn tray_title_ignores_a_balance_snapshot_for_another_route() {
        let active_route_id = RouteId::new();
        let snapshot = tray_snapshot(
            Some((active_route_id, "INPUT")),
            ProxyRuntimeStatus::Running,
        );
        let other_balance = tray_balance(
            RouteId::new(),
            BalanceDisplayStatus::Fresh,
            Some(24.8),
            Some("USD"),
        );

        let presentation =
            tray_presentation("AI Router", &snapshot, Some(&other_balance), false, false);

        assert_eq!(presentation.title, "INPUT(--)");
        assert!(presentation.tooltip.contains("尚无余额"));
    }

    #[test]
    fn tray_route_name_uses_grapheme_safe_middle_ellipsis() {
        assert_eq!(compact_route_name("AI INPUT 工作账号"), "AI INP…工作账号");
        let family_name = "ABCDE👨‍👩‍👧‍👦FGHIJKLM";
        let compact = compact_route_name(family_name);
        assert!(compact.contains("👨‍👩‍👧‍👦"));
        assert!(UnicodeSegmentation::graphemes(compact.as_str(), true).count() <= 12);
    }

    #[test]
    fn tray_balance_is_bounded_without_changing_normal_currency_format() {
        let normal = BalanceResult {
            is_valid: true,
            remaining: Some(24.8),
            used: None,
            total: None,
            unit: Some("USD".to_owned()),
            plan_name: None,
            invalid_message: None,
            extra: None,
        };
        let oversized = BalanceResult {
            remaining: Some(1.0e20),
            unit: Some("very-long-custom-unit".to_owned()),
            ..normal.clone()
        };

        assert_eq!(compact_balance_value(&normal), "$24.80");
        assert_eq!(
            UnicodeSegmentation::graphemes(compact_balance_value(&oversized).as_str(), true)
                .count(),
            TRAY_BALANCE_GRAPHEMES
        );
        assert!(compact_balance_value(&oversized).ends_with('…'));

        let route_id = RouteId::new();
        let snapshot = tray_snapshot(
            Some((route_id.clone(), "INPUT")),
            ProxyRuntimeStatus::Running,
        );
        let balance = BalanceDisplaySnapshot {
            route_id,
            value: Some(oversized.clone()),
            status: BalanceDisplayStatus::Fresh,
            last_success_at_ms: Some(1),
            last_completion_at_ms: Some(1),
            next_due_at_ms: None,
            error: None,
        };
        let presentation = tray_presentation("AI Router", &snapshot, Some(&balance), false, false);

        assert!(presentation.title.contains('…'));
        assert!(
            presentation
                .tooltip
                .contains(&format_balance_value(&oversized))
        );
    }

    #[test]
    fn tray_state_icons_are_transparent_monochrome_templates() {
        for (name, bytes) in [
            (
                "ready",
                include_bytes!("../icons/tray-route.png").as_slice(),
            ),
            (
                "active-static",
                include_bytes!("../icons/tray-active-static.png").as_slice(),
            ),
            (
                "active-a",
                include_bytes!("../icons/tray-active-a.png").as_slice(),
            ),
            (
                "active-b",
                include_bytes!("../icons/tray-active-b.png").as_slice(),
            ),
        ] {
            let image = tauri::image::Image::from_bytes(bytes).expect("valid tray PNG");
            assert_eq!((image.width(), image.height()), (44, 44));
            assert!(
                image.rgba().chunks_exact(4).any(|pixel| pixel[3] == 0),
                "{name} needs a fully transparent pixel"
            );
            assert!(
                image.rgba().chunks_exact(4).any(|pixel| pixel[3] > 0),
                "{name} needs a visible pixel"
            );
            assert!(
                image
                    .rgba()
                    .chunks_exact(4)
                    .all(|pixel| pixel[3] == 0 || pixel[..3] == [0, 0, 0]),
                "{name} visible pixels must be black"
            );
        }
    }
}
