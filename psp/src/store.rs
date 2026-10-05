//! Storage: the pack's sections read one by one, and the PSPLINK mailbox.
//!
//! The machine has 24 MB. Each section that stays is read straight into the
//! memory that keeps it; the cells' near levels come and go (`stream`). The mailbox (a status file written, a control file read, on the
//! computer's share) is served by a thread with a lower priority than the
//! frame's: it runs while the frame waits for the GE or the display, so host
//! I/O never delays a frame.

use alloc::vec::Vec;
use core::ffi::c_void;
use core::ptr;
use core::sync::atomic::{AtomicU32, Ordering};

use psp::sys::*;

pub struct Section {
    pub tag: u32,
    pub offset: u32,
    pub size: u32,
}

pub struct PackFile {
    pub fd: SceUid,
    /// The path it was opened by, with its closing zero: the cells' reader opens it again for itself.
    pub path: &'static [u8],
    pub sections: Vec<Section>,
    pub bytes: u32,
    /// Whether the computer's share is there (PSPLINK): the pack came from it, or it holds `tokyo/boot.txt`.
    pub host: bool,
}

/// Reads exactly `len` bytes; storage returns short reads on large requests.
pub unsafe fn read_exact(fd: SceUid, dst: *mut u8, len: usize) -> bool {
    let mut at = 0;
    while at < len {
        let n = sceIoRead(fd, dst.add(at) as *mut c_void, (len - at).min(256 * 1024) as u32);
        if n <= 0 {
            return false;
        }
        at += n as usize;
    }
    true
}

impl PackFile {
    pub unsafe fn open() -> Result<PackFile, &'static str> {
        // Beside the EBOOT first (a packaged copy), then the computer's share.
        for (path, host) in [(&b"city.pack\0"[..], false), (&b"host0:/tokyo/city.pack\0"[..], true)] as [(&'static [u8], bool); 2] {
            let fd = sceIoOpen(path.as_ptr(), IoOpenFlags::RD_ONLY, 0);
            if fd.0 < 0 {
                continue;
            }
            let bytes = sceIoLseek32(fd, 0, IoWhence::End).max(0) as u32;
            sceIoLseek32(fd, 0, IoWhence::Set);
            let mut head = [0u32; 4];
            if !read_exact(fd, head.as_mut_ptr().cast(), 16) || head[0] != tokyo_pack::MAGIC {
                return Err("city.pack is not a pack");
            }
            if head[1] != tokyo_pack::VERSION {
                return Err("city.pack has another version");
            }
            let n = head[2] as usize;
            if n > 64 {
                return Err("city.pack section table");
            }
            let mut table = alloc::vec![0u32; n * 4];
            if !read_exact(fd, table.as_mut_ptr().cast(), n * 16) {
                return Err("city.pack section table");
            }
            let sections = (0..n).map(|i| Section { tag: table[i * 4], offset: table[i * 4 + 1], size: table[i * 4 + 2] }).collect();
            // The mailbox is there when the computer's share has the game's directory, wherever the pack came from.
            let probe = sceIoOpen(b"host0:/tokyo/boot.txt\0".as_ptr(), IoOpenFlags::RD_ONLY, 0);
            if probe.0 >= 0 {
                sceIoClose(probe);
            }
            return Ok(PackFile { fd, path, sections, bytes, host: host || probe.0 >= 0 });
        }
        Err("no city.pack beside the program or on host0:/tokyo")
    }

    pub fn section(&self, tag: u32) -> Result<&Section, &'static str> {
        self.sections.iter().find(|s| s.tag == tag).ok_or("the pack lacks a section")
    }

    /// Reads `len` bytes at `offset` of a section into `dst`.
    pub unsafe fn read_into(&self, tag: u32, offset: usize, dst: *mut u8, len: usize) -> Result<(), &'static str> {
        let s = self.section(tag)?;
        if offset + len > s.size as usize {
            return Err("read past a section's end");
        }
        sceIoLseek32(self.fd, (s.offset as usize + offset) as i32, IoWhence::Set);
        if read_exact(self.fd, dst, len) {
            Ok(())
        } else {
            Err("reading the pack failed")
        }
    }

    /// A whole section as `T` records (`T` is plain data).
    pub unsafe fn records<T: Copy>(&self, tag: u32) -> Result<Vec<T>, &'static str> {
        let s = self.section(tag)?;
        let n = s.size as usize / core::mem::size_of::<T>();
        let mut v = Vec::<T>::with_capacity(n.max(1));
        self.read_into(tag, 0, v.as_mut_ptr().cast(), n * core::mem::size_of::<T>())?;
        v.set_len(n);
        Ok(v)
    }
}

// ---------------------------------------------------------------- the mailbox

static mut HOST: bool = false;
/// The kernel's answer when a start-up call fails, for the failure record.
pub static mut LAST_CODE: i32 = 0;

// The mailbox: the frame thread hands a status text over and takes control text back.
const STATUS_MAX: usize = 3072;
static mut STATUS: [u8; STATUS_MAX] = [0; STATUS_MAX];
static mut STATUS_LEN: usize = 0;
static STATUS_FLAG: AtomicU32 = AtomicU32::new(0);
const CONTROL_MAX: usize = 512;
static mut CONTROL: [u8; CONTROL_MAX] = [0; CONTROL_MAX];
static mut CONTROL_LEN: usize = 0;
static CONTROL_FLAG: AtomicU32 = AtomicU32::new(0);

