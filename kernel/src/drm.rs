//! Minimal DRM/KMS device (`/dev/dri/card0`) over the boot framebuffer, in the
//! spirit of Linux's `simpledrm`: one connector, encoder, CRTC and primary
//! plane driving the fixed VESA mode, dumb buffers, framebuffer objects,
//! legacy mode setting, page flips with completion events, and PRIME.
//!
//! This module only holds object state and semantics, so it can be unit
//! tested on the host. The syscall layer (`kernel/src/syscall/drm.rs`) owns
//! the uapi structs, user-memory copies, the hidden shmem files that back
//! dumb buffers, and copying scanout buffers to the VESA framebuffer.
//!
//! Open files are refcounted objects referenced from task fd slots
//! (`USER_FD_KIND_DRM`, object index in the slot's inode field), like
//! eventfds. Time is passed in explicitly (`now_ms`); flip events become
//! readable on a 60 Hz cadence and readiness is evaluated lazily.

use crate::sync::SpinLock;

const EACCES: i64 = 13;
const EBADF: i64 = 9;
const EBUSY: i64 = 16;
const EINVAL: i64 = 22;
const EMFILE: i64 = 24;
const ENOENT: i64 = 2;
const ENOMEM: i64 = 12;
const ENOSPC: i64 = 28;

pub const MAX_FILES: usize = 16;
pub const MAX_GEMS: usize = 64;
pub const MAX_FBS: usize = 32;
pub const MAX_EVENTS: usize = 8;

/// KMS object ids (unique across object types, as on Linux).
pub const CRTC_ID: u32 = 31;
pub const ENCODER_ID: u32 = 32;
pub const CONNECTOR_ID: u32 = 33;
pub const PLANE_ID: u32 = 34;
/// Property ids.
pub const PROP_DPMS: u32 = 1;
pub const PROP_PLANE_TYPE: u32 = 2;
const FB_ID_BASE: u32 = 100;

/// `DRM_FORMAT_XRGB8888` / `DRM_FORMAT_ARGB8888` fourccs.
pub const FORMAT_XRGB8888: u32 = 0x3432_5258;
pub const FORMAT_ARGB8888: u32 = 0x3432_5241;
pub const MAX_DIMENSION: u32 = 8192;
/// Largest dumb buffer (the shmem object limit).
pub const DUMB_MAX_BYTES: u64 = 256 * 1024 * 1024;
/// Fake mmap offsets for dumb buffers: one 256 MiB window per GEM slot,
/// above anything a real file offset would use.
pub const DUMB_MMAP_BASE: u64 = 1 << 40;
pub const DUMB_MMAP_STRIDE: u64 = DUMB_MAX_BYTES;
/// Flip events are delivered no sooner than one 60 Hz frame apart.
pub const FRAME_MS: u64 = 16;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FlipEvent {
    pub user_data: u64,
    pub ready_ms: u64,
    pub sequence: u32,
}

impl FlipEvent {
    const EMPTY: Self = Self { user_data: 0, ready_ms: 0, sequence: 0 };
}

#[derive(Clone, Copy)]
struct File {
    used: bool,
    refs: u16,
    universal_planes: bool,
    next_handle: u32,
    events: [FlipEvent; MAX_EVENTS],
    ev_head: usize,
    ev_len: usize,
}

impl File {
    const EMPTY: Self = Self {
        used: false,
        refs: 0,
        universal_planes: false,
        next_handle: 1,
        events: [FlipEvent::EMPTY; MAX_EVENTS],
        ev_head: 0,
        ev_len: 0,
    };
}

/// A buffer object visible to one file through `handle`. `inode` is the
/// hidden shmem file holding its pages; several GEM entries (in different
/// files, after PRIME import) may share one inode.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Gem {
    pub used: bool,
    pub file: u16,
    /// 0 once the handle is closed but framebuffers still reference it.
    pub handle: u32,
    pub inode: u16,
    pub size: u64,
    /// Handle reference plus one per framebuffer.
    refs: u16,
}

