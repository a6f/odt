//! Type implementing a rational number which can store any u64 exactly.

use core::cmp::Ordering;
use core::fmt::Display;
use core::ops::{Add, Div, Mul, Neg, Rem, Sub};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Fraction {
    n: u64,
    d: i64,
}

const fn gcd(mut a: u64, mut b: u64) -> u64 {
    if a == 0 || b == 0 {
        return a | b;
    }
    let twos = (a | b).trailing_zeros();
    a >>= a.trailing_zeros();
    while b != 0 {
        b >>= b.trailing_zeros();
        if a > b {
            (a, b) = (b, a);
        }
        b -= a;
    }
    a << twos
}

const fn gcd_u128(mut a: u128, mut b: u128) -> u128 {
    if a == 0 || b == 0 {
        return a | b;
    }
    let twos = (a | b).trailing_zeros();
    a >>= a.trailing_zeros();
    while b != 0 {
        b >>= b.trailing_zeros();
        if a > b {
            (a, b) = (b, a);
        }
        b -= a;
    }
    a << twos
}

impl Fraction {
    pub const ZERO: Self = Self { n: 0, d: 1 };
    pub const ONE: Self = Self { n: 1, d: 1 };

    const fn reduce(n: u64, d: i64) -> Self {
        assert!(d != 0);
        if n == 0 {
            return Self::ZERO; // discard sign
        }
        let g = gcd(n, d.unsigned_abs());
        let n = n / g;
        let d = d / g as i64;
        assert!(d != i64::MIN); // forbid -2^63 because we can't negate it
        Self { n, d }
    }

    const fn reduce_i128(n: i128, d: i128) -> Option<Self> {
        if n < 0 {
            Self::reduce_u128(n.unsigned_abs(), -d)
        } else {
            Self::reduce_u128(n.unsigned_abs(), d)
        }
    }

    const fn reduce_u128(n: u128, d: i128) -> Option<Self> {
        if d == 0 {
            return None;
        }
        if n == 0 {
            return Some(Self::ZERO); // discard sign
        }
        // Simplify fraction without precision loss.
        let g = gcd_u128(n, d.unsigned_abs());
        let n = n / g;
        let d = d / g as i128;
        // Truncate to u64 / i64.
        let xn = 128 - n.leading_zeros() as i32 - 64;
        let xd = 128 - d.abs().leading_zeros() as i32 - 63;
        let shift = if xn > xd { xn } else { xd };
        if shift <= 0 {
            // Results are exact.
            let n = n as u64;
            let d = d as i64;
            return Some(Self { n, d });
        }
        let d = d / (1 << shift);
        if d == 0 {
            // The denominator shrank to zero, meaning the result overflowed.
            return None;
        }
        let n = n / (1 << shift);
        // We lost some precision, but the result is still representable.
        // It may now be denormalized, so simplify again.
        Some(Self::reduce(n as u64, d as i64))
    }

    pub const fn into_inner(self) -> (u64, i64) {
        (self.n, self.d)
    }

    pub const fn from_bool(value: bool) -> Self {
        Self::from_u64(value as u64)
    }

    pub const fn from_u64(value: u64) -> Self {
        Self { n: value, d: 1 }
    }

    pub const fn from_f64(value: f64) -> Self {
        let bits = value.to_bits() as i64;
        let sign = bits >> 63 | 1;
        let exponent = ((bits >> 52) & 0x7ff) as i32 - 1023;
        // This is wrong for subnormals, but all bits will be shifted away in that case.
        let mantissa = bits as u64 & (1 << 52) - 1 | (1 << 52);
        match exponent {
            64.. => Self::reduce(u64::MAX, sign),
            52..64 => Self::reduce(mantissa << (exponent - 52), sign),
            -10..52 => Self::reduce(mantissa, sign << (52 - exponent)),
            // Split at -10 to avoid shifting the denominator into i64's sign bit.
            ..-10 => Self::reduce(mantissa.unbounded_shr((-10 - exponent) as u32), sign << 62),
        }
    }

