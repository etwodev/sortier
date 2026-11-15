# Sortier

A desktop app for organising, tagging, searching and reading **sequenced images**: multi-page art,
manga chapters, pixiv-style `_p0/_p1` posts, comics, webtoon strips, plus GIFs and videos.
GPUI draws the interface; wgpu draws the pictures. The layout follows Spline's: a title bar, the
menus and workspace tabs, and Blender-style areas you can resize, split and join (right-click a seam).

## Run

```bash
cargo run --release -p sortier-app
```

Pass folders to add them (`cargo run --release -p sortier-app -- "D:\Art"`), use File › Add Folder,
or drop folders on the window. Data lives in `%LOCALAPPDATA%\sortier` (`library.json`,
`workspaces.json`, thumbnails); set `SORTIER_DATA_DIR` to use another folder. Files on disk are never
moved or changed.

## Editors and workspaces

| Editor | What it does |
|---|---|
| Library | The results as tiles. Search, tag the selection, sort and tile options. A/D/W/S or arrows move, Enter opens. |
| Viewer | The open page drawn by wgpu: fit, fit width/height, 1:1, zoom and pan, two-page spread, right to left, animated GIF/PNG/WebP, video with sound. A/D turn pages and carry on into the next item; W/S jump items. |
| Pages | The open sequence's pages; drag to reorder, make a cover, take a page out. |
| Browser | Quick filters, folders, and tags grouped by namespace. Click a tag to require it, Alt-click to exclude it. |
| Inspector | The selection's file details, sequence name, tags, and the open page's own tags. |

Workspaces **Library**, **Reader** and **Tagging** start set up; add, rename or rearrange them from the tabs.
Help › Keyboard Shortcuts (F1) lists every key.

Search: `cat dog`, `-sketch`, `cat|dog`, `artist:*`, `type:video|gif|image`, `is:untagged`, `is:seq`,
`name:…`, `seq:…`, `ext:…`.

Videos: H.264 MP4/MOV play without anything extra. Other formats need ffmpeg, found on `PATH` or
downloaded by File › Download ffmpeg. With ffmpeg, videos decode on the graphics card when it can (NVIDIA
NVDEC, Direct3D 11 or DXVA 2 on Windows, VideoToolbox on macOS, VA-API on Linux); codecs the card can't
decode fall back to the processor. The badge in the video controls says which is in use, and View › Decode
Video on the GPU turns it off.

## Crates

| Crate | Purpose |
|---|---|
| `sortier-app` | The window shell, area host, workspace tabs, menus, the shared `LibraryModel`, and the editors. |
| `sortier-ui` | Design tokens, theme and UI primitives, ported from Spline's `spline-ui` (the only crate using gpui-component). |
| `sortier-areas` | The area layout tree (Spline's `spline-areas`). |
| `sortier-library` | Items, sequences, tags, search, scanning and the thumbnail cache. No UI. |
| `sortier-render` | The wgpu renderer: textures with mipmaps (tiled past the GPU's size limit), views read back as BGRA frames. No UI. |
| `sortier-media` | Decoding stills, animations and video (OpenH264, ffmpeg) and playing sound (symphonia, cpal). No UI. |

```bash
cargo test --workspace      # sortier-render's tests need a GPU
cargo clippy --workspace --all-targets
```
