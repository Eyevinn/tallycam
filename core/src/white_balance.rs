//! Manual white balance as colour temperature (K) and tint, for cameras that
//! take raw RGB gains and a colour matrix (Android Camera2's
//! `COLOR_CORRECTION_MODE_TRANSFORM_MATRIX` with `COLOR_CORRECTION_GAINS`
//! and `COLOR_CORRECTION_TRANSFORM`), the way iOS takes
//! `AVCaptureDevice.WhiteBalanceTemperatureAndTintValues`.
//!
//! The colour model is the DNG one (Adobe DNG Specification 1.4, chapter 6,
//! "Mapping Camera Color Space to CIE XYZ Space"), ported from the DNG SDK:
//!
//! - Temperature and tint to and from CIE 1931 xy: Robertson's method on the
//!   DNG SDK's 31-line isotherm table (`dng_temperature.cpp`). Tint is the
//!   distance from the Planckian locus along the isotherm in CIE 1960 uv,
//!   times -3000 (Adobe tint units, the ±150 scale of AVFoundation and
//!   Lightroom).
//! - The XYZ-to-camera matrix for an illuminant: `CameraCalibration *
//!   ColorMatrix` of the two reference illuminants, interpolated linearly in
//!   inverse temperature and clamped outside them (`dng_color_spec.cpp`,
//!   `FindXYZtoCamera`). AnalogBalance is the identity.
//! - Camera neutral: that matrix times the illuminant's XYZ (Y = 1), scaled
//!   so its largest channel is 1 (`SetWhiteXY`). The gains are its inverse,
//!   so the smallest gain is 1, as Camera2 wants.
//! - Neutral to illuminant (the auto gains a capture result reports back to
//!   K and tint): the DNG SDK's `NeutralToXY` fixed-point iteration.
//! - Camera RGB to linear sRGB: the interpolated ForwardMatrix (white-balanced
//!   reference camera RGB to XYZ D50) when the camera has them, else the
//!   inverse of the colour matrix with a Bradford adaptation to D50, then
//!   Bradford D50 to D65 and XYZ to linear sRGB.
//!
//! **Tint sign.** Positive tint is an illuminant above the Planckian locus
//! (greener light, Duv > 0): the gains take green out, so the picture moves
//! towards magenta, as Lightroom's and (by its ±150 range and the descriptions
//! available) AVFoundation's tint does. Daylight sits at about +10 (D65 is
//! 6504 K, tint +9.8 here; Lightroom's Daylight preset is 5500 K, +10).
//!
//! Pure functions; no allocation.

/// A 3x3 matrix, row-major.
pub type Mat3 = [[f64; 3]; 3];

pub const IDENTITY: Mat3 = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];

/// The temperature range the functions clamp to, K: the apps' range (above
/// 10000 K AVFoundation's gains run out, and Android follows iOS).
pub const MIN_TEMPERATURE_K: f64 = 2000.0;
pub const MAX_TEMPERATURE_K: f64 = 10000.0;
/// The tint range the functions clamp to (Adobe tint units, as AVFoundation).
/// Green tints at low temperatures are clamped further, see [`max_tint`].
pub const MAX_TINT: f64 = 150.0;
/// The largest x + y (CIE 1931) of an illuminant the functions take. Near
/// the yellow edge of the chromaticity diagram (x + y = 1: no blue) the DNG
/// SDK pins the illuminant's XYZ, so the gains no longer depend on the tint
/// and the inverse gives back a different one (2000 K +33 came back as
/// 2004 K +32, 2000 K +150 as 2403 K +53). 0.98 keeps a margin.
pub const MAX_X_PLUS_Y: f64 = 0.98;

/// One reference illuminant of a camera's colour calibration, as Camera2's
/// `CameraCharacteristics` gives it (DNG tags in brackets).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Reference {
    /// EXIF LightSource value: `SENSOR_REFERENCE_ILLUMINANTn`
    /// (CalibrationIlluminantN).
    pub illuminant: u32,
    /// CIE XYZ to reference camera RGB: `SENSOR_COLOR_TRANSFORMn`
    /// (ColorMatrixN).
    pub color_matrix: Mat3,
    /// Reference camera RGB to this unit's camera RGB:
    /// `SENSOR_CALIBRATION_TRANSFORMn` (CameraCalibrationN); [`IDENTITY`]
    /// if the camera has none.
    pub calibration: Mat3,
    /// White-balanced reference camera RGB to CIE XYZ D50:
    /// `SENSOR_FORWARD_MATRIXn` (ForwardMatrixN), if any.
    pub forward_matrix: Option<Mat3>,
}

/// A camera's colour calibration: one or two reference illuminants.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Calibration {
    pub reference1: Reference,
    pub reference2: Option<Reference>,
}

/// Length of [`Calibration::from_flat`]'s input.
pub const FLAT_LEN: usize = 56;

impl Calibration {
    /// A calibration from a flat array (for FFI), [`FLAT_LEN`] values:
    /// illuminant 1, illuminant 2 (0: none), then six row-major 3x3
    /// matrices: color matrix 1 and 2, calibration 1 and 2, forward matrix
    /// 1 and 2. An all-zero calibration is the identity, an all-zero
    /// forward matrix is none, and an illuminant 2 of 0 or an all-zero
    /// color matrix 2 means a single reference. None if the slice is short.
    pub fn from_flat(v: &[f64]) -> Option<Self> {
        if v.len() < FLAT_LEN {
            return None;
        }
        let mat = |i: usize| -> Mat3 {
            let o = 2 + 9 * i;
            [
                [v[o], v[o + 1], v[o + 2]],
                [v[o + 3], v[o + 4], v[o + 5]],
                [v[o + 6], v[o + 7], v[o + 8]],
            ]
        };
        let zero = |m: &Mat3| m.iter().flatten().all(|&x| x == 0.0);
        let reference = |illuminant: f64, k: usize| {
            let calibration = mat(2 + k);
            let forward = mat(4 + k);
            Reference {
                illuminant: if illuminant.is_finite() && illuminant > 0.0 {
                    illuminant as u32
                } else {
                    0
                },
                color_matrix: mat(k),
                calibration: if zero(&calibration) {
                    IDENTITY
                } else {
                    calibration
                },
                forward_matrix: (!zero(&forward)).then_some(forward),
            }
        };
        let reference2 = (v[1] > 0.0 && !zero(&mat(1))).then(|| reference(v[1], 1));
        Some(Self {
            reference1: reference(v[0], 0),
            reference2,
        })
    }
}

