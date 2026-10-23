//! Explicitly consented, client-local desktop capture. No network transport.
pub(crate) mod native;
pub(crate) mod texture;
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Face {
    pub block: [i32; 3],
    pub side: i32,
}
impl Face {
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
            std::array::from_fn(|i| f64::from(self.block[i]) + corner[i]),
            right,
            down,
        )
    }
    pub fn pixel(self, point: [f64; 3], size: [u32; 2]) -> Option<[f64; 2]> {
        let (origin, right, down) = self.basis();
        let p: [f64; 3] = std::array::from_fn(|i| point[i] - origin[i]);
        let uv = [dot(p, right), dot(p, down)];
        let extent = fit(size);
        let normalized: [f64; 2] =
            std::array::from_fn(|i| (uv[i] - (1. - extent[i]) * 0.5) / extent[i]);
        normalized
            .iter()
            .all(|n| (0. ..1.).contains(n))
            .then(|| std::array::from_fn(|i| normalized[i] * f64::from(size[i])))
    }
}
fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a.into_iter().zip(b).map(|(a, b)| a * b).sum()
}
pub(crate) fn fit(size: [u32; 2]) -> [f64; 2] {
    let max = f64::from(size[0].max(size[1]).max(1));
    [f64::from(size[0]) / max, f64::from(size[1]) / max]
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
            };
            let (o, r, d) = face.basis();
            let center = std::array::from_fn(|i| o[i] + r[i] * 0.5 + d[i] * 0.5);
            assert_eq!(face.pixel(center, [1920, 1080]), Some([960., 540.]));
            let top = std::array::from_fn(|i| o[i] + r[i] * 0.5);
            assert_eq!(face.pixel(top, [1920, 1080]), None);
        }
    }
}
