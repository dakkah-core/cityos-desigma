<h1 align="center">
  <img alt="desigma: the design tool you actually own. Free, open source, offline, written in Rust." src="marketing/png/launch-banner-1600x900.png" width="100%">
</h1>

<p align="center">
  <b>Interface design; an independent, local-first, open-source design tool built in Rust.</b><br>
  Frames, auto layout, components and variants, prototyping, vector editing and real Figma file import,<br>
  in a native desktop app written entirely in Rust. Free, offline, and yours to change.
</p>

<p align="center">
  <img alt="100% Rust" src="https://img.shields.io/badge/100%25-Rust-b7410e?style=flat-square&logo=rust">
  <img alt="Windows · Linux" src="https://img.shields.io/badge/Windows%20%C2%B7%20Linux-native-2f7bf5?style=flat-square">
  <img alt="License: MIT OR Apache-2.0" src="https://img.shields.io/badge/license-MIT%20%2F%20Apache--2.0-3a3a3a?style=flat-square">
  <img alt="Status: early alpha" src="https://img.shields.io/badge/status-early%20alpha-d69e2e?style=flat-square">
  <a href="https://github.com/deepfounder-ai/desigma/actions/workflows/ci.yml"><img alt="CI" src="https://img.shields.io/github/actions/workflow/status/deepfounder-ai/desigma/ci.yml?branch=main&style=flat-square&label=CI"></a>
</p>

<br>

<p align="center">
  <img src="docs/images/desigma-hero.png" alt="The desigma editor on the Marlow showcase file: four phone screens in a row (welcome, home dashboard, spending and a dark settings screen), the Home frame selected and the inspector showing its vertical auto layout" width="900">
  <br>
  <sub>The editor on the built-in showcase file: four 390 x 844 screens built with auto layout and components, one of them in the Dark mode of a variable collection.</sub>
</p>

<br>

<table>
  <tr>
    <td width="25%" valign="top">
      <h3>🎛️ Familiar by design</h3>
      Familiar design workflows and shortcuts, from <kbd>F</kbd> for Frame to <kbd>Shift</kbd>+<kbd>A</kbd> for auto layout. Desigma is developing its own editor interface, with document fidelity and a fast editing experience as the priorities.
    </td>
    <td width="25%" valign="top">
      <h3>⚡ Native and fast</h3>
      A deterministic CPU renderer on tiny-skia, a spatial index and a parallel 256&nbsp;px tile cache. Panning and dragging on a 5,000-layer page stay well under a frame. No Electron, no web view.
    </td>
    <td width="25%" valign="top">
      <h3>🗂️ Figma files in</h3>
      Import from the Figma REST API JSON or straight from <code>.fig</code> files (the kiwi container, deflate or zstd), with components, instance overrides, styles and prototype links.
    </td>
    <td width="25%" valign="top">
      <h3>🔤 Open fonts, built in</h3>
      Inter ships inside the binary. Any other open-licensed family a file uses is fetched from Fontsource and cached, and proprietary fonts map to metric-compatible open alternatives.
    </td>
  </tr>
</table>

<br>

## Features

### Desigma Decks

Create a presentation with **File → New deck**. Decks use editable 16:9 frames with slide thumbnails, add/duplicate/delete controls, ordering, overview, and speaker notes. Switch to **Layers** for the existing text, shape, image, and vector editing tools. Notes save with **Save notes** or when navigating to another slide. Slide edits and saved notes support Undo/Redo and persist in the document.

**Present** starts at the active slide. Use Right/Space/Page Down and Left/Page Up, or Previous/Next; Home/End jump to the first/last slide. Click **Fullscreen** or press **F11** for a borderless presentation with the controls hidden; **Esc** returns to preview, then exits. Launch directly with `--fixture deck --fullscreen`. This initial version does not yet include transitions, a template gallery, or a separate presenter display. Preview the sample with `cargo run -p desigma-app -- --fixture deck`.

Export the current slide or the whole deck using the **Slide PDF** / **Deck PDF** controls or File menu. Each slide becomes one PDF page in document order, with no speaker notes or editor controls. The initial PDF export uses high-resolution raster pages; text is not selectable.

Every screenshot here is the real app on its built-in showcase file, captured offscreen at 2x with its own <code>--screenshot</code> mode.

