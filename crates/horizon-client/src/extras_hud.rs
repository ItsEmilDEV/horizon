//! HUD boxes in the corners: the minimap's place and mods' panels, small
//! boxes in GUI pixels stacked per corner (`docs/extras.md`).
use crate::{
    extras::Corner,
    hud::{HudAtlas, HudBatch, Rect, rgba},
};

/// The gap between an overlay and the screen edge or the next overlay.
const MARGIN: f32 = 2.0;
const BACKGROUND: u32 = 0x7800_0000;

/// Where the next overlay goes in each corner.
pub struct Corners {
    width: f32,
    height: f32,
    /// Distance from the corner's top or bottom edge, per `Corner::ALL`.
    used: [f32; 4],
}
impl Corners {
    /// `top_right` reserves the status effect icons' rows.
    pub fn new(width: f32, height: f32, top_right: f32) -> Self {
        Self {
            width,
            height,
            used: [0.0, top_right, 0.0, 0.0],
        }
    }
    /// The top-left position of a `w` x `h` box in `corner`, below (or
    /// above) what the corner already holds.
    pub fn place(&mut self, corner: Corner, w: f32, h: f32) -> (f32, f32) {
        let i = Corner::ALL.iter().position(|c| *c == corner).unwrap_or(0);
        let edge = self.used[i] + MARGIN;
        self.used[i] = edge + h;
        let x = if corner.left() {
            MARGIN
        } else {
            self.width - MARGIN - w
        };
        let y = if corner.top() {
            edge
        } else {
            self.height - edge - h
        };
        (x, y)
    }
}

/// How the minimap shows: its corner and side in GUI pixels, its image (an
/// atlas rect), the player arrow (the map item's decoration sprite) turned
/// `arrow` degrees clockwise from up, and the rim's N toward `north` when the
/// map turns.
pub struct MinimapLook<'a> {
    pub corner: Corner,
    pub size: f32,
    pub image: Rect,
    pub arrow: f32,
    pub north: Option<(f32, f32)>,
    pub dots: &'a [Dot],
    pub canvas: CanvasLook,
}
/// Where the painted canvas lies in the world, so the GPU can shift and turn it.
#[derive(Clone, Copy, Debug)]
pub struct CanvasLook {
    pub centre: (f64, f64),
    pub blocks_per_texel: f64,
    pub player: (f64, f64),
    /// The world's right and forward as the map's x and up (`View::axes`).
    pub right: [f64; 2],
    pub forward: [f64; 2],
    pub round: bool,
    pub texels: f32,
    pub window: f32,
}
impl CanvasLook {
    /// The canvas texture coordinate under a point `(dx, dy)` GUI pixels from the map's centre.
    fn uv(&self, image: Rect, size: f32, dx: f32, dy: f32) -> [f32; 2] {
        let scale = f64::from(self.window / size);
        let (u, v) = (f64::from(dx) * scale, f64::from(dy) * scale);
        let bpp = self.blocks_per_texel;
        let wx = self.player.0 + bpp * (u * self.right[0] - v * self.forward[0]);
        let wz = self.player.1 + bpp * (u * self.right[1] - v * self.forward[1]);
        let half = f64::from(self.texels) / 2.0;
        let tx = ((wx - self.centre.0) / bpp + half) / f64::from(self.texels);
        let ty = ((wz - self.centre.1) / bpp + half) / f64::from(self.texels);
        [
            image.u0 + (image.u1 - image.u0) * tx as f32,
            image.v0 + (image.v1 - image.v0) * ty as f32,
        ]
    }
}
/// An entity marker: its offset from the map centre in GUI pixels
/// (x right, y down) and its 0xAARRGGBB colour.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Dot {
    pub x: f32,
    pub y: f32,
    pub color: u32,
}
pub fn draw_minimap(
    batch: &mut HudBatch,
    atlas: &HudAtlas,
    corners: &mut Corners,
    look: &MinimapLook<'_>,
) {
    let MinimapLook {
        corner,
        size,
        image,
        arrow,
        north,
        dots,
        canvas,
    } = *look;
    let (x, y) = corners.place(corner, size, size);
    let (cx, cy) = (x + size / 2.0, y + size / 2.0);
    let half = size / 2.0;
    let edge = rgba(10, 10, 10, 230);
    if canvas.round {
        const SEGMENTS: usize = 64;
        let ring = |i: usize, r: f32| {
            let a = i as f32 / SEGMENTS as f32 * std::f32::consts::TAU;
            (a.cos() * r, a.sin() * r)
        };
        for i in 0..SEGMENTS {
            let (a, b) = (ring(i, half), ring(i + 1, half));
            let at = |p: (f32, f32)| ([cx + p.0, cy + p.1], canvas.uv(image, size, p.0, p.1));
            let (pa, pb) = (at(a), at(b));
            let c = ([cx, cy], canvas.uv(image, size, 0.0, 0.0));
            batch.quad([c.0, pa.0, pb.0, pb.0], [c.1, pa.1, pb.1, pb.1], u32::MAX);
            let (ia, ib) = (ring(i, half - 1.5), ring(i + 1, half - 1.5));
            batch.fill_quad(
                atlas,
                [
                    [cx + a.0, cy + a.1],
                    [cx + b.0, cy + b.1],
                    [cx + ib.0, cy + ib.1],
                    [cx + ia.0, cy + ia.1],
                ],
                edge,
            );
        }
    } else {
        let corner = |dx: f32, dy: f32| ([cx + dx, cy + dy], canvas.uv(image, size, dx, dy));
        let (a, b, c, d) = (
            corner(-half, -half),
            corner(half, -half),
            corner(half, half),
            corner(-half, half),
        );
        batch.quad([a.0, b.0, c.0, d.0], [a.1, b.1, c.1, d.1], u32::MAX);
        batch.fill(atlas, x, y, size, 1.0, edge);
        batch.fill(atlas, x, y + size - 1.0, size, 1.0, edge);
        batch.fill(atlas, x, y, 1.0, size, edge);
        batch.fill(atlas, x + size - 1.0, y, 1.0, size, edge);
    }
    for d in dots {
        batch.fill(atlas, cx + d.x - 2.0, cy + d.y - 2.0, 4.0, 4.0, 0xFF00_0000);
        batch.fill(atlas, cx + d.x - 1.5, cy + d.y - 1.5, 3.0, 3.0, d.color);
    }
    if let Some(r) = atlas.sprites.get("map/decorations/player").copied() {
        // MapRenderer's decorations draw the sprite flipped: its arrow
        // points down the texture, so up on screen is half a turn.
        let (s, c) = (arrow + 180.0).to_radians().sin_cos();
        let p = |dx: f32, dy: f32| [cx + dx * c - dy * s, cy + dx * s + dy * c];
        let h = 4.0;
        batch.quad(
            [p(-h, h), p(h, h), p(h, -h), p(-h, -h)],
            [[r.u0, r.v0], [r.u1, r.v0], [r.u1, r.v1], [r.u0, r.v1]],
            u32::MAX,
        );
    }
    if let Some((nx, ny)) = north {
        let reach = size / 2.0 - 6.0;
        let tw = HudBatch::text_width(atlas, "N");
        let (tx, ty) = (cx + nx * reach - tw / 2.0, cy + ny * reach - 4.0);
        batch.text(
            atlas,
            "N",
            tx.round(),
            ty.round(),
            rgba(255, 255, 255, 255),
            true,
        );
    }
}

