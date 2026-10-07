//! PDF document loading, page metadata, and page rasterization. Independent of GPUI.

use std::{path::Path, sync::Arc};

pub const MAX_RASTER_BYTES: usize = 64 * 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Page {
    pub width: f64,
    pub height: f64,
    pub rotation: i32,
}

impl Page {
    pub fn display_size(self) -> (f64, f64) {
        if self.rotation.rem_euclid(180) == 90 {
            (self.height, self.width)
        } else {
            (self.width, self.height)
        }
    }

    /// Convert top-left unrotated PDF points to top-left displayed points.
    pub fn to_display(self, x: f64, y: f64) -> (f64, f64) {
        match self.rotation.rem_euclid(360) {
            90 => (self.height - y, x),
            180 => (self.width - x, self.height - y),
            270 => (y, self.width - x),
            _ => (x, y),
        }
    }

    pub fn from_display(self, x: f64, y: f64) -> (f64, f64) {
        match self.rotation.rem_euclid(360) {
            90 => (y, self.height - x),
            180 => (self.width - x, self.height - y),
            270 => (self.width - y, x),
            _ => (x, y),
        }
    }

    pub fn raster_size(self, scale: f64) -> Result<(usize, usize), String> {
        let (width, height) = self.display_size();
        if !scale.is_finite() || scale <= 0.0 || !width.is_finite() || !height.is_finite() || width <= 0.0 || height <= 0.0 {
            return Err("Invalid PDF page size or render scale".into());
        }
        let width = (width * scale).ceil();
        let height = (height * scale).ceil();
        if width * height * 4.0 > MAX_RASTER_BYTES as f64 || width > 16384.0 || height > 16384.0 {
            return Err("PDF page raster exceeds the 64 MiB limit. Reduce zoom.".into());
        }
        Ok((width as usize, height as usize))
    }
}

pub struct Raster {
    pub width: u32,
    pub height: u32,
    pub bgra: Vec<u8>,
}

/// Immutable PDF snapshot. Native handles are created and released on each worker.
#[derive(Clone, Debug)]
pub struct Document {
    bytes: Arc<[u8]>,
    pages: Vec<Page>,
}

impl Document {
    pub fn load(path: &Path) -> Result<Self, String> {
        let bytes = std::fs::read(path).map_err(|error| format!("Cannot read PDF '{}': {error}", path.display()))?;
        Self::from_bytes(bytes.into())
    }

    pub fn from_bytes(bytes: Arc<[u8]>) -> Result<Self, String> {
        let pages = backend::pages(&bytes)?;
        Ok(Self { bytes, pages })
    }

    pub fn pages(&self) -> &[Page] {
        &self.pages
    }

    pub fn render(&self, page: usize, scale: f64) -> Result<Raster, String> {
        let metadata = self.pages.get(page).ok_or("PDF page is out of range")?;
        let (width, height) = metadata.raster_size(scale)?;
        backend::render(&self.bytes, page, width, height)
    }
}

#[cfg(target_os = "macos")]
mod backend {
    use super::*;
    use objc2_core_foundation::{CFData, CFRetained, CGPoint, CGRect, CGSize};
    use objc2_core_graphics::{
        CGBitmapContextCreate, CGColorSpace, CGContext, CGDataProvider, CGImageAlphaInfo, CGImageByteOrderInfo, CGPDFBox, CGPDFDocument, CGPDFPage,
    };

    fn open(bytes: &[u8]) -> Result<CFRetained<CGPDFDocument>, String> {
        let data = CFData::from_bytes(bytes);
        let provider = CGDataProvider::with_cf_data(Some(&data)).ok_or("Cannot create PDF data provider")?;
        let document = CGPDFDocument::with_provider(Some(&provider)).ok_or("Corrupt or unsupported PDF document")?;
        if !CGPDFDocument::is_unlocked(Some(&document)) {
            return Err("Password-protected PDF documents are unsupported".into());
        }
        Ok(document)
    }

    pub fn pages(bytes: &[u8]) -> Result<Vec<Page>, String> {
        let document = open(bytes)?;
        let count = CGPDFDocument::number_of_pages(Some(&document));
        if count == 0 {
            return Err("PDF document contains no pages".into());
        }
        (1..=count)
            .map(|index| {
                let page = CGPDFDocument::page(Some(&document), index).ok_or("Cannot load PDF page")?;
                let rect = CGPDFPage::box_rect(Some(&page), CGPDFBox::MediaBox);
                let metadata = Page {
                    width: rect.size.width,
                    height: rect.size.height,
                    rotation: CGPDFPage::rotation_angle(Some(&page)).rem_euclid(360),
                };
                if metadata.rotation % 90 != 0
                    || !metadata.width.is_finite()
                    || !metadata.height.is_finite()
                    || metadata.width <= 0.0
                    || metadata.height <= 0.0
                {
                    return Err("Invalid PDF page dimensions or rotation".into());
                }
                Ok(metadata)
            })
            .collect()
    }

