//! CMOS real-time clock (MC146818): the wall-clock time at boot.
//!
//! The kernel reads it once and counts PIT ticks from there, so
//! `CLOCK_REALTIME` is "RTC at boot + uptime". The RTC is taken to be UTC,
//! which is what QEMU provides by default.

// Host tests cover the decoding; the port I/O is not built there.
#![cfg_attr(test, allow(dead_code))]

#[cfg(not(test))]
use crate::port::{inb, outb};

const CMOS_ADDRESS: u16 = 0x70;
const CMOS_DATA: u16 = 0x71;

const REG_SECONDS: u8 = 0x00;
const REG_MINUTES: u8 = 0x02;
const REG_HOURS: u8 = 0x04;
const REG_DAY: u8 = 0x07;
const REG_MONTH: u8 = 0x08;
const REG_YEAR: u8 = 0x09;
const REG_STATUS_A: u8 = 0x0A;
const REG_STATUS_B: u8 = 0x0B;

const STATUS_A_UPDATE_IN_PROGRESS: u8 = 0x80;
const STATUS_B_24_HOUR: u8 = 0x02;
const STATUS_B_BINARY: u8 = 0x04;
const HOURS_PM: u8 = 0x80;

/// The raw time registers, as read from the chip.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct RawTime {
    pub seconds: u8,
    pub minutes: u8,
    pub hours: u8,
    pub day: u8,
    pub month: u8,
    pub year: u8,
}

/// A decoded calendar time (UTC).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct DateTime {
    pub year: u32,
    pub month: u32,
    pub day: u32,
    pub hour: u32,
    pub minute: u32,
    pub second: u32,
}

fn bcd_to_binary(value: u8) -> u8 {
    (value & 0x0F) + (value >> 4) * 10
}

/// Decode raw registers according to status register B: BCD or binary
/// values, and 12-hour (PM in bit 7 of the hours) or 24-hour time. The
/// two-digit year is taken as 20xx. Returns `None` for out-of-range fields
/// (an unset or absent chip).
pub fn decode(raw: RawTime, status_b: u8) -> Option<DateTime> {
    let binary = (status_b & STATUS_B_BINARY) != 0;
    let field = |value: u8| if binary { value } else { bcd_to_binary(value) };
    let pm = (raw.hours & HOURS_PM) != 0;
    let mut hour = field(raw.hours & !HOURS_PM) as u32;
    if (status_b & STATUS_B_24_HOUR) == 0 {
        // 12-hour clock: 12 AM is midnight, 12 PM is noon.
        if !(1..=12).contains(&hour) {
            return None;
        }
        hour %= 12;
        if pm {
            hour += 12;
        }
    }
    let time = DateTime {
        year: 2000 + field(raw.year) as u32,
        month: field(raw.month) as u32,
        day: field(raw.day) as u32,
        hour,
        minute: field(raw.minutes) as u32,
        second: field(raw.seconds) as u32,
    };
    let valid = (1..=12).contains(&time.month)
        && (1..=days_in_month(time.year, time.month)).contains(&time.day)
        && time.hour < 24
        && time.minute < 60
        && time.second < 60;
    if valid {
        Some(time)
    } else {
        None
    }
}

fn is_leap_year(year: u32) -> bool {
    (year % 4 == 0 && year % 100 != 0) || year % 400 == 0
}

fn days_in_month(year: u32, month: u32) -> u32 {
    match month {
        4 | 6 | 9 | 11 => 30,
        2 if is_leap_year(year) => 29,
        2 => 28,
        _ => 31,
    }
}

/// Seconds since 1970-01-01 00:00:00 UTC for a valid date from 1970 on.
pub fn unix_seconds(time: DateTime) -> u64 {
    let mut days = 0u64;
    for year in 1970..time.year {
        days += if is_leap_year(year) { 366 } else { 365 };
    }
    for month in 1..time.month {
        days += days_in_month(time.year, month) as u64;
    }
    days += (time.day - 1) as u64;
    ((days * 24 + time.hour as u64) * 60 + time.minute as u64) * 60 + time.second as u64
}

#[cfg(not(test))]
unsafe fn read_register(reg: u8) -> u8 {
    // Bit 7 of the address port masks NMIs; leave them enabled.
    outb(CMOS_ADDRESS, reg);
    inb(CMOS_DATA)
}

