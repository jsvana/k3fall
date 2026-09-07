#![allow(dead_code)]

pub fn shape_row(bins: &[u8], start: usize, end: usize, out_width: usize) -> Vec<u8> {
    let slice = &bins[start..end];
    let n = slice.len();
    let mut out = Vec::with_capacity(out_width);
    for i in 0..out_width {
        let lo = i * n / out_width;
        let hi = ((i + 1) * n / out_width).max(lo + 1).min(n);
        out.push(slice[lo..hi].iter().copied().max().unwrap_or(0));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn single_bin_peak_survives_any_width() {
        let mut bins = vec![10u8; 32768];
        bins[20_000] = 250;
        for width in [4096usize, 1920, 1000, 333, 17] {
            let out = shape_row(&bins, 0, 32768, width);
            assert_eq!(out.len(), width);
            assert_eq!(*out.iter().max().unwrap(), 250, "width {width}");
        }
    }

    #[test]
    fn slice_respects_bounds() {
        let mut bins = vec![0u8; 1000];
        bins[100] = 200; // outside slice
        bins[600] = 180; // inside slice
        let out = shape_row(&bins, 500, 900, 100);
        assert_eq!(*out.iter().max().unwrap(), 180);
    }

    #[test]
    fn upsample_when_slice_narrower_than_width() {
        let bins: Vec<u8> = (0..=99).collect();
        let out = shape_row(&bins, 0, 100, 400);
        assert_eq!(out.len(), 400);
        assert_eq!(out[0], 0);
        assert_eq!(out[399], 99);
    }
}
