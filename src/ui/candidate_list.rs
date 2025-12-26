use std::
    mem::{ManuallyDrop, size_of}
;

use csscolorparser::Color;
use log::{debug, error, trace};
use windows::{
    core::{PCSTR, Result, s, w},
    Win32::{
        Foundation::{BOOL, GetLastError, HWND, LPARAM, LRESULT, RECT, SIZE, WPARAM},
        Graphics::{
            Direct2D::{
                Common::{D2D1_ALPHA_MODE_PREMULTIPLIED, D2D1_PIXEL_FORMAT, D2D_RECT_F},
                D2D1CreateFactory, ID2D1Factory, ID2D1HwndRenderTarget,
                ID2D1SolidColorBrush, D2D1_FACTORY_TYPE_SINGLE_THREADED,
                D2D1_HWND_RENDER_TARGET_PROPERTIES, D2D1_PRESENT_OPTIONS_NONE,
                D2D1_RENDER_TARGET_PROPERTIES, D2D1_RENDER_TARGET_TYPE_DEFAULT,
                D2D1_DRAW_TEXT_OPTIONS_ENABLE_COLOR_FONT,
            },
            DirectWrite::{
                DWriteCreateFactory, IDWriteFactory, IDWriteTextFormat, IDWriteTextLayout,
                DWRITE_FACTORY_TYPE_SHARED, DWRITE_FONT_STRETCH_NORMAL, DWRITE_FONT_STYLE_NORMAL,
                DWRITE_FONT_WEIGHT_NORMAL, DWRITE_PARAGRAPH_ALIGNMENT_CENTER,
                DWRITE_TEXT_ALIGNMENT_LEADING, DWRITE_MEASURING_MODE_NATURAL,
                DWRITE_TEXT_METRICS,
            },
            Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM,
            Gdi::{
                self, BeginPaint, EndPaint, GetDC, GetDeviceCaps,
                HDC, InvalidateRect, LOGPIXELSY,
                PAINTSTRUCT, ReleaseDC,
            },
        },
        UI::WindowsAndMessaging::{
            CS_DROPSHADOW, CS_HREDRAW, CS_IME, CS_VREDRAW, CreateWindowExA, DefWindowProcA,
            DestroyWindow, GetClientRect, GetWindowLongPtrA, HICON, HWND_TOPMOST, IDC_ARROW,
            LoadCursorW, RegisterClassExA, SW_HIDE, SW_SHOWNOACTIVATE, SWP_NOACTIVATE, SWP_NOMOVE,
            SWP_NOSIZE, SetWindowLongPtrA, SetWindowPos, ShowWindow, WINDOW_LONG_PTR_INDEX,
            WM_PAINT, WNDCLASSEXA, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST,
            WS_POPUP,
        },
    },
};

use crate::{
    CANDI_INDEX_SUFFIX, CANDI_INDEX_SUFFIX_MONO, CANDI_INDEXES,
    conf::{self},
    extend::{ColorExt, OsStrExt2},
    global::{self, CANDI_NUM},
};

const WINDOW_CLASS: PCSTR = s!("CANDIDATE_LIST");
// Layout
const CLIP_WIDTH: i32 = 3;
const LABEL_PADDING_TOP: i32 = 4;
const LABEL_PADDING_BOTTOM: i32 = 4;
const LABEL_PADDING_LEFT: i32 = 5;
const LABEL_PADDING_RIGHT: i32 = 6;
const INDEX_CANDI_GAP: i32 = 6;
const BORDER_WIDTH: i32 = 0;

const POS_OFFSETX: i32 = 2;
const POS_OFFSETY: i32 = 2;

#[cfg(target_pointer_width = "64")]
type LongPointer = isize;
#[cfg(target_pointer_width = "32")]
type LongPointer = i32;

// Thread-local storage for Direct2D/DirectWrite factories
thread_local! {
    static D2D_FACTORY: ID2D1Factory = unsafe {
        D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None).unwrap()
    };
    static DW_FACTORY: IDWriteFactory = unsafe {
        DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED).unwrap()
    };
}

