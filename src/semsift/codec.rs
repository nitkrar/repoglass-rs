//! How a vector is stored: unit length, little-endian float16. Norms and
//! sums follow numpy's float32 order, so stored bytes match Python's.

use half::f16;

/// numpy's pairwise sum: eight accumulators up to 128 items, halves above.
pub fn pairwise<T: Copy + Default + std::ops::Add<Output = T>>(a: &[T]) -> T {
    let n = a.len();
    if n < 8 {
        let mut res = T::default();
        for &x in a {
            res = res + x;
        }
        res
    } else if n <= 128 {
        let mut r = [a[0], a[1], a[2], a[3], a[4], a[5], a[6], a[7]];
        let mut i = 8;
        while i < n - n % 8 {
            for j in 0..8 {
                r[j] = r[j] + a[i + j];
            }
            i += 8;
        }
        let mut res = ((r[0] + r[1]) + (r[2] + r[3])) + ((r[4] + r[5]) + (r[6] + r[7]));
        while i < n {
            res = res + a[i];
            i += 1;
        }
        res
    } else {
        let mut n2 = n / 2;
        n2 -= n2 % 8;
        pairwise(&a[..n2]) + pairwise(&a[n2..])
    }
}

/// A vector's norm as semsift computes it: squares summed pairwise in
/// float64, square root in float64, then narrowed to float32.
pub fn norm32(v: &[f32]) -> f32 {
    let squares: Vec<f64> = v.iter().map(|&x| x as f64 * x as f64).collect();
    pairwise(&squares).sqrt() as f32
}

/// A float32 dot product as numpy sums it: elementwise products, pairwise.
pub fn dot32(a: &[f32], b: &[f32]) -> f32 {
    let products: Vec<f32> = a.iter().zip(b).map(|(x, y)| x * y).collect();
    pairwise(&products)
}

/// Normalise to unit length in float32, then store as float16.
pub fn pack(v: &[f64]) -> Vec<u8> {
    let v32: Vec<f32> = v.iter().map(|&x| x as f32).collect();
    let norm = norm32(&v32);
    let mut out = Vec::with_capacity(v.len() * 2);
    for x in v32 {
        let y = if norm != 0.0 { x / norm } else { x };
        out.extend_from_slice(&f16::from_f32(y).to_le_bytes());
    }
    out
}

pub fn unpack(blob: &[u8]) -> Vec<f32> {
    blob.chunks_exact(2).map(|b| f16::from_le_bytes([b[0], b[1]]).to_f32()).collect()
}

/// Rows scaled to unit length in float32, norms summed pairwise.
pub fn unit_rows(rows: &mut [f32], dims: usize) {
    if dims == 0 {
        return;
    }
    for row in rows.chunks_exact_mut(dims) {
        let squares: Vec<f32> = row.iter().map(|x| x * x).collect();
        let norm = (0.0f32 + pairwise(&squares)).sqrt();
        let norm = if norm == 0.0 { 1.0 } else { norm };
        for x in row.iter_mut() {
            *x /= norm;
        }
    }
}
