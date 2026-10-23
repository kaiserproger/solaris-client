//! Explicitly consented local capture/input, with separately permission-gated pictures.
pub(crate) mod native;
pub(crate) mod texture;
pub(crate) mod relay;
#[cfg(target_os = "linux")]
mod wayland;
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

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct Face {
    pub block: [i32; 3],
    pub side: i32,
    /// Row length; block is the minimum-coordinate member, not image-left.
    pub width: u8,
}
impl Face {
    pub fn row(block: [i32; 3], side: i32, member: impl Fn([i32; 3]) -> bool) -> Option<Self> {
        if !(2..=5).contains(&side) || !member(block) { return None; }
        let axis = if side <= 3 { 0 } else { 2 };
        let mut first = block;
        let mut width = 1u8;
        loop {
            let mut previous = first;
            previous[axis] = previous[axis].checked_sub(1)?;
            if !member(previous) { break; }
            if width == 16 { return None; }
            first = previous;
            width += 1;
        }
        let mut next = block;
        loop {
            next[axis] = next[axis].checked_add(1)?;
            if !member(next) { break; }
            if width == 16 { return None; }
            width += 1;
        }
        Some(Self { block: first, side, width })
    }
    pub fn contains(self, block: [i32; 3], side: i32) -> bool {
        let axis = if self.side <= 3 { 0 } else { 2 };
        side == self.side && (0..3).all(|i| if i == axis {
            (0..i64::from(self.width)).contains(&(i64::from(block[i]) - i64::from(self.block[i])))
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
                + if right[i] < 0. { f64::from(self.width.saturating_sub(1)) } else { 0. }),
            right,
            down,
        )
    }
    pub fn pixel(self, point: [f64; 3], size: [u32; 2]) -> Option<[f64; 2]> {
        let (origin, right, down) = self.basis();
        let p: [f64; 3] = std::array::from_fn(|i| point[i] - origin[i]);
        let uv = [dot(p, right), dot(p, down)];
        let extent = fit(size, self.width);
        let bounds = [f64::from(self.width), 1.];
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
pub(crate) fn fit(size: [u32; 2], width: u8) -> [f64; 2] {
    // Preserve the outer bezel; internal seams are covered by the continuous image.
    let bounds = [f64::from(width) - 0.125, 0.875];
    let scale = (bounds[0] / f64::from(size[0].max(1)))
        .min(bounds[1] / f64::from(size[1].max(1)));
    [f64::from(size[0]) * scale, f64::from(size[1]) * scale]
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn every_face_maps_center_and_rejects_letterbox() {
        for side in 0..6 {
            let face = Face {
                block: [-3, 20, 7],
                side,
                width: 1,
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
            let face = Face::row(clicked, side, member).unwrap();
            assert_eq!(face.width, 3);
            let (o, r, d) = face.basis();
            let center = std::array::from_fn(|i| o[i] + 1.5 * r[i] + 0.5 * d[i]);
            assert_eq!(face.pixel(center, [1920, 640]), Some([960., 320.]));
            let left = std::array::from_fn(|i| center[i] - 0.5 * r[i]);
            let right = std::array::from_fn(|i| center[i] + 0.5 * r[i]);
            assert!(face.pixel(left, [1920, 640]).unwrap()[0] < 960.);
            assert!(face.pixel(right, [1920, 640]).unwrap()[0] > 960.);
            assert!(face.contains(clicked, side));
            assert_eq!(Face::row(clicked, side, |p| member(p) && p[axis] < 2).unwrap().width, 2);
            assert!(Face::row(clicked, side, |_| true).is_none());
        }
    }
}
