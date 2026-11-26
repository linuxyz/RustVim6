//! Direct2D/DirectWrite window and input loop.
//!
//! The editor state and layout come from rvim-app's Tui; this module only owns
//! the native window, maps Win32 input to Key values, and paints the Grid with
//! Direct2D/DirectWrite.

use std::collections::HashMap;
use std::mem::zeroed;
use std::ptr::{null, null_mut};
use std::sync::atomic::{AtomicIsize, Ordering};
use std::time::{Duration, Instant};
use winapi::ctypes::c_void;

use crate::scrollbar::{self, Rect as ScrollRect, VerticalScrollbar};
use rvim_app::Session;
use rvim_app::tui::Tui;
use rvim_core::Pos;
use rvim_editor::maps::Key;
use rvim_editor::{Mode, RegType, Register, WindowId};
use rvim_normal::operators::{OpRange, yank_range};
use rvim_syntax::highlight::HlColor;
use rvim_term::{Color, Style};

use winapi::shared::basetsd::LONG_PTR;
use winapi::shared::dxgiformat::DXGI_FORMAT_B8G8R8A8_UNORM;
use winapi::shared::guiddef::GUID;
use winapi::shared::minwindef::{BOOL, DWORD, HGLOBAL, LPARAM, LRESULT, UINT, WPARAM};
use winapi::shared::ntdef::HANDLE;
use winapi::shared::windef::{HBRUSH, HWND, POINT, RECT};
use winapi::shared::winerror::D2DERR_RECREATE_TARGET;
use winapi::um::combaseapi::{CoInitializeEx, CoUninitialize};
use winapi::um::commdlg::{
    CF_EFFECTS, CF_INITTOLOGFONTSTRUCT, CF_SCREENFONTS, CHOOSEFONTW, ChooseFontW,
};
use winapi::um::d2d1::{
    D2D1_COLOR_F, D2D1_DRAW_TEXT_OPTIONS_ENABLE_COLOR_FONT, D2D1_FACTORY_TYPE_SINGLE_THREADED,
    D2D1_FEATURE_LEVEL_DEFAULT, D2D1_HWND_RENDER_TARGET_PROPERTIES, D2D1_POINT_2F,
    D2D1_PRESENT_OPTIONS_NONE, D2D1_RECT_F, D2D1_RENDER_TARGET_PROPERTIES,
    D2D1_RENDER_TARGET_TYPE_DEFAULT, D2D1_RENDER_TARGET_USAGE_NONE, D2D1_ROUNDED_RECT, D2D1_SIZE_U,
    D2D1_TEXT_ANTIALIAS_MODE_CLEARTYPE, D2D1CreateFactory, ID2D1Brush, ID2D1Factory,
    ID2D1HwndRenderTarget, ID2D1SolidColorBrush,
};
use winapi::um::dcommon::{D2D1_ALPHA_MODE_IGNORE, D2D1_MATRIX_3X2_F, D2D1_PIXEL_FORMAT};
use winapi::um::dwrite::{
    DWRITE_FACTORY_TYPE_SHARED, DWRITE_FONT_STRETCH_NORMAL, DWRITE_FONT_STYLE_ITALIC,
    DWRITE_FONT_STYLE_NORMAL, DWRITE_FONT_WEIGHT_BOLD, DWRITE_FONT_WEIGHT_NORMAL,
    DWRITE_PARAGRAPH_ALIGNMENT_NEAR, DWRITE_TEXT_ALIGNMENT_LEADING, DWRITE_TEXT_METRICS,
    DWRITE_WORD_WRAPPING_NO_WRAP, DWriteCreateFactory, IDWriteFactory, IDWriteFont,
    IDWriteFontCollection, IDWriteFontFace, IDWriteFontFamily, IDWriteGdiInterop,
    IDWriteLocalizedStrings, IDWriteTextFormat, IDWriteTextLayout,
};
use winapi::um::imm::{
    CFS_CANDIDATEPOS, CFS_POINT, COMPOSITIONFORM, HIMC, ImmGetContext, ImmReleaseContext,
    ImmSetCompositionWindow,
};
use winapi::um::libloaderapi::GetModuleHandleW;
use winapi::um::objbase::COINIT_APARTMENTTHREADED;
use winapi::um::unknwnbase::IUnknown;
use winapi::um::winbase::{GMEM_MOVEABLE, GlobalAlloc, GlobalFree, GlobalLock, GlobalUnlock};
use winapi::um::wingdi::LOGFONTW;
use winapi::um::winuser::*;

static DIALOG_OWNER: AtomicIsize = AtomicIsize::new(0);

const GCS_RESULTSTR: DWORD = 0x0800;
// GCS_RESULTREADSTR, GCS_RESULTREADCLAUSE, GCS_RESULTSTR, GCS_RESULTCLAUSE.
const GCS_RESULT_FLAGS: LPARAM = 0x1e00;
#[repr(C)]
struct CandidateForm {
    dw_index: DWORD,
    dw_style: DWORD,
    pt_current_pos: POINT,
    rc_area: RECT,
}

#[link(name = "imm32")]
unsafe extern "system" {
    fn ImmGetCompositionStringW(himc: HIMC, index: DWORD, buf: *mut c_void, len: DWORD) -> i32;
    fn ImmSetCompositionFontW(himc: HIMC, lf: *const LOGFONTW) -> BOOL;
    fn ImmSetCandidateWindow(himc: HIMC, form: *const CandidateForm) -> BOOL;
}

const IID_ID2D1_FACTORY: GUID = GUID {
    Data1: 0x0615_2247,
    Data2: 0x6f50,
    Data3: 0x465a,
    Data4: [0x92, 0x45, 0x11, 0x8b, 0xfd, 0x3b, 0x60, 0x07],
};

const IID_IDWRITE_FACTORY: GUID = GUID {
    Data1: 0xb859_ee5a,
    Data2: 0xd838,
    Data3: 0x4b5b,
    Data4: [0xa2, 0xe8, 0x1a, 0xdc, 0x7d, 0x93, 0xdb, 0x48],
};

const DEFAULT_FG: (u8, u8, u8) = (212, 212, 212);
const DEFAULT_BG: (u8, u8, u8) = (30, 30, 30);
const BASE_FONT_POINTS: f32 = 12.0;
const DEFAULT_FONT: &str = "Consolas";

fn log(message: &str) {
    if let Ok(path) = std::env::var("RVIM_GUI_LOG") {
        use std::io::Write;
        if let Ok(mut file) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
        {
            let _ = writeln!(file, "{message}");
        }
    }
}

#[derive(Clone, Copy)]
struct PaneScrollbar {
    window: WindowId,
    geometry: VerticalScrollbar,
}

#[derive(Clone, Copy)]
struct ScrollDrag {
    window: WindowId,
    grab_offset: f32,
}

struct Gui {
    hwnd: HWND,
    tui: Tui,
    d2d: *mut ID2D1Factory,
    dwrite: *mut IDWriteFactory,
    rt: *mut ID2D1HwndRenderTarget,
    formats: Vec<*mut IDWriteTextFormat>,
    fallback_formats: Vec<*mut IDWriteTextFormat>,
    primary_face: *mut IDWriteFontFace,
    fallback_face: *mut IDWriteFontFace,
    glyph_cache: HashMap<(u8, char), bool>,
    font_spec: String,
    wide_spec: String,
    brushes: HashMap<u32, *mut ID2D1SolidColorBrush>,
    scale: f32,
    cell_w: f32,
    cell_h: f32,
    cols: usize,
    rows: usize,
    client_w: i32,
    client_h: i32,
    focused: bool,
    cursor_px: (f32, f32),
    last_clipboard_seq: u32,
    last_clipboard_text: String,
    blink_on: bool,
    blink_initial: bool,
    last_blink: Instant,
    com_initialized: bool,
    scrollbars: Vec<PaneScrollbar>,
    scroll_drag: Option<ScrollDrag>,
    scroll_hover: Option<WindowId>,
    scroll_activity: Instant,
    pointer_position: Option<(f32, f32)>,
    tracking_mouse: bool,
    wheel_remainder: i32,
    pending_surrogate: Option<u16>,
}

impl Gui {
    #[allow(clippy::result_large_err)]
    fn new(mut session: Session) -> Result<Self, (Session, String)> {
        unsafe {
            if CoInitializeEx(null_mut(), COINIT_APARTMENTTHREADED) < 0 {
                return Err((session, "cannot initialize COM".to_string()));
            }
        }

        let mut d2d: *mut ID2D1Factory = null_mut();
        let hr = unsafe {
            D2D1CreateFactory(
                D2D1_FACTORY_TYPE_SINGLE_THREADED,
                &IID_ID2D1_FACTORY,
                null(),
                &mut d2d as *mut *mut ID2D1Factory as *mut *mut c_void,
            )
        };
        if hr < 0 || d2d.is_null() {
            unsafe { CoUninitialize() };
            return Err((session, "cannot create Direct2D factory".to_string()));
        }

        let mut unknown: *mut IUnknown = null_mut();
        let hr = unsafe {
            DWriteCreateFactory(
                DWRITE_FACTORY_TYPE_SHARED,
                &IID_IDWRITE_FACTORY,
                &mut unknown,
            )
        };
        if hr < 0 || unknown.is_null() {
            unsafe {
                release(d2d);
                CoUninitialize();
            }
            return Err((session, "cannot create DirectWrite factory".to_string()));
        }
        let dwrite = unknown as *mut IDWriteFactory;

        let formats = match make_formats(dwrite, 1.0, &parse_font("")) {
            Ok(formats) => formats,
            Err(error) => {
                unsafe {
                    release(dwrite);
                    release(d2d);
                    CoUninitialize();
                }
                return Err((session, error));
            }
        };

        session.editor.gui_running = true;
        Ok(Self {
            hwnd: null_mut(),
            tui: Tui::new(session),
            d2d,
            dwrite,
            rt: null_mut(),
            formats,
            fallback_formats: Vec::new(),
            primary_face: null_mut(),
            fallback_face: null_mut(),
            glyph_cache: HashMap::new(),
            font_spec: String::new(),
            wide_spec: String::new(),
            brushes: HashMap::new(),
            scale: 1.0,
            cell_w: 9.0,
            cell_h: 18.0,
            cols: 80,
            rows: 24,
            client_w: 0,
            client_h: 0,
            focused: true,
            cursor_px: (0.0, 0.0),
            last_clipboard_seq: 0,
            last_clipboard_text: String::new(),
            blink_on: true,
            blink_initial: true,
            last_blink: Instant::now(),
            com_initialized: true,
            scrollbars: Vec::new(),
            scroll_drag: None,
            scroll_hover: None,
            scroll_activity: Instant::now(),
            pointer_position: None,
            tracking_mouse: false,
            wheel_remainder: 0,
            pending_surrogate: None,
        })
    }

    fn cleanup(&mut self) {
        unsafe {
            self.clear_brushes();
            release(self.rt);
            self.rt = null_mut();
            for format in self.formats.drain(..) {
                release(format);
            }
            for format in self.fallback_formats.drain(..) {
                release(format);
            }
            release(self.primary_face);
            self.primary_face = null_mut();
            release(self.fallback_face);
            self.fallback_face = null_mut();
            release(self.dwrite);
            self.dwrite = null_mut();
            release(self.d2d);
            self.d2d = null_mut();
            if self.com_initialized {
                CoUninitialize();
                self.com_initialized = false;
            }
        }
    }

