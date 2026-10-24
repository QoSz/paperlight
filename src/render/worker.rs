//! The `pdf-worker` thread: owns the PDF library, opens, renders and searches documents.

use std::{
    collections::{HashSet, VecDeque},
    error::Error,
    path::{Path, PathBuf},
    sync::mpsc::{Receiver, Sender, channel},
    thread,
    time::Instant,
};

use egui::{Color32, ColorImage, Rect, pos2};
use pdfium_render::prelude::{
    PdfBitmap, PdfBitmapFormat, PdfDocument, PdfPage, PdfRect, PdfRenderConfig, PdfSearchDirection,
    PdfSearchOptions, Pdfium, PdfiumError, PdfiumInternalError,
};
use tracing::{debug, warn};

use super::pdfium_loader::LIB_NAME;
use crate::{
    layout::{PageSize, Rotation},
    search::Hit,
    selection::CharBox,
};

/// UI → worker.
#[derive(Debug, Clone, PartialEq)]
pub enum Cmd {
    /// Replace the open document. `password` is needed only for an encrypted file.
    Open {
        path: PathBuf,
        password: Option<String>,
    },
    /// Render `page` at `scale_px` px per pt (ppp already included). Older `generation`s are dropped.
    /// `zoomed`: the view is zoomed in past fit width (see `pixel_capacity`).
    Render {
        page: usize,
        scale_px: f32,
        generation: u64,
        zoomed: bool,
    },
    /// Find `query` (case-insensitive) in all pages. "" cancels and replies with no hits.
    Search { query: String, generation: u64 },
    /// Extract the chars and boxes of `page` (for text selection).
    Text { page: usize },
}

/// Worker → UI. Each send is followed by `ctx.request_repaint()`.
pub enum Msg {
    Opened {
        name: String,
        sizes: Vec<PageSize>,
    },
    OpenFailed(String),
    /// `path` is encrypted; `wrong` if a password was given and rejected.
    NeedsPassword {
        path: PathBuf,
        wrong: bool,
    },
    /// No libpdfium could be bound. Sent at startup and again for every later `Open`.
    LibraryMissing(String),
    Page {
        page: usize,
        scale_px: f32,
        image: ColorImage,
    },
    RenderFailed {
        page: usize,
        scale_px: f32,
    },
    SearchDone {
        generation: u64,
        hits: Vec<Hit>,
    },
    /// Chars of `page`; empty if extraction failed.
    Text {
        page: usize,
        chars: Vec<CharBox>,
    },
}

/// Starts the `pdf-worker` thread. If the spawn fails, `LibraryMissing` is queued instead.
pub fn spawn(ctx: egui::Context, candidates: Vec<PathBuf>) -> (Sender<Cmd>, Receiver<Msg>) {
    let (cmd_tx, cmd_rx) = channel();
    let (msg_tx, msg_rx) = channel();
    let worker_tx = msg_tx.clone();
    let spawned = thread::Builder::new()
        .name("pdf-worker".into())
        .spawn(move || run(&ctx, &candidates, &cmd_rx, &worker_tx));
    if let Err(e) = spawned {
        let _ = msg_tx.send(Msg::LibraryMissing(format!(
            "cannot start render thread: {e}"
        )));
    }
    (cmd_tx, msg_rx)
}

/// A search in progress, advanced one page per loop iteration so commands can pre-empt it.
struct SearchJob {
    query: String,
    generation: u64,
    next_page: usize,
    hits: Vec<Hit>,
}

