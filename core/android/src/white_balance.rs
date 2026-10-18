//! JNI wrappers of the core's manual white balance
//! (`tallycam_core::white_balance`) for `se.eyevinn.tallycam.core.Hot`:
//! Camera2 `COLOR_CORRECTION_GAINS` and `COLOR_CORRECTION_TRANSFORM` for a
//! temperature and tint, and back from the gains of an auto capture result.
//!
//! Called per capture result in auto, so they take and fill `double[]`s and
//! allocate nothing. Unlike the other wrappers these call the core directly:
//! white balance has no C ABI in `tallycam-core-ffi`, as iOS does it in
//! AVFoundation.
//!
//! The calibration is [`FLAT_LEN`] (56) doubles, from `CameraCharacteristics`
//! (rationals as doubles; matrices row-major, the order of
//! `ColorSpaceTransform.copyElements`):
//!
//! | index  | value                                                 |
//! |--------|-------------------------------------------------------|
//! | 0      | `SENSOR_REFERENCE_ILLUMINANT1`                        |
//! | 1      | `SENSOR_REFERENCE_ILLUMINANT2` (0: none)              |
//! | 2-10   | `SENSOR_COLOR_TRANSFORM1`                             |
//! | 11-19  | `SENSOR_COLOR_TRANSFORM2` (zeros: none)               |
//! | 20-28  | `SENSOR_CALIBRATION_TRANSFORM1` (zeros: identity)     |
//! | 29-37  | `SENSOR_CALIBRATION_TRANSFORM2` (zeros: identity)     |
//! | 38-46  | `SENSOR_FORWARD_MATRIX1` (zeros: none)                |
//! | 47-55  | `SENSOR_FORWARD_MATRIX2` (zeros: none)                |

use jni::objects::{JClass, JDoubleArray};
use jni::sys::{jboolean, jdouble, jint, JNI_FALSE, JNI_TRUE};
use jni::JNIEnv;
use tallycam_core::white_balance::{self, Calibration, Gains, FLAT_LEN};

fn calibration(env: &JNIEnv, cal: &JDoubleArray) -> Option<Calibration> {
    let mut v = [0.0f64; FLAT_LEN];
    env.get_double_array_region(cal, 0, &mut v).ok()?;
    Calibration::from_flat(&v)
}

/// `cal` (length 56, above). Fills `out` (length 13): the gains R, Gr, Gb,
/// B (all ≥ 1), then the colour transform, row-major (rows sum to 1).
/// Returns how many values it wrote: 13, 4 (gains only: no transform for
/// this calibration and illuminant) or 0 (none: a bad calibration or NaN).
/// Temperature clamps to 2000-15000 K, tint to ±150.
#[no_mangle]
pub extern "system" fn Java_se_eyevinn_tallycam_core_Hot_whiteBalanceFor(
    env: JNIEnv,
    _class: JClass,
    cal: JDoubleArray,
    temperature_k: jdouble,
    tint: jdouble,
    out: JDoubleArray,
) -> jint {
    let Some(cal) = calibration(&env, &cal) else {
        return 0;
    };
    let Some(g) = white_balance::gains_for(temperature_k, tint, &cal) else {
        return 0;
    };
    let mut v = [0.0f64; 13];
    v[..4].copy_from_slice(&[g.r, g.gr, g.gb, g.b]);
    let n = match white_balance::color_transform_for(temperature_k, tint, &cal) {
        Some(t) => {
            for (dst, src) in v[4..].iter_mut().zip(t.iter().flatten()) {
                *dst = *src;
            }
            13
        }
        None => 4,
    };
    if env.set_double_array_region(&out, 0, &v[..n]).is_err() {
        return 0;
    }
    n as jint
}

/// `cal` (length 56, above); `gains` (length 4): R, Gr, Gb, B, e.g. an
/// auto capture result's `COLOR_CORRECTION_GAINS`. On true, `out`
/// (length 2) holds the temperature (K, 2000-15000) and tint (±150) they
/// neutralise.
#[no_mangle]
pub extern "system" fn Java_se_eyevinn_tallycam_core_Hot_whiteBalanceTemperatureTint(
    env: JNIEnv,
    _class: JClass,
    cal: JDoubleArray,
    gains: JDoubleArray,
    out: JDoubleArray,
) -> jboolean {
    let Some(cal) = calibration(&env, &cal) else {
        return JNI_FALSE;
    };
    let mut g = [0.0f64; 4];
    if env.get_double_array_region(&gains, 0, &mut g).is_err() {
        return JNI_FALSE;
    }
    let gains = Gains {
        r: g[0],
        gr: g[1],
        gb: g[2],
        b: g[3],
    };
    let Some((k, tint)) = white_balance::temperature_tint_for(gains, &cal) else {
        return JNI_FALSE;
    };
    if env.set_double_array_region(&out, 0, &[k, tint]).is_err() {
        return JNI_FALSE;
    }
    JNI_TRUE
}
