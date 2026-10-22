use std::path::PathBuf;

use gpui::{
    Bounds, Context, Entity, FocusHandle, Focusable, IntoElement, Render, Subscription, Task, TitlebarOptions, WeakEntity, Window, WindowBounds,
    WindowOptions, div, prelude::*, px, size,
};
use ligature_settings::{Configuration, Indentation, SaveBeforeBuild, Settings};
use ligature_theme::{colors, spacing, typography, window_background};
use ligature_ui::{Button, TextInput, caption, chrome_strip, hairline, inline_error};

use crate::{Close, OpenSettings, Save, Workspace};

#[cfg(test)]
#[path = "settings_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "../tests/unit/formatter_settings.rs"]
mod formatter_tests;

impl Workspace {
    pub fn open_settings(&mut self, cx: &mut Context<Self>) {
        if let Some(handle) = self.settings_window
            && handle.update(cx, |_, window, _| window.activate()).is_ok()
        {
            return;
        }
        let workspace = cx.weak_entity();
        let configuration = self.configuration.clone();
        let path = self.configuration_path.clone();
        let project = self.project.as_ref().map(|project| project.root.clone());
        let bounds = Bounds::centered(None, size(px(560.0), px(640.0)), cx);
        match cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                titlebar: Some(TitlebarOptions {
                    title: Some("Settings".into()),
                    ..Default::default()
                }),
                window_min_size: Some(size(px(480.0), px(400.0))),
                ..Default::default()
            },
            |window, cx| cx.new(|cx| SettingsView::new(workspace, configuration, path, project, window, cx)),
        ) {
            Ok(handle) => self.settings_window = Some(handle),
            Err(error) => {
                self.error = Some(format!("Could not open Settings: {error}"));
                cx.notify();
            }
        }
    }

    fn configuration_saved(&mut self, configuration: Configuration, cx: &mut Context<Self>) {
        ligature_theme::set_theme(configuration.theme.as_deref(), cx);
        let settings = configuration.settings_for(self.project.as_ref().map(|project| project.root.as_path()));
        let language_changed = self.settings.language_server != settings.language_server;
        self.settings = settings;
        self.configuration = configuration;
        self.configuration_errors.clear();
        self.settings_errors.clear();
        self.apply_settings(cx);
        if language_changed {
            self.restart_language_server(cx);
        }
        self.search.preview = None;
        self.queue_project_search(cx);
        self.queue_semantics(cx);
        cx.notify();
    }
}

struct Fields {
    settings: Settings,
    family: Entity<TextInput>,
    size: Entity<TextInput>,
    width: Entity<TextInput>,
    output: Entity<TextInput>,
    reruns: Entity<TextInput>,
    root: Entity<TextInput>,
    _subscriptions: Vec<Subscription>,
}

impl Fields {
    fn new(settings: Settings, cx: &mut Context<SettingsView>) -> Self {
        let mut subscriptions = Vec::new();
        let mut input = |value: &str, placeholder: &str| {
            let input = cx.new(|cx| {
                let mut input = TextInput::new(placeholder.to_owned(), cx);
                input.set_text(value, cx);
                input
            });
            let mut previous = input.read(cx).text().to_owned();
            subscriptions.push(cx.observe(&input, move |view, input, cx| {
                let text = input.read(cx).text();
                if text != previous {
                    text.clone_into(&mut previous);
                    view.saved = false;
                    cx.notify();
                }
            }));
            input
        };
        Self {
            family: input(settings.font.family.as_deref().unwrap_or_default(), "System default"),
            size: input(&settings.font.size.to_string(), "15"),
            width: input(&settings.indentation.width.to_string(), "4"),
            output: input(&settings.build.output_directory.to_string_lossy(), "build"),
            reruns: input(&settings.build.reruns.map(|value| value.to_string()).unwrap_or_default(), "Automatic"),
            root: input(
                &settings
                    .root_document
                    .as_ref()
                    .map(|path| path.to_string_lossy().into_owned())
                    .unwrap_or_default(),
                "Automatic discovery",
            ),
            settings,
            _subscriptions: subscriptions,
        }
    }