/// Camera2 `COLOR_CORRECTION_GAINS`: red, green on red rows, green on blue
/// rows, blue. All ≥ 1 from [`gains_for`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Gains {
    pub r: f64,
    pub gr: f64,
    pub gb: f64,
    pub b: f64,
}

/// A white balance for Camera2's `COLOR_CORRECTION_MODE_TRANSFORM_MATRIX`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WhiteBalance {
    pub gains: Gains,
    /// `COLOR_CORRECTION_TRANSFORM`: white-balanced camera RGB (the gains
    /// applied) to linear sRGB, row-major. Each row sums to 1, so a neutral
    /// stays neutral.
    pub transform: Mat3,
}

/// The correlated colour temperature the DNG SDK gives an EXIF LightSource
/// value (`dng_camera_profile::IlluminantToTemperature`); None for unknown,
/// flash-less "other" and the like.
pub fn illuminant_temperature_k(light_source: u32) -> Option<f64> {
    Some(match light_source {
        // Standard light A, tungsten.
        17 | 3 => 2850.0,
        // ISO studio tungsten.
        24 => 3200.0,
        // D50.
        23 => 5000.0,
        // D55, daylight, fine weather, flash, standard light B.
        20 | 1 | 9 | 4 | 18 => 5500.0,
        // D65, standard light C, cloudy.
        21 | 19 | 10 => 6500.0,
        // D75, shade.
        22 | 11 => 7500.0,
        // Daylight fluorescent (D, 5700-7100 K).
        12 => (5700.0 + 7100.0) * 0.5,
        // Day white fluorescent (N, 4600-5500 K).
        13 => (4600.0 + 5500.0) * 0.5,
        // Cool white fluorescent (W, 3800-4500 K), fluorescent.
        14 | 2 => (3800.0 + 4500.0) * 0.5,
        // White fluorescent (WW, 3250-3800 K).
        15 => (3250.0 + 3800.0) * 0.5,
        // Warm white fluorescent (L, 2600-3250 K).
        16 => (2600.0 + 3250.0) * 0.5,
        _ => return None,
    })
}

/// The camera gains for an illuminant of `temperature_k` and `tint`
/// (clamped to 2000-10000 K and -150 to [`max_tint`]): a neutral under it comes out
/// neutral. None for a NaN input or an unusable calibration.
pub fn gains_for(temperature_k: f64, tint: f64, cal: &Calibration) -> Option<Gains> {
    let (_, _, _, white) = prepare(temperature_k, tint, cal)?;
    Some(gains_of(white))
}

/// The matching `COLOR_CORRECTION_TRANSFORM` (see [`WhiteBalance`]). Without
/// forward matrices, None also where the colour matrix cannot be inverted or
/// the illuminant has too little blue for the Bradford adaptation: near the
/// green tint clamp at low temperatures ([`max_tint`]; with the test
/// calibration from x + y 0.977, 3000 K from +83). The gains are still there.
pub fn color_transform_for(temperature_k: f64, tint: f64, cal: &Calibration) -> Option<Mat3> {
    let (spec, xy, m, white) = prepare(temperature_k, tint, cal)?;
    spec.camera_to_srgb(&m, xy, white)
}

/// The gains and the colour transform for an illuminant (see [`gains_for`]
/// and [`color_transform_for`]).
pub fn white_balance_for(temperature_k: f64, tint: f64, cal: &Calibration) -> Option<WhiteBalance> {
    let (spec, xy, m, white) = prepare(temperature_k, tint, cal)?;
    Some(WhiteBalance {
        gains: gains_of(white),
        transform: spec.camera_to_srgb(&m, xy, white)?,
    })
}

/// The prepared calibration, the illuminant's xy, its matrices and its
/// camera neutral (largest channel 1; channels pinned to 0.001 or more, as
/// the DNG SDK does).
fn prepare(
    temperature_k: f64,
    tint: f64,
    cal: &Calibration,
) -> Option<(Spec, [f64; 2], Matrices, [f64; 3])> {
    if temperature_k.is_nan() || tint.is_nan() {
        return None;
    }
    let (k, t) = clamp(temperature_k, tint);
    let spec = Spec::new(cal)?;
    let xy = xy_for_temperature_tint(k, t);
    let m = spec.find(xy);
    let white = mul_vec(&m.color_matrix, &xy_to_xyz(xy));
    let max = white[0].max(white[1]).max(white[2]);
    if !(max.is_finite() && max > 0.0) {
        return None;
    }
    let white = white.map(|c| (c / max).clamp(0.001, 1.0));
    Some((spec, xy, m, white))
}

/// Gains that make the camera neutral `white` (largest channel 1) neutral:
/// the smallest is 1.
fn gains_of(white: [f64; 3]) -> Gains {
    Gains {
        r: 1.0 / white[0],
        gr: 1.0 / white[1],
        gb: 1.0 / white[1],
        b: 1.0 / white[2],
    }
}

/// The illuminant (temperature K, tint) that the gains neutralise, e.g. the
/// `COLOR_CORRECTION_GAINS` of an auto white balance capture result; the
/// inverse of [`gains_for`]. Clamped as [`gains_for`] clamps. None for
/// non-positive or non-finite gains or an unusable calibration.
pub fn temperature_tint_for(gains: Gains, cal: &Calibration) -> Option<(f64, f64)> {
    let g = (gains.gr + gains.gb) * 0.5;
    let all = [gains.r, g, gains.b];
    if !all.iter().all(|x| x.is_finite() && *x > 0.0) {
        return None;
    }
    let spec = Spec::new(cal)?;
    let neutral = all.map(|x| 1.0 / x);
    let xy = spec.neutral_to_xy(neutral)?;
    let (k, t) = temperature_tint_for_xy(xy);
    if !(k.is_finite() && t.is_finite()) {
        return None;
    }
    Some(clamp(k, t))
}

