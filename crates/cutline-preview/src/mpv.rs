//! Minimal libmpv binding, loaded with dlopen so Cutline starts without it.
//! Frames are rendered with mpv's software renderer into memory and handed
//! to the UI as textures; mpv does decoding, audio and A/V sync.

use crate::{fit_size, Frame, Waker};
use libloading::{Library, Symbol};
use std::ffi::{c_char, c_double, c_int, c_void, CString};
use std::path::Path;

type MpvHandle = c_void;
type RenderCtx = c_void;

#[repr(C)]
struct RenderParam {
    kind: c_int,
    data: *mut c_void,
}

#[repr(C)]
struct MpvEvent {
    event_id: c_int,
    error: c_int,
    reply_userdata: u64,
    data: *mut c_void,
}

const FORMAT_FLAG: c_int = 3;
const FORMAT_INT64: c_int = 4;
const FORMAT_DOUBLE: c_int = 5;
const PARAM_INVALID: c_int = 0;
const PARAM_API_TYPE: c_int = 1;
const PARAM_BLOCK_FOR_TARGET_TIME: c_int = 12;
const PARAM_SW_SIZE: c_int = 17;
const PARAM_SW_FORMAT: c_int = 18;
const PARAM_SW_STRIDE: c_int = 19;
const PARAM_SW_POINTER: c_int = 20;
const UPDATE_FRAME: u64 = 1;
const EVENT_NONE: c_int = 0;
const EVENT_END_FILE: c_int = 7;
const EVENT_FILE_LOADED: c_int = 8;
const LC_NUMERIC: c_int = 1;

extern "C" {
    fn setlocale(category: c_int, locale: *const c_char) -> *mut c_char;
}

struct Api {
    _lib: Library,
    create: unsafe extern "C" fn() -> *mut MpvHandle,
    initialize: unsafe extern "C" fn(*mut MpvHandle) -> c_int,
    terminate_destroy: unsafe extern "C" fn(*mut MpvHandle),
    set_option_string: unsafe extern "C" fn(*mut MpvHandle, *const c_char, *const c_char) -> c_int,
    set_property_string:
        unsafe extern "C" fn(*mut MpvHandle, *const c_char, *const c_char) -> c_int,
    get_property: unsafe extern "C" fn(*mut MpvHandle, *const c_char, c_int, *mut c_void) -> c_int,
    command: unsafe extern "C" fn(*mut MpvHandle, *mut *const c_char) -> c_int,
    command_async: unsafe extern "C" fn(*mut MpvHandle, u64, *mut *const c_char) -> c_int,
    wait_event: unsafe extern "C" fn(*mut MpvHandle, c_double) -> *mut MpvEvent,
    set_wakeup_callback: unsafe extern "C" fn(
        *mut MpvHandle,
        Option<unsafe extern "C" fn(*mut c_void)>,
        *mut c_void,
    ),
    render_create:
        unsafe extern "C" fn(*mut *mut RenderCtx, *mut MpvHandle, *mut RenderParam) -> c_int,
    render_set_update_callback: unsafe extern "C" fn(
        *mut RenderCtx,
        Option<unsafe extern "C" fn(*mut c_void)>,
        *mut c_void,
    ),
    render_update: unsafe extern "C" fn(*mut RenderCtx) -> u64,
    render_render: unsafe extern "C" fn(*mut RenderCtx, *mut RenderParam) -> c_int,
    render_free: unsafe extern "C" fn(*mut RenderCtx),
}

impl Api {
    fn load() -> Result<Api, String> {
        let names = ["libmpv.so.2", "libmpv.so.1", "libmpv.so"];
        let lib = names
            .iter()
            .find_map(|n| unsafe { Library::new(n).ok() })
            .ok_or_else(|| "libmpv not found".to_string())?;
        unsafe {
            macro_rules! sym {
                ($name:literal) => {{
                    let s: Symbol<_> = lib
                        .get(concat!($name, "\0").as_bytes())
                        .map_err(|e| format!("{}: {e}", $name))?;
                    *s
                }};
            }
            Ok(Api {
                create: sym!("mpv_create"),
                initialize: sym!("mpv_initialize"),
                terminate_destroy: sym!("mpv_terminate_destroy"),
                set_option_string: sym!("mpv_set_option_string"),
                set_property_string: sym!("mpv_set_property_string"),
                get_property: sym!("mpv_get_property"),
                command: sym!("mpv_command"),
                command_async: sym!("mpv_command_async"),
                wait_event: sym!("mpv_wait_event"),
                set_wakeup_callback: sym!("mpv_set_wakeup_callback"),
                render_create: sym!("mpv_render_context_create"),
                render_set_update_callback: sym!("mpv_render_context_set_update_callback"),
                render_update: sym!("mpv_render_context_update"),
                render_render: sym!("mpv_render_context_render"),
                render_free: sym!("mpv_render_context_free"),
                _lib: lib,
            })
        }
    }
}