/// Thread body. `pdfium-render` objects are `!Send` and documents borrow `pdfium`, so all live here.
fn run(ctx: &egui::Context, candidates: &[PathBuf], rx: &Receiver<Cmd>, tx: &Sender<Msg>) {
    let send = |msg| {
        let _ = tx.send(msg);
        ctx.request_repaint();
    };
    let pdfium = match bind_pdfium(candidates) {
        Ok(pdfium) => pdfium,
        Err(e) => {
            warn!("{e}");
            send(Msg::LibraryMissing(e.clone()));
            for cmd in rx {
                if matches!(cmd, Cmd::Open { .. }) {
                    send(Msg::LibraryMissing(e.clone()));
                }
            }
            return;
        }
    };
    let mut doc: Option<PdfDocument<'_>> = None;
    let mut queue = VecDeque::new();
    let mut job: Option<SearchJob> = None;
    loop {
        if queue.is_empty() && job.is_none() {
            let Ok(cmd) = rx.recv() else { return };
            queue.push_back(cmd);
        }
        queue.extend(rx.try_iter());
        compact(&mut queue);
        let next = next_index(&queue);
        if let Some(cmd) = queue.remove(next) {
            match cmd {
                Cmd::Open { path, password } => {
                    job = None;
                    drop(doc.take());
                    let (opened, msg) = open_doc(&pdfium, path, password.as_deref());
                    doc = opened;
                    send(msg);
                }
                Cmd::Render {
                    page,
                    scale_px,
                    zoomed,
                    ..
                } => {
                    if let Some(d) = &doc {
                        match render(d, page, scale_px, zoomed) {
                            Ok(image) => send(Msg::Page {
                                page,
                                scale_px,
                                image,
                            }),
                            Err(e) => {
                                warn!(page, scale_px, "render failed: {e}");
                                send(Msg::RenderFailed { page, scale_px });
                            }
                        }
                    }
                }
                Cmd::Text { page } => {
                    if let Some(chars) = doc.as_ref().map(|d| page_chars(d, page)) {
                        send(Msg::Text { page, chars });
                    }
                }
                Cmd::Search { query, generation } => {
                    if query.is_empty() || doc.is_none() {
                        job = None;
                        send(Msg::SearchDone {
                            generation,
                            hits: Vec::new(),
                        });
                    } else {
                        job = Some(SearchJob {
                            query,
                            generation,
                            next_page: 0,
                            hits: Vec::new(),
                        });
                    }
                }
            }
        } else if let (Some(j), Some(d)) = (job.as_mut(), doc.as_ref()) {
            match search_page(d, j.next_page, &j.query) {
                Ok(hits) => j.hits.extend(hits),
                Err(e) => debug!(page = j.next_page, "search skipped page: {e}"),
            }
            j.next_page += 1;
            if j.next_page >= usize::try_from(d.pages().len()).unwrap_or(0) {
                send(Msg::SearchDone {
                    generation: j.generation,
                    hits: std::mem::take(&mut j.hits),
                });
                job = None;
            }
        } else {
            job = None;
        }
    }
}

/// Index of the command to run next: the first that is not a `Text`, else the newest `Text`.
/// Text loads wait for renders, and the newest is the page on screen now: scrolling past pages
/// must not delay the destination.
fn next_index(queue: &VecDeque<Cmd>) -> usize {
    queue
        .iter()
        .position(|c| !matches!(c, Cmd::Text { .. }))
        .unwrap_or(queue.len().saturating_sub(1))
}

/// Binds the first loadable candidate, then the system library. Bindings are process-global,
/// so this runs once per process.
fn bind_pdfium(candidates: &[PathBuf]) -> Result<Pdfium, String> {
    for path in candidates {
        match Pdfium::bind_to_library(path) {
            Ok(bindings) => return Ok(Pdfium::new(bindings)),
            Err(e) => debug!(path = %path.display(), "cannot bind: {e}"),
        }
    }
    Pdfium::bind_to_system_library()
        .map(Pdfium::new)
        .map_err(|e| {
            debug!("cannot bind system library: {e}");
            let tried: Vec<String> = candidates
                .iter()
                .map(|p| p.display().to_string())
                .chain(["system library".to_owned()])
                .collect();
            format!(
                "{LIB_NAME} not found (tried {}). Run ./install.sh",
                tried.join(", ")
            )
        })
}