<table>
  <tr>
    <td width="50%" valign="top">
      <img src="docs/images/desigma-components.png" alt="A Button instance selected on the welcome screen: the inspector shows the Button component set with Type and Size variant pickers, a Label text property, a Show icon toggle and a Changed row with a Reset button" width="440">
      <br>
      <sub>A Button instance: variant pickers (Type, Size), a text property, a boolean property and the overrides on this copy.</sub>
      <h3>Components that stay in sync</h3>
      Turn any layer into a component with <kbd>Ctrl</kbd>+<kbd>Alt</kbd>+<kbd>K</kbd>, drop instances from the Assets panel, and edit the main component: every instance updates in the same undo step while keeping its own overrides.
      <br><br>
      <b>Boolean, text, variant and instance-swap properties</b>, Combine as variants, Go to main component, Reset all changes and Detach, all undoable.
    </td>
    <td width="50%" valign="top">
      <img src="docs/images/desigma-prototype.png" alt="The Prototype tab on the Mobile page: the Welcome frame selected with two blue connectors leading to the Home screen, flag chips marking the Onboarding and Dashboard flows above their start frames" width="440">
      <br>
      <sub>Prototype mode: flow starting points, and connectors from the selected screen to where its buttons lead.</sub>
      <h3>Prototype what you draw</h3>
      Drag from a layer's hotspot to a frame to wire it up, then refine the interaction: triggers, actions, overlays and transitions with easing curves and springs.
      <br><br>
      <b>Present</b> plays it back with Navigate, Back, overlays, Change to (variants), Scroll to and Smart Animate, which pairs layers by name and interpolates them. **Variables drive it**: Set variable (with a small safe expression language), Set variable mode and Conditional (If / Else if / Else) re-render bound text, colors and visibility live, without ever changing the file. Key, gamepad and drag triggers, interactive components (hover and press states with automatic revert) and video fills (Motion JPEG plays in the pure-Rust build, other codecs show their poster; `cargo build -p desigma-app --features video` adds H.264 through OpenH264, which compiles C++) are included.
    </td>
  </tr>
  <tr>
    <td width="50%" valign="top">
      <img src="docs/images/desigma-variables.png" alt="The Variables view on the Theme collection: Light and Dark mode columns, color variables grouped as Surface, Text, Border, Brand and Status with aliases to primitives, and number variables for radius" width="440">
      <br>
      <sub>The Variables view: a Theme collection with Light and Dark modes, aliases to primitives and number variables.</sub>
      <h3>Variables and modes</h3>
      Color, number, string and boolean variables in collections with modes, bound to fills, strokes, sizes and spacing from a picker next to the field. Switch a frame to another mode and everything bound to it follows.
      <br><br>
      <b>Library variables</b> and color styles appear in the same pickers; using one copies it into the file in the same undo step.
    </td>
    <td width="50%" valign="top">
      <img src="docs/images/desigma-dev.png" alt="Dev mode with a balance card selected: a Ready for dev status, an annotation and a dev link, the box model with padding and margin, and SwiftUI code for the card" width="440">
      <br>
      <sub>Dev mode: status, annotations, measurements and SwiftUI for the selected card.</sub>
      <h3>Hand off as code</h3>
      Switch to Dev mode for measurements, a box model and generated code in CSS, Tailwind, SwiftUI, Jetpack Compose or React. Mark frames Ready for dev, and add annotations and links for whoever builds them.
      <br><br>
      <b>Copy</b> a snippet in one click, or switch language without leaving the panel.
    </td>
  </tr>
  <tr>
    <td width="50%" valign="top">
      <img src="docs/images/desigma-web.png" alt="A 1440 pixel wide web dashboard frame: a sidebar with navigation, four KPI cards with sparklines, a cash flow line chart, a donut chart and a transactions table with status tags" width="440">
      <br>
      <sub>A 1440 px dashboard: sidebar, KPI cards, charts and a table, all auto layout and component instances.</sub>
      <h3>Real layouts, not just rectangles</h3>
      Nested auto layout frames with hug, fill and fixed sizing, padding, gaps and alignment keep a whole dashboard in shape when the copy changes. Charts are plain vectors, rectangles and arcs.
      <br><br>
      <b>Tag, Avatar and Button instances</b> in the table and the header come from the same component sets as the mobile screens.
    </td>
    <td width="50%" valign="top">
      <img src="docs/images/desigma-gradient.png" alt="The gradient picker open on a balance card: gradient stops on a bar, a color field, hue and opacity sliders, a hex input and the colors used on the page" width="440">
      <br>
      <sub>The color picker on a gradient fill, with the page's colors underneath.</sub>
      <h3>Fills, gradients and effects</h3>
      Solid, linear, radial, angular and diamond gradients, images and blend modes on every fill, with stops you drag on the layer itself. Shared color styles and variables show up in the same picker.
      <br><br>
      <b>Effects</b> include drop and inner shadows, layer and background blur, noise, texture and glass.
    </td>
  </tr>
  <tr>
    <td width="50%" valign="top">
      <img src="docs/images/desigma-home.png" alt="The projects home screen: a sidebar with Recents, Starred and a Marlow project, and a grid of four file cards with thumbnails of a whiteboard, a design system, a web dashboard and a mobile app" width="440">
      <br>
      <sub>The projects home screen: a Marlow project with a board, a design system, a dashboard and an app.</sub>
      <h3>Your files, in one place</h3>
      A local projects library with Recents, Starred, projects and folders, search, grid and list views, thumbnails, trash and drag and drop. Files autosave as you work, and "Open in new window" starts another editor on the same library.
      <br><br>
      <b>Shared libraries</b> publish a file's components, styles and variables for other files to enable, with a review step when a new version lands.
    </td>
    <td width="50%" valign="top">
      <img src="docs/images/desigma-board.png" alt="A board on a dotted canvas: a section holding three pastel sticky notes, a diamond and a rounded shape joined by elbow connectors with a yes label, and loose stamps" width="440">
      <br>
      <sub>A board: stickies, connectors, shapes with text, stamps and a section.</sub>
      <h3>Boards for thinking out loud</h3>
      New board in the File menu or on the home screen opens a whiteboard with its own toolbar: stickies whose text shrinks to fit, shapes with text, markers, stamps and sections. <kbd>Tab</kbd> starts the next sticky, <kbd>Alt</kbd>-drag duplicates.
      <br><br>
      <b>Connectors</b> come out of the magnet dots on any shape, route around the boxes they join and follow them when they move. FigJam files open as boards.
    </td>
  </tr>
  <tr>
    <td width="50%" valign="top">
      <img src="docs/images/desigma-renderer.png" alt="A renderer test sheet: rounded rectangles with independent radii, arcs, a star, a vector path, linear, radial, angular and diamond gradients, image fills, inside, center and outside strokes, dashes, drop and inner shadows, layer and background blur, masks, boolean operations, corner smoothing and styled text" width="440">
      <br>
      <sub>The renderer's feature sheet, produced by <code>cargo run -p desigma-render --example render_demo</code>.</sub>
      <h3>A renderer that sweats the pixels</h3>
      Independent corner radii and iOS-style corner smoothing, every gradient type, image fills, inside, center and outside strokes, drop and inner shadows, layer and background blur, masks and boolean operations.
      <br><br>
      <b>All 19 blend modes</b>, including exact linear burn and linear dodge, and text shaped with cosmic-text, with runs, line height, letter spacing, case and truncation.
    </td>
    <td width="50%" valign="top">
      <img src="docs/images/desigma-palette.png" alt="The Actions command palette open over the showcase file, with a search field and a list of commands and their keyboard shortcuts" width="440">
      <br>
      <sub>Actions (<kbd>Ctrl</kbd>+<kbd>K</kbd>): every command, searchable, with its shortcut.</sub>
      <h3>Everything a keystroke away</h3>
      The main menu, tool dropdowns, context menus and the command palette all share one consistent menu widget, with full keyboard navigation and submenus.
      <br><br>
      <b>Export</b> to PNG, JPG and real vector SVG at any scale, copy as PNG, SVG or CSS, and switch to Dev mode for measurements and ready-to-paste CSS.
    </td>
  </tr>
