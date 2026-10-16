//! The raw socket remains packed UYVY on both ISPs. Normalize only requested frames; the
//! encoder keeps the capture buffer and never pays for a full-frame colour conversion.
use anyhow::{Result, ensure};

fn plane(data: &[u8], stride: usize, row_bytes: usize, rows: usize) -> Result<()> {
    ensure!(
        stride >= row_bytes && rows > 0,
        "invalid video plane stride"
    );
    let required = (rows - 1)
        .checked_mul(stride)
        .and_then(|n| n.checked_add(row_bytes));
    ensure!(
        required.is_some_and(|n| data.len() >= n),
        "truncated video plane"
    );
    Ok(())
}

pub fn packed_uyvy(data: &[u8], stride: usize, width: usize, height: usize) -> Result<Vec<u8>> {
    ensure!(width > 0 && width % 2 == 0, "UYVY needs an even width");
    let row = width
        .checked_mul(2)
        .ok_or_else(|| anyhow::anyhow!("frame too large"))?;
    plane(data, stride, row, height)?;
    let mut out = Vec::with_capacity(row * height);
    for y in 0..height {
        out.extend_from_slice(&data[y * stride..y * stride + row]);
    }
    Ok(out)
}

pub fn nv12_to_uyvy(
    y: &[u8],
    y_stride: usize,
    uv: &[u8],
    uv_stride: usize,
    width: usize,
    height: usize,
) -> Result<Vec<u8>> {
    ensure!(
        width > 0 && width % 2 == 0 && height % 2 == 0,
        "NV12 needs even dimensions"
    );
    plane(y, y_stride, width, height)?;
    plane(uv, uv_stride, width, height / 2)?;
    let bytes = width
        .checked_mul(height)
        .and_then(|n| n.checked_mul(2))
        .ok_or_else(|| anyhow::anyhow!("frame too large"))?;
    let mut out = Vec::with_capacity(bytes);
    for row in 0..height {
        let luma = &y[row * y_stride..][..width];
        let chroma = &uv[(row / 2) * uv_stride..][..width];
        for x in (0..width).step_by(2) {
            out.extend_from_slice(&[chroma[x], luma[x], chroma[x + 1], luma[x + 1]]);
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_padding_without_shifting_rows() {
        assert_eq!(
            packed_uyvy(&[1, 2, 3, 4, 99, 99, 5, 6, 7, 8], 6, 2, 2).unwrap(),
            [1, 2, 3, 4, 5, 6, 7, 8]
        );
    }

    #[test]
    fn nv12_preserves_luma_uv_order_and_repeats_chroma_vertically() {
        // Independent plane strides are legal; reading a single contiguous block shears it.
        let y = [
            1, 2, 3, 4, 99, 99, 5, 6, 7, 8, 99, 99, 9, 10, 11, 12, 99, 99, 13, 14, 15, 16,
        ];
        let uv = [20, 30, 40, 50, 99, 60, 70, 80, 90];
        assert_eq!(
            nv12_to_uyvy(&y, 6, &uv, 5, 4, 4).unwrap(),
            [
                20, 1, 30, 2, 40, 3, 50, 4, 20, 5, 30, 6, 40, 7, 50, 8, 60, 9, 70, 10, 80, 11, 90,
                12, 60, 13, 70, 14, 80, 15, 90, 16
            ]
        );
    }

    #[test]
    fn malformed_buffers_are_rejected_instead_of_panicking_in_a_gst_callback() {
        assert!(nv12_to_uyvy(&[0; 4], 2, &[0], 2, 2, 2).is_err());
        assert!(nv12_to_uyvy(&[], 0, &[], 0, 0, 0).is_err());
        assert!(packed_uyvy(&[0; 8], 3, 2, 2).is_err());
        assert!(packed_uyvy(&[0; 8], 4, 3, 1).is_err());
    }
}
