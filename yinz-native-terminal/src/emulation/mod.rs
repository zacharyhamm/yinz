pub mod color;
pub mod damage;
pub mod kitty;
pub mod palette;
pub mod screen_buffer;
pub mod selection;
pub mod sixel;
pub mod terminal;

#[cfg(test)]
#[path = "unit/production_round_trip_tests.rs"]
mod production_round_trip_tests;
