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

pub fn zoom_anchor(pan: f64, viewport: f64, old_scale: f64, new_scale: f64) -> f64 {
    (pan + viewport / 2.0) * new_scale / old_scale - viewport / 2.0
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
        let pan = zoom_anchor(100.0, 400.0, 1.0, 2.0);
        assert_eq!((pan + 200.0) / 2.0, 300.0);
        for scale in [0.5, 1.0, 2.5] {
            let (x, y) = page.to_display(60.0, 120.0);
            assert_eq!(page.from_display(x * scale / scale, y * scale / scale), (60.0, 120.0));
        }
    }
}