/// Opens `path`: the document, if it loaded, and the message telling the UI what happened.
fn open_doc<'a>(
    pdfium: &'a Pdfium,
    path: PathBuf,
    password: Option<&str>,
) -> (Option<PdfDocument<'a>>, Msg) {
    let name = path.file_name().map_or_else(
        || path.display().to_string(),
        |n| n.to_string_lossy().into_owned(),
    );
    match open(pdfium, &path, password) {
        Ok((doc, sizes)) => {
            debug!(name, pages = sizes.len(), "opened");
            (Some(doc), Msg::Opened { name, sizes })
        }
        Err(PdfiumError::PdfiumLibraryInternalError(PdfiumInternalError::PasswordError)) => {
            debug!(name, wrong = password.is_some(), "needs password");
            (
                None,
                Msg::NeedsPassword {
                    path,
                    wrong: password.is_some(),
                },
            )
        }
        Err(e) => {
            let text = open_error_text(&name, e);
            warn!("{text}");
            (None, Msg::OpenFailed(text))
        }
    }
}

/// Loads `path` and its page sizes (no page is loaded).
fn open<'a>(
    pdfium: &'a Pdfium,
    path: &Path,
    password: Option<&str>,
) -> Result<(PdfDocument<'a>, Vec<PageSize>), PdfiumError> {
    let doc = pdfium.load_pdf_from_file(path, password)?;
    let sizes = doc
        .pages()
        .page_sizes()?
        .iter()
        .map(|r| PageSize {
            w: r.width().value,
            h: r.height().value,
        })
        .collect();
    Ok((doc, sizes))
}

/// One-line message for a failed open. `PdfiumError`'s Display is a multi-line `{:#?}` dump.
fn open_error_text(name: &str, e: PdfiumError) -> String {
    match e {
        PdfiumError::IoError(io) => format!("Cannot open {name}: {io}"),
        PdfiumError::PdfiumLibraryInternalError(PdfiumInternalError::FormatError) => {
            format!("Cannot open {name}: not a PDF, or damaged")
        }
        e => format!("Cannot open {name}: {e:?}"),
    }
}

/// Pixel buffers of zoomed-in renders get capacity above glibc's dynamic mmap ceiling (32 MiB on
/// 64-bit, mallopt(3)). Unless a free arena chunk is that big, malloc maps them on their own and
/// unmaps them on free without raising its mmap threshold. Otherwise freed zoom buffers stayed in
/// the worker's arena for the rest of the session (~50 MB). Fit-width and zoomed-out buffers stay
/// on the heap at any size, where reuse saves page faults.
const GLIBC_MMAP_CEILING_PX: usize = (32 << 20) / 4 + 1;
fn pixel_capacity(n: usize, zoomed: bool) -> usize {
    if zoomed {
        n.max(GLIBC_MMAP_CEILING_PX)
    } else {
        n
    }
}

/// Renders one page at `scale_px` px per pt straight into the image egui uploads.
fn render(
    doc: &PdfDocument<'_>,
    page: usize,
    scale_px: f32,
    zoomed: bool,
) -> Result<ColorImage, Box<dyn Error>> {
    let started = Instant::now();
    let pdf_page = doc.pages().get(i32::try_from(page)?)?;
    // The size `scale_page_by_factor` renders at (pdfium-render `apply_to_page`).
    #[expect(
        clippy::cast_possible_truncation,
        reason = "page pixel sizes are far below i32::MAX"
    )]
    let px = |pt: f32| (pt * scale_px).round() as i32;
    let (w, h) = (px(pdf_page.width().value), px(pdf_page.height().value));
    let size = [usize::try_from(w)?, usize::try_from(h)?];
    let n = size[0] * size[1];
    let mut pixels = Vec::with_capacity(pixel_capacity(n, zoomed));
    // Opaque white, as pdfium would clear it; pages are drawn over it, so alpha stays 255 and
    // pdfium's RGBA output (pdfium-render's default byte order) is already a premultiplied Color32.
    pixels.resize(n, Color32::WHITE);
    let mut image = ColorImage::new(size, pixels);
    let config = PdfRenderConfig::new()
        .scale_page_by_factor(scale_px)
        .clear_before_rendering(false);
    let mut bitmap = PdfBitmap::from_bytes(w, h, PdfBitmapFormat::BGRA, image.as_raw_mut())?;
    pdf_page.render_into_bitmap_with_config(&mut bitmap, &config)?;
    drop(bitmap);
    debug!(
        page,
        scale_px,
        ms = started.elapsed().as_secs_f64() * 1e3,
        "rendered"
    );
    Ok(image)
}

