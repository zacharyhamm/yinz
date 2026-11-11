use yinz_ipc::ipc::{ClientToServerMsg, ColorRegister};

use crate::emulation::color::{self, Paints};

pub fn seed_messages(paints: &Paints) -> Vec<ClientToServerMsg> {
    vec![
        ClientToServerMsg::ForegroundColor {
            color: xparse_color(paints.foreground),
        },
        ClientToServerMsg::BackgroundColor {
            color: xparse_color(paints.background),
        },
        ClientToServerMsg::ColorRegisters {
            color_registers: color_registers(paints),
        },
    ]
}

fn color_registers(paints: &Paints) -> Vec<ColorRegister> {
    (0..=255u8)
        .map(|index| ColorRegister {
            index: index as usize,
            color: xparse_color(paints.indexed(index)),
        })
        .collect()
}

pub fn xparse_color([r, g, b]: color::Srgb) -> String {
    format!(
        "rgb:{:04x}/{:04x}/{:04x}",
        (r as u16) * 0x0101,
        (g as u16) * 0x0101,
        (b as u16) * 0x0101,
    )
}

#[cfg(test)]
#[path = "unit/palette_tests.rs"]
mod tests;