impl Gem {
    const EMPTY: Self = Self { used: false, file: 0, handle: 0, inode: 0, size: 0, refs: 0 };
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Fb {
    pub used: bool,
    pub id: u32,
    pub file: u16,
    pub gem: u16,
    pub width: u32,
    pub height: u32,
    pub pitch: u32,
    pub format: u32,
}

impl Fb {
    const EMPTY: Self =
        Self { used: false, id: 0, file: 0, gem: 0, width: 0, height: 0, pitch: 0, format: 0 };
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Mode {
    pub width: u32,
    pub height: u32,
}

/// What the syscall layer must copy to the screen after a state change.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Scanout {
    pub inode: u16,
    pub width: u32,
    pub height: u32,
    pub pitch: u32,
    pub x: u32,
    pub y: u32,
}

#[derive(Clone, Copy)]
struct Crtc {
    fb_id: u32,
    x: u32,
    y: u32,
    /// File that set the current mode; the console comes back when it closes.
    owner: Option<u16>,
    sequence: u32,
    /// A flip is pending until this time (its event's ready time).
    flip_ready_ms: u64,
    dpms_on: bool,
}

struct State {
    files: [File; MAX_FILES],
    gems: [Gem; MAX_GEMS],
    fbs: [Fb; MAX_FBS],
    crtc: Crtc,
    master: Option<u16>,
    next_fb_id: u32,
}

impl State {
    const fn new() -> Self {
        Self {
            files: [File::EMPTY; MAX_FILES],
            gems: [Gem::EMPTY; MAX_GEMS],
            fbs: [Fb::EMPTY; MAX_FBS],
            crtc: Crtc {
                fb_id: 0,
                x: 0,
                y: 0,
                owner: None,
                sequence: 0,
                flip_ready_ms: 0,
                dpms_on: true,
            },
            master: None,
            next_fb_id: FB_ID_BASE,
        }
    }

    fn file(&mut self, id: u16) -> Result<&mut File, i64> {
        match self.files.get_mut(id as usize) {
            Some(f) if f.used => Ok(f),
            _ => Err(-EBADF),
        }
    }

    fn gem_index(&self, file: u16, handle: u32) -> Result<usize, i64> {
        if handle == 0 {
            return Err(-ENOENT);
        }
        self.gems
            .iter()
            .position(|g| g.used && g.file == file && g.handle == handle)
            .ok_or(-ENOENT)
    }

    fn fb_index(&self, id: u32) -> Result<usize, i64> {
        if id == 0 {
            return Err(-ENOENT);
        }
        self.fbs.iter().position(|f| f.used && f.id == id).ok_or(-ENOENT)
    }

    /// Drop one GEM reference; returns the inode whose open reference the
    /// caller must release when the GEM entry dies.
    fn gem_unref(&mut self, idx: usize) -> Option<u16> {
        let gem = &mut self.gems[idx];
        gem.refs = gem.refs.saturating_sub(1);
        if gem.refs == 0 {
            let inode = gem.inode;
            *gem = Gem::EMPTY;
            Some(inode)
        } else {
            None
        }
    }

    fn scanout(&self) -> Option<Scanout> {
        let idx = self.fb_index(self.crtc.fb_id).ok()?;
        let fb = &self.fbs[idx];
        Some(Scanout {
            inode: self.gems[fb.gem as usize].inode,
            width: fb.width,
            height: fb.height,
            pitch: fb.pitch,
            x: self.crtc.x,
            y: self.crtc.y,
        })
    }
}

static STATE: SpinLock<State> = SpinLock::new(State::new());

/// Inodes whose open references the caller must release (after the DRM lock
/// is dropped), plus whether the console should take the display back.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Released {
    pub inodes: [u16; MAX_GEMS],
    pub count: usize,
    pub restore_console: bool,
}

impl Released {
    const fn new() -> Self {
        Self { inodes: [0; MAX_GEMS], count: 0, restore_console: false }
    }

    fn push(&mut self, inode: Option<u16>) {
        if let Some(inode) = inode {
            if self.count < MAX_GEMS {
                self.inodes[self.count] = inode;
                self.count += 1;
            }
        }
    }

    pub fn iter(&self) -> impl Iterator<Item = u16> + '_ {
        self.inodes[..self.count].iter().copied()
    }
}

// ---------------------------------------------------------------------------
// Files and master
// ---------------------------------------------------------------------------

/// Open `/dev/dri/card0`. The first opener while there is no master becomes
/// master, as on Linux.
pub fn open() -> Result<u16, i64> {
    let mut st = STATE.lock();
    let idx = st.files.iter().position(|f| !f.used).ok_or(-EMFILE)?;
    st.files[idx] = File { used: true, refs: 1, ..File::EMPTY };
    if st.master.is_none() {
        st.master = Some(idx as u16);
    }
    Ok(idx as u16)
}

