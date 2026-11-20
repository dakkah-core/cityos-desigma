# desigma: independent local-first design editor in Rust

Direction: native Rust, faithful document rendering, Figma file compatibility and an independently designed editor. Preserve familiar workflows without targeting pixel-identical Figma editor chrome. The shadcn/ui Design System (Community) copy remains a candidate document-fidelity fixture, subject to its asset and redistribution licenses.

This is the original phased roadmap, updated to reflect the product direction. Many phases already have implementations; it is not a completion tracker or a description of the current stack. See the README and workspace manifests for the implementation.

## Stack
- `winit` + `wgpu` window and GPU surface
- `vello` (or `tiny-skia` fallback) for vector canvas rendering; `parley`/`cosmic-text` for text; `kurbo` for paths and booleans
- Chrome UI: the implementation uses `egui`; develop a coherent Desigma theme and controls based on usability and its own approved designs.
- Workspace crates: `desigma-doc` (model, undo), `desigma-render`, `desigma-ui`, `desigma-fig` (file formats), `desigma-app`, `desigma-diff` (pixel harness)

## Phase 0: document references and editor requirements
- Capture canvas-only output from permitted test documents at a fixed viewport, zoom and font configuration; record source and asset licenses before distributing fixtures.
- Define Desigma's own colors, fonts, spacing, radii and control states; use common workflow needs to guide information hierarchy.
- Keep document-fidelity cases separate from editor regression cases. The shadcn file pages (Cover, Components, Typography, Colors, Primitives, Icons) can exercise import and rendering without making Figma's toolbar or panels the target.

## Phase 1: tokens, specs, spikes
- Per-component spec for Desigma's controls: sizes, interaction states, keyboard navigation and original or appropriately licensed icons.
- Validate the editor shell's usability and canvas integration within the existing egui and wgpu architecture.

## Phase 2: core
- Scene graph (frame, group, rect, ellipse, line, polygon, star, vector, text, image, component, instance), fills/strokes/effects, constraints, auto-layout, variables/styles
- Undo/redo, selection, hit-testing, snapping/guides, pan/zoom

## Phase 3: editor
1. Independent shell and chrome, with approved Desigma screenshots as regression baselines
2. Canvas, selection handles, shape tools
3. Layers tree, properties panel, text editing
4. Pen/vector edit, boolean ops, auto-layout, components/variants
5. Native `.desigma` save/load
6. Motion (beta): per-page timeline, keyframes with curve and spring easing, 41 presets and clips, Present playback, prototype "Control animation" actions, GIF/WebM/PNG export, CSS/SwiftUI/Compose animation code, MCP motion tools (done)

## Figma file support (`desigma-fig`)
Two import paths, so we are not blocked by the proprietary format:
1. **REST/JSON import**: Figma REST API `GET /v1/files/:key` returns a documented node tree; map to our model. Needs a user-supplied personal access token (never stored in the repo). Images via `/v1/images`.
2. **`.fig` binary import**: `.fig` is a "fig-kiwi" container (header, then deflate-compressed Kiwi-encoded schema and message, often inside a zip with `meta.json`/thumbnail/images). Implement a Kiwi schema decoder in Rust, decode the node-changes message into the same intermediate model. The format is undocumented and community reverse-engineered, so expect version drift; test against the shadcn file exported from Figma (File > Save local copy).
3. **Export**: write `.fig`-compatible output last (stretch goal); the REST JSON shape plus clipboard HTML are fallbacks.
- Fidelity corpus: permitted test documents including the shadcn file. Import them, render, and pixel-diff document content against reference output at matching viewport and font settings.

## Phase 4: pixel verification
- Render each state headlessly with the app, then compare using `desigma-diff` (SSIM plus per-pixel threshold). Set explicit tolerances per fixture and integrate approved baselines into regression checks.
- Document references measure geometry, typography, colors and effects. Record expected anti-aliasing differences without masking layout or text errors.
- Editor references are approved Desigma designs. Figma editor screenshots are not a pixel-match target.

## Risks
- `.fig` format changes without notice; keep decoder tolerant, ignore unknown fields.
- Full MVP in Rust is large; prioritize renderer reliability, an independent editor shell, import fidelity, editing and export.
- Review licenses and attribution for code, fonts, icons and test documents; a Community listing alone does not establish redistribution rights. Obtain appropriate legal review before public release; independent implementation is not a legal clearance claim.
