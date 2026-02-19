use drivers::pci;

const USB_CLASS_SERIAL_BUS: u8 = 0x0C;
const USB_SUBCLASS_USB: u8 = 0x03;
const USB_PROG_IF_UHCI: u8 = 0x00;

const USB_DESCRIPTOR_DEVICE: u8 = 0x01;
const USB_DESCRIPTOR_CONFIG: u8 = 0x02;
const USB_DESCRIPTOR_INTERFACE: u8 = 0x04;
const USB_DESCRIPTOR_ENDPOINT: u8 = 0x05;
const USB_DESCRIPTOR_HID: u8 = 0x21;
const USB_DESCRIPTOR_REPORT: u8 = 0x22;

const USB_REQ_GET_DESCRIPTOR: u8 = 0x06;
const USB_REQ_SET_ADDRESS: u8 = 0x05;
const USB_REQ_SET_CONFIGURATION: u8 = 0x09;
const USB_REQ_SET_IDLE: u8 = 0x0A;

const HID_CLASS: u8 = 0x03;
const HID_USAGE_PAGE_GENERIC_DESKTOP: u16 = 0x01;
const HID_USAGE_PAGE_BUTTON: u16 = 0x09;
const HID_USAGE_X: u16 = 0x30;
const HID_USAGE_Y: u16 = 0x31;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct HidTabletInterface {
    config_value: u8,
    interface_number: u8,
    endpoint_address: u8,
    endpoint_max_packet: u16,
    poll_interval: u8,
    report_descriptor_len: usize,
    subclass: u8,
    protocol: u8,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct AxisField {
    bit_offset: u16,
    bit_size: u8,
    logical_min: i32,
    logical_max: i32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ButtonField {
    bit_offset: u16,
    bit_count: u8,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct TabletReportLayout {
    report_id: Option<u8>,
    x: AxisField,
    y: AxisField,
    buttons: Option<ButtonField>,
    report_bytes: u8,
}

impl TabletReportLayout {
    const fn qemu_default() -> Self {
        Self {
            report_id: None,
            x: AxisField {
                bit_offset: 8,
                bit_size: 16,
                logical_min: 0,
                logical_max: 0x7FFF,
            },
            y: AxisField {
                bit_offset: 24,
                bit_size: 16,
                logical_min: 0,
                logical_max: 0x7FFF,
            },
            buttons: Some(ButtonField {
                bit_offset: 0,
                bit_count: 3,
            }),
            report_bytes: 5,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct TabletSample {
    x: i32,
    y: i32,
    buttons: u8,
}

fn parse_hid_tablet_interface(config_bytes: &[u8]) -> Option<HidTabletInterface> {
    if config_bytes.len() < 9 || config_bytes[1] != USB_DESCRIPTOR_CONFIG {
        return None;
    }

    let config_value = config_bytes[5];
    let mut offset = 0usize;
    let mut active_interface: Option<(u8, u8, u8)> = None;
    let mut hid_report_len = 0usize;
    let mut endpoint: Option<(u8, u16, u8)> = None;

    while offset + 2 <= config_bytes.len() {
        let len = config_bytes[offset] as usize;
        if len < 2 || offset + len > config_bytes.len() {
            break;
        }

        let dtype = config_bytes[offset + 1];
        match dtype {
            USB_DESCRIPTOR_INTERFACE if len >= 9 => {
                let class = config_bytes[offset + 5];
                if class == HID_CLASS {
                    active_interface = Some((
                        config_bytes[offset + 2],
                        config_bytes[offset + 6],
                        config_bytes[offset + 7],
                    ));
                    hid_report_len = 0;
                    endpoint = None;
                } else {
                    active_interface = None;
                    hid_report_len = 0;
                    endpoint = None;
                }
            }
            USB_DESCRIPTOR_HID if len >= 9 && active_interface.is_some() => {
                // HID descriptor list: bNumDescriptors + (type, len) pairs.
                let mut cursor = offset + 6;
                let list_end = offset + len;
                while cursor + 3 <= list_end {
                    let desc_type = config_bytes[cursor];
                    let desc_len = u16::from_le_bytes([
                        config_bytes[cursor + 1],
                        config_bytes[cursor + 2],
                    ]) as usize;
                    if desc_type == USB_DESCRIPTOR_REPORT {
                        hid_report_len = desc_len;
                        break;
                    }
                    cursor += 3;
                }
            }
            USB_DESCRIPTOR_ENDPOINT if len >= 7 && active_interface.is_some() => {
                let addr = config_bytes[offset + 2];
                let attrs = config_bytes[offset + 3];
                if (addr & 0x80) != 0 && (attrs & 0x03) == 0x03 {
                    endpoint = Some((
                        addr,
                        u16::from_le_bytes([config_bytes[offset + 4], config_bytes[offset + 5]]),
                        config_bytes[offset + 6],
                    ));
                    break;
                }
            }
            _ => {}
        }

        offset += len;
    }

    let ((interface_number, subclass, protocol), (endpoint_address, endpoint_max_packet, poll_interval)) =
        (active_interface?, endpoint?);

    Some(HidTabletInterface {
        config_value,
        interface_number,
        endpoint_address,
        endpoint_max_packet,
        poll_interval,
        report_descriptor_len: hid_report_len,
        subclass,
        protocol,
    })
}

#[cfg(test)]
fn hid_value_u32(data: &[u8], size: usize) -> u32 {
    match size {
        0 => 0,
        1 => data[0] as u32,
        2 => u16::from_le_bytes([data[0], data[1]]) as u32,
        _ => u32::from_le_bytes([data[0], data[1], data[2], data[3]]),
    }
}

#[cfg(test)]
fn hid_value_i32(data: &[u8], size: usize) -> i32 {
    match size {
        0 => 0,
        1 => (data[0] as i8) as i32,
        2 => i16::from_le_bytes([data[0], data[1]]) as i32,
        _ => i32::from_le_bytes([data[0], data[1], data[2], data[3]]),
    }
}

#[cfg(test)]
fn resolve_local_usage(
    usages: &[u16; 16],
    usage_count: usize,
    usage_min: Option<u16>,
    usage_max: Option<u16>,
    index: u32,
) -> Option<u16> {
    if usage_count > 0 {
        let idx = core::cmp::min(index as usize, usage_count.saturating_sub(1));
        return Some(usages[idx]);
    }

    if let (Some(min), Some(max)) = (usage_min, usage_max) {
        let candidate = min.saturating_add(index as u16);
        if candidate <= max {
            return Some(candidate);
        }
    }

    None
}

#[cfg(test)]
fn parse_hid_report_layout(descriptor: &[u8]) -> Option<TabletReportLayout> {
    let mut usage_page: u16 = 0;
    let mut logical_min: i32 = 0;
    let mut logical_max: i32 = 0;
    let mut report_size: u32 = 0;
    let mut report_count: u32 = 0;

    let mut usages = [0u16; 16];
    let mut usage_count = 0usize;
    let mut usage_min: Option<u16> = None;
    let mut usage_max: Option<u16> = None;

    let mut report_id: Option<u8> = None;
    let mut bit_offset: u32 = 0;

    let mut x_field: Option<AxisField> = None;
    let mut y_field: Option<AxisField> = None;
    let mut button_field: Option<ButtonField> = None;

    let mut i = 0usize;
    while i < descriptor.len() {
        let prefix = descriptor[i];
        i += 1;

        if prefix == 0xFE {
            if i + 2 > descriptor.len() {
                break;
            }
            let len = descriptor[i] as usize;
            i = i.saturating_add(2).saturating_add(len);
            continue;
        }

        let size_code = (prefix & 0x03) as usize;
        let size = if size_code == 3 { 4 } else { size_code };
        let item_type = (prefix >> 2) & 0x03;
        let tag = (prefix >> 4) & 0x0F;

        if i + size > descriptor.len() {
            break;
        }

        let value = &descriptor[i..i + size];
        i += size;

        match item_type {
            0x01 => match tag {
                0x0 => usage_page = hid_value_u32(value, size) as u16,
                0x1 => logical_min = hid_value_i32(value, size),
                0x2 => logical_max = hid_value_i32(value, size),
                0x7 => report_size = hid_value_u32(value, size),
                0x8 => report_id = Some(hid_value_u32(value, size) as u8),
                0x9 => report_count = hid_value_u32(value, size),
                _ => {}
            },
            0x02 => match tag {
                0x0 => {
                    if usage_count < usages.len() {
                        usages[usage_count] = hid_value_u32(value, size) as u16;
                        usage_count += 1;
                    }
                }
                0x1 => usage_min = Some(hid_value_u32(value, size) as u16),
                0x2 => usage_max = Some(hid_value_u32(value, size) as u16),
                _ => {}
            },
            0x00 if tag == 0x8 => {
                // Input(main) item.
                let flags = hid_value_u32(value, size);
                let is_const = (flags & 0x01) != 0;
                let is_variable = (flags & 0x02) != 0;
                let is_relative = (flags & 0x04) != 0;

                if report_count == 0 || report_size == 0 {
                    usage_count = 0;
                    usage_min = None;
                    usage_max = None;
                    continue;
                }

                if !is_const {
                    for field_index in 0..report_count {
                        let field_offset = bit_offset.saturating_add(field_index * report_size);
                        let usage = resolve_local_usage(
                            &usages,
                            usage_count,
                            usage_min,
                            usage_max,
                            field_index,
                        );

                        if !is_variable {
                            continue;
                        }

                        if usage_page == HID_USAGE_PAGE_GENERIC_DESKTOP && !is_relative {
                            if let Some(HID_USAGE_X) = usage {
                                if x_field.is_none() {
                                    x_field = Some(AxisField {
                                        bit_offset: field_offset as u16,
                                        bit_size: report_size as u8,
                                        logical_min,
                                        logical_max,
                                    });
                                }
                            } else if let Some(HID_USAGE_Y) = usage {
                                if y_field.is_none() {
                                    y_field = Some(AxisField {
                                        bit_offset: field_offset as u16,
                                        bit_size: report_size as u8,
                                        logical_min,
                                        logical_max,
                                    });
                                }
                            }
                        } else if usage_page == HID_USAGE_PAGE_BUTTON && report_size == 1 {
                            if usage.is_some() {
                                match button_field {
                                    Some(mut field) => {
                                        let expected = field.bit_offset as u32 + field.bit_count as u32;
                                        if field_offset == expected && field.bit_count < 8 {
                                            field.bit_count += 1;
                                            button_field = Some(field);
                                        }
                                    }
                                    None => {
                                        button_field = Some(ButtonField {
                                            bit_offset: field_offset as u16,
                                            bit_count: 1,
                                        });
                                    }
                                }
                            }
                        }
                    }
                }

                bit_offset = bit_offset.saturating_add(report_count.saturating_mul(report_size));
                usage_count = 0;
                usage_min = None;
                usage_max = None;
            }
            0x00 => {
                // Other main items reset local state.
                usage_count = 0;
                usage_min = None;
                usage_max = None;
            }
            _ => {}
        }
    }

    let x = x_field?;
    let y = y_field?;
    let mut report_bytes = ((bit_offset + 7) / 8) as u8;
    if report_id.is_some() {
        report_bytes = report_bytes.saturating_add(1);
    }

    Some(TabletReportLayout {
        report_id,
        x,
        y,
        buttons: button_field,
        report_bytes,
    })
}

#[cfg(test)]
fn extract_bits_le(data: &[u8], bit_offset: u16, bit_size: u8) -> Option<u32> {
    if bit_size == 0 || bit_size > 32 {
        return None;
    }

    let mut value = 0u32;
    for bit in 0..bit_size {
        let absolute = bit_offset as usize + bit as usize;
        let byte_index = absolute / 8;
        if byte_index >= data.len() {
            return None;
        }
        let bit_index = absolute % 8;
        let bit_value = (data[byte_index] >> bit_index) & 0x1;
        value |= (bit_value as u32) << bit;
    }
    Some(value)
}

#[cfg(test)]
fn sign_extend(value: u32, bits: u8) -> i32 {
    if bits == 0 || bits >= 32 {
        return value as i32;
    }
    let shift = 32 - bits;
    ((value << shift) as i32) >> shift
}

#[cfg(test)]
fn decode_tablet_report(layout: TabletReportLayout, report: &[u8]) -> Option<TabletSample> {
    let payload = if let Some(id) = layout.report_id {
        if report.first().copied()? != id {
            return None;
        }
        report.get(1..)?
    } else {
        report
    };

    let x_raw = extract_bits_le(payload, layout.x.bit_offset, layout.x.bit_size)?;
    let y_raw = extract_bits_le(payload, layout.y.bit_offset, layout.y.bit_size)?;

    let x = if layout.x.logical_min < 0 {
        sign_extend(x_raw, layout.x.bit_size)
    } else {
        x_raw as i32
    };
    let y = if layout.y.logical_min < 0 {
        sign_extend(y_raw, layout.y.bit_size)
    } else {
        y_raw as i32
    };

    let buttons = if let Some(field) = layout.buttons {
        let mut out = 0u8;
        for idx in 0..field.bit_count {
            let bit = extract_bits_le(payload, field.bit_offset + idx as u16, 1)? as u8;
            out |= bit << idx;
        }
        out
    } else {
        0
    };

    Some(TabletSample { x, y, buttons })
}

fn decode_qemu_tablet_report(report: &[u8]) -> Option<TabletSample> {
    if report.len() < 5 {
        return None;
    }
    Some(TabletSample {
        buttons: report[0] & 0x07,
        x: u16::from_le_bytes([report[1], report[2]]) as i32,
        y: u16::from_le_bytes([report[3], report[4]]) as i32,
    })
}

#[cfg(target_os = "none")]
mod hw {
    use core::mem;
    use core::ptr;

    use arch_x86_64::addr::{VirtAddr, PAGE_SIZE};
    use arch_x86_64::port;

    use super::*;
    use crate::memory::{heap, vmm};
    use crate::mouse;
    use crate::pit;
    use crate::sync::SpinLock;

    const UHCI_REG_USBCMD: u16 = 0x00;
    const UHCI_REG_USBSTS: u16 = 0x02;
    const UHCI_REG_USBINTR: u16 = 0x04;
    const UHCI_REG_FRNUM: u16 = 0x06;
    const UHCI_REG_FLBASEADD: u16 = 0x08;
    const UHCI_REG_SOFMOD: u16 = 0x0C;
    const UHCI_REG_PORTSC1: u16 = 0x10;

    const UHCI_USBCMD_RUN: u16 = 1 << 0;
    const UHCI_USBCMD_HCRESET: u16 = 1 << 1;
    const UHCI_USBCMD_CONFIGURE: u16 = 1 << 6;

    const UHCI_USBSTS_HALTED: u16 = 1 << 5;

    const UHCI_PORTSC_CCS: u16 = 1 << 0;
    const UHCI_PORTSC_CSC: u16 = 1 << 1;
    const UHCI_PORTSC_PE: u16 = 1 << 2;
    const UHCI_PORTSC_PEC: u16 = 1 << 3;
    const UHCI_PORTSC_LSDA: u16 = 1 << 8;
    const UHCI_PORTSC_PR: u16 = 1 << 9;

    const UHCI_LINK_TERMINATE: u32 = 1 << 0;
    const UHCI_LINK_QH: u32 = 1 << 1;

    const UHCI_TD_STATUS_ACTIVE: u32 = 1 << 23;
    const UHCI_TD_STATUS_LOW_SPEED: u32 = 1 << 26;
    const UHCI_TD_STATUS_CERR_3: u32 = 3 << 27;
    const UHCI_TD_STATUS_SPD: u32 = 1 << 29;
    const UHCI_TD_STATUS_NAK: u32 = 1 << 19;
    const UHCI_TD_STATUS_ERROR_MASK: u32 = (1 << 17) | (1 << 18) | (1 << 20) | (1 << 21) | (1 << 22);

    const UHCI_PID_OUT: u8 = 0xE1;
    const UHCI_PID_IN: u8 = 0x69;
    const UHCI_PID_SETUP: u8 = 0x2D;

    const USB_CTRL_TIMEOUT: usize = 800_000;
    const USB_INTR_TIMEOUT: usize = 120_000;
    const USB_ENUM_PORTS: usize = 2;
    const USB_TD_POOL_COUNT: usize = 32;
    const USB_DATA_BUF_BYTES: usize = 512;
    const USB_INTR_BUF_BYTES: usize = 64;

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct SetupPacket {
        request_type: u8,
        request: u8,
        value: u16,
        index: u16,
        length: u16,
    }

    #[repr(C, align(16))]
    #[derive(Clone, Copy)]
    struct UhciQh {
        head: u32,
        element: u32,
    }

    #[repr(C, align(16))]
    #[derive(Clone, Copy)]
    struct UhciTd {
        link_ptr: u32,
        status: u32,
        token: u32,
        buffer: u32,
    }

    #[derive(Clone, Copy)]
    struct DmaRegion {
        virt: *mut u8,
        phys: u32,
        size: usize,
    }

    impl DmaRegion {
        const fn empty() -> Self {
            Self {
                virt: ptr::null_mut(),
                phys: 0,
                size: 0,
            }
        }
    }

    unsafe impl Send for DmaRegion {}

    #[derive(Clone, Copy)]
    struct UsbTabletDriver {
        active: bool,
        io_base: u16,
        low_speed: bool,
        device_address: u8,
        ep0_max_packet: u8,
        endpoint_address: u8,
        endpoint_max_packet: u16,
        poll_interval_ms: u8,
        report_layout: TabletReportLayout,
        interrupt_toggle: u8,
        last_poll_ms: u64,
        last_report_len: usize,
        last_report: [u8; USB_INTR_BUF_BYTES],
        frame_list: DmaRegion,
        qh: DmaRegion,
        td_pool: DmaRegion,
        setup_buf: DmaRegion,
        data_buf: DmaRegion,
        intr_buf: DmaRegion,
    }

    impl UsbTabletDriver {
        const fn empty() -> Self {
            Self {
                active: false,
                io_base: 0,
                low_speed: false,
                device_address: 0,
                ep0_max_packet: 8,
                endpoint_address: 0,
                endpoint_max_packet: 0,
                poll_interval_ms: 10,
                report_layout: TabletReportLayout::qemu_default(),
                interrupt_toggle: 0,
                last_poll_ms: 0,
                last_report_len: 0,
                last_report: [0; USB_INTR_BUF_BYTES],
                frame_list: DmaRegion::empty(),
                qh: DmaRegion::empty(),
                td_pool: DmaRegion::empty(),
                setup_buf: DmaRegion::empty(),
                data_buf: DmaRegion::empty(),
                intr_buf: DmaRegion::empty(),
            }
        }
    }

    unsafe impl Send for UsbTabletDriver {}

    static DRIVER_STATE: SpinLock<UsbTabletDriver> = SpinLock::new(UsbTabletDriver::empty());

    #[inline]
    fn read16(io_base: u16, reg: u16) -> u16 {
        unsafe { port::inw(io_base + reg) }
    }

    #[inline]
    fn write16(io_base: u16, reg: u16, value: u16) {
        unsafe { port::outw(io_base + reg, value) }
    }

    #[inline]
    fn write32(io_base: u16, reg: u16, value: u32) {
        unsafe { port::outl(io_base + reg, value) }
    }

    #[inline]
    fn write8(io_base: u16, reg: u16, value: u8) {
        unsafe { port::outb(io_base + reg, value) }
    }

    fn spin_delay(mut n: usize) {
        while n > 0 {
            core::hint::spin_loop();
            n -= 1;
        }
    }

    fn td_max_len_code(len: usize) -> u32 {
        if len == 0 {
            0x7FF
        } else {
            (len as u32).saturating_sub(1) & 0x7FF
        }
    }

    fn build_td_token(pid: u8, addr: u8, endpoint: u8, toggle: u8, max_len: usize) -> u32 {
        (pid as u32)
            | ((addr as u32 & 0x7F) << 8)
            | ((endpoint as u32 & 0x0F) << 15)
            | ((toggle as u32 & 0x1) << 19)
            | (td_max_len_code(max_len) << 21)
    }

    fn td_phys(base: u32, index: usize) -> u32 {
        base + (index * mem::size_of::<UhciTd>()) as u32
    }

    fn qh_ptr(driver: &UsbTabletDriver) -> *mut UhciQh {
        driver.qh.virt as *mut UhciQh
    }

    fn td_ptr(driver: &UsbTabletDriver, index: usize) -> *mut UhciTd {
        unsafe { (driver.td_pool.virt as *mut UhciTd).add(index) }
    }

    fn set_qh_element(driver: &UsbTabletDriver, element: u32) {
        unsafe {
            let qh = qh_ptr(driver);
            ptr::write_volatile(&mut (*qh).element, element);
        }
    }

    fn write_td(driver: &UsbTabletDriver, index: usize, td: UhciTd) {
        unsafe {
            ptr::write_volatile(td_ptr(driver, index), td);
        }
    }

    fn read_td_status(driver: &UsbTabletDriver, index: usize) -> u32 {
        unsafe { ptr::read_volatile(&(*td_ptr(driver, index)).status) }
    }

    fn alloc_dma_region(size: usize, align: usize) -> Option<DmaRegion> {
        if size == 0 || size > PAGE_SIZE as usize {
            return None;
        }

        let ptr = heap::kmalloc_aligned(size, align);
        if ptr.is_null() {
            return None;
        }

        unsafe {
            ptr::write_bytes(ptr, 0, size);
        }

        let start = VirtAddr::new(ptr as u64);
        let end = VirtAddr::new(ptr as u64 + size as u64 - 1);
        let start_phys = vmm::get_physical(start)?;
        let end_phys = vmm::get_physical(end)?;
        if (start_phys.as_u64() & !(PAGE_SIZE - 1)) != (end_phys.as_u64() & !(PAGE_SIZE - 1)) {
            heap::kfree(ptr);
            return None;
        }
        if start_phys.as_u64() > u32::MAX as u64 {
            heap::kfree(ptr);
            return None;
        }

        Some(DmaRegion {
            virt: ptr,
            phys: start_phys.as_u64() as u32,
            size,
        })
    }

    fn free_dma_region(region: DmaRegion) {
        if !region.virt.is_null() {
            heap::kfree(region.virt);
        }
    }

    fn clear_driver_resources(driver: &mut UsbTabletDriver) {
        free_dma_region(driver.intr_buf);
        free_dma_region(driver.data_buf);
        free_dma_region(driver.setup_buf);
        free_dma_region(driver.td_pool);
        free_dma_region(driver.qh);
        free_dma_region(driver.frame_list);
        *driver = UsbTabletDriver::empty();
    }

    fn uhci_reset(io_base: u16) -> bool {
        write16(io_base, UHCI_REG_USBCMD, UHCI_USBCMD_HCRESET);
        for _ in 0..200_000 {
            if (read16(io_base, UHCI_REG_USBCMD) & UHCI_USBCMD_HCRESET) == 0 {
                return true;
            }
        }
        false
    }

    fn uhci_start(io_base: u16, frame_list_phys: u32) -> bool {
        write16(io_base, UHCI_REG_USBCMD, 0);
        write16(io_base, UHCI_REG_USBINTR, 0);
        write16(io_base, UHCI_REG_USBSTS, 0x003F);
        write16(io_base, UHCI_REG_FRNUM, 0);
        write32(io_base, UHCI_REG_FLBASEADD, frame_list_phys);
        write8(io_base, UHCI_REG_SOFMOD, 64);

        write16(
            io_base,
            UHCI_REG_USBCMD,
            UHCI_USBCMD_CONFIGURE | UHCI_USBCMD_RUN,
        );

        for _ in 0..40_000 {
            if (read16(io_base, UHCI_REG_USBSTS) & UHCI_USBSTS_HALTED) == 0 {
                return true;
            }
        }
        false
    }

    fn uhci_reset_port(io_base: u16, port_index: usize) -> Option<bool> {
        let port_reg = UHCI_REG_PORTSC1 + (port_index as u16 * 2);
        let status = read16(io_base, port_reg);
        if status == 0xFFFF || (status & UHCI_PORTSC_CCS) == 0 {
            return None;
        }

        write16(io_base, port_reg, status | UHCI_PORTSC_CSC | UHCI_PORTSC_PEC);
        write16(io_base, port_reg, status | UHCI_PORTSC_PR);
        spin_delay(350_000);

        let after_reset = read16(io_base, port_reg) & !UHCI_PORTSC_PR;
        write16(io_base, port_reg, after_reset);
        spin_delay(120_000);

        let mut current = read16(io_base, port_reg);
        write16(io_base, port_reg, current | UHCI_PORTSC_CSC | UHCI_PORTSC_PEC);
        current = read16(io_base, port_reg);

        if (current & UHCI_PORTSC_CCS) == 0 {
            return None;
        }

        write16(io_base, port_reg, current | UHCI_PORTSC_PE);
        spin_delay(60_000);

        current = read16(io_base, port_reg);
        if (current & UHCI_PORTSC_PE) == 0 {
            return None;
        }

        Some((current & UHCI_PORTSC_LSDA) != 0)
    }

    fn setup_schedule(driver: &UsbTabletDriver) {
        unsafe {
            let qh = qh_ptr(driver);
            ptr::write_volatile(
                qh,
                UhciQh {
                    head: UHCI_LINK_TERMINATE,
                    element: UHCI_LINK_TERMINATE,
                },
            );

            let frame_list = driver.frame_list.virt as *mut u32;
            for i in 0..1024usize {
                ptr::write_volatile(frame_list.add(i), driver.qh.phys | UHCI_LINK_QH);
            }
        }
    }

    fn wait_td_chain(driver: &UsbTabletDriver, td_count: usize, timeout: usize) -> bool {
        for _ in 0..timeout {
            let mut all_done = true;
            for td in 0..td_count {
                let status = read_td_status(driver, td);
                if (status & UHCI_TD_STATUS_ACTIVE) != 0 {
                    all_done = false;
                    break;
                }
            }
            if all_done {
                return true;
            }
        }
        false
    }

    fn control_transfer(
        driver: &mut UsbTabletDriver,
        device_address: u8,
        setup: SetupPacket,
        data_stage: Option<&mut [u8]>,
    ) -> Option<usize> {
        let requested_len = setup.length as usize;
        if requested_len > driver.data_buf.size {
            return None;
        }

        unsafe {
            ptr::write_volatile(driver.setup_buf.virt as *mut SetupPacket, setup);
        }

        let data_in = (setup.request_type & 0x80) != 0;
        if let Some(data) = data_stage.as_ref() {
            if data.len() < requested_len {
                return None;
            }
            if !data_in && requested_len > 0 {
                unsafe {
                    ptr::copy_nonoverlapping(data.as_ptr(), driver.data_buf.virt, requested_len);
                }
            } else if requested_len > 0 {
                unsafe {
                    ptr::write_bytes(driver.data_buf.virt, 0, requested_len);
                }
            }
        } else if requested_len != 0 {
            return None;
        }

        let max_packet = core::cmp::max(driver.ep0_max_packet as usize, 8);
        let mut td_count = 1usize;
        if requested_len > 0 {
            td_count = td_count.saturating_add((requested_len + max_packet - 1) / max_packet);
        }
        td_count = td_count.saturating_add(1);
        if td_count > USB_TD_POOL_COUNT {
            return None;
        }

        write16(driver.io_base, UHCI_REG_USBSTS, 0x003F);

        let setup_status = UHCI_TD_STATUS_ACTIVE
            | UHCI_TD_STATUS_CERR_3
            | if driver.low_speed {
                UHCI_TD_STATUS_LOW_SPEED
            } else {
                0
            };
        let setup_td = UhciTd {
            link_ptr: if td_count > 1 {
                td_phys(driver.td_pool.phys, 1)
            } else {
                UHCI_LINK_TERMINATE
            },
            status: setup_status,
            token: build_td_token(UHCI_PID_SETUP, device_address, 0, 0, mem::size_of::<SetupPacket>()),
            buffer: driver.setup_buf.phys,
        };
        write_td(driver, 0, setup_td);

        let mut next_td = 1usize;
        let mut bytes_left = requested_len;
        let mut offset = 0usize;
        let mut toggle = 1u8;
        while bytes_left > 0 {
            let chunk = core::cmp::min(bytes_left, max_packet);
            let status = UHCI_TD_STATUS_ACTIVE
                | UHCI_TD_STATUS_CERR_3
                | if data_in { UHCI_TD_STATUS_SPD } else { 0 }
                | if driver.low_speed {
                    UHCI_TD_STATUS_LOW_SPEED
                } else {
                    0
                };
            let td = UhciTd {
                link_ptr: if next_td + 1 < td_count {
                    td_phys(driver.td_pool.phys, next_td + 1)
                } else {
                    UHCI_LINK_TERMINATE
                },
                status,
                token: build_td_token(
                    if data_in { UHCI_PID_IN } else { UHCI_PID_OUT },
                    device_address,
                    0,
                    toggle,
                    chunk,
                ),
                buffer: driver.data_buf.phys + offset as u32,
            };
            write_td(driver, next_td, td);

            bytes_left -= chunk;
            offset += chunk;
            toggle ^= 1;
            next_td += 1;
        }

        let status_pid = if data_in {
            UHCI_PID_OUT
        } else {
            UHCI_PID_IN
        };
        let status_td = UhciTd {
            link_ptr: UHCI_LINK_TERMINATE,
            status: UHCI_TD_STATUS_ACTIVE
                | UHCI_TD_STATUS_CERR_3
                | if driver.low_speed {
                    UHCI_TD_STATUS_LOW_SPEED
                } else {
                    0
                },
            token: build_td_token(status_pid, device_address, 0, 1, 0),
            buffer: 0,
        };
        write_td(driver, td_count - 1, status_td);

        set_qh_element(driver, td_phys(driver.td_pool.phys, 0));
        let completed = wait_td_chain(driver, td_count, USB_CTRL_TIMEOUT);
        set_qh_element(driver, UHCI_LINK_TERMINATE);
        if !completed {
            return None;
        }

        for td in 0..td_count {
            let status = read_td_status(driver, td);
            if (status & UHCI_TD_STATUS_ACTIVE) != 0 {
                return None;
            }
            if (status & UHCI_TD_STATUS_ERROR_MASK) != 0 {
                return None;
            }
            if td + 1 != td_count && (status & UHCI_TD_STATUS_NAK) != 0 {
                return None;
            }
        }

        if data_in && requested_len > 0 {
            if let Some(out) = data_stage {
                let mut copied = 0usize;
                let data_tds = td_count.saturating_sub(2);
                for td in 0..data_tds {
                    let status = read_td_status(driver, td + 1);
                    let actual = (status & 0x7FF) as usize;
                    let actual_len = if actual == 0x7FF { 0 } else { actual + 1 };
                    if copied + actual_len > requested_len {
                        return None;
                    }
                    unsafe {
                        ptr::copy_nonoverlapping(
                            driver.data_buf.virt.add(copied),
                            out.as_mut_ptr().add(copied),
                            actual_len,
                        );
                    }
                    copied += actual_len;
                    if actual_len < max_packet {
                        break;
                    }
                }
                return Some(copied);
            }
        }

        Some(requested_len)
    }

    fn interrupt_in_transfer(driver: &mut UsbTabletDriver, out: &mut [u8]) -> Option<usize> {
        let endpoint = driver.endpoint_address & 0x0F;
        let max_packet = core::cmp::min(driver.endpoint_max_packet as usize, out.len());
        if max_packet == 0 || max_packet > driver.intr_buf.size {
            return None;
        }

        unsafe {
            ptr::write_bytes(driver.intr_buf.virt, 0, max_packet);
        }

        write16(driver.io_base, UHCI_REG_USBSTS, 0x003F);

        let td = UhciTd {
            link_ptr: UHCI_LINK_TERMINATE,
            status: UHCI_TD_STATUS_ACTIVE
                | UHCI_TD_STATUS_CERR_3
                | UHCI_TD_STATUS_SPD
                | if driver.low_speed {
                    UHCI_TD_STATUS_LOW_SPEED
                } else {
                    0
                },
            token: build_td_token(
                UHCI_PID_IN,
                driver.device_address,
                endpoint,
                driver.interrupt_toggle,
                max_packet,
            ),
            buffer: driver.intr_buf.phys,
        };
        write_td(driver, 0, td);

        set_qh_element(driver, td_phys(driver.td_pool.phys, 0));
        let completed = wait_td_chain(driver, 1, USB_INTR_TIMEOUT);
        set_qh_element(driver, UHCI_LINK_TERMINATE);
        if !completed {
            return None;
        }

        let status = read_td_status(driver, 0);
        if (status & UHCI_TD_STATUS_ACTIVE) != 0 {
            return None;
        }
        if (status & UHCI_TD_STATUS_NAK) != 0 {
            return None;
        }
        if (status & UHCI_TD_STATUS_ERROR_MASK) != 0 {
            return None;
        }

        let actual = (status & 0x7FF) as usize;
        let actual_len = if actual == 0x7FF { 0 } else { actual + 1 };
        if actual_len > out.len() {
            return None;
        }
        unsafe {
            ptr::copy_nonoverlapping(driver.intr_buf.virt, out.as_mut_ptr(), actual_len);
        }
        driver.interrupt_toggle ^= 1;
        Some(actual_len)
    }

    fn try_enumerate_tablet(driver: &mut UsbTabletDriver) -> bool {
        let mut device8 = [0u8; 8];
        let setup_device_8 = SetupPacket {
            request_type: 0x80,
            request: USB_REQ_GET_DESCRIPTOR,
            value: (USB_DESCRIPTOR_DEVICE as u16) << 8,
            index: 0,
            length: device8.len() as u16,
        };
        let got = match control_transfer(driver, 0, setup_device_8, Some(&mut device8)) {
            Some(n) => n,
            None => return false,
        };
        if got < 8 || device8[1] != USB_DESCRIPTOR_DEVICE || device8[7] == 0 {
            return false;
        }
        driver.ep0_max_packet = core::cmp::max(device8[7], 8);

        let address = 1u8;
        let set_address = SetupPacket {
            request_type: 0x00,
            request: USB_REQ_SET_ADDRESS,
            value: address as u16,
            index: 0,
            length: 0,
        };
        if control_transfer(driver, 0, set_address, Some(&mut [])).is_none() {
            return false;
        }
        spin_delay(80_000);
        driver.device_address = address;

        let mut device_full = [0u8; 18];
        let setup_device_full = SetupPacket {
            request_type: 0x80,
            request: USB_REQ_GET_DESCRIPTOR,
            value: (USB_DESCRIPTOR_DEVICE as u16) << 8,
            index: 0,
            length: device_full.len() as u16,
        };
        if control_transfer(driver, address, setup_device_full, Some(&mut device_full)).is_none() {
            return false;
        }

        let mut config_head = [0u8; 9];
        let setup_config_head = SetupPacket {
            request_type: 0x80,
            request: USB_REQ_GET_DESCRIPTOR,
            value: (USB_DESCRIPTOR_CONFIG as u16) << 8,
            index: 0,
            length: config_head.len() as u16,
        };
        if control_transfer(driver, address, setup_config_head, Some(&mut config_head)).is_none() {
            return false;
        }
        if config_head[1] != USB_DESCRIPTOR_CONFIG {
            return false;
        }

        let total_len = u16::from_le_bytes([config_head[2], config_head[3]]) as usize;
        if total_len < 9 || total_len > driver.data_buf.size {
            return false;
        }

        let mut config_bytes = [0u8; USB_DATA_BUF_BYTES];
        let setup_config_full = SetupPacket {
            request_type: 0x80,
            request: USB_REQ_GET_DESCRIPTOR,
            value: (USB_DESCRIPTOR_CONFIG as u16) << 8,
            index: 0,
            length: total_len as u16,
        };
        if control_transfer(
            driver,
            address,
            setup_config_full,
            Some(&mut config_bytes[..total_len]),
        )
        .is_none()
        {
            return false;
        }

        let interface = match parse_hid_tablet_interface(&config_bytes[..total_len]) {
            Some(v) => v,
            None => return false,
        };

        let report_layout = TabletReportLayout::qemu_default();

        let set_config = SetupPacket {
            request_type: 0x00,
            request: USB_REQ_SET_CONFIGURATION,
            value: interface.config_value as u16,
            index: 0,
            length: 0,
        };
        if control_transfer(driver, address, set_config, Some(&mut [])).is_none() {
            return false;
        }

        let set_idle = SetupPacket {
            request_type: 0x21,
            request: USB_REQ_SET_IDLE,
            value: 0,
            index: interface.interface_number as u16,
            length: 0,
        };
        let _ = control_transfer(driver, address, set_idle, Some(&mut []));

        driver.endpoint_address = interface.endpoint_address;
        driver.endpoint_max_packet = interface.endpoint_max_packet;
        driver.poll_interval_ms = core::cmp::max(interface.poll_interval, 1);
        driver.report_layout = report_layout;
        driver.last_poll_ms = pit::uptime_ms();
        driver.last_report_len = 0;
        driver.interrupt_toggle = 0;

        crate::serial_println!(
            "[USB] HID tablet configured addr={} iface={} ep=0x{:02X} mps={} poll={}ms report={}B subclass={} proto={} desc_report_len={}",
            driver.device_address,
            interface.interface_number,
            driver.endpoint_address,
            driver.endpoint_max_packet,
            driver.poll_interval_ms,
            driver.report_layout.report_bytes,
            interface.subclass,
            interface.protocol,
            interface.report_descriptor_len
        );

        true
    }

    fn allocate_driver_buffers(driver: &mut UsbTabletDriver) -> bool {
        let frame_list = match alloc_dma_region(PAGE_SIZE as usize, PAGE_SIZE as usize) {
            Some(v) => v,
            None => return false,
        };
        driver.frame_list = frame_list;

        let qh = match alloc_dma_region(mem::size_of::<UhciQh>(), 16) {
            Some(v) => v,
            None => return false,
        };
        driver.qh = qh;

        let td_pool = match alloc_dma_region(mem::size_of::<UhciTd>() * USB_TD_POOL_COUNT, 16) {
            Some(v) => v,
            None => return false,
        };
        driver.td_pool = td_pool;

        let setup_buf = match alloc_dma_region(mem::size_of::<SetupPacket>(), 8) {
            Some(v) => v,
            None => return false,
        };
        driver.setup_buf = setup_buf;

        let data_buf = match alloc_dma_region(USB_DATA_BUF_BYTES, 16) {
            Some(v) => v,
            None => return false,
        };
        driver.data_buf = data_buf;

        let intr_buf = match alloc_dma_region(USB_INTR_BUF_BYTES, 16) {
            Some(v) => v,
            None => return false,
        };
        driver.intr_buf = intr_buf;

        true
    }

    pub fn init(pci_devices: &[pci::DeviceInfo]) {
        let mut state = DRIVER_STATE.lock();
        if state.active {
            return;
        }

        let mut found = None;
        for dev in pci_devices {
            if dev.class_code == USB_CLASS_SERIAL_BUS
                && dev.subclass == USB_SUBCLASS_USB
                && dev.prog_if == USB_PROG_IF_UHCI
            {
                let io_bar = dev
                    .bars
                    .iter()
                    .find(|bar| matches!(bar.kind, pci::PciBarKind::Io) && bar.addr != 0)
                    .map(|bar| bar.addr as u16);
                if let Some(base) = io_bar {
                    found = Some((dev, base));
                    break;
                }
            }
        }

        let (dev, io_base) = match found {
            Some(v) => v,
            None => {
                crate::serial_println!("[USB] UHCI controller not found");
                return;
            }
        };

        // UHCI uses I/O space BARs, so enable IO + bus mastering.
        let mut command = pci::read_config_word(dev.bus, dev.device, dev.function, 0x04);
        command |= (1 << 0) | (1 << 2);
        pci::write_config_word(dev.bus, dev.device, dev.function, 0x04, command);
        let command_after = pci::read_config_word(dev.bus, dev.device, dev.function, 0x04);

        crate::serial_println!(
            "[USB] UHCI {:02X}:{:02X}.{} io=0x{:04X} command=0x{:04X}",
            dev.bus,
            dev.device,
            dev.function,
            io_base,
            command_after
        );

        state.io_base = io_base;
        if !allocate_driver_buffers(&mut state) {
            crate::serial_println!("[USB] Failed to allocate UHCI DMA buffers");
            clear_driver_resources(&mut state);
            return;
        }

        setup_schedule(&state);

        if !uhci_reset(io_base) {
            crate::serial_println!("[USB] UHCI reset timed out");
            clear_driver_resources(&mut state);
            return;
        }
        if !uhci_start(io_base, state.frame_list.phys) {
            crate::serial_println!("[USB] UHCI failed to start");
            clear_driver_resources(&mut state);
            return;
        }

        let mut enumerated = false;
        for port in 0..USB_ENUM_PORTS {
            let Some(low_speed) = uhci_reset_port(io_base, port) else {
                continue;
            };

            state.low_speed = low_speed;
            state.device_address = 0;
            state.ep0_max_packet = 8;

            if try_enumerate_tablet(&mut state) {
                enumerated = true;
                break;
            }
        }

        if !enumerated {
            crate::serial_println!("[USB] No HID tablet enumerated");
            clear_driver_resources(&mut state);
            return;
        }

        state.active = true;
        mouse::set_relative_enabled(false);
        crate::serial_println!("[USB] Tablet input active (PS/2 relative input disabled)");
    }

    pub fn poll() {
        let mut sample = None;
        {
            let mut state = DRIVER_STATE.lock();
            if !state.active {
                return;
            }

            let now = pit::uptime_ms();
            if now.saturating_sub(state.last_poll_ms) < state.poll_interval_ms as u64 {
                return;
            }
            state.last_poll_ms = now;

            let mut report = [0u8; USB_INTR_BUF_BYTES];
            let Some(report_len) = interrupt_in_transfer(&mut state, &mut report) else {
                return;
            };
            if report_len == 0 {
                return;
            }

            if report_len == state.last_report_len
                && report[..report_len] == state.last_report[..report_len]
            {
                return;
            }

            if let Some(decoded) = decode_qemu_tablet_report(&report[..report_len]) {
                state.last_report_len = report_len;
                state.last_report[..report_len].copy_from_slice(&report[..report_len]);
                sample = Some(decoded);
            }
        }

        let Some(decoded) = sample else {
            return;
        };
        let (min_x, max_x, min_y, max_y) = {
            let state = DRIVER_STATE.lock();
            if !state.active {
                return;
            }
            (
                state.report_layout.x.logical_min,
                state.report_layout.x.logical_max,
                state.report_layout.y.logical_min,
                state.report_layout.y.logical_max,
            )
        };
        mouse::set_absolute_position(
            decoded.x,
            decoded.y,
            min_x,
            max_x,
            min_y,
            max_y,
            decoded.buttons,
        );
    }
}

#[cfg(target_os = "none")]
pub fn init(pci_devices: &[pci::DeviceInfo]) {
    hw::init(pci_devices);
}

#[cfg(not(target_os = "none"))]
pub fn init(_pci_devices: &[pci::DeviceInfo]) {}

#[cfg(target_os = "none")]
pub fn poll() {
    hw::poll();
}

#[cfg(not(target_os = "none"))]
pub fn poll() {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_hid_config_finds_interrupt_endpoint() {
        let config = [
            9, 2, 34, 0, 1, 1, 0, 0xA0, 50, // config
            9, 4, 0, 0, 1, 0x03, 0x01, 0x02, 0, // interface
            9, 0x21, 0x11, 0x01, 0x00, 0x01, 0x22, 74, 0, // HID
            7, 5, 0x81, 0x03, 8, 0, 10, // endpoint
        ];
        let parsed = parse_hid_tablet_interface(&config).expect("hid interface");
        assert_eq!(parsed.config_value, 1);
        assert_eq!(parsed.interface_number, 0);
        assert_eq!(parsed.endpoint_address, 0x81);
        assert_eq!(parsed.endpoint_max_packet, 8);
        assert_eq!(parsed.poll_interval, 10);
        assert_eq!(parsed.report_descriptor_len, 74);
        assert_eq!(parsed.subclass, 1);
        assert_eq!(parsed.protocol, 2);
    }

    #[test]
    fn parse_hid_report_layout_extracts_absolute_axes() {
        let report_descriptor = [
            0x05, 0x01, // Usage Page (Generic Desktop)
            0x09, 0x02, // Usage (Mouse)
            0xA1, 0x01, // Collection (Application)
            0x09, 0x01, //   Usage (Pointer)
            0xA1, 0x00, //   Collection (Physical)
            0x05, 0x09, //     Usage Page (Button)
            0x19, 0x01, //     Usage Min (1)
            0x29, 0x03, //     Usage Max (3)
            0x15, 0x00, //     Logical Min (0)
            0x25, 0x01, //     Logical Max (1)
            0x95, 0x03, //     Report Count (3)
            0x75, 0x01, //     Report Size (1)
            0x81, 0x02, //     Input (Data,Var,Abs)
            0x95, 0x01, //     Report Count (1)
            0x75, 0x05, //     Report Size (5)
            0x81, 0x03, //     Input (Const,Var,Abs)
            0x05, 0x01, //     Usage Page (Generic Desktop)
            0x09, 0x30, //     Usage X
            0x09, 0x31, //     Usage Y
            0x16, 0x00, 0x00, //     Logical Min (0)
            0x26, 0xFF, 0x7F, //     Logical Max (32767)
            0x75, 0x10, //     Report Size (16)
            0x95, 0x02, //     Report Count (2)
            0x81, 0x02, //     Input (Data,Var,Abs)
            0xC0, //   End Collection
            0xC0, // End Collection
        ];

        let layout = parse_hid_report_layout(&report_descriptor).expect("layout");
        assert_eq!(layout.x.bit_offset, 8);
        assert_eq!(layout.y.bit_offset, 24);
        assert_eq!(layout.x.bit_size, 16);
        assert_eq!(layout.y.bit_size, 16);
        assert_eq!(layout.x.logical_max, 0x7FFF);
        assert_eq!(layout.report_bytes, 5);
        assert_eq!(
            layout.buttons,
            Some(ButtonField {
                bit_offset: 0,
                bit_count: 3
            })
        );
    }

    #[test]
    fn decode_tablet_report_reads_buttons_and_axes() {
        let layout = TabletReportLayout::qemu_default();
        let report = [0b0000_0101, 0x34, 0x12, 0x78, 0x56];
        let sample = decode_tablet_report(layout, &report).expect("sample");
        assert_eq!(sample.buttons, 0b101);
        assert_eq!(sample.x, 0x1234);
        assert_eq!(sample.y, 0x5678);
    }
}