pub fn retain(file: u16) -> Result<(), i64> {
    let mut st = STATE.lock();
    let f = st.file(file)?;
    f.refs = f.refs.saturating_add(1);
    Ok(())
}

/// Drop one reference; the last one destroys the file's framebuffers and GEM
/// handles. If the file had set the mode, the scanout stops and the console
/// should take the display back.
pub fn release(file: u16) -> Released {
    let mut out = Released::new();
    let mut st = STATE.lock();
    let Ok(f) = st.file(file) else {
        return out;
    };
    f.refs = f.refs.saturating_sub(1);
    if f.refs > 0 {
        return out;
    }
    st.files[file as usize] = File::EMPTY;
    if st.master == Some(file) {
        st.master = None;
    }
    for i in 0..MAX_FBS {
        if st.fbs[i].used && st.fbs[i].file == file {
            if st.crtc.fb_id == st.fbs[i].id {
                st.crtc.fb_id = 0;
            }
            let gem = st.fbs[i].gem as usize;
            st.fbs[i] = Fb::EMPTY;
            out.push(st.gem_unref(gem));
        }
    }
    for i in 0..MAX_GEMS {
        if st.gems[i].used && st.gems[i].file == file && st.gems[i].handle != 0 {
            st.gems[i].handle = 0;
            out.push(st.gem_unref(i));
        }
    }
    if st.crtc.owner == Some(file) {
        st.crtc.owner = None;
        st.crtc.fb_id = 0;
        out.restore_console = true;
    }
    out
}

pub fn is_master(file: u16) -> bool {
    STATE.lock().master == Some(file)
}

/// `DRM_IOCTL_SET_MASTER`: succeeds when nobody else is master.
pub fn set_master(file: u16) -> Result<(), i64> {
    let mut st = STATE.lock();
    st.file(file)?;
    match st.master {
        Some(m) if m != file => Err(-EBUSY),
        _ => {
            st.master = Some(file);
            Ok(())
        }
    }
}

pub fn drop_master(file: u16) -> Result<(), i64> {
    let mut st = STATE.lock();
    st.file(file)?;
    if st.master != Some(file) {
        return Err(-EINVAL);
    }
    st.master = None;
    Ok(())
}

pub fn set_universal_planes(file: u16, on: bool) -> Result<(), i64> {
    let mut st = STATE.lock();
    st.file(file)?.universal_planes = on;
    Ok(())
}

pub fn universal_planes(file: u16) -> bool {
    let mut st = STATE.lock();
    st.file(file).map(|f| f.universal_planes).unwrap_or(false)
}

// ---------------------------------------------------------------------------
// GEM / dumb buffers
// ---------------------------------------------------------------------------

/// `(pitch, size)` for a dumb buffer, or `-EINVAL`. Only 32 bpp is
/// supported; rows are 64-byte aligned and the size is page aligned.
pub fn dumb_layout(width: u32, height: u32, bpp: u32) -> Result<(u32, u64), i64> {
    if bpp != 32 || width == 0 || height == 0 || width > MAX_DIMENSION || height > MAX_DIMENSION {
        return Err(-EINVAL);
    }
    let pitch = (width * 4 + 63) & !63;
    let size = (pitch as u64 * height as u64 + 4095) & !4095;
    if size > DUMB_MAX_BYTES {
        return Err(-EINVAL);
    }
    Ok((pitch, size))
}

/// Register a buffer backed by `inode` for `file`. The caller has taken one
/// open reference on `inode`, which the GEM entry now owns. If `file`
/// already has a handle for `inode` (PRIME re-import), that handle is
/// returned and `Ok((handle, false))` tells the caller to drop its reference.
pub fn gem_add(file: u16, inode: u16, size: u64) -> Result<(u32, bool), i64> {
    let mut st = STATE.lock();
    st.file(file)?;
    if let Some(g) = st.gems.iter().find(|g| g.used && g.file == file && g.inode == inode && g.handle != 0) {
        return Ok((g.handle, false));
    }
    let idx = st.gems.iter().position(|g| !g.used).ok_or(-ENOSPC)?;
    let f = st.file(file)?;
    let handle = f.next_handle;
    f.next_handle = f.next_handle.wrapping_add(1).max(1);
    st.gems[idx] = Gem { used: true, file, handle, inode, size, refs: 1 };
    Ok((handle, true))
}

