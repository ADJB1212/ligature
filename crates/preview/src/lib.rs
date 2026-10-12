//! Continuous GPUI PDF preview, zoom, and rendered-page caching.

mod state;
pub use state::Zoom;

use gpui::{
    App, Bounds, Context, EventEmitter, FocusHandle, Focusable, IntoElement, KeyBinding, MouseButton, Pixels, Point, Render, RenderImage, Size, Task,
    Window, actions, canvas, div, fill, point, prelude::*, px, size,
};
use ligature_pdf::{Document, Page};
use ligature_synctex::{Mapping, PdfPosition, SourcePosition};
use ligature_theme::{colors, spacing};
use ligature_ui::{Indicator, caption, chrome_strip, indicator, inline_error};
use state::{Cache, RenderKey, accepts, raster_scale, zoom_anchor};
use std::{ops::Range, path::PathBuf, sync::Arc};

actions!(ligature_preview, [ZoomIn, ZoomOut, FitWidth, FitPage]);

pub fn init(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("+", ZoomIn, Some("LigaturePreview")),
        KeyBinding::new("-", ZoomOut, Some("LigaturePreview")),
    ]);
}

pub struct InverseNavigation(pub SourcePosition);

pub struct Preview {
    focus: FocusHandle,
    document: Option<Document>,
    pdf_path: Option<PathBuf>,
    mapping: Option<Mapping>,
    mapping_error: Option<String>,
    generation: u64,
    load_generation: u64,
    page: usize,
    zoom: Zoom,
    scale: f64,
    density: f64,
    viewport: Bounds<Pixels>,
    layout: Vec<Bounds<Pixels>>,
    content_size: Size<Pixels>,
    pan: Point<Pixels>,
    cache: Cache<Arc<RenderImage>>,
    images: Vec<(RenderKey, Arc<RenderImage>)>,
    render_task: Option<Task<()>>,
    load_task: Option<Task<()>>,
    failed_keys: Vec<RenderKey>,
    error: Option<String>,
    stale: bool,
    load_failed: bool,
    highlight: Option<PdfPosition>,
}

impl EventEmitter<InverseNavigation> for Preview {}

impl Preview {
    pub fn new(cx: &mut Context<Self>) -> Self {
        Self {
            focus: cx.focus_handle().tab_stop(true),
            document: None,
            pdf_path: None,
            mapping: None,
            mapping_error: None,
            generation: 0,
            load_generation: 0,
            page: 0,
            zoom: Zoom::FitWidth,
            scale: 1.0,
            density: 1.0,
            viewport: Bounds::default(),
            layout: Vec::new(),
            content_size: Size::default(),
            pan: Point::default(),
            cache: Cache::new(ligature_pdf::MAX_RASTER_BYTES),
            images: Vec::new(),
            render_task: None,
            load_task: None,
            failed_keys: Vec::new(),
            error: None,
            stale: false,
            load_failed: false,
            highlight: None,
        }
    }

    pub fn clear(&mut self, cx: &mut Context<Self>) {
        self.generation += 1;
        self.load_generation += 1;
        self.load_task = None;
        self.document = None;
        self.pdf_path = None;
        self.layout.clear();
        self.content_size = Size::default();
        self.page = 0;
        self.pan = Point::default();
        self.mapping = None;
        self.mapping_error = None;
        self.images.clear();
        self.failed_keys.clear();
        self.cache.clear();
        self.error = None;
        self.stale = false;
        self.load_failed = false;
        self.highlight = None;
        cx.notify();
    }

    pub fn set_stale(&mut self, stale: bool, cx: &mut Context<Self>) {
        if self.stale != stale {
            self.stale = stale;
            cx.notify();
        }
    }

    pub fn pdf_snapshot(&self) -> Option<(&std::path::Path, &Document)> {
        self.pdf_path.as_deref().zip(self.document.as_ref())
    }

