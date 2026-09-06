//! Geometry and pure sizing operations shared by the host and plugins.

use crate::pane_size::{Constraint, Dimension, PaneGeom};
use serde::{Deserialize, Serialize};
use std::{fmt, ops::Not, str::FromStr};

#[derive(
    Eq, Clone, Copy, Debug, PartialEq, Hash, Deserialize, Serialize, PartialOrd, Ord, Default,
)]
pub enum Direction {
    #[default]
    Left,
    Right,
    Up,
    Down,
}

impl Direction {
    pub fn invert(&self) -> Direction {
        match *self {
            Direction::Left => Direction::Right,
            Direction::Down => Direction::Up,
            Direction::Up => Direction::Down,
            Direction::Right => Direction::Left,
        }
    }

    pub fn is_horizontal(&self) -> bool {
        matches!(self, Direction::Left | Direction::Right)
    }

    pub fn is_vertical(&self) -> bool {
        matches!(self, Direction::Down | Direction::Up)
    }
}

impl fmt::Display for Direction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Direction::Left => write!(f, "←"),
            Direction::Right => write!(f, "→"),
            Direction::Up => write!(f, "↑"),
            Direction::Down => write!(f, "↓"),
        }
    }
}

impl FromStr for Direction {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "Left" | "left" => Ok(Direction::Left),
            "Right" | "right" => Ok(Direction::Right),
            "Up" | "up" => Ok(Direction::Up),
            "Down" | "down" => Ok(Direction::Down),
            _ => Err(format!(
                "Failed to parse Direction. Unknown Direction: {}",
                s
            )),
        }
    }
}

/// Resize operation to perform.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash, Deserialize, Serialize, Default)]
pub enum Resize {
    #[default]
    Increase,
    Decrease,
}

impl Resize {
    pub fn invert(&self) -> Self {
        match self {
            Resize::Increase => Resize::Decrease,
            Resize::Decrease => Resize::Increase,
        }
    }
}

impl fmt::Display for Resize {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Resize::Increase => write!(f, "+"),
            Resize::Decrease => write!(f, "-"),
        }
    }
}

impl FromStr for Resize {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "Increase" | "increase" | "+" => Ok(Resize::Increase),
            "Decrease" | "decrease" | "-" => Ok(Resize::Decrease),
            _ => Err(format!(
                "failed to parse resize type. Unknown specifier '{}'",
                s
            )),
        }
    }
}

/// Container type that fully describes resize operations.
///
/// This is best thought of as follows:
///
/// - `resize` commands how the total *area* of the pane will change as part of this resize
///   operation.
/// - `direction` has two meanings:
///     - `None` means to resize all borders equally
///     - Anything else means to move the named border to achieve the change in area
#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash, Deserialize, Serialize)]
pub struct ResizeStrategy {
    /// Whether to increase or resize total area
    pub resize: Resize,
    /// With which border, if any, to change area
    pub direction: Option<Direction>,
    /// If set to true (default), increasing resizes towards a viewport border will be inverted.
    /// I.e. a scenario like this ("increase right"):
    ///
    /// ```text
    /// +---+---+
    /// |   | X |->
    /// +---+---+
    /// ```
    ///
    /// turns into this ("decrease left"):
    ///
    /// ```text
    /// +---+---+
    /// |   |-> |
    /// +---+---+
    /// ```
    pub invert_on_boundaries: bool,
}

impl From<Direction> for ResizeStrategy {
    fn from(direction: Direction) -> Self {
        ResizeStrategy::new(Resize::Increase, Some(direction))
    }
}

impl From<Resize> for ResizeStrategy {
    fn from(resize: Resize) -> Self {
        ResizeStrategy::new(resize, None)
    }
}

impl ResizeStrategy {
    pub fn new(resize: Resize, direction: Option<Direction>) -> Self {
        ResizeStrategy {
            resize,
            direction,
            invert_on_boundaries: true,
        }
    }

    pub fn invert(&self) -> ResizeStrategy {
        let resize = match self.resize {
            Resize::Increase => Resize::Decrease,
            Resize::Decrease => Resize::Increase,
        };
        let direction = self.direction.map(|direction| direction.invert());

        ResizeStrategy::new(resize, direction)
    }

    pub fn resize_type(&self) -> Resize {
        self.resize
    }

    pub fn direction(&self) -> Option<Direction> {
        self.direction
    }

    pub fn direction_horizontal(&self) -> bool {
        matches!(
            self.direction,
            Some(Direction::Left) | Some(Direction::Right)
        )
    }

    pub fn direction_vertical(&self) -> bool {
        matches!(self.direction, Some(Direction::Up) | Some(Direction::Down))
    }

    pub fn resize_increase(&self) -> bool {
        self.resize == Resize::Increase
    }

    pub fn resize_decrease(&self) -> bool {
        self.resize == Resize::Decrease
    }

    pub fn move_left_border_left(&self) -> bool {
        (self.resize == Resize::Increase) && (self.direction == Some(Direction::Left))
    }

    pub fn move_left_border_right(&self) -> bool {
        (self.resize == Resize::Decrease) && (self.direction == Some(Direction::Left))
    }

