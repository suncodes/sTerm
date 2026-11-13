//! Agent 状态悬浮窗：独立窗口与持久化设置，状态由主窗口发布。
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use tauri::{AppHandle, Emitter, Manager, PhysicalPosition, PhysicalSize, WebviewUrl, WebviewWindow};

use crate::{diagnostics, storage};

const LABEL: &str = "agent-overlay";
const WIDTH: f64 = 264.0;
const SNAP_DISTANCE: f64 = 24.0;
const EXPOSED_SIZE: f64 = 16.0;
const COLLAPSED_THICKNESS: f64 = 28.0;
const COLLAPSED_SLOT: f64 = 22.0;

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
enum DockEdge { Left, Right, Top, Bottom }

impl DockEdge {
    fn is_vertical(self) -> bool { matches!(self, Self::Left | Self::Right) }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(default)]
struct Preferences {
    enabled: bool,
    position: Option<SavedPosition>,
    background_opacity: u8,
    dock: Option<DockEdge>,
    collapsed: bool,
}

impl Default for Preferences {
    fn default() -> Self {
        Self {
            enabled: false,
            position: None,
            background_opacity: 80,
            dock: None,
            collapsed: false,
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
struct SavedPosition {
    x: i32,
    y: i32,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Agent { Claude, Codex, Pi }

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Status { Running, Idle, Exited }

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct Session {
    id: u32,
    title: String,
    agent: Agent,
    status: Status,
    active: bool,
}

#[derive(Clone, Serialize)]
pub struct Snapshot {
    enabled: bool,
    background_opacity: u8,
    dock: Option<DockEdge>,
    collapsed: bool,
    revision: u64,
    sessions: Vec<Session>,
}

struct Inner {
    prefs: Preferences,
    ready: bool,
    revision: u64,
    sessions: Vec<Session>,
}

pub struct OverlayState {
    file: PathBuf,
    inner: Mutex<Inner>,
}

impl OverlayState {
    pub fn load(app: &AppHandle) -> Result<Self, String> {
        let file = app.path().app_config_dir().map_err(|e| e.to_string())?.join("overlay.json");
        let prefs = match storage::read_json::<Preferences>(&file) {
            Ok(prefs) => prefs.unwrap_or_default(),
            Err(error) => {
                // 损坏的可选设置不会阻断终端启动，后续写入仍会校验并保留原文件。
                diagnostics::record(&format!("overlay preferences: {error}"));
                Preferences::default()
            }
        };
        Ok(Self { file, inner: Mutex::new(Inner { prefs, ready: false, revision: 0, sessions: Vec::new() }) })
    }

    fn snapshot(&self) -> Result<Snapshot, String> {
        let inner = self.inner.lock().map_err(|e| e.to_string())?;
        Ok(Snapshot {
            enabled: inner.prefs.enabled,
            background_opacity: inner.prefs.background_opacity.min(100),
            dock: inner.prefs.dock,
            // 空状态在边缘也使用中性把手，避免会话结束后重新弹出完整窗口。
            collapsed: uses_collapsed_layout(&inner.prefs),
            revision: inner.revision,
            sessions: inner.sessions.clone(),
        })
    }

    fn update_preferences(&self, change: impl FnOnce(&mut Preferences)) -> Result<(), String> {
        let mut inner = self.inner.lock().map_err(|e| e.to_string())?;
        // 每次修改重新读取，避免多个 sTerm 实例丢失对方的窗口位置。
        let prefs = save_preferences(&self.file, change)?;
        inner.prefs = prefs;
        inner.revision += 1;
        Ok(())
    }

    fn update_preferences_if_changed(&self, change: impl FnOnce(&mut Preferences)) -> Result<bool, String> {
        let mut inner = self.inner.lock().map_err(|e| e.to_string())?;
        let (prefs, wrote) = save_preferences_if_changed(&self.file, change)?;
        if !wrote && inner.prefs == prefs {
            return Ok(false);
        }
        inner.prefs = prefs;
        inner.revision += 1;
        Ok(true)
    }
}

fn save_preferences(file: &Path, change: impl FnOnce(&mut Preferences)) -> Result<Preferences, String> {
    let _lock = storage::lock(file)?;
    let mut prefs = storage::read_json::<Preferences>(file)?.unwrap_or_default();
    change(&mut prefs);
    normalize_preferences(&mut prefs);
    let bytes = serde_json::to_vec_pretty(&prefs).map_err(|e| e.to_string())?;
    storage::write(file, &bytes, true)?;
    Ok(prefs)
}

fn normalize_preferences(prefs: &mut Preferences) {
    prefs.background_opacity = prefs.background_opacity.min(100);
    if prefs.dock.is_none() {
        prefs.collapsed = false;
    }
}

fn uses_collapsed_layout(prefs: &Preferences) -> bool {
    prefs.collapsed && prefs.dock.is_some()
}

fn save_preferences_if_changed(
    file: &Path,
    change: impl FnOnce(&mut Preferences),
) -> Result<(Preferences, bool), String> {
    let _lock = storage::lock(file)?;
    let mut prefs = storage::read_json::<Preferences>(file)?.unwrap_or_default();
    let before = prefs.clone();
    change(&mut prefs);
    normalize_preferences(&mut prefs);
    if prefs == before {
        return Ok((prefs, false));
    }
    let bytes = serde_json::to_vec_pretty(&prefs).map_err(|e| e.to_string())?;
    storage::write(file, &bytes, true)?;
    Ok((prefs, true))
}

fn require_window(window: &WebviewWindow, labels: &[&str]) -> Result<(), String> {
    if labels.contains(&window.label()) { Ok(()) } else { Err("此窗口无权执行该操作".into()) }
}

/// 串行执行窗口操作；异步命令释放 IPC 后再调度 UI 线程，避免阻塞消息循环。
async fn on_ui<T: Send + 'static>(
    app: AppHandle,
    action: impl FnOnce(&AppHandle) -> Result<T, String> + Send + 'static,
) -> Result<T, String> {
    let (tx, mut rx) = tauri::async_runtime::channel(1);
    let handle = app.clone();
    app.run_on_main_thread(move || { let _ = tx.try_send(action(&handle)); }).map_err(|e| e.to_string())?;
    rx.recv().await.ok_or_else(|| "窗口任务已取消".to_string())?
}

fn emit_snapshot(app: &AppHandle) -> Result<Snapshot, String> {
    let snapshot = app.state::<OverlayState>().snapshot()?;
    app.emit_to("main", "overlay-changed", &snapshot).map_err(|e| e.to_string())?;
    if app.get_webview_window(LABEL).is_some() {
        app.emit_to(LABEL, "overlay-changed", &snapshot).map_err(|e| e.to_string())?;
    }
    Ok(snapshot)
}

fn height(session_count: usize) -> f64 {
    // 固定行高，上限六行；更多会话可在列表内滚动。
    46.0 + session_count.clamp(1, 6) as f64 * 32.0
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct WorkArea { x: i32, y: i32, width: u32, height: u32 }

impl WorkArea {
    fn right(self) -> i64 { i64::from(self.x) + i64::from(self.width) }
    fn bottom(self) -> i64 { i64::from(self.y) + i64::from(self.height) }
}

fn pixels(value: f64, scale: f64) -> u32 {
    (value * scale).round().max(1.0) as u32
}

fn clamp_position(position: SavedPosition, size: PhysicalSize<u32>, area: WorkArea) -> SavedPosition {
    let max_x = area.right() - i64::from(size.width);
    let max_y = area.bottom() - i64::from(size.height);
    SavedPosition {
        x: i64::from(position.x).clamp(i64::from(area.x), max_x.max(i64::from(area.x))) as i32,
        y: i64::from(position.y).clamp(i64::from(area.y), max_y.max(i64::from(area.y))) as i32,
    }
}

fn default_position(area: WorkArea, size: PhysicalSize<u32>) -> SavedPosition {
    clamp_position(SavedPosition {
        x: (area.right() - i64::from(size.width) - 20) as i32,
        y: area.y.saturating_add(60),
    }, size, area)
}

fn nearest_area(position: SavedPosition, size: PhysicalSize<u32>, areas: &[WorkArea]) -> Option<(WorkArea, SavedPosition)> {
    areas.iter().copied().map(|area| {
        let fitted = clamp_position(position, size, area);
        let dx = i64::from(fitted.x) - i64::from(position.x);
        let dy = i64::from(fitted.y) - i64::from(position.y);
        (i128::from(dx) * i128::from(dx) + i128::from(dy) * i128::from(dy), area, fitted)
    }).min_by_key(|(distance, _, _)| *distance)
        .map(|(_, area, fitted)| (area, fitted))
}

/// 保留多屏负坐标，屏幕移除后放回最近的可用工作区。
fn fit_position(position: Option<SavedPosition>, size: PhysicalSize<u32>, areas: &[WorkArea]) -> Option<SavedPosition> {
    let first = areas.first()?;
    let desired = position.unwrap_or_else(|| default_position(*first, size));
    nearest_area(desired, size, areas).map(|(_, fitted)| fitted)
}

fn collapsed_size(edge: DockEdge, session_count: usize, scale: f64, area: WorkArea) -> PhysicalSize<u32> {
    let thickness = pixels(COLLAPSED_THICKNESS, scale);
    let length = pixels((COLLAPSED_SLOT * session_count.max(1) as f64).max(COLLAPSED_THICKNESS), scale);
    let (width, height) = if edge.is_vertical() {
        (thickness, length)
    } else {
        (length, thickness)
    };
    PhysicalSize::new(width.min(area.width.max(1)), height.min(area.height.max(1)))
}

fn expanded_size(session_count: usize, scale: f64, area: WorkArea) -> PhysicalSize<u32> {
    PhysicalSize::new(
        pixels(WIDTH, scale).min(area.width.max(1)),
        pixels(height(session_count), scale).min(area.height.max(1)),
    )
}

fn docked_position(
    edge: DockEdge,
    collapsed: bool,
    position: SavedPosition,
    size: PhysicalSize<u32>,
    area: WorkArea,
    exposed: u32,
) -> SavedPosition {
    let exposed = i64::from(exposed.min(if edge.is_vertical() { size.width } else { size.height }));
    let hidden_width = i64::from(size.width).saturating_sub(exposed);
    let hidden_height = i64::from(size.height).saturating_sub(exposed);
    let fitted = clamp_position(position, size, area);
    match edge {
        DockEdge::Left => SavedPosition {
            x: if collapsed { (i64::from(area.x) - hidden_width) as i32 } else { area.x },
            y: fitted.y,
        },
        DockEdge::Right => SavedPosition {
            x: if collapsed { (area.right() - exposed) as i32 } else { (area.right() - i64::from(size.width)) as i32 },
            y: fitted.y,
        },
        DockEdge::Top => SavedPosition {
            x: fitted.x,
            y: if collapsed { (i64::from(area.y) - hidden_height) as i32 } else { area.y },
        },
        DockEdge::Bottom => SavedPosition {
            x: fitted.x,
            y: if collapsed { (area.bottom() - exposed) as i32 } else { (area.bottom() - i64::from(size.height)) as i32 },
        },
    }
}

fn distance_to_edge(start: i64, end: i64, edge: i64) -> i64 {
    if start <= edge && edge <= end {
        0
    } else {
        (start - edge).abs().min((end - edge).abs())
    }
}

fn nearby_dock_edge(
    position: SavedPosition,
    size: PhysicalSize<u32>,
    areas: &[WorkArea],
    threshold: u32,
) -> Option<(DockEdge, WorkArea)> {
    let left = i64::from(position.x);
    let top = i64::from(position.y);
    let right = left + i64::from(size.width);
    let bottom = top + i64::from(size.height);
    areas.iter().copied().flat_map(|area| {
        let overlaps_x = right > i64::from(area.x) && left < area.right();
        let overlaps_y = bottom > i64::from(area.y) && top < area.bottom();
        [
            (overlaps_y, distance_to_edge(left, right, i64::from(area.x)), DockEdge::Left),
            (overlaps_y, distance_to_edge(left, right, area.right()), DockEdge::Right),
            (overlaps_x, distance_to_edge(top, bottom, i64::from(area.y)), DockEdge::Top),
            (overlaps_x, distance_to_edge(top, bottom, area.bottom()), DockEdge::Bottom),
        ].into_iter().filter_map(move |(overlaps, distance, edge)| {
            overlaps.then_some((distance, edge, area))
        })
    }).filter(|(distance, _, _)| *distance <= i64::from(threshold))
        .min_by_key(|(distance, edge, _)| {
            let order = match edge {
                DockEdge::Left => 0_u8,
                DockEdge::Right => 1,
                DockEdge::Top => 2,
                DockEdge::Bottom => 3,
            };
            (*distance, order)
        }).map(|(_, edge, area)| (edge, area))
}

fn work_areas(window: &WebviewWindow) -> Result<Vec<WorkArea>, String> {
    let mut monitors = window.available_monitors().map_err(|e| e.to_string())?;
    if let Some(primary) = window.primary_monitor().map_err(|e| e.to_string())? {
        if let Some(index) = monitors.iter().position(|monitor| monitor.position() == primary.position()) {
            monitors.swap(0, index);
        }
    }
    Ok(monitors.iter().map(|monitor| {
        let rect = monitor.work_area();
        WorkArea { x: rect.position.x, y: rect.position.y, width: rect.size.width, height: rect.size.height }
    }).collect())
}

fn resize_and_place(app: &AppHandle, restore: bool) -> Result<(), String> {
    let Some(window) = app.get_webview_window(LABEL) else { return Ok(()); };
    let state = app.state::<OverlayState>();
    let snapshot = state.snapshot()?;
    let scale = window.scale_factor().map_err(|e| e.to_string())?;
    let areas = work_areas(&window)?;
    let first = areas.first().copied().ok_or("未找到可用显示器")?;
    let prefs = state.inner.lock().map_err(|e| e.to_string())?.prefs.clone();
    let initial_size = expanded_size(snapshot.sessions.len(), scale, first);
    let position = if restore {
        prefs.position
    } else {
        let pos = window.outer_position().map_err(|e| e.to_string())?;
        Some(SavedPosition { x: pos.x, y: pos.y })
    }.unwrap_or_else(|| default_position(first, initial_size));
    let (area, _) = nearest_area(position, initial_size, &areas).ok_or("未找到可用显示器")?;
    let collapsed = snapshot.collapsed;
    let size = match (prefs.dock, collapsed) {
        (Some(edge), true) => collapsed_size(edge, snapshot.sessions.len(), scale, area),
        _ => expanded_size(snapshot.sessions.len(), scale, area),
    };
    if window.inner_size().map_err(|e| e.to_string())? != size {
        window.set_size(size).map_err(|e| e.to_string())?;
    }
    let target = match prefs.dock {
        Some(edge) => docked_position(edge, collapsed, position, size, area, pixels(EXPOSED_SIZE, scale)),
        None => clamp_position(position, size, area),
    };
    let target = PhysicalPosition::new(target.x, target.y);
    if window.outer_position().map_err(|e| e.to_string())? != target {
        window.set_position(target).map_err(|e| e.to_string())?;
    }
    Ok(())
}

fn should_show(snapshot: &Snapshot) -> bool {
    // 空列表也是有效状态：让用户能确认开关已生效，并看到“暂无 Agent 会话”。
    snapshot.enabled
}

fn save_drag_position(app: &AppHandle) -> Result<Snapshot, String> {
    let Some(window) = app.get_webview_window(LABEL) else {
        return app.state::<OverlayState>().snapshot();
    };
    let state = app.state::<OverlayState>();
    let snapshot = state.snapshot()?;
    if !snapshot.enabled {
        return Ok(snapshot);
    }

    let scale = window.scale_factor().map_err(|e| e.to_string())?;
    let position = window.outer_position().map_err(|e| e.to_string())?;
    let position = SavedPosition { x: position.x, y: position.y };
    let size = window.inner_size().map_err(|e| e.to_string())?;
    let areas = work_areas(&window)?;
    let (dock, collapsed, saved_position) = if let Some((edge, area)) =
        nearby_dock_edge(position, size, &areas, pixels(SNAP_DISTANCE, scale))
    {
        let expanded = expanded_size(snapshot.sessions.len(), scale, area);
        let anchor = docked_position(edge, false, position, expanded, area, pixels(EXPOSED_SIZE, scale));
        (Some(edge), true, anchor)
    } else {
        (None, false, fit_position(Some(position), size, &areas).unwrap_or(position))
    };
    let changed = state.update_preferences_if_changed(|prefs| {
        prefs.position = Some(saved_position);
        prefs.dock = dock;
        prefs.collapsed = collapsed;
    })?;
    resize_and_place(app, true)?;
    if changed { emit_snapshot(app) } else { state.snapshot() }
}

fn expand_dock(app: &AppHandle) -> Result<Snapshot, String> {
    let state = app.state::<OverlayState>();
    let changed = state.update_preferences_if_changed(|prefs| {
        if prefs.dock.is_some() {
            prefs.collapsed = false;
        }
    })?;
    if !changed {
        return state.snapshot();
    }

    resize_and_place(app, true)?;
    emit_snapshot(app)
}

fn collapse_dock(app: &AppHandle) -> Result<Snapshot, String> {
    let state = app.state::<OverlayState>();
    let changed = state.update_preferences_if_changed(|prefs| {
        if prefs.dock.is_some() {
            prefs.collapsed = true;
        }
    })?;
    if !changed {
        return state.snapshot();
    }

    resize_and_place(app, true)?;
    emit_snapshot(app)
}

fn create_window(app: &AppHandle) -> Result<(), String> {
    if app.get_webview_window(LABEL).is_some() { return Ok(()); }
    app.state::<OverlayState>().inner.lock().map_err(|e| e.to_string())?.ready = false;
    let window = tauri::WebviewWindowBuilder::new(app, LABEL, WebviewUrl::App("overlay.html".into()))
        .title("sTerm Agent 状态")
        .inner_size(WIDTH, height(0))
        .transparent(true)
        .background_color(tauri::webview::Color(0, 0, 0, 0))
        .decorations(false)
        .resizable(false)
        .maximizable(false)
        .minimizable(false)
        .always_on_top(true)
        .skip_taskbar(true)
        .shadow(false)
        .focused(false)
        .focusable(false)
        .visible(false)
        .disable_drag_drop_handler()
        .build().map_err(|e| e.to_string())?;
    // 系统关闭（如 Alt+F4）与标题栏叉号同义；隐藏小窗不会销毁主窗口。
    let handle = app.clone();
    window.on_window_event(move |event| {
        if let tauri::WindowEvent::CloseRequested { api, .. } = event {
            api.prevent_close();
            let handle = handle.clone();
            tauri::async_runtime::spawn(async move {
                if let Err(error) = on_ui(handle, |app| set_enabled(app, false)).await {
                    diagnostics::record(&format!("overlay close: {error}"));
                }
            });
        }
    });
    Ok(())
}

// 悬浮窗显示与置顶维护统一使用 Win32，保持位置和尺寸，不抢输入焦点。
mod native {
    use super::{diagnostics, WebviewWindow};
    use std::{ffi::c_void, io};

    type Hwnd = *mut c_void;
    const GWL_EXSTYLE: i32 = -20;
    const WS_EX_TOPMOST: isize = 0x0000_0008;
    const WS_EX_TOOLWINDOW: isize = 0x0000_0080;
    const WS_EX_APPWINDOW: isize = 0x0004_0000;
    const WS_EX_NOACTIVATE: isize = 0x0800_0000;
    const SWP_NOSIZE: u32 = 0x0001;
    const SWP_NOMOVE: u32 = 0x0002;
    const SWP_NOACTIVATE: u32 = 0x0010;
    const SWP_SHOWWINDOW: u32 = 0x0040;
    const TOPMOST_TIMER: usize = 0x5354_4f56;
    const CHECK_INTERVAL_MS: u32 = 2000;

    #[repr(C)]
    #[derive(Default)]
    struct Rect { left: i32, top: i32, right: i32, bottom: i32 }

    #[link(name = "user32")]
    extern "system" {
        fn ShowWindow(hwnd: Hwnd, command: i32) -> i32;
        fn GetWindowLongPtrW(hwnd: Hwnd, index: i32) -> isize;
        fn SetWindowLongPtrW(hwnd: Hwnd, index: i32, value: isize) -> isize;
        fn SetWindowPos(hwnd: Hwnd, after: Hwnd, x: i32, y: i32, width: i32, height: i32, flags: u32) -> i32;
        fn GetWindow(hwnd: Hwnd, command: u32) -> Hwnd;
        fn GetWindowRect(hwnd: Hwnd, rect: *mut Rect) -> i32;
        fn IsWindowVisible(hwnd: Hwnd) -> i32;
        fn IsIconic(hwnd: Hwnd) -> i32;
        fn SetTimer(hwnd: Hwnd, id: usize, millis: u32, callback: Option<unsafe extern "system" fn(Hwnd, u32, usize, u32)>) -> usize;
        fn KillTimer(hwnd: Hwnd, id: usize) -> i32;
    }

    #[link(name = "kernel32")]
    extern "system" {
        fn SetLastError(error: u32);
        fn GetLastError() -> u32;
    }

    #[link(name = "dwmapi")]
    extern "system" {
        fn DwmGetWindowAttribute(hwnd: Hwnd, attribute: u32, value: *mut u32, size: u32) -> i32;
    }

    fn win32_error(action: &str) -> String {
        format!("{action}: {}", io::Error::last_os_error())
    }

    fn place_on_top(hwnd: Hwnd, show: bool) -> Result<(), String> {
        let flags = SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | if show { SWP_SHOWWINDOW } else { 0 };
        // 不能只改 WS_EX_TOPMOST：必须通过 HWND_TOPMOST 重建实际 Z 顺序。
        if unsafe { SetWindowPos(hwnd, -1isize as Hwnd, 0, 0, 0, 0, flags) } == 0 {
            return Err(win32_error("overlay SetWindowPos(HWND_TOPMOST)"));
        }
        Ok(())
    }

    fn needs_topmost(hwnd: Hwnd) -> Option<&'static str> {
        unsafe {
            if IsWindowVisible(hwnd) == 0 || IsIconic(hwnd) != 0 { return None; }
            if GetWindowLongPtrW(hwnd, GWL_EXSTYLE) & WS_EX_TOPMOST == 0 {
                return Some("topmost-flag-missing");
            }
            // 长时间运行后可能仍有置顶标记，但已落在普通窗口后面。
            // 忽略隐藏、最小化、其他虚拟桌面和没有面积的窗口。
            let mut above = GetWindow(hwnd, 3); // GW_HWNDPREV
            for _ in 0..4096 {
                if above.is_null() { break; }
                if IsWindowVisible(above) != 0 && IsIconic(above) == 0
                    && GetWindowLongPtrW(above, GWL_EXSTYLE) & WS_EX_TOPMOST == 0 {
                    let mut rect = Rect::default();
                    let mut cloaked = 0;
                    let _ = DwmGetWindowAttribute(above, 14, &mut cloaked, 4); // DWMWA_CLOAKED
                    if cloaked == 0 && GetWindowRect(above, &mut rect) != 0
                        && rect.right > rect.left && rect.bottom > rect.top {
                        return Some("normal-window-above");
                    }
                }
                above = GetWindow(above, 3);
            }
        }
        None
    }

    unsafe extern "system" fn maintain_topmost(hwnd: Hwnd, _: u32, _: usize, _: u32) {
        let Some(reason) = needs_topmost(hwnd) else { return; };
        // 不使用 SHOWWINDOW：即使有已排队的定时消息，也不会重新打开关闭的小窗。
        match place_on_top(hwnd, false) {
            Ok(()) => diagnostics::record(&format!("overlay topmost recovered hwnd={hwnd:p} reason={reason}")),
            Err(error) => diagnostics::record(&error),
        }
    }

    fn set_visible(hwnd: Hwnd, show: bool) -> Result<(), String> {
        unsafe {
            if !show {
                KillTimer(hwnd, TOPMOST_TIMER);
                ShowWindow(hwnd, 0); // SW_HIDE
                diagnostics::record(&format!("overlay hidden hwnd={hwnd:p}"));
                return Ok(());
            }
            let style = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
            let target = (style | WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW) & !WS_EX_APPWINDOW;
            if style != target {
                SetLastError(0);
                if SetWindowLongPtrW(hwnd, GWL_EXSTYLE, target) == 0 && GetLastError() != 0 {
                    return Err(win32_error("overlay SetWindowLongPtrW"));
                }
            }
            // 窗口定时器由原生 UI 消息循环驱动，不依赖 WebView 的后台计时器。
            // 隐藏时停止，销毁窗口时由 Windows 自动回收，不创建常驻后台线程。
            if SetTimer(hwnd, TOPMOST_TIMER, CHECK_INTERVAL_MS, Some(maintain_topmost)) == 0 {
                return Err(win32_error("overlay SetTimer"));
            }
            if let Err(error) = place_on_top(hwnd, true) {
                KillTimer(hwnd, TOPMOST_TIMER);
                return Err(error);
            }
        }
        diagnostics::record(&format!("overlay shown hwnd={hwnd:p} topmost-check-ms={CHECK_INTERVAL_MS}"));
        Ok(())
    }

    pub fn visible(window: &WebviewWindow, show: bool) -> Result<(), String> {
        set_visible(window.hwnd().map_err(|e| e.to_string())?.0, show)
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use std::{ptr, thread, time::{Duration, Instant}};

        #[repr(C)]
        #[derive(Default)]
        struct Point { x: i32, y: i32 }

        #[repr(C)]
        #[derive(Default)]
        struct Message {
            hwnd: Hwnd,
            message: u32,
            wparam: usize,
            lparam: isize,
            time: u32,
            point: Point,
            private: u32,
        }

        #[link(name = "user32")]
        extern "system" {
            fn CreateWindowExW(ex: u32, class: *const u16, title: *const u16, style: u32, x: i32, y: i32, width: i32, height: i32, parent: Hwnd, menu: Hwnd, instance: Hwnd, param: Hwnd) -> Hwnd;
            fn DestroyWindow(hwnd: Hwnd) -> i32;
            fn GetForegroundWindow() -> Hwnd;
            fn PeekMessageW(message: *mut Message, hwnd: Hwnd, first: u32, last: u32, remove: u32) -> i32;
            fn DispatchMessageW(message: *const Message) -> isize;
        }

        struct TestWindow(Hwnd);
        impl TestWindow {
            fn new() -> Self {
                let class: Vec<_> = "STATIC\0".encode_utf16().collect();
                // 屏幕外的无焦点测试窗口，不干扰正在使用的应用。
                let hwnd = unsafe { CreateWindowExW(
                    (WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW) as u32, class.as_ptr(), ptr::null(),
                    0x8000_0000, -30000, -30000, 264, 100,
                    ptr::null_mut(), ptr::null_mut(), ptr::null_mut(), ptr::null_mut(),
                ) };
                assert!(!hwnd.is_null(), "{}", win32_error("CreateWindowExW"));
                Self(hwnd)
            }
        }
        impl Drop for TestWindow {
            fn drop(&mut self) { unsafe { DestroyWindow(self.0); } }
        }

        fn rect(hwnd: Hwnd) -> [i32; 4] {
            let mut rect = Rect::default();
            assert_ne!(unsafe { GetWindowRect(hwnd, &mut rect) }, 0);
            [rect.left, rect.top, rect.right, rect.bottom]
        }

        #[test]
        fn showing_and_timer_recover_topmost_without_changing_focus_or_geometry() {
            let normal = TestWindow::new();
            let overlay = TestWindow::new();
            let foreground = unsafe { GetForegroundWindow() };
            let original = rect(overlay.0);
            let flags = SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE;
            assert_ne!(unsafe { SetWindowPos(normal.0, ptr::null_mut(), 0, 0, 0, 0, flags | SWP_SHOWWINDOW) }, 0);
            set_visible(overlay.0, true).unwrap();
            assert!(needs_topmost(overlay.0).is_none());
            assert_eq!(rect(overlay.0), original);
            assert_eq!(unsafe { GetForegroundWindow() }, foreground);

            // 模拟长时间运行后被降为普通窗口；实际派发 WM_TIMER 验证自愈路径。
            assert_ne!(unsafe { SetWindowPos(overlay.0, -2isize as Hwnd, 0, 0, 0, 0, flags) }, 0);
            assert!(needs_topmost(overlay.0).is_some());
            let deadline = Instant::now() + Duration::from_secs(4);
            while needs_topmost(overlay.0).is_some() && Instant::now() < deadline {
                let mut message = Message::default();
                while unsafe { PeekMessageW(&mut message, ptr::null_mut(), 0, 0, 1) } != 0 {
                    unsafe { DispatchMessageW(&message); }
                }
                thread::sleep(Duration::from_millis(10));
            }
            assert!(needs_topmost(overlay.0).is_none(), "原生定时器未恢复置顶");
            assert_eq!(rect(overlay.0), original);
            assert_eq!(unsafe { GetForegroundWindow() }, foreground);

            set_visible(overlay.0, false).unwrap();
            // 模拟关闭前排队的定时消息，不能让已关闭的小窗重新出现。
            unsafe { maintain_topmost(overlay.0, 0, TOPMOST_TIMER, 0); }
            assert_eq!(unsafe { IsWindowVisible(overlay.0) }, 0);
            set_visible(overlay.0, true).unwrap();
            assert!(needs_topmost(overlay.0).is_none());
            assert_ne!(unsafe { IsWindowVisible(overlay.0) }, 0);
            assert_eq!(unsafe { GetForegroundWindow() }, foreground);
        }
    }
}
fn set_enabled(app: &AppHandle, enabled: bool) -> Result<Snapshot, String> {
    if enabled { create_window(app)?; }
    let state = app.state::<OverlayState>();
    let previous_prefs = state.inner.lock().map_err(|e| e.to_string())?.prefs.clone();
    let previous = previous_prefs.enabled;
    let position = if !enabled && previous && previous_prefs.dock.is_none() {
        app.get_webview_window(LABEL).and_then(|window| window.outer_position().ok())
            .map(|pos| SavedPosition { x: pos.x, y: pos.y })
    } else { None };
    state.update_preferences(|prefs| {
        prefs.enabled = enabled;
        if let Some(position) = position { prefs.position = Some(position); }
    })?;
    let result = (|| -> Result<(), String> {
        if let Some(window) = app.get_webview_window(LABEL) {
            if enabled {
                resize_and_place(app, true)?;
                if state.inner.lock().map_err(|e| e.to_string())?.ready {
                    native::visible(&window, should_show(&state.snapshot()?))?;
                }
            } else {
                native::visible(&window, false)?;
            }
        }
        Ok(())
    })();
    if let Err(error) = result {
        let _ = state.update_preferences(|prefs| *prefs = previous_prefs);
        let _ = emit_snapshot(app);
        return Err(error);
    }
    emit_snapshot(app)
}

#[tauri::command]
pub async fn overlay_init(app: AppHandle, window: WebviewWindow) -> Result<Snapshot, String> {
    require_window(&window, &["main"])?;
    on_ui(app, |app| {
        if app.state::<OverlayState>().snapshot()?.enabled {
            create_window(app)?;
            resize_and_place(app, true)?;
        }
        emit_snapshot(app)
    }).await
}

#[tauri::command]
pub fn overlay_snapshot(state: tauri::State<OverlayState>) -> Result<Snapshot, String> {
    state.snapshot()
}

#[tauri::command]
pub async fn overlay_set_enabled(app: AppHandle, window: WebviewWindow, enabled: bool) -> Result<Snapshot, String> {
    require_window(&window, &["main", LABEL])?;
    on_ui(app, move |app| set_enabled(app, enabled)).await
}

#[tauri::command]
pub async fn overlay_set_opacity(app: AppHandle, window: WebviewWindow, opacity: u8) -> Result<Snapshot, String> {
    require_window(&window, &["main"])?;
    if opacity > 100 { return Err("底色不透明度必须在 0 到 100 之间".into()); }
    on_ui(app, move |app| {
        app.state::<OverlayState>().update_preferences(|prefs| prefs.background_opacity = opacity)?;
        emit_snapshot(app)
    }).await
}

#[tauri::command]
pub async fn overlay_publish(app: AppHandle, window: WebviewWindow, sessions: Vec<Session>) -> Result<(), String> {
    require_window(&window, &["main"])?;
    on_ui(app, move |app| {
        let state = app.state::<OverlayState>();
        {
            let mut inner = state.inner.lock().map_err(|e| e.to_string())?;
            if inner.sessions == sessions { return Ok(()); }
            inner.sessions = sessions;
            inner.revision += 1;
        }
        let snapshot = state.snapshot()?;
        if snapshot.enabled {
            resize_and_place(app, snapshot.dock.is_some())?;
            if state.inner.lock().map_err(|e| e.to_string())?.ready {
                let window = app.get_webview_window(LABEL).ok_or("悬浮窗已关闭")?;
                native::visible(&window, should_show(&snapshot))?;
            }
        }
        emit_snapshot(app)?;
        Ok(())
    }).await
}

#[tauri::command]
pub async fn overlay_ready(app: AppHandle, window: WebviewWindow) -> Result<Snapshot, String> {
    require_window(&window, &[LABEL])?;
    on_ui(app, |app| {
        let state = app.state::<OverlayState>();
        state.inner.lock().map_err(|e| e.to_string())?.ready = true;
        resize_and_place(app, true)?;
        let snapshot = state.snapshot()?;
        if snapshot.enabled {
            native::visible(&app.get_webview_window(LABEL).ok_or("悬浮窗已关闭")?, should_show(&snapshot))?;
        }
        Ok(snapshot)
    }).await
}

#[tauri::command]
pub async fn overlay_save_position(app: AppHandle, window: WebviewWindow) -> Result<Snapshot, String> {
    require_window(&window, &[LABEL])?;
    on_ui(app, |app| save_drag_position(app)).await
}

#[tauri::command]
pub async fn overlay_expand(app: AppHandle, window: WebviewWindow) -> Result<Snapshot, String> {
    require_window(&window, &[LABEL])?;
    on_ui(app, |app| expand_dock(app)).await
}

#[tauri::command]
pub async fn overlay_collapse(app: AppHandle, window: WebviewWindow) -> Result<Snapshot, String> {
    require_window(&window, &[LABEL])?;
    on_ui(app, |app| collapse_dock(app)).await
}

#[tauri::command]
pub async fn overlay_activate(app: AppHandle, window: WebviewWindow, id: u32) -> Result<(), String> {
    require_window(&window, &[LABEL])?;
    on_ui(app, move |app| {
        if !app.state::<OverlayState>().snapshot()?.sessions.iter().any(|s| s.id == id) {
            return Err("该会话已关闭".into());
        }
        let main = app.get_webview_window("main").ok_or("主窗口已关闭")?;
        main.unminimize().map_err(|e| e.to_string())?;
        main.show().map_err(|e| e.to_string())?;
        main.set_focus().map_err(|e| e.to_string())?;
        app.emit_to("main", "overlay-activate", id).map_err(|e| e.to_string())
    }).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::tests::TempDir;

    #[test]
    fn positions_survive_negative_monitor_coordinates_and_removed_screens() {
        let size = PhysicalSize::new(296, 380);
        let areas = [
            WorkArea { x: 0, y: 0, width: 1920, height: 1040 },
            WorkArea { x: -1920, y: -200, width: 1920, height: 1080 },
        ];
        let position = SavedPosition { x: -1800, y: -100 };
        assert_eq!(fit_position(Some(position), size, &areas), Some(position));
        assert_eq!(fit_position(Some(position), size, &areas[..1]), Some(SavedPosition { x: 0, y: 0 }));
        assert_eq!(fit_position(None, size, &areas), Some(SavedPosition { x: 1604, y: 60 }));
        assert_eq!(fit_position(None, size, &[]), None);
    }

    #[test]
    fn growing_list_stays_above_taskbar_and_height_is_capped() {
        let areas = [WorkArea { x: 0, y: 0, width: 1280, height: 720 }];
        assert_eq!(fit_position(Some(SavedPosition { x: 1200, y: 700 }), PhysicalSize::new(296, 380), &areas),
            Some(SavedPosition { x: 984, y: 340 }));
        assert_eq!(height(0), 78.0);
        assert_eq!(height(1), 78.0);
        assert_eq!(height(100), 238.0);
    }

    #[test]
    fn enabled_overlay_stays_visible_without_agent_sessions() {
        let mut snapshot = Snapshot {
            enabled: true,
            background_opacity: 80,
            dock: None,
            collapsed: false,
            revision: 0,
            sessions: Vec::new(),
        };
        assert!(should_show(&snapshot));
        snapshot.enabled = false;
        assert!(!should_show(&snapshot));

        let prefs = Preferences {
            enabled: true,
            position: None,
            background_opacity: 80,
            dock: Some(DockEdge::Right),
            collapsed: true,
        };
        assert!(uses_collapsed_layout(&prefs));
        let expanded = Preferences { collapsed: false, ..prefs };
        assert!(!uses_collapsed_layout(&expanded));
    }

    #[test]
    fn edge_docking_exposes_only_the_status_strip_on_all_four_sides() {
        let area = WorkArea { x: -1920, y: -100, width: 1920, height: 1080 };
        let expanded = PhysicalSize::new(264, 110);
        let cases = [
            (SavedPosition { x: -1918, y: 220 }, DockEdge::Left),
            (SavedPosition { x: -266, y: 220 }, DockEdge::Right),
            (SavedPosition { x: -800, y: -98 }, DockEdge::Top),
            (SavedPosition { x: -800, y: 868 }, DockEdge::Bottom),
        ];

        for (position, edge) in cases {
            assert_eq!(nearby_dock_edge(position, expanded, &[area], 24), Some((edge, area)));
            let anchor = docked_position(edge, false, position, expanded, area, 16);
            let compact = collapsed_size(edge, 2, 1.0, area);
            let position = docked_position(edge, true, anchor, compact, area, 16);
            match edge {
                DockEdge::Left => assert_eq!(i64::from(position.x) + i64::from(compact.width), i64::from(area.x) + 16),
                DockEdge::Right => assert_eq!(position.x, (area.right() - 16) as i32),
                DockEdge::Top => assert_eq!(i64::from(position.y) + i64::from(compact.height), i64::from(area.y) + 16),
                DockEdge::Bottom => assert_eq!(position.y, (area.bottom() - 16) as i32),
            }
        }
        assert_eq!(collapsed_size(DockEdge::Left, 1, 1.0, area), PhysicalSize::new(28, 28));
        assert_eq!(collapsed_size(DockEdge::Right, 0, 1.0, area), PhysicalSize::new(28, 28));
        assert_eq!(collapsed_size(DockEdge::Top, 3, 1.0, area), PhysicalSize::new(66, 28));
        assert_eq!(pixels(EXPOSED_SIZE, 1.5), 24);
    }

    #[test]
    fn docking_uses_one_nearest_edge_and_ignores_positions_away_from_edges() {
        let area = WorkArea { x: 0, y: 0, width: 1000, height: 800 };
        let size = PhysicalSize::new(264, 110);
        // 角落不引入独立模式；等距时用稳定顺序选择一条边。
        assert_eq!(nearby_dock_edge(SavedPosition { x: 2, y: 2 }, size, &[area], 24),
            Some((DockEdge::Left, area)));
        // 拖动时允许短暂越过边界，松开后仍应回吸到边缘。
        assert_eq!(nearby_dock_edge(SavedPosition { x: -100, y: 300 }, size, &[area], 24),
            Some((DockEdge::Left, area)));
        assert_eq!(nearby_dock_edge(SavedPosition { x: 300, y: 300 }, size, &[area], 24), None);
    }

    #[test]
    fn preferences_default_off_and_independent_edits_keep_position() {
        let dir = TempDir::new();
        let file = dir.path().join("overlay.json");
        assert!(!Preferences::default().enabled);
        save_preferences(&file, |p| p.enabled = true).unwrap();
        let position = Some(SavedPosition { x: -1500, y: 300 });
        save_preferences(&file, |p| p.position = position).unwrap();
        save_preferences(&file, |p| {
            p.dock = Some(DockEdge::Left);
            p.collapsed = true;
        }).unwrap();
        save_preferences(&file, |p| p.background_opacity = 35).unwrap();
        let prefs = save_preferences(&file, |p| p.enabled = false).unwrap();
        assert_eq!(prefs.background_opacity, 35);
        assert!(!prefs.enabled);
        assert_eq!(prefs.position, position);
        assert_eq!(prefs.dock, Some(DockEdge::Left));
        assert!(prefs.collapsed);
        assert_eq!(storage::read_json::<Preferences>(&file).unwrap(), Some(prefs));
    }

    #[test]
    fn unchanged_dock_preferences_do_not_create_a_backup() {
        let dir = TempDir::new();
        let file = dir.path().join("overlay.json");
        let saved = save_preferences(&file, |p| {
            p.enabled = true;
            p.dock = Some(DockEdge::Right);
        }).unwrap();
        let (unchanged, wrote) = save_preferences_if_changed(&file, |p| {
            p.enabled = true;
            p.dock = Some(DockEdge::Right);
        }).unwrap();
        assert!(!wrote);
        assert_eq!(unchanged, saved);
        assert!(!file.with_file_name("overlay.json.bak").exists());
    }

    #[test]
    fn legacy_preferences_gain_opacity_without_losing_window_settings() {
        let dir = TempDir::new();
        let file = dir.path().join("overlay.json");
        std::fs::write(&file, br#"{"enabled":true,"position":{"x":-1500,"y":300}}"#).unwrap();
        let legacy = storage::read_json::<Preferences>(&file).unwrap().unwrap();
        assert_eq!(legacy.background_opacity, 80);
        assert_eq!(legacy.dock, None);
        assert!(!legacy.collapsed);
        let updated = save_preferences(&file, |p| p.background_opacity = 0).unwrap();
        assert!(updated.enabled);
        assert_eq!(updated.position, legacy.position);
        assert_eq!(updated.background_opacity, 0);
        let updated = save_preferences(&file, |p| p.background_opacity = 100).unwrap();
        assert_eq!(storage::read_json::<Preferences>(&file).unwrap(), Some(updated));
    }

    #[test]
    fn malformed_preferences_are_preserved() {
        let dir = TempDir::new();
        let file = dir.path().join("overlay.json");
        let original = "{broken";
        std::fs::write(&file, original).unwrap();
        assert!(save_preferences(&file, |p| p.enabled = true).is_err());
        assert_eq!(std::fs::read_to_string(file).unwrap(), original);
    }
}
