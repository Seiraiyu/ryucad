//! Colour blindness (design 199, Accessibility): the colours that must
//! stay apart, simulated for protanopia, deuteranopia and tritanopia
//! (Machado, Oliveira and Fernandes 2009, severity 1, on linear RGB) and
//! compared by CIEDE2000 (Sharma, Wu and Dalal 2005).

use super::Rgb;
use ryu_math::{atan2, cbrt, cos, exp, hypot, pow, sin, sqrt};

/// Machado et al.'s matrices at severity 1.0, by name; `normal` is the
/// identity.
pub const VISIONS: [(&str, [[f64; 3]; 3]); 4] = [
    (
        "normal",
        [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
    ),
    (
        "protanopia",
        [
            [0.152286, 1.052583, -0.204868],
            [0.114503, 0.786281, 0.099216],
            [-0.003882, -0.048116, 1.051998],
        ],
    ),
    (
        "deuteranopia",
        [
            [0.367322, 0.860646, -0.227968],
            [0.280085, 0.672501, 0.047413],
            [-0.011820, 0.042940, 0.968881],
        ],
    ),
    (
        "tritanopia",
        [
            [1.255528, -0.076749, -0.178779],
            [-0.078411, 0.930809, 0.147602],
            [0.004733, 0.691367, 0.303900],
        ],
    ),
];

fn linear(c: f64) -> f64 {
    if c <= 0.04045 {
        c / 12.92
    } else {
        pow((c + 0.055) / 1.055, 2.4)
    }
}

/// An sRGB colour as someone with `m`'s vision sees it, in linear RGB.
pub fn seen(c: Rgb, m: &[[f64; 3]; 3]) -> [f64; 3] {
    let l = c.map(|x| linear(f64::from(x)));
    [0, 1, 2].map(|i| (0..3).map(|j| m[i][j] * l[j]).sum::<f64>().clamp(0.0, 1.0))
}

/// CIELAB (D65) of a linear RGB colour.
pub fn lab(l: [f64; 3]) -> [f64; 3] {
    let [r, g, b] = l;
    let x = 0.4124564 * r + 0.3575761 * g + 0.1804375 * b;
    let y = 0.2126729 * r + 0.7151522 * g + 0.0721750 * b;
    let z = 0.0193339 * r + 0.1191920 * g + 0.9503041 * b;
    let f = |t: f64| {
        if t > 216.0 / 24389.0 {
            cbrt(t)
        } else {
            (24389.0 / 27.0 * t + 16.0) / 116.0
        }
    };
    let (fx, fy, fz) = (f(x / 0.95047), f(y), f(z / 1.08883));
    [116.0 * fy - 16.0, 500.0 * (fx - fy), 200.0 * (fy - fz)]
}

/// CIEDE2000's difference of two CIELAB colours.
pub fn de2000(a: [f64; 3], b: [f64; 3]) -> f64 {
    let (l1, a1, b1) = (a[0], a[1], a[2]);
    let (l2, a2, b2) = (b[0], b[1], b[2]);
    let seventh = |x: f64| pow(x, 7.0);
    let c1 = hypot(a1, b1);
    let c2 = hypot(a2, b2);
    let cb = (c1 + c2) / 2.0;
    let g = 0.5 * (1.0 - sqrt(seventh(cb) / (seventh(cb) + seventh(25.0))));
    let (a1p, a2p) = ((1.0 + g) * a1, (1.0 + g) * a2);
    let (c1p, c2p) = (hypot(a1p, b1), hypot(a2p, b2));
    let hue = |b: f64, a: f64| {
        if b == 0.0 && a == 0.0 {
            0.0
        } else {
            atan2(b, a).to_degrees().rem_euclid(360.0)
        }
    };
    let (h1p, h2p) = (hue(b1, a1p), hue(b2, a2p));
    let dl = l2 - l1;
    let dc = c2p - c1p;
    let dh = if c1p * c2p == 0.0 {
        0.0
    } else {
        let d = h2p - h1p;
        if d > 180.0 {
            d - 360.0
        } else if d < -180.0 {
            d + 360.0
        } else {
            d
        }
    };
    let dhh = 2.0 * sqrt(c1p * c2p) * sin(dh.to_radians() / 2.0);
    let lb = (l1 + l2) / 2.0;
    let cbp = (c1p + c2p) / 2.0;
    let hb = if c1p * c2p == 0.0 {
        h1p + h2p
    } else if (h1p - h2p).abs() <= 180.0 {
        (h1p + h2p) / 2.0
    } else if h1p + h2p < 360.0 {
        (h1p + h2p + 360.0) / 2.0
    } else {
        (h1p + h2p - 360.0) / 2.0
    };
    let cosd = |d: f64| cos(d.to_radians());
    let sq = |x: f64| x * x;
    let t = 1.0 - 0.17 * cosd(hb - 30.0) + 0.24 * cosd(2.0 * hb) + 0.32 * cosd(3.0 * hb + 6.0)
        - 0.20 * cosd(4.0 * hb - 63.0);
    let dth = 30.0 * exp(-sq((hb - 275.0) / 25.0));
    let rc = 2.0 * sqrt(seventh(cbp) / (seventh(cbp) + seventh(25.0)));
    let sl = 1.0 + 0.015 * sq(lb - 50.0) / sqrt(20.0 + sq(lb - 50.0));
    let sc = 1.0 + 0.045 * cbp;
    let sh = 1.0 + 0.015 * cbp * t;
    let rt = -sin((2.0 * dth).to_radians()) * rc;
    sqrt(sq(dl / sl) + sq(dc / sc) + sq(dhh / sh) + rt * (dc / sc) * (dhh / sh))
}

/// How far apart two sRGB colours look with vision `m`.
pub fn apart(a: Rgb, b: Rgb, m: &[[f64; 3]; 3]) -> f64 {
    de2000(lab(seen(a, m)), lab(seen(b, m)))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Sharma, Wu and Dalal's test data (pairs 1, 7, 13, 25 and 34).
    #[test]
    fn ciede2000_is_sharma_s() {
        for (a, b, want) in [
            ([50.0, 2.6772, -79.7751], [50.0, 0.0, -82.7485], 2.0425),
            ([50.0, 0.0, 0.0], [50.0, -1.0, 2.0], 2.3669),
            ([50.0, 2.4900, -0.0010], [50.0, -2.4900, 0.0011], 7.2195),
            (
                [60.2574, -34.0099, 36.2677],
                [60.4626, -34.1751, 39.4387],
                1.2644,
            ),
            (
                [22.7233, 20.0904, -46.6940],
                [23.0331, 14.9730, -42.5619],
                2.0373,
            ),
        ] {
            let got = de2000(a, b);
            assert!((got - want).abs() < 1e-4, "{a:?} {b:?}: {got} not {want}");
        }
    }

    #[test]
    fn simulations_keep_grey_grey_and_lose_red_against_green() {
        for (name, m) in VISIONS {
            let g = seen([0.5; 3], &m);
            assert!(g.iter().all(|x| (x - g[0]).abs() < 2e-3), "{name}: {g:?}");
        }
        let (red, green) = ([0.8, 0.2, 0.2], [0.3, 0.6, 0.2]);
        let normal = apart(red, green, &VISIONS[0].1);
        let deutan = apart(red, green, &VISIONS[2].1);
        assert!(normal > 40.0 && deutan < normal / 2.0, "{normal} {deutan}");
    }
}
