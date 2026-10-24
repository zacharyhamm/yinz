//! Terminal mouse encoding and button reports.

use super::{Grid, MouseMode, MouseTracking};
use yinz_types::mouse::MouseEvent;
use yinz_types::mouse::MouseEventType;
use yinz_types::position::Position;

fn utf8_mouse_coordinates(column: usize, line: isize) -> Vec<u8> {
    let mut coordinates = vec![];
    let mouse_pos_encode = |pos: usize| -> Vec<u8> {
        let pos = 32 + pos;
        let first = 0xC0 + pos / 64;
        let second = 0x80 + (pos & 63);
        vec![first as u8, second as u8]
    };

    if column > 95 {
        coordinates.append(&mut mouse_pos_encode(column));
    } else {
        coordinates.push(32 + column as u8);
    }
    if line > 95 {
        coordinates.append(&mut mouse_pos_encode(line as usize));
    } else {
        coordinates.push(32 + line as u8);
    }
    coordinates
}

impl Grid {
    fn mouse_buttons_value_x10(&self, event: &MouseEvent) -> u8 {
        let mut value = 35; // Default to no buttons down.
        if event.event_type == MouseEventType::Release {
            return value;
        }
        if event.left {
            value = 32;
        } else if event.middle {
            value = 33;
        } else if event.right {
            value = 34;
        } else if event.wheel_up {
            value = 96;
        } else if event.wheel_down {
            value = 97;
        }
        if event.event_type == MouseEventType::Motion {
            value += 32;
        }
        if event.shift {
            value |= 0x04;
        }
        if event.alt {
            value |= 0x08;
        }
        if event.ctrl {
            value |= 0x10;
        }
        value
    }
    fn mouse_buttons_value_sgr(&self, event: &MouseEvent) -> u8 {
        let mut value = 3; // Default to no buttons down.
        if event.left {
            value = 0;
        } else if event.middle {
            value = 1;
        } else if event.right {
            value = 2;
        } else if event.wheel_up {
            value = 64;
        } else if event.wheel_down {
            value = 65;
        }
        if event.event_type == MouseEventType::Motion {
            value += 32;
        }
        if event.shift {
            value |= 0x04;
        }
        if event.alt {
            value |= 0x08;
        }
        if event.ctrl {
            value |= 0x10;
        }
        value
    }
    pub fn mouse_event_signal(&self, event: &MouseEvent) -> Option<String> {
        let emit = match (&self.mouse_tracking, event.event_type) {
            (MouseTracking::Off, _) => false,
            (MouseTracking::AnyEventTracking, _) => true,
            (_, MouseEventType::Press | MouseEventType::Release) => true,
            (MouseTracking::ButtonEventTracking, MouseEventType::Motion) => {
                event.left | event.right | event.middle | event.wheel_up | event.wheel_down
            },
            (_, _) => false,
        };

        match (emit, &self.mouse_mode) {
            (true, MouseMode::NoEncoding | MouseMode::Utf8) => {
                let mut msg: Vec<u8> = vec![27, b'[', b'M', self.mouse_buttons_value_x10(event)];
                msg.append(&mut utf8_mouse_coordinates(
                    event.position.column() + 1,
                    event.position.line() + 1,
                ));
                Some(String::from_utf8_lossy(&msg).into())
            },
            (true, MouseMode::Sgr) => Some(format!(
                "\u{1b}[<{:?};{:?};{:?}{}",
                self.mouse_buttons_value_sgr(event),
                event.position.column() + 1,
                event.position.line() + 1,
                match event.event_type {
                    MouseEventType::Press | MouseEventType::Motion => 'M',
                    _ => 'm',
                }
            )),
            (_, _) => None,
        }
    }
    pub fn mouse_left_click_signal(&self, position: &Position, is_held: bool) -> Option<String> {
        let event = if is_held {
            MouseEvent::new_left_motion_event(*position)
        } else {
            MouseEvent::new_left_press_event(*position)
        };
        self.mouse_event_signal(&event)
    }
    pub fn mouse_left_click_release_signal(&self, position: &Position) -> Option<String> {
        self.mouse_event_signal(&MouseEvent::new_left_release_event(*position))
    }
    pub fn mouse_right_click_signal(&self, position: &Position, is_held: bool) -> Option<String> {
        let event = if is_held {
            MouseEvent::new_right_motion_event(*position)
        } else {
            MouseEvent::new_right_press_event(*position)
        };
        self.mouse_event_signal(&event)
    }
    pub fn mouse_right_click_release_signal(&self, position: &Position) -> Option<String> {
        self.mouse_event_signal(&MouseEvent::new_right_release_event(*position))
    }
    pub fn mouse_middle_click_signal(&self, position: &Position, is_held: bool) -> Option<String> {
        let event = if is_held {
            MouseEvent::new_middle_motion_event(*position)
        } else {
            MouseEvent::new_middle_press_event(*position)
        };
        self.mouse_event_signal(&event)
    }
    pub fn mouse_middle_click_release_signal(&self, position: &Position) -> Option<String> {
        self.mouse_event_signal(&MouseEvent::new_middle_release_event(*position))
    }
    pub fn mouse_scroll_up_signal(&self, position: &Position) -> Option<String> {
        self.mouse_event_signal(&MouseEvent::new_scroll_up_event(*position))
    }
    pub fn mouse_scroll_down_signal(&self, position: &Position) -> Option<String> {
        self.mouse_event_signal(&MouseEvent::new_scroll_down_event(*position))
    }
}