/// Starts the thread that serves the mailbox.
pub unsafe fn start(pack: &PackFile) -> Result<(), &'static str> {
    HOST = pack.host;
    let id = sceKernelCreateThread(b"tokyo_mail\0".as_ptr(), reader, 40, 32 * 1024, ThreadAttributes::USER, ptr::null_mut());
    if id.0 < 0 {
        LAST_CODE = id.0;
        return Err("the mailbox thread was not created");
    }
    sceKernelStartThread(id, 0, ptr::null_mut());
    Ok(())
}

unsafe extern "C" fn reader(_: usize, _: *mut c_void) -> i32 {
    let mut last_control = [0u8; CONTROL_MAX];
    let mut last_len = usize::MAX;
    let mut beat = 0u32;
    loop {
        sceKernelDelayThread(200_000);
        if !HOST {
            continue;
        }
        if STATUS_FLAG.load(Ordering::Acquire) == 1 {
            let fd = sceIoOpen(b"host0:/tokyo/status.json\0".as_ptr(), IoOpenFlags::WR_ONLY | IoOpenFlags::CREAT | IoOpenFlags::TRUNC, 0o644);
            if fd.0 >= 0 {
                sceIoWrite(fd, ptr::addr_of!(STATUS) as *const c_void, STATUS_LEN);
                sceIoClose(fd);
            }
            STATUS_FLAG.store(0, Ordering::Release);
        }
        beat += 1;
        if beat % 2 == 0 && CONTROL_FLAG.load(Ordering::Acquire) == 0 {
            let fd = sceIoOpen(b"host0:/tokyo/control.txt\0".as_ptr(), IoOpenFlags::RD_ONLY, 0);
            if fd.0 >= 0 {
                let mut buf = [0u8; CONTROL_MAX];
                let n = sceIoRead(fd, buf.as_mut_ptr() as *mut c_void, CONTROL_MAX as u32).max(0) as usize;
                sceIoClose(fd);
                // What is there at launch is left over from an earlier run: only changes after it count.
                if last_len == usize::MAX {
                    last_control = buf;
                    last_len = n;
                } else if n != last_len || buf[..n] != last_control[..n] {
                    last_control = buf;
                    last_len = n;
                    CONTROL = buf;
                    CONTROL_LEN = n;
                    CONTROL_FLAG.store(1, Ordering::Release);
                }
            }
        }
    }
}

/// Hands a status record to the mailbox; dropped when the previous one is still being written.
pub unsafe fn publish(text: &str) {
    if !HOST || STATUS_FLAG.load(Ordering::Acquire) != 0 {
        return;
    }
    let n = text.len().min(STATUS_MAX);
    ptr::copy_nonoverlapping(text.as_ptr(), ptr::addr_of_mut!(STATUS) as *mut u8, n);
    STATUS_LEN = n;
    STATUS_FLAG.store(1, Ordering::Release);
}

/// New control text from the computer, once.
pub unsafe fn control(f: impl FnOnce(&str)) {
    if CONTROL_FLAG.load(Ordering::Acquire) != 1 {
        return;
    }
    let bytes = core::slice::from_raw_parts(ptr::addr_of!(CONTROL) as *const u8, CONTROL_LEN);
    if let Ok(text) = core::str::from_utf8(bytes) {
        f(text);
    }
    CONTROL_FLAG.store(0, Ordering::Release);
}

/// A small text file from the computer, read once (`host0:/tokyo/boot.txt`: control words applied at start).
pub unsafe fn boot_text(buf: &mut [u8]) -> Option<&str> {
    let fd = sceIoOpen(b"host0:/tokyo/boot.txt\0".as_ptr(), IoOpenFlags::RD_ONLY, 0);
    if fd.0 < 0 {
        return None;
    }
    let n = sceIoRead(fd, buf.as_mut_ptr() as *mut c_void, buf.len() as u32).max(0) as usize;
    sceIoClose(fd);
    core::str::from_utf8(&buf[..n]).ok()
}

/// Writes a frame the frame thread copied out of video memory to `host0:/tokyo/shot.raw`.
pub unsafe fn write_shot(pixels: &[u8]) {
    let fd = sceIoOpen(b"host0:/tokyo/shot.raw\0".as_ptr(), IoOpenFlags::WR_ONLY | IoOpenFlags::CREAT | IoOpenFlags::TRUNC, 0o644);
    if fd.0 >= 0 {
        let mut at = 0;
        while at < pixels.len() {
            let n = sceIoWrite(fd, pixels.as_ptr().add(at) as *const c_void, (pixels.len() - at).min(64 * 1024));
            if n <= 0 {
                break;
            }
            at += n as usize;
        }
        sceIoClose(fd);
    }
}

/// Writes one message for the computer while loading (the frame loop is not running yet).
pub unsafe fn note(text: &str) {
    let fd = sceIoOpen(b"host0:/tokyo/status.json\0".as_ptr(), IoOpenFlags::WR_ONLY | IoOpenFlags::CREAT | IoOpenFlags::TRUNC, 0o644);
    if fd.0 >= 0 {
        sceIoWrite(fd, text.as_ptr() as *const c_void, text.len());
        sceIoClose(fd);
    }
}