    fn clear_brushes(&mut self) {
        unsafe {
            for (_, brush) in self.brushes.drain() {
                release(brush);
            }
        }
    }

    fn on_size(&mut self, width: i32, height: i32) {
        if width <= 0 || height <= 0 {
            return;
        }
        self.cancel_scroll_drag();
        self.client_w = width;
        self.client_h = height;
        self.update_metrics();
        self.resize_target();
    }

    fn update_metrics(&mut self) {
        let dpi = if self.hwnd.is_null() {
            96
        } else {
            unsafe { GetDpiForWindow(self.hwnd) }
        };
        self.update_metrics_at(dpi);
    }

    fn update_metrics_at(&mut self, dpi: u32) {
        let scale = dpi.max(96) as f32 / 96.0;
        let font_spec = self.tui.session.editor.options.str_or("guifont", "");
        let wide_spec = self.tui.session.editor.options.str_or("guifontwide", "");
        if (scale - self.scale).abs() > 0.01
            || font_spec != self.font_spec
            || wide_spec != self.wide_spec
        {
            let font = parse_font(&font_spec);
            if let Ok(formats) = make_formats(self.dwrite, scale, &font) {
                // Explicit wide-font sizes must survive a primary-font change.
                let fallback_formats = if wide_spec.is_empty() {
                    Vec::new()
                } else {
                    let wide_font = parse_font_with_default(&wide_spec, font.points);
                    make_formats(self.dwrite, scale, &wide_font).unwrap_or_default()
                };
                let primary_face = formats
                    .first()
                    .copied()
                    .map(font_face_from_format)
                    .unwrap_or(null_mut());
                let fallback_face = fallback_formats
                    .first()
                    .copied()
                    .map(font_face_from_format)
                    .unwrap_or(null_mut());
                unsafe {
                    for format in self.formats.drain(..) {
                        release(format);
                    }
                    for format in self.fallback_formats.drain(..) {
                        release(format);
                    }
                    release(self.primary_face);
                    release(self.fallback_face);
                }
                self.formats = formats;
                self.fallback_formats = fallback_formats;
                self.primary_face = primary_face;
                self.fallback_face = fallback_face;
                self.glyph_cache.clear();
                self.scale = scale;
                self.font_spec = font_spec;
                self.wide_spec = wide_spec;
            }
        }

        let (cell_w, cell_h) = if let Some(format) = self.formats.first().copied() {
            measure_cell(self.dwrite, format)
        } else {
            (9.0 * self.scale, 18.0 * self.scale)
        };
        self.cell_w = cell_w.max(1.0);
        let wide_height = self
            .fallback_formats
            .first()
            .copied()
            .map(|format| measure_text(self.dwrite, format, "漢").1)
            .unwrap_or(0.0);
        self.cell_h = cell_h.max(wide_height).max(1.0);
        self.cols = ((self.client_w as f32 / self.cell_w).floor() as usize).max(1);
        self.rows = ((self.client_h as f32 / self.cell_h).floor() as usize).max(1);
    }

    fn resize_target(&mut self) {
        if self.hwnd.is_null() || self.client_w <= 0 || self.client_h <= 0 {
            return;
        }
        let size = D2D1_SIZE_U {
            width: self.client_w as u32,
            height: self.client_h as u32,
        };
        if self.rt.is_null() {
            self.create_target(size);
        } else {
            unsafe {
                (*self.rt).Resize(&size);
            }
        }
    }

    fn create_target(&mut self, size: D2D1_SIZE_U) {
        unsafe {
            if !self.rt.is_null() {
                release(self.rt);
                self.rt = null_mut();
                self.clear_brushes();
            }
            let rt_props = D2D1_RENDER_TARGET_PROPERTIES {
                _type: D2D1_RENDER_TARGET_TYPE_DEFAULT,
                pixelFormat: D2D1_PIXEL_FORMAT {
                    format: DXGI_FORMAT_B8G8R8A8_UNORM,
                    alphaMode: D2D1_ALPHA_MODE_IGNORE,
                },
                dpiX: 96.0,
                dpiY: 96.0,
                usage: D2D1_RENDER_TARGET_USAGE_NONE,
                minLevel: D2D1_FEATURE_LEVEL_DEFAULT,
            };
            let hwnd_props = D2D1_HWND_RENDER_TARGET_PROPERTIES {
                hwnd: self.hwnd,
                pixelSize: size,
                presentOptions: D2D1_PRESENT_OPTIONS_NONE,
            };
            let mut rt = null_mut();
            let hr = (*self.d2d).CreateHwndRenderTarget(&rt_props, &hwnd_props, &mut rt);
            if hr >= 0 && !rt.is_null() {
                (*rt).SetTextAntialiasMode(D2D1_TEXT_ANTIALIAS_MODE_CLEARTYPE);
                self.rt = rt;
            }
        }
    }

    fn paint(&mut self) {
        if self.hwnd.is_null() || self.client_w <= 0 || self.client_h <= 0 {
            return;
        }
        if self.rt.is_null() {
            let size = D2D1_SIZE_U {
                width: self.client_w as u32,
                height: self.client_h as u32,
            };
            self.create_target(size);
            if self.rt.is_null() {
                return;
            }
        }
        self.update_metrics();
        let grid = self.tui.gui_grid(self.cols, self.rows);
        self.update_scrollbars();
        let bg = d2d_color(DEFAULT_BG);
        unsafe {
            (*self.rt).BeginDraw();
            (*self.rt).Clear(&bg);
        }

        for row in 0..self.rows {
            let mut col = 0usize;
            while col < self.cols {
                let Some(cell) = grid.cell(row, col) else {
                    break;
                };
                if cell.continuation {
                    col += 1;
                    continue;
                }
                let style = cell.style;
                let font = self.font_index(cell.ch);
                let start = col;
                let mut text = String::new();
                let mut span = 0usize;
                let wide = is_wide_char(cell.ch);
                if wide {
                    // Vim selects the wide font per double-width character and
                    // advances exactly two screen cells.  Drawing one glyph at
                    // a time keeps fallback CJK/emoji glyphs on the same grid.
                    text.push(cell.ch);
                    span = 2;
                    col += 2;
                } else {
                    while col < self.cols {
                        let Some(next) = grid.cell(row, col) else {
                            break;
                        };
                        if next.continuation {
                            col += 1;
                            continue;
                        }
                        if next.style != style
                            || is_wide_char(next.ch)
                            || self.font_index(next.ch) != font
                        {
                            break;
                        }
                        text.push(next.ch);
                        span += 1;
                        col += 1;
                    }
                }
                let (fg, bg) = colors(style);
                let x = start as f32 * self.cell_w;
                let y = row as f32 * self.cell_h;
                let width = span as f32 * self.cell_w;
                if bg != DEFAULT_BG {
                    let rect = D2D1_RECT_F {
                        left: x,
                        top: y,
                        right: x + width,
                        bottom: y + self.cell_h,
                    };
                    let brush = self.brush(bg);
                    if !brush.is_null() {
                        unsafe { (*self.rt).FillRectangle(&rect, brush as *mut ID2D1Brush) };
                    }
                }
                if fg != bg && !text.chars().all(|ch| ch == ' ') {
                    self.draw_text(&text, x, y, fg, style, font);
                }
                if style.underline {
                    let rect = D2D1_RECT_F {
                        left: x,
                        top: y + self.cell_h - 2.0,
                        right: x + width,
                        bottom: y + self.cell_h - 1.0,
                    };
                    let brush = self.brush(fg);
                    if !brush.is_null() {
                        unsafe { (*self.rt).FillRectangle(&rect, brush as *mut ID2D1Brush) };
                    }
                }
            }
        }

        if let Some((row, col)) = grid.cursor() {
            self.cursor_px = (col as f32 * self.cell_w, row as f32 * self.cell_h);
            if self.focused {
                self.update_ime_window();
            }
            if self.focused && self.blink_on && row < self.rows && col < self.cols {
                let cell = grid.cell(row, col).map(|cell| (cell.ch, cell.style));
                self.draw_cursor(row, col, cell);
            }
        }
        self.draw_scrollbars();
        unsafe {
            let hr = (*self.rt).EndDraw(null_mut(), null_mut());
            if hr == D2DERR_RECREATE_TARGET {
                self.clear_brushes();
                release(self.rt);
                self.rt = null_mut();
                InvalidateRect(self.hwnd, null(), 0);
            }
        }
    }

    fn tabline_rows(&self) -> usize {
        usize::from(
            self.tui.session.editor.tabpage_count() > 1
                && self
                    .tui
                    .session
                    .editor
                    .options
                    .str_or("guioptions", "")
                    .contains('e'),
        )
    }

    fn update_scrollbars(&mut self) {
        self.scrollbars.clear();
        let editor = &self.tui.session.editor;
        if scrollbar::right_scrollbar_enabled(&editor.options.str_or("guioptions", "")) {
            let tabline = self.tabline_rows();
            for id in editor.windows.ids() {
                let Some(win) = editor.windows.get(id) else {
                    continue;
                };
                let total = editor
                    .buffer(win.bufnr)
                    .map(|buf| buf.line_count())
                    .unwrap_or(1);
                let rect = win.rect;
                let right = if rect.col + rect.cols >= self.cols {
                    self.client_w as f32
                } else {
                    (rect.col + rect.cols) as f32 * self.cell_w
                };
                let track = ScrollRect {
                    left: (right - 14.0 * self.scale).max(rect.col as f32 * self.cell_w),
                    right,
                    top: (rect.row + tabline) as f32 * self.cell_h,
                    bottom: (rect.row + tabline + rect.text_rows()) as f32 * self.cell_h,
                };
                if let Some(geometry) = VerticalScrollbar::new(
                    track,
                    total,
                    rect.text_rows(),
                    win.top_line,
                    24.0 * self.scale,
                ) {
                    self.scrollbars.push(PaneScrollbar {
                        window: id,
                        geometry,
                    });
                }
            }
        }
        self.scroll_hover = self
            .pointer_position
            .and_then(|(x, y)| self.scrollbar_at(x, y).map(|bar| bar.window));
        if self
            .scroll_drag
            .is_some_and(|drag| !self.scrollbars.iter().any(|bar| bar.window == drag.window))
        {
            self.cancel_scroll_drag();
        }
    }

    fn scrollbar_at(&self, x: f32, y: f32) -> Option<PaneScrollbar> {
        self.scrollbars
            .iter()
            .copied()
            .find(|bar| bar.geometry.track.contains(x, y))
    }