    pub fn move_lower_border_down(&self) -> bool {
        (self.resize == Resize::Increase) && (self.direction == Some(Direction::Down))
    }

    pub fn move_lower_border_up(&self) -> bool {
        (self.resize == Resize::Decrease) && (self.direction == Some(Direction::Down))
    }

    pub fn move_upper_border_up(&self) -> bool {
        (self.resize == Resize::Increase) && (self.direction == Some(Direction::Up))
    }

    pub fn move_upper_border_down(&self) -> bool {
        (self.resize == Resize::Decrease) && (self.direction == Some(Direction::Up))
    }

    pub fn move_right_border_right(&self) -> bool {
        (self.resize == Resize::Increase) && (self.direction == Some(Direction::Right))
    }

    pub fn move_right_border_left(&self) -> bool {
        (self.resize == Resize::Decrease) && (self.direction == Some(Direction::Right))
    }

    pub fn move_all_borders_out(&self) -> bool {
        (self.resize == Resize::Increase) && self.direction.is_none()
    }

    pub fn move_all_borders_in(&self) -> bool {
        (self.resize == Resize::Decrease) && self.direction.is_none()
    }
}

impl fmt::Display for ResizeStrategy {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let resize = match self.resize {
            Resize::Increase => "increase",
            Resize::Decrease => "decrease",
        };
        let border = match self.direction {
            Some(Direction::Left) => "left",
            Some(Direction::Down) => "bottom",
            Some(Direction::Up) => "top",
            Some(Direction::Right) => "right",
            None => "every",
        };

        write!(f, "{} size on {} border", resize, border)
    }
}

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize, Clone, Copy, Default)]
pub enum SplitDirection {
    #[default]
    Horizontal,
    Vertical,
}

impl Not for SplitDirection {
    type Output = Self;

    fn not(self) -> Self::Output {
        match self {
            SplitDirection::Horizontal => SplitDirection::Vertical,
            SplitDirection::Vertical => SplitDirection::Horizontal,
        }
    }
}

impl From<Direction> for SplitDirection {
    fn from(direction: Direction) -> Self {
        match direction {
            Direction::Left | Direction::Right => SplitDirection::Horizontal,
            Direction::Down | Direction::Up => SplitDirection::Vertical,
        }
    }
}

#[derive(Debug, Serialize, Deserialize, Clone, Copy, PartialEq, Eq)]
pub enum SplitSize {
    #[serde(alias = "percent")]
    Percent(usize), // 1 to 100
    #[serde(alias = "fixed")]
    Fixed(usize), // An absolute number of columns or rows
}

impl From<PercentOrFixed> for SplitSize {
    fn from(pof: PercentOrFixed) -> Self {
        match pof {
            PercentOrFixed::Percent(p) => SplitSize::Percent(p),
            PercentOrFixed::Fixed(f) => SplitSize::Fixed(f),
        }
    }
}

impl From<SplitSize> for PercentOrFixed {
    fn from(ss: SplitSize) -> Self {
        match ss {
            SplitSize::Percent(p) => PercentOrFixed::Percent(p),
            SplitSize::Fixed(f) => PercentOrFixed::Fixed(f),
        }
    }
}

impl SplitSize {
    pub fn to_fixed(&self, full_size: usize) -> usize {
        match self {
            SplitSize::Percent(percent) => {
                ((*percent as f64 / 100.0) * full_size as f64).floor() as usize
            },
            SplitSize::Fixed(fixed) => *fixed,
        }
    }
}

#[derive(Debug, Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PercentOrFixed {
    Percent(usize), // 1 to 100
    Fixed(usize),   // An absolute number of columns or rows
}

impl From<Dimension> for PercentOrFixed {
    fn from(dimension: Dimension) -> Self {
        match dimension.constraint {
            Constraint::Percent(percent) => PercentOrFixed::Percent(percent as usize),
            Constraint::Fixed(fixed_size) => PercentOrFixed::Fixed(fixed_size),
        }
    }
}

impl PercentOrFixed {
    pub fn to_position(&self, whole: usize) -> usize {
        match self {
            PercentOrFixed::Percent(percent) => {
                (whole as f64 / 100.0 * *percent as f64).ceil() as usize
            },
            PercentOrFixed::Fixed(fixed) => {
                if *fixed > whole {
                    whole
                } else {
                    *fixed
                }
            },
        }
    }
    pub fn to_fixed(&self, whole: usize) -> usize {
        match self {
            PercentOrFixed::Percent(percent) => {
                ((*percent as f64 / 100.0) * whole as f64).floor() as usize
            },
            PercentOrFixed::Fixed(fixed) => *fixed,
        }
    }
}

impl PercentOrFixed {
    pub fn is_zero(&self) -> bool {
        match self {
            PercentOrFixed::Percent(percent) => *percent == 0,
            PercentOrFixed::Fixed(fixed) => *fixed == 0,
        }
    }
}