/// `GEM_CLOSE` / `DESTROY_DUMB`: drop the handle. Framebuffers keep the
/// buffer alive. Returns the inode to release if the buffer died.
pub fn gem_close(file: u16, handle: u32) -> Result<Option<u16>, i64> {
    let mut st = STATE.lock();
    st.file(file)?;
    let idx = st.gem_index(file, handle)?;
    st.gems[idx].handle = 0;
    Ok(st.gem_unref(idx))
}

/// `(inode, size)` behind a handle (for PRIME export and dumb mapping).
pub fn gem_lookup(file: u16, handle: u32) -> Result<(u16, u64), i64> {
    let mut st = STATE.lock();
    st.file(file)?;
    let idx = st.gem_index(file, handle)?;
    Ok((st.gems[idx].inode, st.gems[idx].size))
}

/// Fake mmap offset for `MAP_DUMB`.
pub fn dumb_map_offset(file: u16, handle: u32) -> Result<u64, i64> {
    let mut st = STATE.lock();
    st.file(file)?;
    let idx = st.gem_index(file, handle)?;
    Ok(DUMB_MMAP_BASE + idx as u64 * DUMB_MMAP_STRIDE)
}

/// Resolve an mmap of the card fd: `(inode, offset within the buffer)` if
/// `[offset, offset + len)` lies inside one of `file`'s dumb buffers.
pub fn dumb_mmap_target(file: u16, offset: u64, len: u64) -> Result<(u16, u64), i64> {
    if offset < DUMB_MMAP_BASE {
        return Err(-EINVAL);
    }
    let idx = ((offset - DUMB_MMAP_BASE) / DUMB_MMAP_STRIDE) as usize;
    let within = (offset - DUMB_MMAP_BASE) % DUMB_MMAP_STRIDE;
    let mut st = STATE.lock();
    st.file(file)?;
    let gem = st.gems.get(idx).copied().ok_or(-EINVAL)?;
    if !gem.used || gem.file != file || gem.handle == 0 {
        return Err(-EINVAL);
    }
    match within.checked_add(len) {
        Some(end) if end <= gem.size => Ok((gem.inode, within)),
        _ => Err(-EINVAL),
    }
}

// ---------------------------------------------------------------------------
// Framebuffers
// ---------------------------------------------------------------------------

/// `ADDFB` / `ADDFB2` (single plane, offset 0, linear).
pub fn fb_add(file: u16, handle: u32, width: u32, height: u32, pitch: u32, format: u32) -> Result<u32, i64> {
    if format != FORMAT_XRGB8888 && format != FORMAT_ARGB8888 {
        return Err(-EINVAL);
    }
    if width == 0 || height == 0 || width > MAX_DIMENSION || height > MAX_DIMENSION || pitch < width * 4 {
        return Err(-EINVAL);
    }
    let mut st = STATE.lock();
    st.file(file)?;
    let gem = st.gem_index(file, handle)?;
    if (pitch as u64) * (height as u64) > st.gems[gem].size {
        return Err(-EINVAL);
    }
    let idx = st.fbs.iter().position(|f| !f.used).ok_or(-ENOSPC)?;
    let id = st.next_fb_id;
    st.next_fb_id = st.next_fb_id.wrapping_add(1).max(FB_ID_BASE);
    st.gems[gem].refs = st.gems[gem].refs.saturating_add(1);
    st.fbs[idx] = Fb { used: true, id, file, gem: gem as u16, width, height, pitch, format };
    Ok(id)
}

/// `RMFB`: a framebuffer being scanned out turns the CRTC off, as on Linux.
pub fn fb_remove(file: u16, id: u32) -> Result<Option<u16>, i64> {
    let mut st = STATE.lock();
    st.file(file)?;
    let idx = st.fb_index(id)?;
    if st.fbs[idx].file != file {
        return Err(-ENOENT);
    }
    if st.crtc.fb_id == id {
        st.crtc.fb_id = 0;
    }
    let gem = st.fbs[idx].gem as usize;
    st.fbs[idx] = Fb::EMPTY;
    Ok(st.gem_unref(gem))
}

pub fn fb_get(id: u32) -> Result<Fb, i64> {
    let st = STATE.lock();
    let idx = st.fb_index(id)?;
    Ok(st.fbs[idx])
}

/// Framebuffer ids owned by `file` (for `GETRESOURCES`).
pub fn fb_ids(file: u16, out: &mut [u32; MAX_FBS]) -> usize {
    let st = STATE.lock();
    let mut n = 0;
    for f in st.fbs.iter() {
        if f.used && f.file == file {
            out[n] = f.id;
            n += 1;
        }
    }
    n
}