fn clamp(temperature_k: f64, tint: f64) -> (f64, f64) {
    let k = temperature_k.clamp(MIN_TEMPERATURE_K, MAX_TEMPERATURE_K);
    (k, tint.clamp(-MAX_TINT, max_tint(k)))
}

/// The greenest tint the functions take at `temperature_k` (2000-10000 K):
/// [`MAX_TINT`], or less where the illuminant would reach
/// [`MAX_X_PLUS_Y`] (+21 at 2000 K, +49 at 2500 K, +82 at 3000 K, +118 at
/// 3500 K; from about 3900 K the full +150).
pub fn max_tint(temperature_k: f64) -> f64 {
    // Along the isotherm u and v move linearly with the offset, and
    // x + y = (1.5u + v) / (u - 4v + 2) = L is linear in them too.
    let (u, v, du, dv) = isotherm(temperature_k);
    let l = MAX_X_PLUS_Y;
    let slope = 1.5 * du + dv - l * (du - 4.0 * dv);
    let offset = (l * (u - 4.0 * v + 2.0) - 1.5 * u - v) / slope;
    let tint = offset * TINT_SCALE;
    if tint.is_finite() && tint > 0.0 {
        tint.min(MAX_TINT)
    } else {
        MAX_TINT
    }
}

// ---------------------------------------------------------------------------
// Temperature and tint (dng_temperature.cpp)
// ---------------------------------------------------------------------------

const TINT_SCALE: f64 = -3000.0;

/// Robertson's isotherms: inverse temperature (mired), the Planckian locus
/// in CIE 1960 uv, and the isotherm slope. As in the DNG SDK, including its
/// u of 0.24702 at 325 mired (Wyszecki & Stiles' misprint of 0.24792), so
/// the values match Adobe's.
const TABLE: [(f64, f64, f64, f64); 31] = [
    (0.0, 0.18006, 0.26352, -0.24341),
    (10.0, 0.18066, 0.26589, -0.25479),
    (20.0, 0.18133, 0.26846, -0.26876),
    (30.0, 0.18208, 0.27119, -0.28539),
    (40.0, 0.18293, 0.27407, -0.30470),
    (50.0, 0.18388, 0.27709, -0.32675),
    (60.0, 0.18494, 0.28021, -0.35156),
    (70.0, 0.18611, 0.28342, -0.37915),
    (80.0, 0.18740, 0.28668, -0.40955),
    (90.0, 0.18880, 0.28997, -0.44278),
    (100.0, 0.19032, 0.29326, -0.47888),
    (125.0, 0.19462, 0.30141, -0.58204),
    (150.0, 0.19962, 0.30921, -0.70471),
    (175.0, 0.20525, 0.31647, -0.84901),
    (200.0, 0.21142, 0.32312, -1.0182),
    (225.0, 0.21807, 0.32909, -1.2168),
    (250.0, 0.22511, 0.33439, -1.4512),
    (275.0, 0.23247, 0.33904, -1.7298),
    (300.0, 0.24010, 0.34308, -2.0637),
    (325.0, 0.24702, 0.34655, -2.4681),
    (350.0, 0.25591, 0.34951, -2.9641),
    (375.0, 0.26400, 0.35200, -3.5814),
    (400.0, 0.27218, 0.35407, -4.3633),
    (425.0, 0.28039, 0.35577, -5.3762),
    (450.0, 0.28863, 0.35714, -6.7262),
    (475.0, 0.29685, 0.35823, -8.5955),
    (500.0, 0.30505, 0.35907, -11.324),
    (525.0, 0.31320, 0.35968, -15.628),
    (550.0, 0.32129, 0.36011, -23.325),
    (575.0, 0.32931, 0.36038, -40.770),
    (600.0, 0.33724, 0.36051, -116.45),
];

/// CIE 1931 xy of an illuminant of `temperature_k` (> 0; the table covers
/// 1667 K and up) and `tint` (`dng_temperature::Get_xy_coord`).
pub fn xy_for_temperature_tint(temperature_k: f64, tint: f64) -> [f64; 2] {
    let (u, v, du, dv) = isotherm(temperature_k);
    let offset = tint / TINT_SCALE;
    let u = u + du * offset;
    let v = v + dv * offset;
    let d = u - 4.0 * v + 2.0;
    [1.5 * u / d, v / d]
}

/// The Planckian locus at `temperature_k` in CIE 1960 uv and the unit
/// direction of its isotherm, interpolated from [`TABLE`].
fn isotherm(temperature_k: f64) -> (f64, f64, f64, f64) {
    let r = 1.0e6 / temperature_k;
    let mut i = 0;
    while i < 29 && r >= TABLE[i + 1].0 {
        i += 1;
    }
    let (r0, u0, v0, t0) = TABLE[i];
    let (r1, u1, v1, t1) = TABLE[i + 1];
    let f = (r1 - r) / (r1 - r0);
    let u = u0 * f + u1 * (1.0 - f);
    let v = v0 * f + v1 * (1.0 - f);
    let len0 = (1.0 + t0 * t0).sqrt();
    let len1 = (1.0 + t1 * t1).sqrt();
    let du = f / len0 + (1.0 - f) / len1;
    let dv = t0 / len0 * f + t1 / len1 * (1.0 - f);
    let len = (du * du + dv * dv).sqrt();
    (u, v, du / len, dv / len)
}