/// To create a window you need to register the window class beforehand.
pub fn setup() -> Result<()> {
    let wcex = WNDCLASSEXA {
        cbSize: size_of::<WNDCLASSEXA>() as u32,
        style: CS_IME | CS_HREDRAW | CS_VREDRAW | CS_DROPSHADOW,
        lpfnWndProc: Some(wind_proc),
        cbClsExtra: 0,
        cbWndExtra: size_of::<Box<PaintArg>>().try_into().unwrap(),
        hInstance: global::dll_module(),
        hIcon: HICON::default(),
        hCursor: unsafe { LoadCursorW(None, IDC_ARROW)? },
        hbrBackground: unsafe { Color::from_linear_rgba8(0, 0, 0, 0).to_hbrush() },
        lpszMenuName: PCSTR::null(),
        lpszClassName: WINDOW_CLASS,
        hIconSm: HICON::default(),
    };
    unsafe {
        if RegisterClassExA(&wcex) == 0 {
            error!("Failed to register window class for candidate list");
            return Err(GetLastError().into());
        }
        debug!("Registered window class for candidate list.");
    }
    Ok(())
}

/// use default handlers for everything but repaint
unsafe extern "system" fn wind_proc(
    window: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_PAINT => paint(window),
        _ => unsafe { DefWindowProcA(window, msg, wparam, lparam) },
    }
}

//----------------------------------------------------------------------------
//
//  Helper function to measure text with DirectWrite
//
//----------------------------------------------------------------------------

fn measure_text_dwrite(
    factory: &IDWriteFactory,
    text: &str,
    format: &IDWriteTextFormat,
) -> (f32, f32) {
    unsafe {
        let text_wide: Vec<u16> = text.encode_utf16().collect();
        let layout: std::result::Result<IDWriteTextLayout, _> = factory.CreateTextLayout(
            &text_wide,
            format,
            10000.0, // max width
            10000.0, // max height
        );

        if let Ok(layout) = layout {
            let mut metrics = DWRITE_TEXT_METRICS::default();
            if layout.GetMetrics(&mut metrics).is_ok() {
                return (metrics.width, metrics.height);
            }
        }
        (0.0, 0.0)
    }
}

//----------------------------------------------------------------------------
//
//  The implementation
//
//----------------------------------------------------------------------------

#[derive(Default)]
pub struct CandidateList {
    window: HWND,
    index_suffix: &'static str,
    font_size: f32,
    index_font_size: f32,
    dpi_scale: f32,
}

impl CandidateList {
    pub fn create(_parent_window: HWND) -> Result<CandidateList> {
        // WS_EX_TOOLWINDOW:  A floating toolbar that won't appear in taskbar and ALT+TAB.
        // WS_EX_NOACTIVATE:  A window that doesn't take the foreground thus not making parent window lose focus.
        // WS_EX_TOPMOST:     A window that is topmost.
        // WS_POPUP:          A window having no top bar or border.
        // see: https://learn.microsoft.com/en-us/windows/win32/winmsg/extended-window-styles
        unsafe {
            let conf = conf::get();
            let window = CreateWindowExA(
                WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE | WS_EX_TOPMOST,
                WINDOW_CLASS,
                PCSTR::null(),
                WS_POPUP,
                0,
                0,
                0,
                0,
                None,
                None,
                global::dll_module(),
                None,
            );
            if window.0 == 0 {
                error!("CreateWindowExA returned null.");
                return Err(GetLastError().into());
            }
            let dc: HDC = GetDC(window);
            let pixel_per_inch = GetDeviceCaps(dc, LOGPIXELSY);
            let dpi_scale = pixel_per_inch as f32 / 96.0;
            
            // DirectWrite uses DIPs (device independent pixels), convert from points
            let font_size = conf.font.size as f32 * dpi_scale;
            let index_font_size = font_size * 0.7;

            let font_name_lower = conf.font.name.to_ascii_lowercase();
            let index_suffix =
                if font_name_lower.contains("mono") || font_name_lower.contains("fairfax") {
                    CANDI_INDEX_SUFFIX_MONO
                } else {
                    CANDI_INDEX_SUFFIX
                };
            ReleaseDC(window, dc);
            Ok(CandidateList {
                window,
                index_suffix,
                font_size,
                index_font_size,
                dpi_scale,
            })
        }
    }