</table>

<br>

## Everything in the box

<table>
  <tr>
    <td width="33%" valign="top">
      <h4>🧰 Tools</h4>
      Move · Hand · Frame · Section · Slice · Rectangle · Line · Arrow · Ellipse · Polygon · Star · Image · Pen · Pencil · Text, with snapping, smart guides and distance labels, multi-select, rotate, alt-drag duplicate and vector networks: point editing with bend, paint bucket, marquee and lasso selection, per-vertex corner radius, caps and handle mirroring, plus the shape builder, outline stroke, flatten, simplify, join and split.
    </td>
    <td width="33%" valign="top">
      <h4>📐 Layout</h4>
      Auto layout with direction, wrap, gap, padding, alignment and hug, fill or fixed sizing, plus drag to reorder. Constraints, groups, clipping frames, and pages you can add, duplicate, reorder and delete.
    </td>
    <td width="33%" valign="top">
      <h4>🎨 Styles and color</h4>
      Shared color, text and effect styles with names, folders and keys, applied in one click. A Selection colors section that edits every use of a color at once.
    </td>
  </tr>
  <tr>
    <td width="33%" valign="top">
      <h4>🗂️ Formats</h4>
      In: <code>.fig</code>, Figma REST JSON and native <code>.desigma</code>. Out: <code>.desigma</code>, Figma REST JSON, PNG, JPG and SVG. Images in PNG, JPEG and WebP.
    </td>
    <td width="33%" valign="top">
      <h4>🔤 Type</h4>
      Bundled Inter variable font, per-run styles, line height in pixels or percent, letter and paragraph spacing, case transforms, decorations, auto-resize and truncation with an ellipsis.
    </td>
    <td width="33%" valign="top">
      <h4>🪄 The everyday essentials</h4>
      Undo and redo for every edit, copy and paste, group and ungroup, align and distribute, z-order, flip and rotate 90°, and zoom to fit or 100%.
    </td>
  </tr>
