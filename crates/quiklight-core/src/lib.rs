//! # quiklight-core
//!
//! Библиотека протокола и HID-транспорта для LED-панели Robobloq QuikLight
//! (Feelin Light Q1, USB-версия, VID `0x1A86`, PID `0xFE07`).
//!
//! Без UI: только протокол (`protocol`) и работа с устройством (`hid`).
//! Полное описание протокола — в `docs/PROTOCOL.md`.

pub mod hid;
pub mod protocol;
pub mod sync;
