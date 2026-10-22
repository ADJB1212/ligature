# Ligature

A native LaTeX editor for macOS, built in Rust with GPUI CE. Write, typeset, and navigate your documents in one workspace, with local compilation and a synchronized PDF preview.

[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)

Ligature bundles Tectonic, TeX resources, New Computer Modern fonts, and the Texlab language server. Supported documents compile offline without a separate TeX installation. Additional packages are available through an explicit full-bundle download.

**Status:** Early development. The current build and packaging setup targets Apple Silicon macOS.

## Features

- **Native editing:** Unicode-aware navigation, syntax highlighting, undo and redo, tabs, resizable editor splits, and IME support.
- **Project navigation:** File browsing, automatic main-document discovery, document outlines, and definition navigation for references and citations.
- **Local typesetting:** Bundled Tectonic builds, cancellation, navigable diagnostics, and build output. Self-contained untitled documents can render without saving first.
- **PDF preview:** Continuous scrolling, zoom and fit modes, PDF export, and bidirectional source navigation through SyncTeX.
- **Language assistance:** Bundled Texlab completion, hover information, and diagnostics, with local completion available for untitled documents.
- **Search and formatting:** Document and project search and replace, replacement previews, a built-in TeX formatter, and optional formatting on save.
- **Document recovery:** Atomic saves, external-change detection, session restoration, and recovery snapshots for unsaved work.
- **Appearance:** Four built-in themes: Press Bed, Midnight, Daylight, and Lucidplasma.

## Build from source

### Requirements

- An Apple Silicon Mac and a native macOS build environment.
- Xcode with its command-line tools selected; app packaging uses `xcrun actool` to compile the application icon.
- Rust **1.99 or newer**, managed through rustup. The repository selects the stable toolchain and includes rustfmt and Clippy.
- Homebrew and the native dependencies listed below.
- Network access for the initial Cargo, Tectonic, and Texlab dependency downloads.

### Compile and package

```sh
git clone https://github.com/ADJB1212/ligature.git
cd ligature

brew install llvm lld pkgconf freetype graphite2 icu4c libpng
cargo build --release
```

The packaging script creates **`target/Ligature.app`**, including the engine, language server, native libraries, TeX resources, fonts, and license notices. Open it in Finder to launch Ligature. The app is ad-hoc signed for local use; distribution signing and notarization are separate steps.

The first build also compiles Tectonic 0.17.0 and Texlab 5.26.0 in separate caches, so it can take substantially longer than subsequent builds. Build scripts stage the required runtime resources automatically. Offline source builds require cached dependencies for both the workspace and the bundled tools.

## First document

1. Create a document with **Cmd+N**, open a `.tex` file with **Cmd+O**, or open a project folder with **Cmd+Shift+O**.
2. Enter a complete LaTeX document, such as:

   ```tex
   \documentclass{article}
   \begin{document}
   Hello from Ligature.
   \end{document}
   ```

3. Press **Cmd+B** to build. A successful build loads the PDF preview; diagnostics and raw output are available in the build output pane.
4. Use **Cmd+Shift+J** to locate the source caret in the PDF, or **Cmd-click** the PDF to return to its source.
5. Export the displayed PDF through **File > Export PDF**.

Untitled builds support self-contained documents and bundled packages. Save your document into a project folder before using relative includes, bibliography files, or other project assets. Saved project builds save modified project documents before compilation by default.

### TeX packages

The included resource bundle provides common TeX packages. For additional packages, open **Ligature > TeX Resources** and choose **Download full TeX bundle**. This is a large, optional download. Normal builds use local resources with automatic downloads and shell escape disabled.

### Settings

Open **Ligature > Settings** with **Cmd+,** to adjust appearance, editor behavior, formatting, and language assistance. Application defaults and project overrides are stored in `settings.json` under `$XDG_CONFIG_HOME/ligature` when `XDG_CONFIG_HOME` is absolute, or `$HOME/.config/ligature` otherwise. Project-local `.ligature.json` files are ignored.

## Keyboard shortcuts

| Action               | Shortcut             |
| -------------------- | -------------------- |
| New document         | Cmd+N                |
| Open file            | Cmd+O                |
| Open project folder  | Cmd+Shift+O          |
| Save / Save As       | Cmd+S / Cmd+Shift+S  |
| Build / Cancel build | Cmd+B / Cmd+Shift+B  |
| Toggle build output  | Cmd+J                |
| Find / Replace       | Cmd+F / Cmd+Option+F |
| Find in project      | Cmd+Shift+F          |
| Format document      | Cmd+Shift+I          |
| Go to definition     | F12                  |
| Show source in PDF   | Cmd+Shift+J          |
| Export PDF           | Cmd+Shift+E          |
| Settings             | Cmd+,                |

## Development

Ligature is a Rust 2024 Cargo workspace. The executable lives in `src/`; focused libraries under `crates/` separate editing, project models, language assistance, builds, and preview rendering. GPUI views are composed in `workspace`, while text, filesystem, TeX, and build models remain independent of the UI.

```sh
cargo fmt --all --check
cargo check --workspace --all-targets --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
```

The root executable is the default workspace member. Use `--workspace` for all packages, or `cargo test -p ligature-<area> --locked` for a specific library.

| Path                          | Purpose                                    |
| ----------------------------- | ------------------------------------------ |
| `src/`                        | Application entry point                    |
| `crates/`                     | Editor, UI, and feature libraries          |
| `crates/toolchain/resources/` | Bundled TeX archive and resource manifests |
| `tools/package-resources/`    | TeX resource regeneration utility          |

## Contributing

Bug reports and focused pull requests are welcome. Include reproduction steps and relevant build output when reporting an issue. For code changes, add tests for meaningful behavior, run the workspace checks above, and describe any manual UI verification. Use Conventional Commit-style subjects such as `fix:`, `feat:`, or `docs:`.

## License

Ligature's source code is licensed under the [MIT License](LICENSE). Bundled tools, TeX packages, fonts, and other dependencies retain their respective licenses. In particular, Texlab is GPL-3.0; the app bundle includes its license, notice, and corresponding source archive.