    pub fn locate(&self, x: i32, y: i32) -> Result<()> {
        trace!("locate({x}, {y})");
        unsafe {
            SetWindowPos(
                self.window,
                HWND_TOPMOST,
                x + POS_OFFSETX,
                y + POS_OFFSETY,
                0,
                0,
                SWP_NOACTIVATE | SWP_NOSIZE,
            )?
        };
        Ok(())
    }

    pub fn show(&self, suggs: &[String]) -> Result<()> {
        unsafe {
            let conf = conf::get();
            
            // Create DirectWrite text formats for measurement
            let (candi_format, index_format) = DW_FACTORY.with(|factory| {
                let font_name_wide: Vec<u16> = conf.font.name
                    .encode_utf16()
                    .chain(std::iter::once(0))
                    .collect();

                let candi_format = factory.CreateTextFormat(
                    windows::core::PCWSTR(font_name_wide.as_ptr()),
                    None,
                    DWRITE_FONT_WEIGHT_NORMAL,
                    DWRITE_FONT_STYLE_NORMAL,
                    DWRITE_FONT_STRETCH_NORMAL,
                    self.font_size,
                    w!("en-us"),
                ).ok();

                let index_format = factory.CreateTextFormat(
                    windows::core::PCWSTR(font_name_wide.as_ptr()),
                    None,
                    DWRITE_FONT_WEIGHT_NORMAL,
                    DWRITE_FONT_STYLE_NORMAL,
                    DWRITE_FONT_STRETCH_NORMAL,
                    self.index_font_size,
                    w!("en-us"),
                ).ok();

                (candi_format, index_format)
            });

            let Some(candi_format) = candi_format else {
                error!("Failed to create candidate text format");
                return Ok(());
            };
            let Some(index_format) = index_format else {
                error!("Failed to create index text format");
                return Ok(());
            };

            let mut indice_str = Vec::with_capacity(suggs.len());
            let mut candis_str = Vec::with_capacity(suggs.len());

            let mut candi_height: f32 = 0.0;
            let mut index_height: f32 = 0.0;
            let mut index_width: f32 = 0.0;
            let mut candi_widths: Vec<f32> = Vec::with_capacity(suggs.len());

            // Measure text using DirectWrite
            DW_FACTORY.with(|factory| {
                for (index, sugg) in suggs.iter().take(CANDI_NUM).enumerate() {
                    let index_str = format!("{}{}", CANDI_INDEXES[index], self.index_suffix);
                    let (w, h) = measure_text_dwrite(factory, &index_str, &index_format);
                    index_height = index_height.max(h);
                    index_width = index_width.max(w);
                    indice_str.push(index_str);

                    let (w, h) = measure_text_dwrite(factory, sugg, &candi_format);
                    candi_height = candi_height.max(h);
                    candi_widths.push(w);
                    candis_str.push(sugg.clone());
                }
            });

            let row_height = candi_height.max(index_height);
            let label_height = LABEL_PADDING_TOP as f32 + row_height + LABEL_PADDING_BOTTOM as f32;
            
            let mut wnd_height: f32 = 0.0;
            let mut wnd_width: f32 = 0.0;
            
            if conf.layout.vertical {
                let candi_num = suggs.len().min(CANDI_NUM) as f32;
                wnd_height += candi_num * label_height;
                let max_candi_width = candi_widths.iter().cloned().fold(0.0f32, f32::max);
                wnd_width += CLIP_WIDTH as f32
                    + LABEL_PADDING_LEFT as f32
                    + index_width
                    + INDEX_CANDI_GAP as f32
                    + max_candi_width
                    + LABEL_PADDING_RIGHT as f32;
                wnd_width = wnd_width.max(wnd_height * 4.0 / 5.0);
            } else {
                wnd_height += label_height;
                wnd_width += CLIP_WIDTH as f32;
                for candi_width in candi_widths.iter() {
                    wnd_width += LABEL_PADDING_LEFT as f32 + LABEL_PADDING_RIGHT as f32;
                    wnd_width += index_width;
                    wnd_width += INDEX_CANDI_GAP as f32;
                    wnd_width += candi_width;
                }
            }
            wnd_height += (BORDER_WIDTH * 2) as f32;
            wnd_width += (BORDER_WIDTH * 2) as f32;

            let highlight_width = if conf.layout.vertical {
                wnd_width - CLIP_WIDTH as f32 - (BORDER_WIDTH * 2) as f32
            } else {
                LABEL_PADDING_LEFT as f32 + index_width + INDEX_CANDI_GAP as f32 + candi_widths[0] + LABEL_PADDING_RIGHT as f32
            };

            let arg = PaintArg {
                wnd_width,
                wnd_height,
                highlight_width,
                label_height,
                row_height,
                index_width,
                index_height,
                candi_widths,
                candi_height,
                candis: candis_str,
                indice: indice_str,
                font_size: self.font_size,
                index_font_size: self.index_font_size,
                font_name: conf.font.name.clone(),
            };
            let long_ptr = arg.into_long_ptr();
            SetWindowLongPtrA(self.window, WINDOW_LONG_PTR_INDEX::default(), long_ptr);
            SetWindowPos(
                self.window,
                HWND_TOPMOST,
                0,
                0,
                wnd_width.ceil() as i32,
                wnd_height.ceil() as i32,
                SWP_NOACTIVATE | SWP_NOMOVE,
            )?;
            ShowWindow(self.window, SW_SHOWNOACTIVATE);
            InvalidateRect(self.window, None, BOOL::from(true));
        };
        Ok(())
    }