/// Temperature (K) and tint of a CIE 1931 xy (`dng_temperature::Set_xy_coord`).
pub fn temperature_tint_for_xy(xy: [f64; 2]) -> (f64, f64) {
    let [x, y] = xy;
    let d = 1.5 - x + 6.0 * y;
    let u = 2.0 * x / d;
    let v = 3.0 * y / d;
    let (mut last_dt, mut last_du, mut last_dv) = (0.0, 0.0, 0.0);
    for i in 1..=30 {
        let (_, ui, vi, ti) = TABLE[i];
        let len = (1.0 + ti * ti).sqrt();
        let (du, dv) = (1.0 / len, ti / len);
        let dt = -(u - ui) * dv + (v - vi) * du;
        if dt <= 0.0 || i == 30 {
            let dt = -dt.min(0.0);
            let f = if i == 1 || last_dt + dt == 0.0 {
                0.0
            } else {
                dt / (last_dt + dt)
            };
            let (r0, u0, v0, _) = TABLE[i - 1];
            let temperature = 1.0e6 / (r0 * f + TABLE[i].0 * (1.0 - f));
            let uu = u - (u0 * f + ui * (1.0 - f));
            let vv = v - (v0 * f + vi * (1.0 - f));
            let du = du * (1.0 - f) + last_du * f;
            let dv = dv * (1.0 - f) + last_dv * f;
            let len = (du * du + dv * dv).sqrt();
            let tint = (uu * du / len + vv * dv / len) * TINT_SCALE;
            return (temperature, tint);
        }
        last_dt = dt;
        last_du = du;
        last_dv = dv;
    }
    unreachable!("the loop returns at i == 30")
}

// ---------------------------------------------------------------------------
// Colour spec (dng_color_spec.cpp)
// ---------------------------------------------------------------------------

/// The DNG SDK's D50 (PCS white) xy.
const D50_XY: [f64; 2] = [0.3457, 0.3585];
const D65_XY: [f64; 2] = [0.3127, 0.3290];

/// CIE XYZ (D65) to linear sRGB (IEC 61966-2-1, Lindbloom's precision).
const XYZ_TO_SRGB: Mat3 = [
    [3.2404542, -1.5371385, -0.4985314],
    [-0.9692660, 1.8760108, 0.0415560],
    [0.0556434, -0.2040259, 1.0572252],
];

/// The interpolated matrices for one illuminant.
struct Matrices {
    /// CameraCalibration * ColorMatrix: XYZ to this unit's camera RGB.
    color_matrix: Mat3,
    forward_matrix: Option<Mat3>,
    calibration: Mat3,
}

/// The calibration prepared as `dng_color_spec`'s constructor does.
struct Spec {
    t1: f64,
    t2: f64,
    cm1: Mat3,
    cm2: Mat3,
    fm1: Option<Mat3>,
    fm2: Option<Mat3>,
    cc1: Mat3,
    cc2: Mat3,
}

impl Spec {
    fn new(cal: &Calibration) -> Option<Self> {
        let finite = |m: &Mat3| m.iter().flatten().all(|x| x.is_finite());
        let usable = |r: &Reference| {
            finite(&r.color_matrix)
                && finite(&r.calibration)
                && r.forward_matrix.as_ref().is_none_or(finite)
        };
        let r1 = &cal.reference1;
        if !usable(r1) {
            return None;
        }
        let fm1 = match r1.forward_matrix {
            Some(m) => Some(normalize_forward(&m)?),
            None => None,
        };
        let cm1 = mul(&r1.calibration, &r1.color_matrix);
        let two = cal.reference2.filter(usable).and_then(|r2| {
            let t1 = illuminant_temperature_k(r1.illuminant)?;
            let t2 = illuminant_temperature_k(r2.illuminant)?;
            (t1 != t2).then_some((t1, t2, r2))
        });
        let Some((t1, t2, r2)) = two else {
            return Some(Self {
                t1: 5000.0,
                t2: 5000.0,
                cm1,
                cm2: cm1,
                fm1,
                fm2: fm1,
                cc1: r1.calibration,
                cc2: r1.calibration,
            });
        };
        let fm2 = match r2.forward_matrix {
            Some(m) => Some(normalize_forward(&m)?),
            None => None,
        };
        let cm2 = mul(&r2.calibration, &r2.color_matrix);
        let (s1, s2) = (
            (t1, cm1, fm1, r1.calibration),
            (t2, cm2, fm2, r2.calibration),
        );
        let (a, b) = if t1 > t2 { (s2, s1) } else { (s1, s2) };
        Some(Self {
            t1: a.0,
            t2: b.0,
            cm1: a.1,
            cm2: b.1,
            fm1: a.2,
            fm2: b.2,
            cc1: a.3,
            cc2: b.3,
        })
    }

    /// `FindXYZtoCamera`: interpolation in inverse temperature.
    fn find(&self, xy: [f64; 2]) -> Matrices {
        let (t, _) = temperature_tint_for_xy(xy);
        let g = if t <= self.t1 {
            1.0
        } else if t >= self.t2 {
            0.0
        } else {
            (1.0 / t - 1.0 / self.t2) / (1.0 / self.t1 - 1.0 / self.t2)
        };
        let g = if g.is_nan() { 1.0 } else { g };
        let lerp = |a: &Mat3, b: &Mat3| -> Mat3 {
            if g >= 1.0 {
                *a
            } else if g <= 0.0 {
                *b
            } else {
                let mut m = [[0.0; 3]; 3];
                for (i, row) in m.iter_mut().enumerate() {
                    for (j, x) in row.iter_mut().enumerate() {
                        *x = g * a[i][j] + (1.0 - g) * b[i][j];
                    }
                }
                m
            }
        };
        Matrices {
            color_matrix: lerp(&self.cm1, &self.cm2),
            forward_matrix: match (&self.fm1, &self.fm2) {
                (Some(a), Some(b)) => Some(lerp(a, b)),
                (a, b) => a.or(*b),
            },
            calibration: lerp(&self.cc1, &self.cc2),
        }
    }

    /// `NeutralToXY`: the illuminant whose camera neutral this is.
    fn neutral_to_xy(&self, neutral: [f64; 3]) -> Option<[f64; 2]> {
        const PASSES: usize = 30;
        let mut last = D50_XY;
        for pass in 0..PASSES {
            let m = self.find(last);
            let xyz = mul_vec(&invert(&m.color_matrix)?, &neutral);
            let mut next = xyz_to_xy(xyz);
            if !(next[0].is_finite() && next[1].is_finite()) {
                return None;
            }
            if (next[0] - last[0]).abs() + (next[1] - last[1]).abs() < 1e-7 {
                return Some(next);
            }
            // No convergence: most likely a two-value oscillation; take
            // the average and give up.
            if pass == PASSES - 1 {
                next = [(last[0] + next[0]) * 0.5, (last[1] + next[1]) * 0.5];
            }
            last = next;
        }
        Some(last)
    }