unsafe extern "C" fn wake_trampoline(data: *mut c_void) {
    let w = &*(data as *const Waker);
    w();
}

pub struct MpvPlayer {
    api: Api,
    handle: *mut MpvHandle,
    render: *mut RenderCtx,
    waker: Box<Waker>,
    buf: Vec<u8>,
    have_file: bool,
    ended: bool,
    dirty: bool,
    last_size: (usize, usize),
}

// libmpv handles are thread-safe; the render context is only used from the UI thread.
unsafe impl Send for MpvPlayer {}

fn cs(s: &str) -> CString {
    CString::new(s).unwrap_or_default()
}

impl MpvPlayer {
    pub fn new(waker: Waker) -> Result<MpvPlayer, String> {
        if std::env::var_os("CUTLINE_NO_MPV").is_some() {
            return Err("disabled by CUTLINE_NO_MPV".into());
        }
        let api = Api::load()?;
        unsafe {
            setlocale(LC_NUMERIC, c"C".as_ptr());
            let handle = (api.create)();
            if handle.is_null() {
                return Err("mpv_create failed".into());
            }
            let opt =
                |k: &str, v: &str| (api.set_option_string)(handle, cs(k).as_ptr(), cs(v).as_ptr());
            opt("vo", "libmpv");
            // Tuned for scrubbing (see `examples/scrub.rs`). Frames are copied
            // to the CPU for the software renderer anyway, and software decoding
            // restarts after a seek far sooner than hwdec with copy-back.
            opt("hwdec", "no");
            opt("keep-open", "yes");
            opt("hr-seek", "yes");
            // Skip decoding frames nothing references on the way to a seek target.
            opt("hr-seek-framedrop", "yes");
            // Show the first frame after a seek instead of first buffering
            // several: ~200 ms off every seek and every file open.
            opt("video-latency-hacks", "yes");
            opt("audio-display", "no");
            opt("osc", "no");
            opt("input-default-bindings", "no");
            opt("terminal", "no");
            opt("msg-level", "all=no");
            opt("idle", "yes");
            opt("config", "no");
            opt("demuxer-max-back-bytes", "200MiB");
            if (api.initialize)(handle) < 0 {
                (api.terminate_destroy)(handle);
                return Err("mpv_initialize failed".into());
            }
            let mut render: *mut RenderCtx = std::ptr::null_mut();
            let sw = cs("sw");
            let mut params = [
                RenderParam {
                    kind: PARAM_API_TYPE,
                    data: sw.as_ptr() as *mut c_void,
                },
                RenderParam {
                    kind: PARAM_INVALID,
                    data: std::ptr::null_mut(),
                },
            ];
            if (api.render_create)(&mut render, handle, params.as_mut_ptr()) < 0 || render.is_null()
            {
                (api.terminate_destroy)(handle);
                return Err("mpv software renderer unavailable".into());
            }
            let waker = Box::new(waker);
            let data = &*waker as *const Waker as *mut c_void;
            (api.render_set_update_callback)(render, Some(wake_trampoline), data);
            (api.set_wakeup_callback)(handle, Some(wake_trampoline), data);
            Ok(MpvPlayer {
                api,
                handle,
                render,
                waker,
                buf: vec![],
                have_file: false,
                ended: false,
                dirty: true,
                last_size: (0, 0),
            })
        }
    }

    fn cmd(&self, args: &[&str]) {
        let owned: Vec<CString> = args.iter().map(|a| cs(a)).collect();
        let mut ptrs: Vec<*const c_char> = owned.iter().map(|c| c.as_ptr()).collect();
        ptrs.push(std::ptr::null());
        unsafe {
            (self.api.command)(self.handle, ptrs.as_mut_ptr());
        }
    }
    fn cmd_async(&self, args: &[&str]) {
        let owned: Vec<CString> = args.iter().map(|a| cs(a)).collect();
        let mut ptrs: Vec<*const c_char> = owned.iter().map(|c| c.as_ptr()).collect();
        ptrs.push(std::ptr::null());
        unsafe {
            (self.api.command_async)(self.handle, 0, ptrs.as_mut_ptr());
        }
    }
    fn set(&self, k: &str, v: &str) {
        unsafe {
            (self.api.set_property_string)(self.handle, cs(k).as_ptr(), cs(v).as_ptr());
        }
    }
    fn get_f64(&self, k: &str) -> Option<f64> {
        let mut v: f64 = 0.0;
        let r = unsafe {
            (self.api.get_property)(
                self.handle,
                cs(k).as_ptr(),
                FORMAT_DOUBLE,
                &mut v as *mut f64 as *mut c_void,
            )
        };
        (r >= 0).then_some(v)
    }
    fn get_i64(&self, k: &str) -> Option<i64> {
        let mut v: i64 = 0;
        let r = unsafe {
            (self.api.get_property)(
                self.handle,
                cs(k).as_ptr(),
                FORMAT_INT64,
                &mut v as *mut i64 as *mut c_void,
            )
        };
        (r >= 0).then_some(v)
    }
    fn get_flag(&self, k: &str) -> Option<bool> {
        let mut v: c_int = 0;
        let r = unsafe {
            (self.api.get_property)(
                self.handle,
                cs(k).as_ptr(),
                FORMAT_FLAG,
                &mut v as *mut c_int as *mut c_void,
            )
        };
        (r >= 0).then_some(v != 0)
    }

