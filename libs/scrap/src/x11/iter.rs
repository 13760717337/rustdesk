use std::ffi::CString;
use std::ptr;
use std::rc::Rc;

use crate::Pixfmt;
use hbb_common::libc;

use super::ffi::*;
use super::{Display, Rect, Server};

//TODO: Do I have to free the displays?

pub struct DisplayIter {
    outer: xcb_screen_iterator_t,
    inner: Option<(
        xcb_randr_monitor_info_iterator_t,
        xcb_window_t,
        Option<Pixfmt>,
    )>,
    server: Rc<Server>,
}

impl DisplayIter {
    pub unsafe fn new(server: Rc<Server>) -> DisplayIter {
        let mut outer = xcb_setup_roots_iterator(server.setup());
        let inner = Self::next_screen(&mut outer, &server);
        DisplayIter {
            outer,
            inner,
            server,
        }
    }

    fn next_screen(
        outer: &mut xcb_screen_iterator_t,
        server: &Server,
    ) -> Option<(
        xcb_randr_monitor_info_iterator_t,
        xcb_window_t,
        Option<Pixfmt>,
    )> {
        if outer.rem == 0 {
            return None;
        }

        unsafe {
            let root = (*outer.data).root;
            let pixfmt = get_pixfmt(server.setup(), &*outer.data);

            let cookie = xcb_randr_get_monitors_unchecked(
                server.raw(),
                root,
                1, //TODO: I don't know if this should be true or false.
            );

            let response = xcb_randr_get_monitors_reply(server.raw(), cookie, ptr::null_mut());

            let inner = xcb_randr_get_monitors_monitors_iterator(response);

            libc::free(response as *mut _);
            xcb_screen_next(outer);

            Some((inner, root, pixfmt))
        }
    }
}

impl Iterator for DisplayIter {
    type Item = Display;

    fn next(&mut self) -> Option<Display> {
        loop {
            if let Some((ref mut inner, root, pixfmt)) = self.inner {
                // If there is something in the current screen, return that.
                if inner.rem != 0 {
                    unsafe {
                        let data = &*inner.data;
                        let name = get_atom_name(self.server.raw(), data.name);
                        let display = Display::new(
                            self.server.clone(),
                            data.primary != 0,
                            Rect {
                                x: data.x,
                                y: data.y,
                                w: data.width,
                                h: data.height,
                            },
                            root,
                            name,
                            pixfmt,
                        );

                        xcb_randr_monitor_info_next(inner);
                        return Some(display);
                    }
                }
            } else {
                // If there is no current screen, the screen iterator is empty.
                return None;
            }

            // The current screen was empty, so try the next screen.
            self.inner = Self::next_screen(&mut self.outer, &self.server);
        }
    }
}

fn get_atom_name(conn: *mut xcb_connection_t, atom: xcb_atom_t) -> String {
    let empty = "".to_owned();
    if atom == 0 {
        return empty;
    }
    unsafe {
        let mut e: *mut xcb_generic_error_t = std::ptr::null_mut();
        let reply = xcb_get_atom_name_reply(conn, xcb_get_atom_name(conn, atom), &mut e as _);
        if reply == std::ptr::null() {
            return empty;
        }
        let length = xcb_get_atom_name_name_length(reply);
        let name = xcb_get_atom_name_name(reply);
        let mut v = vec![0u8; length as _];
        std::ptr::copy_nonoverlapping(name as _, v.as_mut_ptr(), length as _);
        libc::free(reply as *mut _);
        if let Ok(s) = CString::new(v) {
            return s.to_string_lossy().to_string();
        }
        empty
    }
}

// Depth alone does not fix the byte layout, so like FFmpeg's xcbgrab consult
// the pixmap format and the server's byte order, and for depth 30 also require
// the xRGB2101010 masks that libyuv's AR30 expects. `None` means unsupported:
// `Capturer::new` refuses it instead of mislabeling the frames as BGRA.
// 16/24/32 keep their historical little-endian mapping.
unsafe fn get_pixfmt(setup: *const xcb_setup_t, screen: &xcb_screen_t) -> Option<Pixfmt> {
    let depth = screen.root_depth;
    let bpp = pixmap_bits_per_pixel(setup, depth);
    let lsb_first = (*setup).image_byte_order == XCB_IMAGE_ORDER_LSB_FIRST;
    let masks = root_visual_masks(screen);
    let pixfmt = match (depth, bpp) {
        (16, _) => Some(Pixfmt::RGB565LE),
        (24, _) | (32, _) => Some(Pixfmt::BGRA),
        (30, Some(32)) if lsb_first && masks == Some((0x3ff0_0000, 0x000f_fc00, 0x0000_03ff)) => {
            Some(Pixfmt::AR30)
        }
        _ => None,
    };
    if pixfmt.is_none() {
        hbb_common::log::warn!(
            "unsupported X11 root window format: depth {depth}, bits per pixel {bpp:?}, \
             rgb masks {masks:x?}, lsb first {lsb_first}"
        );
    }
    pixfmt
}

unsafe fn pixmap_bits_per_pixel(setup: *const xcb_setup_t, depth: u8) -> Option<u8> {
    let formats = xcb_setup_pixmap_formats(setup);
    (0..xcb_setup_pixmap_formats_length(setup))
        .map(|i| &*formats.add(i as usize))
        .find(|format| format.depth == depth)
        .map(|format| format.bits_per_pixel)
}

unsafe fn root_visual_masks(screen: &xcb_screen_t) -> Option<(u32, u32, u32)> {
    let mut depths = xcb_screen_allowed_depths_iterator(screen);
    while depths.rem > 0 {
        let depth = &*depths.data;
        let visuals = xcb_depth_visuals(depth);
        for i in 0..xcb_depth_visuals_length(depth) {
            let visual = &*visuals.add(i as usize);
            if visual.visual_id == screen.root_visual {
                return Some((visual.red_mask, visual.green_mask, visual.blue_mask));
            }
        }
        xcb_depth_next(&mut depths);
    }
    None
}