    /// White-balanced camera RGB (`white` times the gains) to linear sRGB,
    /// rows normalised to sum to 1.
    fn camera_to_srgb(&self, m: &Matrices, xy: [f64; 2], white: [f64; 3]) -> Option<Mat3> {
        // Camera RGB (raw, before the gains) to XYZ D50 (`SetWhiteXY`'s
        // CameraToPCS).
        let to_pcs = match &m.forward_matrix {
            Some(fm) => {
                let to_reference = invert(&m.calibration)?;
                let ref_white = mul_vec(&to_reference, &white);
                if !ref_white.iter().all(|c| c.is_finite() && *c != 0.0) {
                    return None;
                }
                mul(fm, &mul(&diag(ref_white.map(|c| 1.0 / c)), &to_reference))
            }
            None => invert(&mul(&m.color_matrix, &map_white(D50_XY, xy)))?,
        };
        // The gains (1 / white) come first in the camera, so undo them here.
        let t = mul(
            &XYZ_TO_SRGB,
            &mul(&map_white(D50_XY, D65_XY), &mul(&to_pcs, &diag(white))),
        );
        let mut out = [[0.0; 3]; 3];
        for (row, src) in out.iter_mut().zip(t.iter()) {
            let sum: f64 = src.iter().sum();
            if !(sum.is_finite() && sum > 1e-6) {
                return None;
            }
            *row = src.map(|x| x / sum);
        }
        Some(out)
    }
}

/// `NormalizeForwardMatrix`: scale the rows so camera (1, 1, 1) maps to
/// the D50 white.
fn normalize_forward(m: &Mat3) -> Option<Mat3> {
    let xyz = mul_vec(m, &[1.0, 1.0, 1.0]);
    let pcs = xy_to_xyz(D50_XY);
    let mut out = *m;
    for i in 0..3 {
        if !(xyz[i].is_finite() && xyz[i].abs() > 1e-9) {
            return None;
        }
        out[i] = m[i].map(|x| x * pcs[i] / xyz[i]);
    }
    Some(out)
}

/// `MapWhiteMatrix`: linearised Bradford adaptation from `white1` to
/// `white2`, each cone scale limited to 0.1-10.
fn map_white(white1: [f64; 2], white2: [f64; 2]) -> Mat3 {
    const MB: Mat3 = [
        [0.8951, 0.2664, -0.1614],
        [-0.7502, 1.7135, 0.0367],
        [0.0389, -0.0685, 1.0296],
    ];
    let w1 = mul_vec(&MB, &xy_to_xyz(white1)).map(|c| c.max(0.0));
    let w2 = mul_vec(&MB, &xy_to_xyz(white2)).map(|c| c.max(0.0));
    let scale = |i: usize| {
        if w1[i] > 0.0 {
            (w2[i] / w1[i]).clamp(0.1, 10.0)
        } else {
            10.0
        }
    };
    let a = diag([scale(0), scale(1), scale(2)]);
    // MB is invertible (a constant).
    mul(&invert(&MB).unwrap_or(IDENTITY), &mul(&a, &MB))
}

/// `XYtoXYZ`, Y = 1, with the DNG SDK's pinning.
fn xy_to_xyz(xy: [f64; 2]) -> [f64; 3] {
    let mut x = xy[0].clamp(0.000001, 0.999999);
    let mut y = xy[1].clamp(0.000001, 0.999999);
    if x + y > 0.999999 {
        let scale = 0.999999 / (x + y);
        x *= scale;
        y *= scale;
    }
    [x / y, 1.0, (1.0 - x - y) / y]
}

/// `XYZtoXY`: D50 for a non-positive total.
fn xyz_to_xy(xyz: [f64; 3]) -> [f64; 2] {
    let total = xyz[0] + xyz[1] + xyz[2];
    if total > 0.0 {
        [xyz[0] / total, xyz[1] / total]
    } else {
        D50_XY
    }
}

// ---------------------------------------------------------------------------
// 3x3 helpers
// ---------------------------------------------------------------------------

fn mul(a: &Mat3, b: &Mat3) -> Mat3 {
    let mut m = [[0.0; 3]; 3];
    for (i, row) in m.iter_mut().enumerate() {
        for (j, x) in row.iter_mut().enumerate() {
            *x = (0..3).map(|k| a[i][k] * b[k][j]).sum();
        }
    }
    m
}

fn mul_vec(a: &Mat3, v: &[f64; 3]) -> [f64; 3] {
    [0, 1, 2].map(|i| a[i][0] * v[0] + a[i][1] * v[1] + a[i][2] * v[2])
}

fn diag(v: [f64; 3]) -> Mat3 {
    [[v[0], 0.0, 0.0], [0.0, v[1], 0.0], [0.0, 0.0, v[2]]]
}