impl FromStr for PercentOrFixed {
    type Err = Box<dyn std::error::Error>;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s.ends_with('%') {
            let char_count = s.chars().count();
            let percent_size = s[..char_count.saturating_sub(1)].parse::<usize>()?;
            if percent_size <= 100 {
                Ok(PercentOrFixed::Percent(percent_size))
            } else {
                Err("Percent must be between 0 and 100".into())
            }
        } else {
            let fixed_size = s.parse::<usize>()?;
            Ok(PercentOrFixed::Fixed(fixed_size))
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize, Default)]
pub struct FloatingPaneCoordinates {
    pub x: Option<PercentOrFixed>,
    pub y: Option<PercentOrFixed>,
    pub width: Option<PercentOrFixed>,
    pub height: Option<PercentOrFixed>,
    pub pinned: Option<bool>,
    pub borderless: Option<bool>,
}

impl FloatingPaneCoordinates {
    pub fn new(
        x: Option<String>,
        y: Option<String>,
        width: Option<String>,
        height: Option<String>,
        pinned: Option<bool>,
        borderless: Option<bool>,
    ) -> Option<Self> {
        // Parse x/y coordinates - allows 0% or 0
        let x = x.and_then(|x| PercentOrFixed::from_str(&x).ok());
        let y = y.and_then(|y| PercentOrFixed::from_str(&y).ok());

        // Parse width/height - reject 0% or 0
        let width = width.and_then(|w| {
            PercentOrFixed::from_str(&w)
                .ok()
                .and_then(|size| match size {
                    PercentOrFixed::Percent(0) => None,
                    PercentOrFixed::Fixed(0) => None,
                    _ => Some(size),
                })
        });
        let height = height.and_then(|h| {
            PercentOrFixed::from_str(&h)
                .ok()
                .and_then(|size| match size {
                    PercentOrFixed::Percent(0) => None,
                    PercentOrFixed::Fixed(0) => None,
                    _ => Some(size),
                })
        });

        if x.is_none()
            && y.is_none()
            && width.is_none()
            && height.is_none()
            && pinned.is_none()
            && borderless.is_none()
        {
            None
        } else {
            Some(FloatingPaneCoordinates {
                x,
                y,
                width,
                height,
                pinned,
                borderless,
            })
        }
    }
    pub fn with_x_fixed(mut self, x: usize) -> Self {
        self.x = Some(PercentOrFixed::Fixed(x));
        self
    }
    pub fn with_x_percent(mut self, x: usize) -> Result<Self, &'static str> {
        if x > 100 {
            return Err("x must be between 0 and 100");
        }
        self.x = Some(PercentOrFixed::Percent(x));
        Ok(self)
    }
    pub fn with_y_fixed(mut self, y: usize) -> Self {
        self.y = Some(PercentOrFixed::Fixed(y));
        self
    }
    pub fn with_y_percent(mut self, y: usize) -> Result<Self, &'static str> {
        if y > 100 {
            return Err("y must be between 0 and 100");
        }
        self.y = Some(PercentOrFixed::Percent(y));
        Ok(self)
    }
    pub fn with_width_fixed(mut self, width: usize) -> Self {
        self.width = Some(PercentOrFixed::Fixed(width));
        self
    }
    pub fn with_width_percent(mut self, width: usize) -> Result<Self, &'static str> {
        if width > 100 {
            return Err("width must be between 0 and 100");
        }
        self.width = Some(PercentOrFixed::Percent(width));
        Ok(self)
    }
    pub fn with_height_fixed(mut self, height: usize) -> Self {
        self.height = Some(PercentOrFixed::Fixed(height));
        self
    }
    pub fn with_height_percent(mut self, height: usize) -> Result<Self, &'static str> {
        if height > 100 {
            return Err("height must be between 0 and 100");
        }
        self.height = Some(PercentOrFixed::Percent(height));
        Ok(self)
    }
}

impl From<PaneGeom> for FloatingPaneCoordinates {
    fn from(pane_geom: PaneGeom) -> Self {
        FloatingPaneCoordinates {
            x: Some(PercentOrFixed::Fixed(pane_geom.x)),
            y: Some(PercentOrFixed::Fixed(pane_geom.y)),
            width: Some(PercentOrFixed::Fixed(pane_geom.cols.as_usize())),
            height: Some(PercentOrFixed::Fixed(pane_geom.rows.as_usize())),
            pinned: Some(pane_geom.is_pinned),
            borderless: None,
        }
    }
}

impl FromStr for SplitDirection {
    type Err = Box<dyn std::error::Error>;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "vertical" | "Vertical" => Ok(SplitDirection::Vertical),
            "horizontal" | "Horizontal" => Ok(SplitDirection::Horizontal),
            _ => Err("split direction must be either vertical or horizontal".into()),
        }
    }
}

impl FromStr for SplitSize {
    type Err = Box<dyn std::error::Error>;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s.ends_with('%') {
            let char_count = s.chars().count();
            let percent_size = s[..char_count.saturating_sub(1)].parse::<usize>()?;
            if percent_size > 0 && percent_size <= 100 {
                Ok(SplitSize::Percent(percent_size))
            } else {
                Err("Percent must be between 0 and 100".into())
            }
        } else {
            let fixed_size = s.parse::<usize>()?;
            Ok(SplitSize::Fixed(fixed_size))
        }
    }
}
