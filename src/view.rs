//! Any window size and UI scale: the reader works on a 1920x1080 picture at minimum UI scale (the reference), so a
//! game picture of another size or UI scale is first mapped onto it, and the boxes drawn back the same way.
//!
//! The game's interface scales with the window's height and with its UI scale setting, each part anchored to its
//! edge: the board and pop-ups to the centre, the Chromatic Prisoner column and the Cancel button to the right
//! edge, the objective and instruments panel to the left, the eye button to the top right.

use crate::reader::{Frame, HEIGHT, WIDTH};

/// The reference UI scale (the game's minimum, as its log reports it: "PageScale = 1 0.640 0.64").
pub const REFERENCE_UI_SCALE: f32 = 0.64;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct View {
    pub w: usize,
    pub h: usize,
    /// How many screen pixels one reference pixel takes.
    pub k: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Anchor {
    Centre,
    Left,
    Right,
    TopRight,
}

fn anchor(x: f32, y: f32) -> Anchor {
    if x >= 1560.0 && y < 200.0 {
        Anchor::TopRight
    } else if x >= 1640.0 {
        Anchor::Right
    } else if x < 430.0 && y > 60.0 && y < 470.0 {
        Anchor::Left
    } else {
        Anchor::Centre
    }
}

impl View {
    /// The view for a window of this size and UI scale (None: the UI scale isn't known, take the minimum).
    pub fn new(w: usize, h: usize, ui_scale: Option<f32>) -> View {
        let ui = ui_scale.unwrap_or(REFERENCE_UI_SCALE);
        View {
            w,
            h,
            k: h as f32 / HEIGHT as f32 * ui / REFERENCE_UI_SCALE,
        }
    }

    pub fn with_k(w: usize, h: usize, k: f32) -> View {
        View { w, h, k }
    }

    pub fn is_reference(&self) -> bool {
        self.w == WIDTH && self.h == HEIGHT && (self.k - 1.0).abs() < 0.005
    }

    /// Where a reference point is on the screen.
    pub fn to_screen(&self, x: f32, y: f32) -> (f32, f32) {
        let (w, h, k) = (self.w as f32, self.h as f32, self.k);
        let cx = w / 2.0 + (x - WIDTH as f32 / 2.0) * k;
        let cy = h / 2.0 + (y - HEIGHT as f32 / 2.0) * k;
        match anchor(x, y) {
            Anchor::Centre => (cx, cy),
            Anchor::Left => (x * k, cy),
            Anchor::Right => (w - (WIDTH as f32 - x) * k, cy),
            Anchor::TopRight => (w - (WIDTH as f32 - x) * k, y * k),
        }
    }

    /// Where a reference rectangle is on the screen (scaled about its centre, anchored as its centre is).
    pub fn rect_to_screen(&self, x0: f32, y0: f32, x1: f32, y1: f32) -> (f32, f32, f32, f32) {
        let (cx, cy) = self.to_screen((x0 + x1) / 2.0, (y0 + y1) / 2.0);
        let (hw, hh) = ((x1 - x0) / 2.0 * self.k, (y1 - y0) / 2.0 * self.k);
        (cx - hw, cy - hh, cx + hw, cy + hh)
    }

    /// The game picture mapped onto the reference picture (each reference pixel from its place on screen).
    pub fn to_reference(&self, f: &Frame) -> Frame {
        if self.is_reference() && f.w == WIDTH && f.h == HEIGHT {
            return f.clone();
        }
        self.region_to_reference(f, [0, 0, WIDTH, HEIGHT])
    }

    /// Only a region of the reference picture filled (the rest black): cheap, for trying scales.
    pub fn region_to_reference(&self, f: &Frame, [rx0, ry0, rx1, ry1]: [usize; 4]) -> Frame {
        let mut rgb = vec![0u8; WIDTH * HEIGHT * 3];
        for y in ry0..ry1 {
            for x in rx0..rx1 {
                let (sx, sy) = self.to_screen(x as f32 + 0.5, y as f32 + 0.5);
                let (sx, sy) = (sx - 0.5, sy - 0.5);
                if sx < 0.0 || sy < 0.0 || sx > (f.w - 1) as f32 || sy > (f.h - 1) as f32 {
                    continue;
                }
                // bilinear
                let (x0, y0) = (sx.floor() as usize, sy.floor() as usize);
                let (x1, y1) = ((x0 + 1).min(f.w - 1), (y0 + 1).min(f.h - 1));
                let (fx, fy) = (sx - x0 as f32, sy - y0 as f32);
                for c in 0..3 {
                    let p = |xx: usize, yy: usize| f.rgb[(yy * f.w + xx) * 3 + c] as f32;
                    let v = p(x0, y0) * (1.0 - fx) * (1.0 - fy)
                        + p(x1, y0) * fx * (1.0 - fy)
                        + p(x0, y1) * (1.0 - fx) * fy
                        + p(x1, y1) * fx * fy;
                    rgb[(y * WIDTH + x) * 3 + c] = v.round().clamp(0.0, 255.0) as u8;
                }
            }
        }
        Frame {
            w: WIDTH,
            h: HEIGHT,
            rgb,
        }
    }
}

/// Finds the scale of a board picture of any size: the one where the Interrogation Room's picture matches best, tried
/// from half to twice the size the window's height suggests. None when the board isn't on screen.
pub fn calibrate(f: &Frame) -> Option<(View, f32)> {
    let base = f.h as f32 / HEIGHT as f32;
    let region = crate::reader::office_region();
    let mut best: Option<(View, f32)> = None;
    let mut k = 0.5;
    while k <= 2.0 {
        let v = View::with_k(f.w, f.h, base * k);
        let m = crate::reader::office_match(&v.region_to_reference(f, region));
        if best.map_or(true, |b| m > b.1) {
            best = Some((v, m));
        }
        k *= 1.02;
    }
    // refine around the best
    if let Some((v0, _)) = best {
        let mut k = v0.k * 0.985;
        while k <= v0.k * 1.015 {
            let v = View::with_k(f.w, f.h, k);
            let m = crate::reader::office_match(&v.region_to_reference(f, region));
            if m > best.unwrap().1 {
                best = Some((v, m));
            }
            k *= 1.003;
        }
    }
    best.filter(|b| b.1 > 0.8)
}