/// The inverse; None if singular (relative to the entries' size) or not
/// finite.
fn invert(m: &Mat3) -> Option<Mat3> {
    let c = |i: usize, j: usize| {
        let (i1, i2) = ((i + 1) % 3, (i + 2) % 3);
        let (j1, j2) = ((j + 1) % 3, (j + 2) % 3);
        m[i1][j1] * m[i2][j2] - m[i1][j2] * m[i2][j1]
    };
    let det = m[0][0] * c(0, 0) + m[0][1] * c(0, 1) + m[0][2] * c(0, 2);
    let scale = m.iter().flatten().fold(0.0f64, |a, x| a.max(x.abs()));
    if !(det.is_finite() && scale.is_finite()) || det.abs() <= 1e-10 * scale.powi(3) {
        return None;
    }
    let mut inv = [[0.0; 3]; 3];
    for (i, row) in inv.iter_mut().enumerate() {
        for (j, x) in row.iter_mut().enumerate() {
            // The adjugate is the transposed cofactor matrix.
            *x = c(j, i) / det;
        }
    }
    Some(inv)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Google Pixel 7 Pro main camera, from a DNG its Camera2 stack wrote
    /// (Android DngCreator copies SENSOR_COLOR_TRANSFORMn and
    /// SENSOR_FORWARD_MATRIXn): Standard light A and D65, AsShotNeutral
    /// 0.599579 1 0.543078. Test fixture of github.com/d0ct0rvenkman/mediafiler
    /// (test/main/generateFilenameBase/real/google/pixel7pro.json).
    fn pixel_7_pro() -> Calibration {
        Calibration {
            reference1: Reference {
                illuminant: 17,
                color_matrix: [
                    [1.1934, -0.7069, 0.0784],
                    [-0.2402, 1.0495, 0.2208],
                    [0.0257, 0.1325, 0.6489],
                ],
                calibration: IDENTITY,
                forward_matrix: Some([
                    [0.3517, 0.5102, 0.1024],
                    [0.1594, 0.8122, 0.0284],
                    [0.0422, 0.003, 0.78],
                ]),
            },
            reference2: Some(Reference {
                illuminant: 21,
                color_matrix: [
                    [1.0137, -0.4345, -0.0865],
                    [-0.3544, 1.2039, 0.1671],
                    [-0.031, 0.2024, 0.5504],
                ],
                calibration: IDENTITY,
                forward_matrix: Some([
                    [0.3841, 0.4437, 0.1366],
                    [0.1838, 0.7767, 0.0395],
                    [0.0631, 0.0029, 0.7591],
                ]),
            }),
        }
    }

    /// The same without forward matrices, and with a unit calibration.
    fn no_forward() -> Calibration {
        let mut cal = pixel_7_pro();
        let cc = [[1.02, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 0.97]];
        cal.reference1.forward_matrix = None;
        cal.reference1.calibration = cc;
        let mut r2 = cal.reference2.unwrap();
        r2.forward_matrix = None;
        r2.calibration = cc;
        cal.reference2 = Some(r2);
        cal
    }

    fn single() -> Calibration {
        let mut cal = pixel_7_pro();
        cal.reference2 = None;
        cal
    }

    fn sets() -> [Calibration; 3] {
        [pixel_7_pro(), no_forward(), single()]
    }

    #[test]
    fn planckian_points() {
        // On the locus: 6504 K is (0.3135, 0.3236), illuminant A (2856 K)
        // is (0.4476, 0.4074).
        let [x, y] = xy_for_temperature_tint(6504.0, 0.0);
        assert!(
            (x - 0.3135).abs() < 0.001 && (y - 0.3236).abs() < 0.001,
            "{x} {y}"
        );
        let [x, y] = xy_for_temperature_tint(2856.0, 0.0);
        assert!(
            (x - 0.4476).abs() < 0.001 && (y - 0.4074).abs() < 0.001,
            "{x} {y}"
        );
        // D65 lies above the locus (Duv +0.0032): 6504 K, tint about +10.
        let (k, tint) = temperature_tint_for_xy([0.3127, 0.3290]);
        assert!((k - 6504.0).abs() < 50.0, "{k}");
        assert!((8.0..12.0).contains(&tint), "{tint}");
        let [x, y] = xy_for_temperature_tint(6504.0, tint);
        assert!(
            (x - 0.3127).abs() < 0.003 && (y - 0.3290).abs() < 0.003,
            "{x} {y}"
        );
        // The D65 xy itself is not within 0.003 of the locus point at 0 tint.
        let [_, y] = xy_for_temperature_tint(6504.0, 0.0);
        assert!((y - 0.3290).abs() > 0.003);
    }

    #[test]
    fn xy_round_trip() {
        for k in [2000.0, 2856.0, 4000.0, 5000.0, 6504.0, 8000.0, 10000.0] {
            for tint in [-150.0, -50.0, 0.0, 10.0, 50.0, 150.0] {
                // Exact on the locus; the isotherms are interpolated, so far
                // off it the two directions differ slightly (as in the SDK).
                let (k2, t2) = temperature_tint_for_xy(xy_for_temperature_tint(k, tint));
                assert!((k2 - k).abs() / k < 1e-5, "{k} {tint}: {k2}");
                assert!((t2 - tint).abs() < 1e-3, "{k} {tint}: {t2}");
            }
        }
    }

    #[test]
    fn positive_tint_is_above_the_locus() {
        // Greener light (larger CIE 1960 v at the same u side of the
        // isotherm) for positive tint.
        let v = |xy: [f64; 2]| 3.0 * xy[1] / (1.5 - xy[0] + 6.0 * xy[1]);
        for k in [3000.0, 5500.0, 9000.0] {
            let v0 = v(xy_for_temperature_tint(k, 0.0));
            assert!(v(xy_for_temperature_tint(k, 50.0)) > v0);
            assert!(v(xy_for_temperature_tint(k, -50.0)) < v0);
        }
    }

    #[test]
    fn gains_round_trip() {
        for cal in sets() {
            let mut k = 2500.0;
            while k <= 10000.0 {
                for tint in [-50.0, -25.0, 0.0, 10.0, 25.0, 50.0] {
                    let g = gains_for(k, tint, &cal).unwrap();
                    let (k2, t2) = temperature_tint_for(g, &cal).unwrap();
                    assert!((k2 - k).abs() / k < 0.01, "{k} {tint}: {k2}");
                    assert!((t2 - tint).abs() < 1.0, "{k} {tint}: {t2}");
                }
                k += 250.0;
            }
        }
    }

    #[test]
    fn gains_round_trip_over_the_whole_range() {
        // Every temperature and tint the apps can set, green tints at the
        // per-temperature clamp included, comes back as it went in (after
        // the clamp).
        for cal in sets() {
            let mut k = MIN_TEMPERATURE_K;
            while k <= MAX_TEMPERATURE_K {
                let mut tints = [-150.0, -100.0, -50.0, 0.0, 50.0, 100.0, 150.0, max_tint(k)];
                tints.sort_by(f64::total_cmp);
                for tint in tints {
                    let (k0, t0) = clamp(k, tint);
                    let g = gains_for(k, tint, &cal).unwrap();
                    let (k2, t2) = temperature_tint_for(g, &cal).unwrap();
                    assert!((k2 - k0).abs() / k0 < 0.002, "{k} {tint}: {k2}");
                    assert!((t2 - t0).abs() < 0.5, "{k} {tint}: {t2}");
                }
                k += 100.0;
            }
        }
    }

    #[test]
    fn green_tints_are_clamped_off_the_yellow_edge() {
        let mut k = MIN_TEMPERATURE_K;
        while k <= MAX_TEMPERATURE_K {
            let t = max_tint(k);
            assert!(t > 0.0 && t <= MAX_TINT, "{k}: {t}");
            let [x, y] = xy_for_temperature_tint(k, clamp(k, 400.0).1);
            assert!(x + y <= MAX_X_PLUS_Y + 1e-9, "{k}: x + y {}", x + y);
            k += 50.0;
        }
        assert!((max_tint(2000.0) - 21.5).abs() < 0.5);
        assert_eq!(max_tint(5000.0), MAX_TINT);
        let cal = pixel_7_pro();
        assert_eq!(
            gains_for(2000.0, 150.0, &cal),
            gains_for(2000.0, max_tint(2000.0), &cal)
        );
        // Magenta is not limited.
        assert_eq!(clamp(2000.0, -150.0), (2000.0, -150.0));
    }

    #[test]
    fn gains_are_at_least_one() {
        for cal in sets() {
            for k in [2000.0, 3000.0, 5000.0, 8000.0, 10000.0] {
                for tint in [-150.0, 0.0, 150.0] {
                    let g = gains_for(k, tint, &cal).unwrap();
                    let all = [g.r, g.gr, g.gb, g.b];
                    assert!(all.iter().all(|x| *x >= 1.0), "{k} {tint}: {g:?}");
                    assert!(all.contains(&1.0));
                    assert_eq!(g.gr, g.gb);
                }
            }
        }
    }

    #[test]
    fn lower_temperature_more_blue_gain() {
        for cal in sets() {
            let mut last = 0.0;
            let mut k = 10000.0;
            while k >= 2000.0 {
                let g = gains_for(k, 0.0, &cal).unwrap();
                let ratio = g.b / g.r;
                assert!(ratio > last, "{k}: {ratio} after {last}");
                last = ratio;
                k -= 250.0;
            }
            // Tungsten needs much more blue than red; daylight about even.
            let g = gains_for(2856.0, 0.0, &cal).unwrap();
            assert!(g.b > 1.5 * g.r, "{g:?}");
        }
    }

    #[test]
    fn positive_tint_takes_green_out() {
        for cal in sets() {
            let green = |tint: f64| {
                let g = gains_for(5000.0, tint, &cal).unwrap();
                g.gr / (g.r * g.b).sqrt()
            };
            assert!(green(50.0) < green(0.0));
            assert!(green(0.0) < green(-50.0));
        }
    }

    #[test]
    fn as_shot_neutral_of_a_real_dng() {
        // The Pixel's own auto white balance: a neutral of 0.6 1 0.54
        // (comes out at about 3675 K, tint +1: an indoor shot).
        let cal = pixel_7_pro();
        let neutral = [0.599579, 1.0, 0.543078];
        let gains = Gains {
            r: 1.0 / neutral[0],
            gr: 1.0,
            gb: 1.0,
            b: 1.0 / neutral[2],
        };
        let (k, tint) = temperature_tint_for(gains, &cal).unwrap();
        assert!((3000.0..7000.0).contains(&k), "{k} {tint}");
        assert!(tint.abs() < 30.0, "{k} {tint}");
        let back = gains_for(k, tint, &cal).unwrap();
        assert!((back.r - gains.r).abs() < 1e-4 && (back.b - gains.b).abs() < 1e-4);
    }

    #[test]
    fn transform_keeps_neutrals() {
        for cal in sets() {
            for k in [2500.0, 5000.0, 6504.0, 10000.0] {
                let wb = white_balance_for(k, 10.0, &cal).unwrap();
                let out = mul_vec(&wb.transform, &[1.0, 1.0, 1.0]);
                for c in out {
                    assert!((c - 1.0).abs() < 1e-12, "{k}: {out:?}");
                }
                // A plausible colour matrix: a positive diagonal. (The
                // Pixel's forward matrices are close to sRGB's, so its
                // diagonal is near 1.)
                for i in 0..3 {
                    assert!(wb.transform[i][i] > 0.5, "{k}: {:?}", wb.transform);
                }
            }
        }
        // Over the whole range: with forward matrices always a
        // neutral-keeping transform. Without, none only where a green tint
        // at a low temperature puts the illuminant near the yellow edge of
        // the chromaticity diagram (x + y about 1: no blue, a negative
        // Bradford cone); the gains are still there.
        for cal in sets() {
            let mut k = 2000.0;
            while k <= 10000.0 {
                let mut tint = -150.0;
                while tint <= 150.0 {
                    match color_transform_for(k, tint, &cal) {
                        Some(t) => {
                            let out = mul_vec(&t, &[1.0, 1.0, 1.0]);
                            assert!(out.iter().all(|c| (c - 1.0).abs() < 1e-12));
                        }
                        None => {
                            assert!(cal.reference1.forward_matrix.is_none());
                            let (k0, t0) = clamp(k, tint);
                            let [x, y] = xy_for_temperature_tint(k0, t0);
                            assert!(x + y > 0.975, "{k} {tint}");
                            assert!(gains_for(k, tint, &cal).is_some());
                        }
                    }
                    tint += 10.0;
                }
                k += 250.0;
            }
        }
    }

    #[test]
    fn forward_and_inverse_routes_agree_for_a_consistent_profile() {
        // Forward matrices derived from the colour matrix at its own
        // reference illuminant (what a profile built from one measurement
        // has): both routes give the same transform there.
        let mut cal = no_forward();
        cal.reference2 = None;
        cal.reference1.illuminant = 21;
        let xy = xy_for_temperature_tint(6504.0, 9.7);
        let r = &cal.reference1;
        let cm = mul(&r.calibration, &r.color_matrix);
        let white = mul_vec(&cm, &xy_to_xyz(xy));
        let max = white[0].max(white[1]).max(white[2]);
        let reference_white = mul_vec(&invert(&r.calibration).unwrap(), &white.map(|c| c / max));
        let fm = mul(
            &map_white(xy, D50_XY),
            &mul(&invert(&r.color_matrix).unwrap(), &diag(reference_white)),
        );
        let mut with_fm = cal;
        with_fm.reference1.forward_matrix = Some(fm);
        let a = color_transform_for(6504.0, 9.7, &with_fm).unwrap();
        let b = color_transform_for(6504.0, 9.7, &cal).unwrap();
        for i in 0..3 {
            for j in 0..3 {
                assert!((a[i][j] - b[i][j]).abs() < 1e-9, "{a:?}\n{b:?}");
            }
        }
        // A typical sensor: saturation is added (diagonal above 1).
        for i in 0..3 {
            assert!(b[i][i] > 1.0, "{b:?}");
        }
    }

    #[test]
    fn clamps_out_of_range() {
        let cal = pixel_7_pro();
        assert_eq!(gains_for(100.0, 0.0, &cal), gains_for(2000.0, 0.0, &cal));
        assert_eq!(gains_for(1.0e6, 0.0, &cal), gains_for(10000.0, 0.0, &cal));
        assert_eq!(
            gains_for(f64::INFINITY, 0.0, &cal),
            gains_for(10000.0, 0.0, &cal)
        );
        assert_eq!(
            gains_for(5000.0, 400.0, &cal),
            gains_for(5000.0, 150.0, &cal)
        );
        assert_eq!(
            gains_for(5000.0, -400.0, &cal),
            gains_for(5000.0, -150.0, &cal)
        );
        // Extreme gains come back clamped.
        let (k, tint) = temperature_tint_for(
            Gains {
                r: 1.0,
                gr: 40.0,
                gb: 40.0,
                b: 1.0,
            },
            &cal,
        )
        .unwrap();
        assert!((MIN_TEMPERATURE_K..=MAX_TEMPERATURE_K).contains(&k));
        assert!(tint.abs() <= MAX_TINT);
    }

    #[test]
    fn rejects_bad_input() {
        let cal = pixel_7_pro();
        assert!(gains_for(f64::NAN, 0.0, &cal).is_none());
        assert!(gains_for(5000.0, f64::NAN, &cal).is_none());
        let ok = Gains {
            r: 2.0,
            gr: 1.0,
            gb: 1.0,
            b: 1.5,
        };
        for bad in [0.0, -1.0, f64::NAN, f64::INFINITY] {
            assert!(temperature_tint_for(Gains { r: bad, ..ok }, &cal).is_none());
            assert!(temperature_tint_for(Gains { b: bad, ..ok }, &cal).is_none());
        }
        // A singular colour matrix: no inverse, no forward matrix to fall
        // back on.
        let mut singular = no_forward();
        singular.reference1.color_matrix = [[1.0, 2.0, 3.0], [2.0, 4.0, 6.0], [0.0, 1.0, 1.0]];
        singular.reference2 = None;
        assert!(temperature_tint_for(ok, &singular).is_none());
        assert!(color_transform_for(5000.0, 0.0, &singular).is_none());
        // NaN in the calibration.
        let mut nan = pixel_7_pro();
        nan.reference1.color_matrix[1][1] = f64::NAN;
        assert!(gains_for(5000.0, 0.0, &nan).is_none());
        assert!(temperature_tint_for(ok, &nan).is_none());
        // An all-zero colour matrix.
        let mut zero = single();
        zero.reference1.color_matrix = [[0.0; 3]; 3];
        assert!(gains_for(5000.0, 0.0, &zero).is_none());
        // A singular calibration with forward matrices.
        let mut cc = single();
        cc.reference1.calibration = [[0.0; 3]; 3];
        assert!(color_transform_for(5000.0, 0.0, &cc).is_none());
    }

    #[test]
    fn unknown_second_illuminant_uses_the_first_matrix() {
        let mut cal = pixel_7_pro();
        cal.reference2.as_mut().unwrap().illuminant = 0;
        assert_eq!(
            gains_for(4000.0, 0.0, &cal),
            gains_for(4000.0, 0.0, &single())
        );
        // Reversed order interpolates the same.
        let mut swapped = pixel_7_pro();
        let r2 = swapped.reference2.unwrap();
        swapped.reference2 = Some(swapped.reference1);
        swapped.reference1 = r2;
        let (a, b) = (
            gains_for(4000.0, 5.0, &pixel_7_pro()).unwrap(),
            gains_for(4000.0, 5.0, &swapped).unwrap(),
        );
        assert!((a.r - b.r).abs() < 1e-12 && (a.b - b.b).abs() < 1e-12);
    }

    #[test]
    fn illuminants() {
        assert_eq!(illuminant_temperature_k(17), Some(2850.0));
        assert_eq!(illuminant_temperature_k(21), Some(6500.0));
        assert_eq!(illuminant_temperature_k(23), Some(5000.0));
        assert_eq!(illuminant_temperature_k(20), Some(5500.0));
        assert_eq!(illuminant_temperature_k(22), Some(7500.0));
        assert_eq!(illuminant_temperature_k(24), Some(3200.0));
        assert_eq!(illuminant_temperature_k(14), Some(4150.0));
        assert_eq!(illuminant_temperature_k(0), None);
        assert_eq!(illuminant_temperature_k(255), None);
    }

    #[test]
    fn flat_layout() {
        let cal = pixel_7_pro();
        let mut v = vec![17.0, 21.0];
        let r1 = cal.reference1;
        let r2 = cal.reference2.unwrap();
        for m in [
            r1.color_matrix,
            r2.color_matrix,
            [[0.0; 3]; 3],
            IDENTITY,
            r1.forward_matrix.unwrap(),
            r2.forward_matrix.unwrap(),
        ] {
            v.extend(m.iter().flatten());
        }
        assert_eq!(v.len(), FLAT_LEN);
        assert_eq!(Calibration::from_flat(&v), Some(cal));
        // No second illuminant, no forward matrices.
        v[1] = 0.0;
        v[38..].fill(0.0);
        let parsed = Calibration::from_flat(&v).unwrap();
        assert!(parsed.reference2.is_none());
        assert!(parsed.reference1.forward_matrix.is_none());
        assert!(Calibration::from_flat(&v[..55]).is_none());
    }
}