    fn draw_scrollbars(&mut self) {
        for bar in self.scrollbars.clone() {
            let active = self
                .scroll_drag
                .is_some_and(|drag| drag.window == bar.window);
            let hover = self.scroll_hover == Some(bar.window);
            let opacity =
                scrollbar::opacity(active || hover, self.scroll_activity.elapsed().as_millis());
            if opacity <= 0.0 {
                continue;
            }
            let brush = self.brush((190, 190, 190));
            if brush.is_null() {
                continue;
            }
            let track = bar.geometry.track;
            let thumb = bar.geometry.thumb;
            let width = if active || hover { 10.0 } else { 6.0 } * self.scale;
            let right = (track.right - 2.0 * self.scale)
                .max(track.left + 1.0)
                .min(track.right);
            unsafe {
                if hover || active {
                    (*brush).SetOpacity(0.10);
                    let rect = D2D1_RECT_F {
                        left: track.left,
                        top: track.top,
                        right: track.right,
                        bottom: track.bottom,
                    };
                    (*self.rt).FillRectangle(&rect, brush as *mut ID2D1Brush);
                }
                (*brush).SetOpacity(
                    opacity
                        * if active {
                            0.85
                        } else if hover {
                            0.65
                        } else {
                            0.4
                        },
                );
                let rect = D2D1_ROUNDED_RECT {
                    rect: D2D1_RECT_F {
                        left: (right - width).max(track.left),
                        top: thumb.top,
                        right,
                        bottom: thumb.bottom,
                    },
                    radiusX: 2.0 * self.scale,
                    radiusY: 2.0 * self.scale,
                };
                (*self.rt).FillRoundedRectangle(&rect, brush as *mut ID2D1Brush);
                (*brush).SetOpacity(1.0);
            }
        }
    }

    fn cancel_scroll_drag(&mut self) {
        // Native move/resize loops also capture this HWND. Only release a
        // capture that was acquired by our scrollbar interaction.
        let dragging = self.scroll_drag.take().is_some();
        unsafe {
            if dragging && !self.hwnd.is_null() && GetCapture() == self.hwnd {
                ReleaseCapture();
            }
        }
    }

    fn scroll_to_pointer(&mut self, y: f32) {
        let Some(drag) = self.scroll_drag else {
            return;
        };
        let Some(bar) = self.scrollbars.iter().find(|bar| bar.window == drag.window) else {
            return;
        };
        let top = bar.geometry.top_for_pointer(y, drag.grab_offset);
        self.tui.gui_scroll_to(drag.window, top);
        self.scroll_activity = Instant::now();
        unsafe {
            InvalidateRect(self.hwnd, null(), 0);
        }
    }

    fn on_scrollbar_down(&mut self, x: f32, y: f32) -> bool {
        let Some(bar) = self.scrollbar_at(x, y) else {
            return false;
        };
        let thumb = bar.geometry.thumb;
        let grab_offset = if thumb.contains(x, y) {
            y - thumb.top
        } else {
            thumb.height() * 0.5
        };
        self.scroll_drag = Some(ScrollDrag {
            window: bar.window,
            grab_offset,
        });
        self.scroll_hover = Some(bar.window);
        unsafe {
            SetFocus(self.hwnd);
            SetCapture(self.hwnd);
        }
        self.scroll_to_pointer(y);
        true
    }

    fn on_pointer_move(&mut self, x: f32, y: f32) {
        self.pointer_position = Some((x, y));
        if !self.tracking_mouse {
            let mut track = TRACKMOUSEEVENT {
                cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32,
                dwFlags: TME_LEAVE,
                hwndTrack: self.hwnd,
                dwHoverTime: 0,
            };
            self.tracking_mouse = unsafe { TrackMouseEvent(&mut track) != 0 };
        }
        let hover = self.scrollbar_at(x, y).map(|bar| bar.window);
        if hover != self.scroll_hover {
            self.scroll_hover = hover;
            self.scroll_activity = Instant::now();
            unsafe {
                InvalidateRect(self.hwnd, null(), 0);
            }
        }
        if self.scroll_drag.is_some() {
            self.scroll_to_pointer(y);
        }
        unsafe {
            SetCursor(LoadCursorW(
                null_mut(),
                if self.scroll_hover.is_some() || self.scroll_drag.is_some() {
                    IDC_ARROW
                } else {
                    IDC_IBEAM
                },
            ));
        }
    }

    fn pointer_window(&self) -> WindowId {
        let editor = &self.tui.session.editor;
        if let Some((x, y)) = self.pointer_position {
            let offset = self.tabline_rows();
            if x >= 0.0 && y >= offset as f32 * self.cell_h {
                let row = (y / self.cell_h) as usize - offset;
                let col = (x / self.cell_w) as usize;
                for id in editor.windows.ids() {
                    if editor
                        .windows
                        .get(id)
                        .is_some_and(|win| win.rect.contains(row, col))
                    {
                        return id;
                    }
                }
            }
        }
        editor.windows.current_id()
    }

    fn has_glyph(&mut self, fallback: bool, ch: char) -> bool {
        let face = if fallback {
            self.fallback_face
        } else {
            self.primary_face
        };
        if face.is_null() {
            return false;
        }
        let key = (u8::from(fallback), ch);
        if let Some(value) = self.glyph_cache.get(&key) {
            return *value;
        }
        let value = font_has_glyph(face, ch);
        self.glyph_cache.insert(key, value);
        value
    }

    /// Double-width characters prefer guifontwide even if guifont covers them.
    fn font_index(&mut self, ch: char) -> usize {
        if is_wide_char(ch) && !self.fallback_formats.is_empty() {
            return 1;
        }
        if self.primary_face.is_null() {
            return usize::from(!self.fallback_face.is_null());
        }
        if !self.has_glyph(false, ch) && self.has_glyph(true, ch) {
            1
        } else {
            0
        }
    }

    fn draw_text(
        &mut self,
        text: &str,
        x: f32,
        y: f32,
        fg: (u8, u8, u8),
        style: Style,
        font: usize,
    ) {
        let index = (style.bold as usize) | ((style.italic as usize) << 1);
        let formats = if font == 1 && !self.fallback_formats.is_empty() {
            &self.fallback_formats
        } else {
            &self.formats
        };
        let Some(format) = formats.get(index).copied() else {
            return;
        };
        if format.is_null() {
            return;
        }
        let wide: Vec<u16> = text.encode_utf16().collect();
        let display_width: usize = text
            .chars()
            .map(|ch| rvim_term::char_width(ch).max(1))
            .sum();
        let max_w = display_width as f32 * self.cell_w + 4.0;
        let mut layout: *mut IDWriteTextLayout = null_mut();
        let hr = unsafe {
            (*self.dwrite).CreateTextLayout(
                wide.as_ptr(),
                wide.len() as u32,
                format,
                max_w,
                self.cell_h * 1.5,
                &mut layout,
            )
        };
        if hr < 0 || layout.is_null() {
            return;
        }
        // CJK/emoji layouts contain one double-width character. Keep its
        // requested height, centre it in two cells, and compress only when its
        // natural advance would overlap the next character.
        let mut metrics: DWRITE_TEXT_METRICS = unsafe { zeroed() };
        unsafe { (*layout).GetMetrics(&mut metrics) };
        let single_wide = text.chars().count() == 1 && text.chars().any(is_wide_char);
        let (x_scale, x_offset) = if single_wide {
            wide_placement(metrics.width, display_width as f32 * self.cell_w)
        } else {
            (1.0, 0.0)
        };
        let y = y + ((self.cell_h - metrics.height) * 0.5).max(0.0);
        let brush = self.brush(fg);
        if !brush.is_null() {
            unsafe {
                let mut previous: D2D1_MATRIX_3X2_F = zeroed();
                (*self.rt).GetTransform(&mut previous);
                let transform = D2D1_MATRIX_3X2_F {
                    matrix: [[x_scale, 0.0], [0.0, 1.0], [x + x_offset, y]],
                };
                (*self.rt).SetTransform(&transform);
                (*self.rt).DrawTextLayout(
                    D2D1_POINT_2F { x: 0.0, y: 0.0 },
                    layout,
                    brush as *mut ID2D1Brush,
                    D2D1_DRAW_TEXT_OPTIONS_ENABLE_COLOR_FONT,
                );
                (*self.rt).SetTransform(&previous);
            }
        }
        unsafe { release(layout) };
    }

    fn draw_cursor(&mut self, row: usize, col: usize, cell: Option<(char, Style)>) {
        let (fg, bg) = self.cursor_colors();
        let x = col as f32 * self.cell_w;
        let y = row as f32 * self.cell_h;
        let cell_width = cell
            .map(|(ch, _)| rvim_term::char_width(ch).max(1))
            .unwrap_or(1) as f32
            * self.cell_w;
        match cursor_shape(self.tui.session.editor.mode) {
            CursorShape::Block => {
                let rect = D2D1_RECT_F {
                    left: x,
                    top: y,
                    right: x + cell_width,
                    bottom: y + self.cell_h,
                };
                let brush = self.brush(bg);
                if !brush.is_null() {
                    unsafe { (*self.rt).FillRectangle(&rect, brush as *mut ID2D1Brush) };
                }
                if let Some((ch, style)) = cell {
                    if ch != ' ' {
                        let style = Style {
                            underline: false,
                            ..style
                        };
                        let font = self.font_index(ch);
                        self.draw_text(&ch.to_string(), x, y, fg, style, font);
                    }
                }
            }
            CursorShape::Vertical => {
                let width = (self.cell_w * 0.25).max(2.0);
                let rect = D2D1_RECT_F {
                    left: x,
                    top: y,
                    right: x + width,
                    bottom: y + self.cell_h,
                };
                let brush = self.brush(bg);
                if !brush.is_null() {
                    unsafe { (*self.rt).FillRectangle(&rect, brush as *mut ID2D1Brush) };
                }
            }
            CursorShape::Horizontal(percent) => {
                let height = (self.cell_h * percent).max(2.0);
                let rect = D2D1_RECT_F {
                    left: x,
                    top: y + self.cell_h - height,
                    right: x + cell_width,
                    bottom: y + self.cell_h,
                };
                let brush = self.brush(bg);
                if !brush.is_null() {
                    unsafe { (*self.rt).FillRectangle(&rect, brush as *mut ID2D1Brush) };
                }
            }
        }
        let _ = fg;
    }

    fn cursor_colors(&self) -> ((u8, u8, u8), (u8, u8, u8)) {
        let normal = self.tui.session.editor.syntax_highlight("Normal");
        let cursor = self.tui.session.editor.syntax_highlight("Cursor");
        let normal_fg = normal
            .and_then(|attr| attr.fg)
            .map(hl_rgb)
            .unwrap_or(DEFAULT_FG);
        let normal_bg = normal
            .and_then(|attr| attr.bg)
            .map(hl_rgb)
            .unwrap_or(DEFAULT_BG);
        let (mut fg, mut bg) = if let Some(attr) = cursor {
            (
                attr.fg.map(hl_rgb).unwrap_or(normal_bg),
                attr.bg.map(hl_rgb).unwrap_or(normal_fg),
            )
        } else {
            (normal_bg, normal_fg)
        };
        if cursor.is_some_and(|attr| attr.reverse) {
            std::mem::swap(&mut fg, &mut bg);
        }
        (fg, bg)
    }

    fn brush(&mut self, rgb: (u8, u8, u8)) -> *mut ID2D1SolidColorBrush {
        let key = (u32::from(rgb.0) << 16) | (u32::from(rgb.1) << 8) | u32::from(rgb.2);
        if let Some(brush) = self.brushes.get(&key) {
            return *brush;
        }
        if self.rt.is_null() {
            return null_mut();
        }
        let color = d2d_color(rgb);
        let mut brush = null_mut();
        let hr = unsafe { (*self.rt).CreateSolidColorBrush(&color, null(), &mut brush) };
        if hr < 0 || brush.is_null() {
            return null_mut();
        }
        self.brushes.insert(key, brush);
        brush
    }

