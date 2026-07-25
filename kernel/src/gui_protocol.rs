use core::cmp::{max, min};

const PEET_GUI_MAGIC: u32 = 0x5047_5432;
const PEET_GUI_VERSION: u16 = 2;
const PEET_GUI_MSG_CREATE: u16 = 1;
const PEET_GUI_MSG_ATTACH: u16 = 2;
const PEET_GUI_MSG_COMMIT: u16 = 3;
const PEET_GUI_MSG_DESTROY: u16 = 4;
const PEET_GUI_MSG_DAMAGE: u16 = 5;
const MAX_SCENE_SLOTS: usize = 4;

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum MessageKind {
    Create,
    Attach,
    Commit,
    Destroy,
    Damage,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum ProtocolError {
    InvalidHeader,
    UnknownMessageType,
    ClientIdMismatch,
    SurfaceIdMismatch,
    InvalidCreate,
    InvalidAttach,
    MissingBuffer,
    InvalidCommit,
    InvalidDestroy,
    InvalidDamage,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum ApplyResult {
    NoRedraw,
    Redraw,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
struct Rect {
    x: i32,
    y: i32,
    width: u32,
    height: u32,
}

impl Rect {
    const fn new(x: i32, y: i32, width: u32, height: u32) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }

    fn union(self, other: Self) -> Self {
        let left = min(self.x as i64, other.x as i64);
        let top = min(self.y as i64, other.y as i64);
        let right = max(
            self.x as i64 + self.width as i64,
            other.x as i64 + other.width as i64,
        );
        let bottom = max(
            self.y as i64 + self.height as i64,
            other.y as i64 + other.height as i64,
        );

        Self {
            x: left as i32,
            y: top as i32,
            width: (right - left) as u32,
            height: (bottom - top) as u32,
        }
    }

    fn intersect(self, other: Self) -> Option<Self> {
        let left = max(self.x as i64, other.x as i64);
        let top = max(self.y as i64, other.y as i64);
        let right = min(
            self.x as i64 + self.width as i64,
            other.x as i64 + other.width as i64,
        );
        let bottom = min(
            self.y as i64 + self.height as i64,
            other.y as i64 + other.height as i64,
        );

        if right <= left || bottom <= top {
            return None;
        }

        Some(Self {
            x: left as i32,
            y: top as i32,
            width: (right - left) as u32,
            height: (bottom - top) as u32,
        })
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
struct WireMessage {
    magic: u32,
    version: u16,
    kind: u16,
    client_id: u32,
    surface_id: u32,
    x: i32,
    y: i32,
    width: u32,
    height: u32,
    stride: u32,
}

impl WireMessage {
    fn create(client_id: u32, surface_id: u32, x: i32, y: i32, width: u32, height: u32) -> Self {
        Self {
            magic: PEET_GUI_MAGIC,
            version: PEET_GUI_VERSION,
            kind: PEET_GUI_MSG_CREATE,
            client_id,
            surface_id,
            x,
            y,
            width,
            height,
            stride: 0,
        }
    }

    fn attach(client_id: u32, surface_id: u32, width: u32, height: u32, stride: u32) -> Self {
        Self {
            magic: PEET_GUI_MAGIC,
            version: PEET_GUI_VERSION,
            kind: PEET_GUI_MSG_ATTACH,
            client_id,
            surface_id,
            x: 0,
            y: 0,
            width,
            height,
            stride,
        }
    }

    fn commit(client_id: u32, surface_id: u32) -> Self {
        Self {
            magic: PEET_GUI_MAGIC,
            version: PEET_GUI_VERSION,
            kind: PEET_GUI_MSG_COMMIT,
            client_id,
            surface_id,
            x: 0,
            y: 0,
            width: 0,
            height: 0,
            stride: 0,
        }
    }

    fn destroy(client_id: u32, surface_id: u32) -> Self {
        Self {
            magic: PEET_GUI_MAGIC,
            version: PEET_GUI_VERSION,
            kind: PEET_GUI_MSG_DESTROY,
            client_id,
            surface_id,
            x: 0,
            y: 0,
            width: 0,
            height: 0,
            stride: 0,
        }
    }

    fn damage(client_id: u32, surface_id: u32, x: i32, y: i32, width: u32, height: u32) -> Self {
        Self {
            magic: PEET_GUI_MAGIC,
            version: PEET_GUI_VERSION,
            kind: PEET_GUI_MSG_DAMAGE,
            client_id,
            surface_id,
            x,
            y,
            width,
            height,
            stride: 0,
        }
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
struct SceneSlot {
    client_id: u32,
    surface_id: u32,
    created: bool,
    committed: bool,
    x: i32,
    y: i32,
    width: u32,
    height: u32,
    pending_attached: bool,
    pending_stride: u32,
    committed_stride: u32,
}

const EMPTY_SLOT: SceneSlot = SceneSlot {
    client_id: 0,
    surface_id: 0,
    created: false,
    committed: false,
    x: 0,
    y: 0,
    width: 0,
    height: 0,
    pending_attached: false,
    pending_stride: 0,
    committed_stride: 0,
};

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
struct SceneState {
    slots: [SceneSlot; MAX_SCENE_SLOTS],
    pending_dirty: Option<Rect>,
    pointer_focus: Option<u32>,
    keyboard_focus: Option<u32>,
}

impl SceneState {
    const fn new() -> Self {
        Self {
            slots: [EMPTY_SLOT; MAX_SCENE_SLOTS],
            pending_dirty: None,
            pointer_focus: None,
            keyboard_focus: None,
        }
    }

    fn apply(
        &mut self,
        slot_index: usize,
        message: WireMessage,
        has_buffer_handle: bool,
    ) -> Result<ApplyResult, ProtocolError> {
        let kind = parse_kind(message)?;
        let slot = self.slots[slot_index];

        if slot.client_id != 0 && slot.client_id != message.client_id {
            return Err(ProtocolError::ClientIdMismatch);
        }
        if slot.surface_id != 0 && slot.surface_id != message.surface_id {
            return Err(ProtocolError::SurfaceIdMismatch);
        }

        let mut dirty = None;
        let mut clear_focus_client = None;
        let result = {
            let slot = &mut self.slots[slot_index];
            match kind {
                MessageKind::Create => {
                    if slot.created || message.width == 0 || message.height == 0 {
                        return Err(ProtocolError::InvalidCreate);
                    }
                    slot.client_id = message.client_id;
                    slot.surface_id = message.surface_id;
                    slot.created = true;
                    slot.committed = false;
                    slot.x = message.x;
                    slot.y = message.y;
                    slot.width = message.width;
                    slot.height = message.height;
                    slot.pending_attached = false;
                    slot.pending_stride = 0;
                    slot.committed_stride = 0;
                    ApplyResult::NoRedraw
                }
                MessageKind::Attach => {
                    if !slot.created
                        || slot.pending_attached
                        || message.stride < (slot.width * 4)
                        || (message.stride % 4) != 0
                    {
                        return Err(ProtocolError::InvalidAttach);
                    }
                    if !has_buffer_handle {
                        return Err(ProtocolError::MissingBuffer);
                    }
                    slot.pending_attached = true;
                    slot.pending_stride = message.stride;
                    ApplyResult::NoRedraw
                }
                MessageKind::Commit => {
                    if !slot.created || !slot.pending_attached {
                        return Err(ProtocolError::InvalidCommit);
                    }
                    slot.pending_attached = false;
                    slot.committed = true;
                    slot.committed_stride = slot.pending_stride;
                    slot.pending_stride = 0;
                    dirty = Some(surface_rect(*slot));
                    ApplyResult::Redraw
                }
                MessageKind::Destroy => {
                    if !slot.created {
                        return Err(ProtocolError::InvalidDestroy);
                    }
                    let redraw = slot.committed;
                    let client_id = slot.client_id;
                    if redraw {
                        dirty = Some(surface_rect(*slot));
                    }
                    slot.surface_id = 0;
                    slot.created = false;
                    slot.committed = false;
                    slot.x = 0;
                    slot.y = 0;
                    slot.width = 0;
                    slot.height = 0;
                    slot.pending_attached = false;
                    slot.pending_stride = 0;
                    slot.committed_stride = 0;
                    clear_focus_client = Some(client_id);
                    if redraw {
                        ApplyResult::Redraw
                    } else {
                        ApplyResult::NoRedraw
                    }
                }
                MessageKind::Damage => {
                    if !slot.created || !slot.committed || message.width == 0 || message.height == 0
                    {
                        return Err(ProtocolError::InvalidDamage);
                    }
                    if let Some(rect) = damage_rect(*slot, message) {
                        dirty = Some(rect);
                        ApplyResult::Redraw
                    } else {
                        ApplyResult::NoRedraw
                    }
                }
            }
        };

        if let Some(rect) = dirty {
            self.mark_dirty(rect);
        }
        if let Some(client_id) = clear_focus_client {
            self.clear_focus(client_id);
        }

        Ok(result)
    }

    fn visible_surface_ids(&self) -> [u32; MAX_SCENE_SLOTS] {
        let mut out = [0; MAX_SCENE_SLOTS];
        let mut count = 0;
        for slot in self.slots.iter() {
            if slot.committed {
                out[count] = slot.client_id;
                count += 1;
            }
        }
        out
    }

    fn visible_count(&self) -> usize {
        self.slots.iter().filter(|slot| slot.committed).count()
    }

    fn take_dirty(&mut self) -> Option<Rect> {
        let dirty = self.pending_dirty;
        self.pending_dirty = None;
        dirty
    }

    fn focus_for_point(&self, x: i32, y: i32) -> Option<u32> {
        self.slots
            .iter()
            .rev()
            .find(|slot| point_in_slot(**slot, x, y))
            .map(|slot| slot.client_id)
    }

    fn update_pointer_focus_for_point(&mut self, x: i32, y: i32) -> Option<u32> {
        let focus = self.focus_for_point(x, y);
        self.pointer_focus = focus;
        focus
    }

    fn activate_keyboard_focus_from_pointer(&mut self) -> Option<u32> {
        self.keyboard_focus = self.pointer_focus;
        self.keyboard_focus
    }

    fn assign_focus_for_point(&mut self, x: i32, y: i32) -> Option<u32> {
        let focus = self.update_pointer_focus_for_point(x, y);
        self.keyboard_focus = focus;
        focus
    }

    fn pointer_focus(&self) -> Option<u32> {
        self.pointer_focus
    }

    fn keyboard_focus(&self) -> Option<u32> {
        self.keyboard_focus
    }

    fn mark_dirty(&mut self, rect: Rect) {
        self.pending_dirty = Some(match self.pending_dirty {
            Some(existing) => existing.union(rect),
            None => rect,
        });
    }

    fn clear_focus(&mut self, client_id: u32) {
        if self.pointer_focus == Some(client_id) {
            self.pointer_focus = None;
        }
        if self.keyboard_focus == Some(client_id) {
            self.keyboard_focus = None;
        }
    }

    fn disconnect_slot(&mut self, slot_index: usize) -> ApplyResult {
        let slot = self.slots[slot_index];
        if slot.client_id == 0 {
            return ApplyResult::NoRedraw;
        }

        if slot.committed {
            self.mark_dirty(surface_rect(slot));
        }
        self.clear_focus(slot.client_id);
        self.slots[slot_index] = EMPTY_SLOT;

        if slot.committed {
            ApplyResult::Redraw
        } else {
            ApplyResult::NoRedraw
        }
    }
}

fn surface_rect(slot: SceneSlot) -> Rect {
    Rect::new(slot.x, slot.y, slot.width, slot.height)
}

fn point_in_slot(slot: SceneSlot, x: i32, y: i32) -> bool {
    slot.committed
        && x >= slot.x
        && y >= slot.y
        && (x as i64) < (slot.x as i64 + slot.width as i64)
        && (y as i64) < (slot.y as i64 + slot.height as i64)
}

fn damage_rect(slot: SceneSlot, message: WireMessage) -> Option<Rect> {
    let local_damage = Rect::new(message.x, message.y, message.width, message.height);
    let surface_bounds = Rect::new(0, 0, slot.width, slot.height);
    let clipped = local_damage.intersect(surface_bounds)?;

    Some(Rect::new(
        slot.x + clipped.x,
        slot.y + clipped.y,
        clipped.width,
        clipped.height,
    ))
}

fn parse_kind(message: WireMessage) -> Result<MessageKind, ProtocolError> {
    if message.magic != PEET_GUI_MAGIC
        || message.version != PEET_GUI_VERSION
        || message.client_id == 0
        || message.surface_id == 0
    {
        return Err(ProtocolError::InvalidHeader);
    }

    match message.kind {
        PEET_GUI_MSG_CREATE => Ok(MessageKind::Create),
        PEET_GUI_MSG_ATTACH => Ok(MessageKind::Attach),
        PEET_GUI_MSG_COMMIT => Ok(MessageKind::Commit),
        PEET_GUI_MSG_DESTROY => Ok(MessageKind::Destroy),
        PEET_GUI_MSG_DAMAGE => Ok(MessageKind::Damage),
        _ => Err(ProtocolError::UnknownMessageType),
    }
}

#[cfg(test)]
mod tests {
    use super::{
        ApplyResult, ProtocolError, Rect, SceneState, WireMessage, PEET_GUI_MAGIC,
        PEET_GUI_MSG_ATTACH, PEET_GUI_VERSION,
    };

    #[test]
    fn rejects_invalid_headers_and_unknown_types() {
        let mut scene = SceneState::new();
        let mut invalid = WireMessage::create(1, 1, 10, 20, 64, 48);
        invalid.magic = 0xDEAD_BEEF;
        assert_eq!(
            scene.apply(0, invalid, false),
            Err(ProtocolError::InvalidHeader)
        );

        let mut wrong_version = WireMessage::create(1, 1, 10, 20, 64, 48);
        wrong_version.version = PEET_GUI_VERSION + 1;
        assert_eq!(
            scene.apply(0, wrong_version, false),
            Err(ProtocolError::InvalidHeader)
        );

        let unknown = WireMessage {
            magic: PEET_GUI_MAGIC,
            version: PEET_GUI_VERSION,
            kind: 99,
            client_id: 7,
            surface_id: 1,
            x: 0,
            y: 0,
            width: 0,
            height: 0,
            stride: 0,
        };
        assert_eq!(
            scene.apply(0, unknown, false),
            Err(ProtocolError::UnknownMessageType)
        );
    }

    #[test]
    fn enforces_client_and_surface_identity() {
        let mut scene = SceneState::new();
        assert_eq!(
            scene.apply(0, WireMessage::create(11, 1, 4, 6, 32, 24), false),
            Ok(ApplyResult::NoRedraw)
        );
        assert_eq!(
            scene.apply(0, WireMessage::attach(12, 1, 32, 24, 128), true),
            Err(ProtocolError::ClientIdMismatch)
        );
        assert_eq!(
            scene.apply(0, WireMessage::attach(11, 2, 32, 24, 128), true),
            Err(ProtocolError::SurfaceIdMismatch)
        );
    }

    #[test]
    fn tracks_create_attach_commit_destroy_lifecycle() {
        let mut scene = SceneState::new();
        assert_eq!(scene.visible_count(), 0);

        assert_eq!(
            scene.apply(0, WireMessage::create(21, 1, 40, 50, 64, 32), false),
            Ok(ApplyResult::NoRedraw)
        );
        assert_eq!(
            scene.apply(0, WireMessage::attach(21, 1, 64, 32, 256), true),
            Ok(ApplyResult::NoRedraw)
        );
        assert_eq!(
            scene.apply(0, WireMessage::commit(21, 1), false),
            Ok(ApplyResult::Redraw)
        );
        assert_eq!(scene.visible_count(), 1);
        assert_eq!(scene.visible_surface_ids()[0], 21);
        assert_eq!(scene.take_dirty(), Some(Rect::new(40, 50, 64, 32)));

        assert_eq!(
            scene.apply(0, WireMessage::destroy(21, 1), false),
            Ok(ApplyResult::Redraw)
        );
        assert_eq!(scene.visible_count(), 0);
        assert_eq!(scene.take_dirty(), Some(Rect::new(40, 50, 64, 32)));

        assert_eq!(
            scene.apply(0, WireMessage::commit(21, 1), false),
            Err(ProtocolError::InvalidCommit)
        );
    }

    #[test]
    fn rejects_missing_buffer_and_bad_attach_sequence() {
        let mut scene = SceneState::new();
        assert_eq!(
            scene.apply(0, WireMessage::create(31, 1, 0, 0, 48, 16), false),
            Ok(ApplyResult::NoRedraw)
        );

        assert_eq!(
            scene.apply(0, WireMessage::attach(31, 1, 48, 16, 192), false),
            Err(ProtocolError::MissingBuffer)
        );
        assert_eq!(
            scene.apply(0, WireMessage::commit(31, 1), false),
            Err(ProtocolError::InvalidCommit)
        );

        let mut bad_stride = WireMessage::attach(31, 1, 48, 16, 190);
        bad_stride.kind = PEET_GUI_MSG_ATTACH;
        assert_eq!(
            scene.apply(0, bad_stride, true),
            Err(ProtocolError::InvalidAttach)
        );
    }

    #[test]
    fn clips_damage_to_surface_bounds() {
        let mut scene = SceneState::new();
        assert_eq!(
            scene.apply(0, WireMessage::create(41, 1, 40, 50, 64, 32), false),
            Ok(ApplyResult::NoRedraw)
        );
        assert_eq!(
            scene.apply(0, WireMessage::attach(41, 1, 64, 32, 256), true),
            Ok(ApplyResult::NoRedraw)
        );
        assert_eq!(
            scene.apply(0, WireMessage::commit(41, 1), false),
            Ok(ApplyResult::Redraw)
        );
        assert_eq!(scene.take_dirty(), Some(Rect::new(40, 50, 64, 32)));

        assert_eq!(
            scene.apply(0, WireMessage::damage(41, 1, 60, 20, 20, 20), false),
            Ok(ApplyResult::Redraw)
        );
        assert_eq!(scene.take_dirty(), Some(Rect::new(100, 70, 4, 12)));

        assert_eq!(
            scene.apply(0, WireMessage::damage(41, 1, 80, 40, 8, 8), false),
            Ok(ApplyResult::NoRedraw)
        );
        assert_eq!(scene.take_dirty(), None);
    }

    #[test]
    fn coalesces_multiple_damage_rectangles_until_presented() {
        let mut scene = SceneState::new();
        assert_eq!(
            scene.apply(0, WireMessage::create(51, 1, 10, 20, 40, 30), false),
            Ok(ApplyResult::NoRedraw)
        );
        assert_eq!(
            scene.apply(0, WireMessage::attach(51, 1, 40, 30, 160), true),
            Ok(ApplyResult::NoRedraw)
        );
        assert_eq!(
            scene.apply(0, WireMessage::commit(51, 1), false),
            Ok(ApplyResult::Redraw)
        );
        assert_eq!(scene.take_dirty(), Some(Rect::new(10, 20, 40, 30)));

        assert_eq!(
            scene.apply(0, WireMessage::damage(51, 1, 4, 6, 8, 7), false),
            Ok(ApplyResult::Redraw)
        );
        assert_eq!(
            scene.apply(0, WireMessage::damage(51, 1, 18, 10, 6, 9), false),
            Ok(ApplyResult::Redraw)
        );
        assert_eq!(scene.take_dirty(), Some(Rect::new(14, 26, 20, 13)));
        assert_eq!(scene.take_dirty(), None);
    }

    #[test]
    fn rejects_damage_before_first_commit() {
        let mut scene = SceneState::new();
        assert_eq!(
            scene.apply(0, WireMessage::create(61, 1, 0, 0, 32, 32), false),
            Ok(ApplyResult::NoRedraw)
        );
        assert_eq!(
            scene.apply(0, WireMessage::damage(61, 1, 0, 0, 8, 8), false),
            Err(ProtocolError::InvalidDamage)
        );
    }

    #[test]
    fn keeps_stacking_order_stable_by_slot() {
        let mut scene = SceneState::new();

        assert_eq!(
            scene.apply(1, WireMessage::create(102, 1, 80, 0, 32, 32), false),
            Ok(ApplyResult::NoRedraw)
        );
        assert_eq!(
            scene.apply(1, WireMessage::attach(102, 1, 32, 32, 128), true),
            Ok(ApplyResult::NoRedraw)
        );
        assert_eq!(
            scene.apply(1, WireMessage::commit(102, 1), false),
            Ok(ApplyResult::Redraw)
        );

        assert_eq!(
            scene.apply(0, WireMessage::create(101, 1, 0, 0, 32, 32), false),
            Ok(ApplyResult::NoRedraw)
        );
        assert_eq!(
            scene.apply(0, WireMessage::attach(101, 1, 32, 32, 128), true),
            Ok(ApplyResult::NoRedraw)
        );
        assert_eq!(
            scene.apply(0, WireMessage::commit(101, 1), false),
            Ok(ApplyResult::Redraw)
        );

        assert_eq!(scene.visible_surface_ids(), [101, 102, 0, 0]);
        assert_eq!(
            scene.apply(0, WireMessage::destroy(101, 1), false),
            Ok(ApplyResult::Redraw)
        );
        assert_eq!(scene.visible_surface_ids(), [102, 0, 0, 0]);
    }

    #[test]
    fn hover_focus_tracks_topmost_surface_without_immediate_keyboard_activation() {
        let mut scene = SceneState::new();

        assert_eq!(
            scene.apply(0, WireMessage::create(71, 1, 40, 40, 48, 48), false),
            Ok(ApplyResult::NoRedraw)
        );
        assert_eq!(
            scene.apply(0, WireMessage::attach(71, 1, 48, 48, 192), true),
            Ok(ApplyResult::NoRedraw)
        );
        assert_eq!(
            scene.apply(0, WireMessage::commit(71, 1), false),
            Ok(ApplyResult::Redraw)
        );
        scene.take_dirty();

        assert_eq!(
            scene.apply(1, WireMessage::create(72, 1, 56, 56, 48, 48), false),
            Ok(ApplyResult::NoRedraw)
        );
        assert_eq!(
            scene.apply(1, WireMessage::attach(72, 1, 48, 48, 192), true),
            Ok(ApplyResult::NoRedraw)
        );
        assert_eq!(
            scene.apply(1, WireMessage::commit(72, 1), false),
            Ok(ApplyResult::Redraw)
        );
        scene.take_dirty();

        assert_eq!(scene.update_pointer_focus_for_point(60, 60), Some(72));
        assert_eq!(scene.pointer_focus(), Some(72));
        assert_eq!(scene.keyboard_focus(), None);
        assert_eq!(scene.update_pointer_focus_for_point(45, 45), Some(71));
        assert_eq!(scene.pointer_focus(), Some(71));
        assert_eq!(scene.keyboard_focus(), None);
        assert_eq!(scene.activate_keyboard_focus_from_pointer(), Some(71));
        assert_eq!(scene.keyboard_focus(), Some(71));
        assert_eq!(scene.update_pointer_focus_for_point(8, 8), None);
        assert_eq!(scene.pointer_focus(), None);
        assert_eq!(scene.keyboard_focus(), Some(71));
        assert_eq!(scene.activate_keyboard_focus_from_pointer(), None);
        assert_eq!(scene.keyboard_focus(), None);
    }

    #[test]
    fn clears_focus_when_focused_surface_is_destroyed() {
        let mut scene = SceneState::new();

        assert_eq!(
            scene.apply(0, WireMessage::create(81, 1, 12, 18, 32, 24), false),
            Ok(ApplyResult::NoRedraw)
        );
        assert_eq!(
            scene.apply(0, WireMessage::attach(81, 1, 32, 24, 128), true),
            Ok(ApplyResult::NoRedraw)
        );
        assert_eq!(
            scene.apply(0, WireMessage::commit(81, 1), false),
            Ok(ApplyResult::Redraw)
        );
        scene.take_dirty();
        assert_eq!(scene.assign_focus_for_point(16, 20), Some(81));

        assert_eq!(
            scene.apply(0, WireMessage::destroy(81, 1), false),
            Ok(ApplyResult::Redraw)
        );
        assert_eq!(scene.pointer_focus(), None);
        assert_eq!(scene.keyboard_focus(), None);
    }

    #[test]
    fn clears_focus_when_focused_slot_disconnects() {
        let mut scene = SceneState::new();

        assert_eq!(
            scene.apply(0, WireMessage::create(91, 1, 24, 32, 24, 24), false),
            Ok(ApplyResult::NoRedraw)
        );
        assert_eq!(
            scene.apply(0, WireMessage::attach(91, 1, 24, 24, 96), true),
            Ok(ApplyResult::NoRedraw)
        );
        assert_eq!(
            scene.apply(0, WireMessage::commit(91, 1), false),
            Ok(ApplyResult::Redraw)
        );
        assert_eq!(scene.take_dirty(), Some(Rect::new(24, 32, 24, 24)));
        assert_eq!(scene.assign_focus_for_point(28, 36), Some(91));

        assert_eq!(scene.disconnect_slot(0), ApplyResult::Redraw);
        assert_eq!(scene.take_dirty(), Some(Rect::new(24, 32, 24, 24)));
        assert_eq!(scene.pointer_focus(), None);
        assert_eq!(scene.keyboard_focus(), None);
        assert_eq!(scene.visible_count(), 0);
    }
}