    fn read(&self, cx: &gpui::App) -> Result<Settings, String> {
        let mut settings = self.settings.clone();
        let family = self.family.read(cx).text().trim();
        settings.font.family = (!family.is_empty()).then(|| family.to_owned());
        settings.font.size = self
            .size
            .read(cx)
            .text()
            .trim()
            .parse()
            .map_err(|_| "Font size must be a number from 8 to 72")?;
        settings.indentation.width = self
            .width
            .read(cx)
            .text()
            .trim()
            .parse()
            .map_err(|_| "Indentation width must be a whole number from 1 to 16")?;
        settings.build.output_directory = self.output.read(cx).text().trim().into();
        let reruns = self.reruns.read(cx).text().trim();
        settings.build.reruns = if reruns.is_empty() {
            None
        } else {
            Some(
                reruns
                    .parse()
                    .map_err(|_| "Reruns must be a whole number from 0 to 10, or empty for automatic")?,
            )
        };
        let root = self.root.read(cx).text().trim();
        settings.root_document = (!root.is_empty()).then(|| root.into());
        let errors: Vec<_> = settings
            .validate()
            .into_iter()
            .map(|error| {
                error
                    .split(';')
                    .next()
                    .unwrap_or_default()
                    .replace("font.size", "Font size")
                    .replace("font.family", "Font family")
                    .replace("indentation.width", "Indentation width")
                    .replace("build.reruns", "Reruns")
                    .replace("build.output_directory", "Output folder")
                    .replace("root_document", "Main document")
            })
            .collect();
        if errors.is_empty() { Ok(settings) } else { Err(errors.join("; ")) }
    }
}

pub(super) struct SettingsView {
    workspace: WeakEntity<Workspace>,
    configuration: Configuration,
    path: Option<PathBuf>,
    project: Option<PathBuf>,
    project_scope: bool,
    fields: Fields,
    focus: FocusHandle,
    error: Option<String>,
    saved: bool,
    saving: bool,
    task: Option<Task<()>>,
}

impl SettingsView {
    fn new(
        workspace: WeakEntity<Workspace>, configuration: Configuration, path: Option<PathBuf>, project: Option<PathBuf>, window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let fields = Fields::new(configuration.defaults.clone(), cx);
        let entity = cx.weak_entity();
        window.on_window_should_close(cx, move |_, cx| entity.update(cx, |view, _| !view.saving).unwrap_or(true));
        window.focus(&fields.family.focus_handle(cx), cx);
        Self {
            workspace,
            configuration,
            path,
            project,
            project_scope: false,
            fields,
            focus: cx.focus_handle(),
            error: None,
            saved: false,
            saving: false,
            task: None,
        }
    }

    fn inherits(&self) -> bool {
        self.project_scope && self.project.as_ref().is_none_or(|path| !self.configuration.projects.contains_key(path))
    }

    fn capture(&mut self, cx: &gpui::App) -> Result<(), String> {
        if self.inherits() {
            return Ok(());
        }
        let settings = self.fields.read(cx)?;
        if self.project_scope {
            self.configuration
                .projects
                .insert(self.project.clone().expect("project settings have a project"), settings);
        } else {
            self.configuration.defaults = settings;
        }
        Ok(())
    }

    fn switch_scope(&mut self, project_scope: bool, window: &mut Window, cx: &mut Context<Self>) {
        if let Err(error) = self.capture(cx) {
            self.error = Some(error);
            cx.notify();
            return;
        }
        self.project_scope = project_scope;
        let settings = self
            .configuration
            .settings_for(if project_scope { self.project.as_deref() } else { None });
        self.fields = Fields::new(settings, cx);
        self.error = None;
        self.saved = false;
        window.focus(&self.focus, cx);
        cx.notify();
    }

    fn toggle_project_settings(&mut self, cx: &mut Context<Self>) {
        let path = self.project.clone().expect("project settings have a project");
        if self.inherits() {
            self.configuration.projects.insert(path, self.configuration.defaults.clone());
        } else {
            self.configuration.projects.remove(&path);
        }
        self.fields = Fields::new(self.configuration.settings_for(self.project.as_deref()), cx);
        self.saved = false;
        self.error = None;
        cx.notify();
    }