    pub fn hide(&self) {
        unsafe {
            ShowWindow(self.window, SW_HIDE);
        }
    }

    pub fn destroy(&self) -> Result<()> {
        unsafe { DestroyWindow(self.window) }
    }
}

struct PaintArg {
    wnd_width: f32,
    wnd_height: f32,
    highlight_width: f32,
    label_height: f32,
    row_height: f32,
    index_width: f32,
    index_height: f32,
    candi_widths: Vec<f32>,
    candi_height: f32,
    indice: Vec<String>,
    candis: Vec<String>,
    font_size: f32,
    index_font_size: f32,
    font_name: String,
}

impl PaintArg {
    fn into_long_ptr(self) -> LongPointer {
        ManuallyDrop::new(Box::new(self)).as_ref() as *const PaintArg as LongPointer
    }

    unsafe fn from_long_ptr(long_ptr: LongPointer) -> Option<Box<PaintArg>> {
        if long_ptr == 0 {
            None
        } else {
            Some(unsafe { Box::from_raw(long_ptr as *mut PaintArg) })
        }
    }
}

fn paint(window: HWND) -> LRESULT {
    let conf = conf::get();
    let arg = unsafe {
        PaintArg::from_long_ptr(GetWindowLongPtrA(window, WINDOW_LONG_PTR_INDEX::default()))
    };
    let Some(arg) = arg else {
        error!("Args for repaint is not found.");
        return LRESULT::default();
    };
    unsafe { SetWindowLongPtrA(window, WINDOW_LONG_PTR_INDEX::default(), 0) };

    let mut ps = PAINTSTRUCT::default();
    let _dc: HDC = unsafe { BeginPaint(window, &mut ps) };

    // Create Direct2D render target
    let render_target = D2D_FACTORY.with(|factory| unsafe {
        let mut rect = RECT::default();
        let _ = GetClientRect(window, &mut rect);

        let render_props = D2D1_RENDER_TARGET_PROPERTIES {
            r#type: D2D1_RENDER_TARGET_TYPE_DEFAULT,
            pixelFormat: D2D1_PIXEL_FORMAT {
                format: DXGI_FORMAT_B8G8R8A8_UNORM,
                alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED,
            },
            ..Default::default()
        };

        let hwnd_props = D2D1_HWND_RENDER_TARGET_PROPERTIES {
            hwnd: window,
            pixelSize: windows::Win32::Graphics::Direct2D::Common::D2D_SIZE_U {
                width: (rect.right - rect.left) as u32,
                height: (rect.bottom - rect.top) as u32,
            },
            presentOptions: D2D1_PRESENT_OPTIONS_NONE,
        };

        factory.CreateHwndRenderTarget(&render_props, &hwnd_props)
    });

    let Ok(rt) = render_target else {
        error!("Failed to create render target");
        unsafe { EndPaint(window, &ps) };
        return LRESULT::default();
    };

    // Create text formats
    let text_formats = DW_FACTORY.with(|factory| unsafe {
        let font_name_wide: Vec<u16> = arg
            .font_name
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect();

        let candi_format = factory.CreateTextFormat(
            windows::core::PCWSTR(font_name_wide.as_ptr()),
            None,
            DWRITE_FONT_WEIGHT_NORMAL,
            DWRITE_FONT_STYLE_NORMAL,
            DWRITE_FONT_STRETCH_NORMAL,
            arg.font_size,
            w!("en-us"),
        );

        let index_format = factory.CreateTextFormat(
            windows::core::PCWSTR(font_name_wide.as_ptr()),
            None,
            DWRITE_FONT_WEIGHT_NORMAL,
            DWRITE_FONT_STYLE_NORMAL,
            DWRITE_FONT_STRETCH_NORMAL,
            arg.index_font_size,
            w!("en-us"),
        );

        match (candi_format, index_format) {
            (Ok(cf), Ok(inf)) => {
                let _ = cf.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_LEADING);
                let _ = cf.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER);
                let _ = inf.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_LEADING);
                let _ = inf.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER);
                Some((cf, inf))
            }
            _ => None,
        }
    });

    let Some((candi_format, index_format)) = text_formats else {
        error!("Failed to create text formats");
        unsafe { EndPaint(window, &ps) };
        return LRESULT::default();
    };

    unsafe {
        rt.BeginDraw();

        // Clear with background color
        rt.Clear(Some(&color_to_d2d(&conf.color.background)));

        // Draw clip
        if let Ok(clip_brush) = rt.CreateSolidColorBrush(&color_to_d2d(&conf.color.clip), None) {
            rt.FillRectangle(
                &D2D_RECT_F {
                    left: BORDER_WIDTH as f32,
                    top: BORDER_WIDTH as f32,
                    right: (BORDER_WIDTH + CLIP_WIDTH) as f32,
                    bottom: BORDER_WIDTH as f32 + arg.label_height,
                },
                &clip_brush,
            );
        }

        // Draw highlight
        if let Ok(highlight_brush) =
            rt.CreateSolidColorBrush(&color_to_d2d(&conf.color.highlight), None)
        {
            rt.FillRectangle(
                &D2D_RECT_F {
                    left: (BORDER_WIDTH + CLIP_WIDTH) as f32,
                    top: BORDER_WIDTH as f32,
                    right: (BORDER_WIDTH + CLIP_WIDTH) as f32 + arg.highlight_width,
                    bottom: BORDER_WIDTH as f32 + arg.label_height,
                },
                &highlight_brush,
            );
        }

        // Create text brushes
        let index_brush = rt
            .CreateSolidColorBrush(&color_to_d2d(&conf.color.index), None)
            .ok();
        let highlighted_brush = rt
            .CreateSolidColorBrush(&color_to_d2d(&conf.color.highlighted), None)
            .ok();
        let candidate_brush = rt
            .CreateSolidColorBrush(&color_to_d2d(&conf.color.candidate), None)
            .ok();

        if index_brush.is_none() || highlighted_brush.is_none() || candidate_brush.is_none() {
            error!("Failed to create text brushes");
            let _ = rt.EndDraw(None, None);
            EndPaint(window, &ps);
            return LRESULT::default();
        }

        let index_brush = index_brush.unwrap();
        let highlighted_brush = highlighted_brush.unwrap();
        let candidate_brush = candidate_brush.unwrap();

        // Calculate vertical centering offset
        let index_y_offset = (arg.row_height - arg.index_height) / 2.0;
        let candi_y_offset = (arg.row_height - arg.candi_height) / 2.0;

        // Draw text
        let mut index_x = (BORDER_WIDTH + CLIP_WIDTH + LABEL_PADDING_LEFT) as f32;
        let mut candi_x = index_x + arg.index_width + INDEX_CANDI_GAP as f32;
        let mut text_y = BORDER_WIDTH as f32 + LABEL_PADDING_TOP as f32;

        // Draw highlighted (first) item
        draw_text_with_color_emoji(
            &rt,
            &arg.indice[0],
            &index_format,
            index_x,
            text_y + index_y_offset,
            arg.index_width,
            arg.index_height,
            &index_brush,
        );
        draw_text_with_color_emoji(
            &rt,
            &arg.candis[0],
            &candi_format,
            candi_x,
            text_y + candi_y_offset,
            arg.candi_widths[0] + 10.0,
            arg.candi_height,
            &highlighted_brush,
        );

        // Draw remaining items
        for i in 1..arg.candis.len() {
            if conf.layout.vertical {
                text_y += arg.label_height;
            } else {
                index_x += arg.index_width
                    + INDEX_CANDI_GAP as f32
                    + arg.candi_widths[i - 1]
                    + LABEL_PADDING_LEFT as f32
                    + LABEL_PADDING_RIGHT as f32;
                candi_x = index_x + arg.index_width + INDEX_CANDI_GAP as f32;
            }

            draw_text_with_color_emoji(
                &rt,
                &arg.indice[i],
                &index_format,
                index_x,
                text_y + index_y_offset,
                arg.index_width,
                arg.index_height,
                &index_brush,
            );
            draw_text_with_color_emoji(
                &rt,
                &arg.candis[i],
                &candi_format,
                candi_x,
                text_y + candi_y_offset,
                arg.candi_widths[i] + 10.0,
                arg.candi_height,
                &candidate_brush,
            );
        }

        let _ = rt.EndDraw(None, None);
    }

    unsafe { EndPaint(window, &ps) };
    LRESULT::default()
}