    pub const fn to_f32(self) -> f32 {
        self.n as f32 / self.d as f32
    }

    pub const fn to_f64(self) -> f64 {
        self.n as f64 / self.d as f64
    }

    pub const fn to_bool(self) -> bool {
        self.n != 0
    }

    // TODO:  Remove this when phandles are gone.
    pub const fn to_u32(self) -> u32 {
        self.to_u64() as u32
    }

    /// Truncates toward zero.
    pub const fn to_u64(self) -> u64 {
        match self.d {
            0.. => self.n / self.d as u64,
            ..0 => (self.n / self.d.unsigned_abs()).wrapping_neg(),
        }
    }

    pub const fn neg(self) -> Self {
        if self.n == 0 {
            self
        } else {
            Self {
                n: self.n,
                d: -self.d,
            }
        }
    }

    pub const fn checked_add(self, rhs: Self) -> Option<Self> {
        Self::reduce_i128(
            self.n as i128 * rhs.d as i128 + rhs.n as i128 * self.d as i128,
            self.d as i128 * rhs.d as i128,
        )
    }

    pub const fn checked_sub(self, rhs: Self) -> Option<Self> {
        Self::reduce_i128(
            self.n as i128 * rhs.d as i128 - rhs.n as i128 * self.d as i128,
            self.d as i128 * rhs.d as i128,
        )
    }

    pub const fn checked_mul(self, rhs: Self) -> Option<Self> {
        Self::reduce_u128(
            self.n as u128 * rhs.n as u128,
            self.d as i128 * rhs.d as i128,
        )
    }

    pub const fn checked_div(self, rhs: Self) -> Option<Self> {
        Self::reduce_i128(
            self.n as i128 * rhs.d as i128,
            self.d as i128 * rhs.n as i128,
        )
    }

    pub const fn checked_rem(self, rhs: Self) -> Option<Self> {
        // TODO:  would be nice to use rem_euclid, but sign bits are scattered over all the terms
        match (self.n as i128 * rhs.d as i128).checked_rem(rhs.n as i128 * self.d as i128) {
            None => None,
            Some(n) => Self::reduce_i128(n, self.d as i128 * rhs.d as i128),
        }
    }
}

impl Neg for Fraction {
    type Output = Self;
    fn neg(self) -> Self::Output {
        Fraction::neg(self)
    }
}

impl Add for Fraction {
    type Output = Self;
    fn add(self, rhs: Self) -> Self::Output {
        self.checked_add(rhs).unwrap()
    }
}

impl Sub for Fraction {
    type Output = Self;
    fn sub(self, rhs: Self) -> Self::Output {
        self.checked_sub(rhs).unwrap()
    }
}

impl Mul for Fraction {
    type Output = Self;
    fn mul(self, rhs: Self) -> Self::Output {
        self.checked_mul(rhs).unwrap()
    }
}

impl Div for Fraction {
    type Output = Self;
    fn div(self, rhs: Self) -> Self::Output {
        self.checked_div(rhs).unwrap()
    }
}

impl Rem for Fraction {
    type Output = Self;
    fn rem(self, rhs: Self) -> Self::Output {
        self.checked_rem(rhs).unwrap()
    }
}

impl Ord for Fraction {
    fn cmp(&self, other: &Self) -> Ordering {
        match (self.d > 0).cmp(&(other.d > 0)) {
            Ordering::Equal => (),
            r => return r,
        }
        let a = self.n as i128 * other.d as i128;
        let b = other.n as i128 * self.d as i128;
        a.cmp(&b)
    }
}

impl PartialOrd for Fraction {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

// TODO:  Remove this when phandles are gone.
impl From<u32> for Fraction {
    fn from(value: u32) -> Self {
        Self::from_u64(value.into())
    }
}

impl From<u64> for Fraction {
    fn from(value: u64) -> Self {
        Self::from_u64(value)
    }
}

impl From<f64> for Fraction {
    fn from(value: f64) -> Self {
        Self::from_f64(value)
    }
}

impl Display for Fraction {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self.d {
            1 => write!(f, "{}", self.n),
            -1 => write!(f, "(-{})", self.n),
            0.. => write!(f, "({}/{})", self.n, self.d),
            ..0 => write!(f, "(-{}/{})", self.n, -self.d),
        }
    }
}

