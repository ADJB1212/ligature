//! GPUI PDF preview views, page navigation, zoom, and rendered-page caching.

mod state;
pub use state::Zoom;

use gpui::{
    App, Bounds, Context, Entity, EventEmitter, FocusHandle, Focusable, IntoElement, KeyBinding, MouseButton, Pixels, Point, Render, RenderImage,
    Task, Window, actions, canvas, div, fill, point, prelude::*, px, size,
};
use ligature_pdf::{Document, Page};
use ligature_synctex::{Mapping, PdfPosition, SourcePosition};
use ligature_theme::{colors, spacing};
use ligature_ui::{Button, Indicator, TextInput, caption, chrome_strip, indicator, inline_error};
use state::{Cache, RenderKey, accepts, zoom_anchor};
use std::{path::PathBuf, sync::Arc};

actions!(ligature_preview, [NextPage, PreviousPage, ZoomIn, ZoomOut, FitWidth, FitPage]);

pub fn init(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("right", NextPage, Some("LigaturePreview && !LigaturePageInput")),
        KeyBinding::new("left", PreviousPage, Some("LigaturePreview && !LigaturePageInput")),
        KeyBinding::new("+", ZoomIn, Some("LigaturePreview && !LigaturePageInput")),
        KeyBinding::new("-", ZoomOut, Some("LigaturePreview && !LigaturePageInput")),
    ]);
}

pub struct InverseNavigation(pub SourcePosition);