    fn composition_string(himc: HIMC, flag: DWORD) -> Option<String> {
        let bytes = unsafe { ImmGetCompositionStringW(himc, flag, null_mut(), 0) };
        if bytes < 0 || bytes % 2 != 0 {
            return None;
        }
        if bytes == 0 {
            return Some(String::new());
        }
        let mut buffer = vec![0u16; bytes as usize / 2 + 1];
        let got = unsafe {
            ImmGetCompositionStringW(
                himc,
                flag,
                buffer.as_mut_ptr() as *mut c_void,
                (buffer.len() * 2) as DWORD,
            )
        };
        if got < 0 || got % 2 != 0 || got as usize > buffer.len() * 2 {
            return None;
        }
        Some(String::from_utf16_lossy(&buffer[..got as usize / 2]))
    }

    fn update_ime_window(&self) {
        if self.hwnd.is_null() {
            return;
        }
        unsafe {
            let himc = ImmGetContext(self.hwnd);
            if himc.is_null() {
                return;
            }
            let mut form: COMPOSITIONFORM = zeroed();
            form.dwStyle = CFS_POINT;
            form.ptCurrentPos = POINT {
                x: self.cursor_px.0 as i32,
                y: self.cursor_px.1 as i32,
            };
            ImmSetCompositionWindow(himc, &mut form);
            // Keep the IME candidate list visible and near the caret.  Without
            // an explicit candidate form some IMEs open it at the desktop
            // origin or behind the main window.
            for index in 0..4 {
                let mut candidate: CandidateForm = zeroed();
                candidate.dw_index = index;
                candidate.dw_style = CFS_CANDIDATEPOS;
                candidate.pt_current_pos = POINT {
                    x: self.cursor_px.0 as i32,
                    y: self.cursor_px.1 as i32 + self.cell_h as i32,
                };
                ImmSetCandidateWindow(himc, &candidate);
            }
            let font = self.ime_logfont();
            ImmSetCompositionFontW(himc, &font);
            ImmReleaseContext(self.hwnd, himc);
        }
    }

    fn ime_logfont(&self) -> LOGFONTW {
        let font = parse_font(&self.font_spec);
        let mut lf: LOGFONTW = unsafe { zeroed() };
        lf.lfHeight = -((font.points * (96.0 / 72.0) * self.scale).round() as i32);
        lf.lfWeight = font.weight as i32;
        lf.lfItalic = u8::from(font.italic);
        for (slot, ch) in lf
            .lfFaceName
            .iter_mut()
            .zip(font.family.encode_utf16().take(31))
        {
            *slot = ch;
        }
        lf
    }

    fn on_ime_start(&mut self) {
        self.update_ime_window();
        unsafe { InvalidateRect(self.hwnd, null(), 0) };
    }

    /// Consume only successfully retrieved results. Preedit/cancellation must
    /// reach DefWindowProc so the native IME can display its composition UI.
    fn on_ime_composition(&mut self, flags: DWORD) -> bool {
        if flags & GCS_RESULTSTR == 0 || self.hwnd.is_null() {
            return false;
        }
        let text = unsafe {
            let himc = ImmGetContext(self.hwnd);
            if himc.is_null() {
                return false;
            }
            let text = Self::composition_string(himc, GCS_RESULTSTR);
            ImmReleaseContext(self.hwnd, himc);
            text
        };
        let Some(text) = text else {
            return false;
        };
        self.commit_ime_text(&text);
        true
    }

    fn commit_ime_text(&mut self, text: &str) {
        self.pending_surrogate = None;
        for ch in text.chars() {
            self.feed(Key::Char(ch));
        }
    }

    fn on_ime_end(&mut self) {
        unsafe { InvalidateRect(self.hwnd, null(), 0) };
    }

    fn sync_register_from_clipboard(&mut self) {
        let seq = unsafe { GetClipboardSequenceNumber() };
        if seq == self.last_clipboard_seq {
            return;
        }
        let Some(text) = read_clipboard_text(self.hwnd) else {
            return;
        };
        let text = normalize_newlines(&text);
        self.last_clipboard_seq = seq;
        if text == self.last_clipboard_text {
            return;
        }
        let register = if text.ends_with('\n') {
            Register::linewise(text.clone())
        } else {
            Register {
                text: text.clone(),
                reg_type: RegType::CharWise,
            }
        };
        self.tui.session.editor.registers.put('*', register);
        self.last_clipboard_text = text;
    }

    fn sync_clipboard_from_register(&mut self) {
        let text = self
            .tui
            .session
            .editor
            .registers
            .get('*')
            .map(|register| register.text.clone())
            .unwrap_or_default();
        if text == self.last_clipboard_text {
            return;
        }
        if write_clipboard_text(self.hwnd, &text) {
            self.last_clipboard_text = text;
            self.last_clipboard_seq = unsafe { GetClipboardSequenceNumber() };
        }
    }

    fn paste_clipboard(&mut self) {
        self.sync_register_from_clipboard();
        let text = self
            .tui
            .session
            .editor
            .registers
            .get('*')
            .map(|register| register.text.clone())
            .unwrap_or_default();
        if text.is_empty() {
            return;
        }
        if self.tui.session.editor.mode.is_insert_like() {
            for ch in text.chars() {
                match ch {
                    '\n' => self.feed(Key::Special("CR".to_string())),
                    '\r' => {}
                    ch => self.feed(Key::Char(ch)),
                }
            }
        } else {
            self.feed(Key::Char('"'));
            self.feed(Key::Char('*'));
            self.feed(Key::Char('p'));
        }
    }

    fn copy_to_clipboard(&mut self) {
        let ed = &mut self.tui.session.editor;
        let range = if let Some(range) = rvim_normal::visual::range(ed) {
            range
        } else {
            let line = ed.cursor().line;
            OpRange {
                start: Pos::new(line, 0),
                end: Pos::new(line, 0),
                linewise: true,
            }
        };
        let _ = yank_range(ed, range, Some('*'));
        if ed.visual.is_some() {
            rvim_normal::visual::stop(ed);
        }
        self.sync_clipboard_from_register();
        unsafe { InvalidateRect(self.hwnd, null(), 0) };
    }

    fn feed(&mut self, key: Key) {
        self.scroll_activity = Instant::now();
        self.sync_register_from_clipboard();
        self.blink_on = true;
        self.blink_initial = true;
        self.last_blink = Instant::now();
        let _ = self.tui.feed_key(key);
        self.sync_clipboard_from_register();
        if self.tui.quit {
            unsafe { PostMessageW(self.hwnd, WM_CLOSE, 0, 0) };
        } else {
            unsafe { InvalidateRect(self.hwnd, null(), 0) };
        }
    }

    fn on_key(&mut self, vk: u32) {
        if vk == VK_INSERT as u32 {
            let ctrl = unsafe { GetKeyState(VK_CONTROL) as u16 & 0x8000 != 0 };
            let shift = unsafe { GetKeyState(VK_SHIFT) as u16 & 0x8000 != 0 };
            if shift {
                self.paste_clipboard();
                return;
            }
            if ctrl {
                self.copy_to_clipboard();
                return;
            }
        }
        if let Some(key) = translate_vkey(vk) {
            self.feed(key);
        }
    }

    fn on_char(&mut self, code: u32) {
        let ctrl = unsafe { GetKeyState(VK_CONTROL) as u16 & 0x8000 != 0 };
        let alt = unsafe { GetKeyState(VK_MENU) as u16 & 0x8000 != 0 };
        if ctrl || alt {
            self.pending_surrogate = None;
            return;
        }
        if let Some(ch) = decode_char(&mut self.pending_surrogate, code) {
            if ch >= ' ' && ch != '\u{7f}' {
                self.feed(Key::Char(ch));
            }
        }
    }

    fn on_wheel(&mut self, delta: i16) {
        self.wheel_remainder += i32::from(delta);
        let notches = self.wheel_remainder / 120;
        self.wheel_remainder %= 120;
        if notches == 0 {
            return;
        }
        let id = self.pointer_window();
        if let Some(win) = self.tui.session.editor.windows.get(id) {
            let lines = notches.unsigned_abs() as usize * 3;
            let top = if notches > 0 {
                win.top_line.saturating_sub(lines)
            } else {
                win.top_line.saturating_add(lines)
            };
            self.tui.gui_scroll_to(id, top);
            self.scroll_activity = Instant::now();
            unsafe {
                InvalidateRect(self.hwnd, null(), 0);
            }
        }
    }

    fn on_timer(&mut self) {
        let interval = if self.blink_initial {
            700
        } else if self.blink_on {
            250
        } else {
            400
        };
        if self.last_blink.elapsed() >= Duration::from_millis(interval) {
            self.blink_on = !self.blink_on;
            self.blink_initial = false;
            self.last_blink = Instant::now();
        }
        rvim_ex::m7::pump(
            &mut self.tui.session.editor,
            &mut self.tui.session.viml.env,
            &mut self.tui.session.viml.state,
        );
        if self.tui.quit {
            unsafe { PostMessageW(self.hwnd, WM_CLOSE, 0, 0) };
        } else {
            unsafe { InvalidateRect(self.hwnd, null(), 0) };
        }
    }
}