</table>

<br>

## Figma files

desigma reads two kinds of Figma input, and both land in the same document model:

- **REST JSON**, the body of `GET /v1/files/:key`. Save it to a file and open it, or let the optional `net` feature of `desigma-fig` fetch it with your personal access token. The token is only ever sent as a request header and is never logged or stored.
- **`.fig` files**, from *File → Save local copy*. desigma unpacks the zip wrapper, decodes the kiwi schema and message, and maps node changes, embedded images, shared styles, instance overrides, layout grids and prototype interactions.

The `.fig` format is undocumented. Our decoder follows community reverse engineering and is tested against synthetic files, so expect gaps on real-world files and please report them.

## Under the hood

- **One document model:** `desigma-doc` is plain data plus an undo stack. Every edit is a `Command` with an exact inverse.
- **Deterministic rendering:** `desigma-render` draws on the CPU with tiny-skia, so output is reproducible byte for byte. Tiles stitched together match a whole-page render.
- **Visual regression tooling:** `desigma-diff` compares screenshots with anti-aliasing detection and SSIM, and writes an HTML report. The development target is faithful document rendering and an independently designed editor, with its own approved UI baselines.
- **Independent product direction:** file compatibility and familiar workflows guide development; reproducing Figma's editor appearance is not a goal. Third-party code, fonts and assets retain their own licenses and attribution requirements.
- **Tested:** more than 500 tests across the workspace, from undo and import round trips to UI tests that drive the editor with synthetic input.

| Crate | What it does |
|---|---|
| `desigma-doc` | Scene graph, paints, text, auto layout, components, prototype model, undo/redo |
| `desigma-render` | tiny-skia renderer, spatial index, parallel tile cache |
| `desigma-fig` | Figma REST JSON and `.fig` import, REST JSON export |
| `desigma-fonts` | Bundled Inter, Fontsource fetch and cache, font substitution |
| `desigma-export` | PNG, JPG and SVG export, Dev mode CSS |
| `desigma-app` | The egui desktop editor |
| `desigma-diff` | Pixel-diff library and CLI |
| `desigma-mcp` | Local MCP server for design-file inspection, editing, and code generation |

## Get started

### Download

