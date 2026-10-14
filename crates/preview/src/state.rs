use ligature_pdf::Page;
use std::collections::VecDeque;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Zoom {
    Scale(f64),
    FitWidth,
    FitPage,
}

impl Zoom {
    pub fn scale(self, page: Page, width: f64, height: f64) -> f64 {
        let (w, h) = page.display_size();
        match self {
            Self::Scale(scale) => scale,
            Self::FitWidth => width / w,
            Self::FitPage => (width / w).min(height / h),
        }
        .clamp(0.1, 8.0)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RenderKey {
    pub generation: u64,
    pub page: usize,
    pub scale: u32,
}

pub fn limit_zoom(page: Page, scale: f64, density: f64) -> f64 {
    let display_scale = (scale * density * 1000.0).round() as u32;
    if page.raster_size(f64::from(display_scale) / 1000.0).is_ok() {
        return scale;
    }
    f64::from(bounded_raster_scale(page, 0, display_scale).max(1)) / 1000.0 / density
}

pub fn raster_scale(page: Page, display_scale: u32) -> u32 {
    bounded_raster_scale(page, display_scale, display_scale.saturating_mul(2))
}

fn bounded_raster_scale(page: Page, mut low: u32, mut high: u32) -> u32 {
    if page.raster_size(f64::from(high) / 1000.0).is_ok() {
        return high;
    }
    while low < high {
        let middle = low + (high - low).div_ceil(2);
        if page.raster_size(f64::from(middle) / 1000.0).is_ok() {
            low = middle;
        } else {
            high = middle - 1;
        }
    }
    low
}

pub struct Cache<T> {
    entries: VecDeque<(RenderKey, usize, T)>,
    bytes: usize,
    limit: usize,
}

impl<T: Clone> Cache<T> {
    pub fn new(limit: usize) -> Self {
        Self {
            entries: VecDeque::new(),
            bytes: 0,
            limit,
        }
    }

    pub fn get(&mut self, key: RenderKey) -> Option<T> {
        let index = self.entries.iter().position(|entry| entry.0 == key)?;
        let entry = self.entries.remove(index)?;
        let value = entry.2.clone();
        self.entries.push_back(entry);
        Some(value)
    }

    pub fn insert(&mut self, key: RenderKey, bytes: usize, value: T) {
        if let Some(index) = self.entries.iter().position(|entry| entry.0 == key) {
            self.bytes -= self.entries.remove(index).unwrap().1;
        }
        if bytes > self.limit {
            return;
        }
        while self.bytes + bytes > self.limit {
            self.bytes -= self.entries.pop_front().unwrap().1;
        }
        self.bytes += bytes;
        self.entries.push_back((key, bytes, value));
    }

    pub fn clear(&mut self) {
        self.entries.clear();
        self.bytes = 0;
    }
}

pub fn accepts(result: RenderKey, current: Option<RenderKey>) -> bool {
    current == Some(result)
}

pub fn zoom_anchor(pan: f64, anchor: f64, old_scale: f64, new_scale: f64) -> f64 {
    (pan + anchor) * new_scale / old_scale - anchor
}

#[cfg(test)]
mod tests {
    use super::*;
    fn key(page: usize) -> RenderKey {
        RenderKey {
            generation: 1,
            page,
            scale: 1000,
        }
    }

    #[test]
    fn preview_supersamples_standard_and_retina_displays() {
        let page = Page {
            width: 612.0,
            height: 792.0,
            rotation: 0,
        };
        assert_eq!(page.raster_size(f64::from(raster_scale(page, 1000)) / 1000.0).unwrap(), (1224, 1584));
        assert_eq!(page.raster_size(f64::from(raster_scale(page, 2000)) / 1000.0).unwrap(), (2448, 3168));
    }

    #[test]
    fn supersampling_stays_within_raster_limits_without_reducing_display_resolution() {
        let raster_edge = (ligature_pdf::MAX_RASTER_BYTES as f64 / 4.0).sqrt();
        for (page, display_scale) in [
            (
                Page {
                    width: raster_edge * 0.75,
                    height: raster_edge * 0.75,
                    rotation: 0,
                },
                1000,
            ),
            (
                Page {
                    width: 612.0,
                    height: 792.0,
                    rotation: 90,
                },
                3000,
            ),
            (
                Page {
                    width: 12_000.0,
                    height: 10.0,
                    rotation: 0,
                },
                1000,
            ),
            (
                Page {
                    width: 12_000.0,
                    height: 10.0,
                    rotation: 0,
                },
                3000,
            ),
        ] {
            let scale = raster_scale(page, display_scale);
            assert!(scale >= display_scale);
            assert!(scale <= display_scale * 2);
            if page.raster_size(f64::from(display_scale) / 1000.0).is_ok() {
                let (width, height) = page.raster_size(f64::from(scale) / 1000.0).unwrap();
                assert!(width * height * 4 <= ligature_pdf::MAX_RASTER_BYTES);
                if scale < display_scale * 2 {
                    assert!(page.raster_size(f64::from(scale + 1) / 1000.0).is_err());
                }
            } else {
                assert_eq!(scale, display_scale);
            }
        }
    }

    #[test]
    fn zoom_is_limited_by_raster_bytes_and_dimensions_at_the_display_density() {
        let raster_edge = (ligature_pdf::MAX_RASTER_BYTES as f64 / 4.0).sqrt();
        for page in [
            Page {
                width: raster_edge * 0.75,
                height: raster_edge * 0.75,
                rotation: 90,
            },
            Page {
                width: 12_000.0,
                height: 10.0,
                rotation: 0,
            },
        ] {
            for density in [1.0, 2.0] {
                for requested in [0.25, 1.0, 2.0, 8.0] {
                    let scale = limit_zoom(page, requested, density);
                    assert!(scale > 0.0 && scale <= requested);
                    assert_eq!(limit_zoom(page, scale, density), scale);
                    let display_scale = (scale * density * 1000.0).round() as u32;
                    let (width, height) = page.raster_size(f64::from(display_scale) / 1000.0).unwrap();
                    assert!(width * height * 4 <= ligature_pdf::MAX_RASTER_BYTES);
                    assert!(page.raster_size(f64::from(raster_scale(page, display_scale)) / 1000.0).is_ok());
                    if scale < requested {
                        assert!(page.raster_size(f64::from(display_scale + 1) / 1000.0).is_err());
                    } else {
                        assert!(page.raster_size((requested * density * 1000.0).round() / 1000.0).is_ok());
                    }
                }
            }
        }
    }

    #[test]
    fn cache_evicts_least_recently_used_and_rejects_oversized_pages() {
        let mut cache = Cache::new(10);
        cache.insert(key(0), 4, 0);
        cache.insert(key(1), 4, 1);
        assert_eq!(cache.get(key(0)), Some(0));
        cache.insert(key(2), 4, 2);
        assert_eq!(cache.get(key(1)), None);
        cache.insert(key(3), 11, 3);
        assert_eq!(cache.bytes, 8);
        cache.clear();
        assert_eq!(cache.get(key(0)), None);
    }

    #[test]
    fn obsolete_documents_and_zoom_results_are_rejected() {
        let old = key(0);
        assert!(!accepts(old, Some(RenderKey { generation: 2, ..old })));
        assert!(!accepts(old, Some(RenderKey { scale: 2000, ..old })));
        assert!(accepts(old, Some(old)));
    }

    #[test]
    fn fit_modes_and_zoom_keep_the_center_anchor() {
        let page = Page {
            width: 200.0,
            height: 400.0,
            rotation: 90,
        };
        assert_eq!(Zoom::FitWidth.scale(page, 600.0, 200.0), 1.5);
        assert_eq!(Zoom::FitPage.scale(page, 600.0, 200.0), 1.0);
        let pan = zoom_anchor(100.0, 200.0, 1.0, 2.0);
        assert_eq!((pan + 200.0) / 2.0, 300.0);
        for scale in [0.5, 1.0, 2.5] {
            let (x, y) = page.to_display(60.0, 120.0);
            assert_eq!(page.from_display(x * scale / scale, y * scale / scale), (60.0, 120.0));
        }
    }
}