fn color_to_d2d(color: &Color) -> windows::Win32::Graphics::Direct2D::Common::D2D1_COLOR_F {
    let [r, g, b, a] = color.to_array();
    windows::Win32::Graphics::Direct2D::Common::D2D1_COLOR_F { r, g, b, a }
}

unsafe fn draw_text_with_color_emoji(
    rt: &ID2D1HwndRenderTarget,
    text: &str,
    format: &IDWriteTextFormat,
    x: f32,
    y: f32,
    width: f32,
    height: f32,
    brush: &ID2D1SolidColorBrush,
) {
    let text_wide: Vec<u16> = text.encode_utf16().collect();
    let rect = D2D_RECT_F {
        left: x,
        top: y,
        right: x + width,
        bottom: y + height,
    };

    // D2D1_DRAW_TEXT_OPTIONS_ENABLE_COLOR_FONT enables color emoji rendering
    rt.DrawText(
        &text_wide,
        format,
        &rect,
        brush,
        D2D1_DRAW_TEXT_OPTIONS_ENABLE_COLOR_FONT,
        DWRITE_MEASURING_MODE_NATURAL,
    );
}

// Keep the old FillRect for any remaining GDI usage if needed
#[allow(non_snake_case, dead_code)]
unsafe fn FillRect(hdc: HDC, x: i32, y: i32, width: i32, height: i32, color: &Color) {
    let rect = RECT {
        left: x,
        top: y,
        right: x + width,
        bottom: height,
    };
    unsafe { Gdi::FillRect(hdc, &rect, color.to_hbrush()) };
}