pub struct Preview {
    focus: FocusHandle,
    input: Entity<TextInput>,
    document: Option<Document>,
    mapping: Option<Mapping>,
    mapping_error: Option<String>,
    generation: u64,
    load_generation: u64,
    page: usize,
    zoom: Zoom,
    scale: f64,
    density: f64,
    viewport: Bounds<Pixels>,
    pan: Point<Pixels>,
    cache: Cache<Arc<RenderImage>>,
    image: Option<(RenderKey, Arc<RenderImage>)>,
    render_task: Option<Task<()>>,
    load_task: Option<Task<()>>,
    failed_key: Option<RenderKey>,
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
            input: cx.new(|cx| TextInput::new("Page", cx)),
            document: None,
            mapping: None,
            mapping_error: None,
            generation: 0,
            load_generation: 0,
            page: 0,
            zoom: Zoom::FitWidth,
            scale: 1.0,
            density: 1.0,
            viewport: Bounds::default(),
            pan: Point::default(),
            cache: Cache::new(ligature_pdf::MAX_RASTER_BYTES),
            image: None,
            render_task: None,
            load_task: None,
            failed_key: None,
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
        self.page = 0;
        self.pan = Point::default();
        self.mapping = None;
        self.mapping_error = None;
        self.image = None;
        self.cache.clear();
        self.error = None;
        self.stale = false;
        self.load_failed = false;
        self.highlight = None;
        self.input.update(cx, |input, cx| input.set_text("", cx));
        cx.notify();
    }

    pub fn set_stale(&mut self, stale: bool, cx: &mut Context<Self>) {
        if self.stale != stale {
            self.stale = stale;
            cx.notify();
        }
    }

    /// Load only artifacts from one successful build directory.
    pub fn load(&mut self, pdf: PathBuf, sidecar: Option<PathBuf>, source_directory: PathBuf, cx: &mut Context<Self>) {
        self.load_generation += 1;
        let generation = self.load_generation;
        self.load_task = Some(cx.spawn(async move |this, cx| {
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
                        preview.mapping_error = mapping.as_ref().err().cloned();
                        preview.mapping = mapping.ok();
                        preview.cache.clear();
                        preview.image = None;
                        preview.failed_key = None;
                        preview.error = None;
                        preview.highlight = None;
                        preview.input.update(cx, |input, cx| input.set_text(&(preview.page + 1).to_string(), cx));
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

    fn key(&self) -> Option<RenderKey> {
        self.metadata()?;
        Some(RenderKey {
            generation: self.generation,
            page: self.page,
            scale: (self.scale * self.density * 1000.0).round() as u32,
        })
    }

    fn schedule(&mut self, cx: &mut Context<Self>) {
        let Some(key) = self.key() else {
            return;
        };
        if self.image.as_ref().is_some_and(|image| image.0 == key) || self.failed_key == Some(key) {
            return;
        }
        if let Some(image) = self.cache.get(key) {
            self.image = Some((key, image));
            cx.notify();
            return;
        }
        if self.render_task.is_some() {
            return;
        }
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
                if accepts(key, preview.key()) {
                    match result {
                        Ok((bytes, image)) => {
                            preview.cache.insert(key, bytes, Arc::clone(&image));
                            preview.image = Some((key, image));
                        }
                        Err(error) => {
                            preview.failed_key = Some(key);
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
            self.pan = Point::default();
            self.highlight = None;
        }
        self.input.update(cx, |input, cx| input.set_text(&(page + 1).to_string(), cx));
        self.update_scale(cx);
        self.schedule(cx);
        cx.notify();
    }

    fn submit_page(&mut self, cx: &mut Context<Self>) {
        match self.input.read(cx).text().parse::<usize>() {
            Ok(page) if page > 0 && self.document.as_ref().is_some_and(|document| page <= document.pages().len()) => {
                self.error = None;
                self.go_to(page - 1, cx);
            }
            _ => {
                self.error = Some("Enter a page number within this document".into());
                cx.notify();
            }
        }
    }

    fn set_zoom(&mut self, zoom: Zoom, cx: &mut Context<Self>) {
        if self.failed_key.is_some() {
            self.error = None;
        }
        self.zoom = zoom;
        self.update_scale(cx);
        self.schedule(cx);
        cx.notify();
    }

    fn update_scale(&mut self, _cx: &mut Context<Self>) {
        let Some(page) = self.metadata() else {
            return;
        };
        let width = f64::from(f32::from(self.viewport.size.width));
        let height = f64::from(f32::from(self.viewport.size.height));
        let scale = self.zoom.scale(page, (width - 24.0).max(1.0), (height - 24.0).max(1.0));
        if scale != self.scale {
            let origin = self.page_bounds().origin - self.viewport.origin;
            self.pan = point_px(
                zoom_anchor(-f64::from(f32::from(origin.x)), width, self.scale, scale),
                zoom_anchor(-f64::from(f32::from(origin.y)), height, self.scale, scale),
            );
            self.scale = scale;
            self.pan += self.page_inset();
        }
        self.clamp_pan();
    }

    fn clamp_pan(&mut self) {
        let Some(page) = self.metadata() else {
            return;
        };
        let (width, height) = page.display_size();
        self.pan.x = self.pan.x.clamp(
            px(0.0),
            px((width * self.scale + 24.0 - f64::from(f32::from(self.viewport.size.width))).max(0.0) as f32),
        );
        self.pan.y = self.pan.y.clamp(
            px(0.0),
            px((height * self.scale + 24.0 - f64::from(f32::from(self.viewport.size.height))).max(0.0) as f32),
        );
    }

    fn page_bounds(&self) -> Bounds<Pixels> {
        let (width, height) = self.metadata().map_or((0.0, 0.0), Page::display_size);
        let page_size = size(px((width * self.scale) as f32), px((height * self.scale) as f32));
        Bounds::new(self.viewport.origin + self.page_inset() - self.pan, page_size)
    }

    fn page_inset(&self) -> Point<Pixels> {
        let (width, height) = self.metadata().map_or((0.0, 0.0), Page::display_size);
        let page_size = size(px((width * self.scale) as f32), px((height * self.scale) as f32));
        point(
            (self.viewport.size.width - page_size.width).max(px(24.0)) / 2.0,
            (self.viewport.size.height - page_size.height).max(px(24.0)) / 2.0,
        )
    }

    fn inverse(&mut self, position: Point<Pixels>, cx: &mut Context<Self>) {
        if self.stale || self.load_failed || self.load_task.is_some() {
            self.error = Some("SyncTeX is older than the source. Rebuild before navigating.".into());
            cx.notify();
            return;
        }
        let bounds = self.page_bounds();
        if !bounds.contains(&position) {
            return;
        }
        let Some(page) = self.metadata() else {
            return;
        };
        let relative = position - bounds.origin;
        let (x, y) = page.from_display(
            f64::from(f32::from(relative.x)) / self.scale,
            f64::from(f32::from(relative.y)) / self.scale,
        );
        if let Some(source) = self
            .mapping
            .as_ref()
            .and_then(|mapping| mapping.inverse(PdfPosition { page: self.page + 1, x, y }))
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
            "Cmd-click a page to reveal its source."
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
            .on_action(cx.listener(|this, _: &NextPage, _, cx| this.go_to(this.page + 1, cx)))
            .on_action(cx.listener(|this, _: &PreviousPage, _, cx| this.go_to(this.page.saturating_sub(1), cx)))
            .on_action(cx.listener(|this, _: &ZoomIn, _, cx| this.set_zoom(Zoom::Scale(this.scale * 1.25), cx)))
            .on_action(cx.listener(|this, _: &ZoomOut, _, cx| this.set_zoom(Zoom::Scale(this.scale / 1.25), cx)))
            .on_action(cx.listener(|this, _: &FitWidth, _, cx| this.set_zoom(Zoom::FitWidth, cx)))
            .on_action(cx.listener(|this, _: &FitPage, _, cx| this.set_zoom(Zoom::FitPage, cx)))
            .child(
                chrome_strip()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(spacing::small())
                    .child(
                        Button::new("prev-page", "<")
                            .disabled(count == 0 || self.page == 0)
                            .on_click(cx.listener(|this, _, _, cx| this.go_to(this.page.saturating_sub(1), cx))),
                    )
                    .child(
                        div()
                            .w(px(48.0))
                            .key_context("LigaturePageInput")
                            .child(self.input.clone())
                            .on_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, _, cx| {
                                if event.keystroke.key == "enter" {
                                    this.submit_page(cx);
                                    cx.stop_propagation();
                                }
                            })),
                    )
                    .child(caption(format!("/ {count}")))
                    .child(
                        Button::new("go-page", "Go")
                            .disabled(count == 0)
                            .on_click(cx.listener(|this, _, _, cx| this.submit_page(cx))),
                    )
                    .child(
                        Button::new("next-page", ">")
                            .disabled(count == 0 || self.page + 1 == count)
                            .on_click(cx.listener(|this, _, _, cx| this.go_to(this.page + 1, cx))),
                    )
                    .child(
                        Button::new("zoom-out", "-")
                            .disabled(count == 0)
                            .on_click(cx.listener(|this, _, _, cx| this.set_zoom(Zoom::Scale(this.scale / 1.25), cx))),
                    )
                    .child(caption(format!("{:.0}%", self.scale * 100.0)))
                    .child(
                        Button::new("zoom-in", "+")
                            .disabled(count == 0)
                            .on_click(cx.listener(|this, _, _, cx| this.set_zoom(Zoom::Scale(this.scale * 1.25), cx))),
                    )
                    .child(
                        Button::new("fit-width", "Width")
                            .selected(self.zoom == Zoom::FitWidth)
                            .disabled(count == 0)
                            .on_click(cx.listener(|this, _, _, cx| this.set_zoom(Zoom::FitWidth, cx))),
                    )
                    .child(
                        Button::new("fit-page", "Page")
                            .selected(self.zoom == Zoom::FitPage)
                            .disabled(count == 0)
                            .on_click(cx.listener(|this, _, _, cx| this.set_zoom(Zoom::FitPage, cx))),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_1()
                            .min_w(px(0.0))
                            .items_center()
                            .justify_end()
                            .gap(spacing::small())
                            .child(indicator("preview-status", status_kind))
                            .child(caption(status)),
                    ),
            )
            .when_some(self.error.clone(), |view, error| view.child(inline_error(error)))
            .child(
                div()
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
                        this.pan -= event.delta.pixel_delta(px(20.0));
                        this.clamp_pan();
                        cx.notify();
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
                                    if let Some((key, image)) = &preview.image
                                        && Some(*key) == preview.key()
                                    {
                                        let page_bounds = preview.page_bounds();
                                        let _ = window.paint_image(bounds, page_bounds, gpui::Corners::default(), Arc::clone(image), 0, false);
                                        if let Some(highlight) = preview.highlight
                                            && highlight.page == preview.page + 1
                                            && let Some(page) = preview.metadata()
                                        {
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
                    ),
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
            preview.pan = point(px(30.0), px(70.0));
            preview.load(pdf, None, directory.path().to_path_buf(), cx);
        });
        cx.run_until_parked();
        preview.read_with(&cx, |preview, _| {
            assert_eq!(preview.page, 1);
            assert_eq!(preview.pan, point(px(30.0), px(70.0)));
            assert_eq!(preview.image.as_ref().unwrap().0, preview.key().unwrap());
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
            preview.schedule(cx);
            assert!(preview.render_task.is_some());
            preview.set_zoom(Zoom::Scale(2.0), cx);
            preview.clear(cx);
            preview.load(pdf, None, directory.path().to_path_buf(), cx);
        });
        cx.run_until_parked();
        preview.read_with(&cx, |preview, _| {
            assert!(preview.render_task.is_none());
            assert_eq!(preview.image.as_ref().unwrap().0, preview.key().unwrap());
            assert_eq!(preview.generation, 2);
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
            preview.pan = point(px(60.0), px(90.0));
            let center = preview.viewport.origin + point(px(50.0), px(50.0));
            let before = (center - preview.page_bounds().origin) / preview.scale as f32;
            preview.set_zoom(Zoom::Scale(2.0), cx);
            let after = (center - preview.page_bounds().origin) / preview.scale as f32;
            assert_eq!(before, after);
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
                preview.inverse(point, cx);
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