/// Case-insensitive substring matches on one page, one `Hit` per match.
fn search_page(
    doc: &PdfDocument<'_>,
    page: usize,
    query: &str,
) -> Result<Vec<Hit>, Box<dyn Error>> {
    let pdf_page = doc.pages().get(i32::try_from(page)?)?;
    let frame = PageFrame::of(&pdf_page)?;
    let text = pdf_page.text()?;
    let matches = text.search(query, &PdfSearchOptions::new())?;
    let hits = matches
        .iter(PdfSearchDirection::SearchForward)
        .map(|segments| Hit {
            page,
            rects: segments
                .iter()
                .map(|s| frame.to_page(&s.bounds()))
                .collect(),
        })
        .collect();
    Ok(hits)
}

/// Chars of one page with their loose boxes, in page coordinates like search hits; empty (logged)
/// if the page text cannot be loaded. Chars without a unicode value (pdfium reports 0) or a box
/// are skipped.
fn page_chars(doc: &PdfDocument<'_>, page: usize) -> Vec<CharBox> {
    let load = || -> Result<Vec<CharBox>, Box<dyn Error>> {
        let pdf_page = doc.pages().get(i32::try_from(page)?)?;
        let frame = PageFrame::of(&pdf_page)?;
        let text = pdf_page.text()?;
        let chars = text
            .chars()
            .iter()
            .filter_map(|c| {
                let ch = c.unicode_char().filter(|&ch| ch != '\0')?;
                let b = c.loose_bounds().ok()?;
                Some(CharBox {
                    rect: frame.to_page(&b),
                    ch,
                })
            })
            .collect();
        Ok(chars)
    };
    match load() {
        Ok(chars) => {
            debug!(page, chars = chars.len(), "text");
            chars
        }
        Err(e) => {
            warn!(page, "text extraction failed: {e}");
            Vec::new()
        }
    }
}

/// Keeps only the commands still worth running; order is preserved.
/// 1. Drops everything before the last `Open`. 2. Keeps only the last `Search`.
/// 3. Keeps only the newest `Render` generation, deduped by `(page, scale_px)`. Keeps every `Text`.
fn compact(queue: &mut VecDeque<Cmd>) {
    if let Some(last_open) = queue.iter().rposition(|c| matches!(c, Cmd::Open { .. })) {
        queue.drain(..last_open);
    }
    let last_search = queue.iter().rposition(|c| matches!(c, Cmd::Search { .. }));
    let newest = queue
        .iter()
        .filter_map(|c| match c {
            Cmd::Render { generation, .. } => Some(*generation),
            _ => None,
        })
        .max();
    let mut seen = HashSet::new();
    *queue = std::mem::take(queue)
        .into_iter()
        .enumerate()
        .filter(|(i, c)| match c {
            Cmd::Open { .. } | Cmd::Text { .. } => true,
            Cmd::Search { .. } => Some(*i) == last_search,
            Cmd::Render {
                page,
                scale_px,
                generation,
                ..
            } => Some(*generation) == newest && seen.insert((*page, scale_px.to_bits())),
        })
        .map(|(_, c)| c)
        .collect();
}

/// How a page's user space maps onto the page as displayed: pdfium text boxes are in user space
/// (bottom-left origin, y up, the visible box possibly away from the origin, `/Rotate` not
/// applied), while the rendered page is the visible box, rotated.
#[derive(Debug, Clone, Copy)]
struct PageFrame {
    /// Visible box in user space.
    bbox: PdfRect,
    rotation: Rotation,
}

