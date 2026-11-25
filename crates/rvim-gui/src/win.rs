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

use rvim_app::Session;
use rvim_app::tui::Tui;
use rvim_core::Pos;
use rvim_editor::maps::Key;
use rvim_editor::{Mode, RegType, Register};
use rvim_normal::operators::{OpRange, yank_range};
use rvim_syntax::highlight::HlColor;
use rvim_term::{Color, Style};

use winapi::shared::basetsd::LONG_PTR;
use winapi::shared::dxgiformat::DXGI_FORMAT_B8G8R8A8_UNORM;
use winapi::shared::guiddef::GUID;
use winapi::shared::minwindef::{BOOL, DWORD, HGLOBAL, LPARAM, LRESULT, UINT, WPARAM};
use winapi::shared::ntdef::HANDLE;
use winapi::shared::windef::{HBRUSH, HWND, POINT};
use winapi::shared::winerror::D2DERR_RECREATE_TARGET;
use winapi::um::combaseapi::{CoInitializeEx, CoUninitialize};
use winapi::um::commdlg::{
    CF_EFFECTS, CF_INITTOLOGFONTSTRUCT, CF_SCREENFONTS, CHOOSEFONTW, ChooseFontW,
};
use winapi::um::d2d1::{
    D2D1_COLOR_F, D2D1_DRAW_TEXT_OPTIONS_CLIP, D2D1_DRAW_TEXT_OPTIONS_ENABLE_COLOR_FONT,
    D2D1_FACTORY_TYPE_SINGLE_THREADED, D2D1_FEATURE_LEVEL_DEFAULT,
    D2D1_HWND_RENDER_TARGET_PROPERTIES, D2D1_POINT_2F, D2D1_PRESENT_OPTIONS_NONE, D2D1_RECT_F,
    D2D1_RENDER_TARGET_PROPERTIES, D2D1_RENDER_TARGET_TYPE_DEFAULT, D2D1_RENDER_TARGET_USAGE_NONE,
    D2D1_SIZE_U, D2D1_TEXT_ANTIALIAS_MODE_CLEARTYPE, D2D1CreateFactory, ID2D1Brush, ID2D1Factory,
    ID2D1HwndRenderTarget, ID2D1SolidColorBrush,
};
use winapi::um::dcommon::{D2D1_ALPHA_MODE_IGNORE, D2D1_PIXEL_FORMAT};
use winapi::um::dwrite::{
    DWRITE_FACTORY_TYPE_SHARED, DWRITE_FONT_STRETCH_NORMAL, DWRITE_FONT_STYLE_ITALIC,
    DWRITE_FONT_STYLE_NORMAL, DWRITE_FONT_WEIGHT_BOLD, DWRITE_FONT_WEIGHT_NORMAL,
    DWRITE_PARAGRAPH_ALIGNMENT_NEAR, DWRITE_TEXT_ALIGNMENT_LEADING, DWRITE_TEXT_METRICS,
    DWRITE_WORD_WRAPPING_NO_WRAP, DWriteCreateFactory, IDWriteFactory, IDWriteTextFormat,
    IDWriteTextLayout,
};
use winapi::um::imm::{
    CFS_POINT, COMPOSITIONFORM, HIMC, ImmGetContext, ImmReleaseContext, ImmSetCompositionWindow,
};
use winapi::um::libloaderapi::GetModuleHandleW;
use winapi::um::objbase::COINIT_APARTMENTTHREADED;
use winapi::um::unknwnbase::IUnknown;
use winapi::um::winbase::{GMEM_MOVEABLE, GlobalAlloc, GlobalFree, GlobalLock, GlobalUnlock};
use winapi::um::wingdi::{FW_NORMAL, LOGFONTW};
use winapi::um::winuser::*;

static DIALOG_OWNER: AtomicIsize = AtomicIsize::new(0);

const GCS_COMPSTR: DWORD = 0x0008;
const GCS_RESULTSTR: DWORD = 0x0800;
const ISC_SHOWUICOMPOSITIONWINDOW: isize = 0x8000_0000u32 as isize;

