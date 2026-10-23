//! One-time startup repair for cascaded/restored windows outside the usable desktop.
//! All calculations include native decorations and use physical pixels, including
//! negative monitor origins. Normal in-bounds placement remains untouched.

#[cfg(any(windows, test))]
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct PhysicalRect {
    left: i32,
    top: i32,
    right: i32,
    bottom: i32,
}

#[cfg(any(windows, test))]
#[derive(Debug, PartialEq, Eq)]
struct Placement {
    x: i32,
    y: i32,
    inner_width: i32,
    inner_height: i32,
}

#[cfg(any(windows, test))]
fn fit(outer: PhysicalRect, inner: [i32; 2], work: PhysicalRect) -> Result<Option<Placement>, &'static str> {
    let extent = |min: i32, max: i32| -> Result<i32, &'static str> {
        let value = i64::from(max) - i64::from(min);
        if value <= 0 || value > i64::from(i32::MAX) {
            Err("Invalid desktop geometry")
        } else {
            Ok(value as i32)
        }
    };
    let (width, height) = (extent(outer.left, outer.right)?, extent(outer.top, outer.bottom)?);
    let (work_width, work_height) = (extent(work.left, work.right)?, extent(work.top, work.bottom)?);
    if inner[0] <= 0 || inner[1] <= 0 || inner[0] > width || inner[1] > height {
        return Err("Invalid window decoration geometry");
    }
    let (next_width, next_height) = (width.min(work_width), height.min(work_height));
    let x = outer.left.clamp(work.left, work.right - next_width);
    let y = outer.top.clamp(work.top, work.bottom - next_height);
    if x == outer.left && y == outer.top && next_width == width && next_height == height {
        return Ok(None);
    }
    let inner_width = inner[0] - (width - next_width);
    let inner_height = inner[1] - (height - next_height);
    if inner_width <= 0 || inner_height <= 0 {
        return Err("Monitor work area is smaller than native decorations");
    }
    Ok(Some(Placement { x, y, inner_width, inner_height }))
}

/// False means the first native geometry has not arrived yet. Otherwise the
/// caller retires this repair; moving/resizing the window later is user-owned.
pub fn repair_startup(ctx: &egui::Context) -> bool {
    #[cfg(windows)]
    {
        let (outer, inner, managed) = ctx.input(|input| {
            let viewport = input.viewport();
            (
                viewport.outer_rect,
                viewport.inner_rect,
                viewport.minimized == Some(true) || viewport.maximized == Some(true) || viewport.fullscreen == Some(true),
            )
        });
        if managed {
            return true;
        }
        let (Some(outer), Some(inner)) = (outer, inner) else {
            return false;
        };
        let scale = ctx.pixels_per_point();
        let values = [outer.min.x * scale, outer.min.y * scale, outer.max.x * scale, outer.max.y * scale, inner.width() * scale, inner.height() * scale];
        if !scale.is_finite() || scale <= 0. || values.iter().any(|value| !value.is_finite() || *value >= i32::MAX as f32 || *value <= i32::MIN as f32) {
            eprintln!("desigma: initial window geometry unavailable");
            return true;
        }
        let outer =
            PhysicalRect { left: values[0].round() as i32, top: values[1].round() as i32, right: values[2].round() as i32, bottom: values[3].round() as i32 };
        #[repr(C)]
        struct MonitorInfo {
            size: u32,
            monitor: PhysicalRect,
            work: PhysicalRect,
            flags: u32,
        }
        #[link(name = "user32")]
        unsafe extern "system" {
            fn MonitorFromRect(rect: *const PhysicalRect, flags: u32) -> *mut std::ffi::c_void;
            fn GetMonitorInfoW(monitor: *mut std::ffi::c_void, info: *mut MonitorInfo) -> i32;
        }
        let mut info =
            MonitorInfo { size: std::mem::size_of::<MonitorInfo>() as u32, monitor: PhysicalRect::default(), work: PhysicalRect::default(), flags: 0 };
        // SAFETY: Win32 reads one valid RECT and writes the correctly sized
        // MONITORINFO on this UI thread; no external application is controlled.
        let available = unsafe {
            let monitor = MonitorFromRect(&outer, 2 /* MONITOR_DEFAULTTONEAREST */);
            !monitor.is_null() && GetMonitorInfoW(monitor, &mut info) != 0
        };
        if !available {
            eprintln!("desigma: monitor work area unavailable; initial placement retained");
            return true;
        }
        match fit(outer, [values[4].round() as i32, values[5].round() as i32], info.work) {
            Ok(Some(next)) => {
                let size = egui::vec2(next.inner_width as f32 / scale, next.inner_height as f32 / scale);
                // Small displays must be allowed to fit below the normal design
                // minimum. This adjusts only this window's native constraints.
                ctx.send_viewport_cmd(egui::ViewportCommand::MinInnerSize(egui::vec2(
                    crate::theme::MIN_WINDOW.0.min(size.x),
                    crate::theme::MIN_WINDOW.1.min(size.y),
                )));
                ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(size));
                ctx.send_viewport_cmd(egui::ViewportCommand::OuterPosition(egui::pos2(next.x as f32 / scale, next.y as f32 / scale)));
            }
            Ok(None) => {}
            Err(error) => eprintln!("desigma: {error}; initial placement retained"),
        }
        true
    }
    #[cfg(not(windows))]
    {
        let _ = ctx;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn rect(left: i32, top: i32, right: i32, bottom: i32) -> PhysicalRect {
        PhysicalRect { left, top, right, bottom }
    }
    #[test]
    fn cascaded_window_fits_work_area_including_title_bar_and_taskbar_at_every_dpi() {
        for percent in [100, 125, 150, 200] {
            let p = |value: i32| value * percent / 100;
            let outer = rect(p(200), p(80), p(1816), p(1206));
            let work = rect(0, 0, p(1707), p(1019));
            let result = fit(outer, [p(1600), p(1088)], work).unwrap().unwrap();
            assert_eq!(result, Placement { x: p(91), y: 0, inner_width: p(1600), inner_height: p(1088) - ((p(1206) - p(80)) - p(1019)) });
            assert_eq!(result.y + result.inner_height + (p(1206) - p(80) - p(1088)), work.bottom);
        }
    }
    #[test]
    fn visible_size_position_and_user_managed_windows_need_no_repair() {
        assert_eq!(fit(rect(100, 90, 1100, 790), [984, 662], rect(0, 0, 1707, 1019)).unwrap(), None);
    }
    #[test]
    fn negative_monitor_origins_and_top_or_left_taskbars_are_preserved() {
        let result = fit(rect(-2200, -100, -600, 1000), [1584, 1062], rect(-1900, 40, -20, 1040)).unwrap().unwrap();
        assert_eq!(result, Placement { x: -1900, y: 40, inner_width: 1584, inner_height: 962 });
    }
    #[test]
    fn invalid_or_overflowing_geometry_cannot_emit_a_resize() {
        let work = rect(0, 0, 1920, 1040);
        assert!(fit(rect(0, 0, 0, 10), [1, 1], work).is_err());
        assert!(fit(rect(i32::MIN, 0, i32::MAX, 20), [10, 10], work).is_err());
        assert!(fit(rect(0, 0, 100, 100), [101, 50], work).is_err());
        assert!(fit(rect(0, 0, 100, 100), [80, 50], rect(0, 0, 1, 1)).is_err());
    }
}