impl PageFrame {
    fn of(page: &PdfPage<'_>) -> Result<Self, PdfiumError> {
        let bbox = page.boundaries().bounding()?.bounds;
        #[expect(
            clippy::cast_possible_truncation,
            reason = "degrees are multiples of 90"
        )]
        let turns = (page.rotation()?.as_degrees() / 90.0).round() as i32;
        Ok(Self {
            bbox,
            rotation: Rotation::from_quarter_turns(turns),
        })
    }

    /// A user-space rect to page points, top-left origin, `/Rotate` applied.
    fn to_page(self, r: &PdfRect) -> Rect {
        let (left, top) = (self.bbox.left().value, self.bbox.top().value);
        let unrotated = Rect::from_min_max(
            pos2(r.left().value - left, top - r.top().value),
            pos2(r.right().value - left, top - r.bottom().value),
        );
        let size = PageSize {
            w: self.bbox.width().value,
            h: self.bbox.height().value,
        };
        self.rotation.rect(unrotated, size)
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;
    use crate::render::pdfium_loader::default_candidates;

    fn render(page: usize, scale_px: f32, generation: u64) -> Cmd {
        Cmd::Render {
            page,
            scale_px,
            generation,
            zoomed: false,
        }
    }

    fn search(query: &str, generation: u64) -> Cmd {
        Cmd::Search {
            query: query.to_owned(),
            generation,
        }
    }

    fn compacted(cmds: Vec<Cmd>) -> Vec<Cmd> {
        let mut queue = VecDeque::from(cmds);
        compact(&mut queue);
        queue.into()
    }

    fn open(path: &str) -> Cmd {
        Cmd::Open {
            path: path.into(),
            password: None,
        }
    }

    #[test]
    fn compact_keeps_last_open_and_after() {
        let (a, b) = (open("a"), open("b"));
        let got = compacted(vec![
            render(1, 1.0, 1),
            a,
            render(2, 1.0, 1),
            b.clone(),
            search("x", 2),
        ]);
        assert_eq!(got, [b, search("x", 2)]);
    }

    #[test]
    fn compact_keeps_newest_render_generation() {
        let got = compacted(vec![
            render(1, 1.0, 1),
            render(2, 1.0, 2),
            render(3, 1.0, 2),
        ]);
        assert_eq!(got, [render(2, 1.0, 2), render(3, 1.0, 2)]);
    }

    #[test]
    fn compact_dedupes_same_key() {
        let got = compacted(vec![
            render(1, 1.0, 2),
            render(1, 1.0, 2),
            render(1, 2.0, 2),
        ]);
        assert_eq!(got, [render(1, 1.0, 2), render(1, 2.0, 2)]);
    }

    #[test]
    fn compact_keeps_only_last_search() {
        let got = compacted(vec![search("a", 1), render(0, 1.0, 1), search("ab", 2)]);
        assert_eq!(got, [render(0, 1.0, 1), search("ab", 2)]);
    }

    #[test]
    fn compact_keeps_text_after_last_open() {
        let a = open("a");
        let got = compacted(vec![
            Cmd::Text { page: 1 },
            a.clone(),
            Cmd::Text { page: 2 },
            render(0, 1.0, 1),
            Cmd::Text { page: 3 },
        ]);
        assert_eq!(
            got,
            [
                a,
                Cmd::Text { page: 2 },
                render(0, 1.0, 1),
                Cmd::Text { page: 3 }
            ]
        );
    }

    #[test]
    fn next_index_runs_renders_first_then_newest_text() {
        let text = |page| Cmd::Text { page };
        assert_eq!(next_index(&VecDeque::from([text(1), text(2), text(3)])), 2);
        let mixed = VecDeque::from([text(1), render(0, 1.0, 1), text(2)]);
        assert_eq!(next_index(&mixed), 1);
        assert_eq!(next_index(&VecDeque::new()), 0);
        assert_eq!(next_index(&VecDeque::from([render(0, 1.0, 1), text(1)])), 0);
    }

    #[test]
    fn pixel_capacity_maps_only_zoomed_buffers() {
        // Letter at fit width in a maximized 1080p window (4 px per pt): 31 MB, still on the heap.
        let fit = 2448 * 3168;
        assert_eq!(pixel_capacity(fit, false), fit);
        assert!(pixel_capacity(fit, true) * 4 > 32 << 20);
        assert!(pixel_capacity(1, true) * 4 > 32 << 20);
        let huge = GLIBC_MMAP_CEILING_PX * 2;
        assert_eq!(pixel_capacity(huge, true), huge);
    }

    #[test]
    fn frame_flips_to_top_left_origin() {
        let frame = PageFrame {
            bbox: PdfRect::new_from_values(0.0, 0.0, 792.0, 612.0),
            rotation: Rotation::R0,
        };
        let want = Rect::from_min_max(pos2(72.0, 52.0), pos2(140.0, 77.0));
        let r = PdfRect::new_from_values(715.0, 72.0, 740.0, 140.0);
        assert_eq!(frame.to_page(&r), want);
    }

    #[test]
    fn frame_applies_box_origin_and_rotate() {
        // MediaBox [20 10 632 802], /Rotate 90: the displayed page is 792 wide, 612 tall.
        let frame = PageFrame {
            bbox: PdfRect::new_from_values(10.0, 20.0, 802.0, 632.0),
            rotation: Rotation::R90,
        };
        // Unrotated top-left (72, 52)-(140, 77) turns clockwise to the top-right.
        let want = Rect::from_min_max(pos2(715.0, 72.0), pos2(740.0, 140.0));
        let r = PdfRect::new_from_values(725.0, 92.0, 750.0, 160.0);
        assert_eq!(frame.to_page(&r), want);
    }

    /// Minimal valid PDF: one 612×792 page per entry, each showing its text in Helvetica 24 and a
    /// red 20 pt square 10 pt from the bottom-left corner.
    /// `rotate` and `origin` set every page's `/Rotate` and `MediaBox` origin.
    fn tiny_pdf(pages: &[&str], rotate: u32, origin: (f32, f32)) -> Vec<u8> {
        let n = pages.len();
        let kids: Vec<String> = (0..n).map(|i| format!("{} 0 R", 4 + 2 * i)).collect();
        let mut objs: Vec<String> = vec![
            "<< /Type /Catalog /Pages 2 0 R >>".into(),
            format!("<< /Type /Pages /Kids [{}] /Count {n} >>", kids.join(" ")),
            "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".into(),
        ];
        let (ox, oy) = origin;
        for (i, text) in pages.iter().enumerate() {
            let content = format!(
                "q 1 0 0 rg {} {} 20 20 re f Q BT /F1 24 Tf {} {} Td ({text}) Tj ET",
                10.0 + ox,
                10.0 + oy,
                72.0 + ox,
                720.0 + oy
            );
            objs.push(format!(
                "<< /Type /Page /Parent 2 0 R /MediaBox [{ox} {oy} {} {}] /Rotate {rotate} \
                 /Resources << /Font << /F1 3 0 R >> >> /Contents {} 0 R >>",
                612.0 + ox,
                792.0 + oy,
                5 + 2 * i
            ));
            objs.push(format!(
                "<< /Length {} >>\nstream\n{content}\nendstream",
                content.len()
            ));
        }
        let mut out = b"%PDF-1.4\n".to_vec();
        let mut offsets = Vec::with_capacity(objs.len());
        for (i, body) in objs.iter().enumerate() {
            offsets.push(out.len());
            out.extend_from_slice(format!("{} 0 obj\n{body}\nendobj\n", i + 1).as_bytes());
        }
        let xref = out.len();
        out.extend_from_slice(
            format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes(),
        );
        for off in offsets {
            out.extend_from_slice(format!("{off:010} 00000 n \n").as_bytes());
        }
        out.extend_from_slice(
            format!(
                "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
                objs.len() + 1
            )
            .as_bytes(),
        );
        out
    }

    /// The only test that binds libpdfium (a second bind in one process fails).
    /// Skips when no library is found; run with `PAPERLIGHT_PDFIUM=<path to libpdfium.so>`.
    #[test]
    fn worker_end_to_end() -> Result<(), String> {
        const T: Duration = Duration::from_secs(10);
        let dir = std::env::temp_dir();
        let path = dir.join(format!("paperlight-test-{}.pdf", std::process::id()));
        std::fs::write(&path, tiny_pdf(&["Hello", "World"], 0, (0.0, 0.0)))
            .map_err(|e| e.to_string())?;
        let rotated = dir.join(format!("paperlight-test-{}-rot.pdf", std::process::id()));
        std::fs::write(&rotated, tiny_pdf(&["Hello"], 90, (20.0, 10.0)))
            .map_err(|e| e.to_string())?;
        let (tx, rx) = spawn(egui::Context::default(), default_candidates());
        let send = |cmd| tx.send(cmd).map_err(|e| e.to_string());
        let recv = || rx.recv_timeout(T).map_err(|e| e.to_string());

        send(open(path.to_str().ok_or("path")?))?;
        match recv()? {
            Msg::Opened { name, sizes } => {
                assert_eq!(
                    Some(name.as_str()),
                    path.file_name().and_then(|n| n.to_str())
                );
                assert_eq!(sizes, [PageSize { w: 612.0, h: 792.0 }; 2]);
            }
            Msg::LibraryMissing(e) => {
                eprintln!("skipping worker_end_to_end: {e}");
                return Ok(());
            }
            _ => return Err("expected Opened".into()),
        }

        send(render(0, 0.5, 1))?;
        let Msg::Page { page: 0, image, .. } = recv()? else {
            return Err("expected Page 0".into());
        };
        assert_eq!(image.size, [306, 396]);
        // Opaque white page with ink: the render path's pixel format (RGBA, alpha 255).
        assert!(image.pixels.iter().all(|p| p.a() == 255));
        assert_eq!(image.pixels[0], Color32::WHITE);
        assert!(image.pixels.iter().any(|p| p.r() < 128));
        // The red square's centre (5..15 px across, 381..391 down): catches swapped R and B.
        assert_eq!(image.pixels[386 * 306 + 10], Color32::RED);

        send(search("world", 1))?;
        let Msg::SearchDone {
            generation: 1,
            hits,
        } = recv()?
        else {
            return Err("expected SearchDone 1".into());
        };
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].page, 1);
        let top = hits[0]
            .rects
            .first()
            .map(|r| r.min.y)
            .ok_or("hit without rects")?;
        assert!((40.0..72.0).contains(&top), "hit top {top}");

        send(Cmd::Text { page: 0 })?;
        let Msg::Text { page: 0, chars } = recv()? else {
            return Err("expected Text 0".into());
        };
        assert_eq!(chars.iter().map(|c| c.ch).collect::<String>(), "Hello");
        let r = chars[0].rect;
        assert!(
            (40.0..72.0).contains(&r.min.y) && r.max.y > r.min.y,
            "{r:?}"
        );

        // /Rotate 90 with an offset MediaBox: the page is landscape and the text, drawn at the
        // unrotated top-left, lands at the top-right, near the right edge.
        send(open(rotated.to_str().ok_or("path")?))?;
        let Msg::Opened { sizes, .. } = recv()? else {
            return Err("expected Opened (rotated)".into());
        };
        assert_eq!(sizes, [PageSize { w: 792.0, h: 612.0 }]);
        send(Cmd::Text { page: 0 })?;
        let Msg::Text { page: 0, chars } = recv()? else {
            return Err("expected Text 0 (rotated)".into());
        };
        let r = chars[0].rect;
        assert!(
            (700.0..752.0).contains(&r.min.x) && (60.0..90.0).contains(&r.min.y),
            "{r:?}"
        );

        send(open("/nonexistent.pdf"))?;
        let Msg::OpenFailed(text) = recv()? else {
            return Err("expected OpenFailed".into());
        };
        // Regression: PdfiumError's Display is a multi-line `{:#?}` dump.
        assert_eq!(
            text,
            "Cannot open nonexistent.pdf: No such file or directory (os error 2)"
        );
        std::fs::remove_file(&rotated).map_err(|e| e.to_string())?;
        std::fs::remove_file(&path).map_err(|e| e.to_string())
    }
}