/// A box of text lines (a mod's HUD panel): `(text, 0xRRGGBB)` each.
pub fn draw_lines(
    batch: &mut HudBatch,
    atlas: &HudAtlas,
    corners: &mut Corners,
    corner: Corner,
    lines: &[(&str, u32)],
) {
    if lines.is_empty() {
        return;
    }
    let w = lines
        .iter()
        .map(|(text, _)| HudBatch::text_width(atlas, text))
        .fold(0.0, f32::max)
        + 4.0;
    let h = 10.0 * lines.len() as f32 + 2.0;
    let (x, y) = corners.place(corner, w, h);
    batch.fill(atlas, x, y, w, h, BACKGROUND);
    for (i, (text, color)) in lines.iter().enumerate() {
        let [_, r, g, b] = color.to_be_bytes();
        let y = y + 2.0 + 10.0 * i as f32;
        batch.text(atlas, text, x + 2.0, y, rgba(r, g, b, 255), true);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn corners_stack_away_from_their_edge() {
        let mut c = Corners::new(400.0, 300.0, 26.0);
        assert_eq!(c.place(Corner::TopLeft, 50.0, 12.0), (2.0, 2.0));
        assert_eq!(c.place(Corner::TopLeft, 70.0, 20.0), (2.0, 16.0));
        // Top right starts under the effect icons.
        assert_eq!(c.place(Corner::TopRight, 50.0, 12.0), (348.0, 28.0));
        assert_eq!(c.place(Corner::BottomRight, 50.0, 12.0), (348.0, 286.0));
        assert_eq!(c.place(Corner::BottomRight, 50.0, 10.0), (348.0, 274.0));
        assert_eq!(c.place(Corner::BottomLeft, 10.0, 10.0), (2.0, 288.0));
    }
}