// ---------------------------------------------------------------------------
// CRTC: mode setting, flips, events
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CrtcState {
    pub fb_id: u32,
    pub x: u32,
    pub y: u32,
    pub active: bool,
}

pub fn crtc_state() -> CrtcState {
    let st = STATE.lock();
    CrtcState { fb_id: st.crtc.fb_id, x: st.crtc.x, y: st.crtc.y, active: st.crtc.fb_id != 0 }
}

/// Legacy `SETCRTC`. `fb_id == 0` turns the CRTC off. Otherwise the
/// framebuffer must cover the mode at `(x, y)`. Returns what to copy to the
/// screen (`None` when turning off).
pub fn set_crtc(file: u16, fb_id: u32, x: u32, y: u32, mode: Mode) -> Result<Option<Scanout>, i64> {
    let mut st = STATE.lock();
    st.file(file)?;
    if st.master != Some(file) {
        return Err(-EACCES);
    }
    if fb_id == 0 {
        st.crtc.fb_id = 0;
        return Ok(None);
    }
    let idx = st.fb_index(fb_id)?;
    let fb = st.fbs[idx];
    if x.checked_add(mode.width).map_or(true, |w| w > fb.width)
        || y.checked_add(mode.height).map_or(true, |h| h > fb.height)
    {
        return Err(-ENOSPC);
    }
    st.crtc.fb_id = fb_id;
    st.crtc.x = x;
    st.crtc.y = y;
    st.crtc.owner = Some(file);
    Ok(st.scanout())
}

/// Legacy `PAGE_FLIP`. Only one flip may be pending; with `event`, a flip
/// completion event for `user_data` becomes readable one frame after the
/// previous flip.
pub fn page_flip(file: u16, fb_id: u32, event: bool, user_data: u64, now_ms: u64) -> Result<Scanout, i64> {
    let mut st = STATE.lock();
    st.file(file)?;
    if st.master != Some(file) {
        return Err(-EACCES);
    }
    if st.crtc.fb_id == 0 {
        return Err(-EINVAL);
    }
    if now_ms < st.crtc.flip_ready_ms {
        return Err(-EBUSY);
    }
    let idx = st.fb_index(fb_id)?;
    let (cur_w, cur_h) = {
        let cur = st.fb_index(st.crtc.fb_id).map(|i| st.fbs[i]).ok();
        cur.map(|c| (c.width, c.height)).unwrap_or((0, 0))
    };
    let fb = st.fbs[idx];
    if fb.width != cur_w || fb.height != cur_h {
        // Legacy flips may not change the framebuffer size.
        return Err(-EINVAL);
    }
    // No flip is pending (checked above), so the new one completes one
    // frame from now.
    let ready = now_ms + FRAME_MS;
    if event {
        let f = st.file(file)?;
        if f.ev_len >= MAX_EVENTS {
            return Err(-ENOMEM);
        }
        let slot = (f.ev_head + f.ev_len) % MAX_EVENTS;
        f.events[slot] = FlipEvent { user_data, ready_ms: ready, sequence: 0 };
        f.ev_len += 1;
    }
    st.crtc.flip_ready_ms = ready;
    st.crtc.fb_id = fb_id;
    st.scanout().ok_or(-EINVAL)
}

/// Current scanout (for `DIRTYFB` on the scanned-out framebuffer).
pub fn scanout_if(fb_id: u32) -> Option<Scanout> {
    let st = STATE.lock();
    if st.crtc.fb_id == fb_id && fb_id != 0 {
        st.scanout()
    } else {
        None
    }
}

pub fn set_dpms(on: bool) {
    STATE.lock().crtc.dpms_on = on;
}

pub fn dpms_on() -> bool {
    STATE.lock().crtc.dpms_on
}

/// Pop the oldest flip event that is ready at `now_ms`.
pub fn take_event(file: u16, now_ms: u64) -> Result<Option<FlipEvent>, i64> {
    let mut st = STATE.lock();
    let seq = st.crtc.sequence;
    let f = st.file(file)?;
    if f.ev_len == 0 || f.events[f.ev_head].ready_ms > now_ms {
        return Ok(None);
    }
    let mut ev = f.events[f.ev_head];
    f.ev_head = (f.ev_head + 1) % MAX_EVENTS;
    f.ev_len -= 1;
    ev.sequence = seq.wrapping_add(1);
    st.crtc.sequence = ev.sequence;
    Ok(Some(ev))
}