#[link(name = "imm32")]
unsafe extern "system" {
    fn ImmGetCompositionStringW(himc: HIMC, index: DWORD, buf: *mut c_void, len: DWORD) -> i32;
    fn ImmSetCompositionFontW(himc: HIMC, lf: *const LOGFONTW) -> BOOL;
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

struct Gui {
    hwnd: HWND,
    tui: Tui,
    d2d: *mut ID2D1Factory,
    dwrite: *mut IDWriteFactory,
    rt: *mut ID2D1HwndRenderTarget,
    formats: Vec<*mut IDWriteTextFormat>,
    wide_formats: Vec<*mut IDWriteTextFormat>,
    emoji_formats: Vec<*mut IDWriteTextFormat>,
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
    ime_text: String,
    ime_active: bool,
    cursor_px: (f32, f32),
    last_clipboard_seq: u32,
    last_clipboard_text: String,
    blink_on: bool,
    blink_initial: bool,
    last_blink: Instant,
    com_initialized: bool,
}

impl Gui {
    #[allow(clippy::result_large_err)]
    fn new(session: Session) -> Result<Self, (Session, String)> {
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

        let emoji_formats =
            make_formats(dwrite, 1.0, "Segoe UI Emoji", BASE_FONT_POINTS).unwrap_or_default();
        let formats = match make_formats(dwrite, 1.0, DEFAULT_FONT, BASE_FONT_POINTS) {
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

        Ok(Self {
            hwnd: null_mut(),
            tui: Tui::new(session),
            d2d,
            dwrite,
            rt: null_mut(),
            formats,
            emoji_formats,
            wide_formats: Vec::new(),
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
            ime_text: String::new(),
            ime_active: false,
            cursor_px: (0.0, 0.0),
            last_clipboard_seq: 0,
            last_clipboard_text: String::new(),
            blink_on: true,
            blink_initial: true,
            last_blink: Instant::now(),
            com_initialized: true,
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
            for format in self.wide_formats.drain(..) {
                release(format);
            }
            for format in self.emoji_formats.drain(..) {
                release(format);
            }
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
        self.client_w = width;
        self.client_h = height;
        self.update_metrics();
        self.resize_target();
    }

    fn update_metrics(&mut self) {
        let dpi = if self.hwnd.is_null() {
            96
        } else {
            unsafe { GetDpiForWindow(self.hwnd) }.max(96)
        };
        let scale = dpi as f32 / 96.0;
        let font_spec = self.tui.session.editor.options.str_or("guifont", "");
        let wide_spec = self.tui.session.editor.options.str_or("guifontwide", "");
        if (scale - self.scale).abs() > 0.01
            || font_spec != self.font_spec
            || wide_spec != self.wide_spec
        {
            let (family, points) = parse_font(&font_spec);
            if let Ok(formats) = make_formats(self.dwrite, scale, &family, points) {
                let wide_formats = if wide_spec.is_empty() {
                    Vec::new()
                } else {
                    let (wide_family, wide_points) = parse_font_with_default(&wide_spec, points);
                    make_formats(self.dwrite, scale, &wide_family, wide_points).unwrap_or_default()
                };
                let (_, emoji_points) = parse_font(&font_spec);
                let emoji_formats =
                    make_formats(self.dwrite, scale, "Segoe UI Emoji", emoji_points)
                        .unwrap_or_default();
                unsafe {
                    for format in self.formats.drain(..) {
                        release(format);
                    }
                    for format in self.wide_formats.drain(..) {
                        release(format);
                    }
                    for format in self.emoji_formats.drain(..) {
                        release(format);
                    }
                }
                self.formats = formats;
                self.wide_formats = wide_formats;
                self.emoji_formats = emoji_formats;
                self.scale = scale;
                self.font_spec = font_spec;
                self.wide_spec = wide_spec;
            }
        }

        let normal_metrics = if let Some(format) = self.formats.first().copied() {
            measure_cell(self.dwrite, format, "M")
        } else {
            (9.0 * self.scale, 18.0 * self.scale)
        };
        let wide_metrics = self
            .wide_formats
            .first()
            .copied()
            .map(|format| measure_cell(self.dwrite, format, "漢"));
        let (cell_w, cell_h) = cell_metrics(normal_metrics, wide_metrics);
        self.cell_w = cell_w.max(1.0);
        self.cell_h = cell_h.max(1.0);
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
        let guioptions = self.tui.session.editor.options.str_or("guioptions", "");
        self.tui.set_show_tabline(guioptions.contains('e'));
        let grid = self.tui.grid(self.cols, self.rows);
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
                let start = col;
                let mut text = String::new();
                let mut span = 0usize;
                let wide = is_wide_char(cell.ch);
                let emoji = is_emoji(cell.ch);
                while col < self.cols {
                    let Some(next) = grid.cell(row, col) else {
                        break;
                    };
                    if next.continuation {
                        col += 1;
                        continue;
                    }
                    if next.style != style
                        || (!text.is_empty()
                            && (is_wide_char(next.ch) != wide || is_emoji(next.ch) != emoji))
                    {
                        break;
                    }
                    text.push(next.ch);
                    let width = rvim_term::char_width(next.ch).max(1);
                    span += width;
                    col += width;
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
                    self.draw_text(&text, x, y, fg, style);
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
            if self.focused && self.blink_on && row < self.rows && col < self.cols {
                let cell = grid.cell(row, col).map(|cell| (cell.ch, cell.style));
                self.draw_cursor(row, col, cell);
            }
        }
        if self.ime_active && !self.ime_text.is_empty() {
            let style = Style {
                underline: true,
                ..Style::NORMAL
            };
            let text = self.ime_text.clone();
            self.draw_text(
                &text,
                self.cursor_px.0,
                self.cursor_px.1,
                (255, 210, 80),
                style,
            );
        }

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

    fn draw_text(&mut self, text: &str, x: f32, y: f32, fg: (u8, u8, u8), style: Style) {
        let index = (style.bold as usize) | ((style.italic as usize) << 1);
        let formats =
            if text.chars().next().is_some_and(is_wide_char) && !self.wide_formats.is_empty() {
                &self.wide_formats
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
        let brush = self.brush(fg);
        if !brush.is_null() {
            unsafe {
                (*self.rt).DrawTextLayout(
                    D2D1_POINT_2F { x, y },
                    layout,
                    brush as *mut ID2D1Brush,
                    D2D1_DRAW_TEXT_OPTIONS_CLIP | D2D1_DRAW_TEXT_OPTIONS_ENABLE_COLOR_FONT,
                );
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
                        self.draw_text(&ch.to_string(), x, y, fg, style);
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

    fn composition_string(himc: HIMC, flag: DWORD) -> String {
        let bytes = unsafe { ImmGetCompositionStringW(himc, flag, null_mut(), 0) };
        if bytes <= 0 {
            return String::new();
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
        if got <= 0 {
            return String::new();
        }
        String::from_utf16_lossy(&buffer[..got as usize / 2])
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
            let font = self.ime_logfont();
            ImmSetCompositionFontW(himc, &font);
            ImmReleaseContext(self.hwnd, himc);
        }
    }

    fn ime_logfont(&self) -> LOGFONTW {
        let (family, points) = parse_font(&self.font_spec);
        let mut lf: LOGFONTW = unsafe { zeroed() };
        lf.lfHeight = -((points * (96.0 / 72.0) * self.scale).round() as i32);
        lf.lfWeight = FW_NORMAL;
        for (slot, ch) in lf.lfFaceName.iter_mut().zip(family.encode_utf16().take(31)) {
            *slot = ch;
        }
        lf
    }

    fn on_ime_start(&mut self) {
        self.ime_active = true;
        self.ime_text.clear();
        self.update_ime_window();
        unsafe { InvalidateRect(self.hwnd, null(), 0) };
    }

    fn on_ime_composition(&mut self, flags: DWORD) {
        if self.hwnd.is_null() {
            return;
        }
        unsafe {
            let himc = ImmGetContext(self.hwnd);
            if himc.is_null() {
                return;
            }
            if flags & GCS_RESULTSTR != 0 {
                let text = Self::composition_string(himc, GCS_RESULTSTR);
                for ch in text.chars() {
                    self.feed(Key::Char(ch));
                }
                self.ime_text.clear();
                self.ime_active = false;
            }
            if flags & GCS_COMPSTR != 0 {
                self.ime_text = Self::composition_string(himc, GCS_COMPSTR);
            }
            ImmReleaseContext(self.hwnd, himc);
        }
        unsafe { InvalidateRect(self.hwnd, null(), 0) };
    }

    fn on_ime_end(&mut self) {
        self.ime_active = false;
        self.ime_text.clear();
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
            return;
        }
        if let Some(ch) = char::from_u32(code) {
            if ch >= ' ' && ch != '\u{7f}' {
                self.feed(Key::Char(ch));
            }
        }
    }

    fn on_wheel(&mut self, delta: i16) {
        let key = if delta > 0 { "C-Y" } else { "C-E" };
        let count = (delta.unsigned_abs() / 120).clamp(1, 5);
        for _ in 0..count {
            self.feed(Key::Special(key.to_string()));
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
        WM_IME_SETCONTEXT => {
            let mut context = lparam;
            if wparam != 0 {
                context &= !ISC_SHOWUICOMPOSITIONWINDOW;
            }
            DefWindowProcW(hwnd, msg, wparam, context)
        }
        WM_IME_STARTCOMPOSITION => {
            (*gui).on_ime_start();
            0
        }
        WM_IME_COMPOSITION => {
            (*gui).on_ime_composition(lparam as DWORD);
            0
        }
        WM_IME_ENDCOMPOSITION => {
            (*gui).on_ime_end();
            0
        }
        WM_MOUSEWHEEL => {
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
    family: &str,
    points: f32,
) -> Result<Vec<*mut IDWriteTextFormat>, String> {
    let font = wide(family);
    let locale = wide("en-us");
    let size = points * (96.0 / 72.0) * scale;
    let mut formats = Vec::with_capacity(4);
    for (weight, style) in [
        (DWRITE_FONT_WEIGHT_NORMAL, DWRITE_FONT_STYLE_NORMAL),
        (DWRITE_FONT_WEIGHT_BOLD, DWRITE_FONT_STYLE_NORMAL),
        (DWRITE_FONT_WEIGHT_NORMAL, DWRITE_FONT_STYLE_ITALIC),
        (DWRITE_FONT_WEIGHT_BOLD, DWRITE_FONT_STYLE_ITALIC),
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

fn cell_metrics(normal: (f32, f32), wide: Option<(f32, f32)>) -> (f32, f32) {
    let mut width = normal.0;
    let mut height = normal.1;
    if let Some((wide_width, wide_height)) = wide {
        width = width.max(wide_width / 2.0);
        height = height.max(wide_height);
    }
    (width, height)
}

fn measure_cell(
    dwrite: *mut IDWriteFactory,
    format: *mut IDWriteTextFormat,
    sample_text: &str,
) -> (f32, f32) {
    let sample = wide(sample_text);
    let mut layout: *mut IDWriteTextLayout = null_mut();
    let hr =
        unsafe { (*dwrite).CreateTextLayout(sample.as_ptr(), 1, format, 64.0, 64.0, &mut layout) };
    if hr < 0 || layout.is_null() {
        return (9.0, 18.0);
    }
    let mut metrics: DWRITE_TEXT_METRICS = unsafe { zeroed() };
    unsafe {
        (*layout).GetMetrics(&mut metrics);
        release(layout);
    }
    (metrics.width.max(1.0), (metrics.height * 1.05).max(1.0))
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

fn is_emoji(ch: char) -> bool {
    matches!(
        ch as u32,
        0x1f000..=0x1faff | 0x2600..=0x27bf | 0x2300..=0x23ff | 0x2b00..=0x2bff | 0xfe00..=0xfe0f
    )
}

fn parse_font(spec: &str) -> (String, f32) {
    parse_font_with_default(spec, BASE_FONT_POINTS)
}

fn parse_font_with_default(spec: &str, default_points: f32) -> (String, f32) {
    let mut family = DEFAULT_FONT.to_string();
    let mut points = default_points;
    for (index, part) in spec.split(':').enumerate() {
        if index == 0 {
            if !part.is_empty() && part != "*" {
                family = part.to_string();
            }
        } else if let Some(height) = part.strip_prefix('h') {
            if let Ok(value) = height.parse::<f32>() {
                points = value.clamp(4.0, 200.0);
            }
        }
    }
    (family, points)
}

fn choose_font(current: &str, _wide: bool) -> Option<String> {
    let (family, points) = parse_font(current);
    let mut lf: LOGFONTW = unsafe { zeroed() };
    lf.lfHeight = -((points * 96.0 / 72.0).round() as i32);
    lf.lfWeight = FW_NORMAL;
    let face: Vec<u16> = family.encode_utf16().take(31).collect();
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
    Some(format!("{}:h{}", face, chosen_points.round() as i32))
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
    fn font_specs_parse_vim_height_suffixes() {
        assert_eq!(parse_font(""), ("Consolas".to_string(), 12.0));
        assert_eq!(
            parse_font("Cascadia Mono:h18"),
            ("Cascadia Mono".to_string(), 18.0)
        );
        assert_eq!(parse_font("Consolas:h4"), ("Consolas".to_string(), 4.0));
        assert_eq!(
            parse_font_with_default("MSGothic", 20.0),
            ("MSGothic".to_string(), 20.0)
        );
    }

    #[test]
    fn cell_metrics_follow_the_wide_font_size() {
        assert_eq!(cell_metrics((9.0, 18.0), Some((18.0, 18.0))), (9.0, 18.0));
        assert_eq!(cell_metrics((9.0, 18.0), Some((22.0, 26.0))), (11.0, 26.0));
        assert_eq!(cell_metrics((9.0, 18.0), None), (9.0, 18.0));
    }

    #[test]
    fn wide_character_ranges_are_recognised() {
        assert!(is_wide_char('漢'));
        assert!(is_wide_char('한'));
        assert!(!is_wide_char('A'));
        assert!(is_emoji('😀'));
        assert!(is_emoji('🚀'));
        assert!(!is_emoji('A'));
        assert_eq!(rvim_term::char_width('😀'), 2);
    }
}