pub fn run(session: Session) -> (Session, i32) {
    log("run start");
    rvim_ex::gui_font::set_font_dialog_hook(Some(choose_font));
    let mut gui = match Gui::new(session) {
        Ok(gui) => Box::new(gui),
        Err((session, error)) => {
            log(&format!("Gui::new error: {error}"));
            show_error(&error);
            return (session, 1);
        }
    };
    let hwnd = unsafe { create_window(&mut *gui) };
    log(&format!("hwnd={hwnd:?}"));
    if hwnd.is_null() {
        gui.cleanup();
        rvim_ex::gui_font::set_font_dialog_hook(None);
        DIALOG_OWNER.store(0, Ordering::Relaxed);
        let session = gui.tui.session;
        return (session, 1);
    }
    gui.hwnd = hwnd;
    gui.sync_register_from_clipboard();
    DIALOG_OWNER.store(hwnd as isize, Ordering::Relaxed);
    unsafe {
        let mut rect = zeroed();
        GetClientRect(hwnd, &mut rect);
        gui.on_size(rect.right - rect.left, rect.bottom - rect.top);
        ShowWindow(hwnd, SW_SHOW);
        UpdateWindow(hwnd);
        SetTimer(hwnd, 1, 100, None);
        if let Ok(value) = std::env::var("RVIM_GUI_AUTOCLOSE_MS") {
            if let Ok(ms) = value.parse::<u32>() {
                SetTimer(hwnd, 2, ms, None);
            }
        }
        let mut msg: MSG = zeroed();
        log("message loop");
        while GetMessageW(&mut msg, null_mut(), 0, 0) > 0 {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
    log("message loop end");
    let code = gui.tui.exit_code;
    gui.cleanup();
    rvim_ex::gui_font::set_font_dialog_hook(None);
    DIALOG_OWNER.store(0, Ordering::Relaxed);
    let session = gui.tui.session;
    (session, code)
}

#[allow(clippy::manual_dangling_ptr)]
unsafe fn create_window(gui: *mut Gui) -> HWND {
    let instance = GetModuleHandleW(null());
    let class = Box::leak(wide("RustVimGui").into_boxed_slice());
    let title = Box::leak(wide("RustVim").into_boxed_slice());
    let mut wc: WNDCLASSW = zeroed();
    wc.style = CS_HREDRAW | CS_VREDRAW;
    wc.lpfnWndProc = Some(wndproc);
    wc.hInstance = instance;
    wc.hIcon = LoadIconW(instance, 1 as *const u16);
    wc.hCursor = LoadCursorW(null_mut(), IDC_IBEAM);
    wc.hbrBackground = null_mut::<c_void>() as HBRUSH;
    wc.lpszMenuName = null();
    wc.lpszClassName = class.as_ptr();
    RegisterClassW(&wc);
    CreateWindowExW(
        0,
        class.as_ptr(),
        title.as_ptr(),
        WS_OVERLAPPEDWINDOW | WS_VISIBLE,
        CW_USEDEFAULT,
        CW_USEDEFAULT,
        1100,
        720,
        null_mut(),
        null_mut(),
        instance,
        gui as *mut c_void,
    )
}

fn scrollbar_owns_cursor(hit_test: isize, hovering: bool, dragging: bool) -> bool {
    hit_test == HTCLIENT && (hovering || dragging)
}

unsafe extern "system" fn wndproc(
    hwnd: HWND,
    msg: UINT,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if msg == WM_NCCREATE {
        let create = &*(lparam as *const CREATESTRUCTW);
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, create.lpCreateParams as LONG_PTR);
        return DefWindowProcW(hwnd, msg, wparam, lparam);
    }
    let gui = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut Gui;
    if gui.is_null() {
        return DefWindowProcW(hwnd, msg, wparam, lparam);
    }
    match msg {
        WM_SIZE => {
            let width = (lparam as usize & 0xffff) as i32;
            let height = ((lparam as usize >> 16) & 0xffff) as i32;
            (*gui).on_size(width, height);
            0
        }
        WM_PAINT => {
            let mut paint: PAINTSTRUCT = zeroed();
            BeginPaint(hwnd, &mut paint);
            (*gui).paint();
            EndPaint(hwnd, &paint);
            0
        }
        WM_ERASEBKGND => 1,
        WM_KEYDOWN | WM_SYSKEYDOWN => {
            (*gui).on_key(wparam as u32);
            0
        }
        WM_CHAR => {
            (*gui).on_char(wparam as u32);
            0
        }
        WM_IME_SETCONTEXT => DefWindowProcW(hwnd, msg, wparam, lparam),
        WM_IME_STARTCOMPOSITION => {
            (*gui).on_ime_start();
            DefWindowProcW(hwnd, msg, wparam, lparam)
        }
        WM_IME_COMPOSITION => {
            if (*gui).on_ime_composition(lparam as DWORD) {
                // The result was inserted above. Forward only any simultaneous
                // new preedit, excluding result flags to prevent a second commit.
                let preedit = lparam & !GCS_RESULT_FLAGS;
                if preedit != 0 {
                    DefWindowProcW(hwnd, msg, wparam, preedit)
                } else {
                    0
                }
            } else {
                DefWindowProcW(hwnd, msg, wparam, lparam)
            }
        }
        WM_IME_ENDCOMPOSITION => {
            (*gui).on_ime_end();
            DefWindowProcW(hwnd, msg, wparam, lparam)
        }
        WM_IME_CHAR => {
            // Fallback IMEs deliver committed UTF-16 directly. Do not suppress
            // these characters because a Ctrl/Alt IME-switch key is still down.
            if let Some(ch) = decode_char(&mut (*gui).pending_surrogate, wparam as u32) {
                (*gui).feed(Key::Char(ch));
            }
            0
        }
        WM_UNICHAR => {
            if wparam == UNICODE_NOCHAR {
                return 1;
            }
            (*gui).pending_surrogate = None;
            if let Some(ch) = char::from_u32(wparam as u32) {
                if ch >= ' ' && ch != '\u{7f}' {
                    (*gui).feed(Key::Char(ch));
                }
            }
            0
        }
        WM_MOUSEMOVE => {
            let x = (lparam as u16 as i16) as f32;
            let y = ((lparam as usize >> 16) as u16 as i16) as f32;
            (*gui).on_pointer_move(x, y);
            0
        }
        WM_MOUSELEAVE => {
            (*gui).tracking_mouse = false;
            (*gui).pointer_position = None;
            (*gui).scroll_hover = None;
            (*gui).scroll_activity = Instant::now();
            InvalidateRect(hwnd, null(), 0);
            0
        }
        WM_LBUTTONDOWN => {
            let x = (lparam as u16 as i16) as f32;
            let y = ((lparam as usize >> 16) as u16 as i16) as f32;
            if (*gui).on_scrollbar_down(x, y) {
                0
            } else {
                DefWindowProcW(hwnd, msg, wparam, lparam)
            }
        }
        WM_LBUTTONUP => {
            if (*gui).scroll_drag.is_some() {
                (*gui).cancel_scroll_drag();
                (*gui).scroll_activity = Instant::now();
                InvalidateRect(hwnd, null(), 0);
                0
            } else {
                DefWindowProcW(hwnd, msg, wparam, lparam)
            }
        }
        WM_CAPTURECHANGED => {
            (*gui).scroll_drag = None;
            InvalidateRect(hwnd, null(), 0);
            0
        }
        WM_CANCELMODE => {
            (*gui).cancel_scroll_drag();
            InvalidateRect(hwnd, null(), 0);
            DefWindowProcW(hwnd, msg, wparam, lparam)
        }
        WM_SETCURSOR
            if scrollbar_owns_cursor(
                lparam as u16 as i16 as isize,
                (*gui).scroll_hover.is_some(),
                (*gui).scroll_drag.is_some(),
            ) =>
        {
            SetCursor(LoadCursorW(null_mut(), IDC_ARROW));
            1
        }
        WM_MOUSEWHEEL => {
            let mut point = POINT {
                x: (lparam as u16 as i16) as i32,
                y: ((lparam as usize >> 16) as u16 as i16) as i32,
            };
            ScreenToClient(hwnd, &mut point);
            (*gui).pointer_position = Some((point.x as f32, point.y as f32));
            let delta = ((wparam >> 16) & 0xffff) as u16 as i16;
            (*gui).on_wheel(delta);
            0
        }
        WM_TIMER => {
            if wparam == 2 {
                PostMessageW(hwnd, WM_CLOSE, 0, 0);
            } else {
                (*gui).on_timer();
            }
            0
        }
        WM_SETFOCUS => {
            (*gui).focused = true;
            (*gui).blink_on = true;
            (*gui).blink_initial = true;
            (*gui).last_blink = Instant::now();
            InvalidateRect(hwnd, null(), 0);
            0
        }
        WM_KILLFOCUS => {
            (*gui).pending_surrogate = None;
            (*gui).cancel_scroll_drag();
            (*gui).focused = false;
            InvalidateRect(hwnd, null(), 0);
            0
        }
        WM_DPICHANGED => {
            let suggested = &*(lparam as *const winapi::shared::windef::RECT);
            SetWindowPos(
                hwnd,
                null_mut(),
                suggested.left,
                suggested.top,
                suggested.right - suggested.left,
                suggested.bottom - suggested.top,
                SWP_NOZORDER | SWP_NOACTIVATE,
            );
            // WM_SIZE normally lands before this message returns, but update
            // explicitly from the authoritative WM_DPICHANGED DPI value so the
            // font formats are rebuilt even when the suggested size is
            // unchanged.
            let new_dpi = (wparam & 0xffff) as u32;
            (*gui).update_metrics_at(new_dpi);
            (*gui).resize_target();
            InvalidateRect(hwnd, null(), 0);
            0
        }
        WM_CLOSE => {
            if (*gui).tui.quit || !(*gui).tui.session.editor.current_buffer().is_changed() {
                DestroyWindow(hwnd);
            } else {
                (*gui).tui.message =
                    "E37: No write since last change (add ! to override)".to_string();
                InvalidateRect(hwnd, null(), 0);
            }
            0
        }
        WM_DESTROY => {
            KillTimer(hwnd, 1);
            PostQuitMessage(0);
            0
        }
        WM_NCDESTROY => {
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
            DefWindowProcW(hwnd, msg, wparam, lparam)
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

fn make_formats(
    dwrite: *mut IDWriteFactory,
    scale: f32,
    spec: &FontSpec,
) -> Result<Vec<*mut IDWriteTextFormat>, String> {
    // ChooseFont returns a GDI face name (e.g. "Maple Mono NF CN Light").
    // DirectWrite expects its family name ("Maple Mono NF CN") with the weight
    // passed separately. An unrecognized name silently selects a fallback font.
    let family = resolve_font_family(dwrite, spec).unwrap_or_else(|| spec.family.clone());
    let font = wide(&family);
    let locale = wide("en-us");
    let size = spec.points * (96.0 / 72.0) * scale;
    let base_style = if spec.italic {
        DWRITE_FONT_STYLE_ITALIC
    } else {
        DWRITE_FONT_STYLE_NORMAL
    };
    let bold_weight = spec.weight.max(DWRITE_FONT_WEIGHT_BOLD);
    let mut formats = Vec::with_capacity(4);
    for (weight, style) in [
        (spec.weight, base_style),
        (bold_weight, base_style),
        (spec.weight, DWRITE_FONT_STYLE_ITALIC),
        (bold_weight, DWRITE_FONT_STYLE_ITALIC),
    ] {
        let mut format = null_mut();
        let hr = unsafe {
            (*dwrite).CreateTextFormat(
                font.as_ptr(),
                null_mut(),
                weight,
                style,
                DWRITE_FONT_STRETCH_NORMAL,
                size,
                locale.as_ptr(),
                &mut format,
            )
        };
        if hr < 0 || format.is_null() {
            unsafe {
                for item in formats {
                    release(item);
                }
            }
            return Err("cannot create DirectWrite text format".to_string());
        }
        unsafe {
            (*format).SetWordWrapping(DWRITE_WORD_WRAPPING_NO_WRAP);
            (*format).SetTextAlignment(DWRITE_TEXT_ALIGNMENT_LEADING);
            (*format).SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_NEAR);
        }
        formats.push(format);
    }
    Ok(formats)
}

fn resolve_font_family(dwrite: *mut IDWriteFactory, spec: &FontSpec) -> Option<String> {
    let requested = wide(&spec.family);
    unsafe {
        let mut collection: *mut IDWriteFontCollection = null_mut();
        if (*dwrite).GetSystemFontCollection(&mut collection, 0) >= 0 && !collection.is_null() {
            let mut index = 0;
            let mut exists = 0;
            let found = (*collection).FindFamilyName(requested.as_ptr(), &mut index, &mut exists)
                >= 0
                && exists != 0;
            release(collection);
            if found {
                return Some(spec.family.clone());
            }
        }
        // GDI interop resolves legacy/style-specific and localized face names
        // using the same LOGFONT contract as the native font dialog.
        let mut interop: *mut IDWriteGdiInterop = null_mut();
        if (*dwrite).GetGdiInterop(&mut interop) < 0 || interop.is_null() {
            return None;
        }
        let mut lf: LOGFONTW = zeroed();
        lf.lfWeight = spec.weight as i32;
        lf.lfItalic = u8::from(spec.italic);
        for (slot, unit) in lf
            .lfFaceName
            .iter_mut()
            .zip(spec.family.encode_utf16().take(31))
        {
            *slot = unit;
        }
        let mut font: *mut IDWriteFont = null_mut();
        let result = (*interop).CreateFontFromLOGFONT(&lf, &mut font);
        release(interop);
        if result < 0 || font.is_null() {
            return None;
        }
        let mut family: *mut IDWriteFontFamily = null_mut();
        let result = (*font).GetFontFamily(&mut family);
        release(font);
        if result < 0 || family.is_null() {
            return None;
        }
        let mut names: *mut IDWriteLocalizedStrings = null_mut();
        let result = (*family).GetFamilyNames(&mut names);
        release(family);
        if result < 0 || names.is_null() {
            return None;
        }
        let resolved = (|| {
            let mut index = 0;
            let mut exists = 0;
            if (*names).FindLocaleName(wide("en-us").as_ptr(), &mut index, &mut exists) < 0
                || exists == 0
            {
                index = 0;
            }
            let mut len = 0;
            if (*names).GetStringLength(index, &mut len) < 0 || len == 0 {
                return None;
            }
            let mut buffer = vec![0u16; len as usize + 1];
            if (*names).GetString(index, buffer.as_mut_ptr(), len + 1) < 0 {
                return None;
            }
            String::from_utf16(&buffer[..len as usize]).ok()
        })();
        release(names);
        resolved
    }
}

fn font_face_from_format(format: *mut IDWriteTextFormat) -> *mut IDWriteFontFace {
    unsafe {
        let mut collection: *mut IDWriteFontCollection = null_mut();
        if (*format).GetFontCollection(&mut collection) < 0 || collection.is_null() {
            return null_mut();
        }
        let name_len = (*format).GetFontFamilyNameLength();
        if name_len == 0 {
            release(collection);
            return null_mut();
        }
        let mut name = vec![0u16; name_len as usize + 1];
        if (*format).GetFontFamilyName(name.as_mut_ptr(), name_len + 1) < 0 {
            release(collection);
            return null_mut();
        }
        let mut index = 0u32;
        let mut exists = 0i32;
        if (*collection).FindFamilyName(name.as_ptr(), &mut index, &mut exists) < 0 || exists == 0 {
            release(collection);
            return null_mut();
        }
        let mut family: *mut IDWriteFontFamily = null_mut();
        if (*collection).GetFontFamily(index, &mut family) < 0 || family.is_null() {
            release(collection);
            return null_mut();
        }
        let mut font: *mut winapi::um::dwrite::IDWriteFont = null_mut();
        let hr = (*family).GetFirstMatchingFont(
            (*format).GetFontWeight(),
            (*format).GetFontStretch(),
            (*format).GetFontStyle(),
            &mut font,
        );
        release(family);
        if hr < 0 || font.is_null() {
            release(collection);
            return null_mut();
        }
        let mut face: *mut IDWriteFontFace = null_mut();
        let hr = (*font).CreateFontFace(&mut face);
        release(font);
        release(collection);
        if hr < 0 { null_mut() } else { face }
    }
}

fn font_has_glyph(face: *mut IDWriteFontFace, ch: char) -> bool {
    if face.is_null() {
        return false;
    }
    let code = ch as u32;
    let mut glyph = 0u16;
    let hr = unsafe { (*face).GetGlyphIndices(&code, 1, &mut glyph) };
    hr >= 0 && glyph != 0
}

fn wide_placement(measured: f32, available: f32) -> (f32, f32) {
    if measured <= 0.0 || available <= 0.0 {
        return (1.0, 0.0);
    }
    let scale = (available / measured).min(1.0);
    (scale, (available - measured * scale) * 0.5)
}

fn measure_text(
    dwrite: *mut IDWriteFactory,
    format: *mut IDWriteTextFormat,
    text: &str,
) -> (f32, f32) {
    let sample = wide(text);
    let mut layout: *mut IDWriteTextLayout = null_mut();
    let count = text.encode_utf16().count() as u32;
    let hr = unsafe {
        (*dwrite).CreateTextLayout(
            sample.as_ptr(),
            count,
            format,
            100_000.0,
            200.0,
            &mut layout,
        )
    };
    if hr < 0 || layout.is_null() {
        return (9.0, 18.0);
    }
    let mut metrics: DWRITE_TEXT_METRICS = unsafe { zeroed() };
    unsafe {
        (*layout).GetMetrics(&mut metrics);
        release(layout);
    }
    (metrics.width.max(0.0), (metrics.height * 1.05).max(1.0))
}

fn measure_cell(dwrite: *mut IDWriteFactory, format: *mut IDWriteTextFormat) -> (f32, f32) {
    const LATIN_SAMPLE: &str = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";
    let (width, height) = measure_text(dwrite, format, LATIN_SAMPLE);
    (
        (width / LATIN_SAMPLE.chars().count() as f32).max(1.0),
        height,
    )
}

#[derive(Clone, Copy)]
enum CursorShape {
    Block,
    Vertical,
    Horizontal(f32),
}

fn cursor_shape(mode: Mode) -> CursorShape {
    match mode {
        Mode::Insert => CursorShape::Vertical,
        Mode::Replace => CursorShape::Horizontal(0.2),
        Mode::MotionPending { .. } | Mode::OperatorPending | Mode::RegisterPending => {
            CursorShape::Horizontal(0.5)
        }
        _ => CursorShape::Block,
    }
}

fn hl_rgb(color: HlColor) -> (u8, u8, u8) {
    match color {
        HlColor::Rgb(r, g, b) => (r, g, b),
        HlColor::Indexed(index) => ansi_rgb(index),
    }
}

fn is_wide_char(ch: char) -> bool {
    rvim_term::char_width(ch) == 2
}

#[derive(Debug, PartialEq)]
struct FontSpec {
    family: String,
    points: f32,
    weight: u32,
    italic: bool,
}

fn parse_font(spec: &str) -> FontSpec {
    parse_font_with_default(spec, BASE_FONT_POINTS)
}

fn parse_font_with_default(spec: &str, default_points: f32) -> FontSpec {
    let mut font = FontSpec {
        family: DEFAULT_FONT.to_string(),
        points: default_points,
        weight: DWRITE_FONT_WEIGHT_NORMAL,
        italic: false,
    };
    for (index, part) in spec.split(':').enumerate() {
        if index == 0 {
            if !part.is_empty() && part != "*" {
                font.family = part.replace('_', " ");
            }
        } else if let Some(height) = part.strip_prefix('h') {
            if let Ok(value) = height.parse::<f32>() {
                if value.is_finite() {
                    font.points = value.clamp(4.0, 200.0);
                }
            }
        } else if part == "b" {
            font.weight = DWRITE_FONT_WEIGHT_BOLD;
        } else if part == "i" {
            font.italic = true;
        } else if let Some(weight) = part.strip_prefix('W') {
            if let Ok(value) = weight.parse::<u32>() {
                font.weight = value.clamp(1, 999);
            }
        }
    }
    font
}

fn font_string(family: &str, points: f32, weight: i32, italic: bool) -> String {
    let mut spec = format!("{}:h{}", family.replace(' ', "_"), points);
    if weight != DWRITE_FONT_WEIGHT_NORMAL as i32 {
        spec.push_str(&format!(":W{}", weight.clamp(1, 999)));
    }
    if italic {
        spec.push_str(":i");
    }
    spec
}

fn choose_font(current: &str, _wide: bool) -> Option<String> {
    let font = parse_font(current);
    let points = font.points;
    let mut lf: LOGFONTW = unsafe { zeroed() };
    lf.lfHeight = -((points * 96.0 / 72.0).round() as i32);
    lf.lfWeight = font.weight as i32;
    lf.lfItalic = u8::from(font.italic);
    let face: Vec<u16> = font.family.encode_utf16().take(31).collect();
    for (slot, ch) in lf.lfFaceName.iter_mut().zip(face) {
        *slot = ch;
    }
    let mut cf: CHOOSEFONTW = unsafe { zeroed() };
    cf.lStructSize = std::mem::size_of::<CHOOSEFONTW>() as u32;
    cf.hwndOwner = DIALOG_OWNER.load(Ordering::Relaxed) as HWND;
    cf.lpLogFont = &mut lf;
    cf.iPointSize = (points * 10.0) as i32;
    cf.Flags = CF_SCREENFONTS | CF_INITTOLOGFONTSTRUCT | CF_EFFECTS;
    if unsafe { ChooseFontW(&mut cf) } == 0 {
        return None;
    }
    let face = String::from_utf16_lossy(
        &lf.lfFaceName[..lf
            .lfFaceName
            .iter()
            .position(|ch| *ch == 0)
            .unwrap_or(lf.lfFaceName.len())],
    );
    let chosen_points = if cf.iPointSize > 0 {
        cf.iPointSize as f32 / 10.0
    } else {
        points
    };
    Some(font_string(
        &face,
        chosen_points,
        lf.lfWeight,
        lf.lfItalic != 0,
    ))
}

fn normalize_newlines(text: &str) -> String {
    text.replace("\r\n", "\n").replace('\r', "\n")
}

fn read_clipboard_text(hwnd: HWND) -> Option<String> {
    unsafe {
        if IsClipboardFormatAvailable(CF_UNICODETEXT) == 0 {
            return None;
        }
        if OpenClipboard(hwnd) == 0 {
            return None;
        }
        let handle = GetClipboardData(CF_UNICODETEXT);
        if handle.is_null() {
            CloseClipboard();
            return None;
        }
        let pointer = GlobalLock(handle as HGLOBAL) as *const u16;
        if pointer.is_null() {
            CloseClipboard();
            return None;
        }
        let mut len = 0usize;
        while *pointer.add(len) != 0 {
            len += 1;
        }
        let text = String::from_utf16_lossy(std::slice::from_raw_parts(pointer, len));
        GlobalUnlock(handle as HGLOBAL);
        CloseClipboard();
        Some(normalize_newlines(&text))
    }
}

fn write_clipboard_text(hwnd: HWND, text: &str) -> bool {
    let mut wide: Vec<u16> = text.replace('\n', "\r\n").encode_utf16().collect();
    wide.push(0);
    unsafe {
        if OpenClipboard(hwnd) == 0 {
            return false;
        }
        EmptyClipboard();
        let memory = GlobalAlloc(GMEM_MOVEABLE, wide.len() * 2);
        if memory.is_null() {
            CloseClipboard();
            return false;
        }
        let pointer = GlobalLock(memory) as *mut u16;
        if pointer.is_null() {
            GlobalFree(memory);
            CloseClipboard();
            return false;
        }
        std::ptr::copy_nonoverlapping(wide.as_ptr(), pointer, wide.len());
        GlobalUnlock(memory);
        if SetClipboardData(CF_UNICODETEXT, memory as HANDLE).is_null() {
            GlobalFree(memory);
            CloseClipboard();
            return false;
        }
        CloseClipboard();
        true
    }
}

fn colors(style: Style) -> ((u8, u8, u8), (u8, u8, u8)) {
    let mut fg = color_rgb(style.fg, DEFAULT_FG);
    let mut bg = color_rgb(style.bg, DEFAULT_BG);
    if style.reverse {
        std::mem::swap(&mut fg, &mut bg);
    }
    (fg, bg)
}

fn color_rgb(color: Color, default: (u8, u8, u8)) -> (u8, u8, u8) {
    match color {
        Color::Default => default,
        Color::Rgb(r, g, b) => (r, g, b),
        Color::Ansi(index) => ansi_rgb(index),
    }
}

fn ansi_rgb(index: u8) -> (u8, u8, u8) {
    const BASE: [(u8, u8, u8); 16] = [
        (0, 0, 0),
        (197, 15, 31),
        (19, 161, 14),
        (193, 156, 0),
        (0, 55, 218),
        (136, 23, 152),
        (58, 150, 221),
        (204, 204, 204),
        (118, 118, 118),
        (231, 72, 86),
        (22, 198, 12),
        (249, 241, 165),
        (59, 120, 255),
        (180, 0, 158),
        (97, 214, 214),
        (242, 242, 242),
    ];
    if index < 16 {
        return BASE[index as usize];
    }
    if index < 232 {
        const LEVELS: [u8; 6] = [0, 95, 135, 175, 215, 255];
        let value = index - 16;
        let r = LEVELS[(value / 36) as usize];
        let g = LEVELS[((value % 36) / 6) as usize];
        let b = LEVELS[(value % 6) as usize];
        return (r, g, b);
    }
    let gray = 8 + (index - 232) * 10;
    (gray, gray, gray)
}

fn d2d_color(rgb: (u8, u8, u8)) -> D2D1_COLOR_F {
    D2D1_COLOR_F {
        r: rgb.0 as f32 / 255.0,
        g: rgb.1 as f32 / 255.0,
        b: rgb.2 as f32 / 255.0,
        a: 1.0,
    }
}

/// WM_CHAR/WM_IME_CHAR carry UTF-16 units, not Unicode scalar values.
fn decode_char(pending: &mut Option<u16>, code: u32) -> Option<char> {
    if (0xd800..=0xdbff).contains(&code) {
        *pending = Some(code as u16);
        return None;
    }
    let high = pending.take();
    if (0xdc00..=0xdfff).contains(&code) {
        let high = u32::from(high?);
        return char::from_u32(0x10000 + ((high - 0xd800) << 10) + code - 0xdc00);
    }
    char::from_u32(code)
}

fn translate_vkey(vk: u32) -> Option<Key> {
    let vk = vk as i32;
    let ctrl = unsafe { GetKeyState(VK_CONTROL) as u16 & 0x8000 != 0 };
    let shift = unsafe { GetKeyState(VK_SHIFT) as u16 & 0x8000 != 0 };
    if ctrl {
        if (0x41..=0x5A).contains(&vk) {
            let ch = char::from_u32(vk as u32)?;
            return Some(Key::Special(format!("C-{ch}")));
        }
        let name = match vk {
            VK_LEFT => "LEFT",
            VK_RIGHT => "RIGHT",
            VK_UP => "UP",
            VK_DOWN => "DOWN",
            VK_HOME => "HOME",
            VK_END => "END",
            VK_PRIOR => "PAGEUP",
            VK_NEXT => "PAGEDOWN",
            VK_SPACE => "SPACE",
            VK_OEM_6 => "]",
            _ => return None,
        };
        return Some(Key::Special(format!("C-{name}")));
    }
    let prefix = if shift { "S-" } else { "" };
    let name = match vk {
        VK_BACK => "BS",
        VK_RETURN => "CR",
        VK_ESCAPE => "ESC",
        VK_TAB => "TAB",
        VK_LEFT => "LEFT",
        VK_RIGHT => "RIGHT",
        VK_UP => "UP",
        VK_DOWN => "DOWN",
        VK_HOME => "HOME",
        VK_END => "END",
        VK_DELETE => "DEL",
        VK_INSERT => "INSERT",
        VK_PRIOR => "PAGEUP",
        VK_NEXT => "PAGEDOWN",
        VK_F1 => "F1",
        VK_F2 => "F2",
        VK_F3 => "F3",
        VK_F4 => "F4",
        VK_F5 => "F5",
        VK_F6 => "F6",
        VK_F7 => "F7",
        VK_F8 => "F8",
        VK_F9 => "F9",
        VK_F10 => "F10",
        VK_F11 => "F11",
        VK_F12 => "F12",
        _ => return None,
    };
    Some(Key::Special(format!("{prefix}{name}")))
}

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

fn show_error(message: &str) {
    let text = wide(message);
    let title = wide("RustVim GUI");
    unsafe {
        MessageBoxW(
            null_mut(),
            text.as_ptr(),
            title.as_ptr(),
            MB_OK | MB_ICONERROR,
        );
    }
}

unsafe fn release<T>(pointer: *mut T) {
    if !pointer.is_null() {
        (*(pointer as *mut IUnknown)).Release();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct InputWindow(Box<Gui>);
    impl InputWindow {
        fn new() -> Self {
            let mut gui = Box::new(match Gui::new(Session::new()) {
                Ok(gui) => gui,
                Err((_, error)) => panic!("{error}"),
            });
            let hwnd = unsafe {
                CreateWindowExW(
                    0,
                    wide("STATIC").as_ptr(),
                    wide("IME regression").as_ptr(),
                    WS_OVERLAPPEDWINDOW,
                    0,
                    0,
                    800,
                    400,
                    null_mut(),
                    null_mut(),
                    GetModuleHandleW(null()),
                    null_mut(),
                )
            };
            assert!(!hwnd.is_null());
            gui.hwnd = hwnd;
            unsafe {
                SetWindowLongPtrW(hwnd, GWLP_USERDATA, &mut *gui as *mut Gui as LONG_PTR);
            }
            Self(gui)
        }
        fn message(&mut self, msg: UINT, code: usize, flags: LPARAM) -> LRESULT {
            unsafe { wndproc(self.0.hwnd, msg, code, flags) }
        }
    }
    impl Drop for InputWindow {
        fn drop(&mut self) {
            unsafe {
                SetWindowLongPtrW(self.0.hwnd, GWLP_USERDATA, 0);
                DestroyWindow(self.0.hwnd);
            }
            self.0.hwnd = null_mut();
            self.0.cleanup();
        }
    }

    #[test]
    fn utf16_character_messages_preserve_cjk_and_supplementary_characters() {
        let mut pending = None;
        let text = "中A文𠀀😀";
        let decoded: String = text
            .encode_utf16()
            .filter_map(|unit| decode_char(&mut pending, u32::from(unit)))
            .collect();
        assert_eq!(decoded, text);
        assert_eq!(pending, None);
        assert_eq!(decode_char(&mut pending, 0xdc00), None);
        assert_eq!(decode_char(&mut pending, 0xd800), None);
        assert_eq!(decode_char(&mut pending, '文' as u32), Some('文'));
        assert_eq!(decode_char(&mut pending, 0xdc00), None);
    }

    #[test]
    fn ime_preedit_and_cancel_are_delegated_without_editing_the_buffer() {
        let mut window = InputWindow::new();
        window.0.feed(Key::Char('i'));
        for flags in [0, 0x0008, 0x0080, 0x0008 | 0x0080] {
            assert!(
                !window.0.on_ime_composition(flags),
                "preedit must reach native IME UI"
            );
        }
        assert_eq!(window.0.tui.session.editor.current_buffer().full_text(), "");
        let hwnd = window.0.hwnd;
        window.0.hwnd = null_mut();
        assert!(
            !window.0.on_ime_composition(GCS_RESULTSTR),
            "unreadable results need the default handler"
        );
        window.0.hwnd = hwnd;
        assert_eq!(window.0.tui.session.editor.mode, Mode::Insert);
    }

    #[test]
    fn committed_ime_text_is_visible_once_and_undoes_as_one_insert() {
        let mut window = InputWindow::new();
        window.0.feed(Key::Char('i'));
        window.0.commit_ime_text("中文𠀀😀");
        assert_eq!(
            window.0.tui.session.editor.current_buffer().full_text(),
            "中文𠀀😀"
        );
        let grid = window.0.tui.gui_grid(40, 12);
        for (col, ch) in [(0, '中'), (2, '文'), (4, '𠀀'), (6, '😀')] {
            assert_eq!(grid.cell(0, col).unwrap().ch, ch);
            assert!(grid.cell(0, col + 1).unwrap().continuation);
        }
        window.0.feed(Key::Special("ESC".into()));
        window.0.feed(Key::Char('u'));
        assert_eq!(window.0.tui.session.editor.current_buffer().full_text(), "");
    }

    #[test]
    fn ime_char_fallback_and_wm_char_accept_utf16_without_duplicate_insertion() {
        let mut window = InputWindow::new();
        window.0.feed(Key::Char('i'));
        for unit in "中文𠀀😀".encode_utf16() {
            window.message(WM_IME_CHAR, usize::from(unit), 1);
        }
        for unit in "A😀".encode_utf16() {
            window.message(WM_CHAR, usize::from(unit), 1);
        }
        assert_eq!(
            window.0.tui.session.editor.current_buffer().full_text(),
            "中文𠀀😀A😀"
        );
        assert_eq!(window.message(WM_UNICHAR, UNICODE_NOCHAR, 0), 1);
        window.message(WM_UNICHAR, '𠀀' as usize, 0);
        assert_eq!(
            window.0.tui.session.editor.current_buffer().full_text(),
            "中文𠀀😀A😀𠀀"
        );
    }

    #[test]
    fn ime_characters_are_visible_in_the_command_line_too() {
        let mut window = InputWindow::new();
        window.0.feed(Key::Char(':'));
        for unit in "中文".encode_utf16() {
            window.message(WM_IME_CHAR, usize::from(unit), 1);
        }
        assert!(
            window
                .0
                .tui
                .screen_text(40, 12)
                .lines()
                .last()
                .unwrap()
                .starts_with(":中文")
        );
        assert_eq!(window.0.tui.session.editor.current_buffer().full_text(), "");
    }

    #[test]
    fn focus_loss_clears_an_incomplete_utf16_character() {
        let mut window = InputWindow::new();
        window.0.feed(Key::Char('i'));
        window.message(WM_IME_CHAR, 0xd840, 1);
        window.message(WM_KILLFOCUS, 0, 0);
        window.message(WM_IME_CHAR, 0xdc00, 1);
        window.message(WM_IME_CHAR, '中' as usize, 1);
        assert_eq!(
            window.0.tui.session.editor.current_buffer().full_text(),
            "中"
        );
    }

    #[test]
    fn scrollbar_cursor_never_overrides_native_resize_or_titlebar_cursors() {
        for hit_test in [
            HTLEFT,
            HTRIGHT,
            HTTOP,
            HTBOTTOM,
            HTTOPLEFT,
            HTTOPRIGHT,
            HTBOTTOMLEFT,
            HTBOTTOMRIGHT,
            HTCAPTION,
            HTMINBUTTON,
            HTMAXBUTTON,
            HTCLOSE,
        ] {
            assert!(!scrollbar_owns_cursor(hit_test, true, false));
            assert!(!scrollbar_owns_cursor(hit_test, false, true));
        }
        assert!(scrollbar_owns_cursor(HTCLIENT, true, false));
        assert!(scrollbar_owns_cursor(HTCLIENT, false, true));
        assert!(!scrollbar_owns_cursor(HTCLIENT, false, false));
    }

    #[test]
    fn resizing_preserves_system_capture_but_releases_scrollbar_capture() {
        // A hidden native window exercises real capture ownership without
        // opening a visible GUI or entering the interactive sizing loop.
        let hwnd = unsafe {
            CreateWindowExW(
                0,
                wide("STATIC").as_ptr(),
                wide("capture regression").as_ptr(),
                WS_OVERLAPPEDWINDOW,
                0,
                0,
                400,
                300,
                null_mut(),
                null_mut(),
                GetModuleHandleW(null()),
                null_mut(),
            )
        };
        assert!(!hwnd.is_null());
        let mut gui = match Gui::new(Session::new()) {
            Ok(gui) => gui,
            Err((_, message)) => {
                unsafe {
                    DestroyWindow(hwnd);
                }
                panic!("{message}");
            }
        };
        gui.hwnd = hwnd;
        unsafe {
            SetCapture(hwnd);
        }
        let acquired = unsafe { GetCapture() == hwnd };
        gui.on_size(600, 400);
        let preserved = unsafe { GetCapture() == hwnd };
        gui.scroll_drag = Some(ScrollDrag {
            window: gui.tui.session.editor.windows.current_id(),
            grab_offset: 0.0,
        });
        unsafe {
            SetCapture(hwnd);
        }
        gui.cancel_scroll_drag();
        let released = unsafe { GetCapture() != hwnd };
        let drag_cleared = gui.scroll_drag.is_none();
        gui.cleanup();
        unsafe {
            ReleaseCapture();
            DestroyWindow(hwnd);
        }
        assert!(acquired, "test window must acquire mouse capture");
        assert!(
            preserved,
            "WM_SIZE must preserve capture owned by native window resizing"
        );
        assert!(
            released && drag_cleared,
            "cancelling a scrollbar drag must release its capture"
        );
    }

    #[test]
    fn ansi_palette_covers_cube_and_greyscale() {
        assert_eq!(ansi_rgb(0), (0, 0, 0));
        assert_eq!(ansi_rgb(9), (231, 72, 86));
        assert_eq!(ansi_rgb(16), (0, 0, 0));
        assert_eq!(ansi_rgb(196), (255, 0, 0));
        assert_eq!(ansi_rgb(232), (8, 8, 8));
        assert_eq!(ansi_rgb(255), (238, 238, 238));
    }

    #[test]
    fn reverse_swaps_foreground_and_background() {
        let style = Style {
            fg: Color::Rgb(1, 2, 3),
            bg: Color::Rgb(4, 5, 6),
            reverse: true,
            ..Style::NORMAL
        };
        assert_eq!(colors(style), ((4, 5, 6), (1, 2, 3)));
    }

    #[test]
    fn default_colors_use_the_gui_palette() {
        assert_eq!(colors(Style::NORMAL), (DEFAULT_FG, DEFAULT_BG));
    }

    #[test]
    fn cursor_shapes_follow_vim_modes() {
        assert!(matches!(cursor_shape(Mode::Normal), CursorShape::Block));
        assert!(matches!(cursor_shape(Mode::Insert), CursorShape::Vertical));
        assert!(matches!(
            cursor_shape(Mode::Replace),
            CursorShape::Horizontal(0.2)
        ));
        assert!(matches!(
            cursor_shape(Mode::OperatorPending),
            CursorShape::Horizontal(0.5)
        ));
    }

    #[test]
    fn font_specs_preserve_size_and_style() {
        assert_eq!(parse_font("").family, "Consolas");
        assert_eq!(parse_font("").points, 12.0);
        let font = parse_font("Cascadia_Mono:h18.5:W600:i");
        assert_eq!(font.family, "Cascadia Mono");
        assert_eq!(font.points, 18.5);
        assert_eq!(font.weight, 600);
        assert!(font.italic);
        assert_eq!(parse_font("Consolas:h4:b").weight, 700);
        assert_eq!(parse_font_with_default("MSGothic", 20.0).points, 20.0);
        assert_eq!(parse_font("Consolas:hNaN").points, 12.0);
    }

    #[test]
    fn maple_mono_light_preserves_the_halfwidth_to_fullwidth_ratio() {
        let mut gui = match Gui::new(Session::new()) {
            Ok(gui) => gui,
            Err((_, message)) => panic!("{message}"),
        };
        let canonical =
            make_formats(gui.dwrite, 1.0, &parse_font("Maple_Mono_NF_CN:h12:W300")).unwrap();
        let face = font_face_from_format(canonical[0]);
        if face.is_null() {
            for format in canonical {
                unsafe {
                    release(format);
                }
            }
            gui.cleanup();
            eprintln!("Maple Mono NF CN is not installed");
            return;
        }
        unsafe {
            release(face);
        }
        for format in canonical {
            unsafe {
                release(format);
            }
        }
        for family in ["Maple_Mono_NF_CN", "Maple_Mono_NF_CN_Light"] {
            gui.tui
                .session
                .ex(&format!("set guifont={family}:h12:W300"))
                .unwrap();
            gui.tui
                .session
                .ex(&format!("set guifontwide={family}:h12:W300"))
                .unwrap();
            for dpi in [96, 144] {
                gui.update_metrics_at(dpi);
                assert!(!gui.primary_face.is_null() && !gui.fallback_face.is_null());
                unsafe {
                    assert_eq!((*gui.formats[0]).GetFontWeight(), 300);
                    assert!(
                        ((*gui.formats[0]).GetFontSize() - 16.0 * dpi as f32 / 96.0).abs() < 0.01
                    );
                }
                for format in gui
                    .formats
                    .iter()
                    .chain(gui.fallback_formats.iter())
                    .copied()
                {
                    let cell = gui.cell_w;
                    let m = measure_text(gui.dwrite, format, "M").0;
                    let i = measure_text(gui.dwrite, format, "i").0;
                    let han = measure_text(gui.dwrite, format, "中").0;
                    eprintln!("{family} dpi={dpi}: cell={cell}, M={m}, i={i}, CJK={han}");
                    assert!(
                        (m - cell).abs() < 0.05
                            && (i - cell).abs() < 0.05
                            && (han - 2.0 * cell).abs() < 0.05,
                        "{family}: Latin glyphs must occupy half a CJK glyph's advance"
                    );
                }
            }
        }
        gui.cleanup();
    }

    #[test]
    fn dialog_font_spec_roundtrips_fractional_size_and_weight() {
        let spec = font_string("Microsoft YaHei", 14.5, 600, true);
        assert_eq!(spec, "Microsoft_YaHei:h14.5:W600:i");
        assert_eq!(
            parse_font(&spec),
            parse_font("Microsoft YaHei:h14.5:W600:i")
        );
        assert_eq!(
            font_string("Maple Mono NF CN Light", 12.0, 300, false),
            "Maple_Mono_NF_CN_Light:h12:W300"
        );
    }

    #[test]
    fn overlay_scrollbars_follow_panes_tabline_dpi_and_guioptions() {
        let mut session = Session::new();
        session.ex("tabnew").unwrap();
        let text = (0..100)
            .map(|line| format!("line {line}"))
            .collect::<Vec<_>>()
            .join("\n");
        session.editor.insert_text(Pos::new(0, 0), &text).unwrap();
        session.editor.set_cursor(Pos::new(0, 0));
        let mut gui = match Gui::new(session) {
            Ok(gui) => gui,
            Err((_, message)) => panic!("{message}"),
        };
        gui.client_w = 800;
        gui.client_h = 400;
        gui.cols = 80;
        gui.rows = 20;
        gui.cell_w = 10.0;
        gui.cell_h = 20.0;
        gui.tui.gui_grid(80, 20);
        gui.update_scrollbars();
        assert_eq!(gui.scrollbars.len(), 1);
        assert_eq!(gui.scrollbars[0].geometry.track.top, 20.0);
        assert_eq!(gui.scrollbars[0].geometry.track.right, 800.0);
        assert_eq!(gui.scrollbars[0].geometry.track.left, 786.0);
        let win = gui.tui.session.editor.windows.current();
        assert_eq!(
            gui.scrollbars[0].geometry.track.bottom,
            (win.rect.row + win.rect.text_rows() + 1) as f32 * 20.0
        );
        gui.scale = 2.0;
        gui.update_scrollbars();
        assert_eq!(gui.scrollbars[0].geometry.track.left, 772.0);
        gui.tui.session.ex("vsplit").unwrap();
        gui.tui.gui_grid(80, 20);
        gui.update_scrollbars();
        assert_eq!(gui.scrollbars.len(), 2);
        for bar in &gui.scrollbars {
            let win = gui.tui.session.editor.windows.get(bar.window).unwrap();
            assert!(bar.geometry.track.left >= win.rect.col as f32 * 10.0);
        }
        gui.tui.session.ex("set go=e").unwrap();
        gui.update_scrollbars();
        assert!(gui.scrollbars.is_empty());
        gui.tui.session.ex("set go+=R").unwrap();
        gui.update_scrollbars();
        assert_eq!(gui.scrollbars.len(), 2);
        gui.cleanup();
    }

    #[test]
    fn live_font_options_rebuild_directwrite_formats_and_grid() {
        let mut session = Session::new();
        session.ex("set guifont=Consolas:h12:W500").unwrap();
        session.ex("set guifontwide=Consolas:h18:W600:i").unwrap();
        let mut gui = match Gui::new(session) {
            Ok(gui) => gui,
            Err((_, message)) => panic!("{message}"),
        };
        gui.update_metrics_at(96);
        assert_eq!(gui.font_index('A'), 0);
        assert_eq!(gui.font_index('漢'), 1);
        assert_eq!(gui.font_index('😀'), 1);
        let first_width = gui.cell_w;
        unsafe {
            assert_eq!((*gui.formats[0]).GetFontSize(), 16.0);
            assert_eq!((*gui.formats[0]).GetFontWeight(), 500);
            assert_eq!((*gui.fallback_formats[0]).GetFontSize(), 24.0);
            assert_eq!((*gui.fallback_formats[0]).GetFontWeight(), 600);
            assert_eq!(
                (*gui.fallback_formats[0]).GetFontStyle(),
                DWRITE_FONT_STYLE_ITALIC
            );
        }
        let first_height = gui.cell_h;
        gui.tui
            .session
            .ex("set guifontwide=Consolas:h30:W800")
            .unwrap();
        gui.update_metrics_at(96);
        assert!(gui.cell_h > first_height);
        assert_eq!(gui.cell_w, first_width);
        unsafe {
            assert_eq!((*gui.fallback_formats[0]).GetFontSize(), 40.0);
            assert_eq!((*gui.fallback_formats[0]).GetFontWeight(), 800);
        }
        gui.tui.session.ex("set guifont=Consolas:h16:b").unwrap();
        gui.update_metrics_at(144);
        assert!(gui.cell_w > first_width);
        unsafe {
            assert_eq!((*gui.formats[0]).GetFontSize(), 32.0);
            assert_eq!((*gui.formats[0]).GetFontWeight(), 700);
            // Explicit wide point size survives primary font and DPI changes.
            assert_eq!((*gui.fallback_formats[0]).GetFontSize(), 60.0);
        }
        gui.tui.session.ex("set guifontwide=Consolas").unwrap();
        gui.update_metrics_at(96);
        unsafe {
            assert!(((*gui.fallback_formats[0]).GetFontSize() - 16.0 * 96.0 / 72.0).abs() < 0.001);
        }
        gui.cleanup();
    }

    #[test]
    fn wide_glyphs_fit_two_cells_without_changing_height() {
        assert_eq!(wide_placement(16.0, 20.0), (1.0, 2.0));
        assert_eq!(wide_placement(40.0, 20.0), (0.5, 0.0));
        assert_eq!(wide_placement(20.0, 20.0), (1.0, 0.0));
        assert_eq!(wide_placement(0.0, 20.0), (1.0, 0.0));
    }

    #[test]
    fn wide_character_ranges_are_recognised() {
        assert!(is_wide_char('漢'));
        assert!(is_wide_char('한'));
        assert!(!is_wide_char('A'));
        assert_eq!(rvim_term::char_width('😀'), 2);
    }
}
