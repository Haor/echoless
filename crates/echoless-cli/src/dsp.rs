const DBFS_FLOOR: f64 = -120.0;

pub(crate) fn sum_squares(samples: &[f32]) -> f64 {
    samples.iter().map(|v| f64::from(*v) * f64::from(*v)).sum()
}

pub(crate) fn rms_dbfs_from_sum_squares(sum_sq: f64, samples: u64) -> f64 {
    if samples == 0 || sum_sq <= 0.0 {
        return DBFS_FLOOR;
    }
    let rms = (sum_sq / samples as f64).sqrt().max(1e-6);
    (20.0 * rms.log10()).max(DBFS_FLOOR)
}

pub(crate) fn rms_dbfs(samples: &[f32]) -> f64 {
    rms_dbfs_from_sum_squares(sum_squares(samples), samples.len() as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rms_dbfs_reports_silence_floor() {
        assert_eq!(rms_dbfs(&[]), DBFS_FLOOR);
        assert_eq!(rms_dbfs(&[0.0, 0.0]), DBFS_FLOOR);
        assert_eq!(rms_dbfs_from_sum_squares(0.0, 480), DBFS_FLOOR);
    }

    #[test]
    fn rms_dbfs_reports_full_scale() {
        assert_eq!(rms_dbfs(&[1.0, -1.0]), 0.0);
        assert_eq!(rms_dbfs_from_sum_squares(480.0, 480), 0.0);
    }

    #[test]
    fn rms_dbfs_reports_half_scale() {
        let db = rms_dbfs(&[0.5, -0.5]);

        assert!((db + 6.020_599_913_279_624).abs() < 1e-12);
    }

    #[test]
    fn rms_dbfs_clamps_tiny_nonzero_values() {
        assert_eq!(rms_dbfs(&[1e-8]), DBFS_FLOOR);
    }
}