    fn save(&mut self, cx: &mut Context<Self>) {
        if self.saving {
            return;
        }
        self.saved = false;
        let result = self.capture(cx).and_then(|()| {
            self.path
                .clone()
                .ok_or_else(|| "Set XDG_CONFIG_HOME or HOME to an absolute directory to save settings".into())
        });
        let path = match result {
            Ok(path) => path,
            Err(error) => {
                self.error = Some(error);
                cx.notify();
                return;
            }
        };
        let configuration = self.configuration.clone();
        let project = self.project.clone();
        self.saving = true;
        self.error = None;
        self.task = Some(cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    if let Some(root) = project
                        && let Some(document) = configuration.projects.get(&root).and_then(|settings| settings.root_document.as_ref())
                    {
                        ligature_project::Project::open(&root)?.set_root_document(document)?;
                    }
                    configuration.save(&path)?;
                    Ok::<_, String>(configuration)
                })
                .await;
            this.update(cx, |view, cx| {
                view.saving = false;
                match result {
                    Ok(configuration) => {
                        view.workspace
                            .update(cx, |workspace, cx| workspace.configuration_saved(configuration, cx))
                            .ok();
                        view.saved = true;
                    }
                    Err(error) => view.error = Some(error),
                }
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }

    fn toggle(&self, id: &'static str, label: &'static str, enabled: bool, change: fn(&mut Settings), cx: &mut Context<Self>) -> impl IntoElement {
        row(
            label,
            div().flex().child(
                div()
                    .debug_selector(move || id.into())
                    .child(
                        Button::new(id, if enabled { "On" } else { "Off" })
                            .selected(enabled)
                            .on_click(cx.listener(move |view, _, _, cx| {
                                change(&mut view.fields.settings);
                                view.saved = false;
                                cx.notify();
                            })),
                    ),
            ),
        )
    }

    fn scope_tab(
        &self, id: &'static str, label: &'static str, selected: bool, cx: &mut Context<Self>,
        switch: fn(&mut SettingsView, &mut Window, &mut Context<SettingsView>),
    ) -> gpui::Div {
        let colors = colors();
        div()
            .h_full()
            .flex()
            .items_center()
            .px(spacing::small())
            .border_b_2()
            .border_color(if selected { colors.focus } else { gpui::transparent_black() })
            .when(selected, |tab| tab.bg(colors.background))
            .when(!selected, |tab| tab.hover(|style| style.bg(colors.hover)))
            .child(
                Button::new(id, label)
                    .quiet()
                    .selected(selected)
                    .disabled(self.saving)
                    .on_click(cx.listener(move |view, _, window, cx| switch(view, window, cx))),
            )
    }
}

fn row(label: &'static str, control: impl IntoElement) -> gpui::Div {
    div()
        .flex()
        .items_center()
        .gap(spacing::large())
        .py(spacing::small())
        .child(div().w(px(168.0)).flex_shrink_0().text_color(colors().muted_text).child(label))
        .child(div().flex_1().min_w(px(0.0)).child(control))
}

fn section(title: &'static str) -> gpui::Div {
    div().flex().flex_col().child(
        div()
            .pb(spacing::small())
            .text_size(typography::small_size())
            .line_height(typography::line_height())
            .text_color(colors().muted_text)
            .font_weight(gpui::FontWeight::SEMIBOLD)
            .child(title),
    )
}

fn choices(options: impl IntoIterator<Item = (&'static str, &'static str, bool, fn(&mut Settings))>, cx: &mut Context<SettingsView>) -> gpui::Div {
    div()
        .flex()
        .gap(spacing::small())
        .children(options.into_iter().map(|(id, label, selected, change)| {
            Button::new(id, label).selected(selected).on_click(cx.listener(move |view, _, _, cx| {
                change(&mut view.fields.settings);
                view.saved = false;
                cx.notify();
            }))
        }))
}

impl Render for SettingsView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let settings = &self.fields.settings;
        let indentation = settings.indentation.style;
        let save_policy = settings.build.save_before_build;
        let controls = div()
            .flex()
            .flex_col()
            .gap(spacing::large())
            .when(!self.project_scope, |view| {
                view.child(
                    section("Appearance").child(row(
                        "Theme",
                        div()
                            .flex()
                            .flex_wrap()
                            .gap(spacing::small())
                            .children(ligature_theme::THEMES.iter().map(|theme| {
                                div().debug_selector(move || theme.id.into()).child(
                                    Button::new(theme.id, theme.name)
                                        .selected(ligature_theme::resolve(self.configuration.theme.as_deref()).id == theme.id)
                                        .on_click(cx.listener(move |view, _, _, cx| {
                                            view.configuration.theme = Some(theme.id.to_owned());
                                            view.saved = false;
                                            cx.notify();
                                        })),
                                )
                            })),
                    )),
                )
            })
            .child(
                section("Editor")
                    .child(row("Font family", self.fields.family.clone()))
                    .child(hairline())
                    .child(row("Font size", self.fields.size.clone()))
                    .child(hairline())
                    .child(row(
                        "Indentation",
                        choices(
                            [
                                (
                                    "settings-spaces",
                                    "Spaces",
                                    indentation == Indentation::Spaces,
                                    (|s: &mut Settings| s.indentation.style = Indentation::Spaces) as fn(&mut Settings),
                                ),
                                (
                                    "settings-tabs",
                                    "Tabs",
                                    indentation == Indentation::Tabs,
                                    (|s: &mut Settings| s.indentation.style = Indentation::Tabs) as fn(&mut Settings),
                                ),
                            ],
                            cx,
                        ),
                    ))
                    .child(hairline())
                    .child(row("Width", self.fields.width.clone()))
                    .child(hairline())
                    .child(self.toggle(
                        "settings-format-on-save",
                        "Format on save",
                        settings.format_on_save,
                        |s| s.format_on_save = !s.format_on_save,
                        cx,
                    )),
            )
            .child(
                section("Build")
                    .child(row("Output folder", self.fields.output.clone()))
                    .child(div().pl(px(184.0)).child(caption("Relative to the project folder.")))
                    .child(hairline())
                    .child(row(
                        "Save before building",
                        choices(
                            [
                                (
                                    "settings-save-all",
                                    "Save all",
                                    save_policy == SaveBeforeBuild::SaveAll,
                                    (|s: &mut Settings| s.build.save_before_build = SaveBeforeBuild::SaveAll) as fn(&mut Settings),
                                ),
                                (
                                    "settings-clean",
                                    "Require saved",
                                    save_policy == SaveBeforeBuild::RequireClean,
                                    (|s: &mut Settings| s.build.save_before_build = SaveBeforeBuild::RequireClean) as fn(&mut Settings),
                                ),
                            ],
                            cx,
                        ),
                    ))
                    .child(hairline())
                    .child(row("Reruns", self.fields.reruns.clone()))
                    .child(hairline())
                    .child(self.toggle(
                        "settings-synctex",
                        "PDF source navigation",
                        settings.build.synctex,
                        |s| s.build.synctex = !s.build.synctex,
                        cx,
                    ))
                    .child(hairline())
                    .child(self.toggle(
                        "settings-logs",
                        "Keep build logs",
                        settings.build.keep_logs,
                        |s| s.build.keep_logs = !s.build.keep_logs,
                        cx,
                    ))
                    .child(hairline())
                    .child(self.toggle(
                        "settings-intermediates",
                        "Keep intermediate files",
                        settings.build.keep_intermediates,
                        |s| s.build.keep_intermediates = !s.build.keep_intermediates,
                        cx,
                    ))
                    .when(self.project_scope, |view| {
                        view.child(hairline()).child(row("Main document", self.fields.root.clone()))
                    }),
            )
            .child(
                section("Language")
                    .child(self.toggle(
                        "settings-server",
                        "Texlab language server",
                        settings.language_server.enabled,
                        |s| s.language_server.enabled = !s.language_server.enabled,
                        cx,
                    ))
                    .child(hairline())
                    .child(self.toggle(
                        "settings-completion",
                        "Server completion",
                        settings.language_server.completion,
                        |s| s.language_server.completion = !s.language_server.completion,
                        cx,
                    ))
                    .child(hairline())
                    .child(self.toggle(
                        "settings-hover",
                        "Hover information",
                        settings.language_server.hover,
                        |s| s.language_server.hover = !s.language_server.hover,
                        cx,
                    )),
            );
        div()
            .size_full()
            .flex()
            .flex_col()
            .key_context("LigatureControls LigatureSettings")
            .track_focus(&self.focus)
            .bg(window_background())
            .text_color(colors().text)
            .text_size(typography::control_size())
            .line_height(typography::line_height())
            .on_action(ligature_ui::focus_next)
            .on_action(ligature_ui::focus_previous)
            .on_action(cx.listener(|view, _: &Close, window, _| {
                if !view.saving {
                    window.remove_window();
                }
            }))
            .on_action(cx.listener(|_, _: &OpenSettings, window, _| window.activate()))
            .on_action(cx.listener(|view, _: &Save, _, cx| view.save(cx)))
            .child(
                chrome_strip()
                    .h(px(44.0))
                    .py(px(0.0))
                    .flex()
                    .items_stretch()
                    .gap(spacing::large())
                    .border_b_1()
                    .border_color(colors().border)
                    .child(div().flex().items_center().font_weight(gpui::FontWeight::SEMIBOLD).child("Settings"))
                    .child(div().flex_1().min_w(px(0.0)))
                    .child(
                        div()
                            .flex()
                            .child(
                                self.scope_tab("settings-application", "Application", !self.project_scope, cx, |view, window, cx| {
                                    view.switch_scope(false, window, cx)
                                }),
                            )
                            .when(self.project.is_some(), |view| {
                                view.child(self.scope_tab("settings-project", "Project", self.project_scope, cx, |view, window, cx| {
                                    view.switch_scope(true, window, cx)
                                }))
                            }),
                    ),
            )
            .child(
                div()
                    .id("settings-content")
                    .debug_selector(|| "settings-content".into())
                    .flex_1()
                    .min_h(px(0.0))
                    .overflow_y_scroll()
                    .px(spacing::large())
                    .py(spacing::large())
                    .flex()
                    .flex_col()
                    .gap(spacing::large())
                    .when(!self.project_scope, |view| {
                        view.child(caption("Defaults for all projects and untitled documents."))
                    })
                    .when(self.project_scope, |view| {
                        view.child(caption(
                            self.project.as_ref().expect("project settings have a project").display().to_string(),
                        ))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(spacing::medium())
                                .child(
                                    Button::new(
                                        "settings-inherit",
                                        if self.inherits() {
                                            "Customize project settings"
                                        } else {
                                            "Use application defaults"
                                        },
                                    )
                                    .disabled(self.saving)
                                    .on_click(cx.listener(|view, _, _, cx| view.toggle_project_settings(cx))),
                                )
                                .when(self.inherits(), |row| row.child(caption("This project uses application defaults."))),
                        )
                    })
                    .when(!self.inherits() && !self.saving, |view| view.child(controls))
                    .when(self.saving, |view| view.child(caption("Saving settings..."))),
            )
            .when_some(self.error.clone(), |view, error| view.child(inline_error(error)))
            .child(
                chrome_strip()
                    .id("settings-footer")
                    .debug_selector(|| "settings-footer".into())
                    .h(px(44.0))
                    .flex()
                    .items_center()
                    .gap(spacing::medium())
                    .border_t_1()
                    .border_color(colors().border)
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.0))
                            .when(self.saved, |view| view.child(caption("Settings saved."))),
                    )
                    .child(
                        Button::new("settings-cancel", "Close")
                            .disabled(self.saving)
                            .on_click(|_, window, _| window.remove_window()),
                    )
                    .child(
                        Button::new("settings-save", "Save")
                            .filled()
                            .disabled(self.saving)
                            .on_click(cx.listener(|view, _, _, cx| view.save(cx))),
                    ),
            )
    }
}