pub fn has_ready_event(file: u16, now_ms: u64) -> bool {
    let mut st = STATE.lock();
    match st.file(file) {
        Ok(f) => f.ev_len > 0 && f.events[f.ev_head].ready_ms <= now_ms,
        Err(_) => false,
    }
}

/// `struct drm_event_vblank` for a flip completion (`DRM_EVENT_FLIP_COMPLETE`).
pub fn flip_event_bytes(ev: &FlipEvent, now_ms: u64) -> [u8; 32] {
    const DRM_EVENT_FLIP_COMPLETE: u32 = 0x02;
    let mut b = [0u8; 32];
    b[0..4].copy_from_slice(&DRM_EVENT_FLIP_COMPLETE.to_ne_bytes());
    b[4..8].copy_from_slice(&32u32.to_ne_bytes());
    b[8..16].copy_from_slice(&ev.user_data.to_ne_bytes());
    let t = ev.ready_ms.min(now_ms);
    b[16..20].copy_from_slice(&((t / 1000) as u32).to_ne_bytes());
    b[20..24].copy_from_slice(&(((t % 1000) * 1000) as u32).to_ne_bytes());
    b[24..28].copy_from_slice(&ev.sequence.to_ne_bytes());
    b[28..32].copy_from_slice(&CRTC_ID.to_ne_bytes());
    b
}

/// `struct drm_mode_modeinfo` (68 bytes) for the fixed mode at 60 Hz, with
/// CVT-like blanking so `clock` is plausible.
pub fn modeinfo_bytes(mode: Mode) -> [u8; 68] {
    const TYPE_PREFERRED: u32 = 1 << 3;
    const TYPE_DRIVER: u32 = 1 << 6;
    let w = mode.width.min(u16::MAX as u32) as u16;
    let h = mode.height.min(u16::MAX as u32) as u16;
    let htotal = w.saturating_add(160);
    let vtotal = h.saturating_add(23);
    let clock = (htotal as u32) * (vtotal as u32) * 60 / 1000;
    let mut b = [0u8; 68];
    b[0..4].copy_from_slice(&clock.to_ne_bytes());
    let hs = [w, w.saturating_add(48), w.saturating_add(80), htotal, 0];
    for (i, v) in hs.iter().enumerate() {
        b[4 + i * 2..6 + i * 2].copy_from_slice(&v.to_ne_bytes());
    }
    let vs = [h, h.saturating_add(3), h.saturating_add(9), vtotal, 0];
    for (i, v) in vs.iter().enumerate() {
        b[14 + i * 2..16 + i * 2].copy_from_slice(&v.to_ne_bytes());
    }
    b[24..28].copy_from_slice(&60u32.to_ne_bytes());
    b[32..36].copy_from_slice(&(TYPE_PREFERRED | TYPE_DRIVER).to_ne_bytes());
    let mut name = [0u8; 32];
    let mut n = 0;
    for part in [mode.width, mode.height] {
        if n > 0 {
            name[n] = b'x';
            n += 1;
        }
        let mut digits = [0u8; 10];
        let mut v = part;
        let mut d = 0;
        loop {
            digits[d] = b'0' + (v % 10) as u8;
            d += 1;
            v /= 10;
            if v == 0 {
                break;
            }
        }
        while d > 0 && n < 31 {
            d -= 1;
            name[n] = digits[d];
            n += 1;
        }
    }
    b[36..68].copy_from_slice(&name);
    b
}

/// `(hdisplay, vdisplay)` of a user-supplied `drm_mode_modeinfo`.
pub fn modeinfo_size(b: &[u8]) -> (u32, u32) {
    let hd = u16::from_ne_bytes([b[4], b[5]]) as u32;
    let vd = u16::from_ne_bytes([b[14], b[15]]) as u32;
    (hd, vd)
}