    pub fn render(bytes: &[u8], index: usize, width: usize, height: usize) -> Result<Raster, String> {
        let document = open(bytes)?;
        let page = CGPDFDocument::page(Some(&document), index + 1).ok_or("Cannot load PDF page")?;
        let space = CGColorSpace::new_device_rgb().ok_or("Cannot create PDF color space")?;
        let mut bgra = vec![255u8; width * height * 4];
        // The buffer outlives the context and is not accessed until the context drops.
        let context = unsafe {
            CGBitmapContextCreate(
                bgra.as_mut_ptr().cast(),
                width,
                height,
                8,
                width * 4,
                Some(&space),
                CGImageByteOrderInfo::Order32Little.0 | CGImageAlphaInfo::PremultipliedFirst.0,
            )
        }
        .ok_or("Cannot allocate PDF bitmap context")?;
        let rect = CGRect::new(CGPoint::new(0.0, 0.0), CGSize::new(width as f64, height as f64));
        CGContext::set_rgb_fill_color(Some(&context), 1.0, 1.0, 1.0, 1.0);
        CGContext::fill_rect(Some(&context), rect);
        let media = CGPDFPage::box_rect(Some(&page), CGPDFBox::MediaBox);
        let rotated = CGPDFPage::rotation_angle(Some(&page)).rem_euclid(180) == 90;
        let (logical_width, logical_height) = if rotated {
            (media.size.height, media.size.width)
        } else {
            (media.size.width, media.size.height)
        };
        CGContext::scale_ctm(Some(&context), width as f64 / logical_width, height as f64 / logical_height);
        let logical_rect = CGRect::new(CGPoint::new(0.0, 0.0), CGSize::new(logical_width, logical_height));
        let transform = CGPDFPage::drawing_transform(Some(&page), CGPDFBox::MediaBox, logical_rect, 0, true);
        CGContext::concat_ctm(Some(&context), transform);
        CGContext::draw_pdf_page(Some(&context), Some(&page));
        drop(context);
        Ok(Raster {
            width: width as u32,
            height: height as u32,
            bgra,
        })
    }
}

#[cfg(not(target_os = "macos"))]
mod backend {
    use super::*;
    pub fn pages(_: &[u8]) -> Result<Vec<Page>, String> {
        Err("PDF rendering currently requires macOS".into())
    }
    pub fn render(_: &[u8], _: usize, _: usize, _: usize) -> Result<Raster, String> {
        Err("PDF rendering currently requires macOS".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(target_os = "macos")]
    #[test]
    fn loads_rotated_pages_and_renders_top_left_bgra_pixels() {
        let document = Document::from_bytes(Arc::from(include_bytes!("../tests/fixtures/pages.pdf").as_slice())).unwrap();
        assert_eq!(document.pages().len(), 2);
        assert_eq!(document.pages()[1].rotation, 90);
        assert_eq!(document.pages()[1].display_size(), (300.0, 200.0));
        let raster = document.render(0, 1.0).unwrap();
        assert_eq!((raster.width, raster.height), (200, 300));
        let pixel = |x: usize, y: usize| &raster.bgra[(y * 200 + x) * 4..(y * 200 + x) * 4 + 4];
        assert_eq!(pixel(5, 5), &[255, 0, 0, 255]);
        assert_eq!(pixel(5, 290), &[255, 255, 255, 255]);
        let raster = document.render(1, 2.0).unwrap();
        assert_eq!((raster.width, raster.height), (600, 400));
        assert_eq!(&raster.bgra[(10 * 600 + 590) * 4..(10 * 600 + 590) * 4 + 4], &[0, 255, 0, 255]);
        assert!(document.render(2, 1.0).is_err());
    }

    #[test]
    fn rotations_preserve_coordinate_round_trips() {
        for rotation in [0, 90, 180, 270] {
            let page = Page {
                width: 200.0,
                height: 300.0,
                rotation,
            };
            let (x, y) = page.to_display(40.0, 80.0);
            assert_eq!(page.from_display(x, y), (40.0, 80.0));
            assert_eq!(page.display_size(), if rotation % 180 == 0 { (200.0, 300.0) } else { (300.0, 200.0) });
        }
    }

    #[test]
    fn rejects_invalid_and_excessive_rasters() {
        let page = Page {
            width: 600.0,
            height: 800.0,
            rotation: 0,
        };
        for scale in [0.0, -1.0, f64::NAN, f64::INFINITY, 1000.0] {
            assert!(page.raster_size(scale).is_err());
        }
        assert_eq!(page.raster_size(1.5).unwrap(), (900, 1200));
    }

    #[test]
    fn reports_missing_and_corrupt_documents() {
        let directory = tempfile::tempdir().unwrap();
        assert!(Document::load(&directory.path().join("missing.pdf")).is_err());
        assert!(Document::from_bytes(Arc::from(b"broken".as_slice())).is_err());
    }
}
