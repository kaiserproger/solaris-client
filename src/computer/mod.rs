//! Explicitly consented local capture/input, with separately permission-gated pictures.
pub(crate) mod native;
pub(crate) mod texture;
pub(crate) mod relay;
pub(crate) mod audio;
#[cfg(target_os = "linux")]
pub(crate) mod nested;
#[cfg(target_os = "linux")]
mod wayland;
#[cfg(windows)]
mod loopback;
#[cfg(windows)]
mod windows;

pub struct Frame {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}
#[derive(Clone, Debug)]
pub struct WindowInfo {
    pub id: u64,
    pub title: String,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DesktopKey {
    /// Linux evdev physical key code, translated by non-Linux backends.
    Code(u32),
    Char(char),
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PointerButton {
    Left,
    Right,
    Middle,
}

pub trait DesktopSource: Send {
    fn next_frame(&mut self) -> Result<Option<Frame>, String>;
    /// True only after the backend has obtained both pointer and keyboard access.
    fn control_granted(&self) -> bool;
    fn send_pointer(&mut self, x: f64, y: f64) -> Result<(), String>;
    fn send_button(&mut self, button: PointerButton, pressed: bool) -> Result<(), String>;
    fn send_scroll(&mut self, dx: f64, dy: f64) -> Result<(), String>;
    fn send_key(&mut self, key: DesktopKey, pressed: bool) -> Result<(), String>;
    /// Audio of what this source shows, if it has any. Linux portal windows do
    /// not (a whole-system monitor would feed Solaris's own playback back).
    fn audio(&mut self) -> Option<Box<dyn audio::AudioSource>> {
        None
    }
    /// True when `audio()` is the app's only output (a private sink), so the
    /// owner hears it through the world mixer and capture runs without demand.
    fn private_audio(&self) -> bool {
        false
    }
    fn close(&mut self);
}

#[cfg(target_os = "linux")]
pub use wayland::{list_windows, open};
#[cfg(windows)]
pub use windows::{list_windows, open};
#[cfg(not(any(windows, target_os = "linux")))]
pub fn list_windows() -> Result<Vec<WindowInfo>, String> {
    Err("Desktop capture requires Linux Wayland or Windows".into())
}
#[cfg(not(any(windows, target_os = "linux")))]
pub fn open(_: u64) -> Result<Box<dyn DesktopSource>, String> {
    Err("Desktop capture requires Linux Wayland or Windows".into())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RectangleError {
    InvalidFace,
    WorldBoundary,
    TooLarge { vertical: bool },
    Missing([i32; 3]),
}
impl RectangleError {
    pub fn message(self, expected_side: i32, actual_side: Option<i32>) -> String {
        fn direction(side: i32) -> &'static str {
            match side { 2 => "north", 3 => "south", 4 => "west", 5 => "east", _ => "unknown" }
        }
        match self {
            Self::InvalidFace => "Click the front of a monitor".into(),
            Self::WorldBoundary => "Monitor rectangle reaches the world coordinate boundary".into(),
            Self::TooLarge { vertical } => format!("Monitor wall is at least 17 blocks {}; maximum is 16",
                if vertical { "high" } else { "wide" }),
            Self::Missing([x, y, z]) => match actual_side {
                Some(actual) if actual != expected_side => format!(
                    "Monitor at {x},{y},{z} faces {}; wall faces {}",
                    direction(actual), direction(expected_side)),
                _ => format!("Monitor rectangle has a missing monitor at {x},{y},{z}"),
            },
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct Face {
    pub block: [i32; 3],
    pub side: i32,
    /// Rectangle dimensions; block is its minimum horizontal/Y coordinate.
    pub width: u8,
    pub height: u8,
}
impl Face {
    pub fn rectangle(block: [i32; 3], side: i32, member: impl Fn([i32; 3]) -> bool) -> Result<Self, RectangleError> {
        if !(2..=5).contains(&side) { return Err(RectangleError::InvalidFace); }
        if !member(block) { return Err(RectangleError::Missing(block)); }
        let axis = if side <= 3 { 0 } else { 2 };
        let mut minimum = [block[axis], block[1]];
        let mut maximum = minimum;
        let position = |horizontal, y| {
            let mut point = block;
            point[axis] = horizontal;
            point[1] = y;
            point
        };
        // Expand the connected coplanar component's bounds without heap storage.
        // A final full-area check rejects L shapes and holes rather than silently
        // selecting a smaller row depending on which block was clicked.
        loop {
            let mut grew = false;
            for dimension in 0..2 {
                for positive in [false, true] {
                    let edge = if positive { maximum[dimension].checked_add(1) } else { minimum[dimension].checked_sub(1) }
                        .ok_or(RectangleError::WorldBoundary)?;
                    let other = 1 - dimension;
                    let connected = (minimum[other]..=maximum[other]).any(|coordinate| {
                        member(if dimension == 0 { position(edge, coordinate) } else { position(coordinate, edge) })
                    });
                    if connected {
                        if maximum[dimension] - minimum[dimension] >= 15 {
                            return Err(RectangleError::TooLarge { vertical: dimension == 1 });
                        }
                        if positive { maximum[dimension] = edge; } else { minimum[dimension] = edge; }
                        grew = true;
                    }
                }
            }
            if !grew { break; }
        }
        for y in minimum[1]..=maximum[1] {
            for horizontal in minimum[0]..=maximum[0] {
                if !member(position(horizontal, y)) {
                    return Err(RectangleError::Missing(position(horizontal, y)));
                }
            }
        }
        Ok(Self {
            block: position(minimum[0], minimum[1]), side,
            width: (maximum[0] - minimum[0] + 1) as u8,
            height: (maximum[1] - minimum[1] + 1) as u8,
        })
    }
    /// World position of the rectangle's face centre (where its audio is heard).
    pub fn centre(self) -> [f64; 3] {
        let [x, y, z] = self.block.map(f64::from);
        let half = f64::from(self.width) / 2.;
        let up = y + f64::from(self.height) / 2.;
        match self.side {
            2 => [x + half, up, z],
            3 => [x + half, up, z + 1.],
            4 => [x, up, z + half],
            _ => [x + 1., up, z + half],
        }
    }
    pub fn contains(self, block: [i32; 3], side: i32) -> bool {
        let axis = if self.side <= 3 { 0 } else { 2 };
        side == self.side && (0..3).all(|i| if i == axis || i == 1 {
            let length = if i == axis { self.width } else { self.height };
            (0..i64::from(length)).contains(&(i64::from(block[i]) - i64::from(self.block[i])))
        } else { block[i] == self.block[i] })
    }
    // Origin at upper left as viewed from outside, with image-right and image-down axes.
    pub fn basis(self) -> ([f64; 3], [f64; 3], [f64; 3]) {
        let (corner, right, down) = match self.side {
            0 => ([0., -0.002, 0.], [1., 0., 0.], [0., 0., 1.]),
            1 => ([0., 1.002, 1.], [1., 0., 0.], [0., 0., -1.]),
            2 => ([1., 1., -0.002], [-1., 0., 0.], [0., -1., 0.]),
            3 => ([0., 1., 1.002], [1., 0., 0.], [0., -1., 0.]),
            4 => ([-0.002, 1., 0.], [0., 0., 1.], [0., -1., 0.]),
            _ => ([1.002, 1., 1.], [0., 0., -1.], [0., -1., 0.]),
        };
        (
            std::array::from_fn(|i| f64::from(self.block[i]) + corner[i]
                + if right[i] < 0. { f64::from(self.width.saturating_sub(1)) } else { 0. }
                + if down[i] < 0. { f64::from(self.height.saturating_sub(1)) } else { 0. }),
            right,
            down,
        )
    }
    pub fn pixel(self, point: [f64; 3], size: [u32; 2]) -> Option<[f64; 2]> {
        let (origin, right, down) = self.basis();
        let p: [f64; 3] = std::array::from_fn(|i| point[i] - origin[i]);
        let uv = [dot(p, right), dot(p, down)];
        let extent = fit(size, self.width, self.height);
        let bounds = [f64::from(self.width), f64::from(self.height)];
        let normalized: [f64; 2] =
            std::array::from_fn(|i| (uv[i] - (bounds[i] - extent[i]) * 0.5) / extent[i]);
        normalized
            .iter()
            .all(|n| (0. ..1.).contains(n))
            .then(|| std::array::from_fn(|i| normalized[i] * f64::from(size[i])))
    }
}
fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a.into_iter().zip(b).map(|(a, b)| a * b).sum()
}
pub(crate) fn fit(size: [u32; 2], width: u8, height: u8) -> [f64; 2] {
    // Preserve one outer bezel; the full-face quad covers every internal seam.
    let bounds = [f64::from(width) - 0.125, f64::from(height) - 0.125];
    let scale = (bounds[0] / f64::from(size[0].max(1)))
        .min(bounds[1] / f64::from(size[1].max(1)));
    [f64::from(size[0]) * scale, f64::from(size[1]) * scale]
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rectangle_errors_name_wrong_facing_holes_and_oversized_dimensions() {
        let error = RectangleError::Missing([12, 80, -7]);
        assert_eq!(error.message(4, Some(5)), "Monitor at 12,80,-7 faces east; wall faces west");
        assert_eq!(error.message(4, None), "Monitor rectangle has a missing monitor at 12,80,-7");
        let too_wide = Face::rectangle([0, 0, 0], 2, |[x, y, z]| (0..17).contains(&x) && y == 0 && z == 0);
        assert_eq!(too_wide, Err(RectangleError::TooLarge { vertical: false }));
        assert!(too_wide.unwrap_err().message(2, None).contains("17 blocks wide"));
    }

    #[test]
    fn face_centre_sits_on_the_front_plane_of_the_rectangle() {
        let face = |side| Face { block: [10, 64, -4], side, width: 4, height: 2 };
        assert_eq!(face(2).centre(), [12., 65., -4.]);
        assert_eq!(face(3).centre(), [12., 65., -3.]);
        assert_eq!(face(4).centre(), [10., 65., -2.]);
        assert_eq!(face(5).centre(), [11., 65., -2.]);
    }
    #[test]
    fn every_face_maps_center_and_rejects_letterbox() {
        for side in 0..6 {
            let face = Face {
                block: [-3, 20, 7],
                side,
                width: 1,
                height: 1,
            };
            let (o, r, d) = face.basis();
            let center = std::array::from_fn(|i| o[i] + r[i] * 0.5 + d[i] * 0.5);
            assert_eq!(face.pixel(center, [1920, 1080]), Some([960., 540.]));
            let top = std::array::from_fn(|i| o[i] + r[i] * 0.5);
            assert_eq!(face.pixel(top, [1920, 1080]), None);
        }
    }
    #[test]
    fn horizontal_rows_share_one_uv_space_and_recompute_after_edits() {
        for side in 2..=5 {
            let axis = if side <= 3 { 0 } else { 2 };
            let member = |p: [i32; 3]| p[1] == 4 && p[2 - axis] == 7 && (0..3).contains(&p[axis]);
            let mut clicked = [7, 4, 7];
            clicked[axis] = 1;
            let face = Face::rectangle(clicked, side, member).unwrap();
            assert_eq!(face.width, 3);
            let (o, r, d) = face.basis();
            let center = std::array::from_fn(|i| o[i] + 1.5 * r[i] + 0.5 * d[i]);
            assert_eq!(face.pixel(center, [1920, 640]), Some([960., 320.]));
            let left = std::array::from_fn(|i| center[i] - 0.5 * r[i]);
            let right = std::array::from_fn(|i| center[i] + 0.5 * r[i]);
            assert!(face.pixel(left, [1920, 640]).unwrap()[0] < 960.);
            assert!(face.pixel(right, [1920, 640]).unwrap()[0] > 960.);
            assert!(face.contains(clicked, side));
            assert_eq!(Face::rectangle(clicked, side, |p| member(p) && p[axis] < 2).unwrap().width, 2);
            assert!(Face::rectangle(clicked, side, |_| true).is_err());
        }
    }
    #[test]
    fn rectangles_share_one_surface_from_every_member_and_reject_holes() {
        for side in 2..=5 {
            let axis = if side <= 3 { 0 } else { 2 };
            let member = |p: [i32; 3]| (4..6).contains(&p[1]) && p[2 - axis] == 7 && (0..4).contains(&p[axis]);
            let mut anchor = [7, 4, 7];
            anchor[axis] = 0;
            let face = Face::rectangle(anchor, side, member).unwrap();
            assert_eq!((face.width, face.height), (4, 2));
            for horizontal in 0..4 {
                for y in 4..6 {
                    let mut clicked = anchor; clicked[axis] = horizontal; clicked[1] = y;
                    assert_eq!(Face::rectangle(clicked, side, member), Ok(face));
                    assert!(face.contains(clicked, side));
                }
            }
            let (origin, right, down) = face.basis();
            let center = std::array::from_fn(|i| origin[i] + right[i] * 2. + down[i]);
            assert_eq!(face.pixel(center, [1280, 640]), Some([640., 320.]));
            let upper = std::array::from_fn(|i| center[i] - down[i] * 0.5);
            assert!(face.pixel(upper, [1280, 640]).unwrap()[1] < 320.);
            assert!(matches!(Face::rectangle(anchor, side, |p| member(p) && !(p[axis] == 1 && p[1] == 5)), Err(RectangleError::Missing(_))));
            assert_eq!(Face::rectangle(anchor, side, |p| p[2-axis] == 7 && p[axis] == 0 && (4..21).contains(&p[1])), Err(RectangleError::TooLarge { vertical: true }));
        }
    }
}