    pub fn open(&mut self, path: &Path, start: f64, play: bool) {
        self.set("start", &format!("{start:.6}"));
        self.set("pause", if play { "no" } else { "yes" });
        self.cmd(&["loadfile", &path.to_string_lossy(), "replace"]);
        self.have_file = true;
        self.ended = false;
        self.dirty = true;
    }
    pub fn set_paused(&mut self, paused: bool) {
        if !paused && self.at_end() {
            self.cmd(&["seek", "0", "absolute+exact"]);
        }
        self.set("pause", if paused { "yes" } else { "no" });
    }
    pub fn paused(&self) -> bool {
        self.get_flag("pause").unwrap_or(true)
    }
    pub fn seek(&mut self, t: f64) {
        self.ended = false;
        self.cmd_async(&["seek", &format!("{:.6}", t.max(0.0)), "absolute+exact"]);
    }
    pub fn position(&self) -> f64 {
        self.get_f64("time-pos").unwrap_or(0.0)
    }
    pub fn at_end(&self) -> bool {
        self.get_flag("eof-reached").unwrap_or(false) || self.ended
    }
    pub fn set_volume(&mut self, v: f64) {
        self.set("volume", &format!("{:.0}", v.clamp(0.0, 100.0)));
    }

    pub fn update(&mut self, max_w: usize, max_h: usize) -> Option<Frame> {
        unsafe {
            loop {
                let ev = (self.api.wait_event)(self.handle, 0.0);
                if ev.is_null() || (*ev).event_id == EVENT_NONE {
                    break;
                }
                match (*ev).event_id {
                    EVENT_END_FILE => self.ended = true,
                    EVENT_FILE_LOADED => {
                        self.ended = false;
                        self.dirty = true;
                    }
                    _ => {}
                }
            }

            if !self.have_file {
                return None;
            }
            let flags = (self.api.render_update)(self.render);
            let (dw, dh) = (
                self.get_i64("dwidth").unwrap_or(0) as usize,
                self.get_i64("dheight").unwrap_or(0) as usize,
            );
            if dw == 0 || dh == 0 {
                return None;
            }
            let (w, h) = fit_size(dw, dh, max_w.max(64), max_h.max(64));
            let resized = (w, h) != self.last_size;
            if flags & UPDATE_FRAME == 0 && !resized && !self.dirty {
                return None;
            }
            self.dirty = false;
            self.last_size = (w, h);
            let stride = w * 4;
            self.buf.resize(stride * h, 0);
            let mut size: [c_int; 2] = [w as c_int, h as c_int];
            let fmt = c"rgb0";
            let mut stride_v: usize = stride;
            let mut block: c_int = 0;
            let mut params = [
                RenderParam {
                    kind: PARAM_SW_SIZE,
                    data: size.as_mut_ptr() as *mut c_void,
                },
                RenderParam {
                    kind: PARAM_SW_FORMAT,
                    data: fmt.as_ptr() as *mut c_void,
                },
                RenderParam {
                    kind: PARAM_SW_STRIDE,
                    data: &mut stride_v as *mut usize as *mut c_void,
                },
                RenderParam {
                    kind: PARAM_SW_POINTER,
                    data: self.buf.as_mut_ptr() as *mut c_void,
                },
                RenderParam {
                    kind: PARAM_BLOCK_FOR_TARGET_TIME,
                    data: &mut block as *mut c_int as *mut c_void,
                },
                RenderParam {
                    kind: PARAM_INVALID,
                    data: std::ptr::null_mut(),
                },
            ];
            if (self.api.render_render)(self.render, params.as_mut_ptr()) < 0 {
                return None;
            }
            // Hand the buffer over rather than copy it; the next frame gets a new one.
            let mut rgba = std::mem::take(&mut self.buf);
            // Force every alpha byte opaque (RGBA, so bytes 3, 7, 11, …).
            for a in rgba.iter_mut().skip(3).step_by(4) {
                *a = 255;
            }
            Some(Frame {
                width: w,
                height: h,
                rgba,
            })
        }
    }

    pub fn close(&mut self) {
        self.cmd(&["stop"]);
        self.have_file = false;
    }
}

impl Drop for MpvPlayer {
    fn drop(&mut self) {
        unsafe {
            (self.api.render_set_update_callback)(self.render, None, std::ptr::null_mut());
            (self.api.set_wakeup_callback)(self.handle, None, std::ptr::null_mut());
            (self.api.render_free)(self.render);
            (self.api.terminate_destroy)(self.handle);
        }
        let _ = &self.waker;
    }
}