#[cfg(test)]
pub(crate) fn reset_for_test() {
    *STATE.lock() = State::new();
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    static LOCK: Mutex<()> = Mutex::new(());

    fn fresh() -> std::sync::MutexGuard<'static, ()> {
        let g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        reset_for_test();
        g
    }

    const MODE: Mode = Mode { width: 1024, height: 768 };

    fn dumb(file: u16, inode: u16) -> (u32, u32, u64) {
        let (pitch, size) = dumb_layout(1024, 768, 32).unwrap();
        let (h, fresh) = gem_add(file, inode, size).unwrap();
        assert!(fresh);
        (h, pitch, size)
    }

    #[test]
    fn first_opener_is_master_and_master_moves() {
        let _g = fresh();
        let a = open().unwrap();
        let b = open().unwrap();
        assert!(is_master(a) && !is_master(b));
        assert_eq!(set_master(b), Err(-EBUSY));
        drop_master(a).unwrap();
        set_master(b).unwrap();
        assert!(is_master(b));
        let r = release(b);
        assert_eq!(r.count, 0);
        assert!(!is_master(b));
    }

    #[test]
    fn dumb_layout_aligns_and_rejects() {
        assert_eq!(dumb_layout(1024, 768, 32), Ok((4096, 4096 * 768)));
        assert_eq!(dumb_layout(10, 3, 32), Ok((64, 4096)));
        assert_eq!(dumb_layout(10, 3, 16), Err(-EINVAL));
        assert_eq!(dumb_layout(0, 3, 32), Err(-EINVAL));
        assert_eq!(dumb_layout(9000, 3, 32), Err(-EINVAL));
    }

    #[test]
    fn framebuffer_keeps_buffer_alive_after_handle_close() {
        let _g = fresh();
        let f = open().unwrap();
        let (h, pitch, _) = dumb(f, 40);
        let fb = fb_add(f, h, 1024, 768, pitch, FORMAT_XRGB8888).unwrap();
        assert_eq!(gem_close(f, h), Ok(None), "fb still references it");
        assert_eq!(gem_lookup(f, h), Err(-ENOENT));
        assert_eq!(fb_remove(f, fb), Ok(Some(40)), "last reference releases the inode");
    }

    #[test]
    fn fb_add_validates_format_size_and_pitch() {
        let _g = fresh();
        let f = open().unwrap();
        let (h, pitch, _) = dumb(f, 41);
        assert_eq!(fb_add(f, h, 1024, 768, pitch, 0x1234), Err(-EINVAL));
        assert_eq!(fb_add(f, h, 1024, 769, pitch, FORMAT_XRGB8888), Err(-EINVAL));
        assert_eq!(fb_add(f, h, 1024, 768, 100, FORMAT_XRGB8888), Err(-EINVAL));
        assert_eq!(fb_add(f, 99, 1024, 768, pitch, FORMAT_XRGB8888), Err(-ENOENT));
        assert!(fb_add(f, h, 1024, 768, pitch, FORMAT_ARGB8888).is_ok());
    }

    #[test]
    fn setcrtc_requires_master_and_covering_fb() {
        let _g = fresh();
        let m = open().unwrap();
        let other = open().unwrap();
        let (h, pitch, _) = dumb(m, 42);
        let fb = fb_add(m, h, 1024, 768, pitch, FORMAT_XRGB8888).unwrap();
        assert_eq!(set_crtc(other, fb, 0, 0, MODE), Err(-EACCES));
        assert_eq!(set_crtc(m, fb, 1, 0, MODE), Err(-ENOSPC));
        let s = set_crtc(m, fb, 0, 0, MODE).unwrap().unwrap();
        assert_eq!((s.inode, s.width, s.pitch), (42, 1024, 4096));
        assert!(crtc_state().active);
        assert_eq!(set_crtc(m, 0, 0, 0, MODE), Ok(None));
        assert!(!crtc_state().active);
    }

    #[test]
    fn page_flip_queues_event_one_frame_later_and_rejects_while_pending() {
        let _g = fresh();
        let f = open().unwrap();
        let (h1, pitch, _) = dumb(f, 43);
        let (h2, _, _) = dumb(f, 44);
        let fb1 = fb_add(f, h1, 1024, 768, pitch, FORMAT_XRGB8888).unwrap();
        let fb2 = fb_add(f, h2, 1024, 768, pitch, FORMAT_XRGB8888).unwrap();
        set_crtc(f, fb1, 0, 0, MODE).unwrap();
        let s = page_flip(f, fb2, true, 0xabcd, 1000).unwrap();
        assert_eq!(s.inode, 44);
        assert_eq!(page_flip(f, fb1, true, 1, 1005), Err(-EBUSY));
        assert!(!has_ready_event(f, 1010));
        assert_eq!(take_event(f, 1010), Ok(None));
        assert!(has_ready_event(f, 1000 + FRAME_MS));
        let ev = take_event(f, 1020).unwrap().unwrap();
        assert_eq!((ev.user_data, ev.sequence), (0xabcd, 1));
        let bytes = flip_event_bytes(&ev, 1020);
        assert_eq!(u32::from_ne_bytes(bytes[0..4].try_into().unwrap()), 2);
        assert_eq!(u32::from_ne_bytes(bytes[4..8].try_into().unwrap()), 32);
        assert_eq!(u64::from_ne_bytes(bytes[8..16].try_into().unwrap()), 0xabcd);
        assert_eq!(u32::from_ne_bytes(bytes[28..32].try_into().unwrap()), CRTC_ID);
        // The next flip is allowed once the previous one completed.
        assert!(page_flip(f, fb1, false, 0, 1020).is_ok());
        assert_eq!(take_event(f, 5000), Ok(None), "no event requested");
    }

    #[test]
    fn page_flip_rejects_size_change_and_inactive_crtc() {
        let _g = fresh();
        let f = open().unwrap();
        let (h1, pitch, _) = dumb(f, 45);
        let fb1 = fb_add(f, h1, 1024, 768, pitch, FORMAT_XRGB8888).unwrap();
        let fb_small = fb_add(f, h1, 800, 600, pitch, FORMAT_XRGB8888).unwrap();
        assert_eq!(page_flip(f, fb1, false, 0, 0), Err(-EINVAL), "CRTC off");
        set_crtc(f, fb1, 0, 0, MODE).unwrap();
        assert_eq!(page_flip(f, fb_small, false, 0, 100), Err(-EINVAL));
    }

    #[test]
    fn closing_the_mode_setting_file_cleans_up_and_restores_console() {
        let _g = fresh();
        let f = open().unwrap();
        retain(f).unwrap(); // e.g. fork
        let (h1, pitch, _) = dumb(f, 46);
        let (_h2, _, _) = dumb(f, 47);
        let fb = fb_add(f, h1, 1024, 768, pitch, FORMAT_XRGB8888).unwrap();
        set_crtc(f, fb, 0, 0, MODE).unwrap();
        assert_eq!(release(f).count, 0, "still referenced");
        let r = release(f);
        assert!(r.restore_console);
        let mut inodes: Vec<u16> = r.iter().collect();
        inodes.sort();
        assert_eq!(inodes, vec![46, 47]);
        assert!(!crtc_state().active);
        assert!(open().is_ok());
    }

    #[test]
    fn prime_reimport_returns_existing_handle() {
        let _g = fresh();
        let a = open().unwrap();
        let b = open().unwrap();
        let (h, _, size) = dumb(a, 48);
        // Import into another file gets a new handle on the same inode.
        let (hb, fresh_b) = gem_add(b, 48, size).unwrap();
        assert!(fresh_b);
        assert_eq!(gem_lookup(b, hb), Ok((48, size)));
        // Re-import into the owner returns its existing handle.
        assert_eq!(gem_add(a, 48, size), Ok((h, false)));
    }

    #[test]
    fn dumb_mmap_offsets_resolve_only_inside_own_buffers() {
        let _g = fresh();
        let a = open().unwrap();
        let b = open().unwrap();
        let (h, _, size) = dumb(a, 49);
        let off = dumb_map_offset(a, h).unwrap();
        assert!(off >= DUMB_MMAP_BASE && off % 4096 == 0);
        assert_eq!(dumb_mmap_target(a, off, size), Ok((49, 0)));
        assert_eq!(dumb_mmap_target(a, off + 4096, 4096), Ok((49, 4096)));
        assert_eq!(dumb_mmap_target(a, off, size + 4096), Err(-EINVAL));
        assert_eq!(dumb_mmap_target(b, off, 4096), Err(-EINVAL));
        assert_eq!(dumb_mmap_target(a, 0, 4096), Err(-EINVAL));
    }

    #[test]
    fn modeinfo_encodes_size_refresh_and_name() {
        let b = modeinfo_bytes(MODE);
        assert_eq!(modeinfo_size(&b), (1024, 768));
        assert_eq!(u32::from_ne_bytes(b[24..28].try_into().unwrap()), 60);
        assert_eq!(&b[36..44], b"1024x768");
        assert_eq!(b[44], 0);
        let clock = u32::from_ne_bytes(b[0..4].try_into().unwrap());
        assert_eq!(clock, (1024 + 160) * (768 + 23) * 60 / 1000);
    }
}