Grab the build for your platform from the [latest release](https://github.com/deepfounder-ai/desigma/releases): a `.dmg` for macOS (one file for Apple Silicon and Intel), a `.zip` for Windows and a `.tar.gz` with an installer script for Linux.

> [!NOTE]
> Release builds are **not code-signed yet**, so the first launch shows a warning. On **Windows**, click **More info → Run anyway** in the SmartScreen dialog. On **macOS**, open the `.dmg`, drag desigma to Applications, open it once, then click **Open Anyway** in **System Settings → Privacy & Security**, or run `xattr -dr com.apple.quarantine /Applications/desigma.app`. Step-by-step instructions: [docs/install.md](docs/install.md).

### Build from source

```sh
git clone https://github.com/deepfounder-ai/desigma
cd desigma
cargo run --release -p desigma-app               # the editor, with a demo document
cargo run --release -p desigma-app -- file.fig   # open a Figma file
cargo test --workspace                           # the test suite
```

On Linux you need the usual windowing headers: `pkg-config libxkbcommon-dev libwayland-dev libx11-dev libxcursor-dev libxrandr-dev libxi-dev libgl1-mesa-dev libfontconfig1-dev`. macOS is built and tested in CI.

Want to contribute or cut a release? See [CONTRIBUTING.md](CONTRIBUTING.md) and [docs/releasing.md](docs/releasing.md).

Useful flags for automation and testing:

```sh
desigma --screenshot out.png --size 1600x1000 --select Card   # render one frame and exit
desigma --fixture showcase --select Home --size 1600x1000 --scale 2 --screenshot hero.png   # the showcase file, rendered at 2x (3200x2000)
desigma --select Welcome --rulers --layout-grid columns --zoom 120 --screenshot out.png   # rulers + column grid
desigma --edit-text "Welcome back" --select-text 0-7 --screenshot out.png                 # mid-edit with a selection
desigma --tab prototype --present                              # open straight into Present
desigma --fixture proto-vars --select Add --tab prototype --panel interaction --screenshot i.png   # the interaction editor with a Conditional
desigma --fixture proto-vars --present --panel present-variables --screenshot p.png                # Present with the live Variables panel
desigma --seed-library ./my-library                            # fill a library directory with the showcase sample files
desigma --home --library ./my-library --screenshot home.png    # the projects home screen on a library directory
desigma --fixture variables --panel variables --screenshot v.png  # the Variables view
desigma-diff compare reference.png actual.png --out diff.png   # pixel-compare two images
```

> [!IMPORTANT]
> **Status:** desigma is in early alpha. Much of Figma's everyday design surface exists, but **it is not yet a replacement for professional daily work**. The biggest gaps are collaboration, plugins, PDF export, `.fig` features that real files haven't exercised yet, and code-signed release builds. Expect rough edges, and please file issues with your OS, a sample file if you can share one, and a screenshot.

## MCP integration

Build the local server with `cargo build --locked -p desigma-mcp`, then configure your MCP client to launch `desigma-mcp --workspace /path/to/designs`. It provides 13 tools for importing files, inspecting layers and tokens, generating code, editing with undo/redo, and saving a new `.desigma` file. The server runs an isolated file session; live desktop-editor control is not yet connected. See the [setup and tool reference](crates/desigma-mcp/README.md).

## Native plugin installation

Open **Plugins** from the editor rail or main menu, choose **Install plugin**, then **Install Icon Browser**. This included plugin provides 12 Lucide icons offline and requires no account or token. Choose **Open** in the plugin library to use it in your browser, then select an icon to insert editable vectors into the open document.

To install another native plugin from disk, choose **Choose manifest…** and select `manifest.json` from an extracted bundle containing its declared assets and sibling `LICENSE`. Installation copies the validated bundle into the current library; it does not execute the plugin. You can move the original folder afterward.

For the optional private [desigma-plugins](https://github.com/deepfounder-ai/desigma-plugins) registry, paste a manifest URL and choose **Install URL**. GitHub `blob` and raw URLs are supported, for example:

```text
https://github.com/deepfounder-ai/desigma-plugins/blob/main/plugins/icon-browser/manifest.json
```

The registry is private. Before launching the editor, set `DESIGMA_PLUGINS_TOKEN` to a GitHub token with read access to that repository's contents. The installer uses the GitHub contents API, does not discover credentials automatically, and never puts credentials in URLs or installed metadata. Prefer a commit SHA in place of `main` for a fixed package revision.

Schema 2 plugins include a root HTML panel and declared HTML/CSS/JavaScript assets. Choose **Open** on an installed plugin to launch its panel in your browser. Icon Browser can insert static SVG into the current document as editable layers; one Undo removes each insertion. **Stop**, switching documents, or returning to the library revokes the session. Schema 1 packages remain installable but do not run.

The browser panel is served from a temporary loopback address. Only `document:insert-svg` and `storage:local` permissions are supported, with optional access to `https://api.iconify.design`. The bridge checks the session token and request origin; plugins cannot read the document or access GitHub credentials. Browser JavaScript retains normal browser capabilities; this is a scoped document bridge, not a general-purpose JavaScript sandbox. Favorites in a launched panel last for that browser session because each launch uses a fresh origin.

Schema 2 adds `panel`, `files` (including the main script and panel), and `network` to the native manifest. Bundles require a sibling `LICENSE`. Downloads are limited to 16 assets, 2 MiB per file and 8 MiB total, with redirects disabled. Extra paths, unsupported permissions, Figma Community URLs and other repositories are rejected. SVG insertion accepts static vectors only and is blocked during presentation, version preview, and active editing gestures.

## Spread the word

Logos, social images, banners and copy are in [`marketing/`](marketing/).

## License

Licensed under either of [MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE), at your option. Unless you state otherwise, any contribution you submit for inclusion is dual-licensed the same way, without additional terms.

Bundled Inter is © The Inter Project Authors, licensed under the SIL Open Font License 1.1 (see `crates/desigma-fonts/assets/inter/OFL.txt`).

Editor icons use Lucide under its ISC license (see `crates/desigma-app/assets/icons/lucide/LICENSE`). Release packages include both asset notices in `licenses/` (inside `desigma.app/Contents/Resources/` on macOS).

desigma is an independent project. It is not affiliated with, endorsed by or sponsored by Figma, Inc. Figma is a trademark of Figma, Inc., used here only to describe file compatibility.
