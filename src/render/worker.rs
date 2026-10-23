//! The `pdf-worker` thread: owns the PDF library, opens, renders and searches documents.

use std::{
    collections::{HashSet, VecDeque},
    error::Error,
    path::{Path, PathBuf},
    thread,
    time::Instant,
};

use crossbeam_channel::{Receiver, Sender, unbounded};
use egui::{ColorImage, Rect, pos2};
use pdfium_render::prelude::{
    PdfDocument, PdfRenderConfig, PdfSearchDirection, PdfSearchOptions, Pdfium, PdfiumError,
    PdfiumInternalError,
};
use tracing::{debug, warn};

use super::pdfium_loader::LIB_NAME;
use crate::{layout::PageSize, search::Hit, selection::CharBox};

/// UI → worker.
#[derive(Debug, Clone, PartialEq)]
pub enum Cmd {
    /// Replace the open document.
    Open(PathBuf),
    /// Render `page` at `scale_px` px per pt (ppp already included). Older `generation`s are dropped.
    Render {
        page: usize,
        scale_px: f32,
        generation: u64,
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
    /// No libpdfium could be bound. Sent at startup and again for every later `Open`.
    LibraryMissing(String),
    Page {
        page: usize,
        scale_px: f32,
        image: ColorImage,
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
    let (cmd_tx, cmd_rx) = unbounded();
    let (msg_tx, msg_rx) = unbounded();
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
                if matches!(cmd, Cmd::Open(_)) {
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
        if let Some(cmd) = queue.pop_front() {
            match cmd {
                Cmd::Open(path) => {
                    job = None;
                    doc = None;
                    let name = path.file_name().map_or_else(
                        || path.display().to_string(),
                        |n| n.to_string_lossy().into_owned(),
                    );
                    match open(&pdfium, &path) {
                        Ok((d, sizes)) => {
                            debug!(name, pages = sizes.len(), "opened");
                            doc = Some(d);
                            send(Msg::Opened { name, sizes });
                        }
                        Err(e) => {
                            let text = open_error_text(&name, e);
                            warn!("{text}");
                            send(Msg::OpenFailed(text));
                        }
                    }
                }
                Cmd::Render { page, scale_px, .. } => {
                    if let Some(d) = &doc {
                        match render(d, page, scale_px) {
                            Ok(image) => send(Msg::Page {
                                page,
                                scale_px,
                                image,
                            }),
                            Err(e) => warn!(page, scale_px, "render failed: {e}"),
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

/// Loads `path` and its page sizes (no page is loaded).
fn open<'a>(
    pdfium: &'a Pdfium,
    path: &Path,
) -> Result<(PdfDocument<'a>, Vec<PageSize>), PdfiumError> {
    let doc = pdfium.load_pdf_from_file(path, None)?;
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
        PdfiumError::PdfiumLibraryInternalError(PdfiumInternalError::PasswordError) => {
            format!("{name} is password-protected (not supported)")
        }
        PdfiumError::IoError(io) => format!("Cannot open {name}: {io}"),
        PdfiumError::PdfiumLibraryInternalError(PdfiumInternalError::FormatError) => {
            format!("Cannot open {name}: not a PDF, or damaged")
        }
        e => format!("Cannot open {name}: {e:?}"),
    }
}

/// Renders one page to an RGBA image at `scale_px` px per pt.
fn render(doc: &PdfDocument<'_>, page: usize, scale_px: f32) -> Result<ColorImage, Box<dyn Error>> {
    let started = Instant::now();
    let pdf_page = doc.pages().get(i32::try_from(page)?)?;
    let bitmap =
        pdf_page.render_with_config(&PdfRenderConfig::new().scale_page_by_factor(scale_px))?;
    let size = [
        usize::try_from(bitmap.width())?,
        usize::try_from(bitmap.height())?,
    ];
    let image = ColorImage::from_rgba_unmultiplied(size, &bitmap.as_rgba_bytes());
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
    let page_h = pdf_page.height().value;
    let text = pdf_page.text()?;
    let matches = text.search(query, &PdfSearchOptions::new())?;
    let hits = matches
        .iter(PdfSearchDirection::SearchForward)
        .map(|segments| Hit {
            page,
            rects: segments
                .iter()
                .map(|s| {
                    let b = s.bounds();
                    flip(
                        b.left().value,
                        b.bottom().value,
                        b.right().value,
                        b.top().value,
                        page_h,
                    )
                })
                .collect(),
        })
        .collect();
    Ok(hits)
}

/// Chars of one page with their loose boxes, flipped like search hits; empty (logged) if the page
/// text cannot be loaded. Chars without a unicode value (pdfium reports 0) or a box are skipped.
fn page_chars(doc: &PdfDocument<'_>, page: usize) -> Vec<CharBox> {
    let load = || -> Result<Vec<CharBox>, Box<dyn Error>> {
        let pdf_page = doc.pages().get(i32::try_from(page)?)?;
        let page_h = pdf_page.height().value;
        let text = pdf_page.text()?;
        let chars = text
            .chars()
            .iter()
            .filter_map(|c| {
                let ch = c.unicode_char().filter(|&ch| ch != '\0')?;
                let b = c.loose_bounds().ok()?;
                Some(CharBox {
                    rect: flip(
                        b.left().value,
                        b.bottom().value,
                        b.right().value,
                        b.top().value,
                        page_h,
                    ),
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
    if let Some(last_open) = queue.iter().rposition(|c| matches!(c, Cmd::Open(_))) {
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
            Cmd::Open(_) | Cmd::Text { .. } => true,
            Cmd::Search { .. } => Some(*i) == last_search,
            Cmd::Render {
                page,
                scale_px,
                generation,
            } => Some(*generation) == newest && seen.insert((*page, scale_px.to_bits())),
        })
        .map(|(_, c)| c)
        .collect();
}

/// PDF rect (bottom-left origin, y up) to page points with a top-left origin.
fn flip(left: f32, bottom: f32, right: f32, top: f32, page_h: f32) -> Rect {
    Rect::from_min_max(pos2(left, page_h - top), pos2(right, page_h - bottom))
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

    #[test]
    fn compact_keeps_last_open_and_after() {
        let (a, b) = (Cmd::Open("a".into()), Cmd::Open("b".into()));
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
        let a = Cmd::Open("a".into());
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
    fn flip_converts_bottom_left_origin() {
        let want = Rect::from_min_max(pos2(72.0, 52.0), pos2(140.0, 77.0));
        assert_eq!(flip(72.0, 715.0, 140.0, 740.0, 792.0), want);
    }

    /// Minimal valid PDF: one 612×792 page per entry, each showing its text in Helvetica 24.
    fn tiny_pdf(pages: &[&str]) -> Vec<u8> {
        let n = pages.len();
        let kids: Vec<String> = (0..n).map(|i| format!("{} 0 R", 4 + 2 * i)).collect();
        let mut objs: Vec<String> = vec![
            "<< /Type /Catalog /Pages 2 0 R >>".into(),
            format!("<< /Type /Pages /Kids [{}] /Count {n} >>", kids.join(" ")),
            "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".into(),
        ];
        for (i, text) in pages.iter().enumerate() {
            let content = format!("BT /F1 24 Tf 72 720 Td ({text}) Tj ET");
            objs.push(format!(
                "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] \
                 /Resources << /Font << /F1 3 0 R >> >> /Contents {} 0 R >>",
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
        let path = std::env::temp_dir().join(format!("paperlight-test-{}.pdf", std::process::id()));
        std::fs::write(&path, tiny_pdf(&["Hello", "World"])).map_err(|e| e.to_string())?;
        let (tx, rx) = spawn(egui::Context::default(), default_candidates());
        let send = |cmd| tx.send(cmd).map_err(|e| e.to_string());
        let recv = || rx.recv_timeout(T).map_err(|e| e.to_string());

        send(Cmd::Open(path.clone()))?;
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

        send(Cmd::Open("/nonexistent.pdf".into()))?;
        let Msg::OpenFailed(text) = recv()? else {
            return Err("expected OpenFailed".into());
        };
        // Regression: PdfiumError's Display is a multi-line `{:#?}` dump.
        assert_eq!(
            text,
            "Cannot open nonexistent.pdf: No such file or directory (os error 2)"
        );
        std::fs::remove_file(&path).map_err(|e| e.to_string())
    }
}
