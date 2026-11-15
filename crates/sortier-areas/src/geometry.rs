use serde::{Deserialize, Serialize};

/// A position in the area host's coordinate space (logical pixels).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Point {
    pub x: f32,
    pub y: f32,
}

impl Point {
    pub const fn new(x: f32, y: f32) -> Self {
        Self { x, y }
    }

    pub(crate) fn along(self, axis: Axis) -> f32 {
        match axis {
            Axis::X => self.x,
            Axis::Y => self.y,
        }
    }
}

/// A width and height in logical pixels.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Size {
    pub width: f32,
    pub height: f32,
}

impl Size {
    pub const fn new(width: f32, height: f32) -> Self {
        Self { width, height }
    }

    pub(crate) fn along(self, axis: Axis) -> f32 {
        match axis {
            Axis::X => self.width,
            Axis::Y => self.height,
        }
    }
}

/// An axis-aligned rectangle in logical pixels.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Rect {
    pub origin: Point,
    pub size: Size,
}

impl Rect {
    pub const fn new(x: f32, y: f32, width: f32, height: f32) -> Self {
        Self {
            origin: Point::new(x, y),
            size: Size::new(width, height),
        }
    }

    pub fn right(&self) -> f32 {
        self.origin.x + self.size.width
    }

    pub fn bottom(&self) -> f32 {
        self.origin.y + self.size.height
    }

    /// Half-open containment: the left and top edges are inside, the right and
    /// bottom edges are not, so adjacent rectangles never both contain a point.
    pub fn contains(&self, point: Point) -> bool {
        point.x >= self.origin.x && point.x < self.right() && point.y >= self.origin.y && point.y < self.bottom()
    }

    pub(crate) fn start(&self, axis: Axis) -> f32 {
        self.origin.along(axis)
    }

    pub(crate) fn extent(&self, axis: Axis) -> f32 {
        self.size.along(axis)
    }

    /// Splits along `axis`: the first part spans `first_extent`, then `gap` is
    /// skipped, and the second part takes the remainder.
    pub(crate) fn split(&self, axis: Axis, first_extent: f32, gap: f32) -> (Rect, Rect) {
        let second_extent = (self.extent(axis) - first_extent - gap).max(0.0);
        match axis {
            Axis::X => (
                Rect::new(self.origin.x, self.origin.y, first_extent, self.size.height),
                Rect::new(self.origin.x + first_extent + gap, self.origin.y, second_extent, self.size.height),
            ),
            Axis::Y => (
                Rect::new(self.origin.x, self.origin.y, self.size.width, first_extent),
                Rect::new(self.origin.x, self.origin.y + first_extent + gap, self.size.width, second_extent),
            ),
        }
    }

    /// The strip of width `gap` between the two parts of [`Rect::split`].
    pub(crate) fn seam(&self, axis: Axis, first_extent: f32, gap: f32) -> Rect {
        match axis {
            Axis::X => Rect::new(self.origin.x + first_extent, self.origin.y, gap, self.size.height),
            Axis::Y => Rect::new(self.origin.x, self.origin.y + first_extent, self.size.width, gap),
        }
    }

    pub(crate) fn inflate(&self, amount: f32) -> Rect {
        Rect::new(
            self.origin.x - amount,
            self.origin.y - amount,
            self.size.width + 2.0 * amount,
            self.size.height + 2.0 * amount,
        )
    }
}

/// The direction of the seam that divides a split.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Orientation {
    /// A vertical seam: the first child is on the left, the second on the right.
    Vertical,
    /// A horizontal seam: the first child is on top, the second below.
    Horizontal,
}

impl Orientation {
    /// The axis along which a split with this seam divides its space.
    pub(crate) fn axis(self) -> Axis {
        match self {
            Orientation::Vertical => Axis::X,
            Orientation::Horizontal => Axis::Y,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Axis {
    X,
    Y,
}

impl Axis {
    pub(crate) fn cross(self) -> Axis {
        match self {
            Axis::X => Axis::Y,
            Axis::Y => Axis::X,
        }
    }
}