#[cfg(not(test))]
unsafe fn read_raw() -> RawTime {
    RawTime {
        seconds: read_register(REG_SECONDS),
        minutes: read_register(REG_MINUTES),
        hours: read_register(REG_HOURS),
        day: read_register(REG_DAY),
        month: read_register(REG_MONTH),
        year: read_register(REG_YEAR),
    }
}

/// Read the current time from the CMOS RTC as seconds since the Unix epoch.
/// Waits out an update in progress and reads until two reads agree, so a
/// rollover between registers cannot produce a mixed time. Returns `None`
/// when the chip does not settle or holds an invalid date.
#[cfg(not(test))]
pub fn read_unix_seconds() -> Option<u64> {
    unsafe {
        let mut previous: Option<RawTime> = None;
        for _ in 0..16 {
            let mut settled = false;
            for _ in 0..100_000 {
                if (read_register(REG_STATUS_A) & STATUS_A_UPDATE_IN_PROGRESS) == 0 {
                    settled = true;
                    break;
                }
                core::hint::spin_loop();
            }
            if !settled {
                return None;
            }
            let raw = read_raw();
            if previous == Some(raw) {
                return decode(raw, read_register(REG_STATUS_B)).map(unix_seconds);
            }
            previous = Some(raw);
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn raw(seconds: u8, minutes: u8, hours: u8, day: u8, month: u8, year: u8) -> RawTime {
        RawTime { seconds, minutes, hours, day, month, year }
    }

    #[test]
    fn decodes_bcd_24_hour_time() {
        // QEMU's default: BCD, 24-hour.
        let time = decode(raw(0x59, 0x34, 0x23, 0x10, 0x11, 0x26), STATUS_B_24_HOUR).unwrap();
        assert_eq!(time, DateTime { year: 2026, month: 11, day: 10, hour: 23, minute: 34, second: 59 });
    }

    #[test]
    fn decodes_binary_and_12_hour_time() {
        let binary = STATUS_B_BINARY | STATUS_B_24_HOUR;
        let time = decode(raw(7, 8, 9, 29, 2, 24), binary).unwrap();
        assert_eq!(time, DateTime { year: 2024, month: 2, day: 29, hour: 9, minute: 8, second: 7 });
        // 12-hour BCD: 12 AM is hour 0, 12 PM is hour 12, 11 PM is hour 23.
        assert_eq!(decode(raw(0, 0, 0x12, 1, 1, 0x25), 0).unwrap().hour, 0);
        assert_eq!(decode(raw(0, 0, 0x12 | HOURS_PM, 1, 1, 0x25), 0).unwrap().hour, 12);
        assert_eq!(decode(raw(0, 0, 0x11 | HOURS_PM, 1, 1, 0x25), 0).unwrap().hour, 23);
        assert_eq!(decode(raw(0, 0, 0x01, 1, 1, 0x25), 0).unwrap().hour, 1);
    }

    #[test]
    fn rejects_invalid_dates() {
        let b = STATUS_B_24_HOUR;
        assert_eq!(decode(raw(0, 0, 0, 0, 0, 0), b), None); // no chip: all zero
        assert_eq!(decode(raw(0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF), b), None); // floating bus
        assert_eq!(decode(raw(0, 0, 0, 0x30, 0x02, 0x24), b), None); // 30 February
        assert_eq!(decode(raw(0, 0, 0, 0x29, 0x02, 0x25), b), None); // not a leap year
        assert_eq!(decode(raw(0, 0, 0x24, 0x01, 0x01, 0x25), b), None); // hour 24
        assert_eq!(decode(raw(0, 0, 0x13, 0x01, 0x01, 0x25), 0), None); // 13 o'clock, 12-hour
    }

    #[test]
    fn converts_dates_to_unix_seconds() {
        let at = |year, month, day, hour, minute, second| {
            unix_seconds(DateTime { year, month, day, hour, minute, second })
        };
        assert_eq!(at(1970, 1, 1, 0, 0, 0), 0);
        assert_eq!(at(2000, 1, 1, 0, 0, 0), 946_684_800);
        assert_eq!(at(2000, 3, 1, 0, 0, 0), 951_868_800); // 2000 is a leap year
        assert_eq!(at(2024, 2, 29, 12, 0, 0), 1_709_208_000);
        assert_eq!(at(2026, 11, 10, 23, 34, 59), 1_794_353_699);
        assert_eq!(at(2038, 1, 19, 3, 14, 8), 2_147_483_648);
    }
}