#[test]
fn test_from_f64() {
    for base in [1.0f64, 1.1, 2.0, 3.0] {
        for exp in -30..41 {
            for sign in [-1.0, 1.0] {
                let f = sign * base.powi(exp);
                let r = Fraction::from_f64(f);
                let f2 = r.to_f64();
                let err = (f / f2).max(f2 / f) - 1.0;
                assert!(err < 0.0001, "{f} -> {r} -> {f2}, relative error of {err}");
            }
        }
    }

    assert_eq!(Fraction::from_f64(1e-20), Fraction::ZERO);
    assert_eq!(Fraction::from_f64(1e-300), Fraction::ZERO);
    assert_eq!(Fraction::from_f64(1e-324), Fraction::ZERO);
}

#[test]
fn test_arithmetic() {
    for ea in 0..64 {
        for eb in 0..64 {
            for da in [-1, 0, 1] {
                for db in [-1, 0, 1] {
                    let ia = 1u64.unbounded_shl(ea).saturating_add_signed(da);
                    let ib = 1u64.unbounded_shl(eb).saturating_add_signed(db);
                    let a = Fraction::from_u64(ia);
                    let b = Fraction::from_u64(ib);
                    assert_eq!(a.neg().neg(), a);
                    assert_eq!(a.checked_add(b), ia.checked_add(ib).map(Fraction::from_u64));
                    assert_eq!(
                        a.checked_sub(b),
                        ia.checked_sub(ib)
                            .map(Fraction::from_u64)
                            .or_else(|| Some(Fraction::from_u64(ib - ia).neg()))
                    );
                    assert_eq!(a.checked_add(b.neg()), a.checked_sub(b));
                    assert_eq!(a.checked_sub(b.neg()), a.checked_add(b));
                    assert_eq!(a.neg().checked_add(b), b.checked_sub(a));
                    let negsum = a.checked_add(b).map(Fraction::neg);
                    assert_eq!(a.neg().checked_sub(b), negsum);
                    assert_eq!(a.neg().checked_add(b.neg()), negsum);
                    assert_eq!(a.neg().checked_sub(b.neg()), b.checked_sub(a));
                    assert_eq!(a.checked_mul(b.neg()), a.neg().checked_mul(b));
                    assert_eq!(a.neg().checked_mul(b.neg()), a.checked_mul(b));
                    assert_eq!(a.checked_mul(b), ia.checked_mul(ib).map(Fraction::from_u64));
                    assert_eq!(a.checked_rem(b), ia.checked_rem(ib).map(Fraction::from_u64));
                    if ia == 0 {
                        assert_eq!(Fraction::ONE.checked_div(a), None);
                    } else if ia < (1 << 63) {
                        // TODO: reciprocal is not bijective... maybe better to just eat the
                        // overhead of a separate sign bit than suffer quirks like this
                        assert_eq!(Fraction::ONE / (Fraction::ONE / a), a);
                    }
                    if ib != 0 && ib < (1 << 63) {
                        assert_eq!(
                            a.checked_div(Fraction::ONE / b),
                            ia.checked_mul(ib).map(Fraction::from_u64)
                        );
                    }
                }
            }
        }
    }

    // Test reduction after truncation to u64/i64.
    let n1 = Fraction::from_u64(611618166);
    let d1 = Fraction::from_u64(6074001097);
    let n2 = Fraction::from_u64(1413048844);
    let d2 = Fraction::from_u64(6074001001);
    let c = n1 / d1 + n2 / d2;
    assert_eq!(c.into_inner(), (1, 3));

    // Check underflow to zero.
    let x = Fraction::from_u64(1 << 63);
    let y = Fraction::ONE / x;
    assert_eq!(y * y, Fraction::ZERO);
    assert_eq!(y / x, Fraction::ZERO);
}