    /// Load only artifacts from one successful build directory.
    pub fn load(&mut self, pdf: PathBuf, sidecar: Option<PathBuf>, source_directory: PathBuf, cx: &mut Context<Self>) {
        self.load_generation += 1;
        let generation = self.load_generation;
        self.load_task = Some(cx.spawn(async move |this, cx| {
            let pdf_path = pdf.clone();
            let result = cx
                .background_executor()
                .spawn(async move {
                    let document = Document::load(&pdf)?;
                    let sidecar = sidecar.or_else(|| ligature_synctex::sidecar(&pdf));
                    let mapping = match sidecar {
                        Some(path)
                            if path.parent() == pdf.parent()
                                && (path == pdf.with_extension("synctex.gz") || path == pdf.with_extension("synctex")) =>
                        {
                            Mapping::load(&path, &source_directory, document.pages().len())
                        }
                        Some(_) => Err("SyncTeX sidecar belongs to another build".into()),
                        None => Err("No SyncTeX sidecar. Enable SyncTeX in build settings and rebuild.".into()),
                    };
                    Ok::<_, String>((document, mapping))
                })
                .await;
            this.update(cx, |preview, cx| {
                if preview.load_generation != generation {
                    return;
                }
                preview.load_task = None;
                match result {
                    Ok((document, mapping)) => {
                        preview.load_failed = false;
                        let old_page = preview.page;
                        let anchor = preview.metadata().map(|page| {
                            let center = preview.viewport.origin + (preview.viewport.size / 2.0).into();
                            let position = center - preview.page_bounds().origin;
                            page.from_display(
                                f64::from(f32::from(position.x)) / preview.scale,
                                f64::from(f32::from(position.y)) / preview.scale,
                            )
                        });
                        preview.generation += 1;
                        preview.page = preview.page.min(document.pages().len() - 1);
                        preview.document = Some(document);
                        preview.pdf_path = Some(pdf_path);
                        preview.mapping_error = mapping.as_ref().err().cloned();
                        preview.mapping = mapping.ok();
                        preview.cache.clear();
                        preview.images.clear();
                        preview.failed_keys.clear();
                        preview.error = None;
                        preview.highlight = None;
                        preview.update_scale(cx);
                        if let Some((x, y)) = anchor
                            && preview.page == old_page
                            && let Some(page) = preview.metadata()
                        {
                            let (x, y) = page.to_display(x, y);
                            preview.pan =
                                point_px(x * preview.scale, y * preview.scale) - (preview.viewport.size / 2.0).into() + preview.page_inset();
                            preview.clamp_pan();
                        } else {
                            preview.pan = Point::default();
                        }
                        preview.schedule(cx);
                    }
                    Err(error) => {
                        preview.load_failed = true;
                        preview.error = Some(error);
                    }
                }
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }

    pub fn forward(&mut self, source: SourcePosition, cx: &mut Context<Self>) {
        if self.stale || self.load_failed || self.load_task.is_some() {
            self.error = Some("Preview and SyncTeX are older than the source. Rebuild before navigating.".into());
        } else if let Some(mapping) = &self.mapping {
            if let Some(point) = mapping.forward(&source) {
                self.go_to(point.page - 1, cx);
                self.highlight = Some(point);
                if let Some(page) = self.metadata() {
                    let (x, y) = page.to_display(point.x, point.y);
                    self.pan = point_px(
                        x * self.scale - f64::from(f32::from(self.viewport.size.width)) / 2.0,
                        y * self.scale - f64::from(f32::from(self.viewport.size.height)) / 2.0,
                    ) + self.page_inset();
                    self.clamp_pan();
                }
                self.schedule(cx);
                self.error = None;
            } else {
                self.error = Some("No PDF mapping for this source document".into());
            }
        } else {
            self.error = Some(
                self.mapping_error
                    .clone()
                    .unwrap_or_else(|| "Build a document before navigating to the preview".into()),
            );
        }
        cx.notify();
    }

    fn metadata(&self) -> Option<Page> {
        self.document.as_ref()?.pages().get(self.page).copied()
    }

    fn key(&self, page: usize) -> RenderKey {
        let display_scale = (self.scale * self.density * 1000.0).round() as u32;
        let scale = self
            .document
            .as_ref()
            .and_then(|document| document.pages().get(page))
            .map_or(display_scale, |page| raster_scale(*page, display_scale));
        RenderKey {
            generation: self.generation,
            page,
            scale,
        }
    }

    fn visible_pages(&self) -> Range<usize> {
        let first = self.layout.partition_point(|bounds| bounds.bottom() <= self.pan.y);
        let end = self
            .layout
            .partition_point(|bounds| bounds.origin.y < self.pan.y + self.viewport.size.height);
        first..end.max(first)
    }

    fn schedule(&mut self, cx: &mut Context<Self>) {
        let visible = self.visible_pages();
        let keys: Vec<_> = visible.map(|page| self.key(page)).collect();
        self.images.retain(|(key, _)| keys.contains(key));
        for key in &keys {
            if !self.images.iter().any(|(cached, _)| cached == key)
                && let Some(image) = self.cache.get(*key)
            {
                self.images.push((*key, image));
            }
        }
        if self.render_task.is_some() {
            return;
        }
        let Some(key) = keys
            .into_iter()
            .find(|key| !self.images.iter().any(|(cached, _)| cached == key) && !self.failed_keys.contains(key))
        else {
            return;
        };
        let document = self.document.as_ref().unwrap().clone();
        self.render_task = Some(cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    let raster = document.render(key.page, f64::from(key.scale) / 1000.0)?;
                    let bytes = raster.bgra.len();
                    let buffer = image::RgbaImage::from_raw(raster.width, raster.height, raster.bgra).ok_or("Invalid PDF raster")?;
                    Ok::<_, String>((bytes, Arc::new(RenderImage::new(vec![image::Frame::new(buffer)]))))
                })
                .await;
            this.update(cx, |preview, cx| {
                preview.render_task = None;
                if accepts(key, preview.document.as_ref().map(|_| preview.key(key.page))) {
                    match result {
                        Ok((bytes, image)) => {
                            preview.cache.insert(key, bytes, Arc::clone(&image));
                            preview.images.push((key, image));
                        }
                        Err(error) => {
                            preview.failed_keys.push(key);
                            preview.error = Some(error);
                        }
                    }
                }
                preview.schedule(cx);
                cx.notify();
            })
            .ok();
        }));
    }

    fn go_to(&mut self, page: usize, cx: &mut Context<Self>) {
        let Some(document) = &self.document else {
            return;
        };
        let page = page.min(document.pages().len() - 1);
        if self.page != page {
            self.page = page;
            self.highlight = None;
        }
        self.update_scale(cx);
        self.pan.y = self.page_inset().y - px(12.0);
        self.clamp_pan();
        self.schedule(cx);
        cx.notify();
    }

    pub fn zoom_in(&mut self, cx: &mut Context<Self>) {
        self.set_zoom(Zoom::Scale(self.scale * 1.25), cx);
    }

    pub fn zoom_out(&mut self, cx: &mut Context<Self>) {
        self.set_zoom(Zoom::Scale(self.scale / 1.25), cx);
    }

    pub fn set_zoom(&mut self, zoom: Zoom, cx: &mut Context<Self>) {
        if !self.failed_keys.is_empty() {
            self.error = None;
        }
        self.failed_keys.clear();
        self.zoom = zoom;
        self.update_scale(cx);
        self.schedule(cx);
        cx.notify();
    }

    fn update_scale(&mut self, _cx: &mut Context<Self>) {
        if self.document.is_none() {
            return;
        }
        let width = f64::from(f32::from(self.viewport.size.width));
        let height = f64::from(f32::from(self.viewport.size.height));
        self.rebuild_layout();
        let scale = self
            .document
            .as_ref()
            .unwrap()
            .pages()
            .iter()
            .map(|page| self.zoom.scale(*page, (width - 24.0).max(1.0), (height - 24.0).max(1.0)))
            .fold(f64::INFINITY, f64::min);
        if scale != self.scale {
            let origin = self.page_bounds().origin - self.viewport.origin;
            self.pan = point_px(
                zoom_anchor(-f64::from(f32::from(origin.x)), width, self.scale, scale),
                zoom_anchor(-f64::from(f32::from(origin.y)), height, self.scale, scale),
            );
            self.scale = scale;
            self.rebuild_layout();
            self.pan += self.page_inset();
        }
        self.clamp_pan();
    }

    fn clamp_pan(&mut self) {
        if self.document.is_none() {
            return;
        }
        self.pan.x = self
            .pan
            .x
            .clamp(px(0.0), (self.content_size.width - self.viewport.size.width).max(px(0.0)));
        self.pan.y = self
            .pan
            .y
            .clamp(px(0.0), (self.content_size.height - self.viewport.size.height).max(px(0.0)));
    }

    fn page_bounds(&self) -> Bounds<Pixels> {
        self.bounds_for_page(self.page)
    }

    fn bounds_for_page(&self, page: usize) -> Bounds<Pixels> {
        self.layout.get(page).map_or_else(Bounds::default, |bounds| {
            Bounds::new(bounds.origin + self.viewport.origin - self.pan, bounds.size)
        })
    }

    fn page_inset(&self) -> Point<Pixels> {
        self.page_bounds().origin - self.viewport.origin + self.pan
    }

    fn rebuild_layout(&mut self) {
        self.layout.clear();
        let Some(document) = &self.document else { return };
        let width = document.pages().iter().map(|page| page.display_size().0 * self.scale).fold(0.0, f64::max);
        let content_width = px(width as f32 + 24.0).max(self.viewport.size.width);
        let mut top = px(12.0);
        self.layout.extend(document.pages().iter().map(|page| {
            let (width, height) = page.display_size();
            let page_size = size(px((width * self.scale) as f32), px((height * self.scale) as f32));
            let origin = point((content_width - page_size.width) / 2.0, top);
            top += page_size.height + px(12.0);
            Bounds::new(origin, page_size)
        }));
        self.content_size = size(content_width, top);
    }

    fn scroll(&mut self, delta: Point<Pixels>, cx: &mut Context<Self>) {
        self.pan -= delta;
        self.clamp_pan();
        let center = self.pan.y + self.viewport.size.height / 2.0;
        let next = self.layout.partition_point(|bounds| bounds.bottom() < center);
        if let Some(page) = (next.saturating_sub(1)..(next + 1).min(self.layout.len())).min_by(|&a, &b| {
            let distance = |bounds: &Bounds<Pixels>| (bounds.origin.y - center).max(center - bounds.bottom()).max(px(0.0));
            distance(&self.layout[a]).partial_cmp(&distance(&self.layout[b])).unwrap()
        }) {
            self.page = page;
        }
        self.schedule(cx);
        cx.notify();
    }

    fn inverse(&mut self, position: Point<Pixels>, cx: &mut Context<Self>) {
        if self.stale || self.load_failed || self.load_task.is_some() {
            self.error = Some("SyncTeX is older than the source. Rebuild before navigating.".into());
            cx.notify();
            return;
        }
        let Some((index, bounds)) = self
            .visible_pages()
            .map(|index| (index, self.bounds_for_page(index)))
            .find(|(_, bounds)| bounds.contains(&position))
        else {
            return;
        };
        let page = self.document.as_ref().unwrap().pages()[index];
        let relative = position - bounds.origin;
        let (x, y) = page.from_display(
            f64::from(f32::from(relative.x)) / self.scale,
            f64::from(f32::from(relative.y)) / self.scale,
        );
        if let Some(source) = self
            .mapping
            .as_ref()
            .and_then(|mapping| mapping.inverse(PdfPosition { page: index + 1, x, y }))
        {
            cx.emit(InverseNavigation(source));
        } else {
            self.error = Some(
                self.mapping_error
                    .clone()
                    .unwrap_or_else(|| "No source mapping at this PDF location".into()),
            );
        }
        cx.notify();
    }
}

fn point_px(x: f64, y: f64) -> Point<Pixels> {
    point(px(x as f32), px(y as f32))
}

impl Focusable for Preview {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for Preview {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let entity = cx.entity();
        let paint_entity = entity.clone();
        let count = self.document.as_ref().map_or(0, |document| document.pages().len());
        let live = self.load_task.is_some() || self.render_task.is_some();
        let status = if count == 0 {
            if self.load_task.is_some() {
                "Loading PDF..."
            } else {
                "No compiled document."
            }
        } else if self.stale || self.load_failed {
            "Preview is older than the source."
        } else if self.load_task.is_some() {
            "Reloading PDF..."
        } else if self.render_task.is_some() {
            "Rendering page..."
        } else {
            ""
        };
        let status_kind = if live {
            Indicator::Live
        } else if self.stale || self.load_failed {
            Indicator::Alert
        } else if count > 0 {
            Indicator::On
        } else {
            Indicator::Idle
        };
        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(colors().background)
            .key_context("LigaturePreview")
            .track_focus(&self.focus)
            .on_action(cx.listener(|this, _: &ZoomIn, _, cx| this.zoom_in(cx)))
            .on_action(cx.listener(|this, _: &ZoomOut, _, cx| this.zoom_out(cx)))
            .on_action(cx.listener(|this, _: &FitWidth, _, cx| this.set_zoom(Zoom::FitWidth, cx)))
            .on_action(cx.listener(|this, _: &FitPage, _, cx| this.set_zoom(Zoom::FitPage, cx)))
            .child(
                chrome_strip()
                    .id("preview-toolbar")
                    .debug_selector(|| "preview-toolbar".into())
                    .flex()
                    .h(px(40.0))
                    .items_center()
                    .gap(spacing::large())
                    .border_b_1()
                    .border_color(colors().border)
                    .child(div().flex_1().child(caption("PDF Preview")))
                    .when(count > 0, |view| {
                        view.child(caption(format!("{} / {count}", self.page + 1)))
                            .child(caption(format!("{:.0}%", self.scale * 100.0)))
                    }),
            )
            .when_some(self.error.clone(), |view, error| view.child(inline_error(error)))
            .child(
                div()
                    .id("preview-viewport")
                    .debug_selector(|| "preview-viewport".into())
                    .relative()
                    .flex_1()
                    .min_h(px(0.0))
                    .overflow_hidden()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, event: &gpui::MouseDownEvent, window, cx| {
                            window.focus(&this.focus, cx);
                            if event.modifiers.platform {
                                this.inverse(event.position, cx);
                            }
                        }),
                    )
                    .on_scroll_wheel(cx.listener(|this, event: &gpui::ScrollWheelEvent, _, cx| {
                        this.scroll(event.delta.pixel_delta(px(20.0)), cx);
                        cx.stop_propagation();
                    }))
                    .child(
                        canvas(
                            move |bounds, window, cx| {
                                entity.update(cx, |preview, cx| {
                                    let density = f64::from(window.scale_factor());
                                    if preview.viewport != bounds || preview.density != density {
                                        preview.viewport = bounds;
                                        preview.density = density;
                                        preview.update_scale(cx);
                                        preview.schedule(cx);
                                    }
                                });
                            },
                            move |bounds, (), window, cx| {
                                paint_entity.update(cx, |preview, _| {
                                    for index in preview.visible_pages() {
                                        let page_bounds = preview.bounds_for_page(index);
                                        window.with_content_mask(Some(gpui::ContentMask { bounds }), |window| {
                                            window.paint_quad(fill(page_bounds, gpui::white()));
                                        });
                                        if let Some((_, image)) = preview.images.iter().find(|(key, _)| *key == preview.key(index)) {
                                            let _ = window.paint_image(bounds, page_bounds, gpui::Corners::default(), Arc::clone(image), 0, false);
                                        }
                                        if let Some(highlight) = preview.highlight
                                            && highlight.page == index + 1
                                        {
                                            let page = preview.document.as_ref().unwrap().pages()[index];
                                            let (x, y) = page.to_display(highlight.x, highlight.y);
                                            let highlight_bounds = Bounds::new(
                                                page_bounds.origin + point_px(x * preview.scale - 8.0, y * preview.scale - 8.0),
                                                size(px(16.0), px(16.0)),
                                            );
                                            window.with_content_mask(Some(gpui::ContentMask { bounds }), |window| {
                                                window.paint_quad(fill(highlight_bounds, colors().selection))
                                            });
                                        }
                                    }
                                });
                            },
                        )
                        .size_full(),
                    )
                    .when(count > 0 && !status.is_empty(), |view| {
                        view.child(
                            chrome_strip()
                                .absolute()
                                .bottom_0()
                                .left_0()
                                .right_0()
                                .flex()
                                .items_center()
                                .gap(spacing::medium())
                                .child(indicator("preview-status", status_kind))
                                .child(caption(status)),
                        )
                    })
                    .when(count == 0, |view| {
                        view.child(
                            div()
                                .absolute()
                                .inset_0()
                                .flex()
                                .flex_col()
                                .items_center()
                                .justify_center()
                                .gap(spacing::medium())
                                .text_color(colors().muted_text)
                                .child("Your document preview")
                                .child(caption(if live {
                                    "Loading PDF..."
                                } else if cfg!(target_os = "macos") {
                                    "Build with Cmd+B"
                                } else {
                                    "Build with Ctrl+B"
                                })),
                        )
                    }),
            )
    }
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;
    use gpui::{AppContext, TestAppContext};
    use std::cell::RefCell;
    use std::rc::Rc;

    fn fixture() -> (tempfile::TempDir, PathBuf) {
        let directory = tempfile::tempdir().unwrap();
        let pdf = directory.path().join("proof.pdf");
        std::fs::write(&pdf, include_bytes!("../../pdf/tests/fixtures/pages.pdf")).unwrap();
        (directory, pdf)
    }

    #[test]
    fn transient_status_preserves_the_pdf_viewport_bounds() {
        let (_directory, pdf) = fixture();
        let mut app = TestAppContext::single();
        let window = app.add_window(|_, cx| Preview::new(cx));
        let preview = window.root(&mut app).unwrap();
        preview.update(&mut app, |preview, _| preview.document = Some(Document::load(&pdf).unwrap()));
        let mut cx = gpui::VisualTestContext::from_window(window.into(), &app);
        cx.simulate_resize(size(px(400.0), px(600.0)));
        cx.update(|window, cx| window.draw(cx).clear(cx));
        let bounds = cx.debug_bounds("preview-viewport").unwrap();
        cx.update(|window, cx| {
            preview.update(cx, |preview, cx| {
                preview.set_stale(true, cx);
                preview.render_task = Some(cx.background_executor().spawn(async {}));
            });
            window.draw(cx).clear(cx);
        });
        assert_eq!(cx.debug_bounds("preview-viewport").unwrap(), bounds);
        cx.update(|window, cx| {
            preview.update(cx, |preview, cx| {
                preview.set_stale(false, cx);
                preview.render_task = None;
            });
            window.draw(cx).clear(cx);
        });
        assert_eq!(cx.debug_bounds("preview-viewport").unwrap(), bounds);
    }

    #[test]
    fn reload_preserves_page_and_pan_and_failed_load_keeps_the_last_document() {
        let (directory, pdf) = fixture();
        let mut cx = TestAppContext::single();
        let preview = cx.new(Preview::new);
        preview.update(&mut cx, |preview, cx| preview.load(pdf.clone(), None, directory.path().to_path_buf(), cx));
        cx.run_until_parked();
        preview.update(&mut cx, |preview, cx| {
            preview.viewport = Bounds::new(Point::default(), size(px(100.0), px(100.0)));
            preview.set_zoom(Zoom::Scale(1.0), cx);
            preview.go_to(1, cx);
            preview.pan.x = px(30.0);
            preview.pan.y += px(70.0);
            preview.load(pdf, None, directory.path().to_path_buf(), cx);
        });
        cx.run_until_parked();
        preview.read_with(&cx, |preview, _| {
            assert_eq!(preview.page, 1);
            assert_eq!(preview.pan, point(px(30.0), preview.page_inset().y + px(58.0)));
            assert!(preview.images.iter().any(|(key, _)| *key == preview.key(1)));
        });
        preview.update(&mut cx, |preview, cx| {
            preview.load(directory.path().join("missing.pdf"), None, directory.path().to_path_buf(), cx)
        });
        cx.run_until_parked();
        preview.read_with(&cx, |preview, _| {
            assert_eq!(preview.document.as_ref().unwrap().pages().len(), 2);
            assert_eq!(preview.page, 1);
            assert!(preview.error.as_ref().unwrap().contains("Cannot read PDF"));
            assert!(preview.load_failed);
        });
    }

    #[test]
    fn replacing_documents_and_zoom_during_render_accepts_only_the_latest_result() {
        let (directory, pdf) = fixture();
        let document = Document::load(&pdf).unwrap();
        let mut cx = TestAppContext::single();
        let preview = cx.new(Preview::new);
        preview.update(&mut cx, |preview, cx| {
            preview.document = Some(document);
            preview.viewport = Bounds::new(Point::default(), size(px(100.0), px(100.0)));
            preview.rebuild_layout();
            preview.schedule(cx);
            assert!(preview.render_task.is_some());
            preview.set_zoom(Zoom::Scale(2.0), cx);
            preview.clear(cx);
            preview.load(pdf, None, directory.path().to_path_buf(), cx);
        });
        cx.run_until_parked();
        preview.read_with(&cx, |preview, _| {
            assert!(preview.render_task.is_none());
            assert!(!preview.images.is_empty());
            assert!(preview.images.iter().all(|(key, _)| *key == preview.key(key.page)));
            assert_eq!(preview.generation, 2);
        });
    }

    #[test]
    fn scrolling_crosses_page_boundaries_and_renders_both_visible_pages() {
        let (_directory, pdf) = fixture();
        let document = Document::load(&pdf).unwrap();
        let mut cx = TestAppContext::single();
        let preview = cx.new(Preview::new);
        preview.update(&mut cx, |preview, cx| {
            preview.document = Some(document);
            preview.viewport = Bounds::new(point(px(10.0), px(20.0)), size(px(100.0), px(100.0)));
            preview.zoom = Zoom::Scale(1.0);
            preview.rebuild_layout();
            let pages = [preview.bounds_for_page(0), preview.bounds_for_page(1)];
            assert_eq!(pages[1].origin.y - pages[0].bottom(), px(12.0));
            let offset = pages[0].bottom() - preview.viewport.origin.y - px(20.0);
            preview.scroll(point(px(0.0), -offset), cx);
            assert_eq!(preview.visible_pages(), 0..2);
            assert_eq!(preview.page, 1);
        });
        cx.run_until_parked();
        preview.read_with(&cx, |preview, _| {
            assert_eq!(preview.images.len(), 2);
            assert!(preview.images.iter().any(|(key, _)| key.page == 0));
            assert!(preview.images.iter().any(|(key, _)| key.page == 1));
        });
        preview.update(&mut cx, |preview, cx| {
            let offset = preview.bounds_for_page(1).origin.y - preview.viewport.origin.y;
            preview.scroll(point(px(0.0), -offset), cx);
            assert_eq!(preview.visible_pages(), 1..2);
            preview.scroll(point(px(0.0), px(100_000.0)), cx);
            assert_eq!(preview.pan.y, px(0.0));
            assert_eq!(preview.page, 0);
            preview.scroll(point(px(0.0), px(-100_000.0)), cx);
            assert_eq!(preview.bounds_for_page(1).bottom(), preview.viewport.bottom() - px(12.0));
        });
    }

    #[test]
    fn viewport_translation_and_resize_refresh_page_geometry() {
        let (_directory, pdf) = fixture();
        let document = Document::load(&pdf).unwrap();
        let mut cx = TestAppContext::single();
        let preview = cx.new(Preview::new);
        preview.update(&mut cx, |preview, cx| {
            preview.document = Some(document);
            preview.viewport = Bounds::new(Point::default(), size(px(100.0), px(100.0)));
            preview.set_zoom(Zoom::Scale(1.0), cx);
            let before = preview.page_bounds();
            let visible = preview.visible_pages();
            preview.viewport.origin = point(px(30.0), px(50.0));
            assert_eq!(preview.page_bounds().origin, before.origin + preview.viewport.origin);
            assert_eq!(preview.visible_pages(), visible);

            preview.viewport.size = size(px(1000.0), px(1000.0));
            preview.update_scale(cx);
            assert_eq!(preview.pan, Point::default());
            for index in 0..2 {
                let bounds = preview.bounds_for_page(index);
                assert_eq!(bounds.origin.x + bounds.size.width / 2.0, preview.viewport.origin.x + px(500.0));
            }
            preview.set_zoom(Zoom::FitWidth, cx);
            let pages = preview.document.as_ref().unwrap().pages();
            let expected = pages
                .iter()
                .map(|page| Zoom::FitWidth.scale(*page, 976.0, 976.0))
                .fold(f64::INFINITY, f64::min);
            assert_eq!(preview.scale, expected);
            for (index, page) in pages.iter().enumerate() {
                let (width, height) = page.display_size();
                assert_eq!(
                    preview.bounds_for_page(index).size,
                    size(px((width * expected) as f32), px((height * expected) as f32))
                );
            }
        });
    }

    #[test]
    fn zoom_preserves_the_pdf_point_at_the_viewport_center() {
        let (_directory, pdf) = fixture();
        let document = Document::load(&pdf).unwrap();
        let mut cx = TestAppContext::single();
        let preview = cx.new(Preview::new);
        preview.update(&mut cx, |preview, cx| {
            preview.document = Some(document);
            preview.viewport = Bounds::new(point(px(10.0), px(20.0)), size(px(100.0), px(100.0)));
            preview.zoom = Zoom::Scale(1.0);
            preview.scale = 1.0;
            for page in [0, 1] {
                preview.page = page;
                preview.set_zoom(Zoom::Scale(1.0), cx);
                preview.pan = point(px(60.0), preview.page_inset().y + px(90.0));
                let center = preview.viewport.origin + point(px(50.0), px(50.0));
                let before = (center - preview.page_bounds().origin) / preview.scale as f32;
                preview.set_zoom(Zoom::Scale(2.0), cx);
                let after = (center - preview.page_bounds().origin) / preview.scale as f32;
                assert_eq!(before, after);
            }
        });
    }

    #[test]
    fn navigation_converts_rotation_and_zoom_and_rejects_stale_mappings() {
        let (_directory, pdf) = fixture();
        let document = Document::load(&pdf).unwrap();
        let mapping = Mapping::parse("SyncTeX Version:1\nInput:1:/project/chapter with spaces.tex\nOutput:pdf\nMagnification:1000\nUnit:1\nX Offset:0\nY Offset:0\nContent:\n{2\nx1,3:2621440,5242880\n}2\nPostamble:\nCount:1\n", std::path::Path::new("/project"), 2).unwrap();
        let source = SourcePosition {
            path: "/project/chapter with spaces.tex".into(),
            line: 3,
            column: 0,
        };
        let mut cx = TestAppContext::single();
        let preview = cx.new(Preview::new);
        let received = Rc::new(RefCell::new(None));
        let receiver = Rc::clone(&received);
        let _subscription = cx.update(|cx| {
            cx.subscribe(&preview, move |_, event: &InverseNavigation, _| {
                *receiver.borrow_mut() = Some(event.0.clone())
            })
        });
        preview.update(&mut cx, |preview, cx| {
            preview.document = Some(document);
            preview.mapping = Some(mapping);
            preview.viewport = Bounds::new(point(px(10.0), px(20.0)), size(px(100.0), px(100.0)));
            preview.forward(source.clone(), cx);
            assert_eq!(preview.page, 1);
            assert!(preview.highlight.is_some());
            for scale in [0.5, 1.0, 2.0] {
                preview.set_zoom(Zoom::Scale(scale), cx);
                let highlight = preview.highlight.unwrap();
                let (x, y) = preview.metadata().unwrap().to_display(highlight.x, highlight.y);
                let point = preview.page_bounds().origin + point_px(x * preview.scale, y * preview.scale);
                preview.page = 0;
                preview.inverse(point, cx);
                preview.page = 1;
            }
        });
        cx.run_until_parked();
        assert_eq!(*received.borrow(), Some(source.clone()));
        preview.update(&mut cx, |preview, cx| {
            preview.set_stale(true, cx);
            preview.forward(source, cx);
            assert!(preview.error.as_ref().unwrap().contains("older than the source"));
        });
    }
}
